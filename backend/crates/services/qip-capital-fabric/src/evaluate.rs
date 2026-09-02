//! Scoring a plan once the world has happened.
//!
//! A forecaster with no scorer becomes confident rather than accurate. Nothing
//! in [`crate::plan`] can tell whether its intervals were honest, whether its
//! refusals were right, or whether the capital it moved was moved to the place
//! that turned out to need it — and every one of those is answerable the
//! following week, cheaply, from data the plan already carries.
//!
//! So [`evaluate`] replays a plan against realised demand and produces the four
//! numbers that make the layer improvable:
//!
//! * **Coverage.** Was the capital where it turned out to be needed.
//! * **What the positioning cost.** Transfer costs paid, plus carry on capital
//!   that turned out not to be needed.
//! * **Value against doing nothing.** The headline score. A plan that moved
//!   nothing scores exactly zero — not "unscored", not "skipped". A layer that
//!   only scores the transfers it made cannot distinguish a forecaster that
//!   declined correctly from one that never looked, and those are the two cases
//!   worth telling apart.
//! * **Forecast error and bias.** Signed error against the point estimate, and
//!   whether the interval contained the outcome at the rate it claimed. Bias is
//!   the one that compounds: a forecaster that is 10% low every week is not
//!   noisy, it is wrong in a fixable direction.
//!
//! Every lane the plan considered is scored, including the refused ones, which
//! is what makes a refusal falsifiable.

use crate::forecast::{DemandForecast, DemandKind};
use crate::location::CapitalLocation;
use crate::plan::PrePositioningPlan;
use crate::transfer::ShortfallAsymmetry;
use qip_core::error::Result;
use qip_core::rng::Rng;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What each lane actually needed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RealisedDemand {
    entries: BTreeMap<(CapitalLocation, DemandKind), Decimal>,
}

impl RealisedDemand {
    /// An empty record — every lane needed nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what one lane needed.
    ///
    /// A repeated lane accumulates rather than replaces, so two partial reports
    /// of the same lane sum to the demand that actually arose.
    pub fn with(mut self, location: CapitalLocation, kind: DemandKind, amount: Decimal) -> Self {
        *self
            .entries
            .entry((location, kind))
            .or_insert(Decimal::ZERO) += amount;
        self
    }

    /// What a lane needed. Absent means nothing, which is a real outcome.
    pub fn amount(&self, location: &CapitalLocation, kind: DemandKind) -> Decimal {
        self.entries
            .get(&(location.clone(), kind))
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// Every lane recorded, in a stable order.
    pub fn entries(&self) -> impl Iterator<Item = (&CapitalLocation, DemandKind, Decimal)> {
        self.entries
            .iter()
            .map(|((location, kind), amount)| (location, *kind, *amount))
    }

    /// Draw a realised world from a set of forecasts.
    ///
    /// The only place in this crate that consumes randomness, and it takes the
    /// generator as an argument: a seeded [`qip_core::Xoshiro256`] produces the
    /// same scenario every run, so a backtest is a measurement rather than an
    /// anecdote.
    pub fn sample(forecasts: &[DemandForecast], rng: &mut impl Rng) -> Self {
        let mut realised = Self::new();
        for forecast in forecasts {
            realised = realised.with(
                forecast.location.clone(),
                forecast.kind,
                forecast.sample(rng),
            );
        }
        realised
    }
}

/// How one lane turned out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaneOutcome {
    /// The lane.
    pub location: CapitalLocation,
    /// The kind of demand.
    pub kind: DemandKind,
    /// What it actually needed, in its own currency.
    pub realised: Decimal,
    /// What was already there.
    pub on_hand: Decimal,
    /// What the plan sent.
    pub positioned: Decimal,
    /// Demand that went unmet.
    pub shortfall: Decimal,
    /// Demand that would have gone unmet had the plan done nothing.
    pub baseline_shortfall: Decimal,
    /// Pre-positioned capital that turned out not to be needed.
    ///
    /// Only the part the plan itself sent. Capital that was already sitting
    /// there is not this plan's surplus, and charging it here would score the
    /// planner for a decision somebody else made.
    pub idle_surplus: Decimal,
    /// Whether the realised demand fell inside the forecast interval.
    pub interval_covered: bool,
    /// Realised less the point estimate. A statistic, hence `f64`.
    pub forecast_error_stat: f64,
}

impl LaneOutcome {}

/// What a plan was worth, after the fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanScore {
    /// When the plan was built.
    pub at: Timestamp,
    /// Capital pre-positioned, in the base currency.
    pub positioned: Decimal,
    /// Realised demand that was met, in the base currency.
    pub covered: Decimal,
    /// Realised demand that went unmet, in the base currency.
    pub shortfall: Decimal,
    /// Pre-positioned capital that turned out not to be needed.
    pub idle_surplus: Decimal,
    /// Transfer costs actually paid.
    pub transfer_cost: Decimal,
    /// Penalty on the shortfall the plan left.
    pub shortfall_penalty: Decimal,
    /// Carry on the capital the plan tied up unnecessarily.
    pub surplus_penalty: Decimal,
    /// The penalty a plan that moved nothing would have paid.
    ///
    /// The baseline the score is taken against, reported so the score can be
    /// read without recomputing it.
    pub baseline_shortfall_penalty: Decimal,
    /// The score: penalty avoided, less everything the positioning cost.
    ///
    /// Zero for a plan that moved nothing, positive for a plan that helped,
    /// negative for one that cost more than it saved. Signed on purpose — a
    /// score that cannot go below its baseline cannot tell you that you are
    /// over-positioning.
    pub net_value: Decimal,
    /// Every lane, including the refused ones.
    pub lanes: Vec<LaneOutcome>,
    /// Share of realised demand met across the book. A statistic.
    pub coverage_ratio_stat: f64,
    /// Mean absolute forecast error across lanes. A statistic.
    pub mean_absolute_error_stat: f64,
    /// Mean signed forecast error; positive means demand was under-forecast.
    pub bias_stat: f64,
    /// Share of lanes whose realised demand fell inside its interval.
    ///
    /// Compared against the coverage the intervals claimed, this is the
    /// calibration check: an 80% interval that contains the outcome 40% of the
    /// time is not an 80% interval.
    pub interval_hit_rate_stat: f64,
}

impl PlanScore {
    /// Whether the plan was worth making.
    pub fn beat_doing_nothing(&self) -> bool {
        self.net_value.is_positive()
    }

    /// A line for a weekly review.
    pub fn describe(&self) -> String {
        format!(
            "positioned {} covering {:.1}% of realised demand, {} short and {} idle; \
             {} transfer cost against {} of avoided shortfall, net {}; forecasts ran {:.1} \
             high on average with {:.0}% interval coverage",
            self.positioned,
            self.coverage_ratio_stat * 100.0,
            self.shortfall,
            self.idle_surplus,
            self.transfer_cost,
            self.baseline_shortfall_penalty - self.shortfall_penalty,
            self.net_value,
            -self.bias_stat,
            self.interval_hit_rate_stat * 100.0,
        )
    }
}

/// Score a plan against what the world turned out to need.
///
/// Reads no clock and draws no random numbers; the plan carries the instant it
/// was built at and the rates it was built against, so a replay reproduces the
/// score exactly.
pub fn evaluate(plan: &PrePositioningPlan, realised: &RealisedDemand) -> Result<PlanScore> {
    let mut lanes = Vec::with_capacity(plan.lanes.len());
    let mut positioned = Decimal::ZERO;
    let mut covered = Decimal::ZERO;
    let mut shortfall = Decimal::ZERO;
    let mut idle_surplus = Decimal::ZERO;
    let mut shortfall_penalty = Decimal::ZERO;
    let mut surplus_penalty = Decimal::ZERO;
    let mut baseline_shortfall_penalty = Decimal::ZERO;
    let mut realised_total = Decimal::ZERO;
    let mut errors_stat: Vec<f64> = Vec::with_capacity(plan.lanes.len());
    let mut interval_hits = 0usize;

    for lane in &plan.lanes {
        let currency = lane.location.currency;
        let asymmetry = ShortfallAsymmetry::for_kind(lane.kind)?;
        let need = realised.amount(&lane.location, lane.kind);
        let available = lane.on_hand + lane.positioned;

        let lane_shortfall = (need - available).max(Decimal::ZERO);
        let baseline_shortfall = (need - lane.on_hand).max(Decimal::ZERO);
        // Only the capital this plan sent can be this plan's idle surplus.
        let lane_surplus = (available - need).max(Decimal::ZERO).min(lane.positioned);

        let lane_shortfall_penalty = asymmetry.shortfall_penalty(lane_shortfall, lane.reactive_lag);
        let lane_baseline_penalty =
            asymmetry.shortfall_penalty(baseline_shortfall, lane.reactive_lag);
        let lane_surplus_penalty = asymmetry.surplus_penalty(lane_surplus, lane.committed_for);

        let error_stat = need.to_f64() - lane.forecast_point.to_f64();
        let hit = need >= lane.forecast_lower && need <= lane.forecast_upper;
        if hit {
            interval_hits += 1;
        }
        errors_stat.push(error_stat);

        positioned += plan.rates.to_base(lane.positioned, currency)?;
        covered += plan
            .rates
            .to_base((need - lane_shortfall).max(Decimal::ZERO), currency)?;
        realised_total += plan.rates.to_base(need, currency)?;
        shortfall += plan.rates.to_base(lane_shortfall, currency)?;
        idle_surplus += plan.rates.to_base(lane_surplus, currency)?;
        shortfall_penalty += plan.rates.to_base(lane_shortfall_penalty, currency)?;
        surplus_penalty += plan.rates.to_base(lane_surplus_penalty, currency)?;
        baseline_shortfall_penalty += plan.rates.to_base(lane_baseline_penalty, currency)?;

        lanes.push(LaneOutcome {
            location: lane.location.clone(),
            kind: lane.kind,
            realised: need,
            on_hand: lane.on_hand,
            positioned: lane.positioned,
            shortfall: lane_shortfall,
            baseline_shortfall,
            idle_surplus: lane_surplus,
            interval_covered: hit,
            forecast_error_stat: error_stat,
        });
    }

    // Costs are paid in the destination currency and converted once, through
    // the same rates the plan was decided on.
    let transfer_cost_base = plan
        .moves
        .iter()
        .try_fold(Decimal::ZERO, |acc, m| -> Result<Decimal> {
            Ok(acc + plan.rates.to_base(m.cost.total, m.to.currency)?)
        })?;

    let net_value =
        baseline_shortfall_penalty - shortfall_penalty - surplus_penalty - transfer_cost_base;

    let lane_count = lanes.len().max(1) as f64;
    let mean_absolute_error_stat = errors_stat.iter().map(|e| e.abs()).sum::<f64>() / lane_count;
    let bias_stat = errors_stat.iter().sum::<f64>() / lane_count;

    Ok(PlanScore {
        at: plan.at,
        positioned,
        covered,
        shortfall,
        idle_surplus,
        transfer_cost: transfer_cost_base,
        shortfall_penalty,
        surplus_penalty,
        baseline_shortfall_penalty,
        net_value,
        coverage_ratio_stat: if realised_total.is_positive() {
            covered.to_f64() / realised_total.to_f64()
        } else {
            1.0
        },
        mean_absolute_error_stat,
        bias_stat,
        interval_hit_rate_stat: if lanes.is_empty() {
            1.0
        } else {
            interval_hits as f64 / lanes.len() as f64
        },
        lanes,
    })
}
