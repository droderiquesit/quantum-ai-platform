//! The Model Tournament and Forecast Scoreboard (blueprint §9.7, §9.8, §22.1;
//! R12-MODEL-7198, -3089..-3095, -6018..-6022; R12-WORLD-3086..-3088).
//!
//! Many forecasters — standing specialists and temporary ones that expire —
//! forecast the *same* question independently. When the question resolves
//! each is scored, and influence over the aggregate, and over the compute the
//! platform spends on them, is earned from that record rather than assigned.
//!
//! # What a forecaster is scored on
//!
//! Per forecaster **per regime** — the regime captured when the question was
//! opened, never the one in force when it resolved (a crisis call scored
//! under the quiet tape it resolved in credits the model for conditions it
//! was never asked about; see `qip_cost_router::reputation`): Brier score,
//! log loss, calibration error, tail accuracy, economic usefulness and
//! robustness — the six §9.8 names — plus directional accuracy and quantile
//! (pinball) loss (§22.1), incremental information value, and the cost and
//! latency of producing each forecast. A metric that could not be computed
//! is [`Measure::Unmeasured`] with the reason, never a zero.
//!
//! # How influence is earned
//!
//! [`Tournament::influence`] multiplies six factors in `(0, 1]`: the
//! forecaster's share of the synthetic-credit market
//! ([`crate::forecast_market`]), calibration, information value, cost,
//! latency, and the meta-model's predicted survival in the regime now in
//! force ([`FailurePredictor`]). Each is a stated formula below, and each can
//! only *reduce* a forecaster's weight relative to its market share.
//!
//! # Advisory
//!
//! The tournament holds no handle to a thesis, a size or an order. Its
//! aggregate and its compute allocation are records; what the platform does
//! with them is a separate decision.
//!
//! # Bounded
//!
//! At most [`MAX_FORECASTERS`] forecasters (expired temporaries are evicted
//! first, then admission is refused), [`MAX_OPEN_QUESTIONS`] open questions,
//! [`SCORE_WINDOW`] scores per forecaster per regime, [`MAX_REGIMES`] regimes
//! per forecaster and [`MAX_RESEARCH_TASKS`] undrained research tasks.

use crate::forecast_market::{ForecastMarket, SyntheticCredits, allocate};
use qip_ai::evaluation::{Calibration, PredictionOutcome};
use qip_contracts::forecast::{Forecast, OutcomeDistribution};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Forecasters the tournament holds. "Thousands" (§9.8) as a cap.
pub const MAX_FORECASTERS: usize = 4_096;

/// Questions open at once.
pub const MAX_OPEN_QUESTIONS: usize = 1_024;

/// Scores each forecaster keeps per regime; older ones fall off the front.
pub const SCORE_WINDOW: usize = 128;

/// Regimes each forecaster keeps a record for, least recently scored evicted.
pub const MAX_REGIMES: usize = 32;

/// Research tasks held between drains.
pub const MAX_RESEARCH_TASKS: usize = 256;

/// Resolved forecasts a forecaster needs before it may be promoted out of
/// probation — the same ten the self-model withholds an estimate below.
pub const PROMOTION_SAMPLE: usize = crate::self_model::MINIMUM_SAMPLE;

/// Spread in probability on any one outcome, across forecasters on one
/// question, above which the disagreement becomes a research task. Forty
/// points: two forecasters at 30% and 70% disagree about the direction of the
/// world, not its degree.
pub const DISAGREEMENT_BAR: f64 = 0.4;

/// The smallest probability any score takes the log of. A forecaster that
/// put zero on what happened is scored as if it had said one in a million:
/// heavily, finitely, and the same way on every replay.
pub const PROBABILITY_FLOOR: f64 = 1e-6;

/// Bins the calibration error is computed over.
const CALIBRATION_BINS: usize = 10;

/// The horizon bands of the Temporal Forecast Lattice (§9.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatticeHorizon {
    /// Under an hour: the reflex band.
    Reflex,
    /// An hour to two weeks.
    HoursDays,
    /// Two weeks to six months.
    WeeksMonths,
    /// Six months and beyond.
    QuartersYears,
}

impl LatticeHorizon {
    /// The band a horizon falls in.
    pub fn of(horizon: Duration) -> Self {
        let hours = horizon.as_nanos() / 3_600_000_000_000;
        match hours {
            ..1 => Self::Reflex,
            1..336 => Self::HoursDays,
            336..4_320 => Self::WeeksMonths,
            _ => Self::QuartersYears,
        }
    }

    /// The only lane a forecaster in this band may be promoted to (§9.7's
    /// table). A model promoted to the wrong lane breaks the latency or the
    /// authority bound of the lane it lands in.
    pub const fn admitted_lane(self) -> PromotionLane {
        match self {
            Self::Reflex => PromotionLane::Reflex,
            Self::HoursDays => PromotionLane::StrategyCapitalRisk,
            Self::WeeksMonths => PromotionLane::PortfolioTreasury,
            Self::QuartersYears => PromotionLane::StrategicAllocationResearch,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reflex => "reflex",
            Self::HoursDays => "hours_days",
            Self::WeeksMonths => "weeks_months",
            Self::QuartersYears => "quarters_years",
        }
    }
}

/// Where a forecaster's output may be promoted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionLane {
    Reflex,
    StrategyCapitalRisk,
    PortfolioTreasury,
    StrategicAllocationResearch,
}

impl PromotionLane {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reflex => "reflex",
            Self::StrategyCapitalRisk => "strategy_capital_risk",
            Self::PortfolioTreasury => "portfolio_treasury",
            Self::StrategicAllocationResearch => "strategic_allocation_research",
        }
    }
}

/// Whether a forecaster is standing or temporary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ForecasterKind {
    /// A standing specialist.
    Specialist,
    /// Spawned for one company, event or shock and retired at `expires_at`.
    Temporary { expires_at: Timestamp },
}

/// What a forecaster is allowed to do with what it has earned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "standing")]
pub enum Standing {
    /// Scored and settled, but carries no influence and no compute share
    /// until it has [`PROMOTION_SAMPLE`] resolved forecasts.
    Probation,
    /// Promoted: carries influence in its lane.
    Active,
    /// Held out by the verifier society; scored, never promoted until the
    /// refutation is withdrawn.
    Blocked { reason: String },
}

/// What admission needs to know about a forecaster.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForecasterSpec {
    pub id: String,
    pub kind: ForecasterKind,
    pub horizon: LatticeHorizon,
    /// The lane it asks to be promoted to; refused unless it is the band's.
    pub lane: PromotionLane,
}

/// A metric that was computed, or the reason it was not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Measure {
    Measured { value: f64 },
    Unmeasured { reason: String },
}

impl Measure {
    pub fn value(&self) -> Option<f64> {
        match self {
            Self::Measured { value } => Some(*value),
            Self::Unmeasured { .. } => None,
        }
    }

    fn mean(values: &[f64], reason: &str) -> Self {
        if values.is_empty() {
            return Self::Unmeasured {
                reason: reason.to_string(),
            };
        }
        Self::Measured {
            value: values.iter().sum::<f64>() / values.len() as f64,
        }
    }
}

/// One forecaster's record in one regime: the six §9.8 metrics and the four
/// §22.1 adds. Every field is required on the wire — a scorecard missing a
/// metric is a schema break, not a quiet gap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scorecard {
    pub forecaster: String,
    pub regime: String,
    pub resolved: usize,
    pub brier: Measure,
    pub log_loss: Measure,
    /// Expected calibration error over the forecaster's stated probabilities.
    pub calibration_error: Measure,
    /// Mean Brier on questions whose realised move was a tail move. Lower is
    /// better; a forecaster good on average and wrong in the tails scores
    /// badly here and nowhere else.
    pub tail_brier: Measure,
    /// Mean of `(p(outcome) − 1/k) × |realised move|`: conviction toward
    /// what happened, sized by how much it mattered. A statistic in the
    /// question's own units (basis points of move), not P&L.
    pub economic_usefulness: Measure,
    /// The forecaster's mean Brier across its regimes minus this regime's:
    /// negative where this regime is a weakness.
    pub robustness: Measure,
    /// Share of resolved forecasts whose most likely outcome happened.
    pub directional_accuracy: Measure,
    /// Mean pinball loss over stated quantiles, where any were stated.
    pub quantile_loss: Measure,
    /// Mean log-score gain the forecaster added over the aggregate without it.
    pub information_value: Measure,
    pub mean_cost: f64,
    pub mean_latency_ms: f64,
}

/// A forecast as the tournament scores it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TournamentForecast {
    pub forecaster: String,
    pub forecast_id: String,
    pub target: String,
    pub issued_at: Timestamp,
    /// Probability per outcome, in basis points summing to 10,000, where the
    /// forecaster stated a categorical view.
    pub probabilities_bp: Option<BTreeMap<String, u32>>,
    /// `(quantile, value)` in increasing quantile order, where it stated a
    /// distribution over the realised value.
    pub quantiles: Option<Vec<(f64, f64)>>,
    /// Compute units the forecast cost to produce.
    pub cost: f64,
    pub latency: Duration,
}

impl TournamentForecast {
    /// Read a §9.14 [`Forecast`] contract. The contract is already validated,
    /// so the distribution's masses sum to one and its quantiles increase.
    pub fn from_contract(forecast: &Forecast, cost: f64, latency: Duration) -> Self {
        let (probabilities_bp, quantiles) = match &forecast.distribution {
            OutcomeDistribution::Categorical { outcomes } => (
                Some(
                    outcomes
                        .iter()
                        .map(|o| (o.outcome.clone(), o.probability_bp))
                        .collect(),
                ),
                None,
            ),
            OutcomeDistribution::Quantiles { points } => (
                None,
                Some(
                    points
                        .iter()
                        .map(|p| (f64::from(p.quantile_bp) / 10_000.0, p.value.to_f64()))
                        .collect(),
                ),
            ),
        };
        Self {
            forecaster: forecast.model.clone(),
            forecast_id: forecast.forecast_id.clone(),
            target: forecast.target.clone(),
            issued_at: forecast.issued_at,
            probabilities_bp,
            quantiles,
            cost,
            latency,
        }
    }

    fn probability(&self, outcome: &str) -> Option<f64> {
        self.probabilities_bp
            .as_ref()
            .map(|p| f64::from(p.get(outcome).copied().unwrap_or(0)) / 10_000.0)
    }
}

/// What opening a question needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionSpec {
    pub id: String,
    pub target: String,
    /// The exhaustive outcomes, at least two.
    pub outcomes: Vec<String>,
    /// The regime label in force **now**, at opening. Every score on this
    /// question is filed under it.
    pub regime: String,
    pub resolves_at: Timestamp,
    /// A realised move at least this large (in the realised value's units)
    /// is a tail outcome. `None` where nobody could state one.
    pub tail_threshold: Option<f64>,
}

/// An open question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Question {
    spec: QuestionSpec,
    opened_at: Timestamp,
    forecasts: BTreeMap<String, TournamentForecast>,
    /// Held out of the market aggregate by the verifier society.
    held_out: Option<String>,
}

impl Question {
    pub fn spec(&self) -> &QuestionSpec {
        &self.spec
    }

    pub fn forecasts(&self) -> &BTreeMap<String, TournamentForecast> {
        &self.forecasts
    }

    pub fn held_out(&self) -> Option<&str> {
        self.held_out.as_deref()
    }
}

/// What happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub outcome: String,
    /// The realised value (a move in basis points, say), where measured.
    pub realised_value: Option<f64>,
    pub observed_at: Timestamp,
}

/// One forecast as scored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoredForecast {
    pub forecaster: String,
    pub forecast_id: String,
    pub question: String,
    pub regime: String,
    /// The outcome that happened.
    pub outcome: String,
    pub brier: Option<f64>,
    pub log_loss: Option<f64>,
    pub hit: Option<bool>,
    pub tail: Option<bool>,
    pub pinball: Option<f64>,
    pub economic: Option<f64>,
    pub information_value: Option<f64>,
    /// `(stated probability, whether it happened)` for every outcome, for
    /// the calibration error.
    pub calibration_points: Vec<(f64, bool)>,
    pub cost: f64,
    pub latency_ms: f64,
}

/// Material disagreement, preserved as a research signal (R12-MODEL-3091).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchTask {
    pub question: String,
    pub target: String,
    /// The widest spread on any outcome.
    pub spread: f64,
    pub outcome: String,
    /// Every forecast on the question, cited by id with the probability it
    /// gave `outcome`, in forecaster order. The disagreement itself — not a
    /// merged number.
    pub cites: Vec<(String, f64)>,
    pub raised_at: Timestamp,
}

/// A resolved question, as the record keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedQuestion {
    pub question: String,
    pub regime: String,
    pub outcome: String,
    /// The aggregate the market held for the outcome, before settlement.
    pub aggregate: Option<f64>,
    pub scores: Vec<ScoredForecast>,
    pub settlements: Vec<crate::forecast_market::Settlement>,
    pub held_out: Option<String>,
}

/// The six factors behind one forecaster's weight, each in `(0, 1]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InfluenceWeight {
    pub forecaster: String,
    pub market_share: f64,
    pub calibration: f64,
    /// [`information_factor`] of the mean leave-one-out information value:
    /// one half for a forecaster that adds nothing the others did not, and
    /// zero, not one half, for a record whose value is not a finite number.
    pub information: f64,
    pub cost: f64,
    pub latency: f64,
    pub regime_survival: f64,
    /// The product, normalised across the forecasters weighed together.
    pub weight: f64,
}

/// A promotion the tournament made or refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionDecision {
    pub forecaster: String,
    pub lane: PromotionLane,
    pub promoted: bool,
    pub reason: String,
}

/// The information factor of a forecaster whose mean leave-one-out
/// information value is `mean_value` nats: `r / (1 + r)` with
/// `r = exp(mean_value)`, computed as `1 / (1 + exp(−mean_value))` so a large
/// value cannot overflow to `∞ / ∞`.
///
/// Why not `exp(min(mean_value, 0))`, the factor this replaced: it read one
/// for every forecaster whose value was zero or better, so a forecaster that
/// added *nothing* — an exact copy of another, whose leave-one-out value is
/// zero because the aggregate pools identical distributions as one signal —
/// kept the same full weight as one whose forecasts the ensemble could not
/// have produced without it. Each copy then drew a full share of influence
/// and compute for the same information, and the ensemble double-counted it
/// everywhere influence is read rather than pooled (R12-MODEL-3090). Here
/// the factor is strictly increasing in the value, so zero incremental
/// information reads one half and anything that genuinely moved the
/// aggregate toward what happened reads above it.
///
/// A value that is not a finite number earns nothing: it is not a
/// measurement anybody made — every stored value is the difference of two
/// logarithms of probabilities floored at [`PROBABILITY_FLOOR`] — and
/// reading it as neutral would grant influence on a corrupted record, while
/// letting it through would make every forecaster's normalised weight NaN.
fn information_factor(mean_value: f64) -> f64 {
    if !mean_value.is_finite() {
        return 0.0;
    }
    1.0 / (1.0 + (-mean_value).exp())
}

/// The meta-model (R12-MODEL-3095): which forecasters are likely to fail in
/// which regime.
///
/// A failure is a forecast scoring worse than a uniform guess (Brier above
/// `1 − 1/k`). The predicted failure probability in a regime is that
/// regime's failure count shrunk toward the forecaster's failure rate across
/// all regimes with two pseudo-observations — so a forecaster with no record
/// in a regime is predicted at its record elsewhere, and one with no record
/// at all at a coin flip, never at zero.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FailurePredictor {
    counts: BTreeMap<String, BTreeMap<String, (u32, u32)>>,
}

impl FailurePredictor {
    fn observe(&mut self, forecaster: &str, regime: &str, failed: bool) {
        let regimes = self.counts.entry(forecaster.to_string()).or_default();
        if !regimes.contains_key(regime) && regimes.len() >= MAX_REGIMES {
            // Drop the regime with the fewest observations; ties by name.
            if let Some(victim) = regimes
                .iter()
                .min_by(|a, b| a.1.1.cmp(&b.1.1).then_with(|| a.0.cmp(b.0)))
                .map(|(name, _)| name.clone())
            {
                regimes.remove(&victim);
            }
        }
        let entry = regimes.entry(regime.to_string()).or_insert((0, 0));
        entry.1 = entry.1.saturating_add(1);
        if failed {
            entry.0 = entry.0.saturating_add(1);
        }
    }

    fn forget(&mut self, forecaster: &str) {
        self.counts.remove(forecaster);
    }

    /// The predicted probability `forecaster` fails a question in `regime`.
    pub fn predict(&self, forecaster: &str, regime: &str) -> f64 {
        let Some(regimes) = self.counts.get(forecaster) else {
            return 0.5;
        };
        let (failed_all, seen_all) = regimes
            .values()
            .fold((0u32, 0u32), |(f, n), (rf, rn)| (f + rf, n + rn));
        let base = if seen_all == 0 {
            0.5
        } else {
            f64::from(failed_all) / f64::from(seen_all)
        };
        let (failed, seen) = regimes.get(regime).copied().unwrap_or((0, 0));
        (f64::from(failed) + 2.0 * base) / (f64::from(seen) + 2.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Forecaster {
    spec: ForecasterSpec,
    standing: Standing,
    admitted_at: Timestamp,
    /// Resolved scores by the regime the question was opened in.
    regimes: BTreeMap<String, VecDeque<ScoredForecast>>,
    /// The real P&L attributed to the positions this forecaster informed
    /// (R12-MODEL-6021), cumulative.
    attributed_pnl: Decimal,
    last_scored: Timestamp,
}

impl Forecaster {
    fn all_scores(&self) -> impl Iterator<Item = &ScoredForecast> {
        self.regimes.values().flatten()
    }

    fn resolved(&self) -> usize {
        self.regimes.values().map(VecDeque::len).sum()
    }

    fn expired(&self, now: Timestamp) -> bool {
        matches!(self.spec.kind, ForecasterKind::Temporary { expires_at } if now >= expires_at)
    }
}

/// The tournament.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tournament {
    forecasters: BTreeMap<String, Forecaster>,
    questions: BTreeMap<String, Question>,
    market: ForecastMarket,
    meta: FailurePredictor,
    research: VecDeque<ResearchTask>,
}

impl Tournament {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.forecasters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.forecasters.is_empty()
    }

    pub fn market(&self) -> &ForecastMarket {
        &self.market
    }

    pub fn meta_model(&self) -> &FailurePredictor {
        &self.meta
    }

    pub fn question(&self, id: &str) -> Option<&Question> {
        self.questions.get(id)
    }

    pub fn open_questions(&self) -> impl Iterator<Item = &Question> {
        self.questions.values()
    }

    pub fn is_admitted(&self, forecaster: &str) -> bool {
        self.forecasters.contains_key(forecaster)
    }

    pub fn standing(&self, forecaster: &str) -> Option<&Standing> {
        self.forecasters.get(forecaster).map(|f| &f.standing)
    }

    pub fn attributed_pnl(&self, forecaster: &str) -> Option<Decimal> {
        self.forecasters.get(forecaster).map(|f| f.attributed_pnl)
    }

    /// Admit a forecaster.
    ///
    /// Refuses a lane its horizon band may not promote to (R12-WORLD-3086,
    /// -3087, -3088), a blank id, a temporary one already expired, and — at
    /// capacity, after expired temporaries are evicted — any newcomer.
    /// Re-admitting a known forecaster is a no-op, so it keeps its record.
    pub fn admit(&mut self, spec: ForecasterSpec, now: Timestamp) -> Result<()> {
        if spec.id.trim().is_empty() {
            return Err(Error::invalid(
                "a forecaster needs an id; an unnamed one cannot hold a record",
            ));
        }
        let lane = spec.horizon.admitted_lane();
        if spec.lane != lane {
            return Err(Error::denied(format!(
                "{} forecasts on the {} horizon, whose output is promoted only to {}; it asked \
                 for {} — a model promoted to the wrong lane breaks that lane's latency or \
                 authority bound",
                spec.id,
                spec.horizon.as_str(),
                lane.as_str(),
                spec.lane.as_str()
            )));
        }
        if let ForecasterKind::Temporary { expires_at } = spec.kind
            && expires_at <= now
        {
            return Err(Error::invalid(format!(
                "{} is a temporary forecaster that expired at {expires_at}; spawn it with an \
                 expiry in the future",
                spec.id
            )));
        }
        if self.forecasters.contains_key(&spec.id) {
            return Ok(());
        }
        if self.forecasters.len() >= MAX_FORECASTERS {
            self.retire_expired(now);
        }
        if self.forecasters.len() >= MAX_FORECASTERS {
            return Err(Error::denied(format!(
                "the tournament holds {MAX_FORECASTERS} forecasters and none has expired; retire \
                 one before admitting {}",
                spec.id
            )));
        }
        self.market.open(&spec.id);
        self.forecasters.insert(
            spec.id.clone(),
            Forecaster {
                spec,
                standing: Standing::Probation,
                admitted_at: now,
                regimes: BTreeMap::new(),
                attributed_pnl: Decimal::ZERO,
                last_scored: now,
            },
        );
        Ok(())
    }

    /// Retire every temporary forecaster past its expiry, returning their ids.
    /// Their open forecasts stay on their questions and are scored; they just
    /// stop holding a record and a balance.
    pub fn retire_expired(&mut self, now: Timestamp) -> Vec<String> {
        let expired: Vec<String> = self
            .forecasters
            .iter()
            .filter(|(_, f)| f.expired(now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.forecasters.remove(id);
            self.market.close(id);
            self.meta.forget(id);
        }
        expired
    }

    /// Open a question. Refuses fewer than two outcomes, a duplicate id, a
    /// resolution not after `now`, a blank regime, and a full book.
    pub fn open(&mut self, spec: QuestionSpec, now: Timestamp) -> Result<()> {
        let distinct: BTreeSet<&String> = spec.outcomes.iter().collect();
        if distinct.len() < 2 || distinct.len() != spec.outcomes.len() {
            return Err(Error::invalid(format!(
                "question {} states {} distinct outcome(s); a question needs at least two, each \
                 once",
                spec.id,
                distinct.len()
            )));
        }
        if spec.regime.trim().is_empty() {
            return Err(Error::invalid(format!(
                "question {} names no regime; every score is filed under the regime in force at \
                 opening, so it must be captured now",
                spec.id
            )));
        }
        if spec.resolves_at <= now {
            return Err(Error::invalid(format!(
                "question {} resolves at {} which is not after {now}; a question that has \
                 already resolved is not a forecast",
                spec.id, spec.resolves_at
            )));
        }
        if let Some(threshold) = spec.tail_threshold
            && !(threshold.is_finite() && threshold > 0.0)
        {
            return Err(Error::invalid(format!(
                "question {} states a tail threshold of {threshold}; give a positive one or none",
                spec.id
            )));
        }
        if self.questions.contains_key(&spec.id) {
            return Err(Error::invalid(format!(
                "question {} is already open",
                spec.id
            )));
        }
        if self.questions.len() >= MAX_OPEN_QUESTIONS {
            return Err(Error::denied(format!(
                "{MAX_OPEN_QUESTIONS} questions are open; resolve or abandon one before \
                 opening {}",
                spec.id
            )));
        }
        self.questions.insert(
            spec.id.clone(),
            Question {
                spec,
                opened_at: now,
                forecasts: BTreeMap::new(),
                held_out: None,
            },
        );
        Ok(())
    }

    /// Abandon a question nobody can resolve, unscored. Returns it so the
    /// caller can put the abandonment on the record.
    pub fn abandon(&mut self, question: &str) -> Option<Question> {
        self.questions.remove(question)
    }

    /// Hold a question out of the market aggregate, with the reason (the
    /// verifier society refuted the prediction it stands behind). Its
    /// forecasts are still scored when it resolves.
    pub fn hold_out(&mut self, question: &str, reason: impl Into<String>) -> Result<()> {
        let open = self
            .questions
            .get_mut(question)
            .ok_or_else(|| Error::not_found(format!("no open question {question}")))?;
        open.held_out = Some(reason.into());
        Ok(())
    }

    /// Enter one forecast.
    ///
    /// Refuses a forecaster not admitted or expired; a question not open; a
    /// forecast issued in the future, or at or after the question resolves
    /// (a forecast written once the answer is knowable is not a forecast);
    /// a target other than the question's; a categorical view over outcomes
    /// other than the question's or not summing to 10,000 bp; a forecast
    /// stating neither probabilities nor quantiles; a second forecast from
    /// the same forecaster; and a negative or non-finite cost.
    pub fn submit(
        &mut self,
        question: &str,
        forecast: TournamentForecast,
        now: Timestamp,
    ) -> Result<()> {
        let Some(forecaster) = self.forecasters.get(&forecast.forecaster) else {
            return Err(Error::denied(format!(
                "{} is not admitted to the tournament; admit it before it forecasts",
                forecast.forecaster
            )));
        };
        if forecaster.expired(now) {
            return Err(Error::denied(format!(
                "{} expired; a retired specialist does not forecast",
                forecast.forecaster
            )));
        }
        let open = self
            .questions
            .get_mut(question)
            .ok_or_else(|| Error::not_found(format!("no open question {question}")))?;
        if forecast.issued_at > now {
            return Err(Error::invalid(format!(
                "{} is issued at {}, after now ({now}); a forecast cannot be dated ahead",
                forecast.forecast_id, forecast.issued_at
            )));
        }
        if forecast.issued_at >= open.spec.resolves_at {
            return Err(Error::denied(format!(
                "{} is issued at {}, at or after question {question} resolves; a forecast \
                 written once the answer is knowable is leakage, not skill",
                forecast.forecast_id, forecast.issued_at
            )));
        }
        if forecast.target != open.spec.target {
            return Err(Error::invalid(format!(
                "{} forecasts {} and question {question} asks about {}",
                forecast.forecast_id, forecast.target, open.spec.target
            )));
        }
        if forecast.probabilities_bp.is_none() && forecast.quantiles.is_none() {
            return Err(Error::invalid(format!(
                "{} states neither probabilities nor quantiles; there is nothing to score",
                forecast.forecast_id
            )));
        }
        if let Some(probabilities) = &forecast.probabilities_bp {
            let stated: BTreeSet<&String> = probabilities.keys().collect();
            let asked: BTreeSet<&String> = open.spec.outcomes.iter().collect();
            if stated != asked {
                return Err(Error::invalid(format!(
                    "{} states outcomes {:?} and question {question} has {:?}",
                    forecast.forecast_id, stated, asked
                )));
            }
            let total: u64 = probabilities.values().map(|bp| u64::from(*bp)).sum();
            if total != 10_000 {
                return Err(Error::invalid(format!(
                    "{} states probabilities summing to {total} bp, not 10000",
                    forecast.forecast_id
                )));
            }
        }
        if let Some(quantiles) = &forecast.quantiles {
            let ordered = quantiles.windows(2).all(|pair| match pair {
                [a, b] => a.0 < b.0 && a.1 <= b.1,
                _ => true,
            });
            let inside = quantiles
                .iter()
                .all(|(q, v)| q.is_finite() && *q > 0.0 && *q < 1.0 && v.is_finite());
            if quantiles.is_empty() || !ordered || !inside {
                return Err(Error::invalid(format!(
                    "{} states quantiles that are empty, outside (0, 1) or not increasing",
                    forecast.forecast_id
                )));
            }
        }
        if !(forecast.cost.is_finite() && forecast.cost >= 0.0) {
            return Err(Error::invalid(format!(
                "{} states a cost of {}; give a non-negative one",
                forecast.forecast_id, forecast.cost
            )));
        }
        if open.forecasts.contains_key(&forecast.forecaster) {
            return Err(Error::invalid(format!(
                "{} has already forecast question {question}; one view per forecaster per \
                 question, or the tournament scores a revision as a second opinion",
                forecast.forecaster
            )));
        }
        open.forecasts.insert(forecast.forecaster.clone(), forecast);
        Ok(())
    }

    /// Whether the forecasts on `question` disagree materially, and if so
    /// the research task it raises, which is also queued for
    /// [`Self::drain_research`].
    ///
    /// The widest spread on any outcome across forecasters stating
    /// probabilities; material at [`DISAGREEMENT_BAR`]. The task cites every
    /// forecast — the disagreement is the finding, and averaging it away
    /// first would throw away exactly what the platform does not know.
    pub fn review_disagreement(
        &mut self,
        question: &str,
        now: Timestamp,
    ) -> Result<Option<ResearchTask>> {
        let open = self
            .questions
            .get(question)
            .ok_or_else(|| Error::not_found(format!("no open question {question}")))?;
        let mut widest: Option<(f64, &String)> = None;
        for outcome in &open.spec.outcomes {
            let stated: Vec<f64> = open
                .forecasts
                .values()
                .filter_map(|f| f.probability(outcome))
                .collect();
            if stated.len() < 2 {
                continue;
            }
            let high = stated.iter().copied().fold(f64::MIN, f64::max);
            let low = stated.iter().copied().fold(f64::MAX, f64::min);
            let spread = high - low;
            if widest.is_none_or(|(best, _)| spread > best) {
                widest = Some((spread, outcome));
            }
        }
        let Some((spread, outcome)) = widest else {
            return Ok(None);
        };
        if spread < DISAGREEMENT_BAR {
            return Ok(None);
        }
        let task = ResearchTask {
            question: question.to_string(),
            target: open.spec.target.clone(),
            spread,
            outcome: outcome.clone(),
            cites: open
                .forecasts
                .values()
                .filter_map(|f| f.probability(outcome).map(|p| (f.forecast_id.clone(), p)))
                .collect(),
            raised_at: now,
        };
        if self.research.len() >= MAX_RESEARCH_TASKS {
            self.research.pop_front();
        }
        self.research.push_back(task.clone());
        Ok(Some(task))
    }

    /// Take every research task raised since the last drain.
    pub fn drain_research(&mut self) -> Vec<ResearchTask> {
        self.research.drain(..).collect()
    }

    /// The aggregate probability per outcome on `question`, weighted by
    /// [`Self::influence`] in `regime`.
    ///
    /// Forecasters stating the *same* distribution are one signal: their
    /// cluster carries the largest of its members' weights, not the sum, so
    /// copies of a model buy no extra say and a duplicate adds no
    /// information (R12-MODEL-3093, -6020). Where no forecaster on the
    /// question has been promoted, every forecaster weighs equally — stated,
    /// because nobody has yet earned more. `None` where nobody stated
    /// probabilities.
    pub fn aggregate(&self, question: &str, regime: &str) -> Result<Option<BTreeMap<String, f64>>> {
        let open = self
            .questions
            .get(question)
            .ok_or_else(|| Error::not_found(format!("no open question {question}")))?;
        let weights = self.question_weights(open, regime, None);
        Ok(Self::pooled(open, &weights))
    }

    fn question_weights(
        &self,
        open: &Question,
        regime: &str,
        without: Option<&str>,
    ) -> BTreeMap<String, f64> {
        let submitted: Vec<&str> = open
            .forecasts
            .values()
            .filter(|f| f.probabilities_bp.is_some())
            .map(|f| f.forecaster.as_str())
            .filter(|name| Some(*name) != without)
            .collect();
        let influence = self.influence_among(submitted.iter().copied(), regime);
        let earned: f64 = influence.values().map(|w| w.weight).sum();
        if earned > 0.0 {
            submitted
                .iter()
                .map(|name| {
                    (
                        (*name).to_string(),
                        influence.get(*name).map_or(0.0, |w| w.weight),
                    )
                })
                .collect()
        } else {
            submitted
                .iter()
                .map(|name| ((*name).to_string(), 1.0))
                .collect()
        }
    }

    fn pooled(open: &Question, weights: &BTreeMap<String, f64>) -> Option<BTreeMap<String, f64>> {
        // Cluster identical stated distributions.
        let mut clusters: BTreeMap<Vec<(String, u32)>, f64> = BTreeMap::new();
        for (name, weight) in weights {
            let Some(forecast) = open.forecasts.get(name) else {
                continue;
            };
            let Some(probabilities) = &forecast.probabilities_bp else {
                continue;
            };
            let key: Vec<(String, u32)> = probabilities
                .iter()
                .map(|(o, bp)| (o.clone(), *bp))
                .collect();
            let entry = clusters.entry(key).or_insert(0.0);
            *entry = entry.max(*weight);
        }
        let total: f64 = clusters.values().sum();
        if clusters.is_empty() || total <= 0.0 {
            return None;
        }
        let mut pooled: BTreeMap<String, f64> = open
            .spec
            .outcomes
            .iter()
            .map(|o| (o.clone(), 0.0))
            .collect();
        for (distribution, weight) in &clusters {
            for (outcome, bp) in distribution {
                if let Some(p) = pooled.get_mut(outcome) {
                    *p += weight * f64::from(*bp) / 10_000.0 / total;
                }
            }
        }
        Some(pooled)
    }

    /// Resolve a question: score every forecast on it, settle the market,
    /// teach the meta-model, and close the question.
    ///
    /// Refuses before the question's resolution instant — a forecast scored
    /// on the part of the path that happened to suit it is a track record
    /// manufactured — and an outcome the question did not list.
    pub fn resolve(
        &mut self,
        question: &str,
        resolution: Resolution,
        now: Timestamp,
    ) -> Result<ResolvedQuestion> {
        let open = self
            .questions
            .get(question)
            .ok_or_else(|| Error::not_found(format!("no open question {question}")))?;
        if now < open.spec.resolves_at || resolution.observed_at < open.spec.resolves_at {
            return Err(Error::denied(format!(
                "question {question} resolves at {}; it cannot be scored at {now} on an \
                 observation from {}",
                open.spec.resolves_at, resolution.observed_at
            )));
        }
        if !open.spec.outcomes.contains(&resolution.outcome) {
            return Err(Error::invalid(format!(
                "question {question} has no outcome {:?}",
                resolution.outcome
            )));
        }
        let regime = open.spec.regime.clone();
        let outcome = resolution.outcome.clone();
        let k = open.spec.outcomes.len() as f64;
        let tail = match (open.spec.tail_threshold, resolution.realised_value) {
            (Some(threshold), Some(value)) if value.is_finite() => Some(value.abs() >= threshold),
            _ => None,
        };
        let weights = self.question_weights(open, &regime, None);
        let aggregate = Self::pooled(open, &weights);
        let aggregate_outcome = aggregate
            .as_ref()
            .and_then(|p| p.get(&outcome).copied())
            .map(|p| p.max(PROBABILITY_FLOOR));

        let mut scores = Vec::with_capacity(open.forecasts.len());
        for forecast in open.forecasts.values() {
            let mut calibration_points = Vec::new();
            let (brier, log_loss, hit) = match &forecast.probabilities_bp {
                Some(probabilities) => {
                    let mut brier = 0.0;
                    for (o, bp) in probabilities {
                        let p = f64::from(*bp) / 10_000.0;
                        let happened = *o == outcome;
                        brier += (p - f64::from(u8::from(happened))).powi(2);
                        calibration_points.push((p, happened));
                    }
                    let p_outcome = forecast.probability(&outcome).unwrap_or(0.0);
                    let top = probabilities
                        .iter()
                        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                        .map(|(o, _)| o.clone());
                    (
                        Some(brier),
                        Some(-p_outcome.max(PROBABILITY_FLOOR).ln()),
                        top.map(|o| o == outcome),
                    )
                }
                None => (None, None, None),
            };
            let pinball = match (&forecast.quantiles, resolution.realised_value) {
                (Some(quantiles), Some(value)) if value.is_finite() => Some(
                    quantiles
                        .iter()
                        .map(|(q, predicted)| {
                            let error = value - predicted;
                            (q * error).max((q - 1.0) * error)
                        })
                        .sum::<f64>()
                        / quantiles.len() as f64,
                ),
                _ => None,
            };
            let p_outcome = forecast.probability(&outcome);
            let economic = match (p_outcome, resolution.realised_value) {
                (Some(p), Some(value)) if value.is_finite() => Some((p - 1.0 / k) * value.abs()),
                _ => None,
            };
            // Leave-one-out: the log-score gain of the aggregate with this
            // forecaster over the aggregate without it. Alone on the
            // question, it is measured against a uniform guess.
            let information_value = match (p_outcome, aggregate_outcome) {
                (Some(_), Some(with)) => {
                    let without_weights =
                        self.question_weights(open, &regime, Some(&forecast.forecaster));
                    let without = Self::pooled(open, &without_weights)
                        .and_then(|p| p.get(&outcome).copied())
                        .unwrap_or(1.0 / k)
                        .max(PROBABILITY_FLOOR);
                    Some(with.ln() - without.ln())
                }
                _ => None,
            };
            scores.push(ScoredForecast {
                forecaster: forecast.forecaster.clone(),
                forecast_id: forecast.forecast_id.clone(),
                question: question.to_string(),
                regime: regime.clone(),
                outcome: outcome.clone(),
                brier,
                log_loss,
                hit,
                tail,
                pinball,
                economic,
                information_value,
                calibration_points,
                cost: forecast.cost,
                latency_ms: forecast.latency.as_millis() as f64,
            });
        }

        // Settle the market against the aggregate, among forecasters that
        // still hold an account (a retired temporary does not).
        let held_out = open.held_out.clone();
        let settlements = match (aggregate_outcome, &held_out) {
            (Some(aggregate), None) => {
                let participants: Vec<(String, f64)> = open
                    .forecasts
                    .values()
                    .filter(|f| self.market.balance(&f.forecaster).is_some())
                    .filter_map(|f| {
                        f.probability(&outcome)
                            .map(|p| (f.forecaster.clone(), p.max(PROBABILITY_FLOOR)))
                    })
                    .collect();
                self.market.settle(&participants, aggregate)?
            }
            _ => Vec::new(),
        };

        // Teach the meta-model and file each score under the opening regime.
        let failure_bar = 1.0 - 1.0 / k;
        for score in &scores {
            if let Some(brier) = score.brier {
                self.meta
                    .observe(&score.forecaster, &regime, brier > failure_bar);
            }
            if let Some(forecaster) = self.forecasters.get_mut(&score.forecaster) {
                if !forecaster.regimes.contains_key(&regime)
                    && forecaster.regimes.len() >= MAX_REGIMES
                    && let Some(victim) = forecaster
                        .regimes
                        .iter()
                        .min_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.0.cmp(b.0)))
                        .map(|(name, _)| name.clone())
                {
                    forecaster.regimes.remove(&victim);
                }
                let window = forecaster.regimes.entry(regime.clone()).or_default();
                if window.len() >= SCORE_WINDOW {
                    window.pop_front();
                }
                window.push_back(score.clone());
                forecaster.last_scored = now;
            }
        }
        self.questions.remove(question);
        Ok(ResolvedQuestion {
            question: question.to_string(),
            regime,
            outcome,
            aggregate: aggregate_outcome,
            scores,
            settlements,
            held_out,
        })
    }

    /// Record the real P&L attributed to the positions `forecaster` informed
    /// (R12-MODEL-6021). `cumulative` is the attribution's running figure,
    /// replacing the last one — the attribution is recomputed whole each
    /// cycle, so adding would count every fill once per cycle.
    pub fn credit_attribution(&mut self, forecaster: &str, cumulative: Decimal) -> Result<()> {
        let entry = self.forecasters.get_mut(forecaster).ok_or_else(|| {
            Error::not_found(format!(
                "{forecaster} is not in the tournament; nothing to credit"
            ))
        })?;
        entry.attributed_pnl = cumulative;
        Ok(())
    }

    /// Every regime's scorecard for `forecaster`, in regime order.
    pub fn scorecards(&self, forecaster: &str) -> Vec<Scorecard> {
        let Some(entry) = self.forecasters.get(forecaster) else {
            return Vec::new();
        };
        let regime_briers: BTreeMap<&String, f64> = entry
            .regimes
            .iter()
            .filter_map(|(regime, scores)| {
                let briers: Vec<f64> = scores.iter().filter_map(|s| s.brier).collect();
                Measure::mean(&briers, "").value().map(|b| (regime, b))
            })
            .collect();
        let cross_regime = if regime_briers.len() >= 2 {
            Some(regime_briers.values().sum::<f64>() / regime_briers.len() as f64)
        } else {
            None
        };
        entry
            .regimes
            .iter()
            .map(|(regime, scores)| {
                Self::scorecard(
                    forecaster,
                    regime,
                    scores,
                    cross_regime,
                    regime_briers.get(regime).copied(),
                )
            })
            .collect()
    }

    fn scorecard(
        forecaster: &str,
        regime: &str,
        scores: &VecDeque<ScoredForecast>,
        cross_regime: Option<f64>,
        this_regime: Option<f64>,
    ) -> Scorecard {
        let collect = |f: fn(&ScoredForecast) -> Option<f64>| -> Vec<f64> {
            scores.iter().filter_map(f).collect()
        };
        let points: Vec<PredictionOutcome> = scores
            .iter()
            .flat_map(|s| s.calibration_points.iter())
            .map(|(p, happened)| PredictionOutcome::new(*p, *happened))
            .collect();
        let calibration_error = if points.is_empty() {
            Measure::Unmeasured {
                reason: "no forecast here stated probabilities".to_string(),
            }
        } else {
            Measure::Measured {
                value: Calibration::assess(&points, CALIBRATION_BINS).expected_calibration_error,
            }
        };
        let tail: Vec<f64> = scores
            .iter()
            .filter(|s| s.tail == Some(true))
            .filter_map(|s| s.brier)
            .collect();
        let hits: Vec<f64> = scores
            .iter()
            .filter_map(|s| s.hit.map(|h| f64::from(u8::from(h))))
            .collect();
        let robustness = match (cross_regime, this_regime) {
            (Some(all), Some(here)) => Measure::Measured { value: all - here },
            _ => Measure::Unmeasured {
                reason: "scored in fewer than two regimes; robustness compares regimes".to_string(),
            },
        };
        Scorecard {
            forecaster: forecaster.to_string(),
            regime: regime.to_string(),
            resolved: scores.len(),
            brier: Measure::mean(
                &collect(|s| s.brier),
                "no forecast here stated probabilities",
            ),
            log_loss: Measure::mean(
                &collect(|s| s.log_loss),
                "no forecast here stated probabilities",
            ),
            calibration_error,
            tail_brier: Measure::mean(&tail, "no question here resolved on a tail move"),
            economic_usefulness: Measure::mean(
                &collect(|s| s.economic),
                "no question here measured the realised move",
            ),
            robustness,
            directional_accuracy: Measure::mean(&hits, "no forecast here stated probabilities"),
            quantile_loss: Measure::mean(
                &collect(|s| s.pinball),
                "no forecast here stated quantiles",
            ),
            information_value: Measure::mean(
                &collect(|s| s.information_value),
                "no aggregate existed to measure against",
            ),
            mean_cost: Measure::mean(&scores.iter().map(|s| s.cost).collect::<Vec<_>>(), "")
                .value()
                .unwrap_or(0.0),
            mean_latency_ms: Measure::mean(
                &scores.iter().map(|s| s.latency_ms).collect::<Vec<_>>(),
                "",
            )
            .value()
            .unwrap_or(0.0),
        }
    }

    /// Influence over the aggregate for every promoted forecaster, in
    /// `regime` (R12-MODEL-3090, -6022). See [`Self::influence_among`].
    pub fn influence(&self, regime: &str) -> BTreeMap<String, InfluenceWeight> {
        let names: Vec<&str> = self.forecasters.keys().map(String::as_str).collect();
        self.influence_among(names, regime)
    }

    /// Influence among `names`, counting only [`Standing::Active`] ones.
    ///
    /// Each factor is in `(0, 1]`:
    ///
    /// * **market share** — the forecaster's synthetic-credit balance over
    ///   the largest balance among those weighed;
    /// * **calibration** — `1 − ECE` over its whole record;
    /// * **information** — [`information_factor`] of the mean leave-one-out
    ///   information value: `r / (1 + r)` where `r = exp(mean value)` is the
    ///   geometric-mean ratio of the probability the aggregate *with* the
    ///   forecaster put on what happened to the probability the aggregate
    ///   *without* it did. A forecaster that adds nothing the others did not
    ///   already say reads one half, one that adds information approaches
    ///   one, one that subtracts it approaches zero;
    /// * **cost** and **latency** — `m / (m + x)` where `x` is its mean and
    ///   `m` the median across those weighed, so the median forecaster reads
    ///   one half and the cheapest approach one;
    /// * **regime survival** — one minus the meta-model's predicted failure
    ///   probability in `regime`.
    ///
    /// The weight is the product, normalised to sum to one.
    pub fn influence_among<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
        regime: &str,
    ) -> BTreeMap<String, InfluenceWeight> {
        let active: Vec<&Forecaster> = names
            .into_iter()
            .filter_map(|name| self.forecasters.get(name))
            .filter(|f| f.standing == Standing::Active)
            .collect();
        if active.is_empty() {
            return BTreeMap::new();
        }
        let largest = active
            .iter()
            .filter_map(|f| self.market.balance(&f.spec.id))
            .map(SyntheticCredits::as_f64)
            .fold(0.0, f64::max);
        let mean = |f: &Forecaster, field: fn(&ScoredForecast) -> f64| -> f64 {
            let values: Vec<f64> = f.all_scores().map(field).collect();
            Measure::mean(&values, "").value().unwrap_or(0.0)
        };
        let median = |values: &mut Vec<f64>| -> f64 {
            values.sort_by(f64::total_cmp);
            values.get(values.len() / 2).copied().unwrap_or(0.0)
        };
        let mut costs: Vec<f64> = active.iter().map(|f| mean(f, |s| s.cost)).collect();
        let mut latencies: Vec<f64> = active.iter().map(|f| mean(f, |s| s.latency_ms)).collect();
        let cost_median = median(&mut costs);
        let latency_median = median(&mut latencies);
        let relative = |x: f64, m: f64| -> f64 {
            if m <= 0.0 {
                if x <= 0.0 { 1.0 } else { 0.5 }
            } else {
                m / (m + x)
            }
        };
        let mut raw: Vec<InfluenceWeight> = active
            .iter()
            .map(|f| {
                let balance = self
                    .market
                    .balance(&f.spec.id)
                    .map_or(0.0, SyntheticCredits::as_f64);
                let market_share = if largest > 0.0 {
                    balance / largest
                } else {
                    0.0
                };
                let points: Vec<PredictionOutcome> = f
                    .all_scores()
                    .flat_map(|s| s.calibration_points.iter())
                    .map(|(p, happened)| PredictionOutcome::new(*p, *happened))
                    .collect();
                let calibration = if points.is_empty() {
                    1.0
                } else {
                    (1.0 - Calibration::assess(&points, CALIBRATION_BINS)
                        .expected_calibration_error)
                        .max(0.0)
                };
                let information_values: Vec<f64> =
                    f.all_scores().filter_map(|s| s.information_value).collect();
                let information = Measure::mean(&information_values, "")
                    .value()
                    .map_or(1.0, information_factor);
                InfluenceWeight {
                    forecaster: f.spec.id.clone(),
                    market_share,
                    calibration,
                    information,
                    cost: relative(mean(f, |s| s.cost), cost_median),
                    latency: relative(mean(f, |s| s.latency_ms), latency_median),
                    regime_survival: 1.0 - self.meta.predict(&f.spec.id, regime),
                    weight: 0.0,
                }
            })
            .collect();
        for w in &mut raw {
            w.weight = w.market_share
                * w.calibration
                * w.information
                * w.cost
                * w.latency
                * w.regime_survival;
        }
        let total: f64 = raw.iter().map(|w| w.weight).sum();
        raw.into_iter()
            .map(|mut w| {
                w.weight = if total > 0.0 { w.weight / total } else { 0.0 };
                (w.forecaster.clone(), w)
            })
            .collect()
    }

    /// Split `budget` compute units across the promoted forecasters by
    /// influence in `regime` (R12-MODEL-6022). A forecaster on probation or
    /// blocked receives nothing.
    pub fn compute_allocation(&self, regime: &str, budget: u64) -> Result<BTreeMap<String, u64>> {
        let weights: BTreeMap<String, f64> = self
            .influence(regime)
            .into_iter()
            .map(|(name, w)| (name, w.weight))
            .collect();
        allocate(&weights, budget)
    }

    /// Forecasters on probation with enough resolved forecasts to be
    /// considered for promotion — the ones the verifier society should
    /// attack before [`Self::review_promotions`] runs.
    pub fn promotion_candidates(&self) -> Vec<String> {
        self.forecasters
            .iter()
            .filter(|(_, f)| {
                !matches!(f.standing, Standing::Active) && f.resolved() >= PROMOTION_SAMPLE
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// When `forecaster` was last scored, if it is admitted.
    pub fn last_scored(&self, forecaster: &str) -> Option<Timestamp> {
        self.forecasters.get(forecaster).map(|f| f.last_scored)
    }

    /// Mean Brier across `forecaster`'s whole record, where it stated
    /// probabilities.
    pub fn mean_brier(&self, forecaster: &str) -> Option<f64> {
        let entry = self.forecasters.get(forecaster)?;
        let briers: Vec<f64> = entry.all_scores().filter_map(|s| s.brier).collect();
        Measure::mean(&briers, "").value()
    }

    /// Directional hit rate and sample per realised outcome, for the
    /// verifier society's bias critics.
    pub fn hit_rates_by_outcome(&self, forecaster: &str) -> BTreeMap<String, (f64, usize)> {
        let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        if let Some(entry) = self.forecasters.get(forecaster) {
            for score in entry.all_scores() {
                if let Some(hit) = score.hit {
                    let row = tally.entry(score.outcome.clone()).or_insert((0, 0));
                    row.0 += usize::from(hit);
                    row.1 += 1;
                }
            }
        }
        tally
            .into_iter()
            .map(|(outcome, (hits, n))| (outcome, (hits as f64 / n.max(1) as f64, n)))
            .collect()
    }

    /// Promote every forecaster on probation that has earned it, and block
    /// every forecaster the verifier society refuted.
    ///
    /// Promotion needs [`PROMOTION_SAMPLE`] resolved forecasts and a mean
    /// Brier better than a uniform guess over two outcomes; a refutation in
    /// `refuted` (forecaster → reason) blocks promotion and demotes an active
    /// forecaster, and a blocked forecaster absent from `refuted` returns to
    /// probation to earn its way back.
    pub fn review_promotions(
        &mut self,
        refuted: &BTreeMap<String, String>,
    ) -> Vec<PromotionDecision> {
        let mut decisions = Vec::new();
        for (id, forecaster) in &mut self.forecasters {
            let lane = forecaster.spec.lane;
            if let Some(reason) = refuted.get(id) {
                if forecaster.standing
                    != (Standing::Blocked {
                        reason: reason.clone(),
                    })
                {
                    forecaster.standing = Standing::Blocked {
                        reason: reason.clone(),
                    };
                    decisions.push(PromotionDecision {
                        forecaster: id.clone(),
                        lane,
                        promoted: false,
                        reason: format!("blocked by the verifier society: {reason}"),
                    });
                }
                continue;
            }
            match &forecaster.standing {
                Standing::Blocked { .. } => {
                    forecaster.standing = Standing::Probation;
                }
                Standing::Active => continue,
                Standing::Probation => {}
            }
            let resolved = forecaster.resolved();
            if resolved < PROMOTION_SAMPLE {
                continue;
            }
            let briers: Vec<f64> = forecaster.all_scores().filter_map(|s| s.brier).collect();
            let Some(brier) = Measure::mean(&briers, "").value() else {
                continue;
            };
            if brier < 0.5 {
                forecaster.standing = Standing::Active;
                decisions.push(PromotionDecision {
                    forecaster: id.clone(),
                    lane,
                    promoted: true,
                    reason: format!(
                        "{resolved} resolved forecasts at mean Brier {brier:.4}, better than a \
                         coin flip"
                    ),
                });
            }
        }
        decisions
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;

    fn at(day: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_days(day))
    }

    fn spec(id: &str) -> ForecasterSpec {
        ForecasterSpec {
            id: id.to_string(),
            kind: ForecasterKind::Specialist,
            horizon: LatticeHorizon::HoursDays,
            lane: PromotionLane::StrategyCapitalRisk,
        }
    }

    fn question(id: &str, regime: &str, opened: i64) -> QuestionSpec {
        QuestionSpec {
            id: id.to_string(),
            target: "AAA/direction".to_string(),
            outcomes: vec!["not_up".to_string(), "up".to_string()],
            regime: regime.to_string(),
            resolves_at: at(opened + 1),
            tail_threshold: Some(500.0),
        }
    }

    fn forecast(forecaster: &str, question: &str, up_bp: u32, issued: i64) -> TournamentForecast {
        TournamentForecast {
            forecaster: forecaster.to_string(),
            forecast_id: format!("{question}/{forecaster}"),
            target: "AAA/direction".to_string(),
            issued_at: at(issued),
            probabilities_bp: Some(BTreeMap::from([
                ("up".to_string(), up_bp),
                ("not_up".to_string(), 10_000 - up_bp),
            ])),
            quantiles: None,
            cost: 1.0,
            latency: Duration::from_millis(10),
        }
    }

    fn resolution(outcome: &str, move_bps: f64, day: i64) -> Resolution {
        Resolution {
            outcome: outcome.to_string(),
            realised_value: Some(move_bps),
            observed_at: at(day),
        }
    }

    /// Run `rounds` questions in `regime`, with each forecaster's up
    /// probability, all resolving to `outcome`.
    fn rounds(
        t: &mut Tournament,
        regime: &str,
        start: i64,
        rounds: i64,
        views: &[(&str, u32)],
        outcome: &str,
        move_bps: f64,
    ) -> Result<()> {
        for i in 0..rounds {
            let day = start + 2 * i;
            let id = format!("{regime}-{day}");
            t.open(question(&id, regime, day), at(day))?;
            for (name, up) in views {
                t.submit(&id, forecast(name, &id, *up, day), at(day))?;
            }
            t.resolve(&id, resolution(outcome, move_bps, day + 1), at(day + 1))?;
        }
        Ok(())
    }

    /// R12-MODEL-3089 / -7198's defining property: many independent
    /// forecasters on one question, each scored separately. Mutation:
    /// collapsing `submit` to keep one forecast per question fails the count.
    #[test]
    fn a_thousand_forecasters_forecast_one_question_and_each_is_scored() -> Result<()> {
        let mut t = Tournament::new();
        t.open(question("q", "trending", 0), at(0))?;
        for i in 0..1_000u32 {
            let name = format!("specialist-{i:04}");
            t.admit(spec(&name), at(0))?;
            t.submit("q", forecast(&name, "q", 1 + i * 9, 0), at(0))?;
        }
        assert_eq!(t.question("q").map(|q| q.forecasts().len()), Some(1_000));
        let resolved = t.resolve("q", resolution("up", 120.0, 1), at(1))?;
        assert_eq!(resolved.scores.len(), 1_000, "a forecast went unscored");
        assert_eq!(
            resolved.settlements.len(),
            1_000,
            "a forecaster went unsettled"
        );
        Ok(())
    }

    /// R12-WORLD-3086/-3087/-3088. Mutation: deleting the lane check in
    /// `admit` admits all three mismatches.
    #[test]
    fn a_forecaster_asking_for_a_lane_its_horizon_may_not_reach_is_refused_at_admission() {
        let mut t = Tournament::new();
        for (horizon, wrong) in [
            (LatticeHorizon::HoursDays, PromotionLane::PortfolioTreasury),
            (
                LatticeHorizon::WeeksMonths,
                PromotionLane::StrategyCapitalRisk,
            ),
            (
                LatticeHorizon::QuartersYears,
                PromotionLane::PortfolioTreasury,
            ),
        ] {
            let refused = t.admit(
                ForecasterSpec {
                    id: format!("m-{}", horizon.as_str()),
                    kind: ForecasterKind::Specialist,
                    horizon,
                    lane: wrong,
                },
                at(0),
            );
            assert!(
                refused.is_err(),
                "{} admitted to {}",
                horizon.as_str(),
                wrong.as_str()
            );
            assert!(
                t.admit(
                    ForecasterSpec {
                        id: format!("ok-{}", horizon.as_str()),
                        kind: ForecasterKind::Specialist,
                        horizon,
                        lane: horizon.admitted_lane(),
                    },
                    at(0),
                )
                .is_ok(),
                "the band's own lane was refused"
            );
        }
        assert_eq!(
            LatticeHorizon::of(Duration::from_days(3)),
            LatticeHorizon::HoursDays
        );
        assert_eq!(
            LatticeHorizon::of(Duration::from_days(60)),
            LatticeHorizon::WeeksMonths
        );
        assert_eq!(
            LatticeHorizon::of(Duration::from_days(400)),
            LatticeHorizon::QuartersYears
        );
    }

    /// R12-MODEL-3092: all six §9.8 metrics, per model, per regime; a
    /// scorecard missing one fails to deserialise. Mutation: removing any
    /// field from `Scorecard` fails the key assertion, and removing the
    /// regime keying merges the two rows.
    #[test]
    fn the_scoreboard_carries_six_metrics_per_model_per_regime() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("a"), at(0))?;
        t.admit(spec("b"), at(0))?;
        rounds(
            &mut t,
            "trending",
            0,
            3,
            &[("a", 8_000), ("b", 3_000)],
            "up",
            600.0,
        )?;
        rounds(
            &mut t,
            "crisis",
            10,
            3,
            &[("a", 8_000), ("b", 3_000)],
            "not_up",
            -50.0,
        )?;
        let cards = t.scorecards("a");
        assert_eq!(cards.len(), 2, "one row per regime");
        let value = serde_json::to_value(&cards[0]).map_err(|e| Error::invalid(e.to_string()))?;
        for key in [
            "brier",
            "log_loss",
            "calibration_error",
            "tail_brier",
            "economic_usefulness",
            "robustness",
        ] {
            assert!(value.get(key).is_some(), "the scorecard has no {key}");
        }
        let mut stripped = value.clone();
        if let Some(map) = stripped.as_object_mut() {
            map.remove("robustness");
        }
        assert!(
            serde_json::from_value::<Scorecard>(stripped).is_err(),
            "a scorecard without robustness was read as complete"
        );
        let crisis = cards
            .iter()
            .find(|c| c.regime == "crisis")
            .map(|c| c.brier.clone());
        let trending = cards
            .iter()
            .find(|c| c.regime == "trending")
            .map(|c| c.brier.clone());
        assert_ne!(crisis, trending, "the two regimes were scored as one");
        Ok(())
    }

    /// R12-MODEL-6019. Mutation: computing `tail_brier` over every score
    /// instead of the tail ones makes both forecasters equal.
    #[test]
    fn a_forecaster_that_misses_only_the_tails_scores_worse_on_the_tail_metric() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("calm"), at(0))?;
        t.admit(spec("steady"), at(0))?;
        // Ordinary moves: both right.
        rounds(
            &mut t,
            "quiet",
            0,
            4,
            &[("calm", 8_000), ("steady", 8_000)],
            "up",
            40.0,
        )?;
        // Tail moves (≥ 500 bp): calm misses, steady does not.
        rounds(
            &mut t,
            "quiet",
            20,
            2,
            &[("calm", 1_000), ("steady", 9_000)],
            "up",
            900.0,
        )?;
        let tail = |name: &str| {
            t.scorecards(name)
                .first()
                .and_then(|c| c.tail_brier.value())
                .unwrap_or(f64::NAN)
        };
        assert!(
            tail("calm").is_finite() && tail("steady").is_finite(),
            "premise: tails were scored"
        );
        assert!(
            tail("calm") > tail("steady"),
            "missing the tails cost nothing on the tail metric"
        );
        Ok(())
    }

    /// R12-MODEL-6018. Mutation: dropping the pinball arm leaves
    /// `quantile_loss` unmeasured.
    #[test]
    fn quantile_and_directional_accuracy_are_scored_per_forecaster() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("q"), at(0))?;
        t.open(question("x", "quiet", 0), at(0))?;
        let mut f = forecast("q", "x", 7_000, 0);
        f.quantiles = Some(vec![(0.1, -100.0), (0.5, 50.0), (0.9, 200.0)]);
        t.submit("x", f, at(0))?;
        t.resolve("x", resolution("up", 50.0, 1), at(1))?;
        let card = t.scorecards("q").remove(0);
        let loss = card.quantile_loss.value().unwrap_or(f64::NAN);
        // Pinball at 50: q0.1 → 0.1×150, q0.5 → 0, q0.9 → 0.1×150 → mean 10.
        assert!((loss - 10.0).abs() < 1e-9, "pinball loss {loss}");
        assert_eq!(card.directional_accuracy.value(), Some(1.0));
        Ok(())
    }

    /// R12-MODEL-6020. Mutation: summing cluster weights instead of taking
    /// the largest makes the duplicate move the aggregate and earn value.
    #[test]
    fn a_forecaster_duplicating_another_adds_no_information() -> Result<()> {
        let mut t = Tournament::new();
        for name in ["original", "copy", "other"] {
            t.admit(spec(name), at(0))?;
        }
        t.open(question("d", "quiet", 0), at(0))?;
        t.submit("d", forecast("original", "d", 8_000, 0), at(0))?;
        t.submit("d", forecast("copy", "d", 8_000, 0), at(0))?;
        t.submit("d", forecast("other", "d", 4_000, 0), at(0))?;
        let resolved = t.resolve("d", resolution("up", 100.0, 1), at(1))?;
        let value = |name: &str| {
            resolved
                .scores
                .iter()
                .find(|s| s.forecaster == name)
                .and_then(|s| s.information_value)
                .unwrap_or(f64::NAN)
        };
        assert!(
            value("copy").abs() < 1e-12,
            "a duplicate added {}",
            value("copy")
        );
        assert!(
            value("other").abs() > 1e-6,
            "premise: a distinct view moves the aggregate"
        );
        Ok(())
    }

    /// Mark every forecaster active, so influence weighs each on the record
    /// it has rather than on whether review happened to promote it.
    fn activate_all(t: &mut Tournament) {
        for f in t.forecasters.values_mut() {
            f.standing = Standing::Active;
        }
    }

    fn mean_information(t: &Tournament, name: &str) -> f64 {
        t.forecasters.get(name).map_or(f64::NAN, |f| {
            let values: Vec<f64> = f.all_scores().filter_map(|s| s.information_value).collect();
            Measure::mean(&values, "").value().unwrap_or(f64::NAN)
        })
    }

    /// R12-MODEL-3090. The failure prevented: an exact copy of a forecaster
    /// carries none of its own information, yet under `exp(min(value, 0))`
    /// each copy kept the full information factor of one — the same factor
    /// the forecaster earned alone — so two copies drew two full shares of
    /// influence and compute for one signal. Mutation: restoring
    /// `iv.min(0.0).exp()` reads one for the twins and for the solo, and the
    /// strict comparison fails.
    #[test]
    fn two_identical_forecasters_each_lose_influence_for_carrying_no_incremental_information()
    -> Result<()> {
        let mut alone = Tournament::new();
        alone.admit(spec("solo"), at(0))?;
        rounds(&mut alone, "quiet", 0, 12, &[("solo", 8_000)], "up", 60.0)?;
        activate_all(&mut alone);

        let mut paired = Tournament::new();
        paired.admit(spec("twin-a"), at(0))?;
        paired.admit(spec("twin-b"), at(0))?;
        rounds(
            &mut paired,
            "quiet",
            0,
            12,
            &[("twin-a", 8_000), ("twin-b", 8_000)],
            "up",
            60.0,
        )?;
        activate_all(&mut paired);

        // Premise: alone, the forecaster beats a uniform guess; as a pair,
        // each twin is exactly redundant with the other.
        let solo_value = mean_information(&alone, "solo");
        assert!(solo_value > 0.1, "premise: solo informative, {solo_value}");
        for twin in ["twin-a", "twin-b"] {
            let value = mean_information(&paired, twin);
            assert!(value.abs() < 1e-12, "premise: {twin} redundant, {value}");
        }

        let solo = alone.influence("quiet");
        let pair = paired.influence("quiet");
        assert_eq!(pair.len(), 2, "premise: both twins are weighed");
        let solo_factor = solo["solo"].information;
        for twin in ["twin-a", "twin-b"] {
            let factor = pair[twin].information;
            assert!(
                factor < solo_factor,
                "{twin} kept information factor {factor} against the solo's {solo_factor}"
            );
            assert!(factor < 1.0, "{twin} kept full information weight");
        }
        Ok(())
    }

    /// R12-MODEL-3090: influence moves with incremental information. Two
    /// twins always say "up"; a third forecaster calls the direction the
    /// twins cannot. The failure prevented is the third and the twins
    /// reading the same information factor although only one of the three
    /// carries information the others lack. Mutation: restoring
    /// `iv.min(0.0).exp()` reads one for all three and the comparison
    /// fails.
    #[test]
    fn a_forecaster_with_unique_informative_forecasts_outweighs_a_redundant_one() -> Result<()> {
        let mut t = Tournament::new();
        for name in ["twin-a", "twin-b", "caller"] {
            t.admit(spec(name), at(0))?;
        }
        for i in 0..12i64 {
            let day = 2 * i;
            let id = format!("u-{day}");
            let up = i % 2 == 0;
            t.open(question(&id, "quiet", day), at(day))?;
            t.submit(&id, forecast("twin-a", &id, 8_000, day), at(day))?;
            t.submit(&id, forecast("twin-b", &id, 8_000, day), at(day))?;
            let call = if up { 8_000 } else { 2_000 };
            t.submit(&id, forecast("caller", &id, call, day), at(day))?;
            let outcome = if up { "up" } else { "not_up" };
            t.resolve(&id, resolution(outcome, 60.0, day + 1), at(day + 1))?;
        }
        activate_all(&mut t);

        // Premise: the caller adds information the twins lack; each twin
        // adds none the other did not.
        let caller_value = mean_information(&t, "caller");
        assert!(
            caller_value > 0.0,
            "premise: caller informative, {caller_value}"
        );
        for twin in ["twin-a", "twin-b"] {
            let value = mean_information(&t, twin);
            assert!(value.abs() < 1e-12, "premise: {twin} redundant, {value}");
        }

        let influence = t.influence("quiet");
        assert_eq!(influence.len(), 3, "premise: all three are weighed");
        let caller = influence["caller"].information;
        for twin in ["twin-a", "twin-b"] {
            let factor = influence[twin].information;
            assert!(
                caller > factor,
                "caller's information factor {caller} did not exceed {twin}'s {factor}"
            );
            assert!(
                influence["caller"].weight > influence[twin].weight,
                "the redundant {twin} kept as much say as the caller"
            );
        }
        Ok(())
    }

    /// R12-MODEL-3091. Mutation: averaging the forecasts before review (or
    /// raising the bar above the spread) raises no task.
    #[test]
    fn material_disagreement_raises_a_research_task_citing_every_forecast() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("bull"), at(0))?;
        t.admit(spec("bear"), at(0))?;
        t.open(question("s", "quiet", 0), at(0))?;
        t.submit("s", forecast("bull", "s", 8_500, 0), at(0))?;
        t.submit("s", forecast("bear", "s", 2_000, 0), at(0))?;
        let task = t
            .review_disagreement("s", at(0))?
            .ok_or_else(|| Error::invalid("material disagreement raised nothing"))?;
        assert!(task.spread >= DISAGREEMENT_BAR);
        assert_eq!(task.cites.len(), 2, "the record dropped a forecast");
        assert_eq!(t.drain_research().len(), 1);
        Ok(())
    }

    /// R12-MODEL-3090 / -6022: a forecaster whose synthetic credits fall
    /// loses weight and compute share. Mutation: fixing every factor at one
    /// (or ignoring the market share) leaves the two weights equal.
    #[test]
    fn a_forecaster_whose_synthetic_capital_falls_loses_weight_and_compute() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("good"), at(0))?;
        t.admit(spec("poor"), at(0))?;
        rounds(
            &mut t,
            "trending",
            0,
            12,
            &[("good", 8_000), ("poor", 2_500)],
            "up",
            80.0,
        )?;
        let decisions = t.review_promotions(&BTreeMap::new());
        assert!(
            decisions
                .iter()
                .any(|d| d.forecaster == "good" && d.promoted)
        );
        // Promote "poor" too so both are weighed, on the record it has.
        if let Some(f) = t.forecasters.get_mut("poor") {
            f.standing = Standing::Active;
        }
        let influence = t.influence("trending");
        let good = influence.get("good").map_or(0.0, |w| w.weight);
        let poor = influence.get("poor").map_or(0.0, |w| w.weight);
        assert!(good > poor, "weights {good} vs {poor}");
        assert!(
            influence["poor"].market_share < influence["good"].market_share,
            "premise: the poor forecaster's credits fell"
        );
        assert!(influence["poor"].calibration < influence["good"].calibration);
        let compute = t.compute_allocation("trending", 1_000)?;
        assert!(compute["good"] > compute["poor"], "compute {:?}", compute);
        Ok(())
    }

    /// R12-MODEL-3093: with every factor but the market share tied, the
    /// aggregate weighs each forecaster by exactly its reputation share — its
    /// settled synthetic-credit balance over the total among those weighed.
    /// The weight is influence and the share is its first factor; this pins
    /// that the other five reduce nothing when they agree. Mutation:
    /// normalising by forecaster count instead of the product (or dropping
    /// `market_share` from the product) fails both the weight and the pooled
    /// probability.
    #[test]
    fn with_every_other_factor_equal_the_aggregate_weights_are_the_reputation_shares() -> Result<()>
    {
        let mut t = Tournament::new();
        for name in ["sharp", "blunt"] {
            t.admit(spec(name), at(0))?;
        }
        activate_all(&mut t);
        // Scored history moves the market, nothing else: the settlement is
        // the market's own, so neither forecaster carries a score that would
        // separate calibration, information, cost or latency.
        t.market.settle(
            &[("sharp".to_string(), 0.8), ("blunt".to_string(), 0.3)],
            0.55,
        )?;
        t.open(question("q", "quiet", 0), at(0))?;
        t.submit("q", forecast("sharp", "q", 7_000, 0), at(0))?;
        t.submit("q", forecast("blunt", "q", 4_000, 0), at(0))?;

        let shares = t.market.shares(["sharp", "blunt"]);
        // Premise: settlement separated the shares, and the other factors tie.
        assert!(shares["sharp"] > shares["blunt"], "shares {shares:?}");
        let influence = t.influence("quiet");
        for w in influence.values() {
            assert_eq!(
                (w.calibration, w.information, w.cost, w.latency),
                (1.0, 1.0, 1.0, 1.0),
                "{w:?}"
            );
        }
        assert!(
            (influence["sharp"].regime_survival - influence["blunt"].regime_survival).abs() < 1e-12
        );

        let open = t
            .question("q")
            .cloned()
            .ok_or_else(|| Error::not_found("q"))?;
        let weights = t.question_weights(&open, "quiet", None);
        for name in ["sharp", "blunt"] {
            assert!(
                (weights[name] - shares[name]).abs() < 1e-12,
                "{name}: weight {} share {}",
                weights[name],
                shares[name]
            );
        }
        let pooled = t
            .aggregate("q", "quiet")?
            .ok_or_else(|| Error::invalid("no aggregate"))?;
        let expected = shares["sharp"] * 0.7 + shares["blunt"] * 0.4;
        assert!(
            (pooled["up"] - expected).abs() < 1e-12,
            "pooled {} expected {expected}",
            pooled["up"]
        );
        Ok(())
    }

    /// R12-MODEL-3090's cost and latency factors. Mutation: dropping either
    /// factor from the product leaves the weights equal.
    #[test]
    fn cost_and_latency_each_move_influence() -> Result<()> {
        let mut t = Tournament::new();
        for name in ["cheap", "dear"] {
            t.admit(spec(name), at(0))?;
        }
        for i in 0..PROMOTION_SAMPLE as i64 {
            let day = 2 * i;
            let id = format!("c-{day}");
            t.open(question(&id, "quiet", day), at(day))?;
            t.submit(&id, forecast("cheap", &id, 7_000, day), at(day))?;
            let mut dear = forecast("dear", &id, 7_000, day);
            dear.cost = 50.0;
            dear.latency = Duration::from_millis(5_000);
            t.submit(&id, dear, at(day))?;
            t.resolve(&id, resolution("up", 30.0, day + 1), at(day + 1))?;
        }
        t.review_promotions(&BTreeMap::new());
        let influence = t.influence("quiet");
        assert!(influence["cheap"].cost > influence["dear"].cost);
        assert!(influence["cheap"].latency > influence["dear"].latency);
        assert!(influence["cheap"].weight > influence["dear"].weight);
        Ok(())
    }

    /// R12-MODEL-3095. Mutation: predicting from the overall rate only
    /// (ignoring the regime counts) leaves the weight unchanged across the
    /// regime change.
    #[test]
    fn a_regime_change_predicted_to_break_a_forecaster_reweights_the_ensemble() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("trend-follower"), at(0))?;
        t.admit(spec("all-weather"), at(0))?;
        rounds(
            &mut t,
            "trending",
            0,
            10,
            &[("trend-follower", 9_000), ("all-weather", 7_000)],
            "up",
            80.0,
        )?;
        rounds(
            &mut t,
            "crisis",
            40,
            6,
            &[("trend-follower", 9_000), ("all-weather", 3_000)],
            "not_up",
            -300.0,
        )?;
        t.review_promotions(&BTreeMap::new());
        if let Some(f) = t.forecasters.get_mut("trend-follower") {
            f.standing = Standing::Active;
        }
        if let Some(f) = t.forecasters.get_mut("all-weather") {
            f.standing = Standing::Active;
        }
        assert!(
            t.meta_model().predict("trend-follower", "crisis")
                > t.meta_model().predict("trend-follower", "trending"),
            "premise: the meta-model expects the trend follower to fail in a crisis"
        );
        let calm = t.influence("trending")["trend-follower"].weight;
        let stressed = t.influence("crisis")["trend-follower"].weight;
        assert!(
            stressed < calm,
            "the regime change did not reweight: {calm} → {stressed}"
        );
        Ok(())
    }

    /// The verifier society blocks promotion. Mutation: ignoring `refuted`
    /// in `review_promotions` promotes the refuted forecaster.
    #[test]
    fn a_forecaster_the_society_refuted_is_not_promoted() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("suspect"), at(0))?;
        rounds(&mut t, "quiet", 0, 12, &[("suspect", 8_000)], "up", 80.0)?;
        let refuted = BTreeMap::from([("suspect".to_string(), "data-poisoning".to_string())]);
        let decisions = t.review_promotions(&refuted);
        assert!(decisions.iter().all(|d| !d.promoted));
        assert!(matches!(
            t.standing("suspect"),
            Some(Standing::Blocked { .. })
        ));
        assert!(t.compute_allocation("quiet", 10)?.is_empty());
        Ok(())
    }

    /// Point-in-time: a forecast issued once the answer is knowable is
    /// refused, and a question cannot be scored early. Mutation: deleting
    /// either check admits the leak.
    #[test]
    fn a_late_forecast_and_an_early_resolution_are_both_refused() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(spec("late"), at(0))?;
        t.open(question("p", "quiet", 0), at(0))?;
        assert!(
            t.submit("p", forecast("late", "p", 6_000, 1), at(1))
                .is_err()
        );
        assert!(t.resolve("p", resolution("up", 1.0, 0), at(0)).is_err());
        Ok(())
    }

    #[test]
    fn an_expired_temporary_specialist_is_retired_and_stops_forecasting() -> Result<()> {
        let mut t = Tournament::new();
        t.admit(
            ForecasterSpec {
                id: "election-2026".to_string(),
                kind: ForecasterKind::Temporary { expires_at: at(3) },
                horizon: LatticeHorizon::HoursDays,
                lane: PromotionLane::StrategyCapitalRisk,
            },
            at(0),
        )?;
        t.open(question("e", "quiet", 5), at(5))?;
        assert!(
            t.submit("e", forecast("election-2026", "e", 6_000, 5), at(5))
                .is_err()
        );
        assert_eq!(t.retire_expired(at(5)), vec!["election-2026".to_string()]);
        assert!(!t.is_admitted("election-2026"));
        Ok(())
    }
}
