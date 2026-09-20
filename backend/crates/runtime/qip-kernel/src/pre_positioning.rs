//! Blueprint §25.2's movement half, on the loop rather than beside it.
//!
//! The capital engine's table asks two questions this crate could answer and
//! answered for nobody: *how much to move* ("enough to restore target, never
//! more") and *where* ("cheapest signed corridor that arrives in time").
//! [`crate::platform::Platform::pre_position`] computed both, and
//! [`crate::platform::Platform::evaluate_pre_positioning`] scored the answer
//! against what the world then needed — and until this module the only
//! callers of either were tests. Every DECIDE forecast where capital would
//! have to be; nothing ever said how much to send there, and nothing ever
//! learned whether the forecast had been right.
//!
//! So DECIDE now plans, and LEARN now scores, and the two are joined by the
//! plan itself: one plan is retained until its horizon has elapsed, then
//! scored against the demand the platform actually recorded inside that
//! window, then released. That gate on the clock is the whole discipline. A
//! plan scored before its window closes is scored against a partial world in
//! which nothing has been needed *yet*, and reads as a plan that over-moved —
//! a forecaster judged that way learns to send nothing. A plan scored twice
//! learns from the same day twice. One plan per window, scored once, is what
//! makes the number a measurement.
//!
//! # What this deliberately does not do
//!
//! A plan's moves never become transfer intents, and nothing here reaches
//! the fabric's gate. That is not a gap left for later. A
//! [`qip_capital_fabric::journal::GateCommand`] carries §37.4's three
//! enforcement-point attestations, and those are facts a human establishes
//! out of band — a venue allowlist somebody configured, a corridor somebody
//! signed, a custody share somebody holds. A machine that manufactured them
//! from its own plan would be attesting to itself, which is the control
//! measuring itself. The plan is a record of what the engine would move and
//! why, with every refused lane and its figures beside it; under ADR 0021
//! that record is the permitted half, and it is the half worth having.
//!
//! # Money and statistics
//!
//! Amounts, costs and net value are money and stay [`Decimal`] from the plan
//! through the score into the journal. The coverage ratio, the bias and the
//! interval hit rate are statistics and are `f64` — a share of demand met is
//! not an amount of anything. The crossing happens once, inside
//! [`qip_capital_fabric::evaluate`], and nothing here crosses back.

use qip_capital_fabric::evaluate::{PlanScore, RealisedDemand};
use qip_capital_fabric::forecast::{DemandKind, DemandObservation};
use qip_capital_fabric::location::CapitalLocation;
use qip_capital_fabric::plan::PrePositioningPlan;
use qip_core::{Decimal, Duration, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The plan DECIDE retained for scoring, as the log carries it.
///
/// Filed under [`Topic::PortfolioProposed`]: a pre-positioning plan is a
/// proposal about where capital should sit, in the same sense a construction
/// is a proposal about what the book should hold, and like a construction it
/// commits nothing by being written down. Journaled once per window — the
/// plan the score will later be about — rather than on every cycle, because
/// DECIDE re-plans each cycle and the intermediate plans are superseded
/// before anything could act on them. The stage sentence carries those.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrePositioningPlanned {
    /// The cycle that built it.
    pub cycle: u64,
    /// How far ahead the plan reaches; the window LEARN will score it over.
    pub horizon: Duration,
    /// The plan itself, every lane and every refusal included.
    pub plan: PrePositioningPlan,
}

impl EventBody for PrePositioningPlanned {
    const TOPIC: Topic = Topic::PortfolioProposed;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("pre-positioning-plan:{}", self.plan.at.as_nanos()))
    }
}

/// What a retained plan turned out to be worth, as the log carries it.
///
/// Filed under [`Topic::OutcomeObserved`] because that is what it is: the
/// world has said what each lane needed, and the plan is measured against
/// it. Keyed on the instant the plan was built rather than the instant it
/// was scored, so a replay that scores the same plan twice collapses to one
/// record instead of two claims about one window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrePositioningScored {
    /// The cycle that scored it.
    pub cycle: u64,
    /// When the plan was built.
    pub planned_at: Timestamp,
    /// When it was scored — at or after `planned_at` plus the horizon.
    pub scored_at: Timestamp,
    /// How many transfers the plan proposed.
    pub moves_planned: usize,
    /// The score, every lane included.
    pub score: PlanScore,
}

impl EventBody for PrePositioningScored {
    const TOPIC: Topic = Topic::OutcomeObserved;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "pre-positioning-score:{}",
            self.planned_at.as_nanos()
        ))
    }
}

/// The headline of a score, as the cycle journal carries it beside LEARN's
/// other findings.
///
/// A summary rather than the whole [`PlanScore`], because the cycle entry is
/// the line an operator reads and the full score is already on the log under
/// its own topic. Every figure is copied from the score, none is recomputed,
/// so the two records cannot disagree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrePositioningJournal {
    pub planned_at: Timestamp,
    pub scored_at: Timestamp,
    pub moves_planned: usize,
    pub lanes_scored: usize,
    /// Capital the plan sent, in the base currency.
    pub positioned: Decimal,
    /// Realised demand the plan met.
    pub covered: Decimal,
    /// Realised demand that went unmet.
    pub shortfall: Decimal,
    /// Capital sent that turned out not to be needed.
    pub idle_surplus: Decimal,
    /// What moving it cost.
    pub transfer_cost: Decimal,
    /// Value against having moved nothing. Positive means the plan paid.
    pub net_value: Decimal,
    /// Share of realised demand met. A statistic.
    pub coverage_ratio_stat: f64,
    /// Mean signed forecast error; positive means demand was under-forecast.
    pub bias_stat: f64,
    /// Share of lanes whose realised demand fell inside its interval.
    pub interval_hit_rate_stat: f64,
}

impl PrePositioningJournal {
    /// The journal line for a score.
    pub fn of(scored: &PrePositioningScored) -> Self {
        let score = &scored.score;
        Self {
            planned_at: scored.planned_at,
            scored_at: scored.scored_at,
            moves_planned: scored.moves_planned,
            lanes_scored: score.lanes.len(),
            positioned: score.positioned,
            covered: score.covered,
            shortfall: score.shortfall,
            idle_surplus: score.idle_surplus,
            transfer_cost: score.transfer_cost,
            net_value: score.net_value,
            coverage_ratio_stat: score.coverage_ratio_stat,
            bias_stat: score.bias_stat,
            interval_hit_rate_stat: score.interval_hit_rate_stat,
        }
    }
}

/// The instant a plan built at `planned_at` over `horizon` may be scored.
///
/// Saturating, so a plan near the end of representable time is scored at
/// the sentinel rather than wrapping to a moment before it was built and
/// being scored at once against nothing.
pub fn window_closes_at(planned_at: Timestamp, horizon: Duration) -> Timestamp {
    planned_at.saturating_add(horizon)
}

/// What every lane actually needed between `from` and `to`, inclusive at
/// both ends, from the platform's own demand history.
///
/// Inclusive at `from` because DECIDE builds the plan before ACT books the
/// cycle's fills at the same instant, so a fill at exactly `from` is demand
/// that arose after the plan was made and inside the window it was meant to
/// cover. Inclusive at `to` for the mirror reason: the window's last instant
/// is inside the window. A lane the plan considered and the world never
/// touched is absent here, and [`RealisedDemand::amount`] reads absent as
/// zero — a real outcome, not a missing one.
///
/// Walks the history in key order and returns a [`RealisedDemand`] built on
/// a [`BTreeMap`], so the same history yields the same record on every
/// machine.
pub fn realised_between(
    history: &BTreeMap<(CapitalLocation, DemandKind), Vec<DemandObservation>>,
    from: Timestamp,
    to: Timestamp,
) -> RealisedDemand {
    let mut realised = RealisedDemand::new();
    for ((location, kind), observations) in history {
        let inside = observations
            .iter()
            .filter(|observation| observation.at >= from && observation.at <= to)
            .fold(Decimal::ZERO, |sum, observation| sum + observation.amount);
        if !inside.is_zero() {
            realised = realised.with(location.clone(), *kind, inside);
        }
    }
    realised
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_capital_fabric::location::Region;
    use qip_contracts::venue::VenueId;
    use qip_core::{Currency, dec};

    fn lane(venue: &str) -> CapitalLocation {
        CapitalLocation::new(Region::new("us-east1"), Currency::USD, VenueId::new(venue))
    }

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    #[test]
    fn realised_demand_counts_only_the_observations_inside_the_window_and_both_ends_of_it() {
        let mut history = BTreeMap::new();
        history.insert(
            (lane("XNYS"), DemandKind::Cash),
            vec![
                DemandObservation::new(at(99), dec!("1")),
                DemandObservation::new(at(100), dec!("10")),
                DemandObservation::new(at(150), dec!("100")),
                DemandObservation::new(at(200), dec!("1000")),
                DemandObservation::new(at(201), dec!("10000")),
            ],
        );
        history.insert(
            (lane("XTKS"), DemandKind::Cash),
            vec![DemandObservation::new(at(300), dec!("5"))],
        );
        // Premise: the history spans both sides of the window.
        assert_eq!(history.values().map(Vec::len).sum::<usize>(), 6);

        let realised = realised_between(&history, at(100), at(200));
        assert_eq!(
            realised.amount(&lane("XNYS"), DemandKind::Cash),
            dec!("1110")
        );
        // A lane the window never touched is absent, and absent reads as zero.
        assert_eq!(
            realised.amount(&lane("XTKS"), DemandKind::Cash),
            Decimal::ZERO
        );
        assert_eq!(realised.entries().count(), 1);
    }

    #[test]
    fn a_window_closes_at_the_plan_instant_plus_the_horizon_and_never_before_it() {
        let planned = at(1_000);
        let closes = window_closes_at(planned, Duration::from_days(1));
        assert_eq!(closes, at(1_000 + 86_400));
        assert!(window_closes_at(Timestamp::MAX, Duration::from_days(1)) >= planned);
    }
}
