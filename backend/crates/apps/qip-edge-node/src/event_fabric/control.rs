//! ADR 0100 §1 and §8: the node's P0 control consumer — grants, halts and
//! policy pushed down over the event fabric's `control.local` stream, taken
//! through the same verify code as the mesh downlink, and the start barrier
//! that keeps the first pass from running before that control is read.
//! SLICE-57.
//!
//! # One verify path, whatever the wire
//!
//! ADR 0100 §8 says control down goes over P0 "through the SAME verify code
//! as the mesh downlink" (FABRIC-007). A fabric consumer that decoded a grant
//! itself, or trusted the broker's CRCs as if they were a signature, would be
//! a second verification path — the route by which an unsigned grant reaches
//! a cell while the mesh path still looks perfectly guarded (FABRIC-081). So
//! nothing here decodes a capital envelope, a policy payload or a halt
//! command. Every record's [`AnyEvent`] is handed, one frame at a time, to a
//! [`CapitalDownlink`] and a [`PolicyDownlink`] through `poll_from` — the
//! `qip_edge::mesh::FrameSource` seam SLICE-04 built so that a frame's wire
//! cannot change the checks it meets — and the only things that come back
//! out are the downlinks' own `VerifiedEnvelope`, `VerifiedPolicy` and
//! `VerifiedHalt`, which have no constructor but `verify`. The capital
//! downlink is one value for the life of the consumer, so its grant memory
//! is what makes a redelivered grant — the broker's at-least-once, or the
//! same grant stamped at a second offset — cross once.
//!
//! # What crosses, and what a refusal leaves behind
//!
//! A verified value crosses into SLICE-33's [`Handoff`](crate::control::Handoff) with its
//! [`ControlPosition`] — stream, partition, offset, event id — and the
//! decision thread applies it at a pass boundary. A refused frame never
//! crosses as a value: the cell keeps the envelope, policy and halt state it
//! had, because nothing reached it. The refusal crosses instead as a
//! [`Refused`] record on a companion channel ([`Refusals`]) the scheduler
//! drains beside the handoff and lists in the pass marker, and it is counted
//! on `qip_edge_event_fabric_control_refused_total{reason}`. A separate
//! channel rather than an arm of the handoff because the handoff's value type
//! admits only verified values (SLICE-33), and that is a property worth more
//! than one channel's fewer moving parts.
//!
//! # The start barrier, and why it is a watermark rather than a silence
//!
//! Red-team major: which pass the fixture grant landed on raced the control
//! fetch against the pass timer, so two runs of one tape could diverge
//! (ADR 0100 §8 tests 2 and 3). The consumer therefore takes the partition's
//! high watermark from the first answer the broker gives it, holds that
//! figure fixed, and fires SLICE-33's [`CaughtUpSignal`] only when it has
//! fetched, judged and handed across every record below it. Not "the first
//! fetch came back", which fires with the rest of a multi-batch backlog
//! unread; and not "a fetch came back empty", which on a stream the centre
//! keeps writing to might never happen. A broker that answers "nothing here"
//! at the very first ask is at its head, and that is caught up too.
//!
//! # Fabric down
//!
//! A fetch that fails has already spent the client's own retry ladder and
//! breaker ([`Consumer::fetch`]). The consumer then waits its idle interval
//! and asks again, indefinitely. Nothing is sent to the decision thread about
//! it — the cell runs on its last valid envelope and policy until they expire
//! (ADR 0008) — and before the barrier has fired it simply does not fire: a
//! consumer that cannot reach or verify the stream leaves the cell unable to
//! start, and how long to wait for that is the scheduler's start deadline
//! (SLICE-55), not a decision made here.
//!
//! # Limits stated rather than hidden
//!
//! - The consume grant is the node's own `reflex:<cell>` on its own key
//!   (SLICE-53); the broker enforces it against the transport's bearer
//!   identity. This consumer is given a [`Consumer`] the composition root
//!   built over whichever `FabricTransport` carries that identity, and never
//!   builds a transport itself. What it enforces client-side is the stream:
//!   a consumer on anything but [`CONTROL_STREAM`] is refused.
//! - The partition is the consumer's; nothing in the tree yet defines the
//!   key-to-partition hash, and inventing one here would be a second answer
//!   the broker need not agree with. A frame for another cell read from a
//!   wrong partition is still refused by `verify`, which checks the cell.
//! - Every start reads the partition from offset zero and nothing commits.
//!   The cell's control state is in memory and rebuilt at start, so a
//!   committed offset would describe a cell that no longer exists; a record
//!   re-read is re-verified at the present instant, so an expired grant is
//!   refused rather than revived, and the grant memory absorbs the rest.

use crate::control::{CaughtUpSignal, Delivery, HandoffSender};
use crate::event_fabric::telemetry::OutboxTelemetry;
use qip_contracts::replay::ControlPosition;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, EventId, Timestamp};
use qip_edge::mesh::{CapitalDownlink, HaltTopic, PolicyDownlink, ScriptedFrames};
use qip_events::AnyEvent;
use qip_transport::event_fabric::consumer::Consumer;
use qip_transport::retry::Sleeper;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

/// The P0 stream a cell's control arrives on, as the committed catalogue
/// declares it (SLICE-53). The only stream this consumer will read: the
/// node's consume grant names this stream and no other.
pub const CONTROL_STREAM: &str = "control.local";

/// The most refusal records the companion channel holds. Past this they are
/// counted as unlisted rather than waited on — see [`RefusalSender`].
pub const MAX_REFUSAL_CAPACITY: usize = 4096;

/// Why a control record was refused, as the `reason` label of
/// `qip_edge_event_fabric_control_refused_total`. An enum so the label set
/// is bounded by the type and never by a refusal's own text, which carries
/// strings off the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusalReason {
    /// A capital grant the capital downlink did not verify.
    Grant,
    /// A policy payload the policy downlink did not verify.
    Policy,
    /// A halt command the policy downlink did not verify.
    Halt,
    /// A record on the control stream whose topic is none of the three.
    NotControl,
    /// A record whose payload does not decode as an event at all.
    Undecodable,
}

impl RefusalReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Grant => "grant_unverified",
            Self::Policy => "policy_unverified",
            Self::Halt => "halt_unverified",
            Self::NotControl => "not_control",
            Self::Undecodable => "undecodable",
        }
    }
}

/// A control record that was refused, and where it was read. What the pass
/// marker lists in place of a value: the position the stream was read
/// through is part of the record whether or not the record changed the cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub reason: RefusalReason,
    /// The downlink's own reason, verbatim.
    pub detail: String,
    pub position: ControlPosition,
}

/// The producing half of the refusal channel, held by the control consumer.
///
/// Never blocks. The consumer's thread is the thread a halt arrives on, and a
/// refusal record the scheduler has not drained must not stand in front of
/// it; a full channel instead counts the record as unlisted, and
/// [`Refusals::drain`] reports how many, so a pass marker says it is short
/// rather than silently being short. The metric counts every refusal either
/// way.
#[derive(Clone, Debug)]
pub struct RefusalSender {
    sender: SyncSender<Refused>,
    unlisted: Arc<AtomicU64>,
}

impl RefusalSender {
    /// `false` when the record was counted as unlisted rather than queued.
    fn offer(&self, refused: Refused) -> bool {
        match self.sender.try_send(refused) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                // Checked rather than wrapping: a wrap would read as a
                // marker that lost nothing.
                let _ = self
                    .unlisted
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        Some(count.saturating_add(1))
                    });
                false
            }
        }
    }
}

/// What one drain of the refusal channel produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefusalDrain {
    /// Every refusal queued since the last drain, in the order read.
    pub refused: Vec<Refused>,
    /// Refusals counted but not queued since the last drain because the
    /// channel was full.
    pub unlisted: u64,
}

/// The consuming half, drained by the scheduler at a pass boundary beside
/// the handoff.
#[derive(Debug)]
pub struct Refusals {
    receiver: Receiver<Refused>,
    unlisted: Arc<AtomicU64>,
    capacity: usize,
}

impl Refusals {
    /// A refusal channel holding at most `capacity` records. Refuses zero —
    /// a channel that holds nothing lists nothing, which reads as a stream
    /// with no refusals — and anything past [`MAX_REFUSAL_CAPACITY`].
    pub fn bounded(capacity: usize) -> Result<(RefusalSender, Self)> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a refusal channel of capacity zero lists no refusal, and a pass marker would \
                 read as a stream nothing was refused on; give it room",
            ));
        }
        if capacity > MAX_REFUSAL_CAPACITY {
            return Err(Error::invalid(format!(
                "a refusal channel of capacity {capacity} is past the {MAX_REFUSAL_CAPACITY} \
                 this node holds"
            )));
        }
        let (sender, receiver) = sync_channel(capacity);
        let unlisted = Arc::new(AtomicU64::new(0));
        Ok((
            RefusalSender {
                sender,
                unlisted: Arc::clone(&unlisted),
            },
            Self {
                receiver,
                unlisted,
                capacity,
            },
        ))
    }

    /// Take what has been refused since the last drain: at most the
    /// channel's capacity, so a flood cannot hold the decision thread here.
    pub fn drain(&mut self) -> RefusalDrain {
        let mut refused = Vec::new();
        while refused.len() < self.capacity {
            match self.receiver.try_recv() {
                Ok(record) => refused.push(record),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        RefusalDrain {
            refused,
            unlisted: self.unlisted.swap(0, Ordering::AcqRel),
        }
    }
}

/// What the consumer is built from, beside its [`Consumer`] and channels.
#[derive(Clone)]
pub struct ControlConfig {
    /// The cell every verified value must name.
    pub cell: String,
    /// The key grants, payloads and halts are verified against — the same
    /// key the mesh downlinks hold.
    pub key: Vec<u8>,
    /// How many applied grants the capital downlink remembers.
    pub grant_memory: usize,
    /// The clock verification is judged at: the tape's clock in the slice.
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ControlConfig {
    /// The key is not printed: a debug line is a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlConfig")
            .field("cell", &self.cell)
            .field("grant_memory", &self.grant_memory)
            .finish_non_exhaustive()
    }
}

/// Counters the consumer keeps about itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlStats {
    /// Fetches that returned an answer, empty or not.
    pub fetches: u64,
    /// Fetches that failed after the client's own retries.
    pub fetch_failures: u64,
    /// Records judged.
    pub frames: u64,
    /// Verified values handed across.
    pub delivered: u64,
    /// Grants the downlink's memory recognised as already applied.
    pub duplicates: u64,
    /// Records refused.
    pub refused: u64,
    /// Refusals counted but not listed because the channel was full.
    pub refusals_unlisted: u64,
    /// The high watermark taken at start, once the broker has answered.
    pub start_watermark: Option<u64>,
    /// The barrier has been fired.
    pub caught_up: bool,
    /// The barrier was fired after its waiting half had gone, so nothing
    /// heard it — a scheduler that gave up at its start deadline.
    pub barrier_unheard: bool,
}

/// What one [`ControlConsumer::step`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepReport {
    /// A fetch was made and answered.
    pub fetched: bool,
    /// The fetch failed; the reason, for a health surface.
    pub fabric_error: Option<String>,
    pub delivered: usize,
    pub duplicates: usize,
    pub refused: usize,
    /// Records fetched but recorded after the present instant, waiting.
    pub held: usize,
    /// This step fired the start barrier.
    pub fired: bool,
}

impl StepReport {
    /// Whether the step moved anything; a loop sleeps when it did not.
    fn progressed(&self) -> bool {
        self.fetched && self.fabric_error.is_none() && self.held == 0
    }
}

/// A frame read at a position and not yet judged.
#[derive(Debug)]
struct Pending {
    position: ControlPosition,
    frame: std::result::Result<AnyEvent, String>,
}

/// The P0 control consumer. See the module documentation.
pub struct ControlConsumer {
    consumer: Consumer,
    capital: CapitalDownlink,
    policy: PolicyDownlink,
    handoff: HandoffSender,
    refusals: RefusalSender,
    telemetry: Arc<OutboxTelemetry>,
    clock: Arc<dyn Clock>,
    signal: Option<CaughtUpSignal>,
    /// Fetched and not yet judged, in offset order. Filled only when empty,
    /// so it never holds more than one fetch's credit.
    held: VecDeque<Pending>,
    stats: ControlStats,
}

impl std::fmt::Debug for ControlConsumer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlConsumer")
            .field("consumer", &self.consumer)
            .field("cell", &self.capital.cell())
            .field("held", &self.held.len())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl ControlConsumer {
    /// A consumer reading `consumer`'s partition of [`CONTROL_STREAM`] from
    /// its start, verifying through the two downlinks, handing values to
    /// `handoff` and refusals to `refusals`, and firing `signal` once caught
    /// up with the watermark it observes first.
    ///
    /// Refuses a consumer on another stream, and — through the downlinks'
    /// own constructors, so the refusal is one piece of code for every
    /// source — an empty key or a zero grant memory.
    pub fn new(
        config: ControlConfig,
        mut consumer: Consumer,
        handoff: HandoffSender,
        refusals: RefusalSender,
        signal: CaughtUpSignal,
        telemetry: Arc<OutboxTelemetry>,
    ) -> Result<Self> {
        if consumer.stream() != CONTROL_STREAM {
            return Err(Error::denied(format!(
                "the control consumer reads only {CONTROL_STREAM}, under the node's own consume \
                 grant; it was given a consumer on '{}'",
                consumer.stream()
            )));
        }
        // The downlinks' own sources are never polled: every frame goes in
        // through `poll_from`, one at a time, so each value keeps its
        // position. An empty script is the honest placeholder for "no
        // source of its own".
        let capital = CapitalDownlink::from_source(
            config.cell.clone(),
            &config.key,
            config.grant_memory,
            Box::new(ScriptedFrames::new(1)?),
        )?;
        let policy = PolicyDownlink::from_source(
            config.cell,
            &config.key,
            Box::new(ScriptedFrames::new(1)?),
        )?;
        consumer.seek(0);
        Ok(Self {
            consumer,
            capital,
            policy,
            handoff,
            refusals,
            telemetry,
            clock: config.clock,
            signal: Some(signal),
            held: VecDeque::new(),
            stats: ControlStats::default(),
        })
    }

    pub const fn stats(&self) -> ControlStats {
        self.stats
    }

    /// One turn: judge what is held and knowable, fetch when nothing is
    /// held, and fire the barrier if the start watermark has been read
    /// through.
    ///
    /// A fabric failure is reported in the [`StepReport`], never as `Err`:
    /// the only `Err` is a closed handoff, which means the decision thread is
    /// gone and there is nothing left to consume for.
    pub fn step(&mut self) -> Result<StepReport> {
        let mut report = StepReport::default();
        let now = self.clock.now();
        self.judge_held(now, &mut report)?;

        if self.held.is_empty() {
            match self.consumer.fetch() {
                Ok(Some(fetched)) => {
                    report.fetched = true;
                    self.stats.fetches = self.stats.fetches.saturating_add(1);
                    // The first answer's watermark is the start watermark,
                    // and it does not move after: a stream the centre keeps
                    // writing to must not keep the barrier down forever.
                    if self.stats.start_watermark.is_none() {
                        self.stats.start_watermark = Some(fetched.high_watermark);
                    }
                    let batch = fetched.batch;
                    for (index, record) in batch.records.iter().enumerate() {
                        let offset = u64::try_from(index)
                            .ok()
                            .and_then(|index| batch.base_offset.checked_add(index))
                            .ok_or_else(|| {
                                Error::invalid(format!(
                                    "a record's offset past base offset {} does not fit a u64",
                                    batch.base_offset
                                ))
                            });
                        let offset = match offset {
                            Ok(offset) => offset,
                            Err(error) => {
                                report.fabric_error = Some(error.message().to_string());
                                self.stats.fetch_failures =
                                    self.stats.fetch_failures.saturating_add(1);
                                return Ok(report);
                            }
                        };
                        let frame = record
                            .decode_payload(batch.encoding)
                            .map_err(|error| error.message().to_string());
                        let event_id = match &frame {
                            Ok(event) => event.event_id.clone(),
                            Err(_) => EventId::from_string(record.event_id.clone()),
                        };
                        self.held.push_back(Pending {
                            position: ControlPosition::new(
                                self.consumer.stream(),
                                self.consumer.partition(),
                                offset,
                                event_id,
                            ),
                            frame,
                        });
                    }
                    self.judge_held(now, &mut report)?;
                }
                Ok(None) => {
                    report.fetched = true;
                    self.stats.fetches = self.stats.fetches.saturating_add(1);
                    // Nothing at the offset asked for: the broker's head.
                    // Asked first, that head is the start watermark.
                    if self.stats.start_watermark.is_none() {
                        self.stats.start_watermark = Some(self.consumer.next_offset());
                    }
                }
                Err(error) => {
                    self.stats.fetch_failures = self.stats.fetch_failures.saturating_add(1);
                    report.fabric_error = Some(error.message().to_string());
                }
            }
        }

        report.held = self.held.len();
        if let Some(watermark) = self.stats.start_watermark
            && self.held.is_empty()
            && self.consumer.next_offset() >= watermark
            && let Some(signal) = self.signal.take()
        {
            // One-shot either way; whether anybody heard it is kept, because
            // a scheduler that stopped waiting started on the deadline, and
            // that is a different start from one on caught-up control.
            self.stats.barrier_unheard = !signal.fire();
            self.stats.caught_up = true;
            report.fired = true;
        }
        Ok(report)
    }

    /// Judge held frames in order, stopping at the first recorded after
    /// `now`: a frame is not judged before it was knowable, and one behind
    /// it waits with it rather than overtaking it — a release must not land
    /// before the halt it follows.
    fn judge_held(&mut self, now: Timestamp, report: &mut StepReport) -> Result<()> {
        while let Some(pending) = self.held.front() {
            if let Ok(frame) = &pending.frame
                && frame.recorded_at > now
            {
                break;
            }
            let Some(Pending { position, frame }) = self.held.pop_front() else {
                break;
            };
            self.stats.frames = self.stats.frames.saturating_add(1);
            match frame {
                Ok(frame) => self.judge(position, &frame, now, report)?,
                Err(detail) => self.refuse(position, RefusalReason::Undecodable, detail, report),
            }
        }
        Ok(())
    }

    /// One frame through both downlinks' own absorb. Each takes only the
    /// frames of its own topics and ignores the rest, so for any frame at
    /// most one of them produces anything; a frame neither takes is not
    /// control and is refused as such.
    fn judge(
        &mut self,
        position: ControlPosition,
        frame: &AnyEvent,
        now: Timestamp,
        report: &mut StepReport,
    ) -> Result<()> {
        let capital = self.capital.poll_from(&mut one_frame(frame)?, now)?;
        let policy = self.policy.poll_from(&mut one_frame(frame)?, now)?;

        let taken = !capital.is_empty() || !policy.is_empty();
        for envelope in capital.verified {
            self.deliver(Delivery::envelope(position.clone(), envelope), report)?;
        }
        for halt in policy.halts {
            self.deliver(Delivery::halt(position.clone(), halt), report)?;
        }
        for payload in policy.verified {
            self.deliver(Delivery::policy(position.clone(), payload), report)?;
        }
        let duplicates = capital.duplicates.len();
        self.stats.duplicates = self
            .stats
            .duplicates
            .saturating_add(u64::try_from(duplicates).unwrap_or(u64::MAX));
        report.duplicates = report.duplicates.saturating_add(duplicates);
        for refused in capital.refused {
            self.refuse(
                position.clone(),
                RefusalReason::Grant,
                refused.reason,
                report,
            );
        }
        let policy_reason = if frame.topic == HaltTopic::TOPIC {
            RefusalReason::Halt
        } else {
            RefusalReason::Policy
        };
        for refused in policy.refused {
            self.refuse(position.clone(), policy_reason, refused.reason, report);
        }
        if !taken {
            self.refuse(
                position,
                RefusalReason::NotControl,
                format!(
                    "the control stream carried a {:?} frame, which is not a grant, a policy \
                     payload or a halt",
                    frame.topic
                ),
                report,
            );
        }
        Ok(())
    }

    fn deliver(&mut self, delivery: Delivery, report: &mut StepReport) -> Result<()> {
        self.handoff.send(delivery)?;
        self.stats.delivered = self.stats.delivered.saturating_add(1);
        report.delivered = report.delivered.saturating_add(1);
        Ok(())
    }

    fn refuse(
        &mut self,
        position: ControlPosition,
        reason: RefusalReason,
        detail: String,
        report: &mut StepReport,
    ) {
        self.telemetry.control_refused(reason.as_str());
        self.stats.refused = self.stats.refused.saturating_add(1);
        report.refused = report.refused.saturating_add(1);
        let listed = self.refusals.offer(Refused {
            reason,
            detail,
            position,
        });
        if !listed {
            self.stats.refusals_unlisted = self.stats.refusals_unlisted.saturating_add(1);
        }
    }

    /// Run the consumer on its own thread until the decision thread's
    /// handoff closes or the returned handle is dropped, sleeping `idle`
    /// whenever a turn moved nothing — a broker at its head, a fabric that is
    /// down, or a frame not yet knowable.
    ///
    /// Refuses a non-positive `idle`: a turn that moved nothing would be
    /// asked again at once, a busy loop against a broker that is down.
    pub fn spawn(self, sleeper: Arc<dyn Sleeper>, idle: Duration) -> Result<ControlThread> {
        if idle.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a control consumer with no idle interval asks a broker that has nothing, or \
                 is down, again at once; name a period",
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let published = Arc::new(Mutex::new(self.stats));
        let thread_stop = Arc::clone(&stop);
        let thread_published = Arc::clone(&published);
        let mut consumer = self;
        std::thread::Builder::new()
            .name(CONTROL_THREAD.to_string())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    let Ok(report) = consumer.step() else {
                        // The handoff closed: nothing will apply another
                        // value. Dropping the consumer here drops an unfired
                        // signal too, which the barrier reads as abandoned.
                        return;
                    };
                    let Ok(mut slot) = thread_published.lock() else {
                        return;
                    };
                    *slot = consumer.stats();
                    drop(slot);
                    if !report.progressed() {
                        sleeper.sleep(idle);
                    }
                }
            })
            .map_err(|error| Error::io(format!("cannot start the control consumer: {error}")))?;
        Ok(ControlThread { stop, published })
    }
}

/// The name the consumer's thread runs under, so a stack dump or a test can
/// tell it from the decision thread.
pub const CONTROL_THREAD: &str = "qip-control-consumer";

/// A single frame, as the source a downlink's `poll_from` takes.
fn one_frame(frame: &AnyEvent) -> Result<ScriptedFrames> {
    let mut source = ScriptedFrames::new(1)?;
    source.push(frame.clone())?;
    Ok(source)
}

/// The handle to a running consumer. Dropping it tells the thread to stop
/// after its current turn; it is not joined, because a turn blocked in a
/// fetch's timeout would make the drop wait with it, and the drop is on the
/// composition root's thread.
#[derive(Debug)]
pub struct ControlThread {
    stop: Arc<AtomicBool>,
    published: Arc<Mutex<ControlStats>>,
}

impl ControlThread {
    /// The counters as of the thread's last turn.
    pub fn stats(&self) -> ControlStats {
        match self.published.lock() {
            Ok(stats) => *stats,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }
}

impl Drop for ControlThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
