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

use crate::dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
use crate::envelope::VerifiedEnvelope;
use crate::journal::{Decision, Journal, Mirror};
use crate::mesh::{CellStateDelta, DeltaOrder, DeltaRefusal, StrategyUtilisation};
use crate::policy::{VerifiedHalt, VerifiedPolicy};
use crate::seam::CellLiquidity;
use qip_contracts::capital::{CapitalGrant, Utilisation};
use qip_contracts::degradation::{DegradationState, StrategyClass};
use qip_contracts::message::{BookSide, MarketMessage};
use qip_contracts::signal::{Signal, SignalKind, StrategyId};
use qip_contracts::venue::{VenueId, VenueStatus};
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
}

impl CellConfig {
    pub fn new(cell_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            cell_id: cell_id.into(),
            region: region.into(),
            venues: Vec::new(),
            max_staleness: Duration::from_secs(5),
            strategy_budget: 4_096,
        }
    }

    pub fn with_venue(mut self, venue: VenueId) -> Self {
        self.venues.push(venue);
        self
    }
}

/// What one pass of the cell's work produced.
#[derive(Clone, Debug, Default)]
pub struct WorkReport {
    pub signals: Vec<Signal>,
    pub orders: Vec<PlacedOrder>,
    /// Every gate that said no, and why. A cell must answer "why did nothing
    /// trade" as precisely as "why did this trade".
    pub refusals: Vec<(String, String)>,
    pub halted: bool,
}

/// An order the cell actually sent.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedOrder {
    pub order_id: String,
    pub strategy: StrategyId,
    pub object_id: ObjectId,
    pub venue: VenueId,
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
            journal: Journal::new(),
            fills: Vec::new(),
            breaks: Vec::new(),
            breaks_omitted: 0,
            order_sequence: 0,
            config,
        })
    }

    pub fn config(&self) -> &CellConfig {
        &self.config
    }

    pub fn protocols_mut(&mut self) -> &mut ProtocolRegistry {
        &mut self.protocols
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
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

        let vector = self.features.evaluate(now)?;
        let strategy_ids: Vec<String> = self.deployed.keys().cloned().collect();

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
            report.signals.push(signal.clone());

            if let Some(order) = self.place(&signal, multiplier, now, gateway, &mut report)? {
                report.orders.push(order);
            }
        }

        Ok(report)
    }

    /// Take one signal through every gate to a venue, or refuse it.
    fn place(
        &mut self,
        signal: &Signal,
        multiplier: Decimal,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<Option<PlacedOrder>> {
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

        self.order_sequence += 1;
        let order_id = format!("{}-{}", self.config.cell_id, self.order_sequence);
        let simulated = gateway.is_simulated();
        gateway.place(
            &order_id,
            &signal.object_id,
            &venue,
            side,
            quantity,
            price,
            now,
        )?;

        if let Some(deployed) = self.deployed.get_mut(&key) {
            deployed.utilisation.gross_committed += quantity * price;
            deployed.utilisation.orders_sent += 1;
        }
        self.fills.push(CellFill {
            order_id: order_id.clone(),
            venue: venue.clone(),
            quantity,
            price,
        });
        self.journal.record(
            Decision::OrderSent {
                order_id: order_id.clone(),
                venue: venue.as_str().to_string(),
                quantity: quantity.to_string(),
                simulated,
            },
            now,
        );

        Ok(Some(PlacedOrder {
            order_id,
            strategy: signal.strategy.clone(),
            object_id: signal.object_id.clone(),
            venue,
            side,
            quantity,
            price,
            simulated,
        }))
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
        let Some(deployed) = self.deployed.get_mut(&key) else {
            return Err(Error::not_found(format!(
                "no strategy {key} is deployed at this cell, so there is nothing for the grant to \
                 fund; a cell does not deploy a strategy because capital arrived for it"
            )));
        };
        let approver = envelope.approver().to_string();
        let expires_at = envelope.expires_at();
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
