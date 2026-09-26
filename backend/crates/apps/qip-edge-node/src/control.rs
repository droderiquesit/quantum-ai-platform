//! ADR 0100 §6: control compilation, halt-wire polling and file reads move
//! off the decision thread. SLICE-33.
//!
//! Red-team M5 found three `std::fs` reads on the thread that runs
//! `Cell::work` — `StrategyInstaller::install` reading and compiling the
//! plan, `HaltFlag::poll`, and the region wire's `poll` — none of which an
//! import scan of the pass loop shows, because each is one call away. Each
//! is a blocking read of a file that lives on a mount: a hung mount stopped
//! every pass for as long as it hung, and a halt that could not be read
//! stopped trading by *freezing* it, with orders resting and nothing
//! withdrawing them at their time to live. This module is where those reads
//! go instead, and the narrow ways their results come back:
//!
//! - [`Handoff`] — the one bounded channel by which verified control values
//!   (a [`VerifiedEnvelope`], a [`VerifiedPolicy`], a [`VerifiedHalt`]) and
//!   [`CompiledPlan`]s reach the decision thread. The scheduler drains it
//!   with `try_recv` at a pass boundary and never inside a pass, and every
//!   value carries where it came from — a [`ControlPosition`] for a fabric
//!   frame, a path and digest for a plan — so the pass that applied it can
//!   record it.
//! - [`PlanCompiler`] — the thread that reads and compiles a plan and hands
//!   the result across. The decision thread asks it with `try_send` and
//!   never waits for an answer.
//! - [`Poller`] — the threads that read the halt flag and the region wire
//!   and publish each reading with the instant it was read. A reading older
//!   than its bound is read as [`PolledHalt::Unreadable`] and
//!   [`RegionOutlook::Unreadable`]: a reader that has stopped, whether its
//!   file hung or its thread died, leaves the cell **halted**, never frozen
//!   at its last good reading ("stale is treated as engaged", §46.2).
//! - [`CaughtUp`] — the one-shot the event-fabric consumer (SLICE-57) fires
//!   once it has replayed to the head, and the scheduler (SLICE-55) waits on
//!   before its first pass.
//!
//! No async runtime: std threads and `std::sync::mpsc`, every channel
//! bounded, every wait on the decision thread either non-blocking or given
//! an explicit timeout.
//!
//! Additive. `StrategyInstaller::install`, `HaltFlag::poll` and
//! `DarkRegionWire::poll` keep their signatures and behaviour for the mesh
//! path and `main.rs` until SLICE-36 switches fabric mode over to this.

use crate::dark::DarkRegionWire;
use crate::halt::HaltFlag;
use crate::strategies::{CompiledPlan, PlanInstallation, StrategyInstaller};
use qip_contracts::policy::PlanDigest;
use qip_contracts::replay::ControlPosition;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_edge::cell::{Cell, PolledHalt};
use qip_edge::envelope::VerifiedEnvelope;
use qip_edge::policy::{VerifiedHalt, VerifiedPolicy};
use qip_edge::region::RegionOutlook;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

/// The most values a handoff may hold. A channel sized past this is not a
/// handoff but a store: control that has waited that long behind a stalled
/// decision thread is control the centre would reissue anyway.
pub const MAX_HANDOFF_CAPACITY: usize = 4096;

/// The longest a poller's reading may be trusted for. §46.2's polled halt is
/// the path that must work when the mesh does not; a bound past this would
/// let a dead reader hold a released cell released for longer than an
/// operator waits before reaching for the broadcast instead.
pub const MAX_STALENESS_BOUND: StdDuration = StdDuration::from_secs(30);

/// Where a value handed across came from — what the pass that applied it
/// records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// A frame read from a control stream at this position.
    Fabric(ControlPosition),
    /// A plan read from this path, digesting to this digest.
    Plan { path: PathBuf, digest: String },
}

/// The four kinds of value that may cross to the decision thread. Nothing
/// else can: a reading from the filesystem goes through [`Poller`], and an
/// unverified frame has no arm here.
#[derive(Clone, Debug)]
pub enum ControlValue {
    Envelope(VerifiedEnvelope),
    /// Boxed because a verified payload is several times the size of every
    /// other arm, and a channel slot is sized for its widest value.
    Policy(Box<VerifiedPolicy>),
    Halt(VerifiedHalt),
    Plan(CompiledPlan),
}

impl ControlValue {
    /// The kind, for a report.
    pub fn kind(&self) -> ControlKind {
        match self {
            Self::Envelope(_) => ControlKind::Envelope,
            Self::Policy(_) => ControlKind::Policy,
            Self::Halt(_) => ControlKind::Halt,
            Self::Plan(_) => ControlKind::Plan,
        }
    }
}

/// The kind of a value handed across, in the order a boundary applies them.
///
/// Halts first, because a halt is never improved by waiting; capital before
/// policy and policy before plans, as the mesh tick applies them, so a grant
/// and the payload whose manifest names it land in the same boundary in the
/// order the payload's share is summed against. Ordered by kind rather than
/// arrival, so a boundary's effect does not depend on which producer thread
/// won a race to the channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ControlKind {
    Halt,
    Envelope,
    Policy,
    Plan,
}

/// One value and where it came from. The only constructors pair a value
/// with its provenance, so nothing reaches the decision thread without the
/// position a pass needs to record it.
#[derive(Clone, Debug)]
pub struct Delivery {
    provenance: Provenance,
    value: ControlValue,
}

impl Delivery {
    pub fn envelope(position: ControlPosition, envelope: VerifiedEnvelope) -> Self {
        Self {
            provenance: Provenance::Fabric(position),
            value: ControlValue::Envelope(envelope),
        }
    }

    pub fn policy(position: ControlPosition, policy: VerifiedPolicy) -> Self {
        Self {
            provenance: Provenance::Fabric(position),
            value: ControlValue::Policy(Box::new(policy)),
        }
    }

    pub fn halt(position: ControlPosition, halt: VerifiedHalt) -> Self {
        Self {
            provenance: Provenance::Fabric(position),
            value: ControlValue::Halt(halt),
        }
    }

    /// A compiled plan, whose provenance is taken from the plan itself so
    /// the recorded digest cannot disagree with the programs it names.
    pub fn plan(plan: CompiledPlan) -> Self {
        Self {
            provenance: Provenance::Plan {
                path: plan.path().to_path_buf(),
                digest: plan.digest().to_string(),
            },
            value: ControlValue::Plan(plan),
        }
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn kind(&self) -> ControlKind {
        self.value.kind()
    }
}

/// The producing half of a [`Handoff`]: held by the fabric consumer and the
/// plan compiler, never by the decision thread.
#[derive(Clone, Debug)]
pub struct HandoffSender {
    sender: SyncSender<Delivery>,
}

impl HandoffSender {
    /// Hand a value across, waiting while the handoff is full.
    ///
    /// Blocking is the producer's to do, on its own thread: a full handoff
    /// means the decision thread has not reached a boundary, and a producer
    /// that dropped the value instead would lose a halt. Refused when the
    /// decision thread has gone, so a producer does not wait on a receiver
    /// that will never drain.
    pub fn send(&self, delivery: Delivery) -> Result<()> {
        self.sender.send(delivery).map_err(|_| {
            Error::denied(
                "the decision thread's handoff is closed; nothing will apply this value, and \
                 the producer should stop",
            )
        })
    }

    /// Hand a value across if there is room now.
    pub fn try_send(&self, delivery: Delivery) -> Result<()> {
        self.sender.try_send(delivery).map_err(|error| match error {
            TrySendError::Full(_) => Error::guard(
                "the handoff is full: the decision thread has not reached a pass boundary \
                 since it filled; retry after the next one",
            ),
            TrySendError::Disconnected(_) => Error::denied(
                "the decision thread's handoff is closed; nothing will apply this value",
            ),
        })
    }
}

/// What a boundary did with one value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Applied to the cell, or for a plan, activated in the installer.
    Applied,
    /// A grant for a strategy the cell does not run, held by the installer
    /// until a plan naming it deploys.
    Held,
    /// Refused, with the reason. Consumed all the same: a value refused
    /// once is not retried at the next boundary, because the refusal is the
    /// cell's answer to it and asking again cannot change that answer.
    Refused(String),
}

/// One value a boundary consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub kind: ControlKind,
    pub provenance: Provenance,
    pub outcome: Outcome,
}

/// What one pass boundary did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Boundary {
    /// Every value consumed, in the order applied.
    pub applied: Vec<Applied>,
    /// Every producer has dropped its sender. Reported and nothing more:
    /// with the fabric down the cell runs on its last valid envelope and
    /// policy until they expire (ADR 0008), and a boundary that halted on an
    /// empty channel would make every fabric outage a trading outage.
    pub producers_gone: bool,
    /// What the strategy installer did with the compiled plan in hand, when
    /// the boundary was given one.
    pub plan: Option<PlanInstallation>,
}

impl Boundary {
    /// The fabric positions this boundary consumed, applied or refused, in
    /// order — where the pass stopped reading each control stream.
    pub fn positions(&self) -> Vec<ControlPosition> {
        self.applied
            .iter()
            .filter_map(|applied| match &applied.provenance {
                Provenance::Fabric(position) => Some(position.clone()),
                Provenance::Plan { .. } => None,
            })
            .collect()
    }
}

/// The consuming half: owned by the decision thread and drained only at a
/// pass boundary.
#[derive(Debug)]
pub struct Handoff {
    receiver: Receiver<Delivery>,
    capacity: usize,
}

impl Handoff {
    /// A handoff holding at most `capacity` values.
    ///
    /// Refuses zero — a rendezvous channel would make every producer wait
    /// for a boundary, so a halt would reach the cell only when the decision
    /// thread happened to be draining — and anything past
    /// [`MAX_HANDOFF_CAPACITY`].
    pub fn bounded(capacity: usize) -> Result<(HandoffSender, Self)> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a handoff of capacity zero is a rendezvous, and a producer would wait on the \
                 decision thread for every value; give it room for at least one",
            ));
        }
        if capacity > MAX_HANDOFF_CAPACITY {
            return Err(Error::invalid(format!(
                "a handoff of capacity {capacity} is past the {MAX_HANDOFF_CAPACITY} this node \
                 holds; control that waits that long is control the centre reissues"
            )));
        }
        let (sender, receiver) = sync_channel(capacity);
        Ok((HandoffSender { sender }, Self { receiver, capacity }))
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Apply what has been handed across since the last boundary.
    ///
    /// Call between passes and never inside one: this is the only point at
    /// which a value handed across changes the cell, so a pass runs under
    /// exactly the control in force when it began. Takes at most
    /// [`Self::capacity`] values — a producer faster than the drain cannot
    /// hold the decision thread here, and whatever it sends past that waits
    /// for the next boundary. Each value is consumed once: applied, held or
    /// refused, it is not seen again.
    ///
    /// With `strategies`, a grant for a strategy the cell does not run is
    /// held for the plan, a compiled plan is activated, and the installer
    /// then deploys whatever the plan in hand names — so a plan and the
    /// grants that fund it deploy in the boundary the last of them arrives.
    pub fn boundary(
        &mut self,
        cell: &mut Cell,
        mut strategies: Option<&mut StrategyInstaller>,
        now: Timestamp,
    ) -> Boundary {
        let mut report = Boundary::default();
        let mut drained = Vec::new();
        while drained.len() < self.capacity {
            match self.receiver.try_recv() {
                Ok(delivery) => drained.push(delivery),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    report.producers_gone = true;
                    break;
                }
            }
        }
        // Stable: within a kind, arrival order.
        drained.sort_by_key(Delivery::kind);

        for Delivery { provenance, value } in drained {
            let kind = value.kind();
            let outcome = match value {
                ControlValue::Halt(halt) => {
                    cell.apply_halt(halt, now);
                    Outcome::Applied
                }
                ControlValue::Envelope(envelope) => {
                    apply_envelope(cell, strategies.as_deref_mut(), envelope, now)
                }
                ControlValue::Policy(policy) => match cell.apply_policy(*policy, now) {
                    Ok(()) => Outcome::Applied,
                    Err(error) => Outcome::Refused(error.message().to_string()),
                },
                ControlValue::Plan(plan) => match strategies.as_deref_mut() {
                    Some(installer) => {
                        installer.activate(plan);
                        Outcome::Applied
                    }
                    None => Outcome::Refused(
                        "a compiled plan reached a boundary with no strategy installer to \
                         activate it; nothing is deployed from it"
                            .to_string(),
                    ),
                },
            };
            report.applied.push(Applied {
                kind,
                provenance,
                outcome,
            });
        }

        if let Some(installer) = strategies {
            report.plan = Some(installer.install_compiled(cell, now));
        }
        report
    }
}

/// Route a verified grant as the mesh tick does: held for the plan when the
/// cell refuses it only because nothing runs under that name, renewed
/// otherwise.
fn apply_envelope(
    cell: &mut Cell,
    strategies: Option<&mut StrategyInstaller>,
    envelope: VerifiedEnvelope,
    now: Timestamp,
) -> Outcome {
    let strategy = envelope.strategy().as_str().to_string();
    let not_deployed = !cell.deployed_strategies().contains(&strategy.as_str())
        && cell
            .arbitrage()
            .is_none_or(|desk| desk.strategy().as_str() != strategy);
    if not_deployed && let Some(installer) = strategies {
        return match installer.offer(envelope) {
            Ok(()) => Outcome::Held,
            Err(error) => Outcome::Refused(error.message().to_string()),
        };
    }
    match cell.renew_capital(envelope, now) {
        Ok(()) => Outcome::Applied,
        Err(error) => Outcome::Refused(error.message().to_string()),
    }
}

/// The thread that reads and compiles the plan a payload names, and hands
/// the result across.
///
/// The decision thread's side is [`Self::ensure`]: a `try_send` of the
/// digest to compile, and nothing that waits. A plan file on a mount that
/// never answers stops this thread and no other; the cell keeps running the
/// plan it already has, and the payload's own freshness bounds how long that
/// may last.
#[derive(Debug)]
pub struct PlanCompiler {
    path: PathBuf,
    requests: SyncSender<PlanDigest>,
    /// The digest last queued, so a boundary does not queue it again every
    /// pass while the compile is in flight.
    requested: Option<String>,
    /// The digest and reason of the last compile the thread refused.
    refusal: Arc<Mutex<Option<(String, String)>>>,
}

impl PlanCompiler {
    /// Start the compiler for the plan at `path`, handing compiled plans to
    /// `handoff`. The thread ends when this is dropped or the decision
    /// thread's handoff closes.
    pub fn spawn(path: PathBuf, handoff: HandoffSender) -> Result<Self> {
        // One request in the queue and one in flight: a newer payload's
        // digest queued behind an older one is compiled next, and a third
        // waits for a boundary to ask again rather than piling up.
        let (requests, received) = sync_channel::<PlanDigest>(1);
        let refusal = Arc::new(Mutex::new(None));
        let thread_path = path.clone();
        let thread_refusal = Arc::clone(&refusal);
        std::thread::Builder::new()
            .name("qip-plan-compiler".to_string())
            .spawn(move || {
                for named in received {
                    match CompiledPlan::read_and_compile(&thread_path, &named) {
                        Ok(plan) => {
                            if handoff.send(Delivery::plan(plan)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let Ok(mut slot) = thread_refusal.lock() else {
                                return;
                            };
                            *slot = Some((named.digest.clone(), error.message().to_string()));
                        }
                    }
                }
            })
            .map_err(|error| Error::io(format!("cannot start the plan compiler: {error}")))?;
        Ok(Self {
            path,
            requests,
            requested: None,
            refusal,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ask for `named` to be compiled, unless it already has been asked for
    /// and not refused since. Never waits: a full queue answers `false` and
    /// the next boundary asks again.
    ///
    /// A digest whose compile was refused — the file was not there yet, or
    /// did not digest to what the payload names — is asked for again, as
    /// `install` re-reads on every tick, so a plan that lands after the
    /// payload naming it is deployed without a new payload.
    pub fn ensure(&mut self, named: &PlanDigest) -> Result<bool> {
        let refused_again = self
            .last_refusal()
            .is_some_and(|(digest, _)| digest == named.digest);
        if self.requested.as_deref() == Some(named.digest.as_str()) && !refused_again {
            return Ok(false);
        }
        if refused_again && let Ok(mut slot) = self.refusal.lock() {
            *slot = None;
        }
        match self.requests.try_send(named.clone()) {
            Ok(()) => {
                self.requested = Some(named.digest.clone());
                Ok(true)
            }
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Disconnected(_)) => Err(Error::io(
                "the plan compiler thread has stopped; no plan will be compiled until the node \
                 restarts",
            )),
        }
    }

    /// The digest and reason of the last compile the thread refused.
    pub fn last_refusal(&self) -> Option<(String, String)> {
        match self.refusal.lock() {
            Ok(slot) => slot.clone(),
            Err(_) => Some((
                String::new(),
                "the plan compiler panicked while recording a refusal".to_string(),
            )),
        }
    }
}

/// A reading a poller thread published, and when.
#[derive(Clone, Debug)]
struct Published<T> {
    value: T,
    read_at: Timestamp,
    at: Instant,
}

type Slot<T> = Arc<Mutex<Option<Published<T>>>>;

/// What the decision thread takes from the poller at a boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireReading<T> {
    /// The reading to apply: the last one published if it is within the
    /// bound, and `Unreadable` otherwise.
    pub value: T,
    /// When the last publication was read, if there has been one — present
    /// beside an `Unreadable` value too, so a report can say how long the
    /// reader has been silent.
    pub read_at: Option<Timestamp>,
}

/// The halt flag's reading and, on a node with one, the region wire's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Readings {
    pub halt: WireReading<PolledHalt>,
    pub region: Option<WireReading<RegionOutlook>>,
}

/// The threads that read §46.2's polled halt flag and §36.3's region wire,
/// and the decision thread's view of what they last read.
///
/// One thread per wire, so a region wire on a hung mount suspends mirrors
/// and leaves the halt flag readable, and a hung halt flag halts without
/// blinding the region wire. The decision thread reads only the published
/// slot; the only file it could block on is one it never opens.
#[derive(Debug)]
pub struct Poller {
    flag: HaltFlag,
    halt: Slot<PolledHalt>,
    region: Option<Slot<RegionOutlook>>,
    bound: StdDuration,
    stop: Arc<AtomicBool>,
}

impl Poller {
    /// Start reading `flag`, and `wire` when given, every `interval`,
    /// trusting a reading for at most `bound`.
    ///
    /// Refuses a zero interval, a bound no longer than the interval — every
    /// healthy reader would read as stale between two of its own reads — and
    /// a bound past [`MAX_STALENESS_BOUND`].
    pub fn spawn(
        flag: HaltFlag,
        wire: Option<DarkRegionWire>,
        interval: StdDuration,
        bound: StdDuration,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        if interval.is_zero() {
            return Err(Error::invalid(
                "a poller interval of zero is a busy loop on the flag's mount; name a period",
            ));
        }
        if bound <= interval {
            return Err(Error::invalid(format!(
                "a staleness bound of {bound:?} is no longer than the {interval:?} interval, so \
                 a healthy poller would read as stale between its own reads; make the bound \
                 several intervals"
            )));
        }
        if bound > MAX_STALENESS_BOUND {
            return Err(Error::invalid(format!(
                "a staleness bound of {bound:?} is past the {MAX_STALENESS_BOUND:?} a halt \
                 reading may be trusted for; a dead reader would hold the cell released that \
                 long"
            )));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let halt: Slot<PolledHalt> = Arc::new(Mutex::new(None));
        let reader = flag.clone();
        spawn_reader(
            "qip-halt-poller",
            move || reader.read(),
            Arc::clone(&halt),
            Arc::clone(&clock),
            interval,
            Arc::clone(&stop),
        )?;
        let region = match wire {
            Some(wire) => {
                let slot: Slot<RegionOutlook> = Arc::new(Mutex::new(None));
                spawn_reader(
                    "qip-region-poller",
                    move || wire.read(),
                    Arc::clone(&slot),
                    clock,
                    interval,
                    Arc::clone(&stop),
                )
                .inspect_err(|_| {
                    // The halt reader is already running; without this it
                    // would read the flag forever for a poller nobody holds.
                    stop.store(true, Ordering::Release);
                })?;
                Some(slot)
            }
            None => None,
        };
        Ok(Self {
            flag,
            halt,
            region,
            bound,
            stop,
        })
    }

    pub fn flag(&self) -> &HaltFlag {
        &self.flag
    }

    /// What the poller last published, each reading replaced by
    /// `Unreadable` when it is older than the bound, was never published,
    /// or cannot be taken because its reader panicked.
    pub fn latest(&self) -> Readings {
        let halt = match fresh(&self.halt, self.bound, "halt flag") {
            Ok(published) => WireReading {
                value: published.value,
                read_at: Some(published.read_at),
            },
            Err((reason, read_at)) => WireReading {
                value: PolledHalt::Unreadable(reason),
                read_at,
            },
        };
        let region =
            self.region
                .as_ref()
                .map(|slot| match fresh(slot, self.bound, "region wire") {
                    Ok(published) => WireReading {
                        value: published.value,
                        read_at: Some(published.read_at),
                    },
                    Err((reason, read_at)) => WireReading {
                        value: RegionOutlook::Unreadable(reason),
                        read_at,
                    },
                });
        Readings { halt, region }
    }

    /// Apply [`Self::latest`] to the cell and return it, so the pass can
    /// record what it applied. Opens no file.
    pub fn apply(&self, cell: &mut Cell, now: Timestamp) -> Readings {
        let readings = self.latest();
        cell.apply_polled_halt(readings.halt.value.clone(), now);
        if let Some(region) = &readings.region {
            cell.apply_region_outlook(region.value.clone(), now);
        }
        readings
    }
}

impl Drop for Poller {
    /// Tells the readers to stop after their current read. Not joined: a
    /// reader blocked on a hung mount would make the drop hang with it, and
    /// the drop is on the decision thread.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// The published reading if it is within `bound`; otherwise why not, and
/// when the last one was read.
fn fresh<T: Clone>(
    slot: &Slot<T>,
    bound: StdDuration,
    wire: &str,
) -> std::result::Result<Published<T>, (String, Option<Timestamp>)> {
    let Ok(guard) = slot.lock() else {
        return Err((
            format!(
                "the {wire} reader panicked while publishing; the wire's state is unknown and \
                 reads as engaged"
            ),
            None,
        ));
    };
    let Some(published) = guard.as_ref() else {
        return Err((
            format!(
                "the {wire} reader has not published a reading yet; the wire's state is \
                 unknown and reads as engaged"
            ),
            None,
        ));
    };
    let age = Instant::now().saturating_duration_since(published.at);
    if age > bound {
        return Err((
            format!(
                "the {wire} reader last published {age:?} ago, past its {bound:?} bound; a \
                 reader that has stopped is a wire whose state is unknown, and that reads as \
                 engaged"
            ),
            Some(published.read_at),
        ));
    }
    Ok(published.clone())
}

fn spawn_reader<T: Send + 'static>(
    name: &str,
    read: impl Fn() -> T + Send + 'static,
    slot: Slot<T>,
    clock: Arc<dyn Clock>,
    interval: StdDuration,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let value = read();
                let published = Published {
                    value,
                    read_at: clock.now(),
                    at: Instant::now(),
                };
                // A poisoned slot cannot be published into; the thread ends
                // and the decision thread reads the poison as unreadable.
                let Ok(mut guard) = slot.lock() else {
                    return;
                };
                *guard = Some(published);
                drop(guard);
                std::thread::sleep(interval);
            }
        })
        .map(|_| ())
        .map_err(|error| Error::io(format!("cannot start the {name} thread: {error}")))
}

/// Where the start barrier stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    /// The consumer has not caught up yet.
    Waiting,
    /// The consumer fired: control is current and passes may begin.
    CaughtUp,
    /// The consumer ended without firing. The scheduler must not start on
    /// it: a first pass run on control nobody finished replaying is a pass
    /// under a payload the centre may already have replaced.
    Abandoned,
}

/// The firing half of the start barrier: fired once, by the fabric
/// consumer, when it has replayed its control streams to the head. Firing
/// takes it by value, so it cannot fire twice.
#[derive(Debug)]
pub struct CaughtUpSignal {
    sender: SyncSender<()>,
}

impl CaughtUpSignal {
    /// Fire. `false` when the waiting half has already gone and nobody will
    /// read it.
    pub fn fire(self) -> bool {
        self.sender.try_send(()).is_ok()
    }
}

/// The waiting half of the start barrier, held by the scheduler.
#[derive(Debug)]
pub struct CaughtUp {
    receiver: Receiver<()>,
    state: Barrier,
}

impl CaughtUp {
    /// A barrier and the one signal that can lift it.
    pub fn new() -> (CaughtUpSignal, Self) {
        let (sender, receiver) = sync_channel(1);
        (
            CaughtUpSignal { sender },
            Self {
                receiver,
                state: Barrier::Waiting,
            },
        )
    }

    /// Where the barrier stood at the last wait.
    pub const fn state(&self) -> Barrier {
        self.state
    }

    /// Wait at most `timeout` for the signal. Once caught up or abandoned
    /// the answer is final and returned without waiting.
    pub fn wait(&mut self, timeout: StdDuration) -> Barrier {
        if self.state != Barrier::Waiting {
            return self.state;
        }
        self.state = match self.receiver.recv_timeout(timeout) {
            Ok(()) => Barrier::CaughtUp,
            Err(RecvTimeoutError::Timeout) => Barrier::Waiting,
            Err(RecvTimeoutError::Disconnected) => Barrier::Abandoned,
        };
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_consumer_that_ends_without_firing_abandons_the_barrier_rather_than_lifting_it() {
        // The failure: a consumer thread that died mid-replay reads, to a
        // barrier built on "has the channel stopped", exactly like one that
        // finished — and the first pass then runs on half the control.
        let (signal, mut barrier) = CaughtUp::new();
        assert_eq!(
            barrier.wait(StdDuration::from_millis(1)),
            Barrier::Waiting,
            "the premise is a barrier that waits while the consumer is alive"
        );
        drop(signal);
        assert_eq!(
            barrier.wait(StdDuration::from_millis(1)),
            Barrier::Abandoned
        );
    }

    #[test]
    fn a_fired_barrier_stays_caught_up_at_every_later_wait() {
        // One-shot: the channel is empty after the first receive, and a
        // barrier that asked it again would fall back to waiting — or, with
        // the sender gone, to abandoned — after it had already let passes
        // begin.
        let (signal, mut barrier) = CaughtUp::new();
        assert!(signal.fire(), "the premise is a waiter present to hear it");
        assert_eq!(
            barrier.wait(StdDuration::from_millis(50)),
            Barrier::CaughtUp
        );
        assert_eq!(barrier.wait(StdDuration::from_millis(1)), Barrier::CaughtUp);
        assert_eq!(barrier.state(), Barrier::CaughtUp);
    }
}
