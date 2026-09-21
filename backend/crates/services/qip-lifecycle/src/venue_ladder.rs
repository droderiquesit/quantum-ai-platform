//! Blueprint §34.4: a venue walks up a ladder, and every rung is earned
//! against measurement rather than against the venue's own documentation.
//!
//! # Why this is the strategy ladder and not a second one
//!
//! The blueprint draws §34.4 as a six-rung table with a gate per rung, which
//! is the shape [`qip_contracts::gate::GateStage`] already holds for
//! strategies: one step at a time, no skipping, promotion needing evidence
//! and demotion needing nobody. Building a second enum for venues would have
//! given the platform two answers to one question — "what does it take to
//! move something up a rung" — and the two would have drifted at the first
//! change to either. So the subject changes and the ladder does not:
//!
//! | §34.4 rung | [`GateStage`] | Reachable here |
//! |---|---|---|
//! | Registered  | `Candidate` | yes |
//! | Observed    | `Holdout`   | yes |
//! | Simulated   | `Paper`     | yes — and this is the ceiling |
//! | Shadow      | `Shadow`    | **no** |
//! | Capped live | `Pilot`     | **no** |
//! | Full        | `Scaled`    | **no** |
//!
//! `Holdout` for "Observed" is the closest correspondence rather than the
//! most obvious one, and it is exact: a holdout rung judges a claim against
//! data held out of the fitting that produced it, and the observed rung
//! judges a venue's declared fees, latency and order types against
//! measurement taken independently of those declarations. Both refuse the
//! same fallacy, which is a number checking itself.
//!
//! # Promotion here cannot enable a live venue, and that is structural
//!
//! **This module promotes a venue into the *simulator*, never into real
//! execution.** [`VENUE_PROMOTION_CEILING`] is [`GateStage::Paper`], and
//! [`attempt_promotion`] refuses any step above it by *computing* the target
//! from [`GateStage::next`] and comparing, so there is no argument a caller
//! can pass to reach a higher rung and no policy field that raises the
//! ceiling. Three facts make the refusal the honest one rather than a
//! placeholder:
//!
//! * There is no live-class adapter in this workspace to promote a venue on
//!   to. `qip_brokers::AdapterClass` has two variants, `Simulated` and
//!   `Sandbox`, and no string deserialises into a third.
//! * A venue reached from this ladder is reached through the desk's
//!   submission ladder in `qip_execution_engine::oms`, whose
//!   `LiveVenueBelowLiveAutonomy` step refuses a non-simulated broker below a
//!   live autonomy level. Nothing here touches that ladder, and nothing here
//!   is consulted by it. (The type's own name is deliberately not written in
//!   this file: `qip-acceptance`'s `architecture` suite refuses the literal
//!   anywhere outside a composition root, on a plain substring match, because
//!   a crate that *constructs* one has an order path nobody reviewed — and a
//!   doc comment that merely mentions it is indistinguishable from one that
//!   holds it, to a check that cannot afford to be clever.)
//! * The platform's autonomy ceiling is paper trading at three independent
//!   layers (ADR 0003), none of which this module can see, let alone raise.
//!
//! [`VenueLadder::admits`] therefore answers one question — may the simulator
//! use this venue — and there is no method on this type that answers "may
//! real capital".
//!
//! # A gate that no venue could cross would be worse than no gate
//!
//! Every check below has a passing case and a failing case in this module's
//! own tests, and the two are driven from the same fixture so a refusal is
//! attributable to its own rule. That is not ceremony: this repository's
//! standing example of what not to ship is a limit that read as protection
//! and could never fire.

use qip_contracts::gate::{GateOutcome, GateStage, Promotion};
use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_financial::pool::DexModel;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The highest rung a venue can reach in this build: the simulator.
///
/// Not a default and not a policy field. Raising it would require editing
/// this line, which is the point — a ceiling a configuration can move is a
/// ceiling somebody moves at three in the morning.
pub const VENUE_PROMOTION_CEILING: GateStage = GateStage::Paper;

/// The blueprint's own name for each rung, so a record and a refusal read in
/// the words §34.4 uses rather than in the strategy ladder's.
///
/// Total over [`GateStage`] because the three rungs this build cannot reach
/// still need names: a refusal that says "shadow is not a rung this platform
/// has" is more use than one that says "paper is the maximum".
pub const fn rung_name(stage: GateStage) -> &'static str {
    match stage {
        GateStage::Candidate => "registered",
        GateStage::Holdout => "observed",
        GateStage::Paper => "simulated",
        GateStage::Shadow => "shadow",
        GateStage::Pilot => "capped_live",
        GateStage::Scaled => "full",
        GateStage::Retired => "withdrawn",
    }
}

/// The literals each check reports under, declared once so a finding keyed on
/// one is bounded by this file.
pub const CHECK_MEASUREMENT_PRESENT: &str = "measurement_present";
pub const CHECK_SAMPLE_SUFFICIENT: &str = "sample_sufficient";
pub const CHECK_FEES_NOT_UNDERSTATED: &str = "fees_not_understated";
pub const CHECK_LATENCY_NOT_UNDERSTATED: &str = "latency_not_understated";
pub const CHECK_ORDER_TYPES_VERIFIED: &str = "order_types_verified";
pub const CHECK_SIMULATION_PRESENT: &str = "simulation_present";
pub const CHECK_SESSIONS_REPLAYED: &str = "sessions_replayed";
pub const CHECK_RECONCILIATION_VERIFIED: &str = "reconciliation_verified";
pub const CHECK_DEX_MODEL_COMPLETE: &str = "dex_model_complete";
pub const CHECK_CROSSING_COST_WITHIN_BOUND: &str = "crossing_cost_within_bound";

/// What a venue says about itself.
///
/// Every field here is a claim, and §34.4's whole argument is that a claim is
/// not evidence: "a venue is never enabled from its own documentation alone".
/// The type exists so that the claim and the measurement are two values that
/// can be compared, rather than one value that was believed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueDeclaration {
    venue: VenueId,
    class: VenueClass,
    /// The taker fee the venue's documentation states, in basis points.
    fee_bps: Decimal,
    /// The acknowledgement latency the venue's documentation states.
    latency: Duration,
    /// The order types the venue's documentation says it accepts.
    order_types: BTreeSet<String>,
}

impl VenueDeclaration {
    /// Refuses a negative fee, a negative latency, and an empty order-type
    /// list.
    ///
    /// The empty list is the interesting refusal. A venue declaring no order
    /// types would pass [`ObservedGate`]'s order-type check vacuously — every
    /// member of an empty set is supported — and arrive at the simulator with
    /// nothing verified. An unstated capability is a question nobody asked,
    /// not a capability that is absent.
    pub fn new(
        venue: VenueId,
        class: VenueClass,
        fee_bps: Decimal,
        latency: Duration,
        order_types: BTreeSet<String>,
    ) -> Result<Self> {
        if fee_bps.is_negative() {
            return Err(Error::invalid(format!(
                "{venue} declares a fee of {fee_bps} basis points; state a non-negative figure, \
                 because a venue that pays takers is a rebate and belongs in its own field"
            )));
        }
        if latency.as_nanos() < 0 {
            return Err(Error::invalid(format!(
                "{venue} declares a latency of {} nanoseconds; state a non-negative duration",
                latency.as_nanos()
            )));
        }
        if order_types.is_empty() {
            return Err(Error::invalid(format!(
                "{venue} declares no order types; the observed rung verifies declared support \
                 against measurement and an empty declaration would pass that check having \
                 verified nothing"
            )));
        }
        Ok(Self {
            venue,
            class,
            fee_bps,
            latency,
            order_types,
        })
    }

    pub const fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub const fn class(&self) -> VenueClass {
        self.class
    }

    pub const fn fee_bps(&self) -> Decimal {
        self.fee_bps
    }

    pub const fn latency(&self) -> Duration {
        self.latency
    }

    pub const fn order_types(&self) -> &BTreeSet<String> {
        &self.order_types
    }
}

/// What connecting read-only actually found.
///
/// Built by whatever observed the venue — an adapter's own acknowledgement
/// statistics, a recorded session replayed — and never by this crate, which
/// judges but does not measure.
///
/// # Why two of these fields are options
///
/// A venue that does not report a fact has to be distinguishable from one
/// reporting zero, and these two are the fields where the difference decides
/// a promotion. A latency of `Some(Duration::ZERO)` says the venue answered
/// instantly; `None` says nobody timed it. Read as a figure, the second
/// clears the latency check against any declaration whatsoever — a venue
/// nobody measured would be promoted for being faster than it claimed. The
/// same holds for a fee of zero, which makes a venue the cheapest one
/// available.
///
/// So neither has a default and neither is filled in by this crate. A
/// measurement that is absent fails its check by name, which is
/// [`CHECK_LATENCY_NOT_UNDERSTATED`] and [`CHECK_FEES_NOT_UNDERSTATED`]
/// reporting what they could not evaluate rather than evaluating nothing and
/// passing. A ladder fed a fabricated measurement is worse than an empty one,
/// because an empty ladder is visibly empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueMeasurement {
    /// The fee actually charged, in basis points, over `observations`.
    ///
    /// [`None`] from a venue that itemises no fees, and from one that has
    /// filled no notional to divide by.
    pub fee_bps: Option<Decimal>,
    /// The acknowledgement latency actually seen.
    ///
    /// [`None`] from a venue that does not time its acknowledgements —
    /// including every venue that applies a configured latency rather than
    /// measuring one, because a setting read back is not an observation.
    pub latency: Option<Duration>,
    /// Order types the venue accepted when one was sent.
    pub accepted_order_types: BTreeSet<String>,
    /// Order types the venue rejected when one was sent. Kept separately from
    /// "not accepted" because a type nobody tried and a type the venue
    /// refused are different findings, and only the second is the venue
    /// misdescribing itself.
    pub rejected_order_types: BTreeSet<String>,
    /// How many acknowledgements the figures above are taken over.
    pub observations: usize,
}

/// What trading it in the simulator against replayed data found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationEvidence {
    /// Recorded sessions replayed through the simulator.
    pub replayed_sessions: usize,
    /// Reconciliation breaks found across those sessions. §34.4's simulated
    /// rung is "traded in sim against recorded and replayed data,
    /// reconciliation verified", and verified means none.
    pub reconciliation_breaks: usize,
    /// The clip the crossing cost is measured at, in the pool's input units.
    /// A cost figure with no size attached is not a figure.
    pub reference_clip: Decimal,
}

/// Everything a venue has offered in support of its next rung.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct VenueEvidence {
    measurement: Option<VenueMeasurement>,
    simulation: Option<SimulationEvidence>,
    dex: Option<DexModel>,
}

impl VenueEvidence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_measurement(mut self, measurement: VenueMeasurement) -> Self {
        self.measurement = Some(measurement);
        self
    }

    pub fn with_simulation(mut self, simulation: SimulationEvidence) -> Self {
        self.simulation = Some(simulation);
        self
    }

    /// The §34.3 model, for a venue whose class is
    /// [`VenueClass::DecentralisedExchange`]. Absent for every other class,
    /// and absent for a decentralised venue that has not built one — which is
    /// the case [`SimulatedGate`] refuses.
    pub fn with_dex_model(mut self, dex: DexModel) -> Self {
        self.dex = Some(dex);
        self
    }

    pub fn measurement(&self) -> Option<&VenueMeasurement> {
        self.measurement.as_ref()
    }

    pub fn simulation(&self) -> Option<SimulationEvidence> {
        self.simulation
    }

    pub fn dex_model(&self) -> Option<&DexModel> {
        self.dex.as_ref()
    }
}

/// The thresholds the venue rungs apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VenuePromotionPolicy {
    /// Acknowledgements the observed rung needs before a measured fee or
    /// latency means anything.
    pub minimum_observations: usize,
    /// How far the measured fee may exceed the declared one, in basis points,
    /// before the venue is judged to have understated it.
    pub fee_tolerance_bps: Decimal,
    /// How far the measured latency may exceed the declared one.
    pub latency_tolerance: Duration,
    /// Recorded sessions the simulated rung needs replayed.
    pub minimum_replayed_sessions: usize,
    /// The most a decentralised venue may cost to cross, in basis points, at
    /// the reference clip — slippage plus the MEV headroom together.
    pub maximum_crossing_cost_bps: Decimal,
}

impl Default for VenuePromotionPolicy {
    fn default() -> Self {
        Self {
            // Thirty acknowledgements is the smallest sample where a latency
            // figure has a shape rather than a value, and it is deliberately
            // not the ten that `VENUE_WITHDRAWAL_MIN_SAMPLE` uses: that one
            // counts refusals, where ten is a pattern, and this one counts
            // acknowledgements, where ten is one quiet minute.
            minimum_observations: 30,
            // A basis point of slack, because a fee schedule quoted in whole
            // basis points and a fee charged on a rounded notional will
            // disagree in the last place without either being wrong. Two
            // would admit a venue charging double a one-basis-point fee.
            fee_tolerance_bps: Decimal::ONE,
            // Ten milliseconds. A venue that acknowledges ten milliseconds
            // slower than it says is a venue whose documentation is stale;
            // one that acknowledges a hundred slower is a different venue
            // from the one the desk modelled.
            latency_tolerance: Duration::from_millis(10),
            // Five sessions, so a replay covers more than one day's
            // idiosyncrasy. Reconciliation over a single session proves the
            // adapter parsed one file.
            minimum_replayed_sessions: 5,
            // A hundred basis points. One per cent of a clip in slippage and
            // forfeited headroom together is the point past which a venue is
            // an expense rather than an execution channel, and the bound is
            // stated here rather than per-venue so that a venue cannot be
            // admitted by relaxing the number it failed against.
            maximum_crossing_cost_bps: Decimal::from_raw(100 * qip_core::decimal::SCALE),
        }
    }
}

/// A rung's admission test for a venue.
///
/// Returns a [`GateOutcome`] rather than a `Result` for the same reason the
/// strategy [`crate::gates::Gate`] does: evidence that was not submitted and
/// evidence that falls short mean the same thing to a caller, which is that
/// the promotion does not happen, and an error would tempt one of them to be
/// retried.
pub trait VenueGate {
    fn stage(&self) -> GateStage;

    fn evaluate(
        &self,
        declaration: &VenueDeclaration,
        evidence: &VenueEvidence,
        now: Timestamp,
    ) -> GateOutcome;
}

// §34.4's "Registered" rung has no gate type here, and that is deliberate.
//
// The rung's requirement is "adapter provides everything in 34.1, venue type
// identified", which is exactly what `VenueDeclaration::new` refuses a
// venue for lacking: a class, a non-negative fee, a non-negative latency and
// at least one order type. A venue with no declaration is not standing at
// the bottom of this ladder — it is not on it. So the first rung is held by
// a constructor rather than by a `RegisteredGate` struct, and there is no
// gate object that always passes.
//
// A first cut of this module did ship that struct. It could never run:
// [`VenueLadder::stage_of`] returns [`GateStage::Candidate`] for a venue
// nobody has promoted, [`attempt_promotion`] computes the rung *above* where
// a venue stands, and so the rung a `RegisteredGate` admitted to was one
// every venue already occupied. It was a gate with no input that could make
// it fire, which is the precise defect this repository names as the template
// for what not to ship, and it is recorded here rather than silently deleted
// because the same mistake is available to the next rung somebody adds.

/// §34.4's "Observed" rung: connected read-only, and every declaration
/// checked against what was actually seen.
///
/// The rung the blueprint's closing sentence is about — "a venue that
/// misdescribes itself is a venue that will surprise the platform with real
/// capital at risk". Three declarations, three comparisons, and each fails
/// only in the direction that costs the desk money or time: a venue charging
/// *less* than it said and answering *faster* than it said are both admitted,
/// because neither is a surprise the platform has to survive.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ObservedGate {
    pub policy: VenuePromotionPolicy,
}

impl ObservedGate {
    pub fn new(policy: VenuePromotionPolicy) -> Self {
        Self { policy }
    }
}

impl VenueGate for ObservedGate {
    fn stage(&self) -> GateStage {
        GateStage::Holdout
    }

    fn evaluate(
        &self,
        declaration: &VenueDeclaration,
        evidence: &VenueEvidence,
        now: Timestamp,
    ) -> GateOutcome {
        let outcome = GateOutcome::new(GateStage::Holdout, now);
        let Some(measured) = evidence.measurement() else {
            return outcome.record(
                CHECK_MEASUREMENT_PRESENT,
                false,
                format!(
                    "{} has not been connected read-only; nothing has been measured against its \
                     declarations, and a venue is never enabled from its own documentation alone",
                    declaration.venue()
                ),
            );
        };
        let outcome = outcome.record(
            CHECK_MEASUREMENT_PRESENT,
            true,
            format!("{} observations", measured.observations),
        );
        let outcome = outcome.record(
            CHECK_SAMPLE_SUFFICIENT,
            measured.observations >= self.policy.minimum_observations,
            format!(
                "{} acknowledgement(s) measured against a minimum of {}",
                measured.observations, self.policy.minimum_observations
            ),
        );

        // Understatement only. A fee lower than declared is a pleasant
        // surprise and a latency faster than declared is a faster venue;
        // neither is what §34.4 guards against.
        //
        // An *unmeasured* fee or latency is a third case and it fails, which
        // is the whole reason `VenueMeasurement` carries options here. Read as
        // a figure, a missing latency is zero, zero is faster than anything a
        // venue could declare, and a venue nobody has ever timed would clear
        // the latency check on every cycle for ever. That is not a gate with
        // a gap in it; it is a gate whose easiest subjects are the venues
        // nothing is known about.
        let fee_ceiling = declaration
            .fee_bps()
            .checked_add(self.policy.fee_tolerance_bps);
        let outcome = match (measured.fee_bps, fee_ceiling) {
            (Some(measured_fee), Some(ceiling)) => outcome.record(
                CHECK_FEES_NOT_UNDERSTATED,
                measured_fee <= ceiling,
                format!(
                    "declared {} basis points, measured {}, tolerated up to {}",
                    declaration.fee_bps(),
                    measured_fee,
                    ceiling
                ),
            ),
            (None, _) => outcome.record(
                CHECK_FEES_NOT_UNDERSTATED,
                false,
                format!(
                    "{} declares {} basis points and nothing has measured what it charged; the                      observed rung compares a declaration against measurement and there is no                      measurement, which is not the same as a fee of zero",
                    declaration.venue(),
                    declaration.fee_bps()
                ),
            ),
            (Some(_), None) => outcome.record(
                CHECK_FEES_NOT_UNDERSTATED,
                false,
                format!(
                    "a declared fee of {} basis points plus the tolerance is not representable",
                    declaration.fee_bps()
                ),
            ),
        };

        let latency_ceiling = declaration
            .latency()
            .as_nanos()
            .checked_add(self.policy.latency_tolerance.as_nanos());
        let outcome = match (measured.latency, latency_ceiling) {
            (Some(measured_latency), Some(ceiling)) => outcome.record(
                CHECK_LATENCY_NOT_UNDERSTATED,
                measured_latency.as_nanos() <= ceiling,
                format!(
                    "declared {} ns, measured {} ns, tolerated up to {} ns",
                    declaration.latency().as_nanos(),
                    measured_latency.as_nanos(),
                    ceiling
                ),
            ),
            (None, _) => outcome.record(
                CHECK_LATENCY_NOT_UNDERSTATED,
                false,
                format!(
                    "{} declares {} ns and does not time its acknowledgements; an unmeasured                      venue is not a fast venue, so the observed rung is refused rather than                      cleared on a figure nobody took",
                    declaration.venue(),
                    declaration.latency().as_nanos()
                ),
            ),
            (Some(_), None) => outcome.record(
                CHECK_LATENCY_NOT_UNDERSTATED,
                false,
                format!(
                    "a declared latency of {} ns plus the tolerance is not representable",
                    declaration.latency().as_nanos()
                ),
            ),
        };

        // A declared type the venue rejected is the clearest case of a venue
        // misdescribing itself; a declared type nobody tried is the quieter
        // one, and both fail, because the rung's promise is that support was
        // *verified* and not merely unrefuted.
        let rejected: Vec<&str> = declaration
            .order_types()
            .iter()
            .filter(|kind| measured.rejected_order_types.contains(*kind))
            .map(String::as_str)
            .collect();
        let untested: Vec<&str> = declaration
            .order_types()
            .iter()
            .filter(|kind| {
                !measured.accepted_order_types.contains(*kind)
                    && !measured.rejected_order_types.contains(*kind)
            })
            .map(String::as_str)
            .collect();
        let detail = if rejected.is_empty() && untested.is_empty() {
            format!(
                "every one of {} declared order type(s) was sent and accepted",
                declaration.order_types().len()
            )
        } else {
            format!(
                "declared but rejected: [{}]; declared but never tried: [{}]",
                rejected.join(", "),
                untested.join(", ")
            )
        };
        outcome.record(
            CHECK_ORDER_TYPES_VERIFIED,
            rejected.is_empty() && untested.is_empty(),
            detail,
        )
    }
}

/// §34.4's "Simulated" rung: traded in sim against recorded and replayed
/// data, reconciliation verified — and, for a decentralised venue, the §34.3
/// model complete and the cost of crossing inside the policy's bound.
///
/// The second half is where §34.3 and §34.4 meet, and it is the one place an
/// MEV estimate changes an outcome rather than decorating a report. A
/// decentralised venue reaches the simulator only if its pool math, its
/// block-time execution mode, its slippage tolerance and its contract-risk
/// flag all exist, and only if slippage plus adversarial headroom at the
/// reference clip comes to no more than
/// [`VenuePromotionPolicy::maximum_crossing_cost_bps`]. A venue missing any
/// piece stays observe-only, which is §34.3's own fallback rather than a
/// rule invented here.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SimulatedGate {
    pub policy: VenuePromotionPolicy,
}

impl SimulatedGate {
    pub fn new(policy: VenuePromotionPolicy) -> Self {
        Self { policy }
    }
}

impl VenueGate for SimulatedGate {
    fn stage(&self) -> GateStage {
        GateStage::Paper
    }

    fn evaluate(
        &self,
        declaration: &VenueDeclaration,
        evidence: &VenueEvidence,
        now: Timestamp,
    ) -> GateOutcome {
        let outcome = GateOutcome::new(GateStage::Paper, now);
        let Some(simulation) = evidence.simulation() else {
            return outcome.record(
                CHECK_SIMULATION_PRESENT,
                false,
                format!(
                    "{} has not been traded in the simulator against replayed data",
                    declaration.venue()
                ),
            );
        };
        let outcome = outcome
            .record(
                CHECK_SIMULATION_PRESENT,
                true,
                format!("{} session(s) replayed", simulation.replayed_sessions),
            )
            .record(
                CHECK_SESSIONS_REPLAYED,
                simulation.replayed_sessions >= self.policy.minimum_replayed_sessions,
                format!(
                    "{} session(s) replayed against a minimum of {}",
                    simulation.replayed_sessions, self.policy.minimum_replayed_sessions
                ),
            )
            .record(
                CHECK_RECONCILIATION_VERIFIED,
                simulation.reconciliation_breaks == 0,
                format!(
                    "{} reconciliation break(s) across the replay; verified means none",
                    simulation.reconciliation_breaks
                ),
            );

        if declaration.class() != VenueClass::DecentralisedExchange {
            return outcome;
        }
        let Some(dex) = evidence.dex_model() else {
            return outcome.record(
                CHECK_DEX_MODEL_COMPLETE,
                false,
                format!(
                    "{} is a decentralised venue with no §34.3 model at all; it stays \
                     observe-only until its pool math, block-time execution, MEV estimate and \
                     contract-risk flag exist",
                    declaration.venue()
                ),
            );
        };
        let missing = dex.missing();
        let outcome = outcome.record(
            CHECK_DEX_MODEL_COMPLETE,
            missing.is_empty(),
            if missing.is_empty() {
                "pool math, block-time execution, MEV estimate and contract-risk flag all present"
                    .to_string()
            } else {
                format!("still owed: {}", missing.join(", "))
            },
        );
        match dex.quote(simulation.reference_clip) {
            Ok(quote) => match quote.total_cost_bps() {
                Ok(cost) => outcome.record(
                    CHECK_CROSSING_COST_WITHIN_BOUND,
                    cost <= self.policy.maximum_crossing_cost_bps,
                    format!(
                        "crossing {} costs {cost} basis points ({} slippage plus {} of \
                         extractable headroom) against a bound of {}",
                        simulation.reference_clip,
                        quote.pool.slippage_bps,
                        quote.mev.extractable_bps,
                        self.policy.maximum_crossing_cost_bps
                    ),
                ),
                Err(error) => outcome.record(
                    CHECK_CROSSING_COST_WITHIN_BOUND,
                    false,
                    error.message().to_string(),
                ),
            },
            Err(error) => outcome.record(
                CHECK_CROSSING_COST_WITHIN_BOUND,
                false,
                format!(
                    "the venue could not price a {} clip: {}",
                    simulation.reference_clip,
                    error.message()
                ),
            ),
        }
    }
}

/// Where every venue stands, and how it got there.
///
/// The stage map has no public constructor taking stages, so a venue's only
/// route above [`GateStage::Candidate`] is [`attempt_promotion`], and that
/// function's ceiling is the only ceiling there is. A ledger cannot be built
/// with a venue already at a rung that holds capital.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VenueLadder {
    stages: BTreeMap<String, GateStage>,
    history: Vec<VenueMove>,
}

/// One recorded move, the venue it was about, and the gate findings behind
/// it.
///
/// [`Promotion`] is the strategy ladder's own record type, reused rather than
/// re-declared: the fields a move needs — from, to, when, who approved,
/// why, and the evidence — do not become different fields because the subject
/// is a venue.
#[derive(Clone, Debug, PartialEq)]
pub struct VenueMove {
    pub venue: String,
    pub promotion: Promotion,
}

impl VenueLadder {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many venues the ladder holds a rung for.
    ///
    /// Exists so a reader can tell an *unconfigured* ladder from a configured
    /// one with a gap in it, and those deserve different treatment: an empty
    /// ladder is this platform's state today — nothing declares a venue to it —
    /// while a ladder holding four venues and not the fifth is somebody's
    /// oversight. Reporting both as the same finding put a problem on every
    /// cycle of every deployment, which is how an operator learns that problems
    /// are noise.
    pub fn len(&self) -> usize {
        self.stages.len()
    }

    /// Whether the ladder holds no venue at all. See [`Self::len`].
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Where a venue stands. A venue nobody has promoted is registered, which
    /// is the bottom rung and not an error: §34.4's first rung is the one an
    /// adapter arrives at.
    pub fn stage_of(&self, venue: &str) -> GateStage {
        self.stages
            .get(venue)
            .copied()
            .unwrap_or(GateStage::Candidate)
    }

    /// Whether the **simulator** may use this venue.
    ///
    /// The one question this type answers about what a venue may be used for,
    /// and it is deliberately the only one: there is no `admits_capital`, no
    /// `is_live`, and no method whose true value would let an order reach
    /// real money. Equality with the ceiling rather than an ordering, so that
    /// a rung above the ceiling — which [`attempt_promotion`] cannot produce
    /// — would read as *not* admitted rather than as more than admitted.
    pub fn admits(&self, venue: &str) -> bool {
        self.stage_of(venue) == VENUE_PROMOTION_CEILING
    }

    /// Whether this ladder holds any record of `venue` at all.
    ///
    /// Distinct from [`Self::stage_of`], which answers `Candidate` for a
    /// venue nobody has promoted *and* for a venue nobody has ever declared.
    /// The two are the same rung and a different fact: the first is a venue
    /// working its way up, the second is a venue that was never registered,
    /// and a reader that cannot tell them apart would report a deployment's
    /// entire venue list as "standing at the bottom rung" on the day the
    /// ladder was introduced.
    pub fn knows(&self, venue: &str) -> bool {
        self.stages.contains_key(venue)
    }

    /// Every venue the simulator may use, in a deterministic order.
    pub fn admitted(&self) -> BTreeSet<String> {
        self.stages
            .iter()
            .filter(|(_, stage)| **stage == VENUE_PROMOTION_CEILING)
            .map(|(venue, _)| venue.clone())
            .collect()
    }

    pub fn history(&self) -> &[VenueMove] {
        &self.history
    }

    /// Put a declared venue on the ladder at §34.4's first rung.
    ///
    /// The Registered rung is earned by having a declaration at all — a
    /// class, a non-negative fee, a non-negative latency and at least one
    /// order type — which is exactly what [`VenueDeclaration::new`] refuses a
    /// venue for lacking. There is no `RegisteredGate` and this is not one:
    /// it admits nothing, it moves no venue above [`GateStage::Candidate`],
    /// and a venue it seats still faces every gate above.
    ///
    /// It exists because [`Self::knows`] was documented to tell a venue
    /// working its way up from a venue nobody registered, and until this
    /// method there was no way to make the first true without also making the
    /// venue clear a measured rung. A reviewer reading "no record of it"
    /// could not tell a venue whose adapter had never been wired from one
    /// whose evidence fell short.
    ///
    /// Idempotent, and that is load-bearing rather than tidy: this is called
    /// once per cycle from the LEARN stage, and a version that pushed a
    /// history entry each time would grow the record without bound for a
    /// venue that never moved. Returns whether the venue was newly seated.
    pub fn register(&mut self, declaration: &VenueDeclaration, now: Timestamp) -> bool {
        let venue = declaration.venue().as_str().to_string();
        if self.stages.contains_key(&venue) {
            return false;
        }
        self.stages.insert(venue.clone(), GateStage::Candidate);
        self.history.push(VenueMove {
            venue,
            promotion: Promotion {
                from: GateStage::Candidate,
                to: GateStage::Candidate,
                at: now,
                approver: None,
                rationale: format!(
                    "{} declares a class, a fee of {} basis points, a latency of {} ns and {}                      order type(s); §34.4's registered rung is having a declaration to check",
                    declaration.venue(),
                    declaration.fee_bps(),
                    declaration.latency().as_nanos(),
                    declaration.order_types().len()
                ),
                evidence: Vec::new(),
            },
        });
        true
    }

    /// Push a venue back down, with no approver and no evidence.
    ///
    /// The same asymmetry the strategy ladder rests on: a false demotion
    /// costs a day of an unused venue, a missed one costs whatever the venue
    /// does next. `to` is refused if it is not below where the venue stands,
    /// because a "demotion" that raised a rung would be a promotion wearing
    /// the word that needs no authority.
    pub fn demote(
        &mut self,
        venue: &str,
        to: GateStage,
        rationale: impl Into<String>,
        now: Timestamp,
    ) -> Result<()> {
        let from = self.stage_of(venue);
        if to >= from && to != GateStage::Retired {
            return Err(Error::invalid(format!(
                "{venue} stands at {} and a demotion to {} is not a demotion; promotion is the \
                 path upward and it needs evidence",
                rung_name(from),
                rung_name(to)
            )));
        }
        self.stages.insert(venue.to_string(), to);
        self.history.push(VenueMove {
            venue: venue.to_string(),
            promotion: Promotion {
                from,
                to,
                at: now,
                approver: None,
                rationale: rationale.into(),
                evidence: Vec::new(),
            },
        });
        Ok(())
    }
}

/// Run the gate for the rung above where `venue` stands, and move it if the
/// gate passed.
///
/// # The refusal that matters
///
/// The target rung is *computed* from [`GateStage::next`] and refused if it
/// is above [`VENUE_PROMOTION_CEILING`]. There is no parameter naming a
/// target, so a caller cannot ask for one; and the refusal names why rather
/// than reporting a bare failure, because the reason is not a limitation of
/// this module: there is no live-class adapter in this workspace, the
/// platform's autonomy ceiling is paper trading at three layers, and a
/// promotion ladder that could reach past the simulator would be a fourth
/// way in that none of the three watches.
pub fn attempt_promotion(
    ladder: &mut VenueLadder,
    declaration: &VenueDeclaration,
    evidence: &VenueEvidence,
    policy: VenuePromotionPolicy,
    approver: Option<String>,
    rationale: impl Into<String>,
    now: Timestamp,
) -> Result<Promotion> {
    let venue = declaration.venue().as_str().to_string();
    let from = ladder.stage_of(&venue);
    let Some(to) = from.next() else {
        return Err(Error::denied(format!(
            "{venue} stands at {} and there is no rung above it",
            rung_name(from)
        )));
    };
    if to > VENUE_PROMOTION_CEILING {
        return Err(Error::denied(format!(
            "{venue} stands at {} and the rung above it is {}, which this platform does not \
             have: there is no live-class venue adapter in this workspace and the autonomy \
             ceiling is paper trading (ADR 0003). A venue is promoted into the simulator and \
             no further, so {} is the last rung and reaching it is the whole of what this \
             ladder can do",
            rung_name(from),
            rung_name(to),
            rung_name(VENUE_PROMOTION_CEILING)
        )));
    }

    let outcome = match to {
        GateStage::Holdout => ObservedGate::new(policy).evaluate(declaration, evidence, now),
        GateStage::Paper => SimulatedGate::new(policy).evaluate(declaration, evidence, now),
        // Unreachable while the ceiling is `Paper`, and written as a refusal
        // rather than a `match` arm that cannot happen: if the ceiling ever
        // moves, this is the line that stops a rung being promoted through a
        // gate nobody wrote.
        other => {
            return Err(Error::denied(format!(
                "{venue} would be promoted to {} and no gate admits to that rung",
                rung_name(other)
            )));
        }
    };
    if !outcome.passed {
        let failures: Vec<String> = outcome
            .failures()
            .into_iter()
            .map(|(name, _, detail)| format!("{name}: {detail}"))
            .collect();
        return Err(Error::denied(format!(
            "{venue} does not clear the {} rung: {}",
            rung_name(to),
            failures.join("; ")
        )));
    }

    let promotion = Promotion {
        from,
        to,
        at: now,
        approver,
        rationale: rationale.into(),
        evidence: outcome
            .findings
            .iter()
            .map(|(name, passed, detail)| format!("{name}={passed}: {detail}"))
            .collect(),
    };
    ladder.stages.insert(venue.clone(), to);
    ladder.history.push(VenueMove {
        venue,
        promotion: promotion.clone(),
    });
    Ok(promotion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_financial::pool::{BlockExecution, ContractRisk, PoolCurve, PoolState};

    fn now() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    fn order_types() -> BTreeSet<String> {
        ["limit".to_string(), "market".to_string()]
            .into_iter()
            .collect()
    }

    fn declaration(class: VenueClass) -> VenueDeclaration {
        VenueDeclaration::new(
            VenueId::new("XPOOL"),
            class,
            dec!("10"),
            Duration::from_millis(50),
            order_types(),
        )
        .expect("a valid declaration")
    }

    /// A measurement that agrees with [`declaration`] on every axis.
    fn honest_measurement() -> VenueMeasurement {
        VenueMeasurement {
            fee_bps: Some(dec!("10")),
            latency: Some(Duration::from_millis(50)),
            accepted_order_types: order_types(),
            rejected_order_types: BTreeSet::new(),
            observations: 40,
        }
    }

    fn clean_simulation() -> SimulationEvidence {
        SimulationEvidence {
            replayed_sessions: 10,
            reconciliation_breaks: 0,
            reference_clip: dec!("10"),
        }
    }

    /// A complete §34.3 model whose crossing cost at a ten-unit clip is well
    /// inside the default bound.
    fn cheap_dex_model() -> DexModel {
        let pool = PoolState::new(
            PoolCurve::ConstantProduct,
            dec!("1000000"),
            dec!("1000000"),
            dec!("5"),
        )
        .expect("a valid pool");
        DexModel::new()
            .with_pool(pool)
            .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a mode"))
            .with_tolerance_bps(dec!("20"))
            .expect("a tolerance")
            .with_contract(ContractRisk::new("0xpool", Some(now()), false).expect("a flag"))
    }

    fn walk_to_observed(ladder: &mut VenueLadder, declaration: &VenueDeclaration) {
        attempt_promotion(
            ladder,
            declaration,
            &VenueEvidence::new().with_measurement(honest_measurement()),
            VenuePromotionPolicy::default(),
            None,
            "measured read-only",
            now(),
        )
        .expect("an honest venue clears the observed rung");
    }

    #[test]
    fn a_venue_that_measures_as_it_declares_walks_registered_to_observed_to_simulated() {
        // The premise every refusal test below depends on: this ladder can be
        // walked. A gate no venue can cross is indistinguishable, from its
        // refusals alone, from one that works.
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut ladder = VenueLadder::new();
        assert_eq!(ladder.stage_of("XPOOL"), GateStage::Candidate);
        assert!(!ladder.admits("XPOOL"));
        walk_to_observed(&mut ladder, &declaration);
        assert_eq!(ladder.stage_of("XPOOL"), GateStage::Holdout);
        assert!(
            !ladder.admits("XPOOL"),
            "an observed venue was admitted to the simulator without ever being traded in it"
        );
        // And the set, not only the predicate. A code-review finding: a
        // mutation that loosened `admitted`'s filter from "at the ceiling" to
        // "at or above the observed rung" passed every assertion in this
        // module, because nothing asserted the set was *empty* at a rung
        // below the ceiling. A caller installing `admitted()` as a whitelist
        // would then have been given a venue nobody had ever replayed.
        assert!(
            ladder.admitted().is_empty(),
            "the admitted set carried a venue standing below the simulator rung: {:?}",
            ladder.admitted()
        );
        let promotion = attempt_promotion(
            &mut ladder,
            &declaration,
            &VenueEvidence::new()
                .with_measurement(honest_measurement())
                .with_simulation(clean_simulation()),
            VenuePromotionPolicy::default(),
            None,
            "replayed and reconciled",
            now(),
        )
        .expect("a reconciled venue clears the simulated rung");
        assert_eq!(promotion.to, GateStage::Paper);
        assert!(ladder.admits("XPOOL"));
        assert_eq!(
            ladder.admitted(),
            ["XPOOL".to_string()].into_iter().collect()
        );
        // Two moves, not three: the registered rung is where a declared
        // venue starts, held by `VenueDeclaration::new` rather than by a gate
        // that would always pass.
        assert_eq!(ladder.history().len(), 2);
    }

    #[test]
    fn the_ladder_refuses_every_rung_above_the_simulator_and_names_why() {
        // The most dangerous thing this module could do is enable a venue for
        // real execution. It cannot: the target is computed, the ceiling is a
        // constant, and there is no argument that names a rung.
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut ladder = VenueLadder::new();
        walk_to_observed(&mut ladder, &declaration);
        attempt_promotion(
            &mut ladder,
            &declaration,
            &VenueEvidence::new()
                .with_measurement(honest_measurement())
                .with_simulation(clean_simulation()),
            VenuePromotionPolicy::default(),
            None,
            "replayed and reconciled",
            now(),
        )
        .expect("the premise: it reaches the ceiling");
        assert_eq!(ladder.stage_of("XPOOL"), VENUE_PROMOTION_CEILING);

        let refused = attempt_promotion(
            &mut ladder,
            &declaration,
            &VenueEvidence::new()
                .with_measurement(honest_measurement())
                .with_simulation(clean_simulation()),
            VenuePromotionPolicy::default(),
            Some("an operator".to_string()),
            "every gate passed and an operator signed",
            now(),
        );
        assert!(
            refused.is_err(),
            "a venue was promoted past the simulator on passing evidence and a signature"
        );
        let message = refused
            .err()
            .map(|error| error.message().to_string())
            .unwrap_or_default();
        // Delimited-token matching: "shadow" is a substring of nothing here,
        // but the rung names are checked as whole words against the mapping
        // rather than by eye.
        assert!(
            message.contains(rung_name(GateStage::Shadow)),
            "the refusal did not name the rung it refused: {message}"
        );
        assert!(message.contains("ADR 0003"), "{message}");
        assert_eq!(
            ladder.stage_of("XPOOL"),
            VENUE_PROMOTION_CEILING,
            "the refused promotion moved the venue anyway"
        );
    }

    #[test]
    fn the_ceiling_rung_is_one_that_holds_no_capital_and_reaches_no_venue() {
        // Stated as an assertion rather than left to the reader of the
        // constant: if `VENUE_PROMOTION_CEILING` were ever edited to a rung
        // `GateStage::holds_capital` calls true, this fails before anything
        // else does.
        assert!(!VENUE_PROMOTION_CEILING.holds_capital());
        assert!(!VENUE_PROMOTION_CEILING.may_reach_a_venue());
        assert!(!VENUE_PROMOTION_CEILING.requires_human_approval());
        assert_eq!(rung_name(VENUE_PROMOTION_CEILING), "simulated");
    }

    #[test]
    fn a_venue_that_charges_more_than_it_declares_is_refused_at_the_observed_rung() {
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut measured = honest_measurement();
        // Eleven and a half against a declared ten, tolerating one.
        measured.fee_bps = Some(dec!("11.5"));
        let outcome = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(measured.clone()),
            now(),
        );
        assert!(!outcome.passed);
        let failed: Vec<&str> = outcome
            .failures()
            .into_iter()
            .map(|(name, _, _)| name.as_str())
            .collect();
        assert_eq!(failed, vec![CHECK_FEES_NOT_UNDERSTATED]);

        // The other direction is admitted: a venue charging less than it said
        // is not a surprise the platform has to survive.
        measured.fee_bps = Some(dec!("1"));
        let generous = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(measured),
            now(),
        );
        assert!(generous.passed, "a cheaper-than-declared venue was refused");
    }

    #[test]
    fn a_venue_that_does_not_time_its_acknowledgements_is_refused_rather_than_read_as_instant() {
        // The refusal this whole seam exists for. A venue whose adapter
        // reports no acknowledgement latency must not clear the latency
        // check: read as a figure, an absent latency is zero, zero beats
        // every declaration there is, and the venues nothing is known about
        // would be the easiest ones to promote.
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut unmeasured = honest_measurement();
        unmeasured.latency = None;
        let outcome = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(unmeasured),
            now(),
        );
        assert!(
            !outcome.passed,
            "an untimed venue cleared the observed rung"
        );
        let failed: Vec<&str> = outcome
            .failures()
            .into_iter()
            .map(|(name, _, _)| name.as_str())
            .collect();
        assert_eq!(
            failed,
            vec![CHECK_LATENCY_NOT_UNDERSTATED],
            "the refusal came from a check other than the latency one"
        );
        // Asserted on the reason and not merely on the failure, because the
        // one wrong implementation this test guards against — treating a
        // missing latency as zero — would still refuse a venue that was also
        // short on its sample, and a bare `!passed` would have passed it.
        let detail = outcome
            .failures()
            .into_iter()
            .find(|(name, _, _)| name == CHECK_LATENCY_NOT_UNDERSTATED)
            .map(|(_, _, detail)| detail.clone())
            .unwrap_or_default();
        assert!(
            detail.contains("does not time its acknowledgements"),
            "the refusal does not name the missing measurement: {detail}"
        );

        // The premise, and the distinction the option carries: a venue that
        // *did* time its acknowledgements and found them instant is admitted.
        // `None` and `Some(ZERO)` are two different claims and only one of
        // them is evidence.
        let mut instant = honest_measurement();
        instant.latency = Some(Duration::ZERO);
        assert!(
            ObservedGate::default()
                .evaluate(
                    &declaration,
                    &VenueEvidence::new().with_measurement(instant),
                    now()
                )
                .passed,
            "a venue measured at zero was refused, so the refusal above is not about measurement"
        );
    }

    #[test]
    fn a_venue_that_itemises_no_fee_is_refused_rather_than_read_as_free() {
        // The same failure on the money axis: an absent fee read as zero
        // makes an unmeasured venue the cheapest venue there is, which is
        // exactly the direction §34.4 guards.
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut unmeasured = honest_measurement();
        unmeasured.fee_bps = None;
        let outcome = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(unmeasured),
            now(),
        );
        assert!(
            !outcome.passed,
            "an unbilled venue cleared the observed rung"
        );
        let failed: Vec<&str> = outcome
            .failures()
            .into_iter()
            .map(|(name, _, _)| name.as_str())
            .collect();
        assert_eq!(failed, vec![CHECK_FEES_NOT_UNDERSTATED]);
        let detail = outcome
            .failures()
            .into_iter()
            .find(|(name, _, _)| name == CHECK_FEES_NOT_UNDERSTATED)
            .map(|(_, _, detail)| detail.clone())
            .unwrap_or_default();
        assert!(
            detail.contains("nothing has measured what it charged"),
            "the refusal does not name the missing measurement: {detail}"
        );

        // And the premise: a venue measured at zero basis points clears it.
        let mut free = honest_measurement();
        free.fee_bps = Some(Decimal::ZERO);
        assert!(
            ObservedGate::default()
                .evaluate(
                    &declaration,
                    &VenueEvidence::new().with_measurement(free),
                    now()
                )
                .passed
        );
    }

    #[test]
    fn a_venue_the_ladder_has_registered_cannot_be_promoted_past_the_simulator_however_measured() {
        // The ceiling, exercised from the seat a production caller leaves a
        // venue in rather than from a bare ladder: register, walk it to the
        // ceiling on perfect evidence, and then ask for one more rung.
        let mut ladder = VenueLadder::new();
        let declaration = declaration(VenueClass::CryptoExchange);
        assert!(
            ladder.register(&declaration, now()),
            "the premise: registration seats the venue"
        );
        assert!(ladder.knows(declaration.venue().as_str()));
        assert_eq!(
            ladder.stage_of(declaration.venue().as_str()),
            GateStage::Candidate,
            "registration moved a venue above the first rung"
        );
        assert!(
            !ladder.register(&declaration, now()),
            "registration is not idempotent, so the history grows for a venue that never moved"
        );
        assert_eq!(ladder.history().len(), 1);

        let evidence = VenueEvidence::new()
            .with_measurement(honest_measurement())
            .with_simulation(clean_simulation());
        for _ in 0..2 {
            attempt_promotion(
                &mut ladder,
                &declaration,
                &evidence,
                VenuePromotionPolicy::default(),
                None,
                "measured, replayed and reconciled",
                now(),
            )
            .expect("an honest venue walks from registered to the simulator");
        }
        assert_eq!(
            ladder.stage_of(declaration.venue().as_str()),
            VENUE_PROMOTION_CEILING
        );

        // The rung above, asked for with an approver and on evidence that
        // cleared every gate. There is no input that makes this succeed, and
        // the refusal is asserted by its reason rather than by `is_err`:
        // with the ceiling check deleted this still fails, because no gate
        // admits to the shadow rung, and a bare `is_err` would pass a build
        // whose ceiling had been removed.
        let message = attempt_promotion(
            &mut ladder,
            &declaration,
            &evidence,
            VenuePromotionPolicy::default(),
            Some("an operator".to_string()),
            "an operator asked for it",
            now(),
        )
        .err()
        .map(|error| error.message().to_string())
        .unwrap_or_default();
        assert!(
            message.contains("ADR 0003"),
            "refused by something other than the paper-trading ceiling: {message}"
        );
        assert_eq!(
            ladder.stage_of(declaration.venue().as_str()),
            VENUE_PROMOTION_CEILING
        );
    }

    #[test]
    fn a_venue_slower_than_it_declares_is_refused_and_a_faster_one_is_admitted() {
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut measured = honest_measurement();
        // Declared 50ms, tolerating 10, measured 61.
        measured.latency = Some(Duration::from_millis(61));
        let outcome = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(measured.clone()),
            now(),
        );
        assert!(!outcome.passed);
        let failed: Vec<&str> = outcome
            .failures()
            .into_iter()
            .map(|(name, _, _)| name.as_str())
            .collect();
        assert_eq!(failed, vec![CHECK_LATENCY_NOT_UNDERSTATED]);

        measured.latency = Some(Duration::from_millis(5));
        assert!(
            ObservedGate::default()
                .evaluate(
                    &declaration,
                    &VenueEvidence::new().with_measurement(measured),
                    now()
                )
                .passed
        );
    }

    #[test]
    fn an_order_type_the_venue_declares_and_rejects_is_refused_and_so_is_one_nobody_tried() {
        let declaration = declaration(VenueClass::CryptoExchange);

        let mut rejected = honest_measurement();
        rejected.accepted_order_types = ["limit".to_string()].into_iter().collect();
        rejected.rejected_order_types = ["market".to_string()].into_iter().collect();
        let outcome = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(rejected),
            now(),
        );
        assert!(!outcome.passed);
        assert_eq!(
            outcome
                .failures()
                .into_iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec![CHECK_ORDER_TYPES_VERIFIED]
        );

        // The quieter failure: a declared type nobody ever sent. The rung
        // promises support was verified, and unrefuted is not verified.
        let mut untested = honest_measurement();
        untested.accepted_order_types = ["limit".to_string()].into_iter().collect();
        let outcome = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(untested),
            now(),
        );
        assert!(
            !outcome.passed,
            "a declared order type nobody tried was treated as supported"
        );
    }

    #[test]
    fn a_venue_nobody_has_connected_to_cannot_leave_the_registered_rung() {
        // §34.4's closing sentence, as a refusal: a venue is never enabled
        // from its own documentation alone.
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut ladder = VenueLadder::new();
        // The premise: a declared venue stands at the registered rung already,
        // so the refusal below is the observed rung's and not an absence.
        assert_eq!(ladder.stage_of("XPOOL"), GateStage::Candidate);
        let refused = attempt_promotion(
            &mut ladder,
            &declaration,
            &VenueEvidence::new(),
            VenuePromotionPolicy::default(),
            None,
            "the venue's own documentation says the fees are ten basis points",
            now(),
        );
        assert!(refused.is_err());
        assert!(
            refused
                .err()
                .map(|error| error.message().contains(CHECK_MEASUREMENT_PRESENT))
                .unwrap_or(false)
        );
        assert_eq!(
            ladder.stage_of("XPOOL"),
            GateStage::Candidate,
            "a venue moved up on its own documentation"
        );
    }

    #[test]
    fn too_few_acknowledgements_refuse_the_observed_rung_however_well_they_agree() {
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut measured = honest_measurement();
        measured.observations = 29;
        let outcome = ObservedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_measurement(measured.clone()),
            now(),
        );
        assert!(!outcome.passed);
        assert_eq!(
            outcome
                .failures()
                .into_iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec![CHECK_SAMPLE_SUFFICIENT]
        );
        measured.observations = 30;
        assert!(
            ObservedGate::default()
                .evaluate(
                    &declaration,
                    &VenueEvidence::new().with_measurement(measured),
                    now()
                )
                .passed,
            "the minimum itself was refused, which would make the bar uncrossable"
        );
    }

    #[test]
    fn a_reconciliation_break_in_the_replay_refuses_the_simulated_rung() {
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut simulation = clean_simulation();
        simulation.reconciliation_breaks = 1;
        let outcome = SimulatedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new().with_simulation(simulation),
            now(),
        );
        assert!(!outcome.passed);
        assert_eq!(
            outcome
                .failures()
                .into_iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec![CHECK_RECONCILIATION_VERIFIED]
        );
        assert!(
            SimulatedGate::default()
                .evaluate(
                    &declaration,
                    &VenueEvidence::new().with_simulation(clean_simulation()),
                    now()
                )
                .passed
        );
    }

    #[test]
    fn a_decentralised_venue_with_no_contract_risk_flag_stays_observe_only() {
        // §34.3's fallback reached through §34.4's ladder: the model is
        // incomplete, so the venue cannot be simulated however clean its
        // replay was.
        let declaration = declaration(VenueClass::DecentralisedExchange);
        let incomplete = DexModel::new()
            .with_pool(
                PoolState::new(
                    PoolCurve::ConstantProduct,
                    dec!("1000000"),
                    dec!("1000000"),
                    dec!("5"),
                )
                .expect("a pool"),
            )
            .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a mode"))
            .with_tolerance_bps(dec!("20"))
            .expect("a tolerance");
        let outcome = SimulatedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new()
                .with_simulation(clean_simulation())
                .with_dex_model(incomplete),
            now(),
        );
        assert!(!outcome.passed);
        let failed: Vec<&str> = outcome
            .failures()
            .into_iter()
            .map(|(name, _, _)| name.as_str())
            .collect();
        assert!(failed.contains(&CHECK_DEX_MODEL_COMPLETE), "{failed:?}");

        // The premise, and the half that proves the gate is not simply shut
        // for decentralised venues: the same evidence with the flag present
        // passes.
        let complete = SimulatedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new()
                .with_simulation(clean_simulation())
                .with_dex_model(cheap_dex_model()),
            now(),
        );
        assert!(complete.passed, "{:?}", complete.failures());
    }

    #[test]
    fn a_decentralised_venue_with_no_model_at_all_is_refused_rather_than_treated_as_central() {
        // The dangerous default: a `None` model read as "nothing to check".
        let decentralised = declaration(VenueClass::DecentralisedExchange);
        let centralised = declaration(VenueClass::CryptoExchange);
        let outcome = SimulatedGate::default().evaluate(
            &decentralised,
            &VenueEvidence::new().with_simulation(clean_simulation()),
            now(),
        );
        assert!(!outcome.passed);
        // The same evidence at a central venue passes, so the refusal is
        // about the class and not about the evidence.
        let central = SimulatedGate::default().evaluate(
            &centralised,
            &VenueEvidence::new().with_simulation(clean_simulation()),
            now(),
        );
        assert!(central.passed);
    }

    #[test]
    fn a_wide_slippage_tolerance_alone_keeps_a_decentralised_venue_out_of_the_simulator() {
        // The MEV estimate deciding something. Two models identical in every
        // respect except the tolerance orders are submitted under: the pool,
        // the clip, the block time and the contract are the same, so the only
        // thing that moved is the headroom an adversary can take. If the
        // crossing-cost check read slippage alone, both would pass.
        let declaration = declaration(VenueClass::DecentralisedExchange);
        let tight = cheap_dex_model();
        let wide = DexModel::new()
            .with_pool(
                PoolState::new(
                    PoolCurve::ConstantProduct,
                    dec!("1000000"),
                    dec!("1000000"),
                    dec!("5"),
                )
                .expect("a pool"),
            )
            .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a mode"))
            .with_tolerance_bps(dec!("500"))
            .expect("a tolerance")
            .with_contract(ContractRisk::new("0xpool", Some(now()), false).expect("a flag"));

        let clip = clean_simulation().reference_clip;
        let tight_quote = tight.quote(clip).expect("a quote");
        let wide_quote = wide.quote(clip).expect("a quote");
        // The premise: the curve charges both trades the same, so the check
        // below cannot be passing on slippage.
        assert_eq!(tight_quote.pool.slippage_bps, wide_quote.pool.slippage_bps);
        assert!(wide_quote.mev.extractable_bps > tight_quote.mev.extractable_bps);

        let admitted = SimulatedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new()
                .with_simulation(clean_simulation())
                .with_dex_model(tight),
            now(),
        );
        assert!(admitted.passed, "{:?}", admitted.failures());

        let refused = SimulatedGate::default().evaluate(
            &declaration,
            &VenueEvidence::new()
                .with_simulation(clean_simulation())
                .with_dex_model(wide),
            now(),
        );
        assert!(
            !refused.passed,
            "a venue offering five hundred basis points of headroom to the mempool was admitted"
        );
        assert_eq!(
            refused
                .failures()
                .into_iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec![CHECK_CROSSING_COST_WITHIN_BOUND]
        );
    }

    #[test]
    fn a_demotion_needs_no_approver_and_a_demotion_that_would_raise_a_rung_is_refused() {
        let declaration = declaration(VenueClass::CryptoExchange);
        let mut ladder = VenueLadder::new();
        walk_to_observed(&mut ladder, &declaration);
        assert_eq!(ladder.stage_of("XPOOL"), GateStage::Holdout);
        ladder
            .demote(
                "XPOOL",
                GateStage::Candidate,
                "the venue's fee schedule changed under us",
                now(),
            )
            .expect("a demotion needs nobody");
        assert_eq!(ladder.stage_of("XPOOL"), GateStage::Candidate);
        assert!(
            ladder
                .history()
                .last()
                .map(|entry| entry.promotion.approver.is_none())
                .unwrap_or(false)
        );
        let refused = ladder.demote("XPOOL", GateStage::Paper, "promoting quietly", now());
        assert!(
            refused.is_err(),
            "a promotion was taken through the path that needs no authority"
        );
        assert_eq!(ladder.stage_of("XPOOL"), GateStage::Candidate);
    }

    #[test]
    fn a_declaration_naming_no_order_types_is_refused_before_it_can_pass_a_vacuous_check() {
        let refused = VenueDeclaration::new(
            VenueId::new("XPOOL"),
            VenueClass::CryptoExchange,
            dec!("10"),
            Duration::from_millis(50),
            BTreeSet::new(),
        );
        assert!(refused.is_err());
        // The premise: the same declaration with one order type is admitted,
        // so the constructor is not simply refusing everything.
        assert!(
            VenueDeclaration::new(
                VenueId::new("XPOOL"),
                VenueClass::CryptoExchange,
                dec!("10"),
                Duration::from_millis(50),
                ["limit".to_string()].into_iter().collect(),
            )
            .is_ok()
        );
    }
}
