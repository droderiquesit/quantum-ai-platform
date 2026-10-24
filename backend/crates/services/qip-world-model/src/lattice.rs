//! The Temporal Forecast Lattice — blueprint v12.0 §9.7, GCP v3.0 §11.3
//! (WORLD-3082..3085, WORLD-7197, WORLD-9022..9027).
//!
//! # The failure this exists to prevent
//!
//! The platform forecast at whatever horizon a thesis happened to state, and
//! scored the result under a label formatted from that duration — so there
//! was no population of models per horizon, no way to say "our one-day
//! forecasts are calibrated and our one-month ones are not", and nothing
//! stopped a microsecond model's output being promoted to a capital decision
//! it was never built to inform. This module makes each of those structural:
//!
//! * **Five fixed horizons** ([`LatticeHorizon`]), the §9.7 table's rows, each
//!   a fixed label, each with its own model population and its own promotion
//!   target. A model admitted at a horizon with any other target is refused
//!   ([`Population::admit`]), and a forecast issued from a lane the horizon
//!   does not promote to is refused ([`issue`]).
//! * **Every forecast is a calibrated distribution** — lane H's §9.14
//!   [`Forecast`], with quantiles, the aleatoric/epistemic split, a tail, the
//!   models consulted and their spread, and an expiry — never a point.
//! * **Horizons constrain each other** ([`constrain`]): a longer horizon may
//!   not be more certain about a cumulative quantity than a shorter one, and
//!   a shorter horizon's median must sit inside the longer one's 80%
//!   interval. A violation is flagged, never silently repaired.
//! * **Calibration is scored per horizon and per regime** ([`Calibrator`]):
//!   Brier on direction, pinball loss on the quantiles, interval coverage,
//!   with a bounded score history; and when the calibrator cannot run, model
//!   influence **freezes at the last safe weights** rather than drifting on
//!   whatever partial update was in flight (WORLD-9027).
//!
//! # Advisory, and structurally so
//!
//! Nothing here sizes, places or cancels anything. [`staleness_multiplier`]
//! states how a consumer *should* contract on a stale forecast; no order-path
//! caller reads it, and making one would be a sizing decision for an ADR.
//!
//! # Point in time
//!
//! A forecast is issued from state knowable at `issued_at` and resolved only
//! against an observation that closed at or after `issued_at + horizon` and
//! was itself knowable at the instant of scoring; [`Calibrator::score`]
//! refuses anything else.

use qip_contracts::forecast::{
    Calibration, Forecast, ForecastFields, OutcomeDistribution, QuantilePoint, Stated, TailMass,
    TailSide, UncertaintyDecomposition,
};
use qip_contracts::validated::Contract;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_numerics::distributions::normal_cdf;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// The 90th percentile of the standard normal.
const Z_80: f64 = 1.281_551_565_544_600_4;
/// The 1st percentile's distance from the mean, in standard deviations.
const Z_99: f64 = 2.326_347_874_040_841;

/// Score records kept in memory; the event log keeps the rest.
pub const SCORE_HISTORY: usize = 1_024;
/// Distinct regime labels a scorecard is kept for; past it a regime is
/// scored under [`OTHER_REGIME`] rather than minting another key.
pub const MAX_REGIMES: usize = 64;
pub const OTHER_REGIME: &str = "other";
/// Resolved forecasts a model needs at a horizon before its error moves its
/// weight. Fewer and the default ensemble answers.
pub const MIN_RESOLVED: u64 = 5;

/// The §9.7 table's five horizons. Fixed, so they are a bounded label set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatticeHorizon {
    MicrosecondsMs,
    SecondsMinutes,
    HoursDays,
    WeeksMonths,
    QuartersYears,
}

impl LatticeHorizon {
    pub const ALL: [Self; 5] = [
        Self::MicrosecondsMs,
        Self::SecondsMinutes,
        Self::HoursDays,
        Self::WeeksMonths,
        Self::QuartersYears,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MicrosecondsMs => "microseconds_ms",
            Self::SecondsMinutes => "seconds_minutes",
            Self::HoursDays => "hours_days",
            Self::WeeksMonths => "weeks_months",
            Self::QuartersYears => "quarters_years",
        }
    }

    /// The horizon a duration falls in. Each bucket is inclusive of its upper
    /// bound: under a second, up to an hour, up to fourteen days, up to a
    /// hundred and twenty days, and beyond.
    pub fn of(horizon: Duration) -> Self {
        if horizon < Duration::from_secs(1) {
            Self::MicrosecondsMs
        } else if horizon <= Duration::from_hours(1) {
            Self::SecondsMinutes
        } else if horizon <= Duration::from_days(14) {
            Self::HoursDays
        } else if horizon <= Duration::from_days(120) {
            Self::WeeksMonths
        } else {
            Self::QuartersYears
        }
    }

    /// The horizon the lattice forecasts at for this row.
    pub const fn representative(self) -> Duration {
        match self {
            Self::MicrosecondsMs => Duration::from_millis(1),
            Self::SecondsMinutes => Duration::from_mins(5),
            Self::HoursDays => Duration::from_days(1),
            Self::WeeksMonths => Duration::from_days(30),
            Self::QuartersYears => Duration::from_days(365),
        }
    }

    /// How long a forecast at this row may be read before it is stale and
    /// must be re-issued (WORLD-9024). Never longer than the horizon itself.
    pub const fn refresh(self) -> Duration {
        match self {
            Self::MicrosecondsMs => Duration::from_millis(1),
            Self::SecondsMinutes => Duration::from_mins(5),
            Self::HoursDays => Duration::from_days(1),
            Self::WeeksMonths => Duration::from_days(7),
            Self::QuartersYears => Duration::from_days(30),
        }
    }

    /// The §9.7 promotion target for this row.
    pub const fn promotion_target(self) -> PromotionTarget {
        match self {
            Self::MicrosecondsMs => PromotionTarget::LocalReflexOnly,
            Self::SecondsMinutes => PromotionTarget::ReflexWarmPackages,
            Self::HoursDays => PromotionTarget::StrategyCapitalRisk,
            Self::WeeksMonths => PromotionTarget::PortfolioTreasury,
            Self::QuartersYears => PromotionTarget::StrategicAllocationResearch,
        }
    }

    /// The rows served by the separate strategic service (WORLD-9025).
    pub const fn is_strategic(self) -> bool {
        matches!(self, Self::WeeksMonths | Self::QuartersYears)
    }

    /// The lane that may issue at this row.
    pub const fn lane(self) -> Lane {
        match self {
            Self::MicrosecondsMs => Lane::LocalReflex,
            _ => Lane::Central,
        }
    }
}

/// Where a horizon's outputs may be promoted (§9.7's last column).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionTarget {
    LocalReflexOnly,
    ReflexWarmPackages,
    StrategyCapitalRisk,
    PortfolioTreasury,
    StrategicAllocationResearch,
}

impl PromotionTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalReflexOnly => "local_reflex_only",
            Self::ReflexWarmPackages => "reflex_warm_packages",
            Self::StrategyCapitalRisk => "strategy_capital_risk",
            Self::PortfolioTreasury => "portfolio_treasury",
            Self::StrategicAllocationResearch => "strategic_allocation_research",
        }
    }
}

/// Which lane is issuing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// The reflex cell, which alone serves the microsecond row.
    LocalReflex,
    /// The central cycle and the deep brain.
    Central,
}

/// What family of model a member is: how it turns the same [`TargetInput`]
/// into a distribution. Two members that read the same state and answer
/// differently: their spread is real disagreement.
///
/// The first two are generic and serve every row. The other four are the
/// §9.7 table's named specialists (WORLD-3085..3088), each admitted at the
/// one row its signal speaks to and refused anywhere else, because an
/// order-flow model at the quarters row would be scored on a signal that has
/// decayed long before the forecast resolves and would earn a reputation on
/// noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFamily {
    /// The level is a martingale: the median is the level now.
    Martingale,
    /// The NOW state's rate of change is projected forward.
    Drift,
    /// WORLD-3085, seconds-minutes: order-flow imbalance
    /// ([`crate::order_flow::FlowObservation::imbalance_ratio`]) shifts the
    /// median by [`FLOW_SHIFT_PER_SD`] of the horizon's standard deviation
    /// per unit of imbalance and widens the row by the imbalance's share of
    /// the horizon's variance. No observed flow: the member states the level.
    OrderFlow,
    /// WORLD-3086, hours-days: a pending catalyst widens the row by the
    /// expected absolute reaction squared, direction unknown. No catalyst:
    /// the member states the level.
    EventReaction,
    /// WORLD-3087, weeks-months: the drift the current regime label has
    /// shown, not the drift of the last few bars; a regime change moves the
    /// median. No drift for the regime: the NOW drift, as the drift member.
    RegimePath,
    /// WORLD-3088, quarters-years: every live long-horizon causal claim
    /// bearing on the target adds its transmission squared of the row's own
    /// variance, because the claim passes on a fraction of a move whose
    /// direction and size over the horizon are unknown. A retired claim adds
    /// nothing, so retiring one narrows the row.
    Structural,
}

impl ModelFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Martingale => "martingale",
            Self::Drift => "drift",
            Self::OrderFlow => "order_flow",
            Self::EventReaction => "event_reaction",
            Self::RegimePath => "regime_path",
            Self::Structural => "structural",
        }
    }

    /// The one row a specialist serves, or `None` for a generic family.
    pub const fn served_row(self) -> Option<LatticeHorizon> {
        match self {
            Self::Martingale | Self::Drift => None,
            Self::OrderFlow => Some(LatticeHorizon::SecondsMinutes),
            Self::EventReaction => Some(LatticeHorizon::HoursDays),
            Self::RegimePath => Some(LatticeHorizon::WeeksMonths),
            Self::Structural => Some(LatticeHorizon::QuartersYears),
        }
    }
}

/// How far a full one-sided order-flow imbalance moves the seconds row's
/// median, in the horizon's own standard deviations. Half: flow is a lean,
/// not a forecast of the whole move.
pub const FLOW_SHIFT_PER_SD: f64 = 0.5;

/// One member of a horizon's model population.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatticeModel {
    pub id: String,
    pub horizon: LatticeHorizon,
    pub target: PromotionTarget,
    pub family: ModelFamily,
}

/// A catalyst the hours-days row is waiting on (WORLD-3086).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalystExpectation {
    /// What is pending: a macro series id, an event id.
    pub source: String,
    /// The expected absolute reaction as a fraction of the level, from the
    /// reaction episodes and macro surprises the platform has seen.
    pub expected_abs_move: f64,
}

/// A live long-horizon causal claim bearing on the target (WORLD-3088), as
/// the structural member reads it: who, through what, how much.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuralClaim {
    pub cause: String,
    pub mechanism: String,
    /// The signed fraction of the cause's move passed on to the target
    /// ([`crate::causal::CausalEdge::signed_transmission`]).
    pub transmission: f64,
    pub lag: Duration,
}

/// What the specialist families read beyond the level and its variance.
/// Every field is optional or empty by default, so a caller with no signal
/// gets the generic families' answer from every member rather than a
/// refusal; a caller with a signal that is not a number is refused.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ForecastContext {
    /// Order-flow imbalance in `[-1, 1]`, buy-positive, where flow was
    /// observed.
    pub flow_imbalance: Option<f64>,
    pub pending_catalyst: Option<CatalystExpectation>,
    /// The regime label in force now, as the calibrator scores it.
    pub regime: Option<String>,
    /// The drift per day, in target units, each regime label has shown.
    pub regime_drifts: BTreeMap<String, f64>,
    pub structural_claims: Vec<StructuralClaim>,
}

/// The per-horizon model populations (WORLD-3082).
#[derive(Clone, Debug, Default)]
pub struct Population {
    models: BTreeMap<LatticeHorizon, BTreeMap<String, LatticeModel>>,
}

impl Population {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a model. Refuses a blank or repeated id and — the §9.7
    /// constraint — a target other than its horizon's promotion target
    /// (WORLD-3084, WORLD-3085). A microsecond model aimed at a capital
    /// decision is the failure this exists for.
    pub fn admit(&mut self, model: LatticeModel) -> Result<()> {
        if model.id.trim().is_empty() {
            return Err(Error::invalid("a lattice model needs a non-blank id"));
        }
        if let Some(row) = model.family.served_row()
            && row != model.horizon
        {
            return Err(Error::denied(format!(
                "`{}` is a {} model, which serves only the {} row; it may not be admitted at {}",
                model.id,
                model.family.as_str(),
                row.as_str(),
                model.horizon.as_str()
            )));
        }
        let allowed = model.horizon.promotion_target();
        if model.target != allowed {
            return Err(Error::denied(format!(
                "`{}` serves the {} horizon, which promotes only to {}; it may not target {}",
                model.id,
                model.horizon.as_str(),
                allowed.as_str(),
                model.target.as_str()
            )));
        }
        let row = self.models.entry(model.horizon).or_default();
        if row.contains_key(&model.id) {
            return Err(Error::invalid(format!(
                "`{}` is already admitted at {}",
                model.id,
                model.horizon.as_str()
            )));
        }
        row.insert(model.id.clone(), model);
        Ok(())
    }

    pub fn models(&self, horizon: LatticeHorizon) -> impl Iterator<Item = &LatticeModel> {
        self.models
            .get(&horizon)
            .into_iter()
            .flat_map(|m| m.values())
    }

    /// Two generic members per row — a martingale and a drift-following
    /// model — and the row's specialist where §9.7 names one: order flow at
    /// seconds-minutes, event reaction at hours-days, the regime path at
    /// weeks-months and the structural claims at quarters-years. The
    /// microsecond row's members are admitted here so the population is
    /// whole, and only the reflex lane may issue from them.
    pub fn standard() -> Result<Self> {
        let mut p = Self::new();
        for horizon in LatticeHorizon::ALL {
            for family in [ModelFamily::Martingale, ModelFamily::Drift] {
                p.admit(LatticeModel {
                    id: format!("{}/{}", horizon.as_str(), family.as_str()),
                    horizon,
                    target: horizon.promotion_target(),
                    family,
                })?;
            }
        }
        for family in [
            ModelFamily::OrderFlow,
            ModelFamily::EventReaction,
            ModelFamily::RegimePath,
            ModelFamily::Structural,
        ] {
            let Some(horizon) = family.served_row() else {
                continue;
            };
            p.admit(LatticeModel {
                id: format!("{}/{}", horizon.as_str(), family.as_str()),
                horizon,
                target: horizon.promotion_target(),
                family,
            })?;
        }
        Ok(p)
    }
}

/// A forecast question (WORLD-9022).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub target: String,
    pub domain: String,
    pub horizon: LatticeHorizon,
}

/// How a question was routed: the weight each model carries, and whether the
/// reputation cache answered or the default ensemble did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Routed {
    pub weights: BTreeMap<String, f64>,
    pub fallback: bool,
}

impl Routed {
    /// The model with the most weight; ties go to the first id.
    pub fn best(&self) -> Option<&str> {
        self.weights
            .iter()
            .fold(None, |best: Option<(&String, f64)>, (id, w)| match best {
                Some((_, bw)) if bw >= *w => best,
                _ => Some((id, *w)),
            })
            .map(|(id, _)| id.as_str())
    }
}

/// What a reputation is earned in: a domain, a row, a model (WORLD-9022).
/// Keyed by domain because a member that forecasts closes well is not
/// thereby a good forecaster of a macro series, and a cache keyed by row
/// alone handed the one reputation to every question on the row.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReputationKey {
    pub domain: String,
    pub horizon: LatticeHorizon,
    pub model: String,
}

/// The reputation cache: weight per (domain, horizon, model).
pub type Reputation = BTreeMap<ReputationKey, f64>;

/// Route a question to its horizon's population. With a reputation cache
/// that weighs every member of the row in the question's domain, the
/// cache's weights; otherwise — the cache unavailable, or the row not yet
/// scored in that domain — the default ensemble, equal weights, marked as a
/// fallback.
pub fn route(
    population: &Population,
    question: &Question,
    reputation: Option<&Reputation>,
) -> Result<Routed> {
    let ids: Vec<String> = population
        .models(question.horizon)
        .map(|m| m.id.clone())
        .collect();
    if ids.is_empty() {
        return Err(Error::invalid(format!(
            "no model serves the {} horizon; admit one before asking it",
            question.horizon.as_str()
        )));
    }
    if let Some(cache) = reputation {
        let weights: Option<BTreeMap<String, f64>> = ids
            .iter()
            .map(|id| {
                cache
                    .get(&ReputationKey {
                        domain: question.domain.clone(),
                        horizon: question.horizon,
                        model: id.clone(),
                    })
                    .copied()
                    .filter(|w| w.is_finite() && *w >= 0.0)
                    .map(|w| (id.clone(), w))
            })
            .collect();
        if let Some(weights) = weights {
            let total: f64 = weights.values().sum();
            if total > 0.0 {
                return Ok(Routed {
                    weights: weights.into_iter().map(|(k, w)| (k, w / total)).collect(),
                    fallback: false,
                });
            }
        }
    }
    let equal = 1.0 / ids.len() as f64;
    Ok(Routed {
        weights: ids.into_iter().map(|id| (id, equal)).collect(),
        fallback: true,
    })
}

/// What a forecast of one target is computed from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetInput {
    /// The variable, e.g. `obj-AAA/close`.
    pub target: String,
    /// The level now, as the NOW state or the last close states it.
    pub level: f64,
    /// Uncertainty about the level itself, in squared target units: the
    /// higher-level state constraining every horizon below it.
    pub level_variance: f64,
    /// The NOW state's rate of change per day, where one was measured.
    pub drift_per_day: Option<f64>,
    /// Variance the target gains per day, in squared target units, from the
    /// observed microstructure: the lower level constraining the higher.
    pub variance_per_day: f64,
    pub regime_confidence_bp: u32,
    /// The signed quantity held, for the expected impact of the move.
    pub held_quantity: f64,
    /// At least one.
    pub evidence: Vec<String>,
    /// What the row's specialist reads, if anything.
    pub context: ForecastContext,
}

/// One issued forecast and what scoring it later needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Issued {
    pub forecast: Forecast,
    pub horizon: LatticeHorizon,
    pub model_means: BTreeMap<String, f64>,
    /// The forecast probability that the target ends above `reference`.
    pub p_up: f64,
    pub reference: f64,
    pub quantiles: [f64; 3],
}

fn decimal(x: f64, what: &str) -> Result<Decimal> {
    Decimal::from_f64(x)
        .ok_or_else(|| Error::numeric(format!("the forecast's {what} ({x}) has no decimal form")))
}

/// Issue one forecast at `horizon` from `lane`.
///
/// Refuses a lane the horizon does not promote to — the central cycle may not
/// issue a microsecond forecast, and the reflex lane may not issue anything
/// else — and any input the [`Forecast`] contract would refuse.
pub fn issue(
    lane: Lane,
    input: &TargetInput,
    horizon: LatticeHorizon,
    population: &Population,
    routed: &Routed,
    calibration: Calibration,
    issued_at: Timestamp,
) -> Result<Issued> {
    if lane != horizon.lane() {
        return Err(Error::denied(format!(
            "the {} horizon is served only from the {:?} lane; this lane may not issue it",
            horizon.as_str(),
            horizon.lane()
        )));
    }
    if !(input.level.is_finite()
        && input.level_variance.is_finite()
        && input.level_variance >= 0.0
        && input.variance_per_day.is_finite()
        && input.variance_per_day >= 0.0)
    {
        return Err(Error::numeric(format!(
            "the inputs for {} are not finite and non-negative; refuse the target rather than \
             forecast from them",
            input.target
        )));
    }
    check_context(&input.context, &input.target)?;
    let span = horizon.representative();
    let days = span.as_days_f64();
    let base_variance = input.variance_per_day * days;
    let mut model_means = BTreeMap::new();
    for model in population.models(horizon) {
        let Some(&w) = routed.weights.get(&model.id) else {
            continue;
        };
        if w <= 0.0 {
            continue;
        }
        let (mean, extra) = project(model.family, input, days, base_variance);
        model_means.insert(model.id.clone(), (mean, extra, w));
    }
    if model_means.is_empty() {
        return Err(Error::invalid(format!(
            "no routed model with weight serves {}",
            horizon.as_str()
        )));
    }
    let total_w: f64 = model_means.values().map(|(_, _, w)| w).sum();
    let mean = model_means.values().map(|(m, _, w)| m * w).sum::<f64>() / total_w;
    let between = model_means
        .values()
        .map(|(m, _, w)| w * (m - mean).powi(2))
        .sum::<f64>()
        / total_w;
    let (lo, hi) = model_means
        .values()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (m, _, _)| {
            (lo.min(*m), hi.max(*m))
        });
    // The variance the horizon adds, plus what each specialist says its
    // signal adds, weighted as the members are: a catalyst the event member
    // carries at full weight widens the row by all of it, and at no weight
    // by none of it.
    let aleatoric = base_variance
        + model_means
            .values()
            .map(|(_, extra, w)| extra * w)
            .sum::<f64>()
            / total_w;
    let epistemic = input.level_variance + between;
    let sd = (aleatoric + epistemic).sqrt();
    let quantiles = [mean - Z_80 * sd, mean, mean + Z_80 * sd];
    let p_up = if sd > 0.0 {
        normal_cdf((mean - input.level) / sd)
    } else if mean > input.level {
        1.0
    } else {
        0.0
    };
    let models_consulted = u32::try_from(model_means.len()).unwrap_or(u32::MAX);
    let disagreement = if models_consulted > 1 { hi - lo } else { 0.0 };
    // Statistic to money: the held quantity times the expected move, in the
    // target's own units (a price, so the account currency for a close).
    let expected_impact = input.held_quantity * (mean - input.level);
    let fields = ForecastFields {
        schema_version: ForecastFields::SCHEMA_VERSION,
        forecast_id: format!(
            "lattice/{}/{}/{}",
            horizon.as_str(),
            input.target,
            issued_at.as_nanos()
        ),
        model: format!("lattice-ensemble/{}", horizon.as_str()),
        target: input.target.clone(),
        issued_at,
        horizon: span,
        distribution: OutcomeDistribution::Quantiles {
            points: vec![
                QuantilePoint {
                    quantile_bp: 1_000,
                    value: decimal(quantiles[0], "10th percentile")?,
                },
                QuantilePoint {
                    quantile_bp: 5_000,
                    value: decimal(quantiles[1], "median")?,
                },
                QuantilePoint {
                    quantile_bp: 9_000,
                    value: decimal(quantiles[2], "90th percentile")?,
                },
            ],
        },
        uncertainty: UncertaintyDecomposition {
            epistemic: decimal(epistemic, "epistemic variance")?,
            aleatoric: decimal(aleatoric, "aleatoric variance")?,
        },
        tail: TailMass {
            side: TailSide::Below,
            threshold: decimal(mean - Z_99 * sd, "tail threshold")?,
            probability_bp: 100,
        },
        models_consulted,
        model_disagreement: decimal(disagreement, "model disagreement")?,
        regime_confidence_bp: Stated::Value(input.regime_confidence_bp),
        expected_impact: Stated::Value(decimal(expected_impact, "expected impact")?),
        calibration,
        evidence_lineage: input.evidence.clone(),
        expires_at: issued_at.saturating_add(horizon.refresh()),
    };
    let forecast = Forecast::new(fields)?;
    Ok(Issued {
        forecast,
        horizon,
        model_means: model_means
            .into_iter()
            .map(|(k, (m, _, _))| (k, m))
            .collect(),
        p_up,
        reference: input.level,
        quantiles,
    })
}

/// Refuse a context carrying a signal that is not a number or out of its
/// range. Refused, not clamped: an imbalance of 1.7 is a caller that
/// computed it wrong, and a forecast built on it would be scored as if the
/// flow had been read.
fn check_context(context: &ForecastContext, target: &str) -> Result<()> {
    if let Some(i) = context.flow_imbalance
        && !(i.is_finite() && (-1.0..=1.0).contains(&i))
    {
        return Err(Error::numeric(format!(
            "the order-flow imbalance for {target} ({i}) is not in [-1, 1]; refuse the reading \
             rather than forecast from it"
        )));
    }
    if let Some(c) = &context.pending_catalyst
        && !(c.expected_abs_move.is_finite() && c.expected_abs_move >= 0.0)
    {
        return Err(Error::numeric(format!(
            "the expected reaction to {} for {target} ({}) is not a non-negative number",
            c.source, c.expected_abs_move
        )));
    }
    if let Some((regime, d)) = context.regime_drifts.iter().find(|(_, d)| !d.is_finite()) {
        return Err(Error::numeric(format!(
            "the {regime} regime's drift for {target} ({d}) is not a number"
        )));
    }
    if let Some(c) = context
        .structural_claims
        .iter()
        .find(|c| !c.transmission.is_finite())
    {
        return Err(Error::numeric(format!(
            "the claim {} -> {target} through {} transmits {}, which is not a number",
            c.cause, c.mechanism, c.transmission
        )));
    }
    Ok(())
}

/// One member's answer: its mean, and the variance its own signal adds to
/// the row's `base_variance` (zero for the generic families). The
/// per-family arithmetic is documented on [`ModelFamily`].
fn project(family: ModelFamily, input: &TargetInput, days: f64, base_variance: f64) -> (f64, f64) {
    let level = input.level;
    let now_drift = input.drift_per_day.unwrap_or(0.0);
    let context = &input.context;
    match family {
        ModelFamily::Martingale => (level, 0.0),
        ModelFamily::Drift => (level + now_drift * days, 0.0),
        ModelFamily::OrderFlow => match context.flow_imbalance {
            Some(i) => (
                level + i * FLOW_SHIFT_PER_SD * base_variance.sqrt(),
                i.abs() * base_variance,
            ),
            None => (level, 0.0),
        },
        ModelFamily::EventReaction => match &context.pending_catalyst {
            Some(c) => (level, (c.expected_abs_move * level).powi(2)),
            None => (level, 0.0),
        },
        ModelFamily::RegimePath => {
            let drift = context
                .regime
                .as_ref()
                .and_then(|r| context.regime_drifts.get(r))
                .copied()
                .unwrap_or(now_drift);
            (level + drift * days, 0.0)
        }
        ModelFamily::Structural => (
            level,
            context
                .structural_claims
                .iter()
                .map(|c| c.transmission.powi(2) * base_variance)
                .sum(),
        ),
    }
}

/// How far a consumer should contract on a forecast's age (WORLD-9024): one
/// when fresh, falling linearly to zero at expiry, and refused after it.
/// Advisory — nothing on the order path reads it.
pub fn staleness_multiplier(forecast: &Forecast, now: Timestamp) -> Result<f64> {
    let fields = forecast.read_at(now)?;
    let life = fields.expires_at.since(fields.issued_at).as_secs_f64();
    let age = now.since(fields.issued_at).as_secs_f64().max(0.0);
    Ok((1.0 - age / life).max(0.0))
}

/// How two adjacent horizons disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InconsistencyKind {
    /// The longer horizon's 80% interval is narrower than the shorter's.
    Narrowing,
    /// The shorter horizon's median lies outside the longer's 80% interval.
    MedianOutside,
}

/// A cross-horizon constraint violation (WORLD-3083).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Inconsistency {
    pub target: String,
    pub shorter: String,
    pub longer: String,
    pub kind: InconsistencyKind,
}

fn quantile(f: &ForecastFields, bp: u32) -> Option<f64> {
    match &f.distribution {
        OutcomeDistribution::Quantiles { points } => points
            .iter()
            .find(|p| p.quantile_bp == bp)
            .map(|p| p.value.to_f64()),
        OutcomeDistribution::Categorical { .. } => None,
    }
}

/// Check every adjacent pair of a target's forecasts, shortest first.
/// Flags; never repairs — a silently widened interval is a forecast nobody
/// issued.
pub fn constrain(chain: &[&ForecastFields]) -> Vec<Inconsistency> {
    let mut sorted: Vec<&ForecastFields> = chain.to_vec();
    sorted.sort_by_key(|f| f.horizon);
    let mut out = Vec::new();
    for pair in sorted.windows(2) {
        let [a, b] = pair else { continue };
        if a.target != b.target {
            continue;
        }
        let (Some(a10), Some(a50), Some(a90), Some(b10), Some(b90)) = (
            quantile(a, 1_000),
            quantile(a, 5_000),
            quantile(a, 9_000),
            quantile(b, 1_000),
            quantile(b, 9_000),
        ) else {
            continue;
        };
        let name = |f: &ForecastFields| LatticeHorizon::of(f.horizon).as_str().to_string();
        if b90 - b10 < a90 - a10 {
            out.push(Inconsistency {
                target: a.target.clone(),
                shorter: name(a),
                longer: name(b),
                kind: InconsistencyKind::Narrowing,
            });
        }
        if a50 < b10 || a50 > b90 {
            out.push(Inconsistency {
                target: a.target.clone(),
                shorter: name(a),
                longer: name(b),
                kind: InconsistencyKind::MedianOutside,
            });
        }
    }
    out
}

/// A forecast reality has answered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub forecast_id: String,
    /// The question's domain, which the reputation earned here is keyed by.
    pub domain: String,
    pub horizon: LatticeHorizon,
    pub regime: String,
    pub model_means: BTreeMap<String, f64>,
    pub quantiles: [f64; 3],
    pub p_up: f64,
    pub reference: f64,
    pub issued_at: Timestamp,
    /// When the observation that answered it closed.
    pub observed_at: Timestamp,
    pub outcome: f64,
    pub resolved_at: Timestamp,
}

/// One scored forecast.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreRecord {
    pub forecast_id: String,
    pub horizon: LatticeHorizon,
    pub regime: String,
    /// Brier score of the direction call against the reference.
    pub brier: f64,
    /// Mean pinball loss over the three quantiles, relative to the reference.
    pub pinball: f64,
    /// Whether the outcome fell inside the 80% interval.
    pub covered: bool,
    pub resolved_at: Timestamp,
}

/// Running totals for one horizon (and, keyed further, one regime).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scorecard {
    pub resolved: u64,
    pub brier_sum: f64,
    pub pinball_sum: f64,
    pub covered: u64,
}

impl Scorecard {
    fn add(&mut self, r: &ScoreRecord) {
        self.resolved += 1;
        self.brier_sum += r.brier;
        self.pinball_sum += r.pinball;
        if r.covered {
            self.covered += 1;
        }
    }

    pub fn brier(&self) -> Option<f64> {
        (self.resolved > 0).then(|| self.brier_sum / self.resolved as f64)
    }

    pub fn pinball(&self) -> Option<f64> {
        (self.resolved > 0).then(|| self.pinball_sum / self.resolved as f64)
    }

    pub fn coverage(&self) -> Option<f64> {
        (self.resolved > 0).then(|| self.covered as f64 / self.resolved as f64)
    }
}

/// One member's measured error in a row: `(model, resolved, mean error)`.
type MemberError = (String, u64, f64);

/// What a weight refresh did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightUpdate {
    Updated,
    /// The calibrator was unavailable; weights held at the last safe set.
    Frozen,
}

/// The forecast calibrator (WORLD-9026, WORLD-9027).
#[derive(Clone, Debug, Default)]
pub struct Calibrator {
    history: VecDeque<ScoreRecord>,
    by_horizon: BTreeMap<LatticeHorizon, Scorecard>,
    by_regime: BTreeMap<(LatticeHorizon, String), Scorecard>,
    model_errors: BTreeMap<ReputationKey, (u64, f64)>,
    weights: Reputation,
    last_safe: Reputation,
    /// The set the last available refresh computed, held until the caller
    /// confirms the review that produced it reached durable storage. Until
    /// then it is not safe: a set built from scores the log refused would,
    /// if frozen on, be influence moved on a record nobody kept.
    unconfirmed: Option<Reputation>,
    frozen: bool,
}

impl Calibrator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Score one resolution. Refuses an observation that closed before the
    /// horizon elapsed or more than a horizon after it (either answers a
    /// different question), one not yet
    /// knowable at the resolution instant (look-ahead), a non-positive
    /// reference, and any non-finite figure.
    pub fn score(&mut self, r: &Resolution) -> Result<ScoreRecord> {
        let due = r.issued_at.saturating_add(r.horizon.representative());
        if r.observed_at < due {
            return Err(Error::invalid(format!(
                "{} is answered by an observation at {} before its horizon elapsed at {due}",
                r.forecast_id, r.observed_at
            )));
        }
        if r.observed_at > due.saturating_add(r.horizon.representative()) {
            return Err(Error::invalid(format!(
                "{} is answered by an observation at {}, more than a horizon past due at \
                 {due}; that answers a longer question than the one forecast",
                r.forecast_id, r.observed_at
            )));
        }
        if r.observed_at > r.resolved_at {
            return Err(Error::invalid(format!(
                "{} is scored at {} against an observation at {}; that is look-ahead",
                r.forecast_id, r.resolved_at, r.observed_at
            )));
        }
        let sound = r.reference.is_finite()
            && r.reference > 0.0
            && r.outcome.is_finite()
            && (0.0..=1.0).contains(&r.p_up)
            && r.quantiles.iter().all(|q| q.is_finite())
            && r.model_means.values().all(|m| m.is_finite());
        if !sound {
            return Err(Error::numeric(format!(
                "{} carries a figure that is not finite or a reference that is not positive",
                r.forecast_id
            )));
        }
        let up = if r.outcome > r.reference { 1.0 } else { 0.0 };
        let brier = (r.p_up - up).powi(2);
        let pinball = [0.1, 0.5, 0.9]
            .iter()
            .zip(r.quantiles)
            .map(|(q, x)| {
                let d = r.outcome - x;
                (q * d).max((q - 1.0) * d)
            })
            .sum::<f64>()
            / 3.0
            / r.reference;
        let covered = r.outcome >= r.quantiles[0] && r.outcome <= r.quantiles[2];
        let regime = if self
            .by_regime
            .keys()
            .any(|(h, g)| *h == r.horizon && *g == r.regime)
            || self.by_regime.len() < MAX_REGIMES
        {
            r.regime.clone()
        } else {
            OTHER_REGIME.to_string()
        };
        let record = ScoreRecord {
            forecast_id: r.forecast_id.clone(),
            horizon: r.horizon,
            regime: regime.clone(),
            brier,
            pinball,
            covered,
            resolved_at: r.resolved_at,
        };
        self.by_horizon.entry(r.horizon).or_default().add(&record);
        self.by_regime
            .entry((r.horizon, regime))
            .or_default()
            .add(&record);
        for (model, mean) in &r.model_means {
            let e = self
                .model_errors
                .entry(ReputationKey {
                    domain: r.domain.clone(),
                    horizon: r.horizon,
                    model: model.clone(),
                })
                .or_insert((0, 0.0));
            e.0 += 1;
            e.1 += (r.outcome - mean).abs() / r.reference;
        }
        self.history.push_back(record.clone());
        while self.history.len() > SCORE_HISTORY {
            self.history.pop_front();
        }
        Ok(record)
    }

    /// Recompute the ensemble weights from measured model error, or — when
    /// `available` is false — hold them at the last safe set. A row whose
    /// members have not all reached [`MIN_RESOLVED`] gets no weights, so the
    /// router's default ensemble answers it.
    ///
    /// A recomputed set is used at once but becomes the last safe set only
    /// through [`Self::confirm_safe`], once the review carrying it is
    /// durable (R12-WORLD-9027). Freezing discards an unconfirmed set.
    pub fn refresh_weights(&mut self, available: bool) -> WeightUpdate {
        if !available {
            self.unconfirmed = None;
            self.weights = self.last_safe.clone();
            self.frozen = true;
            return WeightUpdate::Frozen;
        }
        let mut weights = Reputation::new();
        let mut rows: BTreeMap<(String, LatticeHorizon), Vec<MemberError>> = BTreeMap::new();
        for (key, (n, sum)) in &self.model_errors {
            rows.entry((key.domain.clone(), key.horizon))
                .or_default()
                .push((key.model.clone(), *n, *sum / *n as f64));
        }
        for ((domain, h), members) in rows {
            if members.iter().any(|(_, n, _)| *n < MIN_RESOLVED) {
                continue;
            }
            let inverse: Vec<(String, f64)> = members
                .into_iter()
                .map(|(m, _, err)| (m, 1.0 / (err + 1e-6)))
                .collect();
            let total: f64 = inverse.iter().map(|(_, w)| w).sum();
            for (m, w) in inverse {
                weights.insert(
                    ReputationKey {
                        domain: domain.clone(),
                        horizon: h,
                        model: m,
                    },
                    w / total,
                );
            }
        }
        self.weights = weights.clone();
        self.unconfirmed = Some(weights);
        self.frozen = false;
        WeightUpdate::Updated
    }

    /// The review carrying the last refreshed set reached durable storage:
    /// that set is now the one a later freeze returns to. A no-op when no
    /// refresh is waiting.
    pub fn confirm_safe(&mut self) {
        if let Some(weights) = self.unconfirmed.take() {
            self.last_safe = weights;
        }
    }

    pub fn weights(&self) -> &Reputation {
        &self.weights
    }

    pub fn last_safe(&self) -> &Reputation {
        &self.last_safe
    }

    pub const fn frozen(&self) -> bool {
        self.frozen
    }

    pub fn scorecard(&self, horizon: LatticeHorizon) -> Option<&Scorecard> {
        self.by_horizon.get(&horizon)
    }

    pub fn regime_scorecards(&self) -> &BTreeMap<(LatticeHorizon, String), Scorecard> {
        &self.by_regime
    }

    pub fn history(&self) -> impl Iterator<Item = &ScoreRecord> {
        self.history.iter()
    }

    /// The horizon's calibration as lane H's contract states it: measured
    /// mean pinball loss once anything has resolved, `Uncalibrated` before.
    pub fn calibration(&self, horizon: LatticeHorizon, at: Timestamp) -> Calibration {
        match self.by_horizon.get(&horizon) {
            Some(card) if card.resolved > 0 => card.pinball().and_then(Decimal::from_f64).map_or(
                Calibration::Uncalibrated,
                |score| Calibration::Measured {
                    method: "pinball".to_string(),
                    score,
                    resolved: card.resolved,
                    evaluated_at: at,
                },
            ),
            _ => Calibration::Uncalibrated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_duration_lands_in_exactly_one_of_the_five_rows() {
        let cases = [
            (Duration::from_micros(5), LatticeHorizon::MicrosecondsMs),
            (Duration::from_millis(999), LatticeHorizon::MicrosecondsMs),
            (Duration::from_secs(1), LatticeHorizon::SecondsMinutes),
            (Duration::from_hours(1), LatticeHorizon::SecondsMinutes),
            (Duration::from_days(1), LatticeHorizon::HoursDays),
            (Duration::from_days(30), LatticeHorizon::WeeksMonths),
            (Duration::from_days(365), LatticeHorizon::QuartersYears),
        ];
        for (d, h) in cases {
            assert_eq!(LatticeHorizon::of(d), h, "{d:?}");
        }
        for h in LatticeHorizon::ALL {
            assert_eq!(LatticeHorizon::of(h.representative()), h);
            assert!(h.refresh() <= h.representative());
        }
    }
}
