//! The order management system.
//!
//! Every order goes through [`OrderManager::submit`], and that method is where
//! the platform's safety controls converge. In order:
//!
//! 1. The order must be well formed and trace to a proposal and a hypothesis.
//! 2. If the instrument has a [`crate::feasibility::VenueFeasibility`]
//!    installed through [`OrderManager::with_instrument_feasibility`], or
//!    failing that the destination venue has one installed through
//!    [`OrderManager::with_venue_feasibility`], the order
//!    must sit on its lot and tick grids and clear its minimums. This is
//!    `qip-edge`'s feasibility gate mirrored onto the central path: an
//!    off-lot or below-minimum order is a strategy that does not know the
//!    venue's grid, and it is refused here rather than allowed to ride a
//!    profitable strategy's order through pre-trade risk and out to a venue
//!    that would reject it, or silently trade a size nobody reasoned about.
//!    It reports through [`RefusalReason::Infeasible`], which names the venue
//!    and the gate in fields of their own. It reported through
//!    [`RefusalReason::Malformed`] with the gate written into the detail text
//!    as `infeasible (<gate>):` until ADR 0062's follow-on, and
//!    [`RefusalReason::feasibility_gate`] recovered the gate by parsing that
//!    prefix back out of a sentence written for a person — the exact defect
//!    ADR 0061 had already named one level up, where a rule tally keyed on a
//!    refusal's wording fragments the moment the wording changes. A sentence
//!    is not a key.
//! 3. The kill switch must not be tripped for its scope.
//! 4. The autonomy level must permit execution at all.
//! 5. The venue must not have been withdrawn on feasibility evidence
//!    through [`OrderManager::withdraw_venue`] — blueprint §12.3's fourth
//!    row. Checked against the broker's name whether or not the broker is
//!    simulated, because the simulated broker is the only one the kernel
//!    constructs and its `is_available` is always true; a withdrawal that
//!    lived inside the live-venue arm below would be a control that could
//!    never fire on this platform. After the kill switch and the autonomy
//!    gate on purpose, so a halted platform still reports the halt.
//! 6. A live venue additionally requires a live autonomy level *and* an
//!    available venue. Neither implies the other.
//! 7. Pre-trade risk must approve it, against the state it would produce.
//!
//! Every one of those is a refusal path, and each records why. A rejected
//! order that left no trace is indistinguishable from one that was never sent,
//! and the difference matters when reconstructing why a position was not put on.
//!
//! The ordering is deliberate: feasibility runs right after well-formedness
//! because it is the cheapest question with a venue-specific answer — the
//! same reasoning §18.1 gives for putting the edge crate's feasibility gate
//! ahead of its profitability filter — the kill switch is checked before the
//! autonomy level so that a stopped platform stays stopped even if the level
//! is misconfigured, and risk runs last so that its expensive projection is
//! not computed for an order that was never going to be sent.

use crate::broker::Broker;
use crate::feasibility::{self, VenueFeasibility};
use crate::order::{Fill, Order, OrderState, OrderType};
use crate::session::{RecordedInstruction, RecordedSession, SealOutcome, SessionRecorder};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::OrderId;
use qip_core::time::Timestamp;
use qip_risk::limits::{LimitBreach, RiskState};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel};
use qip_risk_engine::pretrade::{PreTradeChecker, PreTradeDecision, ProposedOrder};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Why an order was refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RefusalReason {
    /// The order was not well formed: it does not validate, or it traces to
    /// no proposal and no hypothesis.
    ///
    /// A feasibility veto used to arrive here too, with the gate written into
    /// `detail` as an `infeasible (<gate>):` prefix. It has its own variant
    /// now — see [`Self::Infeasible`].
    Malformed { detail: String },
    /// The venue named cannot express the order: it is off the lot or tick
    /// grid, or below a minimum the venue states.
    ///
    /// Added by ADR 0062's follow-on, replacing the `infeasible (<gate>):`
    /// prefix on [`Self::Malformed`]. Two facts were being carried in a
    /// sentence and are now carried in fields:
    ///
    /// * `gate` is one of the `crate::feasibility::GATE_*` literals, the same
    ///   vocabulary the edge plane charts its own vetoes under, and it is the
    ///   label `qip-kernel`'s `gate_of` puts on `qip_orders_refused_total`.
    ///   Reading it out of the detail text worked until somebody reworded the
    ///   detail text, and the platform has already been bitten once by
    ///   attribution parsed from prose (ADR 0061 §1).
    /// * `venue` is [`crate::broker::Broker::name`] as `submit` saw it — the
    ///   same string the accepted path puts on `SubmissionResult::venue`, so
    ///   a refusal and a fill on one order can never be charged to different
    ///   venues. Blueprint §12.3's fourth row is keyed on it, and until this
    ///   variant existed the kernel had to re-read the broker's name at the
    ///   capture site because the refusal itself did not say where the order
    ///   had been going.
    ///
    /// Additive on the wire: the tag is `infeasible`, and a
    /// [`SubmissionResult`] serialised before this variant existed still
    /// decodes, as the `malformed` it was written as.
    Infeasible {
        venue: String,
        gate: String,
        detail: String,
    },
    /// The kill switch is tripped.
    Halted { scope: String, detail: String },
    /// The autonomy level does not permit execution.
    AutonomyTooLow {
        level: AutonomyLevel,
        required: AutonomyLevel,
    },
    /// A live venue was selected below a live autonomy level.
    LiveVenueBelowLiveAutonomy { level: AutonomyLevel, venue: String },
    /// The venue cannot be reached.
    VenueUnavailable { venue: String, detail: String },
    /// Pre-trade risk refused it.
    RiskRejected {
        reasons: Vec<String>,
        /// The blocking breaches exactly as `LimitSet::check` produced them,
        /// so the rule that refused is a name the checker wrote and never a
        /// word parsed back out of `reasons`. `reasons` is a sentence for a
        /// person; a rule tally keyed on it would fragment the moment the
        /// sentence was reworded, which is the same failure `gate_of` in
        /// `qip-kernel` already guards against one level up.
        ///
        /// Empty for a refusal that attributes to no rule: the checker
        /// rejects a state naming an unevaluated figure before any limit is
        /// weighed, and `blocking()` is then empty. Such a refusal must not
        /// be charged to a rule, and the blueprint §12.3 rows that count by
        /// rule leave it out rather than file it under a name nobody wrote.
        /// Defaulted so a record serialised before the field existed reads
        /// back.
        #[serde(default)]
        breaches: Vec<LimitBreach>,
    },
    /// The venue refused it.
    VenueRejected { detail: String },
}

impl RefusalReason {
    pub fn describe(&self) -> String {
        match self {
            Self::Malformed { detail } => format!("malformed: {detail}"),
            // Byte-for-byte the sentence the `Malformed` arm produced for a
            // feasibility veto before this variant existed, and deliberately
            // so: this string is what `Platform::capture_submission` writes
            // into the `Action::Rejected` record, and the hash-chained log
            // holds refusals written on both sides of the change. An operator
            // reading the log for a venue's vetoes must find one vocabulary
            // there, not two. Nothing parses it — the fields above are what
            // code reads now, which is the whole point of the variant.
            Self::Infeasible { gate, detail, .. } => {
                format!("malformed: infeasible ({gate}): {detail}")
            }
            Self::Halted { scope, detail } => {
                format!("trading is halted for {scope}: {detail}")
            }
            Self::AutonomyTooLow { level, required } => {
                format!("the autonomy level is {level}, and execution needs at least {required}")
            }
            Self::LiveVenueBelowLiveAutonomy { level, venue } => format!(
                "{venue} is a live venue and the autonomy level is {level}; live trading is disabled"
            ),
            Self::VenueUnavailable { venue, detail } => {
                format!("{venue} is unavailable: {detail}")
            }
            Self::RiskRejected { reasons, .. } => {
                format!("risk refused: {}", reasons.join("; "))
            }
            Self::VenueRejected { detail } => format!("the venue refused: {detail}"),
        }
    }

    /// The rules this refusal is attributed to, by the name each rule was
    /// configured under — `order-notional`, `expected-shortfall` — and never
    /// by a word read out of the refusal's sentence.
    ///
    /// The name rather than the kind, because two limits can share a kind
    /// (`MaxAxisWeight` on `sector` and on `country`) and a regret tally that
    /// merged them would propose loosening a bound that did not refuse
    /// anything. The set is bounded: a `RiskRejected` name comes from the
    /// boot-frozen `LimitSet` through the breach the checker wrote, and a
    /// feasibility veto names one of the four `feasibility::GATE_*` constants
    /// through [`Self::feasibility_gate`], which since ADR 0062's follow-on
    /// reads [`Self::Infeasible`]'s own `gate` field and resolves it against
    /// those four constants. It used to read a prefix off the refusal's
    /// sentence; a name a tally is kept under is a key, and a key parsed out
    /// of prose is one rewording away from fragmenting.
    ///
    /// Two rules on one refusal are two names, sorted and deduplicated, so a
    /// path refused by both counts for each. Every other refusal — halted,
    /// autonomy, venue — is a posture and not a rule, and attributes to none.
    ///
    /// **A breach whose `observed` or `bound` is not a finite number
    /// attributes to nothing either** (code review MEDIUM-2). `Limit::assess`
    /// files that breach at `Severity::Critical` through its `Uncomparable`
    /// arm precisely because no comparison was made — the rule did not
    /// measure the book and say no, arithmetic produced a number nobody
    /// could read — and ADR 0061 §1 already says a refusal on an unevaluated
    /// figure "carries no breach and is charged to nobody". That sentence
    /// was true only of the checker's own pre-limit rejection (empty
    /// `breaches`, the `unevaluated` case below); a `LimitBreach` the
    /// checker *did* write with a non-finite reading reached here anyway,
    /// which fired `qip_rule_fired_total` for a rule that made no comparison,
    /// ended a standing dormancy episode on a figure nobody measured, and let
    /// `RiskState::ratio`'s infinity on non-positive equity "fire" every
    /// ratio limit on a zero-equity book at once — the twin then scoring
    /// evidence that says a limit is too tight when what actually happened
    /// is that the book had no equity to divide by.
    pub fn rule_names(&self) -> Vec<String> {
        match self {
            Self::RiskRejected { breaches, .. } => breaches
                .iter()
                .filter(|breach| breach.observed.is_finite() && breach.bound.is_finite())
                .map(|breach| breach.limit_name.clone())
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect(),
            Self::Infeasible { .. } => self
                .feasibility_gate()
                .map(|gate| vec![gate.to_string()])
                .unwrap_or_default(),
            // An order that does not validate is charged to no rule: there is
            // no bound in the limit set it could propose anything about, and
            // §12.3's per-rule rows would be counting order-validation
            // failures under a name nobody configured.
            Self::Malformed { .. }
            | Self::Halted { .. }
            | Self::AutonomyTooLow { .. }
            | Self::LiveVenueBelowLiveAutonomy { .. }
            | Self::VenueUnavailable { .. }
            | Self::VenueRejected { .. } => Vec::new(),
        }
    }

    /// Whether the refusal is a safety control rather than a transient fault.
    ///
    /// Distinguished because a safety refusal must never be retried
    /// automatically, and a transient one may be. Neither `Malformed` nor
    /// `Infeasible` is a safety control, by the reasoning the single
    /// `Malformed` arm carried before the two were split: the order itself
    /// needs to change, not the platform's posture, so there is nothing here
    /// for an automatic retry to trip over.
    pub const fn is_safety_control(&self) -> bool {
        matches!(
            self,
            Self::Halted { .. }
                | Self::AutonomyTooLow { .. }
                | Self::LiveVenueBelowLiveAutonomy { .. }
                | Self::RiskRejected { .. }
        )
    }

    /// Whether this refusal is a judgment about the order — its shape or
    /// its risk — rather than about the platform's or a venue's posture.
    ///
    /// A code-review finding on ADR 0062: `Platform::capture_submission`
    /// queued *every* refusal, including [`Self::VenueUnavailable`], for the
    /// twin to price, and ADR 0055's `counterfactual_sizing_multiplier`
    /// groups what the twin priced by instrument alone, with no gate filter.
    /// Once a venue is withdrawn, every further order routed there is
    /// refused this way, every cycle, for as long as it stays withdrawn —
    /// and each one is a refusal the venue's own reachability made, not one
    /// a control decided by judging the order. Ten such refusals, easily
    /// reached within a cycle or two of a withdrawal, would flood that
    /// instrument's declined-score sample with evidence that has nothing to
    /// do with whether the order was well sized, and could halve its sizing
    /// confidence for a reason no rule found.
    ///
    /// [`Self::Malformed`] (the order traced to no hypothesis),
    /// [`Self::Infeasible`] (the order's own shape — off its lot or tick
    /// grid, or below a minimum) and [`Self::RiskRejected`] (a control
    /// weighed the book and said no) are judgments about *this* order and
    /// stay evidence. [`Self::Halted`], [`Self::AutonomyTooLow`],
    /// [`Self::LiveVenueBelowLiveAutonomy`], [`Self::VenueUnavailable`] and
    /// [`Self::VenueRejected`] are about the platform's posture or a
    /// venue's state — the same order submitted a moment earlier or later,
    /// or to a different venue, could have drawn any of the five for
    /// reasons that have nothing to do with its size — and none of them
    /// reach the queue [`Self::rule_names`]'s doc comment already calls
    /// "not a rule": a posture is not sizing evidence either.
    ///
    /// `Infeasible` is named here explicitly because splitting it out of
    /// `Malformed` is exactly the change that could have dropped it: this is
    /// a `matches!` and not an exhaustive match, so a new variant is admitted
    /// to no arm and returns `false` without the compiler saying anything. A
    /// feasibility veto was sizing evidence before the split and is sizing
    /// evidence after it; `only_a_judgment_about_the_order_itself_is_sizing_
    /// evidence` holds every variant to its side.
    pub const fn is_sizing_evidence(&self) -> bool {
        matches!(
            self,
            Self::Malformed { .. } | Self::Infeasible { .. } | Self::RiskRejected { .. }
        )
    }

    /// The `feasibility_*` gate literal this refusal names, if it is a
    /// feasibility veto rather than any other refusal.
    ///
    /// Read from [`Self::Infeasible`]'s own `gate` field, and still resolved
    /// *against* the four constants rather than returned as it was found: the
    /// answer is a metric label value and an attribution key, so its
    /// cardinality must be the gate constants' and not whatever string a
    /// decoded record happens to carry. A `gate` that matches none of the
    /// four is no gate — the refusal charts under `order-validation` like any
    /// other malformation and attributes to no rule — which is the same
    /// fail-closed answer the prefix match gave a sentence it did not
    /// recognise.
    ///
    /// It read that prefix out of `detail` until ADR 0062's follow-on. The
    /// prefix was exact, so it was never wrong; it was a key recovered from a
    /// sentence written for a person, and the day somebody improved the
    /// sentence the key would have gone silently missing — the kernel would
    /// have charted every off-lot order under `order-validation` again, and
    /// §12.3's fourth row would have stopped seeing a venue's refusals
    /// without a single test failing.
    pub fn feasibility_gate(&self) -> Option<&'static str> {
        let Self::Infeasible { gate, .. } = self else {
            return None;
        };
        [
            feasibility::GATE_MINIMUM_QUANTITY,
            feasibility::GATE_MINIMUM_NOTIONAL,
            feasibility::GATE_LOT,
            feasibility::GATE_TICK,
        ]
        .into_iter()
        .find(|known| known == gate)
    }
}

/// What happened to a submission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmissionResult {
    pub order_id: OrderId,
    pub at: Timestamp,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refusal: Option<RefusalReason>,
    pub fills: Vec<Fill>,
    /// The venue it went to, if it went anywhere.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub venue: Option<String>,
    /// Whether the fills came from a simulated venue.
    pub simulated: bool,
    /// If the order was resized by risk, what it was resized to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reduced_to: Option<Decimal>,
    /// Where the venue and the order disagreed about what happened.
    ///
    /// A venue that reports a fill the order refuses — an over-fill, a fill on
    /// a closed order — puts the book out of step with reality. Discarding
    /// that refusal is how the divergence becomes invisible, so it is carried
    /// here and counted on the manager instead.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub reconciliation_breaks: Vec<String>,
}

impl SubmissionResult {
    pub fn filled_quantity(&self) -> Decimal {
        self.fills
            .iter()
            .map(|fill| fill.quantity)
            .fold(Decimal::ZERO, |a, b| a + b)
    }
}

/// Manages the order lifecycle.
#[derive(Debug)]
pub struct OrderManager {
    orders: BTreeMap<String, Order>,
    checker: PreTradeChecker,
    /// Submissions that were refused, kept so a missing position can be
    /// explained.
    refusals: Vec<SubmissionResult>,
    /// Every venue/book disagreement seen, so a monitor can halt on one.
    reconciliation_breaks: Vec<String>,
    /// Feasibility grids by venue name, keyed on [`Broker::name`]. A venue
    /// with no entry is checked for nothing here — see
    /// `crate::feasibility`'s module comment for why that is the honest
    /// answer rather than a gap.
    feasibility: BTreeMap<String, VenueFeasibility>,
    /// Feasibility grids by instrument, keyed on the order's `object_id`, and
    /// consulted ahead of the venue's. A lot and a tick are facts about the
    /// instrument's listing, which is where a reference catalogue states
    /// them; a venue-wide grid is the coarser statement for a venue whose
    /// instruments all share one.
    instrument_feasibility: BTreeMap<String, VenueFeasibility>,
    /// Venues withdrawn on feasibility evidence, keyed on [`Broker::name`].
    /// A subtractive set: a name here is refused at step 5 of
    /// [`Self::submit`], and nothing in this manager reads it to admit
    /// anything. Written only through [`Self::withdraw_venue`] and
    /// [`Self::reinstate_venue`], which the kernel calls after it has
    /// journaled the decision; the set is a cache of the event log, not a
    /// second source of truth.
    withdrawn_venues: BTreeSet<String>,
    /// §34.4's recorded sessions: what the desk instructed at each venue and
    /// what the venue answered, bounded and sealed a pass at a time. The
    /// simulated rung of the venue promotion ladder is judged on a replay of
    /// these, and before they existed nothing in any binary could construct
    /// the evidence that rung asks for.
    sessions: SessionRecorder,
    sequence: u64,
}

impl OrderManager {
    pub fn new(checker: PreTradeChecker) -> Self {
        Self {
            orders: BTreeMap::new(),
            checker,
            refusals: Vec::new(),
            reconciliation_breaks: Vec::new(),
            feasibility: BTreeMap::new(),
            instrument_feasibility: BTreeMap::new(),
            withdrawn_venues: BTreeSet::new(),
            sessions: SessionRecorder::default(),
            sequence: 0,
        }
    }

    /// Seal the recorded session at every venue this pass touched.
    ///
    /// Called by the composition at the end of the stage that issued the
    /// orders, because that is what delimits a session: a session is the
    /// traffic of one pass, and a boundary drawn anywhere else would be a
    /// boundary nobody could reproduce from the record.
    pub fn close_sessions(&mut self, at: Timestamp) -> Vec<(String, SealOutcome)> {
        self.sessions.close_all(at)
    }

    /// The sealed sessions at one venue, oldest first.
    pub fn sessions(&self, venue: &str) -> Vec<&RecordedSession> {
        self.sessions.sessions(venue)
    }

    /// Refuse every further order bound for `venue`, until it is reinstated.
    ///
    /// Idempotent: withdrawing a withdrawn venue changes nothing, so a
    /// resumed set and a fresh finding cannot disagree about the state.
    pub fn withdraw_venue(&mut self, venue: impl Into<String>) {
        self.withdrawn_venues.insert(venue.into());
    }

    /// Admit orders to `venue` again. `true` if it was withdrawn.
    ///
    /// The most this can do is remove a name from a subtractive set: an
    /// order to the reinstated venue still walks every other gate in
    /// [`Self::submit`], and a venue the broker is not configured for is
    /// not made reachable by not being withdrawn.
    pub fn reinstate_venue(&mut self, venue: &str) -> bool {
        self.withdrawn_venues.remove(venue)
    }

    /// Whether `venue` is currently withdrawn.
    pub fn is_withdrawn(&self, venue: &str) -> bool {
        self.withdrawn_venues.contains(venue)
    }

    /// The venues currently withdrawn, in name order.
    pub fn withdrawn_venues(&self) -> &BTreeSet<String> {
        &self.withdrawn_venues
    }

    /// Install the lot/tick/minimum grid for one instrument, keyed on the
    /// exact string its `object_id` renders to.
    ///
    /// Takes precedence over [`OrderManager::with_venue_feasibility`] for
    /// that instrument, because the instrument's own record is the more
    /// specific claim: a venue-wide lot of one is true of most of an equity
    /// venue and false of the board lot a particular listing states, and the
    /// order that would be wrong is the one in that listing.
    #[must_use]
    pub fn with_instrument_feasibility(
        mut self,
        object_id: impl Into<String>,
        model: VenueFeasibility,
    ) -> Self {
        self.instrument_feasibility.insert(object_id.into(), model);
        self
    }

    /// The grid installed for one instrument, if any — so the stage that
    /// sizes a leg can express it in whole lots before this manager judges
    /// it, from the same grid, rather than from a second copy of the lot.
    pub fn instrument_feasibility(&self, object_id: &str) -> Option<&VenueFeasibility> {
        self.instrument_feasibility.get(object_id)
    }

    /// Install the lot/tick/minimum grid for one venue, keyed on the exact
    /// string a [`Broker::name`] returns.
    ///
    /// Opt in per venue rather than a single default: a grid guessed at for a
    /// venue nobody has modelled is a rounding rule wearing a refusal's
    /// clothes, and this platform refuses only what it actually knows.
    #[must_use]
    pub fn with_venue_feasibility(
        mut self,
        venue: impl Into<String>,
        model: VenueFeasibility,
    ) -> Self {
        self.feasibility.insert(venue.into(), model);
        self
    }

    /// Every venue/book disagreement recorded since assembly.
    ///
    /// Non-empty means the platform's positions may not match the venue's, and
    /// nothing downstream should be trusted until it is reconciled.
    pub fn reconciliation_breaks(&self) -> &[String] {
        &self.reconciliation_breaks
    }

    pub fn order(&self, order_id: &OrderId) -> Option<&Order> {
        self.orders.get(order_id.as_str())
    }

    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }

    pub fn open_orders(&self) -> Vec<&Order> {
        self.orders
            .values()
            .filter(|order| order.state.is_open())
            .collect()
    }

    pub fn refusals(&self) -> &[SubmissionResult] {
        &self.refusals
    }

    /// Refusals that were safety controls rather than transient faults.
    pub fn safety_refusals(&self) -> Vec<&SubmissionResult> {
        self.refusals
            .iter()
            .filter(|result| {
                result
                    .refusal
                    .as_ref()
                    .is_some_and(RefusalReason::is_safety_control)
            })
            .collect()
    }

    /// Every fill recorded, across all orders.
    pub fn fills(&self) -> Vec<&Fill> {
        self.orders
            .values()
            .flat_map(|order| order.fills.iter())
            .collect()
    }

    /// Whether any fill in the system came from a real venue.
    ///
    /// The question a reconciliation asks, answered from the fills themselves
    /// rather than from configuration — because configuration is exactly what
    /// gets confused between a test and a deployment.
    pub fn has_live_fills(&self) -> bool {
        self.fills().iter().any(|fill| !fill.simulated)
    }

    /// Submit an order. The single path to a venue.
    ///
    /// A `counterparty` the caller can name becomes one more exposure axis on
    /// the projected state, under [`qip_risk::limits::COUNTERPARTY_AXIS`], so
    /// `LimitKind::MaxCounterpartyExposure` reads the same running per-bucket
    /// counter the sector and country caps read. `None` adds no axis and makes
    /// no claim: a caller that cannot say who is on the other side must not
    /// have one invented for it, because the cap would then bind against a
    /// name nobody chose.
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &mut self,
        mut order: Order,
        broker: &mut dyn Broker,
        autonomy: &AutonomyController,
        risk_state: &RiskState,
        mut axes: BTreeMap<String, String>,
        counterparty: Option<String>,
        at: Timestamp,
    ) -> SubmissionResult {
        let refuse = |order: &Order, reason: RefusalReason| SubmissionResult {
            order_id: order.order_id.clone(),
            at,
            accepted: false,
            refusal: Some(reason),
            fills: Vec::new(),
            venue: None,
            simulated: broker.is_simulated(),
            reduced_to: None,
            reconciliation_breaks: Vec::new(),
        };

        // 1. Well formed, and traceable.
        if let Err(error) = order.validate() {
            let result = refuse(
                &order,
                RefusalReason::Malformed {
                    detail: error.message().to_string(),
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 2. Feasibility, before anything spends effort on an order that
        //    cannot be expressed at this venue at all. Opt in per instrument,
        //    then per venue: an instrument with no grid at a venue with no
        //    grid is checked for nothing, which mirrors `qip-edge`'s stated
        //    behaviour for a venue it has not modelled either.
        if let Some(model) = self
            .instrument_feasibility
            .get(order.object_id.as_str())
            .or_else(|| self.feasibility.get(broker.name()))
            && let Err(infeasible) = feasibility::assess(model, &order)
        {
            let result = refuse(
                &order,
                RefusalReason::Infeasible {
                    // The broker's name, not the order's idea of where it was
                    // going: this is the string `SubmissionResult::venue`
                    // carries on the accepted path a few lines down, and one
                    // order's refusal and one order's fill charged to
                    // different venues would make §12.3's fourth row count
                    // refusals at a venue that never saw the order.
                    venue: broker.name().to_string(),
                    gate: infeasible.gate.to_string(),
                    detail: infeasible.reason,
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 3. The kill switch, checked first among the safety controls so a
        //    stopped platform stays stopped even if the autonomy level is
        //    misconfigured.
        if autonomy.kill_switch().is_halted(&order.scope) {
            let detail = autonomy
                .kill_switch()
                .global_trip()
                .or_else(|| autonomy.kill_switch().scoped_trip(&order.scope))
                .map(|trip| format!("{} ({})", trip.reason, trip.tripped_by))
                .unwrap_or_else(|| "no reason recorded".to_string());
            let result = refuse(
                &order,
                RefusalReason::Halted {
                    scope: order.scope.clone(),
                    detail,
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 4. Execution must be permitted at all.
        let level = autonomy.level();
        if !level.executes() {
            let result = refuse(
                &order,
                RefusalReason::AutonomyTooLow {
                    level,
                    required: AutonomyLevel::PaperTrading,
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 5. A venue withdrawn on feasibility evidence refuses every order,
        //    simulated or not — see the module doc for why this cannot live
        //    in the live-venue arm below. Reported through the same
        //    `VenueUnavailable` reason an unreachable live venue uses, so
        //    the kernel's `gate_of` charts both under `venue-availability`
        //    and no new label value appears. Feasibility ran at step 2, so a
        //    withdrawn venue's further infeasible orders still reach the
        //    kernel's window, which keeps its denominator honest.
        if self.withdrawn_venues.contains(broker.name()) {
            let result = refuse(
                &order,
                RefusalReason::VenueUnavailable {
                    venue: broker.name().to_string(),
                    detail: "withdrawn on feasibility evidence; reinstatement needs two \
                             operator signatures"
                        .to_string(),
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 6. A live venue needs a live level *and* an available venue. Neither
        //    implies the other, and treating either as sufficient is how an
        //    order reaches a market nobody intended it to reach.
        if !broker.is_simulated() {
            if !level.is_live() {
                let result = refuse(
                    &order,
                    RefusalReason::LiveVenueBelowLiveAutonomy {
                        level,
                        venue: broker.name().to_string(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
            if !broker.is_available() {
                let result = refuse(
                    &order,
                    RefusalReason::VenueUnavailable {
                        venue: broker.name().to_string(),
                        detail: broker.requirement(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
        }

        // 7. Pre-trade risk, against the state the order would produce.
        if let Some(counterparty) = counterparty {
            axes.insert(
                qip_risk::limits::COUNTERPARTY_AXIS.to_string(),
                counterparty,
            );
        }
        let proposed = ProposedOrder {
            object_id: order.object_id.clone(),
            quantity: order.quantity * Decimal::from_int(i64::from(order.side.sign())),
            reference_price: order.arrival_price,
            axes,
            scope: order.scope.clone(),
        };
        let check = match self.checker.check(&proposed, risk_state, at) {
            Ok(check) => check,
            Err(error) => {
                let result = refuse(
                    &order,
                    RefusalReason::Malformed {
                        detail: error.message().to_string(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
        };

        let mut reduced_to = None;
        match &check.decision {
            PreTradeDecision::Rejected { reasons } => {
                // The breaches are carried beside the sentence, not derived
                // from it: `post_trade_check` is the checker's own record of
                // which limits bound, and it used to be dropped here — so the
                // kernel could count that pre-trade risk refused, but not
                // which rule did.
                let result = refuse(
                    &order,
                    RefusalReason::RiskRejected {
                        reasons: reasons.clone(),
                        breaches: check
                            .post_trade_check
                            .blocking()
                            .into_iter()
                            .cloned()
                            .collect(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
            PreTradeDecision::Reduced {
                permitted_quantity, ..
            } => {
                let permitted = permitted_quantity.abs();
                order.quantity = permitted;
                reduced_to = Some(permitted);
            }
            PreTradeDecision::Approved => {}
        }

        // Cleared. Send it.
        if order
            .transition(OrderState::RiskApproved { at }, at)
            .is_err()
        {
            let result = refuse(
                &order,
                RefusalReason::Malformed {
                    detail: format!("order is already {}", order.state.as_str()),
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }
        // Every refused transition and every refused fill is recorded. The
        // state machine says these cannot happen from here; a break that
        // "cannot happen" and is discarded is one nobody ever finds out about.
        let mut breaks: Vec<String> = Vec::new();
        if let Err(error) = order.transition(
            OrderState::Working {
                at,
                venue: broker.name().to_string(),
            },
            at,
        ) {
            breaks.push(format!(
                "order {} could not be marked working: {}",
                order.order_id.as_str(),
                error.message()
            ));
        }

        // The desk's half of §34.4's recorded session, written before the
        // venue is asked and from the order rather than from any answer. The
        // two halves are only evidence because they were recorded
        // independently; a quantity read back out of a fill would make the
        // reconciliation a tautology.
        self.sessions.instruct(RecordedInstruction {
            order_id: order.order_id.clone(),
            object_id: order.object_id.clone(),
            side: order.side,
            quantity: order.quantity,
            venue: broker.name().to_string(),
            at,
        });

        match broker.submit(&order, at) {
            Ok(fills) => {
                for fill in &fills {
                    // A broker that reported the wrong simulation flag would
                    // let a paper fill be counted as real; the OMS does not
                    // take the broker's word for it.
                    let mut fill = fill.clone();
                    fill.simulated = broker.is_simulated();
                    let quantity = fill.quantity;
                    // The venue's half, recorded *before* `apply_fill` has
                    // had a chance to reject it. Recording the accepted set
                    // instead would leave the replay unable to find an
                    // overfill at all, so every session would reconcile
                    // perfectly forever — zero breaks on the strength of the
                    // breaks having been discarded first.
                    self.sessions.answer(broker.name(), fill.clone(), at);
                    if let Err(error) = order.apply_fill(fill) {
                        breaks.push(format!(
                            "venue {} reported a fill of {} on order {} that the order refused: {}",
                            broker.name(),
                            quantity,
                            order.order_id.as_str(),
                            error.message()
                        ));
                    }
                }
                self.reconciliation_breaks.extend(breaks.iter().cloned());
                let result = SubmissionResult {
                    order_id: order.order_id.clone(),
                    at,
                    accepted: true,
                    refusal: None,
                    fills: order.fills.clone(),
                    venue: Some(broker.name().to_string()),
                    simulated: broker.is_simulated(),
                    reduced_to,
                    reconciliation_breaks: breaks,
                };
                self.orders
                    .insert(order.order_id.as_str().to_string(), order);
                result
            }
            Err(error) => {
                if let Err(transition) = order.transition(
                    OrderState::Rejected {
                        at,
                        reason: error.message().to_string(),
                    },
                    at,
                ) {
                    breaks.push(format!(
                        "order {} could not be marked rejected: {}",
                        order.order_id.as_str(),
                        transition.message()
                    ));
                }
                self.reconciliation_breaks.extend(breaks.iter().cloned());
                let result = SubmissionResult {
                    order_id: order.order_id.clone(),
                    at,
                    accepted: false,
                    refusal: Some(RefusalReason::VenueRejected {
                        detail: error.message().to_string(),
                    }),
                    fills: Vec::new(),
                    venue: Some(broker.name().to_string()),
                    simulated: broker.is_simulated(),
                    reduced_to,
                    reconciliation_breaks: breaks,
                };
                self.orders
                    .insert(order.order_id.as_str().to_string(), order);
                self.refusals.push(result.clone());
                result
            }
        }
    }

    /// Cancel a working order.
    pub fn cancel(
        &mut self,
        order_id: &OrderId,
        broker: &mut dyn Broker,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<()> {
        let Some(order) = self.orders.get_mut(order_id.as_str()) else {
            return Err(Error::not_found(format!("no order {}", order_id.as_str())));
        };
        if !order.state.is_open() {
            return Err(Error::invalid(format!(
                "order {} is {} and cannot be cancelled",
                order_id.as_str(),
                order.state.as_str()
            )));
        }
        broker.cancel(order, at)?;
        order.transition(
            OrderState::Cancelled {
                at,
                reason: reason.into(),
            },
            at,
        )
    }

    /// Cancel every open order in a scope, for a kill switch.
    ///
    /// Returns the ids cancelled and the ones that could not be, because a
    /// halt that silently left orders working would be worse than no halt.
    pub fn cancel_scope(
        &mut self,
        scope: &str,
        broker: &mut dyn Broker,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> (Vec<OrderId>, Vec<(OrderId, String)>) {
        let reason = reason.into();
        let targets: Vec<OrderId> = self
            .orders
            .values()
            .filter(|order| order.state.is_open() && order.scope == scope)
            .map(|order| order.order_id.clone())
            .collect();

        let mut cancelled = Vec::new();
        let mut failed = Vec::new();
        for order_id in targets {
            match self.cancel(&order_id, broker, reason.clone(), at) {
                Ok(()) => cancelled.push(order_id),
                Err(error) => failed.push((order_id, error.message().to_string())),
            }
        }
        (cancelled, failed)
    }

    /// A fresh order id, deterministic across a replay.
    pub fn next_order_id(&mut self, prefix: &str) -> OrderId {
        self.sequence += 1;
        OrderId::from_string(format!("{prefix}-{}", self.sequence))
    }

    fn record_refusal(&mut self, order: Order, at: Timestamp, result: SubmissionResult) {
        let mut order = order;
        if let Some(reason) = &result.refusal {
            let _ = order.transition(
                OrderState::Rejected {
                    at,
                    reason: reason.describe(),
                },
                at,
            );
        }
        self.orders
            .insert(order.order_id.as_str().to_string(), order);
        self.refusals.push(result);
    }
}

/// Choose an order type for a given size relative to available liquidity.
///
/// A market order in an illiquid name is how a small position becomes a large
/// loss, so anything above a few percent of daily volume is worked rather than
/// taken.
///
/// # Why a non-finite participation is refused rather than answered
///
/// This returned [`OrderType::Market`] — the one type that accepts whatever
/// price the market gives — for a participation that was `NaN` or infinite,
/// because the guard `!participation.is_finite()` sat in the same arm as
/// `participation <= 0.01`. A participation is `size / daily_volume`, so it
/// is `NaN` exactly when the volume is zero or could not be measured: the
/// illiquid case this function exists to work rather than take. Answering the
/// unmeasurable case with the most aggressive type is failing open at the one
/// seam whose whole purpose is failing closed.
///
/// A non-positive participation is refused on the same reasoning rather than
/// clamped to the market arm: an order that is zero or a negative share of
/// daily volume is a caller that computed something wrong, and a value
/// silently corrected here is that bug surviving into a fill.
///
/// `participation` is a ratio and not money, so it is `f64` by the statistics
/// rule rather than by an exception to the `Decimal` one.
pub fn order_type_for(participation: f64, window_minutes: u32) -> Result<OrderType> {
    if !participation.is_finite() || participation <= 0.0 {
        return Err(Error::invalid(format!(
            "participation {participation} is not a positive finite share of daily volume, so no \
             order type can be chosen for it; measure the volume, or refuse the order rather than \
             taking the market's price for a size nobody sized"
        )));
    }
    Ok(if participation <= 0.01 {
        OrderType::Market
    } else if participation <= 0.05 {
        OrderType::TimeWeighted {
            minutes: window_minutes,
        }
    } else if participation <= 0.15 {
        OrderType::VolumeWeighted {
            minutes: window_minutes,
        }
    } else {
        // Beyond fifteen percent of a day's volume, the order sets the price
        // rather than taking it, and a fixed participation rate is the only
        // sensible way to work it.
        OrderType::Participation { rate: 0.10 }
    })
}
