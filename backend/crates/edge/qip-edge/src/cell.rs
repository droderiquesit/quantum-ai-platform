//! The cell: one region's hot execution path, assembled.
//!
//! Bytes arrive on a feed and leave as orders, without a network hop to the
//! central plane anywhere in between. That is the whole point of the cell and
//! the reason every safety property here has to be local: there is nobody to
//! ask.
//!
//! What makes it safe is that the cell never decides *how much* it may risk.
//! It receives a [`crate::VerifiedEnvelope`] — signed, bounded, venue-scoped,
//! expiring — and the worst it can do while cut off is spend an amount
//! somebody already approved, for as long as the envelope has left to run.

use crate::arbitrage::ArbitrageDesk;
use crate::dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
use crate::envelope::VerifiedEnvelope;
use crate::feasibility::{self, VenueModel};
use crate::journal::{Decision, Journal, Mirror};
use crate::mesh::{CellStateDelta, DeltaOrder, DeltaRefusal, StrategyUtilisation};
use crate::policy::{VerifiedHalt, VerifiedPolicy};
use crate::seam::CellLiquidity;
use crate::telemetry::CellMetrics;
use qip_arbitrage::scan::{Opportunity, RejectionStage};
use qip_contracts::capital::{CapitalGrant, Utilisation};
use qip_contracts::degradation::{DegradationState, StrategyClass};
use qip_contracts::intent::{Contributor, CycleLeg, Intent, NetIntent, net, netting_ratio};
use qip_contracts::message::{BookSide, MarketMessage};
use qip_contracts::signal::{Signal, SignalKind, StrategyId};
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_feature_dag::engine::FeatureEngine;
use qip_orderbook::venue::VenueState;
use qip_protocols::registry::{FeedKey, ProtocolRegistry};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel};
use qip_sequencing::tracker::{ReorderPolicy, Sequencer};
use qip_strategy::compile::CompiledStrategy;
use qip_strategy::program::Program;
use qip_strategy::runtime::StrategyRuntime;
use std::collections::BTreeMap;

/// How a cell is identified and what it is allowed to reach.
#[derive(Clone, Debug)]
pub struct CellConfig {
    pub cell_id: String,
    pub region: String,
    /// Venues this cell may trade. A venue absent here is unreachable to it
    /// whatever an envelope says — the two are independent bounds, and an
    /// order must clear both.
    pub venues: Vec<VenueId>,
    /// How long a book may go unrefreshed before its prices stop counting.
    pub max_staleness: Duration,
    /// The runtime node budget a strategy may not exceed.
    pub strategy_budget: usize,
    /// What the cell knows about executing at each venue, keyed by venue id
    /// (blueprint §18.1). A venue absent here is judged for depth alone —
    /// see [`crate::feasibility`] for why that is stated rather than
    /// defaulted.
    pub feasibility: BTreeMap<String, VenueModel>,
}

impl CellConfig {
    pub fn new(cell_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            cell_id: cell_id.into(),
            region: region.into(),
            venues: Vec::new(),
            max_staleness: Duration::from_secs(5),
            strategy_budget: 4_096,
            feasibility: BTreeMap::new(),
        }
    }

    pub fn with_venue(mut self, venue: VenueId) -> Self {
        self.venues.push(venue);
        self
    }

    /// Install the feasibility model for a venue.
    ///
    /// The model is keyed by the venue's id and read on every intent for that
    /// venue; installing one for a venue the cell cannot reach is harmless
    /// and installing none for a venue it can is the depth-only case.
    #[must_use]
    pub fn with_feasibility(mut self, venue: &VenueId, model: VenueModel) -> Self {
        self.feasibility.insert(venue.as_str().to_string(), model);
        self
    }
}

/// What one pass of the cell's work produced.
#[derive(Clone, Debug, Default)]
pub struct WorkReport {
    pub signals: Vec<Signal>,
    pub orders: Vec<PlacedOrder>,
    /// Nets that cancelled to zero: strategies that wanted opposite things,
    /// whose disagreement never reached a venue. Recorded because a
    /// cancellation is an outcome the platform should be able to explain, not
    /// an absence.
    pub cancelled: Vec<NetIntent>,
    /// Gross intent over net order volume, per blueprint §27 — the single
    /// best summary of whether the strategy set has genuine diversity. `None`
    /// when everything cancelled, because the ratio is unbounded there and a
    /// sentinel would be a number nobody computed.
    pub netting_ratio: Option<f64>,
    /// Every gate that said no, and why. A cell must answer "why did nothing
    /// trade" as precisely as "why did this trade".
    pub refusals: Vec<(String, String)>,
    /// Every internal cross booked this pass (§27.1). A cross is a trade
    /// between two of the platform's own strategies; it is reported rather
    /// than merely journaled so a caller can see it without replaying the
    /// chain.
    pub crosses: Vec<InternalCross>,
    pub halted: bool,
}

/// One offsetting portion crossed inside the cell rather than at a venue.
///
/// §27.1: the price is the prevailing mid at the netting instant, "never a
/// price either side chose", and both sides are named because the blueprint
/// treats a cross as a ledger entry and a regulatory expectation rather than
/// an optimisation detail.
#[derive(Clone, Debug, PartialEq)]
pub struct InternalCross {
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The matched size — the smaller of the buying and selling sides, which
    /// is exactly how much never needed a venue.
    pub quantity: Decimal,
    /// The prevailing mid at the netting instant, read from the book rather
    /// than taken from any intent's own reference price.
    pub price: Decimal,
    pub bought: Vec<StrategyId>,
    pub sold: Vec<StrategyId>,
}

/// An order the cell actually sent.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedOrder {
    pub order_id: String,
    /// The largest contributor by absolute intended size, kept so every
    /// existing reader of this field still sees a strategy. It is no longer
    /// the whole truth once an order carries more than one — `contributors`
    /// is — and it is retained rather than removed so the change is additive
    /// at every seam that already reads it.
    pub strategy: StrategyId,
    /// Every strategy whose intent this order carries, and how much each
    /// wanted. This is the mechanism by which a fill remains traceable to the
    /// strategies that caused it after netting has collapsed them into one
    /// order.
    pub contributors: Vec<Contributor>,
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The side of the book the order takes: `Ask` is a buy, `Bid` is a sell.
    /// Every gateway reads it this way, and so does the sign on each
    /// contributor below — a positive share bought. Stated here because the
    /// enum's own names do not say which reading an *order* carries.
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// Set by the cell from the gateway's own answer, never taken from the
    /// order. A paper fill counted as real is the single most consequential
    /// bit in the execution path.
    pub simulated: bool,
}

/// A deployed strategy, the arena its plan indexes into, and the capital it
/// runs under.
///
/// The runtime is per-strategy rather than per-cell. One shared arena would
/// mean a plan compiled against one program being evaluated against another,
/// and the failure mode is not a crash: `NodeRef` is an index, so the run
/// would read whatever node happened to sit at that position and emit a signal
/// derived from a different strategy's arithmetic. Giving each deployment the
/// program it was compiled against costs a few kilobytes and removes the
/// aliasing entirely.
#[derive(Debug)]
struct Deployed {
    strategy: CompiledStrategy,
    runtime: StrategyRuntime,
    envelope: VerifiedEnvelope,
    utilisation: Utilisation,
    /// Which of the degradation table's pause rules apply to this strategy.
    /// Everything deployed through [`Cell::deploy`] is `PriceOnly`, which is
    /// true of every strategy this platform ships today: nothing at the edge
    /// consumes world events, so an ingestion or episodic loss must not pause
    /// it.
    class: StrategyClass,
}

/// One edge cell.
#[derive(Debug)]
pub struct Cell {
    config: CellConfig,
    protocols: ProtocolRegistry,
    sequencer: Sequencer,
    liquidity: CellLiquidity,
    features: FeatureEngine,
    deployed: BTreeMap<String, Deployed>,
    autonomy: AutonomyController,
    /// The last verified policy payload applied, if any ever was.
    ///
    /// `None` is not a neutral state: with no payload every payload-fed
    /// capability reads as unavailable and the cell sizes at its conservative
    /// floor. A cell nobody ships policy to trades small, not blind.
    policy: Option<VerifiedPolicy>,
    /// Whether the centre has halted this cell through policy. Separate from
    /// the local kill switch on purpose: the switch clears only with an
    /// operator credential, while this clears only with a newer verified
    /// payload saying it is over. Two halts, two release disciplines, and
    /// neither can release the other.
    policy_halted: bool,
    /// The instant of the newest halt applied. A payload releases the policy
    /// halt only if it was issued *after* this, so a pre-halt payload still in
    /// flight cannot un-halt the cell it was racing.
    policy_halt_barrier: Option<Timestamp>,
    dropcopy: DropCopyReconciler,
    /// The arbitrage desk, if the composition root installed one. `None` is
    /// a cell that runs strategy programs and scans no graph, which is every
    /// cell before this field existed and every test that does not ask for
    /// one.
    desk: Option<ArbitrageDesk>,
    journal: Journal,
    fills: Vec<CellFill>,
    /// Every disagreement between this cell's fills and the venue's own
    /// account, kept so the centre hears about it in the state delta as well as
    /// in the journal.
    ///
    /// Bounded, like everything else that grows on a signal from outside. It
    /// can only grow while an operator keeps resuming a cell that a break has
    /// already halted, and past the bound the count is what travels: a
    /// truncation nobody can see would understate an incident.
    breaks: Vec<String>,
    breaks_omitted: u32,
    order_sequence: u64,
    /// Where the cell's facts go.
    ///
    /// Given, never reached for: a cell assembled without one records into a
    /// registry nobody reads, which is what every test in the tree does. See
    /// [`crate::telemetry`] for why nothing here can block or fail the pass.
    metrics: CellMetrics,
}

/// How many reconciliation breaks a cell keeps for reporting.
const MAX_RETAINED_BREAKS: usize = 32;

impl Cell {
    /// Assemble a cell.
    ///
    /// The autonomy ceiling is paper trading and there is no constructor that
    /// takes another. A cell cannot raise its own ceiling; a live-capable cell
    /// is a differently-assembled deployment the central plane signs off, and
    /// the absence of that constructor here is what makes the claim true
    /// rather than merely intended.
    pub fn new(config: CellConfig, features: FeatureEngine) -> Result<Self> {
        Ok(Self {
            protocols: ProtocolRegistry::new(),
            sequencer: Sequencer::new(ReorderPolicy::default()),
            liquidity: CellLiquidity::new(),
            features,
            deployed: BTreeMap::new(),
            autonomy: AutonomyController::new(),
            policy: None,
            policy_halted: false,
            policy_halt_barrier: None,
            dropcopy: DropCopyReconciler::new(),
            desk: None,
            journal: Journal::new(),
            fills: Vec::new(),
            breaks: Vec::new(),
            breaks_omitted: 0,
            order_sequence: 0,
            metrics: CellMetrics::silent(),
            config,
        })
    }

    pub fn config(&self) -> &CellConfig {
        &self.config
    }

    /// Record into the composition root's registry rather than the silent one
    /// this cell was built with.
    ///
    /// Called once, in `qip-edge-node`, with the handle taken from the
    /// telemetry before it is used anywhere else — exactly as `qip-fastbrain`
    /// and `qip-deepbrain` install theirs. Taking a second registry here would
    /// produce a scrape surface that answers empty forever while the cell
    /// records diligently into one nothing can reach, which is the defect this
    /// seam exists to close rebuilt one level up.
    ///
    /// The halt gauge is written immediately so a cell that starts halted, and
    /// is scraped before its first pass, does not read as running.
    #[must_use]
    pub fn with_metrics(mut self, metrics: std::sync::Arc<qip_observability::Metrics>) -> Self {
        self.metrics = CellMetrics::new(metrics, &self.config.cell_id, &self.config.region);
        self.record_halt();
        self
    }

    /// Install the arbitrage desk this cell scans with.
    ///
    /// A builder rather than a constructor argument, like [`Self::with_metrics`],
    /// so [`Self::new`] stays the one way to assemble a cell and stays
    /// paper-only. Refused when the desk's envelope names another cell, or
    /// when its graph reaches a venue this cell may not: a cycle is priced
    /// against the cell's own books, and a venue absent from the cell's list
    /// has no book here to price against and no gateway here to send to.
    pub fn with_arbitrage(mut self, desk: ArbitrageDesk) -> Result<Self> {
        if desk.envelope().cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot fund the arbitrage desk at {}",
                desk.envelope().cell(),
                self.config.cell_id
            )));
        }
        for edge in desk.graph().edges() {
            for venue in [&edge.from.venue, &edge.to.venue] {
                if !self.config.venues.contains(venue) {
                    return Err(Error::denied(format!(
                        "conversion {} reaches {}, which this cell may not trade; a cycle \
                         through a venue the cell holds no book for cannot be priced here",
                        edge.label(),
                        venue.as_str()
                    )));
                }
            }
        }
        self.desk = Some(desk);
        Ok(self)
    }

    /// The installed arbitrage desk, if any.
    pub fn arbitrage(&self) -> Option<&ArbitrageDesk> {
        self.desk.as_ref()
    }

    /// Publish the halt state as it now stands.
    ///
    /// Called wherever either halt can change, rather than once per pass: a
    /// cell halted by a reconciliation break stops running passes, so a gauge
    /// written only inside `work` would never report the halt that stopped it.
    fn record_halt(&self) {
        self.metrics.halt(
            self.autonomy.kill_switch().is_globally_tripped(),
            self.policy_halted,
        );
    }

    pub fn protocols_mut(&mut self) -> &mut ProtocolRegistry {
        &mut self.protocols
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// The registry every series this cell records lands in.
    ///
    /// Exposed so a composition root's test can prove, by pointer identity,
    /// that it is the same registry the scrape surface serves. A cell that
    /// records into one registry while the health thread serves another
    /// answers every scrape empty forever, and nothing at runtime reports it.
    pub fn metrics_registry(&self) -> &std::sync::Arc<qip_observability::Metrics> {
        self.metrics.registry()
    }

    pub fn autonomy(&self) -> &AutonomyController {
        &self.autonomy
    }

    pub fn autonomy_mut(&mut self) -> &mut AutonomyController {
        &mut self.autonomy
    }

    pub fn liquidity(&self) -> &CellLiquidity {
        &self.liquidity
    }

    pub fn dropcopy_mut(&mut self) -> &mut DropCopyReconciler {
        &mut self.dropcopy
    }

    pub fn fills(&self) -> &[CellFill] {
        &self.fills
    }

    /// Whether the cell is stopped.
    pub fn is_halted(&self) -> bool {
        self.autonomy.kill_switch().is_globally_tripped() || self.policy_halted
    }

    /// The degradation narrowing currently in force, derived from the applied
    /// policy payload.
    ///
    /// With no policy this is [`DegradationState::nothing_known`], which reads
    /// every payload-fed capability as unavailable — the fail-closed floor.
    /// Ingestion is deliberately not observed here: the cell's book-staleness
    /// seam already refuses to route on a stale book, per book, which is
    /// stricter than the capability-level pause would be.
    pub fn narrowing(&self, now: Timestamp) -> DegradationState {
        match &self.policy {
            Some(policy) => policy.payload().narrowing(now),
            None => DegradationState::nothing_known(),
        }
    }

    /// The sequence of the applied policy, if any.
    pub fn policy_sequence(&self) -> Option<u64> {
        self.policy.as_ref().map(VerifiedPolicy::sequence)
    }

    /// Apply a verified halt command.
    ///
    /// Engage-only and idempotent: there is no release command, because
    /// release is a fresh policy decision and rides a newer signed payload
    /// issued after the barrier this records. Applying the same halt twice is
    /// one halt.
    pub fn apply_halt(&mut self, halt: VerifiedHalt, now: Timestamp) {
        // A halt at or behind the barrier of one already resolved is a
        // replay: a captured frame re-delivered after a legitimate release
        // would otherwise re-halt the cell in the gaps between publishes — a
        // bounded denial of service in the safe direction, but free to
        // remove. The asymmetry is preserved with care: a *fresh* halt is
        // accepted unconditionally, and an already-halted cell is never
        // released by this path — refusing the replay below leaves it exactly
        // as halted as it was.
        if !self.policy_halted
            && self
                .policy_halt_barrier
                .is_some_and(|barrier| halt.issued_at() <= barrier)
        {
            self.journal.record(
                Decision::Refused {
                    gate: "halt_replay".to_string(),
                    reason: format!(
                        "a halt issued at {} is at or behind the resolved barrier and does not \
                         re-halt this cell",
                        halt.issued_at()
                    ),
                },
                now,
            );
            return;
        }
        let barrier = match self.policy_halt_barrier {
            Some(existing) if existing >= halt.issued_at() => existing,
            _ => halt.issued_at(),
        };
        self.policy_halt_barrier = Some(barrier);
        if !self.policy_halted {
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: format!("central halt: {}", halt.reason()),
                },
                now,
            );
        }
        self.policy_halted = true;
        self.record_halt();
    }

    /// Apply a verified policy payload by atomic swap.
    ///
    /// "Atomic" in a single-threaded cell means one assignment and never a
    /// partial application: the payload was verified whole before this became
    /// callable — [`VerifiedPolicy`]'s only constructor recomputes the
    /// signature — and nothing below reads a slot before the swap. Trading is
    /// never paused; the previous policy serves until the assignment.
    ///
    /// Sequence discipline lives here because this is where "last applied" is
    /// a fact: a payload at or below the applied sequence is refused, which is
    /// what stops a replayed old payload from un-halting or re-widening the
    /// cell.
    pub fn apply_policy(&mut self, verified: VerifiedPolicy, now: Timestamp) -> Result<()> {
        if verified.payload().cell != self.config.cell_id {
            return Err(Error::denied(format!(
                "a policy payload for cell {} cannot apply to {}",
                verified.payload().cell,
                self.config.cell_id
            )));
        }
        if let Some(applied) = self.policy_sequence()
            && verified.sequence() <= applied
        {
            return Err(Error::denied(format!(
                "policy sequence {} is not newer than the applied {applied}; an old payload \
                 cannot re-widen or un-halt this cell",
                verified.sequence()
            )));
        }

        // A payload that would release the halt must postdate the halt it
        // releases. `halting` is what the cell will actually do, which may be
        // stricter than what the payload says.
        let releasing_too_early = !verified.halted()
            && self.policy_halted
            && self
                .policy_halt_barrier
                .is_some_and(|barrier| verified.payload().issued_at <= barrier);
        let halting = verified.halted() || releasing_too_early;
        let sequence = verified.sequence();
        let was_halted = self.policy_halted;
        let narrowed: Vec<String> = verified
            .payload()
            .narrowing(now)
            .narrowed()
            .iter()
            .map(|(capability, freshness)| {
                format!("{}:{}", capability.as_str(), freshness.as_str())
            })
            .collect();

        self.journal.record(
            Decision::PolicyApplied {
                sequence: verified.sequence(),
                halted: halting,
                narrowed,
            },
            now,
        );
        if releasing_too_early {
            self.journal.record(
                Decision::Refused {
                    gate: "halt_release".to_string(),
                    reason: format!(
                        "policy sequence {} was issued at or before the halt barrier and cannot \
                         release it",
                        verified.sequence()
                    ),
                },
                now,
            );
        }
        if halting != was_halted {
            self.journal.record(
                Decision::HaltChanged {
                    halted: halting,
                    reason: if halting {
                        "the centre halted this cell through policy".to_string()
                    } else {
                        format!(
                            "policy sequence {} released the central halt",
                            verified.sequence()
                        )
                    },
                },
                now,
            );
        }
        if verified.halted() {
            // A halt carried by policy is a halt decision like any other, and
            // it raises the same release barrier: whatever releases it must
            // postdate it, whatever its sequence says.
            self.policy_halt_barrier = Some(match self.policy_halt_barrier {
                Some(existing) if existing >= verified.payload().issued_at => existing,
                _ => verified.payload().issued_at,
            });
        }
        self.policy_halted = halting;
        self.policy = Some(verified);
        self.record_halt();
        // The sequence the cell has *applied*, recorded once the swap has
        // happened. Recording it before would publish a payload the cell might
        // still have refused.
        self.metrics.policy_applied(sequence);
        Ok(())
    }

    /// Track an instrument at a venue.
    pub fn track(&mut self, state: VenueState) {
        self.liquidity.insert(state);
    }

    /// Deploy a strategy, the program its plan indexes into, and the verified
    /// capital envelope it runs under.
    ///
    /// The envelope is the verified type, so a strategy cannot be deployed
    /// against a grant nobody signed. The program is taken here rather than at
    /// assembly for the reason that made this call worth changing: a cell used
    /// to be constructed with an empty arena, so a strategy whose plan pointed
    /// into a real one could be deployed, accepted, and then refuse on every
    /// pass of `work` — a cell that looked healthy, held a strategy, and could
    /// not evaluate it. Every reason that can be established without the market
    /// is established here instead, and a deployment that returns `Ok` is one
    /// the cell can actually run:
    ///
    /// * the envelope names this cell,
    /// * the program is internally consistent,
    /// * every node the plan names exists in that program,
    /// * the strategy fits the cell's evaluation budget.
    ///
    /// What deliberately is *not* checked here is whether the feature engine
    /// will produce the inputs the strategy reads. That depends on the market —
    /// a feature can be registered and still undefined for want of a quote —
    /// so it stays a per-pass judgement the runtime makes against the vector it
    /// was actually handed.
    pub fn deploy(
        &mut self,
        strategy: CompiledStrategy,
        program: Program,
        envelope: VerifiedEnvelope,
    ) -> Result<()> {
        if envelope.cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot deploy into {}",
                envelope.cell(),
                self.config.cell_id
            )));
        }
        if envelope.strategy() != strategy.id() {
            return Err(Error::denied(format!(
                "an envelope for strategy {} cannot deploy {}",
                envelope.strategy().as_str(),
                strategy.id().as_str()
            )));
        }

        // `NodeRef` is an index. A plan naming a node the arena does not hold
        // is the case where an out-of-range read would be the *lucky* outcome:
        // in a larger arena the index resolves, to a node belonging to some
        // other strategy, and the cell emits a signal computed from arithmetic
        // nobody wrote for it.
        program.validate()?;
        for node in strategy.plan() {
            if program.node(*node).is_none() {
                return Err(Error::invalid(format!(
                    "strategy {} plans node {} and the program it was deployed \
                     with holds {} node(s); the plan and the program do not \
                     belong together",
                    strategy.id().as_str(),
                    node.index(),
                    program.len()
                )));
            }
        }

        // `with_budget` refuses a program it could not evaluate in bounded
        // time. Doing it here means an over-budget strategy is refused by the
        // deployment that shipped it rather than silently, later, by a market
        // that moved.
        let runtime = StrategyRuntime::with_budget(program, self.config.strategy_budget)?;
        if strategy.cost() > runtime.budget() {
            return Err(Error::guard(format!(
                "strategy {} needs {} nodes and this cell evaluates at most {}",
                strategy.id().as_str(),
                strategy.cost(),
                runtime.budget()
            )));
        }

        self.deployed.insert(
            envelope.strategy().as_str().to_string(),
            Deployed {
                strategy,
                runtime,
                envelope,
                utilisation: Utilisation::default(),
                class: StrategyClass::PriceOnly,
            },
        );
        Ok(())
    }

    /// Declare which pause rules govern an already-deployed strategy.
    ///
    /// Separate from [`Self::deploy`] so that classification is an explicit
    /// act rather than a defaulted parameter nobody reads. `PriceOnly` is the
    /// deploy-time default because it is true of everything shipped today; a
    /// strategy that consumes world events must say so, and saying so is what
    /// makes the degradation table able to pause it.
    pub fn classify(&mut self, strategy: &str, class: StrategyClass) -> Result<()> {
        match self.deployed.get_mut(strategy) {
            Some(deployed) => {
                deployed.class = class;
                Ok(())
            }
            None => Err(Error::invalid(format!(
                "no strategy named {strategy} is deployed in this cell, so there is nothing to \
                 classify"
            ))),
        }
    }

    pub fn deployed_strategies(&self) -> Vec<&str> {
        self.deployed.keys().map(String::as_str).collect()
    }

    // --- the hot path -------------------------------------------------------

    /// Bytes in: decode, sequence, apply, and mark features dirty.
    ///
    /// Does no file or network I/O. The mirror is drained by [`Cell::flush`]
    /// precisely so that this call's cost is arithmetic and memory, never a
    /// storage system's availability.
    pub fn on_bytes(&mut self, feed: &FeedKey, bytes: &[u8], now: Timestamp) -> Result<usize> {
        let (decoded, skipped) = {
            let decoder = self.protocols.decoder_mut(&feed.venue, &feed.feed)?;
            let decoded = decoder.decode(bytes, now)?;
            // Read the counter after decoding: it is cumulative, and the
            // difference is what this call actually skipped.
            let skipped = usize::try_from(decoder.diagnostics().messages_skipped).unwrap_or(0);
            (decoded, skipped)
        };
        let count = decoded.len();
        self.journal.record(
            Decision::Ingested {
                feed: format!("{}/{}", feed.venue.as_str(), feed.feed),
                decoded: count,
                skipped,
            },
            now,
        );

        let batch = self.sequencer.accept(decoded, now);
        self.apply_batch(batch.released, now)?;
        for event in &batch.events {
            if let Some(detail) = gap_detail(event) {
                self.journal.record(
                    Decision::GapDetected {
                        stream: detail.0,
                        detail: detail.1,
                    },
                    now,
                );
            }
        }
        Ok(count)
    }

    /// Apply released messages to books and the feature graph.
    fn apply_batch(&mut self, messages: Vec<MarketMessage>, _now: Timestamp) -> Result<()> {
        for message in &messages {
            let venue = message.origin.venue.clone();
            if let Some(state) = self.liquidity.get_mut(&venue, &message.object_id) {
                // A message the book refuses is a book that would be wrong if
                // it accepted it; the refusal is recorded by the reset path,
                // not swallowed here.
                state.apply(message)?;
            }
            self.features.ingest(message)?;
        }
        Ok(())
    }

    /// One pass of decide-and-act.
    ///
    /// Every gate that refuses records why, in order, so the reason a cell was
    /// quiet is reconstructable without re-running it.
    pub fn work(&mut self, now: Timestamp, gateway: &mut dyn Placer) -> Result<WorkReport> {
        let mut report = WorkReport {
            halted: self.is_halted(),
            ..WorkReport::default()
        };
        // Recorded before the halt check, so a halted cell still counts its
        // passes. A refusal count with no pass count underneath it cannot tell
        // "nothing was refused" from "the cell never ran".
        self.metrics.work_pass();
        self.record_halt();

        if report.halted {
            // Books keep absorbing and the journal keeps recording while
            // halted. A cell that stops seeing the market cannot tell whether
            // it is safe to resume. The gate names which halt is in force,
            // because the two release disciplines are different and an
            // operator staring at a quiet cell needs to know which door to
            // knock on.
            let gate = if self.autonomy.kill_switch().is_globally_tripped() {
                "kill_switch"
            } else {
                "policy_halt"
            };
            self.refuse(&mut report, gate, "the cell is halted", now);
            return Ok(report);
        }

        // The degradation table, consulted once per pass. Everything below
        // reads the same narrowing, so a payload applied mid-pass changes the
        // next pass, never half of this one.
        let narrowing = self.narrowing(now);
        let multiplier = narrowing.sizing_multiplier();
        // Freshness is a function of `now`, so this is the instant it becomes
        // known and the only instant at which the recorded value is what the
        // cell actually sized against. Before this the whole table was
        // formatted into a journal string and discarded.
        self.metrics.narrowing(&narrowing);

        let vector = self.features.evaluate(now)?;
        let strategy_ids: Vec<String> = self.deployed.keys().cloned().collect();
        // Phase one collects; phase two nets; phase three sends. The split is
        // the blueprint's, and §28 is why the per-strategy gates stay in phase
        // one rather than moving onto the net.
        let mut intents: Vec<Intent> = Vec::new();

        for id in strategy_ids {
            // A paused strategy does not evaluate at all. Refusing before the
            // run rather than after keeps the journal honest about why the
            // cell was quiet: no signal existed, because the capability the
            // strategy depends on is gone.
            if let Some(deployed) = self.deployed.get(&id)
                && narrowing.pauses(deployed.class)
            {
                self.refuse(
                    &mut report,
                    "degradation_pause",
                    &format!("strategy {id} pauses while its capability is degraded"),
                    now,
                );
                continue;
            }
            // Each deployment evaluates against the arena it was compiled
            // with. `runtime` and `strategy` are disjoint fields of the same
            // deployment, so the borrow ends with the call and the refusal
            // path below can take `&mut self` to journal why it refused.
            let outcome = match self.deployed.get_mut(&id) {
                Some(deployed) => deployed.runtime.run(&deployed.strategy, &vector, now),
                None => continue,
            };
            let signal = match outcome {
                Ok(Some(signal)) => signal,
                Ok(None) => continue,
                Err(error) => {
                    self.refuse(&mut report, "strategy_runtime", error.message(), now);
                    continue;
                }
            };

            self.journal.record(
                Decision::SignalRaised {
                    strategy: signal.strategy.as_str().to_string(),
                    object: signal.object_id.as_str().to_string(),
                    kind: signal.kind.as_str().to_string(),
                    conviction_shrunk_f64: signal.conviction.shrunk(),
                },
                now,
            );
            self.metrics.signal(signal.kind);
            report.signals.push(signal.clone());

            if let Some(intent) = self.intent_for(&signal, multiplier, now, &mut report)? {
                intents.push(intent);
            }
        }

        // The arbitrage desk, at the seam §27.2 names: after the strategies
        // have asked and before anything is netted, so that legs and
        // directional intents meet at one place — and part company there,
        // because a leg is never netted. Every leg is judged by the same
        // feasibility gate the directional intents meet below, then held
        // until the nets have gone out.
        let cycles = self.scan_cycles(now, multiplier, &narrowing, &mut report)?;

        // The feasibility gate, between collection and netting (§18.1). An
        // intent that cannot execute at its size never enters the netting
        // set: a net built from an infeasible contributor would carry that
        // contributor's share to the venue inside an order whose other
        // contributors were feasible, and the venue's rejection — or the
        // fee's bite — would land on all of them. `retain` judges in place,
        // so the gate allocates nothing per pass.
        intents.retain(|intent| self.admit_feasible(intent, now, &mut report));

        // Phase two. Everything the strategies asked for collapses onto one
        // intent per instrument, venue and representation — so two strategies
        // buying the same thing send one order and pay the spread once, and
        // two wanting opposite things cancel without either reaching the
        // venue. Before this, each strategy placed its own order and the two
        // could cross each other, which is a self-trade: a regulatory problem
        // and a pure loss at the same time.
        let nets = net(intents);
        report.netting_ratio = netting_ratio(&nets);
        // `None` when everything cancelled: the ratio is unbounded there, and
        // observing a sentinel would put a number nobody computed into the
        // distribution. The cancellation is counted in `place_net` instead.
        if let Some(ratio) = report.netting_ratio {
            self.metrics.netting_ratio(ratio);
        }
        for net_intent in &nets {
            if let Some(order) = self.place_net(net_intent, now, gateway, &mut report)? {
                report.orders.push(order);
            }
        }

        // Cycles go out after the nets. Never through `net`: each leg is
        // sent by the same order path a net intent uses, one leg after
        // another in the plan's order, least reversible first.
        for cycle in &cycles {
            self.place_cycle(cycle, now, gateway, &mut report)?;
        }

        Ok(report)
    }

    /// Take one signal through every per-strategy gate to an intent, or
    /// refuse it.
    ///
    /// Phase one of the two the blueprint separates. §28 is explicit that
    /// strategy-level limits are checked *before* netting, "because a strategy
    /// that has exhausted its budget must not contribute to a net intent at
    /// all" — so expiry, venue, book staleness, pricing, the degradation
    /// multiplier and the capital envelope all run here, per strategy, exactly
    /// as they did when this function placed an order directly. No gate was
    /// removed and none was reordered; what changed is that the admitted size
    /// becomes an intent instead of an order.
    fn intent_for(
        &mut self,
        signal: &Signal,
        multiplier: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Result<Option<Intent>> {
        if !signal.is_live(now) {
            self.refuse(report, "signal_expiry", "the signal is no longer live", now);
            return Ok(None);
        }

        let Some(venue) = self.venue_for(&signal.object_id) else {
            self.refuse(
                report,
                "venue_selection",
                "no venue this cell may reach quotes the instrument",
                now,
            );
            return Ok(None);
        };

        // A stale or unpriceable book routes nothing. The book already refuses
        // to serve a mid; routing against one anyway would use a price from
        // before the gap that made it stale.
        // Read everything needed from the book in one borrow, so the refusal
        // path below can take `&mut self` to journal why it refused.
        let assessment = self.liquidity.get(&venue, &signal.object_id).map(|state| {
            (
                state.is_stale(),
                state
                    .reset_reason()
                    .unwrap_or("the book is awaiting resynchronisation")
                    .to_string(),
                state.status(),
                state.mid(),
            )
        });
        let Some((stale, reset_reason, status, mid)) = assessment else {
            self.refuse(
                report,
                "book",
                "the cell holds no book for the instrument",
                now,
            );
            return Ok(None);
        };
        if stale {
            self.refuse(report, "stale_book", &reset_reason, now);
            return Ok(None);
        }
        if !status.accepts_orders() {
            self.refuse(
                report,
                "venue_status",
                &format!("the venue is {}", status.as_str()),
                now,
            );
            return Ok(None);
        }
        let Some(price) = mid else {
            self.refuse(report, "pricing", "the book serves no usable price", now);
            return Ok(None);
        };

        let side = match signal.kind {
            SignalKind::Enter => BookSide::Ask,
            SignalKind::Exit | SignalKind::Hedge => BookSide::Bid,
            SignalKind::Stand => {
                self.refuse(report, "signal_kind", "the signal asks for no action", now);
                return Ok(None);
            }
        };

        // Confidence-weighted sizing, §6.2's consumer. The multiplier narrows
        // the *ask* before the envelope bounds it, so utilisation accounting
        // sees the quantity that will actually be requested. It is exact
        // arithmetic — this scales a position — and a multiply that cannot be
        // represented narrows to nothing rather than widening, the same
        // asymmetry the degradation table itself keeps.
        let desired = signal
            .desired_quantity
            .checked_mul(multiplier)
            .unwrap_or(Decimal::ZERO);
        if !desired.is_positive() {
            self.refuse(
                report,
                "degradation_sizing",
                "the degradation multiplier narrowed the size to nothing",
                now,
            );
            return Ok(None);
        }
        let notional = desired * price;
        let key = signal.strategy.as_str().to_string();
        let Some(deployed) = self.deployed.get(&key) else {
            self.refuse(
                report,
                "deployment",
                "the strategy is not deployed here",
                now,
            );
            return Ok(None);
        };

        // Expiry is checked at every use rather than once at verification:
        // it is the backstop bounding a cell that lost contact, and a backstop
        // consulted only on arrival is not one.
        if !deployed.envelope.is_live(now) {
            self.refuse(
                report,
                "envelope_expiry",
                "the capital envelope has expired; the cell stops rather than continues",
                now,
            );
            return Ok(None);
        }

        let quantity = match deployed
            .envelope
            .admit(&venue, notional, &deployed.utilisation, now)
        {
            CapitalGrant::Full => desired,
            CapitalGrant::Reduced(cap) => {
                let reduced = cap.checked_div(price).unwrap_or(Decimal::ZERO);
                self.refuse(
                    report,
                    "capital_reduced",
                    &format!("reduced to {reduced} by the capital envelope"),
                    now,
                );
                reduced
            }
            CapitalGrant::Refused(reason) => {
                self.refuse(report, "capital", &reason, now);
                return Ok(None);
            }
        };
        if !quantity.is_positive() {
            self.refuse(
                report,
                "capital",
                "the permitted size rounded to nothing",
                now,
            );
            return Ok(None);
        }

        if self.autonomy.level() == AutonomyLevel::Observation {
            self.refuse(
                report,
                "autonomy",
                "the cell is at observation and sends nothing",
                now,
            );
            return Ok(None);
        }

        // Signed, because netting is addition: a buy is positive, a sell is
        // negative, and two opposing intents of equal size sum to nothing
        // without anybody writing a conditional that could be got backwards.
        //
        // `side` names the side of the book the order *takes* — the one
        // convention every seam past this point shares (`Placer`, the node's
        // gateways, `sweep_cost`) — so taking the ask is the buy and is the
        // positive one. This line once read the other way round, and
        // `place_net` read `is_buy` the other way round to match, so an
        // `Enter` still reached the venue as a buy while every fact computed
        // from the sign in between — the cross ledger's `bought` and `sold`,
        // the contributor shares shipped to the centre, `NetIntent::is_buy`
        // itself — named the buyer as the seller. Two inversions that cancel
        // at the venue are not a convention; they are a defect the venue
        // happens not to see.
        let signed = if matches!(side, BookSide::Ask) {
            quantity
        } else {
            -quantity
        };
        let intent = Intent::new(
            signal.strategy.clone(),
            signal.object_id.clone(),
            venue,
            signed,
            price,
            signal.valid_until,
        )?
        // The revisions travel with the intent because this is the last point
        // that has them: after netting, several strategies' shares share one
        // order, and a fill can only be traced back to the values that caused
        // it if each contributor kept its own.
        .with_inputs(signal.inputs.clone());
        Ok(Some(intent))
    }

    /// Send one net intent as one order, or record that it cancelled.
    ///
    /// Phase three. A net of zero is not a refusal: it is two strategies that
    /// wanted opposite things, cancelled internally, and the venue never sees
    /// either — which is the self-trade this whole mechanism exists to
    /// prevent. It is recorded so the cell can still explain what happened.
    fn place_net(
        &mut self,
        net_intent: &NetIntent,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<Option<PlacedOrder>> {
        // Evaluated before the zero-net guard below so that a net cancelling
        // to nothing is still assessed rather than skipped. It is *not* true,
        // as this comment previously claimed, that doing so lets the cap catch
        // its flagship case: see `cross_internally`, where the arithmetic that
        // makes a full cancellation permanently out of cap is set out.
        let crossed = self.cross_internally(net_intent, now, report);

        let Some(is_buy) = net_intent.is_buy() else {
            // Nothing reached the venue, so the cross — if the cap allowed one
            // — is final at this point and safe to seal into the chain.
            self.record_cross(crossed, now, report);
            self.journal.record(
                Decision::Refused {
                    gate: "internal_cross".to_string(),
                    reason: format!(
                        "{} intents on {} at {} cancelled to zero; nothing reached the venue",
                        net_intent.contributors.len(),
                        net_intent.object_id.as_str(),
                        net_intent.venue.as_str()
                    ),
                },
                now,
            );
            self.metrics.intent_cancelled();
            report.cancelled.push(net_intent.clone());
            return Ok(None);
        };
        // A buy takes the ask. See `intent_for` for why this must agree with
        // the sign there rather than compensate for it.
        let side = if is_buy { BookSide::Ask } else { BookSide::Bid };
        let quantity = net_intent.order_quantity();
        let price = net_intent.reference_price;
        let venue = net_intent.venue.clone();

        let (order_id, simulated) = self.send(
            &net_intent.object_id,
            &venue,
            side,
            quantity,
            price,
            now,
            gateway,
        )?;

        // Only now, past the call that can fail. `gateway.place` propagates its
        // error out of `work`, and the caller loses the report with it — so a
        // cross sealed into the hash-chained journal before this line would
        // assert that two strategies traded during a pass that produced
        // nothing at all. The chain is the record; it may not carry a trade
        // the pass did not make.
        self.record_cross(crossed, now, report);

        // Utilisation is charged per contributor, pro-rata on what each
        // wanted, so a netted order still spends each strategy's own envelope
        // rather than one strategy's. The split sums exactly to the order, so
        // the envelopes together are charged what was actually sent.
        for (strategy, share) in net_intent.split_fill(quantity) {
            if let Some(deployed) = self.deployed.get_mut(strategy.as_str()) {
                deployed.utilisation.gross_committed += share * price;
                deployed.utilisation.orders_sent += 1;
            }
        }
        self.record_sent(&order_id, &venue, quantity, price, simulated, now);

        let largest = net_intent
            .contributors
            .iter()
            .max_by(|left, right| {
                left.signed_size
                    .abs()
                    .cmp(&right.signed_size.abs())
                    .then_with(|| right.strategy.as_str().cmp(left.strategy.as_str()))
            })
            .map_or_else(|| StrategyId::new("unknown"), |c| c.strategy.clone());

        Ok(Some(PlacedOrder {
            order_id,
            strategy: largest,
            contributors: net_intent.contributors.clone(),
            object_id: net_intent.object_id.clone(),
            venue,
            side,
            quantity,
            price,
            simulated,
        }))
    }

    /// Number an order and hand it to the venue.
    ///
    /// The one place a `Placer` is called. Both the net path and the cycle
    /// path go through it, so an order that reaches a venue has been numbered
    /// by the cell's own sequence whichever seam produced it, and a second
    /// route to `gateway.place` — the shape of a control being bypassed —
    /// would have to be written in the open.
    #[allow(clippy::too_many_arguments)]
    fn send(
        &mut self,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        now: Timestamp,
        gateway: &mut dyn Placer,
    ) -> Result<(String, bool)> {
        self.order_sequence += 1;
        let order_id = format!("{}-{}", self.config.cell_id, self.order_sequence);
        let simulated = gateway.is_simulated();
        gateway.place(&order_id, object_id, venue, side, quantity, price, now)?;
        Ok((order_id, simulated))
    }

    /// Record an order the venue accepted: the fill the drop-copy will be
    /// reconciled against, the chain entry, and the series.
    fn record_sent(
        &mut self,
        order_id: &str,
        venue: &VenueId,
        quantity: Decimal,
        price: Decimal,
        simulated: bool,
        now: Timestamp,
    ) {
        self.fills.push(CellFill {
            order_id: order_id.to_string(),
            venue: venue.clone(),
            quantity,
            price,
        });
        self.journal.record(
            Decision::OrderSent {
                order_id: order_id.to_string(),
                venue: venue.as_str().to_string(),
                quantity: quantity.to_string(),
                simulated,
            },
            now,
        );
        self.metrics.order_placed(venue);
    }

    // --- the arbitrage desk --------------------------------------------------

    /// Re-quote the graph from the books, scan it, and admit what survives.
    ///
    /// Returns the cycles to send once the nets have gone, each already past
    /// the feasibility gate and the desk's capital envelope. Everything the
    /// scan refused is journaled under the stage that refused it, every
    /// opportunity found is journaled as priced whether or not it is taken,
    /// and everything past the cap is refused and counted — a scan that
    /// found nothing and said nothing is indistinguishable from one that did
    /// not run.
    ///
    /// Two narrowings stop the scan outright. The degradation table's pause
    /// applies to the desk as to any price-only strategy. And a sizing
    /// multiplier below one opens no cycle at all, rather than a smaller one:
    /// the scanner prices at the size policy's size, edge is not linear in
    /// size, and a cycle re-priced narrower is a different cycle whose legs
    /// no longer close on what was priced. Stopping is the fail-closed
    /// reading of §6.2 for a family whose trades cannot be scaled after the
    /// fact.
    fn scan_cycles(
        &mut self,
        now: Timestamp,
        multiplier: Decimal,
        narrowing: &DegradationState,
        report: &mut WorkReport,
    ) -> Result<Vec<AdmittedCycle>> {
        let Some((cap, validity, strategy)) = self.desk.as_ref().map(|desk| {
            (
                desk.max_cycles_per_pass(),
                desk.leg_validity(),
                desk.strategy().clone(),
            )
        }) else {
            return Ok(Vec::new());
        };
        if narrowing.pauses(StrategyClass::PriceOnly) {
            self.refuse(
                report,
                "degradation_pause",
                "the arbitrage desk pauses while its capability is degraded",
                now,
            );
            return Ok(Vec::new());
        }
        if multiplier < Decimal::ONE {
            self.refuse(
                report,
                "degradation_sizing",
                "the arbitrage desk opens no cycle while sizing is narrowed: a cycle re-priced \
                 at a narrower size is a different cycle, and the scanner priced this one at \
                 the policy's size",
                now,
            );
            return Ok(Vec::new());
        }

        let scanned = {
            // Two fields of `self`, borrowed disjointly: the desk re-quotes
            // its graph from the liquidity it is handed and never reaches
            // for it.
            let Some(desk) = self.desk.as_mut() else {
                return Ok(Vec::new());
            };
            desk.refresh(&self.liquidity)?;
            desk.scan(&self.liquidity, now)
        };

        for rejection in &scanned.rejections {
            self.refuse(
                report,
                scan_gate(rejection.stage),
                &format!(
                    "{} cycle over edges {:?}: {}",
                    rejection.candidate.kind.as_str(),
                    rejection.candidate.edges,
                    rejection.detail
                ),
                now,
            );
        }

        // Bounded by the cap, and by what the scan found if that is fewer.
        let mut cycles: Vec<AdmittedCycle> =
            Vec::with_capacity(cap.min(scanned.opportunities.len()));
        // Notional admitted against the desk's envelope so far this pass, so
        // the second cycle is judged against what the first will spend.
        let mut pending = Decimal::ZERO;
        for (position, opportunity) in scanned.opportunities.iter().enumerate() {
            let cycle_id = opportunity.cycle_id(now);
            self.journal.record(
                Decision::EdgePriced {
                    opportunity: cycle_id.clone(),
                    net: opportunity.net().to_string(),
                    positive: true,
                },
                now,
            );
            if position >= cap {
                self.refuse(
                    report,
                    "arbitrage_cap",
                    &format!(
                        "cycle {cycle_id} is opportunity {} of this pass and the cap is {cap}; \
                         refused and counted rather than dropped, and the next pass will find \
                         it again if it is still there",
                        position + 1
                    ),
                    now,
                );
                continue;
            }
            if self.autonomy.level() == AutonomyLevel::Observation {
                self.refuse(
                    report,
                    "autonomy",
                    "the cell is at observation and sends nothing",
                    now,
                );
                continue;
            }
            let legs = match opportunity.cycle_legs(&strategy, now, now.saturating_add(validity)) {
                Ok(legs) => legs,
                Err(error) => {
                    self.refuse(report, "arbitrage_legs", error.message(), now);
                    continue;
                }
            };
            if let Some(admitted) =
                self.admit_cycle(opportunity, &cycle_id, legs, pending, now, report)
            {
                pending += admitted.notional;
                cycles.push(admitted);
            }
        }
        Ok(cycles)
    }

    /// Take every leg of one cycle through the feasibility gate and the
    /// desk's capital envelope, or veto the cycle whole.
    ///
    /// Whole, because a cycle is an atomic set: a leg that cannot execute at
    /// its size leaves the rest as a position rather than a smaller cycle,
    /// and a leg the envelope would reduce is the same position by another
    /// route. The leg's own refusal is recorded by the gate that found it,
    /// and then the cycle's, so the series counts which rule bound and the
    /// journal says which cycle it bound.
    fn admit_cycle(
        &mut self,
        opportunity: &Opportunity,
        cycle_id: &str,
        legs: Vec<CycleLeg>,
        pending: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<AdmittedCycle> {
        let mut intents: Vec<Intent> = Vec::with_capacity(legs.len());
        let mut fixed_cost = Decimal::ZERO;
        let mut on_chain = false;
        let mut notional = Decimal::ZERO;
        for leg in legs {
            let intent: Intent = leg.into();
            if !self.admit_feasible(&intent, now, report) {
                self.veto_cycle(cycle_id, &intent, "is infeasible at its size", now, report);
                return None;
            }
            let cost = {
                let model = self.config.feasibility.get(intent.venue.as_str());
                on_chain |= model.is_some_and(|model| {
                    matches!(model.class(), VenueClass::DecentralisedExchange)
                });
                feasibility::fixed_cost_fraction(model, self.feasibility_constraints(), &intent)
            };
            match cost {
                Ok(fraction) => fixed_cost += fraction,
                Err(infeasible) => {
                    self.refuse(report, infeasible.gate, &infeasible.reason, now);
                    self.veto_cycle(
                        cycle_id,
                        &intent,
                        "has no notional to charge against",
                        now,
                        report,
                    );
                    return None;
                }
            }
            let Some(admitted) = self.admit_leg(&intent, pending + notional, now, report) else {
                self.veto_cycle(
                    cycle_id,
                    &intent,
                    "is not admitted by the capital envelope",
                    now,
                    report,
                );
                return None;
            };
            notional += admitted;
            intents.push(intent);
        }

        // The edge as a fraction of the start size, the unit the summed leg
        // costs are in. A start quantity the scanner priced at is positive by
        // construction; a division that fails anyway is a refusal, not a
        // pass.
        let edge_fraction = opportunity
            .net()
            .checked_div(opportunity.pricing.start_quantity);
        let Some(edge_fraction) = edge_fraction else {
            self.refuse(
                report,
                "arbitrage_cycle",
                &format!("cycle {cycle_id} is refused whole: its edge cannot be stated per unit of start size"),
                now,
            );
            return None;
        };
        if let Err(infeasible) = feasibility::assess_cycle_cost(fixed_cost, edge_fraction, on_chain)
        {
            self.refuse(report, infeasible.gate, &infeasible.reason, now);
            self.refuse(
                report,
                "arbitrage_cycle",
                &format!("cycle {cycle_id} is refused whole: its fixed costs consume its edge"),
                now,
            );
            return None;
        }
        Some(AdmittedCycle {
            cycle_id: cycle_id.to_string(),
            net: opportunity.net(),
            legs: intents,
            notional,
        })
    }

    fn veto_cycle(
        &mut self,
        cycle_id: &str,
        leg: &Intent,
        why: &str,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        self.refuse(
            report,
            "arbitrage_cycle",
            &format!(
                "cycle {cycle_id} is refused whole: its leg {} of {} at {} {why}, and a cycle \
                 short one leg is a position rather than a smaller cycle",
                leg.signed_size,
                leg.object_id.as_str(),
                leg.venue.as_str()
            ),
            now,
        );
    }

    /// One leg through the gates a directional intent meets in `intent_for`
    /// after pricing: the venue's status, the envelope's life, and capital.
    ///
    /// Returns the leg's notional on admission. `pending` is what this pass
    /// has already admitted against the desk's envelope, added to its
    /// utilisation for the check so a cycle cannot be admitted leg by leg
    /// into more than the envelope holds.
    ///
    /// A `Reduced` grant is a refusal here where `intent_for` takes the
    /// reduction: a directional order at a smaller size is a smaller
    /// position, and a cycle leg at a smaller size is a cycle that no longer
    /// closes.
    fn admit_leg(
        &mut self,
        intent: &Intent,
        pending: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<Decimal> {
        let status = self
            .liquidity
            .get(&intent.venue, &intent.object_id)
            .map(VenueState::status);
        match status {
            None => {
                self.refuse(
                    report,
                    "book",
                    "the cell holds no book for the instrument",
                    now,
                );
                return None;
            }
            Some(status) if !status.accepts_orders() => {
                self.refuse(
                    report,
                    "venue_status",
                    &format!("the venue is {}", status.as_str()),
                    now,
                );
                return None;
            }
            Some(_) => {}
        }
        let Some(notional) = intent.signed_size.abs().checked_mul(intent.reference_price) else {
            self.refuse(
                report,
                "capital",
                "the leg's notional cannot be represented",
                now,
            );
            return None;
        };

        let grant = self.desk.as_ref().map(|desk| {
            if !desk.envelope().is_live(now) {
                return None;
            }
            let mut used = desk.utilisation().clone();
            used.gross_committed += pending;
            Some(desk.envelope().admit(&intent.venue, notional, &used, now))
        });
        match grant {
            None => {
                self.refuse(report, "deployment", "no arbitrage desk is installed", now);
                None
            }
            Some(None) => {
                self.refuse(
                    report,
                    "envelope_expiry",
                    "the desk's capital envelope has expired; the cell stops rather than continues",
                    now,
                );
                None
            }
            Some(Some(CapitalGrant::Full)) => Some(notional),
            Some(Some(CapitalGrant::Reduced(cap))) => {
                self.refuse(
                    report,
                    "arbitrage_capital",
                    &format!(
                        "the envelope would reduce the leg to {cap} notional and a cycle leg cannot \
                         be reduced: a reduced leg is a position, not a smaller cycle"
                    ),
                    now,
                );
                None
            }
            Some(Some(CapitalGrant::Refused(reason))) => {
                self.refuse(report, "capital", &reason, now);
                None
            }
        }
    }

    /// Send every leg of an admitted cycle, in plan order.
    ///
    /// # What this cannot promise, and what it does instead
    ///
    /// The blueprint's cycle is atomic-or-cancelled. This cell's
    /// [`Placer`] can place and cannot cancel, and no fill reaches the cell
    /// until the drop-copy is reconciled, so there is nothing here a
    /// `LegGroup` could act on: the coordinator in
    /// `qip-execution-engine::multileg` decides what to unwind from fills it
    /// is told about, and this seam is told nothing. Building one here would
    /// be a control with no input.
    ///
    /// What the cell can do is refuse to carry on. A leg the venue refuses
    /// after an earlier leg went out leaves the cell holding a position it
    /// did not decide to take, which is the state the multi-leg module calls
    /// the one that "looks, to every downstream report, exactly like a
    /// position somebody chose". So the break is journaled naming the cycle
    /// and how many legs were sent, and the kill switch is tripped as a
    /// reconciliation break trips it: the cell stops until an operator has
    /// looked, and the error propagates so the caller knows the pass did not
    /// complete.
    fn place_cycle(
        &mut self,
        cycle: &AdmittedCycle,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<()> {
        let mut orders: Vec<String> = Vec::with_capacity(cycle.legs.len());
        for (position, leg) in cycle.legs.iter().enumerate() {
            // A buy takes the ask; the sign was fixed where the leg was made.
            let side = if leg.signed_size.is_positive() {
                BookSide::Ask
            } else {
                BookSide::Bid
            };
            let quantity = leg.signed_size.abs();
            let price = leg.reference_price;
            let sent = self.send(
                &leg.object_id,
                &leg.venue,
                side,
                quantity,
                price,
                now,
                gateway,
            );
            let (order_id, simulated) = match sent {
                Ok(sent) => sent,
                Err(error) => {
                    self.break_cycle(
                        &cycle.cycle_id,
                        position,
                        cycle.legs.len(),
                        &error,
                        now,
                        report,
                    );
                    return Err(error);
                }
            };
            self.record_sent(&order_id, &leg.venue, quantity, price, simulated, now);
            if let Some(desk) = self.desk.as_mut() {
                let utilisation = desk.utilisation_mut();
                utilisation.gross_committed += quantity * price;
                utilisation.orders_sent += 1;
            }
            orders.push(order_id.clone());
            report.orders.push(PlacedOrder {
                order_id,
                strategy: leg.strategy.clone(),
                contributors: vec![Contributor {
                    strategy: leg.strategy.clone(),
                    signed_size: leg.signed_size,
                    inputs: leg.inputs.clone(),
                }],
                object_id: leg.object_id.clone(),
                venue: leg.venue.clone(),
                side,
                quantity,
                price,
                simulated,
            });
        }
        self.journal.record(
            Decision::CycleCommitted {
                cycle_id: cycle.cycle_id.clone(),
                orders,
                net: cycle.net.to_string(),
            },
            now,
        );
        Ok(())
    }

    /// A cycle stopped between legs. Record it and stop the cell.
    fn break_cycle(
        &mut self,
        cycle_id: &str,
        sent: usize,
        total: usize,
        error: &Error,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        self.refuse(
            report,
            "arbitrage_cycle_broken",
            &format!(
                "cycle {cycle_id} stopped after {sent} of {total} legs: {}; the legs already sent \
                 are a position nobody chose, and the cell halts until an operator has looked",
                error.message()
            ),
            now,
        );
        if sent > 0 && !self.is_halted() {
            self.autonomy.kill_switch_mut().trip_global(
                now,
                "arbitrage",
                "a cycle stopped between legs and left a position the cell did not decide to take",
            );
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: format!("cycle {cycle_id} broke after {sent} of {total} legs"),
                },
                now,
            );
            self.record_halt();
        }
    }

    /// Work out the offsetting part of a net that should be crossed between its
    /// own contributors, or refuse it and say why (§27.1).
    ///
    /// Returns the cross rather than recording it: the caller seals it into the
    /// journal once the pass has an outcome, because a venue call can fail
    /// afterwards and take the whole report with it.
    ///
    /// The matched size is the smaller of the buying and selling sides, which
    /// is exactly the quantity that never needed a venue. It is computed as a
    /// minimum rather than as `(gross - |net|) / 2` so that no division enters
    /// a money path: the two are equal, and only one of them can introduce a
    /// remainder.
    ///
    /// **The cap refuses; it does not clamp.** Above forty percent of gross
    /// intent the blueprint's objection is that a persistent internal market
    /// forms whose marks drift from reality — and crossing the permitted forty
    /// percent and abandoning the rest would build exactly that market, just
    /// more slowly. Refusing the whole cross leaves the offsetting intents
    /// netted as before: nothing extra reaches the venue, and nothing is
    /// booked between strategies.
    ///
    /// # What this cap cannot do, stated because the arithmetic is not obvious
    ///
    /// The matched size is `min(buy, sell)` and the denominator is
    /// `buy + sell`, so the ratio can never exceed one half, and it reaches one
    /// half exactly when the two sides cancel completely. A forty percent cap
    /// therefore fires only in the narrow band above two fifths — and **a net
    /// that cancels to zero is always refused**, which is §27.1's own flagship
    /// case: "strategies that disagree cost nothing to run together because
    /// their disagreement never reaches a venue". Under this measure that
    /// disagreement is never booked as a cross at all.
    ///
    /// That is a real divergence from the blueprint, left in place deliberately
    /// rather than tuned away. §27.1 caps crossing at forty percent of gross
    /// intent "per instrument **per interval**", and never says how long an
    /// interval is. Measured per net, as here, the bound above is arithmetic.
    /// Measured across an interval, a full cancellation could sit inside a
    /// larger instrument-level gross and be admitted — but the window length
    /// decides when a safety control fires, and choosing one here to make a
    /// case reachable would be inventing the very parameter that governs it.
    /// The interval belongs to whoever owns the cap, not to this function.
    ///
    /// The consequence, so nobody has to derive it: a perfectly offsetting pair
    /// is netted, never reaches a venue, and is recorded as a cap refusal
    /// rather than as a cross. Safe, and less than §27.1 asks for.
    ///
    /// Crossing changes nothing about what is sent. It is a booking decision
    /// on top of netting, which has already decided what a venue sees.
    fn cross_internally(
        &mut self,
        net_intent: &NetIntent,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<InternalCross> {
        let mut bought = Vec::new();
        let mut sold = Vec::new();
        let mut buy_size = Decimal::ZERO;
        let mut sell_size = Decimal::ZERO;
        for contributor in &net_intent.contributors {
            if contributor.signed_size.is_positive() {
                buy_size += contributor.signed_size;
                bought.push(contributor.strategy.clone());
            } else if contributor.signed_size.is_negative() {
                sell_size -= contributor.signed_size;
                sold.push(contributor.strategy.clone());
            }
        }
        // Nothing offset, so there is nothing to cross. The common case, and
        // not a refusal: a net every contributor agreed on has no internal
        // trade in it to record. It is journaled anyway, because a chain that
        // explains a cross but is silent about its absence leaves a reader
        // unable to tell "they agreed" from "the crossing step never ran".
        if buy_size.is_zero() || sell_size.is_zero() {
            self.journal.record(
                Decision::CrossedInternally {
                    object: net_intent.object_id.as_str().to_string(),
                    venue: net_intent.venue.as_str().to_string(),
                    quantity: Decimal::ZERO.to_string(),
                    price: String::new(),
                    bought: bought.iter().map(|id| id.as_str().to_string()).collect(),
                    sold: sold.iter().map(|id| id.as_str().to_string()).collect(),
                },
                now,
            );
            return None;
        }
        let crossed = if buy_size < sell_size {
            buy_size
        } else {
            sell_size
        };

        // Forty percent of gross intent, compared without dividing: the cap is
        // two fifths, so `crossed * 5 > gross * 2` asks the same question in
        // exact arithmetic. A multiply that cannot be represented refuses,
        // because a cap that silently answered "under" on overflow would be a
        // control that cannot fire.
        let over_cap = match (
            crossed.checked_mul(Decimal::from_int(5)),
            net_intent.gross_size.checked_mul(Decimal::from_int(2)),
        ) {
            (Some(five_crossed), Some(two_gross)) => five_crossed > two_gross,
            _ => true,
        };
        if over_cap {
            self.refuse(
                report,
                "internal_cross_cap",
                &format!(
                    "crossing {crossed} of {} gross intent on {} exceeds the forty percent cap; \
                     the cross is refused whole rather than trimmed to the cap, because a cross \
                     repeated at the cap every interval is the persistent internal market the \
                     cap exists to prevent",
                    net_intent.gross_size,
                    net_intent.object_id.as_str()
                ),
                now,
            );
            return None;
        }

        // The prevailing mid at the netting instant, read from the book now
        // rather than taken from `reference_price` — that is the largest
        // contributor's own stamped price, and §27.1 requires a price neither
        // side chose. A book that serves no mid refuses the cross instead of
        // falling back to one, because the fallback is precisely the price the
        // rule forbids.
        let mid = self
            .liquidity
            .get(&net_intent.venue, &net_intent.object_id)
            .and_then(|state| state.mid());
        let Some(price) = mid else {
            self.refuse(
                report,
                "internal_cross_price",
                "the book serves no mid at the netting instant, and a cross has no price either \
                 side may choose",
                now,
            );
            return None;
        };

        Some(InternalCross {
            object_id: net_intent.object_id.clone(),
            venue: net_intent.venue.clone(),
            quantity: crossed,
            price,
            bought,
            sold,
        })
    }

    /// Seal a cross into the hash-chained journal and report it.
    ///
    /// Separate from working the cross out, so that the record is written only
    /// once the pass it belongs to has actually produced its outcome.
    fn record_cross(
        &mut self,
        crossed: Option<InternalCross>,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        let Some(cross) = crossed else {
            return;
        };
        self.journal.record(
            Decision::CrossedInternally {
                object: cross.object_id.as_str().to_string(),
                venue: cross.venue.as_str().to_string(),
                quantity: cross.quantity.to_string(),
                price: cross.price.to_string(),
                bought: cross
                    .bought
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
                sold: cross
                    .sold
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
            },
            now,
        );
        self.metrics.internal_cross(&cross.venue);
        report.crosses.push(cross);
    }

    /// Judge one intent against the feasibility gate, refusing and counting
    /// it under the rule that bound.
    ///
    /// The book is read here, at the netting instant, and the gate itself is
    /// a pure function of what it is handed — so the fact judged is the one
    /// the journal can replay. The size resting at the touch is read on the
    /// side the intent *takes*: a buy takes the ask, so it is the ask's size
    /// that bounds it.
    fn admit_feasible(&mut self, intent: &Intent, now: Timestamp, report: &mut WorkReport) -> bool {
        let touch = self
            .liquidity
            .get(&intent.venue, &intent.object_id)
            .and_then(|state| {
                if intent.signed_size.is_positive() {
                    state.best_ask()
                } else {
                    state.best_bid()
                }
            })
            .map(|level| level.size);
        let verdict = feasibility::assess(
            self.config.feasibility.get(intent.venue.as_str()),
            self.feasibility_constraints(),
            intent,
            touch,
        );
        match verdict {
            Ok(()) => true,
            Err(infeasible) => {
                self.refuse(report, infeasible.gate, &infeasible.reason, now);
                false
            }
        }
    }

    /// Item 11 of the applied policy payload, if the centre has produced it.
    ///
    /// Read whatever its freshness: a venue's minimum order and tick change
    /// on the order of months, the slot's own time-to-live is a day, and a
    /// constraint that has gone stale is still the last thing the centre
    /// knew rather than nothing. The degradation table already narrows the
    /// cell's sizing on the slot's staleness; refusing to read the slot as
    /// well would be a second control on the same fact with a different
    /// threshold.
    fn feasibility_constraints(&self) -> Option<&qip_contracts::policy::FeasibilityConstraints> {
        self.policy
            .as_ref()
            .and_then(|policy| policy.payload().feasibility_constraints.value())
    }

    fn venue_for(&self, object: &ObjectId) -> Option<VenueId> {
        self.config
            .venues
            .iter()
            .find(|venue| {
                self.liquidity
                    .get(venue, object)
                    .is_some_and(|state| state.status() != VenueStatus::Unreachable)
            })
            .cloned()
    }

    fn refuse(&mut self, report: &mut WorkReport, gate: &str, reason: &str, now: Timestamp) {
        // Every gate a *pass* can refuse at funnels through here, so one
        // recording site covers all of them. `gate` is a string literal at
        // each call, and that is what bounds this series' cardinality. The
        // three refusals that journal directly — a replayed halt, a release
        // that predates its barrier, and a net that cancelled to zero — are
        // not pass-time gates and are deliberately not counted here: the
        // first two are control-plane events with no "why was the cell
        // quiet" reading, and the third is counted as a cancellation.
        self.metrics.refusal(gate);
        report.refusals.push((gate.to_string(), reason.to_string()));
        self.journal.record(
            Decision::Refused {
                gate: gate.to_string(),
                reason: reason.to_string(),
            },
            now,
        );
    }

    // --- the mesh seam ------------------------------------------------------

    /// Describe this cell to the central plane.
    ///
    /// Assembled from what the cell already holds rather than accumulated as it
    /// goes, so a delta is a *view* and building one twice with the same report
    /// produces the same value. That matters because the transport underneath
    /// is at-least-once: a delta that is rebuilt and re-sent after a failed
    /// attempt has to be the same fact, not a second one.
    ///
    /// `cell`, `region` and `sequence` are filled in by
    /// [`crate::mesh::CellUplink::publish`], which owns the stream's numbering.
    /// A cell that numbered its own deltas would eventually skip one, and the
    /// centre cannot tell a skipped sequence from a lost delta.
    pub fn state_delta(&self, report: &WorkReport, at: Timestamp) -> CellStateDelta {
        let mut delta = CellStateDelta {
            cell: self.config.cell_id.clone(),
            region: self.config.region.clone(),
            sequence: 0,
            at,
            halted: self.is_halted(),
            utilisation: self
                .deployed
                .values()
                .map(|deployed| StrategyUtilisation {
                    strategy: deployed.envelope.strategy().clone(),
                    utilisation: deployed.utilisation.clone(),
                    envelope_expires_at: deployed.envelope.expires_at(),
                })
                // The desk spends an envelope too, and the centre that issued
                // it hears how much the same way.
                .chain(self.desk.as_ref().map(|desk| StrategyUtilisation {
                    strategy: desk.strategy().clone(),
                    utilisation: desk.utilisation().clone(),
                    envelope_expires_at: desk.envelope().expires_at(),
                }))
                .collect(),
            orders: report
                .orders
                .iter()
                .map(|order| DeltaOrder {
                    order_id: order.order_id.clone(),
                    strategy: order.strategy.clone(),
                    object_id: order.object_id.clone(),
                    venue: order.venue.clone(),
                    side: order.side,
                    quantity: order.quantity,
                    price: order.price,
                    simulated: order.simulated,
                    contributors: order.contributors.clone(),
                })
                .collect(),
            refusals: report
                .refusals
                .iter()
                .map(|(gate, reason)| DeltaRefusal {
                    gate: gate.clone(),
                    reason: reason.clone(),
                })
                .collect(),
            // Set by `bound_refusals` below; the caller does not get to claim
            // a truncation that did not happen.
            refusals_omitted: 0,
            reconciliation_breaks: self.breaks.clone(),
            reconciliation_breaks_omitted: self.breaks_omitted,
            crosses: report
                .crosses
                .iter()
                .map(|cross| qip_contracts::wire::CrossRecord {
                    object_id: cross.object_id.clone(),
                    venue: cross.venue.clone(),
                    quantity: cross.quantity,
                    price: cross.price,
                    bought: cross.bought.clone(),
                    sold: cross.sold.clone(),
                })
                .collect(),
            // Set by `bound_refusals` below, like the refusal counter: the
            // caller does not get to claim a truncation that did not happen.
            crosses_omitted: 0,
        };
        delta.bound_refusals();
        delta
    }

    /// Install a capital envelope the centre issued for a strategy already
    /// deployed here.
    ///
    /// Takes the verified type, so there is no path from a frame off the wire
    /// to a live grant that does not go through
    /// [`crate::VerifiedEnvelope::verify`]. Arriving over the mesh buys an
    /// envelope nothing; this signature is what says so.
    ///
    /// Three things this deliberately does not do:
    ///
    /// * **It does not deploy.** A grant names a strategy; it does not carry
    ///   the compiled strategy or the program its plan indexes into, and a cell
    ///   that started running something because capital arrived for it would be
    ///   promoting its own strategy — the thing ADR 0008 says a cell never
    ///   does. An envelope for a strategy that is not deployed is refused.
    /// * **It does not reset utilisation.** What a strategy has committed is
    ///   measured against positions that are still open, and a renewal that
    ///   zeroed it would hand the strategy its whole gross limit again while
    ///   the previous commitment was still live. Carrying it across is the
    ///   conservative direction, and it is the one that is right.
    /// * **It does not widen anything by itself.** The new envelope replaces
    ///   the old one entirely — wider or narrower — because that is what the
    ///   centre signed. A cell that merged the two would be constructing a
    ///   grant nobody approved.
    pub fn renew_capital(&mut self, envelope: VerifiedEnvelope, now: Timestamp) -> Result<()> {
        // `verify` has already checked the cell, and this checks it again
        // against the cell's own identity rather than against the string a
        // caller passed to the verifier. The two are the same today; a
        // downlink misconfigured with another cell's name is the case where
        // they would not be, and that is exactly the case worth catching.
        if envelope.cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot renew capital at {}",
                envelope.cell(),
                self.config.cell_id
            )));
        }
        let key = envelope.strategy().as_str().to_string();
        let approver = envelope.approver().to_string();
        let expires_at = envelope.expires_at();
        if let Some(desk) = self.desk.as_mut()
            && desk.strategy().as_str() == key
        {
            // The desk is renewed by the same rules as a strategy: the grant
            // replaces the old one whole and utilisation carries across.
            desk.replace_envelope(envelope);
            self.journal.record(
                Decision::CapitalRenewed {
                    strategy: key,
                    approver,
                    expires_at,
                },
                now,
            );
            return Ok(());
        }
        let Some(deployed) = self.deployed.get_mut(&key) else {
            return Err(Error::not_found(format!(
                "no strategy {key} is deployed at this cell, so there is nothing for the grant to \
                 fund; a cell does not deploy a strategy because capital arrived for it"
            )));
        };
        deployed.envelope = envelope;
        self.journal.record(
            Decision::CapitalRenewed {
                strategy: key,
                approver,
                expires_at,
            },
            now,
        );
        Ok(())
    }

    /// Reconciliation breaks this cell has recorded, oldest first.
    pub fn reconciliation_breaks(&self) -> &[String] {
        &self.breaks
    }

    // --- reconciliation and the mirror --------------------------------------

    /// Absorb a fill from the independent drop-copy channel.
    pub fn observe_drop_copy(&mut self, fill: DropCopyFill) {
        self.dropcopy.observe(fill);
    }

    /// Compare the two records and halt on any disagreement.
    ///
    /// Tripping needs no authority, which is why a break can act immediately:
    /// the cost of a false stop is minutes of missed opportunity, the cost of
    /// trading on a book that disagrees with the venue is unbounded.
    pub fn reconcile(&mut self, now: Timestamp) -> Vec<Discrepancy> {
        let fills = self.fills.clone();
        let breaks = self.dropcopy.reconcile(&fills);
        for discrepancy in &breaks {
            let detail = discrepancy.describe();
            if self.breaks.len() < MAX_RETAINED_BREAKS {
                self.breaks.push(detail.clone());
            } else {
                self.breaks_omitted = self.breaks_omitted.saturating_add(1);
            }
            self.metrics.reconciliation_break();
            self.journal
                .record(Decision::ReconciliationBreak { detail }, now);
        }
        if !breaks.is_empty() && !self.is_halted() {
            self.autonomy.kill_switch_mut().trip_global(
                now,
                "drop-copy",
                "the cell's fills disagree with the venue's own account",
            );
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: "reconciliation break".to_string(),
                },
                now,
            );
            // A break halts the cell, which stops it running passes. Without
            // this the gauge would keep reporting the state of the last pass
            // that ran, which is the one before the break.
            self.record_halt();
        }
        breaks
    }

    /// Ship the journal to durable storage.
    ///
    /// The only call in the cell that may block, and deliberately outside both
    /// [`Cell::on_bytes`] and [`Cell::work`].
    pub fn flush(&mut self, mirror: &mut dyn Mirror, now: Timestamp) -> Result<usize> {
        let watermarks = self
            .sequencer
            .watermarks()
            .into_iter()
            .map(|mark| (mark.stream, mark.position))
            .collect();
        crate::journal::ship(
            &mut self.journal,
            mirror,
            &self.config.cell_id,
            watermarks,
            now,
        )
    }
}

/// A cycle past every gate and waiting for the nets to go out first.
#[derive(Clone, Debug)]
struct AdmittedCycle {
    cycle_id: String,
    /// The scanner's net edge, in units of the start instrument.
    net: Decimal,
    /// In plan order. Every one carries `NettingPolicy::NoNet` by
    /// construction, which is what `CycleLeg` exists to guarantee.
    legs: Vec<Intent>,
    /// The sum of the legs' notionals, admitted against the desk's envelope.
    notional: Decimal,
}

/// The gate literal a scan rejection is counted under.
///
/// One literal per stage of the scanner, so §30.1's question — which stage
/// refuses most of what the search proposes — is a series rather than a
/// grep of the journal. Bounded by the enum.
const fn scan_gate(stage: RejectionStage) -> &'static str {
    match stage {
        RejectionStage::Unsized => "arbitrage_scan_unsized",
        RejectionStage::ExactArithmetic => "arbitrage_scan_exact_arithmetic",
        RejectionStage::Unpriceable => "arbitrage_scan_unpriceable",
        RejectionStage::Depth => "arbitrage_scan_depth",
        RejectionStage::Book => "arbitrage_scan_book",
        RejectionStage::NetEdge => "arbitrage_scan_net_edge",
        RejectionStage::Plan => "arbitrage_scan_plan",
    }
}

/// Where a cell sends an order.
///
/// Narrower than the routing crate's `Gateway` on purpose: the cell needs to
/// place and to know whether the venue is simulated, and a wider surface here
/// would be a wider surface to get wrong.
pub trait Placer: std::fmt::Debug {
    /// Whether this is a simulated venue. The cell sets every order's
    /// `simulated` flag from this rather than from anything the caller says.
    fn is_simulated(&self) -> bool;

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()>;

    /// What a production deployment must supply, empty when usable as is.
    fn required_configuration(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The gap events worth journalling, and what to say about each.
///
/// An opened gap may still fill, so it is recorded as an observation. An
/// abandoned one has already produced a reset and invalidated a book, which is
/// the event an incident review is looking for.
fn gap_detail(event: &qip_sequencing::tracker::SequenceEvent) -> Option<(String, String)> {
    use qip_sequencing::tracker::SequenceEvent;
    match event {
        SequenceEvent::GapOpened {
            stream,
            missing_from,
            missing_to,
        } => Some((
            stream.clone(),
            format!("sequences {missing_from}..={missing_to} are missing; holding for reorder"),
        )),
        SequenceEvent::GapAbandoned {
            stream,
            missing_from,
            missing_to,
            reason,
        } => Some((
            stream.clone(),
            format!(
                "sequences {missing_from}..={missing_to} will not arrive ({reason:?}); \
                 the affected books are reset"
            ),
        )),
        SequenceEvent::StreamStarted { .. }
        | SequenceEvent::Duplicate { .. }
        | SequenceEvent::GapFilled { .. } => None,
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod crossing_tests {
    //! §27.1's crossing price, tested where the two candidate prices differ.
    //!
    //! The behavioural tests in `qip-edge-node` cannot tell the book's mid from
    //! `NetIntent::reference_price`, because a cell prices every intent off the
    //! same mid in the same pass and the two numbers are equal there. A
    //! mutation that priced crosses from the reference price survived those
    //! tests for exactly that reason. These drive the private seam with a net
    //! intent whose reference price is deliberately nothing like the book, so
    //! "the prevailing mid at the netting instant, never a price either side
    //! chose" becomes an assertion instead of a coincidence.

    use super::*;
    use qip_contracts::message::{MarketMessage, MessageBody};
    use qip_contracts::venue::{Origin, VenueStatus};
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;
    use qip_orderbook::venue::VenueState;

    const CELL: &str = "london-1";

    fn object() -> ObjectId {
        ObjectId::from_string("ACME")
    }

    fn venue() -> VenueId {
        VenueId::new("XLON")
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000).saturating_add(qip_core::Duration::from_secs(seconds))
    }

    /// A book quoting 99 / 101, so the mid is 100.
    fn book() -> VenueState {
        let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
        for (index, (side, price, size)) in
            [(BookSide::Bid, "99", "900"), (BookSide::Ask, "101", "300")]
                .iter()
                .enumerate()
        {
            let message = MarketMessage::new(
                object(),
                Origin::new(venue(), "feed-a", 0, index as u64),
                MessageBody::LevelSet {
                    side: *side,
                    price: Decimal::parse(price).expect("a decimal literal"),
                    quantity: Decimal::parse(size).expect("a decimal literal"),
                    order_count: None,
                },
                at(index as i64),
                at(index as i64),
            );
            state.apply(&message).expect("a well-formed level");
        }
        state
    }

    fn cell_with_book() -> Result<Cell> {
        let config = CellConfig::new(CELL, "europe-west2").with_venue(venue());
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(book());
        Ok(cell)
    }

    /// Net the given `(strategy, signed size)` pairs on the fixture instrument
    /// through [`net`] itself, every intent stamped with `reference_price`.
    ///
    /// Not a literal. `NetIntent` is sealed to `net` so that nobody can
    /// assemble a vector of contributors `net` would have refused, and these
    /// tests were the one caller still forging one by hand — a fixture that
    /// bypasses the seam it is meant to drive is a second construction path
    /// with a friendlier name. Going through `net` also means the net's own
    /// reference price is whatever `net` chose, which is what the crossing
    /// test needs to be sure of before it can claim the mid was chosen over
    /// it.
    fn netted(sizes: &[(&str, &str)], reference_price: Decimal) -> NetIntent {
        let intents = sizes
            .iter()
            .map(|(strategy, size)| {
                Intent::new(
                    StrategyId::new(*strategy),
                    object(),
                    venue(),
                    Decimal::parse(size).expect("a decimal literal"),
                    reference_price,
                    at(60),
                )
                .expect("a fixture size is never zero")
            })
            .collect();
        let mut nets = net(intents);
        assert_eq!(
            nets.len(),
            1,
            "directional intents on one instrument and venue net to one group"
        );
        nets.pop().expect("exactly one net was just asserted")
    }

    /// A net of a 100 buy against a 20 sell: 20 crosses, which is a sixth of
    /// the 120 gross and so comfortably under the forty percent cap.
    fn offsetting_net(reference_price: Decimal) -> NetIntent {
        netted(&[("alpha", "100"), ("beta", "-20")], reference_price)
    }

    #[test]
    fn a_cross_is_priced_at_the_book_mid_and_not_at_a_price_either_side_chose() -> Result<()> {
        let mut cell = cell_with_book()?;
        // A reference price nothing in the book could produce. If the cross
        // were priced from the net intent, this is the number that would
        // appear — and it is one contributor's own stamped price, which §27.1
        // forbids by name.
        let chosen = Decimal::parse("12345").expect("a decimal literal");
        let net_intent = offsetting_net(chosen);
        // The premise, in two halves: the net `net` built really carries the
        // chosen price — otherwise the assertion below that the cross did not
        // take it would be true of any implementation — and the two candidate
        // prices really do differ here, which is the whole reason this test
        // exists rather than the behavioural one.
        assert_eq!(
            net_intent.reference_price, chosen,
            "the fixture net does not carry the price it was built from"
        );
        let mid = cell
            .liquidity()
            .get(&venue(), &object())
            .and_then(|state| state.mid())
            .expect("the fixture book serves a mid");
        assert_ne!(mid, chosen, "the fixture cannot distinguish the two prices");

        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(&net_intent, at(10), &mut report);
        cell.record_cross(crossed, at(10), &mut report);

        assert_eq!(report.crosses.len(), 1, "nothing was crossed: {report:?}");
        assert_eq!(
            report.crosses[0].price, mid,
            "the cross was priced at {} rather than at the mid",
            report.crosses[0].price
        );
        assert_ne!(
            report.crosses[0].price, chosen,
            "the cross took the reference price, which is a price one side chose"
        );
        assert_eq!(
            report.crosses[0].quantity,
            Decimal::parse("20").expect("a decimal literal"),
            "the matched size is the smaller side, not the net or the gross"
        );
        Ok(())
    }

    /// A venue that refuses everything, so `place_net` returns `Err` and the
    /// caller loses the report.
    #[derive(Debug)]
    struct RefusingGateway;

    impl Placer for RefusingGateway {
        fn is_simulated(&self) -> bool {
            true
        }

        fn place(
            &mut self,
            _order_id: &str,
            _object_id: &ObjectId,
            _venue: &VenueId,
            _side: BookSide,
            _quantity: Decimal,
            _price: Decimal,
            _at: Timestamp,
        ) -> Result<()> {
            Err(qip_core::error::Error::io("the venue refused the order"))
        }
    }

    #[test]
    fn a_venue_that_fails_leaves_no_cross_in_the_chain() -> Result<()> {
        // The journal is hash-chained and is the record. `gateway.place`
        // propagates its error out of `place_net` and out of `work`, and the
        // caller loses the report with it — so a cross written before that call
        // would leave the chain asserting that two strategies traded during a
        // pass that produced nothing at all. Nobody can unwrite it afterwards.
        let mut cell = cell_with_book()?;
        let net_intent = offsetting_net(Decimal::parse("100").expect("a decimal literal"));
        let mut report = WorkReport::default();

        let before = cell.journal().entries().len();
        let outcome = cell.place_net(&net_intent, at(10), &mut RefusingGateway, &mut report);

        // Premise: the venue really did fail, so what follows is about the
        // failure path and not about a quiet success.
        assert!(
            outcome.is_err(),
            "the premise failed: the refusing gateway placed an order"
        );
        let crosses: Vec<_> = cell
            .journal()
            .entries()
            .iter()
            .skip(before)
            .filter(|entry| entry.decision.kind() == "crossed_internally")
            .collect();
        assert!(
            crosses.is_empty(),
            "the chain records a cross for a pass that placed nothing: {crosses:?}"
        );
        Ok(())
    }

    #[test]
    fn a_fully_offsetting_net_is_out_of_cap_by_arithmetic_and_is_never_crossed() -> Result<()> {
        // §27.1's flagship case, and the one this cap cannot admit. The matched
        // size is `min(buy, sell)` over a denominator of `buy + sell`, so the
        // ratio tops out at one half and hits it exactly when the two sides
        // cancel — always above forty percent. The test exists so the
        // divergence is asserted rather than merely described in a comment
        // somebody may later delete as stale.
        let mut cell = cell_with_book()?;
        let opposed = netted(
            &[("alpha", "100"), ("beta", "-100")],
            Decimal::parse("100").expect("a decimal literal"),
        );
        // Premise: the sides really do cancel, so this is the full-offset case
        // and not merely a large partial one.
        assert!(
            opposed.net_size.is_zero(),
            "the premise needs a total offset"
        );

        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(&opposed, at(10), &mut report);
        cell.record_cross(crossed, at(10), &mut report);

        assert!(
            report.crosses.is_empty(),
            "a fully offsetting net was crossed, so the cap arithmetic has \
             changed and the comment explaining it is now wrong"
        );
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "internal_cross_cap"),
            "the full offset was neither crossed nor refused by the cap: {:?}",
            report.refusals
        );
        Ok(())
    }

    #[test]
    fn a_book_with_no_mid_refuses_the_cross_rather_than_pricing_it_from_the_intent() -> Result<()> {
        // The fallback §27.1 forbids is exactly the one a careless
        // implementation reaches for when the book is silent. There is no
        // price neither side chose available, so there is no cross.
        let config = CellConfig::new(CELL, "europe-west2").with_venue(venue());
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(VenueState::aggregated(object(), venue(), VenueStatus::Open));
        // Premise: this book genuinely serves no mid, so the refusal below is
        // about the price and not about something else.
        assert!(
            cell.liquidity()
                .get(&venue(), &object())
                .and_then(|state| state.mid())
                .is_none(),
            "the fixture book serves a mid, so nothing would be refused"
        );

        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(
            &offsetting_net(Decimal::parse("12345").expect("a decimal literal")),
            at(10),
            &mut report,
        );
        cell.record_cross(crossed, at(10), &mut report);

        assert!(report.crosses.is_empty(), "a cross was priced with no mid");
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "internal_cross_price"),
            "the refusal did not name the pricing gate: {:?}",
            report.refusals
        );
        Ok(())
    }
}
