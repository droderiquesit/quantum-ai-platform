//! ADR 0100 §6: passes are driven by a timer, not by health probes, so a slow
//! dependency cannot stretch the interval between decisions. SLICE-55.
//!
//! # What the decision thread does, and what it no longer does
//!
//! [`DecisionLoop::turn`] is everything the thread that runs `Cell::work`
//! does, once per tick of its timer:
//!
//! 1. applies the halt flag and region wire as their poller threads last read
//!    them (`control::Poller`) — it opens no file;
//! 2. applies what the mesh worker polled since the last turn
//!    (`mesh::MeshWorker`) — it opens no socket;
//! 3. asks the plan compiler for the plan the fresh payload names and
//!    deploys one already compiled (`control::PlanCompiler`) — it reads no
//!    plan file;
//! 4. applies journal pressure: the worse of the spool gauge's reading and
//!    the bound on what the cell's own journal holds unshipped
//!    ([`JournalBound`]);
//! 5. runs the pass, if the node has a feed;
//! 6. offers what the pass applied — its marker and the market events the
//!    feed published — to the spool writer, without waiting;
//! 7. flushes the journal through `FabricMirror`, which offers and never
//!    waits; a refusal leaves the entries unshipped and in memory;
//! 8. leaves a state delta for the mesh worker and a body for the health
//!    thread, each with `try_lock`;
//! 9. offers this cell's touch, and the legs its gate authorises in a
//!    peer-coordinated cycle, to the peer link through
//!    `qip_peer_link::boundary::ReflexPort` — one `try_send`, after the
//!    flush, so a peer outage can hold neither the journal nor the centre's
//!    downlink (MESH-5050). Receiving peers' frames, framing the edge state
//!    and expiring epochs run on the peer link's own session thread, never
//!    here (ADR 0116): the port is all of Lane 1 this loop holds, and it has
//!    no method that can wait.
//!
//! Until this loop existed every one of those steps ran on the thread that
//! accepted health probes, in the order a probe arrived: the store write
//! behind `Cell::flush`, the mesh exchange's retry ladder, a plan file read
//! and the halt flag's read all stood in front of the next decision, and a
//! node nobody probed never decided at all. Each is now on a thread of its
//! own behind a bounded channel or a slot, and the only waits left on this
//! thread are the pass itself and the sleep to the next tick.
//!
//! # Why the journal has a bound of its own
//!
//! `Cell::flush` drops what it shipped (`CellConfig::with_journal_trimmed_on_ship`),
//! so in a healthy node the journal holds one turn's entries. When the
//! writer or the store behind the relay stalls, the mirror refuses and the
//! entries stay — they are the only copy — and the journal grows for as long
//! as the stall lasts. The spool gauge cannot see that: a stalled store
//! leaves the spool empty and the writer's heartbeat fresh. So the loop
//! reads the unshipped count against [`JournalBound`] and narrows at half of
//! it and halts new exposure at it, under the same journal-pressure wire the
//! spool uses. A halted cell still confirms fills and withdraws expired
//! orders, so the journal can pass the line by those entries, and no further
//! by new exposure.

use crate::arbitrage::ArbitrageInstaller;
use crate::control::{Handoff as ControlHandoff, PlanCompiler, Poller, Readings};
use crate::edge_quotes::cell_offer;
use crate::event_fabric::drain::DrainThread;
use crate::event_fabric::inputs::{Recorded, RecordedInputs};
use crate::event_fabric::mirror::FabricMirror;
use crate::event_fabric::pressure::PressureGauge;
use crate::event_fabric::relay::JournalRelay;
use crate::event_fabric::telemetry::{OutboxTelemetry, PressureState};
use crate::event_fabric::writer::PassInputs;
use crate::feed::{SIMULATED_FEED, SimulatedFeed};
use crate::gateway::NodeGateway;
use crate::health::HealthSnapshot;
use crate::mesh::{MeshHealth, MeshWorker, apply_inbound};
use crate::pass::{PassLog, PassMeter, PassOutcome, PassStats, run_pass};
use crate::reprice::Requoter;
use crate::rollback::{RollbackOutcome, RollbackWire};
use crate::share::RegionShareStatus;
use crate::strategies::StrategyInstaller;
use qip_contracts::market_event::MarketEvent;
use qip_contracts::replay::{AppliedReadings, PassMarker, PressureRecord, WireRecord};
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp, sha256_hex};
use qip_edge::cell::{Cell, PolledHalt, WorkReport};
use qip_edge::pressure::{Exhaustion, JournalPressure, Narrowing};
use qip_edge::region::RegionOutlook;
use qip_peer_link::boundary::ReflexPort;
use std::sync::Arc;
use std::time::Instant;

// --- the clock the gauge reads ---------------------------------------------

/// A wall-anchored monotonic clock: the wall time at construction plus a
/// monotonic elapsed time.
///
/// `PressureGauge` requires a clock that never runs backwards — the wall
/// clock stepped back by NTP reads the writer's heartbeat as from the future
/// and halts a healthy cell, and stepped forward reads it as stale and does
/// the same. `SystemClock` is the wall clock, and `MonotonicClock` counts
/// reads rather than time, so neither will do. Reading it is one monotonic
/// clock read and an addition, which keeps the gauge's read wait-free.
#[derive(Debug)]
pub struct InstantClock {
    anchor: Timestamp,
    origin: Instant,
}

impl InstantClock {
    pub fn anchored_at(anchor: Timestamp) -> Self {
        Self {
            anchor,
            origin: Instant::now(),
        }
    }
}

impl Clock for InstantClock {
    fn now(&self) -> Timestamp {
        // Saturating: an uptime past 292 years is not a case, and a wrapped
        // timestamp would read as a heartbeat from the distant past.
        let elapsed = i64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(i64::MAX);
        Timestamp::from_nanos(self.anchor.as_nanos().saturating_add(elapsed))
    }
}

// --- the journal's own bound -----------------------------------------------

/// The most unshipped entries the cell's journal may hold before new
/// exposure halts, and the half of it at which sizing narrows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalBound {
    narrow_at: usize,
    exhaust_at: usize,
}

impl JournalBound {
    /// A bound of `max_unshipped` entries, narrowing at half.
    ///
    /// Refuses fewer than two, because the narrowing line must sit strictly
    /// below the exhaustion line and above zero — a bound of one would halt
    /// with no narrowing in between, which is the failure RES-062 names.
    pub fn new(max_unshipped: usize) -> Result<Self> {
        if max_unshipped < 2 {
            return Err(Error::invalid(format!(
                "a journal bound of {max_unshipped} unshipped entries leaves no room to narrow \
                 before halting; configure at least two"
            )));
        }
        Ok(Self {
            narrow_at: max_unshipped / 2,
            exhaust_at: max_unshipped,
        })
    }

    pub fn exhaust_at(self) -> usize {
        self.exhaust_at
    }

    pub fn narrow_at(self) -> usize {
        self.narrow_at
    }

    /// What `retained` unshipped entries read as.
    pub fn reading(self, retained: usize) -> JournalPressure {
        if retained >= self.exhaust_at {
            JournalPressure::Exhausted(Exhaustion::OverBudget)
        } else if retained >= self.narrow_at {
            JournalPressure::Narrow(Narrowing::half())
        } else {
            JournalPressure::Normal
        }
    }
}

/// The more restrictive of two readings. Exhausted beats narrow beats
/// normal; between two of one kind the first is kept, so the spool's cause
/// — the more specific finding — wins a tie.
pub fn worse(first: JournalPressure, second: JournalPressure) -> JournalPressure {
    fn rank(pressure: JournalPressure) -> u8 {
        match pressure {
            JournalPressure::Normal => 0,
            JournalPressure::Narrow(_) => 1,
            JournalPressure::Exhausted(_) => 2,
        }
    }
    match (first, second) {
        (JournalPressure::Narrow(a), JournalPressure::Narrow(b)) => {
            if b.multiplier() < a.multiplier() {
                second
            } else {
                first
            }
        }
        _ if rank(second) > rank(first) => second,
        _ => first,
    }
}

// --- the decision thread's end of the outbox ---------------------------------

/// The decision thread's end of the spool: the mirror the journal flushes
/// through, the recorder of what each pass applied, and the gauge.
#[derive(Debug)]
pub struct Outbox {
    pub mirror: FabricMirror,
    pub inputs: RecordedInputs,
    pub gauge: PressureGauge,
    /// The spool writer's session, which every pass marker names.
    pub session: u64,
}

/// Facts about the build and configuration each pass marker records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkerFacts {
    pub cell: String,
    /// sha256 of the cell's configuration as this build prints it.
    pub config_digest: String,
    /// The simulated venue's seed, which fixes its matching stream.
    pub gateway_seed: u64,
    pub binary_version: String,
}

/// What the health body says that the decision thread does not hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthFacts {
    pub cell: String,
    pub region: String,
    pub venues: usize,
    pub halt_flag_path: Option<String>,
    pub storage_target: String,
    pub durable: bool,
    pub started_secs: i64,
}

/// What one turn did.
#[derive(Debug)]
pub struct Turn {
    /// The journal pressure the turn applied.
    pub pressure: JournalPressure,
    /// Whether a pass ran — `false` with no feed, or on a halted cell.
    pub ran_pass: bool,
    /// Entries the flush handed off, or why it was refused.
    pub flushed: std::result::Result<usize, String>,
    /// What the inputs recorder did, on a node that runs passes.
    pub recorded: Option<Recorded>,
    /// Entries the cell's journal holds in memory after the flush.
    pub retained: usize,
    /// Market events the pass applied and handed to the recorder.
    pub captured: usize,
}

/// Every piece the decision thread owns, and the loop over them.
#[derive(Debug)]
pub struct DecisionLoop {
    cell: Cell,
    gateway: NodeGateway,
    feed: Option<SimulatedFeed>,
    requoter: Option<Requoter>,
    installer: Option<ArbitrageInstaller>,
    strategies: Option<StrategyInstaller>,
    plans: Option<(PlanCompiler, ControlHandoff)>,
    outbox: Outbox,
    bound: JournalBound,
    telemetry: Arc<OutboxTelemetry>,
    marker: MarkerFacts,
    poller: Option<Poller>,
    rollback: Option<RollbackWire>,
    mesh: Option<MeshWorker>,
    mesh_health: Option<MeshHealth>,
    peers: Option<ReflexPort>,
    peer_offers_dropped: u64,
    health: Option<(HealthSnapshot, HealthFacts)>,
    relay: Option<JournalRelay>,
    drain: Option<DrainThread>,
    meter: Option<PassMeter>,
    pass_log: PassLog,
    stats: PassStats,
    last_report: WorkReport,
    last_pressure: Option<JournalPressure>,
    last_polled: Option<PolledHalt>,
    last_outlook: Option<RegionOutlook>,
    last_plan: Option<String>,
    refused_flushes: u64,
    turns: u64,
}

impl DecisionLoop {
    /// A loop over `cell` and `gateway`, journaling through `outbox` under
    /// `bound`.
    ///
    /// Refuses a cell built without the journal wire: a loop that measured
    /// pressure it had nowhere to apply would be computing a control and
    /// ignoring it, and a cell whose journal does not trim on ship would
    /// grow without bound however healthy the writer is.
    pub fn new(
        cell: Cell,
        gateway: NodeGateway,
        outbox: Outbox,
        bound: JournalBound,
        telemetry: Arc<OutboxTelemetry>,
        marker: MarkerFacts,
    ) -> Result<Self> {
        if cell.journal_pressure().is_none() {
            return Err(Error::invalid(
                "the decision loop applies journal pressure every turn, and this cell was built \
                 without the journal wire; build it with CellConfig::with_journal_wire",
            ));
        }
        if !cell.journal().trims_on_ship() {
            return Err(Error::invalid(
                "the decision loop's cell keeps every shipped entry in memory, so its journal \
                 grows with uptime; build it with CellConfig::with_journal_trimmed_on_ship",
            ));
        }
        Ok(Self {
            cell,
            gateway,
            feed: None,
            requoter: None,
            installer: None,
            strategies: None,
            plans: None,
            outbox,
            bound,
            telemetry,
            marker,
            poller: None,
            rollback: None,
            mesh: None,
            mesh_health: None,
            peers: None,
            peer_offers_dropped: 0,
            health: None,
            relay: None,
            drain: None,
            meter: None,
            pass_log: PassLog::new(5, qip_core::Duration::from_secs(10))?,
            stats: PassStats::default(),
            last_report: WorkReport::default(),
            last_pressure: None,
            last_polled: None,
            last_outlook: None,
            last_plan: None,
            refused_flushes: 0,
            turns: 0,
        })
    }

    /// Run passes priced off `feed`, requoting with `requoter` when given.
    #[must_use]
    pub fn with_feed(mut self, feed: SimulatedFeed, requoter: Option<Requoter>) -> Self {
        self.feed = Some(feed);
        self.requoter = requoter;
        self
    }

    #[must_use]
    pub fn with_desk(mut self, installer: ArbitrageInstaller) -> Self {
        self.installer = Some(installer);
        self
    }

    /// Deploy the plan the payload names, compiled off this thread by
    /// `compiler` and handed back over `handoff`. Without a compiler the
    /// installer still holds grants for the plan and deploys nothing, which
    /// is what a node with no plan path has always done.
    #[must_use]
    pub fn with_strategies(
        mut self,
        strategies: StrategyInstaller,
        plans: Option<(PlanCompiler, ControlHandoff)>,
    ) -> Self {
        self.strategies = Some(strategies);
        self.plans = plans;
        self
    }

    #[must_use]
    pub fn with_poller(mut self, poller: Poller) -> Self {
        self.poller = Some(poller);
        self
    }

    /// Honour the operator's rollback request (REFLEX-8071) at the start of
    /// every turn, before the pass, so a pass never decides under a payload
    /// the operator has already asked the cell to leave.
    #[must_use]
    pub fn with_rollback(mut self, wire: RollbackWire) -> Self {
        self.rollback = Some(wire);
        self
    }

    #[must_use]
    pub fn with_mesh(mut self, worker: MeshWorker) -> Self {
        self.mesh = Some(worker);
        self
    }

    /// Offer this cell's touch to its peers through `port` every turn.
    #[must_use]
    pub fn with_peers(mut self, port: ReflexPort) -> Self {
        self.peers = Some(port);
        self
    }

    /// The peer link's port, if the node has one.
    pub fn peers(&self) -> Option<&ReflexPort> {
        self.peers.as_ref()
    }

    /// Turns on which the port refused this cell's offer.
    pub fn peer_offers_dropped(&self) -> u64 {
        self.peer_offers_dropped
    }

    #[must_use]
    pub fn with_health(mut self, snapshot: HealthSnapshot, facts: HealthFacts) -> Self {
        self.health = Some((snapshot, facts));
        self
    }

    #[must_use]
    pub fn with_meter(mut self, meter: PassMeter) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Hold the relay and drain threads, so they live as long as the loop
    /// and the health body can say whether each still runs.
    #[must_use]
    pub fn with_threads(mut self, relay: Option<JournalRelay>, drain: Option<DrainThread>) -> Self {
        self.relay = relay;
        self.drain = drain;
        self
    }

    pub fn cell(&self) -> &Cell {
        &self.cell
    }

    pub fn cell_mut(&mut self) -> &mut Cell {
        &mut self.cell
    }

    pub fn gateway_mut(&mut self) -> &mut NodeGateway {
        &mut self.gateway
    }

    pub fn stats(&self) -> &PassStats {
        &self.stats
    }

    pub fn turns(&self) -> u64 {
        self.turns
    }

    /// Flushes the mirror refused because the writer's channel was full or
    /// closed, since start.
    pub fn refused_flushes(&self) -> u64 {
        self.refused_flushes
    }

    /// One turn of the decision thread. See the module documentation for
    /// the order and why each step may not wait.
    pub fn turn(&mut self, now: Timestamp) -> Result<Turn> {
        self.turns = self.turns.saturating_add(1);
        let readings = self.apply_wires(now);
        self.apply_rollback(now);
        self.apply_mesh(now);
        self.apply_plan(now);
        let pressure = self.apply_pressure(now)?;

        let mut ran_pass = false;
        let mut recorded = None;
        let mut captured = 0;
        if let (Some(feed), Some(simulated)) = (self.feed.as_mut(), self.gateway.simulated_mut()) {
            let cell = &mut self.cell;
            let requoter = self.requoter.as_mut();
            let stats = &mut self.stats;
            let pass = || run_pass(cell, simulated, feed, requoter, stats, now);
            let outcome = match &self.meter {
                Some(meter) => meter.measure(pass),
                None => pass(),
            };
            self.pass_log.write(now, &outcome, &mut std::io::stderr());
            match outcome {
                Ok(PassOutcome::Ran { report, .. }) => {
                    self.last_report = *report;
                    ran_pass = true;
                }
                Ok(PassOutcome::Halted { .. }) => self.last_report = WorkReport::default(),
                // The journal holds the refusal; the loop keeps turning so
                // the wires and the flush keep running.
                Err(_) => {}
            }
            let applied = feed.take_captured();
            captured = applied.len();
            let marker = self.marker(now, &readings, pressure, &applied)?;
            // A recorder that cannot hand off — the writer has gone — is
            // reported and the turn goes on to the flush: returning here
            // would also skip the flush and the health publication, and the
            // writer's stopped heartbeat already halts new exposure.
            recorded = match self.outbox.inputs.record(PassInputs { marker, applied }) {
                Ok(outcome) => {
                    if outcome == Recorded::Dropped {
                        self.telemetry.input_gap();
                    }
                    Some(outcome)
                }
                Err(error) => {
                    if self.turns % 100 == 1 {
                        eprintln!(
                            "qip-edge-node: the pass's inputs could not be recorded: {}",
                            error.message()
                        );
                    }
                    None
                }
            };
        }

        let flushed = match self.cell.flush(&mut self.outbox.mirror, now) {
            Ok(count) => Ok(count),
            Err(error) => {
                // The entries are still in the journal, chained, and the next
                // turn offers them again; the journal bound is what acts on a
                // refusal that lasts.
                self.refused_flushes = self.refused_flushes.saturating_add(1);
                if self.refused_flushes % 100 == 1 {
                    eprintln!(
                        "qip-edge-node: the journal could not be handed off ({}); {} entries \
                         wait in memory",
                        error.message(),
                        self.cell.journal().retained()
                    );
                }
                Err(error.message().to_string())
            }
        };
        let retained = self.cell.journal().retained();
        self.telemetry.ring_depth(retained as u64);

        if let Some(mesh) = &self.mesh {
            mesh.offer_delta(self.cell.state_delta(&self.last_report, now));
            if let Some(health) = mesh.health() {
                self.mesh_health = Some(health);
            }
        }
        // The peer mesh, last: after the flush and the centre's delta, so
        // nothing a peer does — or fails to do — stands in front of either.
        if let Some(port) = &self.peers
            && !port.try_publish(cell_offer(&self.cell, now))
        {
            // A full queue or a stopped session: this turn's touch is
            // dropped, never waited for, and the next turn's supersedes it.
            self.peer_offers_dropped = self.peer_offers_dropped.saturating_add(1);
            if self.peer_offers_dropped % 100 == 1 {
                eprintln!(
                    "qip-edge-node: the peer link refused this cell's edge state ({} turns \
                     so far); the cell decides on regardless",
                    self.peer_offers_dropped
                );
            }
        }
        if let Some((snapshot, facts)) = &self.health {
            snapshot.publish(self.health_body(facts, pressure, retained, now));
        }
        Ok(Turn {
            pressure,
            ran_pass,
            flushed,
            recorded,
            retained,
            captured,
        })
    }

    fn apply_rollback(&mut self, now: Timestamp) {
        let Some(wire) = self.rollback.as_mut() else {
            return;
        };
        match wire.apply(&mut self.cell, now) {
            Some(RollbackOutcome::RolledBack { from, to }) => eprintln!(
                "qip-edge-node: rolled the policy back from sequence {from} to {to} at the \
                 operator's request; the centre must ship a sequence above {from} to move on"
            ),
            Some(RollbackOutcome::Refused { from, reason }) => eprintln!(
                "qip-edge-node: the cell refused to roll back from sequence {from}: {reason}"
            ),
            None => {}
        }
    }

    fn apply_wires(&mut self, now: Timestamp) -> Option<Readings> {
        let poller = self.poller.as_ref()?;
        let readings = poller.apply(&mut self.cell, now);
        let halt = &readings.halt.value;
        let unreadable = matches!(halt, PolledHalt::Unreadable(_));
        if unreadable || self.last_polled.as_ref() != Some(halt) {
            eprintln!(
                "qip-edge-node: polled halt flag {}; the cell is {}",
                halt.describe(),
                if self.cell.is_halted() {
                    "halted"
                } else {
                    "running"
                }
            );
        }
        self.last_polled = Some(halt.clone());
        if let Some(region) = &readings.region {
            let outlook = &region.value;
            if matches!(outlook, RegionOutlook::Unreadable(_))
                || self.last_outlook.as_ref() != Some(outlook)
            {
                eprintln!("qip-edge-node: region availability: {}", outlook.describe());
            }
            self.last_outlook = Some(outlook.clone());
        }
        Some(readings)
    }

    fn apply_mesh(&mut self, now: Timestamp) {
        let Some(mesh) = &self.mesh else {
            return;
        };
        for inbound in mesh.take_inbound() {
            let tick = apply_inbound(
                &mut self.cell,
                inbound,
                now,
                self.installer.as_mut(),
                self.strategies.as_mut(),
            );
            if !tick.is_quiet() {
                eprintln!("qip-edge-node: mesh exchange: {tick:?}");
            }
        }
    }

    fn apply_plan(&mut self, now: Timestamp) {
        let (Some(strategies), Some((compiler, handoff))) =
            (self.strategies.as_mut(), self.plans.as_mut())
        else {
            return;
        };
        if let Some(named) = self.cell.compiled_plan(now).cloned()
            && strategies.compiled_digest() != Some(named.digest.as_str())
            && let Err(error) = compiler.ensure(&named)
        {
            eprintln!("qip-edge-node: plan compiler: {}", error.message());
        }
        let boundary = handoff.boundary(&mut self.cell, Some(strategies), now);
        if let Some(plan) = boundary.plan {
            let described = plan.describe();
            if !plan.is_quiet() && self.last_plan.as_deref() != Some(described.as_str()) {
                eprintln!("qip-edge-node: plan: {described}");
            }
            self.last_plan = Some(described);
        }
    }

    fn apply_pressure(&mut self, now: Timestamp) -> Result<JournalPressure> {
        let spool = self.outbox.gauge.read();
        let memory = self.bound.reading(self.cell.journal().retained());
        let pressure = worse(spool.pressure, memory);
        self.cell.apply_journal_pressure(pressure, now)?;
        self.telemetry.pressure(state_of(pressure));
        if self.last_pressure != Some(pressure) {
            eprintln!(
                "qip-edge-node: journal pressure {} (spool {}, unshipped journal {} of {})",
                pressure.describe(),
                spool.pressure.describe(),
                self.cell.journal().retained(),
                self.bound.exhaust_at
            );
        }
        self.last_pressure = Some(pressure);
        Ok(pressure)
    }

    /// The marker of the pass just run: every field read from something
    /// this turn applied or this build is, so a replay of the same inputs
    /// against the same build reaches the same marker.
    fn marker(
        &self,
        now: Timestamp,
        readings: &Option<Readings>,
        pressure: JournalPressure,
        applied: &[MarketEvent],
    ) -> Result<PassMarker> {
        let tape = serde_json::to_value(applied)?;
        Ok(PassMarker {
            cell: self.marker.cell.clone(),
            session: self.outbox.session.to_string(),
            pass: self.turns,
            now_ns: now.as_nanos(),
            tape_digest: sha256_hex(canonical_json(&tape).as_bytes()),
            tape_from: applied.first().map_or(0, |event| event.origin().sequence),
            tape_to: applied.last().map_or(0, |event| event.origin().sequence),
            // Control reaches this node over the mesh, which carries no
            // fabric position; a fabric control consumer fills this.
            control: Vec::new(),
            readings: AppliedReadings {
                journal_pressure: Some(PressureRecord::new(
                    state_of(pressure).as_str(),
                    pressure.describe(),
                )),
                halt_flag: readings.as_ref().map(|readings| {
                    let halt = &readings.halt.value;
                    WireRecord::new(halt_word(halt), halt.describe())
                }),
                region_wire: readings
                    .as_ref()
                    .and_then(|readings| readings.region.as_ref())
                    .map(|region| {
                        WireRecord::new(region_word(&region.value), region.value.describe())
                    }),
            },
            config_digest: self.marker.config_digest.clone(),
            plan_digest: self
                .strategies
                .as_ref()
                .and_then(StrategyInstaller::compiled_digest)
                .unwrap_or_default()
                .to_string(),
            gateway_seed: self.marker.gateway_seed,
            binary_version: self.marker.binary_version.clone(),
        })
    }

    fn health_body(
        &self,
        facts: &HealthFacts,
        pressure: JournalPressure,
        retained: usize,
        now: Timestamp,
    ) -> String {
        let cell = &self.cell;
        // The fencing epoch, when the node runs as one of a pair: a standby
        // reads `fenced` or `unowned` here and an operator can see which
        // process owns the venue session without reading the lease file.
        let fence = cell.fence().map_or_else(
            || "null".to_string(),
            |fence| {
                let status = fence.status(now);
                format!(
                    r#"{{"state":"{}","admits":{}}}"#,
                    crate::standby::fence_word(&status),
                    status.admits()
                )
            },
        );
        let gateway = &self.gateway;
        let stats = &self.stats;
        let mesh = match self.mesh_health {
            Some(health) => format!(
                r#"{{"peer_circuit":"{}","deltas_delivered":{},"deltas_dead_lettered":{},"circuit_refusals":{},"grants_verified":{},"grants_refused":{},"grants_duplicate":{}}}"#,
                health.circuit.as_str(),
                health.uplink.delivered,
                health.uplink.dead_lettered,
                health.uplink.circuit_refusals,
                health.downlink.verified,
                health.downlink.refused,
                health.downlink.duplicates,
            ),
            None => "null".to_string(),
        };
        let halt_flag = match &facts.halt_flag_path {
            Some(path) => format!(
                r#"{{"path":"{}","engaged":{}}}"#,
                path,
                cell.polled_halt().is_some()
            ),
            None => "null".to_string(),
        };
        let peer_mesh = match &self.peers {
            // Read from atomics the session thread stores, so it is one
            // session turn behind the wire; the figures are diagnostic and
            // nothing decides on them (ADR 0116, "What it costs").
            Some(port) => {
                let status = port.status();
                format!(
                    r#"{{"edge_states_held":{},"refusals":{},"sent":{},"send_failures":{},"queued_drops":{},"received":{},"inbound_drops":{},"publish_failures":{},"dropped_offers":{},"quotes_refused":{},"quotes_withheld":{},"session_turns":{},"clocks_measured":{},"cycles_originated":{},"reservations":{},"declines":{},"fires":{},"legs_sent":{},"fills":{},"unwinds":{},"cycles_rejected":{}}}"#,
                    status.edge_peers_held,
                    status.refusals,
                    status.sent,
                    status.send_failures,
                    status.wire_drops,
                    status.frames_received,
                    status.inbound_drops,
                    status.publish_failures,
                    status.dropped_offers,
                    status.quotes_refused,
                    status.quotes_withheld,
                    status.turns,
                    status.clocks_measured,
                    status.cycles_originated,
                    status.reservations,
                    status.declines,
                    status.fires,
                    status.legs_sent,
                    status.fills,
                    status.unwinds,
                    status.cycles_rejected,
                )
            }
            None => "null".to_string(),
        };
        let pass = match &self.feed {
            Some(feed) => format!(
                r#"{{"feed":"{}","instruments":{},"instruments_omitted":{},"passes":{},"halted_turns":{},"refusals":{},"signals":{},"orders":{},"fills":{},"expired":{},"repriced":{},"breaks":{},"edges_repriced":{},"edges_unchanged":{},"capture_failures":{}}}"#,
                SIMULATED_FEED,
                feed.tracked(),
                feed.omitted_total(),
                stats.passes,
                stats.halted,
                stats.refusals,
                stats.signals,
                stats.orders,
                stats.fills,
                stats.expired,
                stats.repriced,
                stats.breaks,
                stats.edges_repriced,
                stats.edges_unchanged,
                feed.capture_failures(),
            ),
            None => "null".to_string(),
        };
        let region_allocation_free = cell
            .region_allocation_free()
            .map_or_else(|| "null".to_string(), |free| format!("\"{free}\""));
        let region_share = RegionShareStatus::of(cell).to_json();
        let (shipped, store_failures, relay_running) = match &self.relay {
            Some(relay) => (
                relay.shipped_entries(),
                relay.store_failures(),
                relay.is_running(),
            ),
            None => (0, 0, false),
        };
        let drain_running = self.drain.as_ref().is_some_and(DrainThread::is_running);
        format!(
            r#"{{"cell":"{}","region":"{}","halted":{},"fence":{fence},"peer_mesh":{peer_mesh},"halt_flag":{halt_flag},"arbitrage_desk":{},"live_capable":{},"venues":{},"region_allocation_free":{region_allocation_free},"region_share":{region_share},"strategies":{},"journal_entries":{},"journal_retained":{retained},"journal_shipped":{shipped},"journal_pressure":"{}","journal_handoffs_refused":{},"store_failures":{store_failures},"relay_running":{relay_running},"drain_running":{drain_running},"turns":{},"storage":"{}","durable":{},"gateway":{{"class":"{}","venue":"{}","submitted":{},"rejected":{},"reaches_a_socket":{},"unknown_orders":{}}},"pass":{pass},"mesh":{mesh},"started_at":{}}}"#,
            facts.cell,
            facts.region,
            cell.is_halted(),
            cell.arbitrage().is_some(),
            cell.autonomy().ceiling().is_live(),
            facts.venues,
            cell.deployed_strategies().len(),
            cell.journal().len(),
            pressure.describe(),
            self.refused_flushes,
            self.turns,
            facts.storage_target,
            facts.durable,
            gateway.class(),
            gateway.venue(),
            gateway.submitted_count(),
            gateway.rejected_count(),
            gateway.reaches_a_socket(),
            gateway.unknown_orders(),
            facts.started_secs,
        )
    }
}

fn state_of(pressure: JournalPressure) -> PressureState {
    match pressure {
        JournalPressure::Normal => PressureState::Normal,
        JournalPressure::Narrow(_) => PressureState::Narrow,
        JournalPressure::Exhausted(_) => PressureState::Exhausted,
    }
}

fn halt_word(halt: &PolledHalt) -> &'static str {
    match halt {
        PolledHalt::Absent => "absent",
        PolledHalt::Released => "released",
        PolledHalt::Engaged(_) => "engaged",
        PolledHalt::Unreadable(_) => "unreadable",
    }
}

fn region_word(outlook: &RegionOutlook) -> &'static str {
    match outlook {
        RegionOutlook::AllLit => "all_lit",
        RegionOutlook::Dark(_) => "dark",
        RegionOutlook::Unreadable(_) => "unreadable",
    }
}
