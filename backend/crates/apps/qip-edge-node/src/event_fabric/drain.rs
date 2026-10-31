//! ADR 0100 §1: the drain — the thread that reads the spool the writer
//! fsynced and delivers each batch to every configured destination, off the
//! decision thread (§6). SLICE-32.
//!
//! # Why one cursor per sink
//!
//! A batch is delivered to each [`Sink`] independently and each sink's
//! position is committed to the spool's own manifest after it takes the
//! batch. One shared cursor would force a choice on a partial failure: send
//! the batch again to the sink that already took it — a second copy of the
//! same record at the broker or a second lake segment — or skip it for the
//! sink that refused it, which is a hole. Separate cursors make neither
//! necessary: the sink that took it moves on, the one that refused it tries
//! the same batch again on the next step.
//!
//! # What retires a segment, and why that bounds the spool
//!
//! A sealed segment every sink has read past is marked archived and then
//! deleted by `SegmentLog::retain_before`. That is what keeps the spool —
//! and therefore the pressure gauge's reading — bounded while the
//! destinations keep up. A destination that stops leaves its cursor where it
//! was; the segments behind that cursor stay; the spool grows; the gauge
//! narrows and then halts new exposure. The cell is never told that a record
//! is safe while some destination has not taken it.
//!
//! The active segment is sealed on a cadence ([`DrainConfig::new`]'s
//! `seal_every`), because a segment is sealed by size alone otherwise, and a
//! quiet cell's last few batches would sit in an active segment nothing
//! would ever retire.
//!
//! # The destinations
//!
//! - [`FabricSink`]: `qip-fabricd`, through `qip-transport`'s idempotent
//!   producer — one per stream, P1 at the quorum acknowledgement its class
//!   requires and P2 at leader-only (FABRIC-067). Fenced is passed to the
//!   gauge's drain publisher, which halts new exposure: a newer producer
//!   epoch owns the partition and nothing this process spools will be
//!   accepted again.
//! - [`LakeSink`]: the tick lake (`qip_storage::lake`), for the market
//!   events each pass applied. Each event keeps the venue's instant, the
//!   node's capture instant and the feed's own sequence number, because the
//!   lake is where a backtest asks "what did we know then", and a tick
//!   without its sequence cannot show what was missing.
//!
//! A node with no sink configured still runs this thread: segments then
//! retire as soon as they seal, and the spool is the bounded local record
//! the pressure gauge measures rather than an unbounded one.
//!
//! # The drain step of a rollout (R12-REFLEX-9114)
//!
//! Reflex components roll out by drain, fence and epoch activation, not by
//! a pod canary: the old incarnation stops, ships what it holds, and the new
//! one activates under a higher epoch. The epoch half is `standby.rs` (a
//! promotion swaps the lease to `epoch + 1` before granting its fence) and
//! the broker's producer epochs. This is the drain half. A drain spawned
//! with the cell's [`Fence`] ([`Drain::spawn_fenced`]) watches it, and the
//! first time a fence that held stops admitting — its lease lapsed or a
//! higher epoch was read — it seals the spool and drives every sink to what
//! the spool held at that instant ([`Drain::drain_to_tail`]), records how
//! that ended ([`DrainThread::drained_on_fence`]), and stops. The standby's
//! promotion waits `ttl + margin` after the holder's last renewal, and the
//! holder's fence lapses at `ttl`, so the margin is the window this drain
//! runs in before the new incarnation's producer epoch fences it out.

use crate::event_fabric::pressure::DrainPublisher;
use crate::event_fabric::telemetry::OutboxTelemetry;
use crate::event_fabric::writer::{market_event, stream_of};
use qip_contracts::captured_event::CapturedMarketEvent;
use qip_contracts::market_event::MarketEvent;
use qip_core::Clock;
use qip_core::error::{Error, Result};
use qip_edge::fencing::Fence;
use qip_events::event_fabric::bindings;
use qip_events::event_fabric::bindings::MARKET_EVENT_APPLIED;
use qip_events::event_fabric::codec::Batch;
use qip_events::event_fabric::envelope::{FabricContext, FabricEnvelope};
use qip_events::event_fabric::policy::QosClass;
use qip_sequencing::{CaptureStream, DisciplineSettings, Order, Placed, align};
use qip_storage::BlobStore;
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::segment::log::SegmentLog;
use qip_storage::tick_capture::RetentionRight;
use qip_transport::event_fabric::producer::Producer;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The drain thread's name.
pub const DRAIN_THREAD: &str = "qip-spool-drain";

/// The P1 stream the reflex outcomes are produced to (streams.local.json).
pub const OUTCOMES_STREAM: &str = bindings::REFLEX_OUTCOMES_STREAM;

/// The P2 stream the reflex journal, pass markers and tape events are
/// produced to (streams.local.json).
pub const JOURNAL_STREAM: &str = "reflex-journal.local";

/// The most drain steps a drain-out on fence takes before it reports the
/// spool stalled rather than drained: bounded, so a destination that never
/// answers cannot hold the old incarnation open past its margin.
pub const MAX_DRAIN_OUT_STEPS: usize = 64;

/// What a drain-out ([`Drain::drain_to_tail`]) reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrainedTo {
    /// Every sink took everything the spool held when the drain-out began.
    Tail { delivered: usize, steps: usize },
    /// A sink was fenced, or the step bound passed, before every sink took
    /// it all; what is left stays on the spool, named, never reported as
    /// shipped.
    Stalled {
        delivered: usize,
        steps: usize,
        reason: String,
    },
}

/// The manifest key prefix a sink's cursor is committed under.
const CURSOR_PREFIX: &str = "drain.cursor.";

/// One destination the drain delivers spool batches to.
pub trait Sink: Send + std::fmt::Debug {
    /// The sink's name: a source literal, because it is a manifest key and a
    /// metric label, and a runtime string in either would be unbounded.
    fn name(&self) -> &'static str;

    /// Deliver the batch at spool `offset`, bound for `stream`. `Ok` only
    /// once the destination holds it durably by its own definition.
    fn deliver(&mut self, offset: u64, stream: QosClass, batch: &Batch) -> Result<()>;

    /// Whether the destination has told this process it may never write to
    /// it again. Only the fabric can; the default is no.
    fn fenced(&self) -> bool {
        false
    }
}

/// How the drain runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrainConfig {
    poll: Duration,
    seal_every: Duration,
    max_batches_per_step: usize,
}

impl DrainConfig {
    /// Sleep `poll` when a step delivered nothing, seal the active segment
    /// every `seal_every`, and deliver at most `max_batches_per_step` per
    /// sink per step.
    ///
    /// Refuses a zero poll (a spinning thread), a zero seal cadence (a seal
    /// per step, a segment per batch), and a zero batch count (a drain that
    /// delivers nothing and lets the spool fill until the cell halts).
    pub fn new(poll: Duration, seal_every: Duration, max_batches_per_step: usize) -> Result<Self> {
        if poll.is_zero() {
            return Err(Error::invalid(
                "a drain poll of zero spins on an idle spool; configure a bounded wait",
            ));
        }
        if seal_every.is_zero() {
            return Err(Error::invalid(
                "a seal cadence of zero seals a segment per batch; configure a positive period",
            ));
        }
        if max_batches_per_step == 0 {
            return Err(Error::invalid(
                "a drain that takes zero batches per step never delivers, and the spool fills \
                 until the cell halts; configure at least one",
            ));
        }
        Ok(Self {
            poll,
            seal_every,
            max_batches_per_step,
        })
    }
}

/// What one [`Drain::step`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainStep {
    /// Batch deliveries that succeeded, summed over sinks.
    pub delivered: usize,
    /// Sinks that refused a batch this step.
    pub refused: Vec<(&'static str, String)>,
    /// Sealed segments retired from the spool.
    pub retired: usize,
}

#[derive(Debug)]
struct Cursor {
    sink: Box<dyn Sink>,
    next: u64,
}

/// The drain: the spool's reader and retirer.
#[derive(Debug)]
pub struct Drain {
    log: Arc<SegmentLog>,
    cursors: Vec<Cursor>,
    publisher: DrainPublisher,
    telemetry: Arc<OutboxTelemetry>,
    config: DrainConfig,
    last_seal: Instant,
}

impl Drain {
    /// A drain over `log` delivering to `sinks`, each resuming from the
    /// cursor the spool's manifest holds for it.
    ///
    /// Refuses two sinks under one name, which would share a cursor, and a
    /// committed cursor past what the spool holds: that is a manifest from
    /// some other spool, and resuming from it would skip records.
    pub fn open(
        log: Arc<SegmentLog>,
        sinks: Vec<Box<dyn Sink>>,
        publisher: DrainPublisher,
        telemetry: Arc<OutboxTelemetry>,
        config: DrainConfig,
    ) -> Result<Self> {
        let high_water = log.high_water();
        let mut seen = std::collections::BTreeSet::new();
        let mut cursors = Vec::with_capacity(sinks.len());
        for sink in sinks {
            let name = sink.name();
            if !seen.insert(name) {
                return Err(Error::invalid(format!(
                    "two drain sinks are both named {name}; they would share one cursor and \
                     each would skip what the other took"
                )));
            }
            let next = match log.manifest_get(&format!("{CURSOR_PREFIX}{name}"))? {
                None => 0,
                Some(bytes) => parse_cursor(name, &bytes)?,
            };
            if next > high_water {
                return Err(Error::invalid(format!(
                    "the {name} drain cursor is at {next} and the spool holds only up to \
                     {high_water}; that cursor was committed against another spool, and \
                     resuming from it would skip records — inspect the spool rather than clear it"
                )));
            }
            cursors.push(Cursor { sink, next });
        }
        Ok(Self {
            log,
            cursors,
            publisher,
            telemetry,
            config,
            last_seal: Instant::now(),
        })
    }

    /// Each sink's next offset, by name.
    pub fn cursors(&self) -> BTreeMap<&'static str, u64> {
        self.cursors
            .iter()
            .map(|cursor| (cursor.sink.name(), cursor.next))
            .collect()
    }

    /// Deliver what each sink has not yet taken, seal on the cadence, and
    /// retire what every sink has read past.
    pub fn step(&mut self) -> Result<DrainStep> {
        let mut report = DrainStep::default();
        let high_water = self.log.high_water();
        for cursor in &mut self.cursors {
            let mut taken = 0usize;
            while cursor.next < high_water && taken < self.config.max_batches_per_step {
                let Some(batch) = self.log.read(cursor.next)? else {
                    break;
                };
                let stream = stream_of(&batch)?;
                match cursor.sink.deliver(cursor.next, stream, &batch) {
                    Ok(()) => {
                        cursor.next = cursor.next.saturating_add(1);
                        self.log.manifest_put(
                            &format!("{CURSOR_PREFIX}{}", cursor.sink.name()),
                            cursor.next.to_string().as_bytes(),
                        )?;
                        taken += 1;
                        self.telemetry.drain("delivered");
                    }
                    Err(error) => {
                        self.telemetry.drain("refused");
                        if cursor.sink.fenced() {
                            self.publisher.mark_fenced();
                        }
                        report
                            .refused
                            .push((cursor.sink.name(), error.message().to_string()));
                        break;
                    }
                }
            }
            report.delivered += taken;
        }
        let connected = report.refused.is_empty();
        self.publisher.set_connected(connected);
        self.telemetry.connected(connected);

        if self.last_seal.elapsed() >= self.config.seal_every {
            self.log.seal_active()?;
            self.last_seal = Instant::now();
        }
        let floor = self
            .cursors
            .iter()
            .map(|cursor| cursor.next)
            .min()
            .unwrap_or(high_water);
        for segment in self.log.segments() {
            if segment.sealed
                && segment.end_offset <= floor
                && !self.log.is_archived(segment.start_offset)?
            {
                self.log.mark_archived(segment.start_offset)?;
            }
        }
        report.retired = self.log.retain_before(floor)?.deleted.len();
        Ok(report)
    }

    /// The drain step of a rollout: seal the spool, then step until every
    /// sink has taken everything it held at this call, a sink is fenced, or
    /// `max_steps` pass. A refusal from an unfenced sink is retried within
    /// the bound, as an ordinary step retries it.
    pub fn drain_to_tail(&mut self, max_steps: usize) -> Result<DrainedTo> {
        self.log.seal_active()?;
        self.last_seal = Instant::now();
        let target = self.log.high_water();
        let mut delivered = 0usize;
        for steps in 1..=max_steps {
            let step = self.step()?;
            delivered += step.delivered;
            if self.cursors.iter().all(|cursor| cursor.next >= target) {
                return Ok(DrainedTo::Tail { delivered, steps });
            }
            if let Some(cursor) = self.cursors.iter().find(|cursor| cursor.sink.fenced()) {
                return Ok(DrainedTo::Stalled {
                    delivered,
                    steps,
                    reason: format!(
                        "the {} sink is fenced by a newer producer epoch at spool offset {} of \
                         {target}; what is left stays on the spool",
                        cursor.sink.name(),
                        cursor.next
                    ),
                });
            }
        }
        let behind: Vec<String> = self
            .cursors
            .iter()
            .filter(|cursor| cursor.next < target)
            .map(|cursor| format!("{} at {}", cursor.sink.name(), cursor.next))
            .collect();
        Ok(DrainedTo::Stalled {
            delivered,
            steps: max_steps,
            reason: format!(
                "{max_steps} steps passed with {} short of spool offset {target}",
                behind.join(", ")
            ),
        })
    }

    /// Run [`Self::step`] on a thread of its own until stopped.
    pub fn spawn(self) -> Result<DrainThread> {
        self.spawn_with(None)
    }

    /// [`Self::spawn`], watching `fence`: once a fence that has held stops
    /// admitting, the thread drains the spool to its tail, records the
    /// outcome and stops (see the module documentation's last section). A
    /// fence that never held — a standby — triggers nothing.
    pub fn spawn_fenced(self, fence: Fence, clock: Arc<dyn Clock>) -> Result<DrainThread> {
        self.spawn_with(Some((fence, clock)))
    }

    fn spawn_with(mut self, fence: Option<(Fence, Arc<dyn Clock>)>) -> Result<DrainThread> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let drained: Arc<Mutex<Option<DrainedTo>>> = Arc::new(Mutex::new(None));
        let thread_drained = Arc::clone(&drained);
        let poll = self.config.poll;
        let handle = std::thread::Builder::new()
            .name(DRAIN_THREAD.to_string())
            .spawn(move || {
                let mut failures: u64 = 0;
                let mut held = false;
                while !thread_stop.load(Ordering::Acquire) {
                    if let Some((fence, clock)) = &fence {
                        if fence.status(clock.now()).admits() {
                            held = true;
                        } else if held {
                            let outcome =
                                self.drain_to_tail(MAX_DRAIN_OUT_STEPS)
                                    .unwrap_or_else(|error| DrainedTo::Stalled {
                                        delivered: 0,
                                        steps: 0,
                                        reason: error.message().to_string(),
                                    });
                            eprintln!(
                                "qip-edge-node: the fence stopped admitting; the spool drain \
                                 drained out and stopped: {outcome:?}"
                            );
                            if let Ok(mut slot) = thread_drained.lock() {
                                *slot = Some(outcome);
                            }
                            break;
                        }
                    }
                    match self.step() {
                        Ok(step) => {
                            for (sink, reason) in &step.refused {
                                if failures.is_multiple_of(100) {
                                    eprintln!(
                                        "qip-edge-node: the {sink} sink refused a spool batch \
                                         ({reason}); it stays on the spool and is retried"
                                    );
                                }
                                failures = failures.saturating_add(1);
                            }
                            if step.delivered == 0 {
                                std::thread::sleep(poll);
                            }
                        }
                        Err(error) => {
                            if failures.is_multiple_of(100) {
                                eprintln!(
                                    "qip-edge-node: the spool drain failed a step ({}); the \
                                     spool is retried and fills meanwhile",
                                    error.message()
                                );
                            }
                            failures = failures.saturating_add(1);
                            self.publisher.set_connected(false);
                            std::thread::sleep(poll);
                        }
                    }
                }
            })
            .map_err(|error| {
                Error::io(format!("cannot start the {DRAIN_THREAD} thread: {error}"))
            })?;
        Ok(DrainThread {
            stop,
            handle: Some(handle),
            drained,
        })
    }
}

fn parse_cursor(name: &str, bytes: &[u8]) -> Result<u64> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .ok_or_else(|| {
            Error::schema(format!(
                "the {name} drain cursor in the spool manifest does not read as an offset; \
                 restarting it from zero would deliver everything again — inspect it"
            ))
        })
}

/// The running drain. Dropping it stops the thread after its current step;
/// it is not joined, because a step blocked on an unreachable fabric would
/// make the drop hang with it.
#[derive(Debug)]
pub struct DrainThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    drained: Arc<Mutex<Option<DrainedTo>>>,
}

impl DrainThread {
    /// How the drain-out on fence ended, once it has run; `None` before the
    /// fence stopped admitting, and always for a drain spawned unfenced.
    pub fn drained_on_fence(&self) -> Option<DrainedTo> {
        self.drained.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }
}

impl Drop for DrainThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

// --- the fabric -------------------------------------------------------------

/// Delivers P1 batches to [`OUTCOMES_STREAM`] and P2 batches to
/// [`JOURNAL_STREAM`] on `qip-fabricd`, one idempotent producer per stream.
#[derive(Debug)]
pub struct FabricSink {
    outcomes: Producer,
    journal: Producer,
    /// The cell's region, stamped on every envelope opened before a send.
    region: String,
}

impl FabricSink {
    /// A sink over two producers, which must name the two reflex streams:
    /// a producer pointed at the wrong stream would put outcomes where the
    /// ledger never reads them.
    pub fn new(outcomes: Producer, journal: Producer, region: impl Into<String>) -> Result<Self> {
        let region = region.into();
        if region.trim().is_empty() {
            return Err(Error::invalid(
                "the fabric sink needs the cell's region: every envelope it sends is attributed \
                 to one"
                    .to_string(),
            ));
        }
        if outcomes.stream() != OUTCOMES_STREAM || journal.stream() != JOURNAL_STREAM {
            return Err(Error::invalid(format!(
                "the fabric sink produces outcomes to {OUTCOMES_STREAM} and the journal to \
                 {JOURNAL_STREAM}; it was handed producers for {} and {}",
                outcomes.stream(),
                journal.stream()
            )));
        }
        Ok(Self {
            outcomes,
            journal,
            region,
        })
    }
}

impl Sink for FabricSink {
    fn name(&self) -> &'static str {
        "fabric"
    }

    fn deliver(&mut self, offset: u64, stream: QosClass, batch: &Batch) -> Result<()> {
        let producer = match stream {
            QosClass::P1Outcomes => &mut self.outcomes,
            QosClass::P2MarketJournal => &mut self.journal,
            other => {
                return Err(Error::schema(format!(
                    "the spool holds a {} batch, and this node's writer only writes P1 and P2",
                    other.as_str()
                )));
            }
        };
        // ADR 0129 §2: the writer stamps the kind of component that produced
        // the batch, and a consumer refuses a batch carrying none rather than
        // naming one itself (R12-FABRIC-8022). The preflight below cannot see
        // that refusal — it opens the batch as the producer, and a producer's
        // own preflight may name its own provenance — so a spool batch no
        // writer stamped (the format-version-1 batch an older binary left
        // behind) is refused here, before anything is sent, instead of being
        // acknowledged by the broker and stalling the ledger's partition for
        // ever.
        if batch.provenance.is_none() {
            return Err(Error::schema(format!(
                "the spool batch at offset {offset} carries no writer-stamped provenance; the \
                 ledger refuses such a batch rather than naming one, so it is not sent"
            )));
        }
        // R12-CONTRACT-7089: every record is opened as the `FabricEnvelope` the
        // ledger will open it as, before the producer is touched. A record the
        // consumer would refuse (a version it cannot read, a payload that does
        // not match its hash, a topic on the wrong class of stream) would be
        // acknowledged by the broker and then stall the ledger's partition for
        // ever, because the ledger never skips a record it cannot read. Refused
        // here, the batch stays in the spool, nothing is sent, no sequence is
        // consumed and the drain's cursor does not move.
        let context = FabricContext {
            stream: producer.stream().to_string(),
            region: Some(self.region.clone()),
            partition: producer.partition(),
            qos_class: stream,
            auth_context: format!("producer:{}", producer.producer_id()),
            provenance: "qip-edge-node.drain".to_string(),
        };
        let mut preflight = batch.clone();
        preflight.producer_id = producer.producer_id().to_string();
        FabricEnvelope::open_all(&preflight, &context, &bindings::ALL)?;
        // An epoch is taken on first use and again after a fence was for an
        // epoch this producer no longer holds; `init` is the broker's
        // statement of where this producer's dense sequence continues.
        if producer.epoch().is_none() {
            producer.init()?;
        }
        producer.send(batch.clone()).map(|_| ())
    }

    fn fenced(&self) -> bool {
        self.outcomes.is_fenced() || self.journal.is_fenced()
    }
}

// --- the tick lake -------------------------------------------------------------

/// The variable naming which datasets' ticks the lake may keep, and how:
/// `dataset=canonical[|raw];dataset=canonical;…` (R12-DATA-1039).
pub const RETENTION_VARIABLE: &str = "QIP_TICK_RETENTION";

/// The variable naming the codec new lake segments are sealed with:
/// `identity` (the default), `zstd` or `zstd:<level>` (R12-TICK-2057, ADR
/// 0113 §3 — off until a measurement says otherwise).
pub const CODEC_VARIABLE: &str = "QIP_TICK_LAKE_CODEC";

/// The retention policy a node starts with when nothing is configured: the
/// simulator's own depth, canonical only, and nothing else.
///
/// The simulated dataset is generated in this process by the platform's own
/// matching engine, so no venue's terms govern it and keeping it breaches
/// nobody's rights. Every licensed dataset is absent until an operator names
/// what its terms allow — rights nobody evaluated are not rights — and its
/// ticks are used live and not kept.
pub fn default_retention() -> BTreeMap<String, RetentionRight> {
    // Raw too, for this one dataset (ADR 0181 decision 1): its terms are the
    // platform's own, so nothing restricts keeping the bytes the simulated
    // venue published. Every licensed dataset still keeps raw bytes only
    // where its own right says so.
    BTreeMap::from([(
        crate::feed::SIMULATED_DATASET.to_string(),
        RetentionRight {
            canonical: true,
            raw: true,
        },
    )])
}

/// Parse [`RETENTION_VARIABLE`] over [`default_retention`]. Refuses a clause
/// it cannot read and a right it does not know, by name: a typo that read as
/// "retain nothing" would silently drop a licensed history the desk was
/// promised, and one that read as "retain everything" would keep bytes the
/// licence forbids.
pub fn parse_retention(text: Option<&str>) -> Result<BTreeMap<String, RetentionRight>> {
    let mut policy = default_retention();
    let Some(text) = text else {
        return Ok(policy);
    };
    for clause in text.split(';').map(str::trim).filter(|c| !c.is_empty()) {
        let (dataset, rights) = clause.split_once('=').ok_or_else(|| {
            Error::invalid(format!(
                "{RETENTION_VARIABLE} clause {clause:?} is not dataset=canonical[|raw]"
            ))
        })?;
        let dataset = dataset.trim();
        if dataset.is_empty() {
            return Err(Error::invalid(format!(
                "{RETENTION_VARIABLE} clause {clause:?} names no dataset"
            )));
        }
        let mut right = RetentionRight {
            canonical: false,
            raw: false,
        };
        for token in rights.split('|').map(str::trim).filter(|t| !t.is_empty()) {
            match token {
                "canonical" => right.canonical = true,
                "raw" => right.raw = true,
                "none" => {}
                other => {
                    return Err(Error::invalid(format!(
                        "{RETENTION_VARIABLE} grants {dataset} the right {other:?}; the rights                          are canonical, raw and none"
                    )));
                }
            }
        }
        if right.raw && !right.canonical {
            return Err(Error::invalid(format!(
                "{RETENTION_VARIABLE} lets {dataset} keep raw bytes and not the canonical \
                 record; the lake keeps raw bytes only beside the record they decode to"
            )));
        }
        if right.raw && dataset != crate::feed::SIMULATED_DATASET {
            // The lake keeps a venue's bytes by re-encoding each line with the
            // venue's own encoder and checking it against the hash taken at
            // publication (ADR 0181); only the simulated feed's encoder is in
            // this tree. A raw grant for any other dataset is one the sink
            // could not honour, and a sink that silently sealed it raw-less
            // would read as a retained raw history nobody can re-materialise.
            return Err(Error::invalid(format!(
                "{RETENTION_VARIABLE} grants {dataset} raw retention, and the lake can keep raw \
                 bytes only for a feed whose encoder is in this tree ({}); grant it canonical \
                 alone until its raw frames have their own record on the spool (ADR 0181)",
                crate::feed::SIMULATED_DATASET
            )));
        }
        policy.insert(dataset.to_string(), right);
    }
    Ok(policy)
}

/// The most partitions [`LakeSink`] aligns each line against. A cell's feeds
/// carry a few venues and at most `MAX_FEED_INSTRUMENTS` instruments each, so
/// hundreds means the key is wrong. Past the bound the partition whose latest
/// tick arrived longest ago is dropped from the working set, so a line is
/// never aligned against more than this many and the sink's memory does not
/// grow with the session. A dropped partition is left out of later lines'
/// stamps. It is not reported with a stale position as if it were current.
pub const MAX_ALIGNED_PARTITIONS: usize = 256;

/// Half the resolution a `Timestamp` keeps across the spool. A tick reaches
/// [`LakeSink`] as RFC 3339 at millisecond resolution, so each instant it
/// aligns is the true one floored by up to a millisecond, and the lag
/// between two of them is off by less than one millisecond either way.
/// Each side's uncertainty is widened by half that before aligning, so the
/// combined bound covers the truncation: without it, instants 0.999 ms and
/// 1.000 ms on clocks known to 50 µs floor to 0 and 1 ms, read 1 ms apart
/// against a 0.1 ms bound, and are stamped `before` when the clocks cannot
/// order them at all — the false order the alignment exists to refuse.
const SPOOL_TRUNCATION_HALF_WIDTH: qip_core::time::Duration =
    qip_core::time::Duration::from_micros(500);

/// The last tick [`LakeSink`] wrote for one partition, placed on the
/// reference timeline.
#[derive(Clone, Copy, Debug)]
struct LatestTick {
    placed: Placed,
    sequence: u64,
}

/// Delivers the market events in P2 batches to the tick lake, one sealed
/// segment per spool offset per partition. Every other record is not the
/// lake's: a journal entry is the platform's own record and reaches the lake
/// through the central archive, not from a cell.
///
/// What is kept is the retention policy's to decide, per dataset
/// (R12-DATA-1039): a tick whose dataset has no right, or a right that does
/// not permit the canonical record, is used live and not written, and is
/// counted in [`LakeSink::withheld`]. The batch still completes — withholding
/// is the policy working, not a failed delivery, and retrying it would stall
/// the spool until the cell halted.
#[derive(Debug)]
pub struct LakeSink {
    store: Arc<dyn BlobStore>,
    cell: String,
    /// The region the cell runs in, stamped on every line (R12-TICK-2048):
    /// two regions' captures of one instrument are two clocks, and a reader
    /// that cannot tell which region stamped a line cannot tell which clock
    /// error it carries.
    region: String,
    /// The latest tick written per partition (`venue/feed/partition/
    /// instrument`), so each new line can be aligned against every other
    /// partition (R12-TICK-2048). One entry per partition, at most
    /// [`MAX_ALIGNED_PARTITIONS`] of them: a bounded working set, never a
    /// history.
    latest: BTreeMap<String, LatestTick>,
    retention: BTreeMap<String, RetentionRight>,
    codec: SegmentCodec,
    withheld: u64,
    /// Partitions whose raw bytes were not kept because a line's
    /// re-encoding did not hash to what the venue published.
    raw_unverified: u64,
    /// The clock discipline every applied tick passes through, per venue
    /// feed and partition, before its line is written (TICK-1122). Run here,
    /// on the drain thread, because the discipline is a function of the
    /// events' own venue and capture instants in spool order: the decision
    /// thread gains nothing from it and would pay for it on every pass.
    capture: CaptureStream,
}

impl LakeSink {
    /// A lake sink for `cell` in `region`, keeping what `retention` permits
    /// and sealing with `codec`. Refuses a cell id the lake cannot put in a
    /// segment id. The check runs here at start. Otherwise the sink would
    /// refuse every batch and the spool would fill until the cell halted.
    /// Refuses an empty or unprintable region for the same reason. A line
    /// whose region field is blank cannot be placed against another region's
    /// clock, and that is the reason the field exists (R12-TICK-2048).
    ///
    /// The region, policy and codec are arguments, not defaults. No caller
    /// can build a sink without stating where it runs and what it keeps. A
    /// caller with nothing configured passes [`default_retention`] and
    /// [`SegmentCodec::Identity`] and says so.
    pub fn new(
        store: Arc<dyn BlobStore>,
        cell: impl Into<String>,
        region: impl Into<String>,
        retention: BTreeMap<String, RetentionRight>,
        codec: SegmentCodec,
    ) -> Result<Self> {
        let cell = cell.into();
        let region = region.into();
        let nameable = |name: &str| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        };
        if !nameable(&cell) {
            return Err(Error::invalid(format!(
                "cell id {cell:?} cannot name a lake segment: use letters, digits, '.', '_' or \
                 '-'"
            )));
        }
        if !nameable(&region) {
            return Err(Error::invalid(format!(
                "region {region:?} cannot be stamped on a lake line: name the cell's region \
                 (QIP_CELL_REGION) in letters, digits, '.', '_' or '-'"
            )));
        }
        Ok(Self {
            store,
            cell,
            region,
            latest: BTreeMap::new(),
            retention,
            codec,
            withheld: 0,
            raw_unverified: 0,
            capture: CaptureStream::new(DisciplineSettings::default())?,
        })
    }

    /// The discipline the sink holds for `stream` (`venue/feed/partition`),
    /// so a health surface or a test can read the bound it publishes.
    pub fn discipline(&self, stream: &str) -> Option<&qip_sequencing::clock::ClockDiscipline> {
        self.capture.discipline(stream)
    }

    /// Ticks the retention policy kept out of the lake so far.
    pub const fn withheld(&self) -> u64 {
        self.withheld
    }

    /// Partitions sealed without their raw bytes because a line could not
    /// be re-encoded to the bytes the venue published.
    pub const fn raw_unverified(&self) -> u64 {
        self.raw_unverified
    }

    /// Partitions the cross-partition alignment currently holds a latest
    /// tick for; never more than [`MAX_ALIGNED_PARTITIONS`].
    pub fn aligned_partitions(&self) -> usize {
        self.latest.len()
    }

    /// Align the tick about to be written for `partition` against the
    /// latest tick of every other partition, then record it as that
    /// partition's latest (R12-TICK-2048).
    ///
    /// The stamp is `qip_sequencing::align` and nothing else. An order is
    /// stated only when the two stamps are further apart than both
    /// uncertainties together, and is `unordered` otherwise. Before this,
    /// a lake line's `arrival_order` compared a tick only with the previous
    /// tick of its own stream. Any cross-venue or cross-instrument order a
    /// reader wanted had to come from sorting on a timestamp column, and
    /// that sort claims an order two independent clocks cannot support. A
    /// lead/lag model trained on it learns clock error as a reaction.
    ///
    /// A tick the discipline could not place has no position on the
    /// reference timeline. Its stamp is the refusal, it is aligned against
    /// nothing, and it does not become its partition's latest.
    fn align_across(
        &mut self,
        partition: String,
        sequence: u64,
        placed: &Result<Placed>,
    ) -> serde_json::Value {
        let placed = match placed {
            Ok(placed) => *placed,
            Err(error) => return serde_json::json!({ "refused": error.message() }),
        };
        let others: Vec<serde_json::Value> = self
            .latest
            .iter()
            .filter(|(other, _)| **other != partition)
            .map(|(other, latest)| {
                let alignment = align(&latest.placed, &placed);
                serde_json::json!({
                    "partition": other,
                    "sequence": latest.sequence,
                    // The other partition's tick relative to this one:
                    // `before` means it happened first by more than both
                    // clocks' error.
                    "order_of_other": match alignment.order {
                        Order::Before => "before",
                        Order::After => "after",
                        Order::Unordered => "unordered",
                    },
                    "lag_ns": alignment.lag.as_nanos(),
                    "lag_uncertainty_ns": alignment.lag_uncertainty.as_nanos(),
                    "other_known_at_arrival": alignment.first_known_when_second_arrived,
                })
            })
            .collect();
        if !self.latest.contains_key(&partition) && self.latest.len() >= MAX_ALIGNED_PARTITIONS {
            // The partition silent longest leaves; ties broken by name so a
            // replay of the same spool drops the same one.
            let oldest = self
                .latest
                .iter()
                .min_by(|a, b| {
                    a.1.placed
                        .arrival()
                        .cmp(&b.1.placed.arrival())
                        .then_with(|| a.0.cmp(b.0))
                })
                .map(|(name, _)| name.clone());
            if let Some(oldest) = oldest {
                self.latest.remove(&oldest);
            }
        }
        self.latest
            .insert(partition, LatestTick { placed, sequence });
        serde_json::Value::Array(others)
    }
}

impl Sink for LakeSink {
    fn name(&self) -> &'static str {
        "lake"
    }

    fn deliver(&mut self, offset: u64, stream: QosClass, batch: &Batch) -> Result<()> {
        if stream != QosClass::P2MarketJournal {
            return Ok(());
        }
        // Keyed by partition so one segment holds one venue, day,
        // instrument and licence; a BTreeMap so the write order — and a
        // retry's — is the same every time.
        let mut by_partition: BTreeMap<(String, String, String, String), Vec<u8>> = BTreeMap::new();
        // The venue's bytes per partition, where the dataset's right keeps
        // them (ADR 0181): `None` once any line of the partition could not be
        // re-encoded to what was published, so a raw slot is either the whole
        // partition's bytes or absent, never a session with a hole in it.
        let mut raw_by_partition: BTreeMap<(String, String, String, String), Option<Vec<u8>>> =
            BTreeMap::new();
        for record in &batch.records {
            let event = record.decode_payload(batch.encoding)?;
            if event.topic != MARKET_EVENT_APPLIED.topic {
                continue;
            }
            let tick: MarketEvent = market_event(&event)?;
            // Every tick feeds the discipline, kept or withheld: what the
            // retention right governs is what is stored, not whether the
            // venue's clock is measured.
            //
            // A tick the discipline refuses is still written, carrying the
            // refusal in place of the disciplined fields: returning the
            // error would retry the batch forever, fill the spool and halt
            // the cell over a timestamp annotation.
            let captured = self.capture.capture(&tick, None);
            let permitted = self
                .retention
                .get(tick.entitlement().dataset())
                .is_some_and(|right| right.canonical);
            if !permitted {
                self.withheld = self.withheld.saturating_add(1);
                continue;
            }
            // Placed exactly as the capture stream placed it for its own
            // stream's arrival order: the normalized instant, the published
            // uncertainty, and the capture instant on the cell's clock.
            let placed = match &captured {
                Ok(captured) => Placed::new(
                    captured.normalized_time,
                    captured.uncertainty + SPOOL_TRUNCATION_HALF_WIDTH,
                    captured.receive_time,
                ),
                Err(error) => Err(Error::invalid(format!(
                    "the clock discipline refused this tick, so it has no place on the \
                     reference timeline to align from: {}",
                    error.message()
                ))),
            };
            let origin = tick.origin();
            let partition = format!(
                "{}/{}/{}/{}",
                origin.venue,
                origin.feed,
                origin.partition,
                tick.payload().object_id
            );
            let cross = self.align_across(partition, origin.sequence, &placed);
            let line = tick_line(
                &tick,
                &captured,
                Provenance {
                    region: &self.region,
                    cell: &self.cell,
                },
                cross,
            )?;
            let key = (
                tick.origin().venue.as_str().to_string(),
                tick.event_time().to_date_string(),
                tick.payload().object_id.as_str().to_string(),
                tick.entitlement().dataset().to_string(),
            );
            if self
                .retention
                .get(tick.entitlement().dataset())
                .is_some_and(|right| right.raw)
            {
                let slot = raw_by_partition
                    .entry(key.clone())
                    .or_insert_with(|| Some(Vec::new()));
                match (slot.as_mut(), crate::feed::native_line(&tick)) {
                    (Some(bytes), Some(native)) => bytes.extend_from_slice(native.as_bytes()),
                    (Some(_), None) => *slot = None,
                    (None, _) => {}
                }
            }
            let body = by_partition.entry(key).or_default();
            body.extend_from_slice(line.as_bytes());
            body.push(b'\n');
        }
        let lake = Lake::new(self.store.as_ref()).with_codec(self.codec);
        let segment_id = format!("{}-o{offset:020}", self.cell);
        for (key, canonical) in by_partition {
            // The venue's bytes ride beside the decoded lines where the
            // dataset's right keeps them and every line re-encoded to what
            // was published; otherwise the segment is sealed without them and
            // the shortfall counted.
            let raw = match raw_by_partition.remove(&key) {
                Some(Some(bytes)) => Some(bytes),
                Some(None) => {
                    self.raw_unverified = self.raw_unverified.saturating_add(1);
                    None
                }
                None => None,
            };
            let (venue, date, instrument, dataset) = key;
            let raw_retention_permitted = self.retention.get(&dataset).is_some_and(|r| r.raw);
            let partition = Partition::new(
                RecordClass::Market,
                &venue,
                &date,
                &instrument,
                Entitlement {
                    id: dataset,
                    raw_retention_permitted,
                },
            )?;
            match lake.write_segment(&partition, &segment_id, canonical, raw) {
                Ok(_) => {}
                // A segment id is one spool offset of one cell, so a sealed
                // one is this sink's own earlier write of these same records,
                // retried after a later partition of the batch failed. Any
                // other refusal is returned and the batch retried.
                Err(error) if error.code() == "denied" => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// One tick as the lake holds it: the venue's instant, the node's capture
/// instant and the feed's own sequence named beside the event, so a reader
/// of the lake does not have to know the event's serde shape to find the
/// three facts it is partitioned and ordered by.
///
/// Beside them, what the clock discipline made of the tick (TICK-2051,
/// TICK-2089): the normalized instant and its uncertainty as two fields of
/// their own, every correction applied to reach it, and the arrival order
/// against the stream's previous tick. A lake that kept only the raw
/// instants would leave every reader to re-derive the discipline, and two
/// readers that did it differently would disagree about the order of the
/// same two ticks.
///
/// And the synchronisation metadata a cross-venue, cross-instrument or
/// cross-region reader needs (R12-TICK-2048): the region and cell that
/// captured the tick, the clock its normalized instant was taken from
/// ([`clock_source`]), and `cross_partition`, the tick aligned against the
/// latest tick of every other partition ([`LakeSink`]'s `align_across`).
fn tick_line(
    tick: &MarketEvent,
    captured: &Result<CapturedMarketEvent>,
    provenance: Provenance<'_>,
    cross_partition: serde_json::Value,
) -> Result<String> {
    let origin = tick.origin();
    let clock = match captured {
        Ok(captured) => serde_json::json!({
            "normalized_time_ns": captured.normalized_time.as_nanos(),
            "uncertainty_ns": captured.uncertainty.as_nanos(),
            "corrections": serde_json::to_value(&captured.corrections)?,
            "arrival_order": captured.arrival_order,
        }),
        Err(error) => serde_json::json!({ "refused": error.message() }),
    };
    let value = serde_json::json!({
        "exchange_time_ns": tick.event_time().as_nanos(),
        "capture_time_ns": tick.receive_time().as_nanos(),
        "clock": clock,
        "clock_source": clock_source(captured),
        "region": provenance.region,
        "cell": provenance.cell,
        "cross_partition": cross_partition,
        "feed": origin.feed,
        "partition": origin.partition,
        "sequence": origin.sequence,
        "event": serde_json::to_value(tick)?,
    });
    Ok(qip_core::canonical::canonical_json(&value))
}

/// Who captured a tick, as [`tick_line`] stamps it.
#[derive(Clone, Copy, Debug)]
struct Provenance<'a> {
    region: &'a str,
    cell: &'a str,
}

/// Which clock a lake line's normalized instant and uncertainty come from
/// (R12-TICK-2048), named so a reader does not infer it from the shape of
/// the `corrections` list:
///
/// - `offset_estimate`: the venue's stamp moved onto the cell's reference
///   clock by the discipline's measured offset, with that estimate's
///   uncertainty;
/// - `venue_stated`: the venue's own stamp, before the estimate is trusted,
///   with the uncertainty the tick stated for itself;
/// - `unplaced`: the discipline refused the tick, so it has no normalized
///   instant at all.
///
/// Two lines carrying the same `uncertainty_ns` can come from different
/// clocks. A reader that pooled a venue's self-stated figure with a measured
/// estimate would weight an unmeasured clock as though someone had measured
/// it.
fn clock_source(captured: &Result<CapturedMarketEvent>) -> &'static str {
    match captured {
        Ok(captured)
            if captured
                .corrections
                .iter()
                .any(|correction| correction.kind == "offset_estimate") =>
        {
            "offset_estimate"
        }
        Ok(_) => "venue_stated",
        Err(_) => "unplaced",
    }
}
