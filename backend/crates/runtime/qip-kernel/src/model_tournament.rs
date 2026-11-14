//! The Model Tournament, Forecast Market and verifier society as the cycle
//! runs them (blueprint §9.7, §9.8, §9.11, §22.1; R12-MODEL-7198, -7201,
//! -3089..-3095, -6018..-6022; R12-WORLD-3086..-3088; R12-REASON-3108).
//!
//! `qip_learning_engine::{tournament, forecast_market, verifier}` are the
//! arithmetic. This module is the composition: what the platform feeds them,
//! when, and what reaches the log.
//!
//! # Who forecasts, and on what
//!
//! Every hypothesis REASON forms with a directional claim opens one question
//! — "is the subject's next close at or after the horizon above the close
//! the claim was made against?" — and every forecaster that looked at it
//! answers independently, as a [`TournamentForecast`]:
//!
//! * the **detector** whose kind is the hypothesis class
//!   (`detector:<class>`), at the hypothesis's effective confidence;
//! * every **analyst** on the panel that produced an informative finding
//!   (`analyst:<agent>`), at `½ + ½ × sign × effective conviction`, with the
//!   run's metered cost and wall-clock latency.
//!
//! A detector's cost is the evidence items it examined, on the step meter
//! (ADR 0149 decision 13), ranked only among step-metered entrants and never
//! against an analyst's money. Its latency is unmetered: the hypothesis is
//! formed at the pass instant, so it reads the neutral factor, neither best
//! nor worst, and is not a measured latency.
//!
//! The forecasts are categorical rather than full §9.14 [`Forecast`]
//! contracts on purpose: a panel finding states a direction and a
//! conviction, not a tail mass or an uncertainty decomposition, and filling
//! those fields in to satisfy the contract would be inventing a figure. The
//! tournament reads a full contract where a producer has one
//! ([`TournamentForecast::from_contract`]).
//!
//! The question is filed under the regime **at opening** — the market and
//! volatility regime the cost router keys its own reputation on — and is
//! resolved in LEARN against the first bar that closed at or after its
//! horizon, never the bar in front of the platform at the time it looks.
//!
//! [`Forecast`]: qip_contracts::forecast::Forecast
//!
//! # The verifier society
//!
//! Every **important** prediction — approved, or held at
//! [`IMPORTANCE_CONFIDENCE`] or above — is attacked by all eight §9.11
//! critics when it is formed, from evidence captured at that instant. A
//! refuted prediction's question is held out of the market aggregate. Every
//! forecaster about to be promoted is attacked too, on its tournament
//! record, and a refuted one stays unpromoted. And every model the registry
//! is asked to promote is attacked at
//! [`crate::platform::Platform::promote_model_from`]; a refutation refuses
//! the promotion. Every verdict is journaled.
//!
//! # Advisory
//!
//! Nothing here holds a handle to a thesis, a size, a reservation or a
//! broker. The aggregate, the influence weights and the compute allocation
//! are records. A promotion refused is authority not added — never a path
//! created. The one power that reaches DECIDE is ADR 0125 decision 2's veto
//! as ADR 0170 sets it: a refutation of an important prediction is returned
//! on [`ReasonVerdict`] and REASON forms no proposal from it. It can remove a
//! thesis, never size or enlarge one.

use qip_agents::finding::FindingStatus;
use qip_contracts::challenge::{ChallengeRequest, VerificationResult};
use qip_contracts::control::{
    ConsensusWeight, ForecastConsensus, ForecastConsensusFields, ModelVote, ModelVoteFields,
    ScorePoint,
};
use qip_contracts::validated::Contract;
use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_investment_agents::chief::OrganisationReport;
use qip_learning_engine::tournament::{
    CostMeter, ForecasterKind, ForecasterSpec, InfluenceWeight, LatticeHorizon, PromotionDecision,
    QuestionSpec, ResearchTask, Resolution, ResolvedQuestion, Scorecard, ScoredForecast,
    Tournament, TournamentForecast,
};
use qip_learning_engine::verifier::{
    EvidenceItem, ModelDossier, ModelRecord, PanelView, PredictionDossier, Verification,
    VerifierSociety,
};
use qip_market::bar::Bar;
use qip_reasoning_engine::evidence::Stance;
use qip_reasoning_engine::hypothesis::Hypothesis;
use qip_world_model::{WorldModelBranch, WorldModelSpawner};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The producer the per-cycle tournament record carries.
pub const TOURNAMENT_ORIGIN: &str = "kernel/model-tournament";

/// The producer every verifier verdict carries.
pub const VERIFIER_ORIGIN: &str = "kernel/verifier-society";

/// The outcome a question resolves to when the close rose.
pub const UP: &str = "up";

/// The outcome otherwise — flat included, because "up" did not happen.
pub const NOT_UP: &str = "not_up";

/// A prediction at or above this confidence is important enough to attack
/// even if review rejected it.
pub const IMPORTANCE_CONFIDENCE: f64 = 0.6;

/// Compute units the tournament divides among promoted forecasters each
/// cycle it runs. A share, not a quota anything enforces yet.
pub const COMPUTE_BUDGET: u64 = 1_000;

/// How long past its horizon a question waits for a bar before it is
/// abandoned unscored.
pub const ABANDON_AFTER: Duration = Duration::from_days(7);

/// Returns a subject needs before a tail threshold is stated for it.
pub const MIN_TAIL_RETURNS: usize = 10;

/// A move this many standard deviations of the subject's own returns is a
/// tail move.
pub const TAIL_SIGMAS: f64 = 2.0;

/// The window the adverse-tail measure reads.
pub const TAIL_WINDOW: usize = 20;

/// How long a forecaster's tournament record stays a valid "evaluation" for
/// the model-risk critic.
pub const RECORD_VALIDITY: Duration = Duration::from_days(30);

/// The residual — the probability the tournament's aggregate left on the
/// outcome that did not happen — above which a resolution spawns a temporary
/// specialist on its subject (R12-MODEL-7198). At 0.6 the pooled forecasters
/// gave what happened less than four chances in ten: the standing population
/// was wrong about that subject, which is the insufficiency
/// [`WorldModelSpawner`] branches on.
pub const SPAWN_RESIDUAL: f64 = 0.6;

/// How long a spawned temporary specialist competes before the tournament
/// retires it. Long enough for several questions on its subject to resolve,
/// so its record is a record; short enough that a shock's specialist does
/// not outlive the shock.
pub const TEMPORARY_LIFETIME: Duration = Duration::from_days(14);

/// The id prefix of a spawned temporary specialist; the subject follows.
pub const TEMPORARY_PREFIX: &str = "temporary:";

/// The world-model branch a temporary specialist is spawned under names this
/// as its parent: the standing population whose residual triggered it.
pub const STANDING_POPULATION: &str = "tournament/standing";

/// The forecaster-id prefix a fitted model card enters under (ADR 0390), the
/// same id [`TournamentStage::verify_model`] reads a card's record by.
pub const CARD_PREFIX: &str = "model:";

/// One fitted card's forecast of its own instrument's next bar, as the
/// learning desk offers it (ADR 0390, R12-MODEL-3089).
///
/// The probability is a measurement and carries no constant anybody chose.
/// The desk scores the card on the held-out rows the fit never saw and keeps
/// each row's realised error; it scores the newest bar's features for a
/// point prediction; and it counts the held-out errors which, added to that
/// prediction, leave the next return above zero. Had the card erred on the
/// next bar the way it erred on any one held-out row, that many of them
/// would have closed up. `P(up) = (ups + 1) / (rows + 2)`: the rank of zero
/// among prediction-plus-error, with the same Laplace smoothing the
/// temporary specialist uses, so a card with few rows cannot say 0 or 1.
/// No error distribution is assumed; exchangeability of the held-out rows
/// with the next one is, and the tournament's score is what tests it.
///
/// Counts and not a probability, so the figure the tournament holds is
/// derived here, once, from what was counted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardForecast {
    /// The card, as `ModelCard::reference`.
    pub reference: String,
    /// The one subject the card was fitted on.
    pub subject: String,
    /// Held-out rows whose realised error, added to the prediction for the
    /// newest bar, leaves the next bar's return above zero.
    pub holdout_ups: u32,
    /// Held-out rows the card was scored on.
    pub holdout_rows: u32,
    /// The close the prediction is a return from.
    pub from_close: f64,
    /// When that bar closed.
    pub from_close_time: Timestamp,
    /// The spacing of the bars the card was fitted on: the span its fitted
    /// quantity, the next bar's return, covers.
    pub bar: Duration,
}

impl CardForecast {
    /// `(ups + 1) / (rows + 2)`.
    pub fn p_up(&self) -> f64 {
        (f64::from(self.holdout_ups) + 1.0) / (f64::from(self.holdout_rows) + 2.0)
    }

    /// The forecaster id the card competes under.
    pub fn forecaster(&self) -> String {
        format!("{CARD_PREFIX}{}", self.reference)
    }

    /// Why this offer cannot be a forecast, if it cannot. A refusal, never a
    /// repair: an offer with more ups than rows is a counting bug upstream,
    /// and clamping it would enter a probability nobody measured.
    pub fn shape_refusal(&self) -> Option<String> {
        if self.holdout_rows == 0 {
            return Some(
                "it was scored on no held-out row, so there is no error to forecast from; \
                 offer it once its holdout tail holds a row"
                    .to_string(),
            );
        }
        if self.holdout_ups > self.holdout_rows {
            return Some(format!(
                "it counts {} up row(s) among {} held-out row(s); recount the holdout",
                self.holdout_ups, self.holdout_rows
            ));
        }
        if !(self.from_close.is_finite() && self.from_close > 0.0) {
            return Some(format!(
                "the close it forecasts from is {}, not a price",
                self.from_close
            ));
        }
        if self.bar.as_nanos() == 0 {
            return Some(
                "its bars have no spacing, so the span its forecast covers is unknown; \
                 offer it from a series with two distinct close instants"
                    .to_string(),
            );
        }
        None
    }

    /// Why this forecast does not answer a question opened at `now` from
    /// `reference` over `horizon`, if it does not (ADR 0390 decision 3). A
    /// card answers the question its fit is about and no other: the same
    /// close, while that close is still the newest, over exactly one bar.
    fn mismatch(&self, reference: f64, horizon: Duration, now: Timestamp) -> Option<String> {
        if horizon != self.bar {
            return Some(format!(
                "the question spans {horizon:?} and the card was fitted on the next {:?} bar",
                self.bar
            ));
        }
        if now < self.from_close_time || now >= self.from_close_time.saturating_add(self.bar) {
            return Some(
                "its forecast is not from the newest bar: a later bar has closed, or the bar \
                 it names has not"
                    .to_string(),
            );
        }
        if self.from_close.to_bits() != reference.to_bits() {
            return Some(format!(
                "it forecasts from a close of {} and the question measures from {reference}",
                self.from_close
            ));
        }
        None
    }
}

/// A temporary specialist on one subject (R12-MODEL-7198).
///
/// Its model is the subject's own outcome frequency since the shock that
/// spawned it, Laplace-smoothed: `P(up) = (ups + 1) / (n + 2)`, where the
/// triggering resolution is the first observation. That is a fitted model,
/// small on purpose — it answers the tournament's question from outcomes the
/// platform observed and nothing else, so its forecast is one a model made
/// rather than a figure filled in to give the spawn an entrant. Whether that
/// persistence carries information is what the tournament then scores; a
/// specialist with no skill earns no influence and expires.
///
/// Once scored, it is one of the forecasters whose predicted failure the
/// verifier society's model-risk critic reads in a prediction's dossier, so
/// it can contribute to a refutation — ADR 0125 decision 2's veto, which
/// removes a proposal and never creates or enlarges one. That is the only
/// path from it to a decision.
#[derive(Clone, Debug, PartialEq)]
struct TemporarySpecialist {
    id: String,
    expires_at: Timestamp,
    ups: u32,
    observed: u32,
    /// The world-model branch it was spawned under, and its record against
    /// that branch's parent (ADR 0347): kept here so it leaves with the
    /// specialist and is bounded by the live temporaries.
    branch: String,
    /// The resolution instant that spawned it. A respawn on the subject
    /// reuses the forecaster id, and a predecessor's forecasts on questions
    /// still open are scored under that id, so only questions opened after
    /// this instant are the branch's own.
    spawned_at: Timestamp,
    measured: BranchTally,
}

/// Summed Brier scores of a branch's specialist and of its parent, on the
/// questions both forecast after the spawn.
#[derive(Clone, Debug, Default, PartialEq)]
struct BranchTally {
    questions: u32,
    branch: f64,
    parent: f64,
}

/// A spawned branch's out-of-sample record against its parent (ADR 0347,
/// R12-EXPAND-6043, R12-MODEL-6022): the mean Brier score of its temporary
/// specialist and of the standing population's aggregate, on every question
/// on its subject that opened after the spawn and has resolved. Both are
/// taken at the question's opening, before its outcome exists.
///
/// A record, and nothing reads it back: it gates no promotion, no weight and
/// no size. Without it a branch was spawned, competed and expired, and
/// nothing said whether it had forecast better than the population it
/// branched from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchMeasurement {
    /// The branch, as `WorldModelBranch::id`.
    pub branch: String,
    /// Its temporary specialist's forecaster id.
    pub forecaster: String,
    /// The branch's parent, [`STANDING_POPULATION`].
    pub parent: String,
    pub questions: u32,
    pub branch_brier: f64,
    pub parent_brier: f64,
}

impl TemporarySpecialist {
    fn p_up(&self) -> f64 {
        (f64::from(self.ups) + 1.0) / (f64::from(self.observed) + 2.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingQuestion {
    subject: String,
    reference: f64,
    resolves_at: Timestamp,
    /// When the question opened, so a branch counts only questions opened
    /// after its spawn (ADR 0347). Absent on a question opened before this
    /// field existed, which no branch counts.
    #[serde(default)]
    opened_at: Option<Timestamp>,
    /// The aggregate's probability of `up` when the question opened, the
    /// figure a resolution's residual is measured against.
    aggregate_up: Option<f64>,
    /// The tournament's pooling of `up` at opening with every forecaster
    /// weighed alike — the equal-weight baseline the aggregate is measured
    /// against (R12-MODEL-6022, [`Tournament::equal_weight_aggregate`]).
    #[serde(default)]
    equal_up: Option<f64>,
    /// Whether the aggregate's weights were earned when the question opened.
    /// Only such a question measures influence: where nobody had earned any,
    /// the aggregate weighs everyone alike and is the baseline itself.
    #[serde(default)]
    weights_earned: bool,
}

/// ADR 0149 decision 13's measurement (R12-MODEL-6022): whether the
/// influence-weighted aggregate forecast better than the equal-weight mean,
/// on questions resolved after the weights were fixed.
///
/// Out of sample by construction: both figures are taken when the question
/// opens, before its outcome exists, and scored when it resolves. A record,
/// and nothing reads it back — the influence weights and the compute
/// allocation govern nothing until an amendment lets this measurement, once
/// it shows the aggregate ahead, narrow the deep brain's fit budget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AggregateMeasurement {
    /// Questions scored since the process started, each once.
    pub questions: u64,
    /// Mean Brier score of the influence-weighted aggregate, summed over the
    /// question's two outcomes, `2 (p_up - y)^2` — the tournament's own scale,
    /// so it reads beside the scorecards on the same review.
    pub weighted_brier: f64,
    /// Mean Brier score of the equal-weight mean over the same questions.
    pub equal_brier: f64,
}

/// Running sums behind [`AggregateMeasurement`]; constant size whatever
/// the life of the process.
#[derive(Clone, Debug, Default, PartialEq)]
struct AggregateTally {
    questions: u64,
    weighted: f64,
    equal: f64,
}

/// One question opened this cycle, as the record keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenedQuestion {
    pub question: String,
    pub hypothesis: String,
    pub target: String,
    pub regime: String,
    pub resolves_at: Timestamp,
    /// Every forecast, by forecaster: the probability of `up` in basis
    /// points. The disagreement is in this list, not merged out of it.
    pub forecasts: BTreeMap<String, u32>,
    /// The aggregate the market held at opening.
    pub aggregate_up: Option<f64>,
}

/// The §27 pair for one attack: the request that asked every critic, and the
/// society's verdict as the contract result, checked against its own reports
/// and against the request (R12-REASON-9032).
fn challenge_exchange(
    society: &VerifierSociety,
    question: &str,
    dossier: &PredictionDossier,
    verification: &Verification,
    now: Timestamp,
) -> Result<ChallengeExchange> {
    let request = society.request_for_prediction(
        format!("challenge-{question}"),
        dossier,
        VERIFIER_ORIGIN,
        now,
    )?;
    let result = verification.as_result(&request.request_id)?;
    result.answers(&request)?;
    Ok(ChallengeExchange { request, result })
}

/// A challenge and its answer, as one record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChallengeExchange {
    pub request: ChallengeRequest,
    pub result: VerificationResult,
}

/// The tournament's cycle, as the log keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TournamentReview {
    pub cycle: u64,
    pub at: Option<Timestamp>,
    pub opened: Vec<OpenedQuestion>,
    pub refused: Vec<String>,
    pub verifications: Vec<Verification>,
    /// Each important prediction's attack as the §27 request and result
    /// (R12-REASON-9032), beside the society's own record of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub challenges: Vec<ChallengeExchange>,
    pub research: Vec<ResearchTask>,
    pub resolved: Vec<ResolvedQuestion>,
    pub abandoned: Vec<String>,
    pub promotions: Vec<PromotionDecision>,
    pub retired: Vec<String>,
    /// Real P&L attributed per forecaster (R12-MODEL-6021), cumulative.
    pub attributed: BTreeMap<String, Decimal>,
    /// Influence by regime, for the regimes this cycle touched.
    pub influence: BTreeMap<String, BTreeMap<String, InfluenceWeight>>,
    /// Compute allocation by regime, out of [`COMPUTE_BUDGET`].
    pub compute: BTreeMap<String, BTreeMap<String, u64>>,
    /// Scorecards for every forecaster scored this cycle.
    pub scorecards: Vec<Scorecard>,
    /// One §24.9 contract vote per entrant whose forecast the tournament
    /// admitted this cycle and which has a score history to stand on
    /// (R12-CONTRACT-6112).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub votes: Vec<ModelVote>,
    /// Entrants whose forecast was admitted but who cast no vote, with the
    /// reason — no score history yet, or a vote the contract refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voteless: Vec<String>,
    /// The consensus each question opened this cycle formed, with the share
    /// every forecaster carried in it (R12-MODEL-9031).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consensus: Vec<ForecastConsensus>,
    /// The world-model branches this cycle's resolutions spawned, one per
    /// temporary specialist entered (R12-MODEL-7198): the parent, the
    /// residual that triggered it and the threshold it exceeded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawned: Vec<WorldModelBranch>,
    /// Every live branch scored against its parent this cycle (ADR 0347),
    /// cumulative since its spawn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branch_measurements: Vec<BranchMeasurement>,
    /// The aggregate's measurement against the equal-weight mean, on every
    /// cycle that scored a question for it (R12-MODEL-6022).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_measurement: Option<AggregateMeasurement>,
}

/// What every tournament record's idempotency key starts with.
pub const IDEMPOTENCY_PREFIX: &str = "tournament:";

impl EventBody for TournamentReview {
    const TOPIC: Topic = Topic::HypothesisScored;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.cycle))
    }
}

impl TournamentReview {
    fn is_empty(&self) -> bool {
        self.opened.is_empty()
            && self.refused.is_empty()
            && self.verifications.is_empty()
            && self.challenges.is_empty()
            && self.research.is_empty()
            && self.resolved.is_empty()
            && self.abandoned.is_empty()
            && self.promotions.is_empty()
            && self.retired.is_empty()
            && self.spawned.is_empty()
            && self.branch_measurements.is_empty()
            && self.aggregate_measurement.is_none()
    }

    /// One clause for the LEARN stage detail.
    pub fn detail(&self) -> String {
        let refuted = self
            .verifications
            .iter()
            .filter(|v| v.verdict.is_refuted())
            .count();
        format!(
            "tournament: {} question(s) opened, {} resolved, {} abandoned, {} verified ({} \
             refuted), {} disagreement task(s), {} promotion decision(s)",
            self.opened.len(),
            self.resolved.len(),
            self.abandoned.len(),
            self.verifications.len(),
            refuted,
            self.research.len(),
            self.promotions.len()
        )
    }
}

/// A verifier verdict on a model the registry was asked to promote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelVerified {
    pub verification: Verification,
}

impl EventBody for ModelVerified {
    const TOPIC: Topic = Topic::HypothesisChallenged;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        match &self.verification.target {
            qip_learning_engine::verifier::Target::Model { reference } => Some(format!(
                "verified:{reference}:{}",
                self.verification.at.as_nanos()
            )),
            qip_learning_engine::verifier::Target::Prediction { hypothesis_id, .. } => {
                Some(format!(
                    "verified:{hypothesis_id}:{}",
                    self.verification.at.as_nanos()
                ))
            }
        }
    }
}

/// What the tournament tells REASON about one hypothesis: a clause for the
/// stage detail and, where the verifier society refuted an important
/// prediction, the refutation REASON must honour (ADR 0125 decision 2,
/// ADR 0170, R12-REASON-3108).
///
/// The refutation is the society's one path back into the thesis: a veto,
/// never a size. A refuted leading explanation used to be held out of the
/// tournament's aggregate and then sized and traded anyway, because this
/// module returned only a string; the type carries the refusal so the caller
/// cannot read the clause and drop the verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct ReasonVerdict {
    pub clause: String,
    /// The society's summary when it refuted the prediction; `None` when it
    /// survived or the prediction was not important enough to attack.
    pub refuted: Option<String>,
}

/// What REASON hands the tournament about one hypothesis, all captured at
/// the instant it was formed.
#[derive(Debug)]
pub struct ReasonObservation<'a> {
    pub hypothesis: &'a Hypothesis,
    pub report: &'a OrganisationReport,
    pub roster: &'a [String],
    /// The market and volatility regime of the subject, now.
    pub regime: String,
    /// The subject's observed closes, oldest first.
    pub closes: &'a [f64],
    pub spread_ratio: Option<f64>,
    /// Whether the subject's regime is tracked, and crossed recently.
    pub crossed_recently: Option<bool>,
    pub expected_move_bps: f64,
    pub approved: bool,
    /// The agents that explain: those whose manifest role is `Research` or
    /// `Adversarial` (ADR 0170 decision 2). The verifier society reads only
    /// their views and their missing inputs. A control's or a construction
    /// agent's finding is about how the platform acts, not about the subject
    /// (ADR 0147 keeps control findings out of the evidence for the same
    /// reason); counted, a risk control's negative finding read as a panel
    /// leaning against every bullish claim, and a construction agent's
    /// missing quantum backend as a variable every explanation left out.
    /// The panel report itself keeps every finding.
    pub explaining: BTreeSet<String>,
}

/// The stage's state, held by the platform across cycles.
#[derive(Debug, Default)]
pub struct TournamentStage {
    tournament: Tournament,
    society: VerifierSociety,
    pending: BTreeMap<String, PendingQuestion>,
    /// Forecasters the society refuted at promotion review, with the reason.
    refuted: BTreeMap<String, String>,
    /// The regime of the most recent question, for the promotion review's
    /// model-risk reading.
    last_regime: Option<String>,
    review: TournamentReview,
    /// Each forecaster's Brier score at every resolution, oldest first and
    /// bounded by [`SCORE_HISTORY`]: the history a [`ModelVote`] stands on.
    scores: BTreeMap<String, Vec<ScorePoint>>,
    /// Live temporary specialists by subject, at most one each. Bounded by
    /// the subjects the platform asks questions on, and every entry leaves
    /// at its expiry.
    temporaries: BTreeMap<String, TemporarySpecialist>,
    /// The aggregate's out-of-sample record against the equal-weight mean.
    aggregate_tally: AggregateTally,
    /// The learning desk's standing offers, by subject and then by card
    /// reference (ADR 0390). Replaced whole per subject at each offer, so a
    /// card the desk no longer offers leaves; an offer older than its own
    /// bar is dropped where it is read. Bounded by the registry's cards.
    cards: BTreeMap<String, BTreeMap<String, CardForecast>>,
}

/// How many past scores a forecaster's vote carries. Bounded, because the
/// history rides on every vote journaled and a working set that grows with
/// the life of the process is the retention this platform refuses.
pub const SCORE_HISTORY: usize = 32;

/// The consensus a question formed: the aggregate the tournament pooled, and
/// each forecaster's share of it as the tournament weighed it, normalised to
/// sum to one. The shares are the reputation the tournament earned for each
/// forecaster — market share, calibration, information, cost, latency and
/// regime survival — or equal, and said to be, where nobody has earned any.
fn consensus(
    proposition: &str,
    regime: &str,
    aggregate_up: f64,
    weights: &BTreeMap<String, f64>,
    earned: bool,
    now: Timestamp,
) -> Result<ForecastConsensus> {
    let total: f64 = weights.values().sum();
    if !(total.is_finite() && total > 0.0) {
        return Err(qip_core::error::Error::invalid(format!(
            "the weights on {proposition} sum to {total}; no consensus is formed from no weight"
        )));
    }
    let weights = weights
        .iter()
        .map(|(model, weight)| {
            Decimal::from_f64(weight / total)
                .map(|weight| ConsensusWeight {
                    model: model.clone(),
                    weight,
                })
                .ok_or_else(|| {
                    qip_core::error::Error::invalid(format!(
                        "{model}'s weight of {weight} is not a share"
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    ForecastConsensus::new(ForecastConsensusFields {
        schema_version: ForecastConsensusFields::SCHEMA_VERSION,
        proposition: proposition.to_string(),
        regime: regime.to_string(),
        probability_bp: up_bp(aggregate_up.clamp(0.0, 1.0)),
        weights,
        reputation_weighted: earned,
        formed_at: now,
    })
}

/// One entrant's contract vote, or the reason it has none. The version is
/// the manifest digest an analyst ran under, or the kernel's own version for
/// a detector, so a vote names the code that produced it; the cost is the
/// run's metered spend, refused rather than defaulted when it is not money.
fn model_vote(
    forecaster: &str,
    version: &str,
    proposition: &str,
    probability_bp: u32,
    regime: &str,
    history: Option<&Vec<ScorePoint>>,
    cost_micros: f64,
    now: Timestamp,
) -> Result<ModelVote> {
    let score_history = history.cloned().unwrap_or_default();
    // The statistics-to-money crossing for a vote: spend is metered in
    // millionths of the account currency.
    let compute_cost = Decimal::from_f64(cost_micros / 1_000_000.0).ok_or_else(|| {
        qip_core::error::Error::invalid(format!(
            "{forecaster}'s compute cost of {cost_micros} micros is not an amount; no vote is \
             cast on a cost nobody can bill"
        ))
    })?;
    ModelVote::new(ModelVoteFields {
        schema_version: ModelVoteFields::SCHEMA_VERSION,
        model: forecaster.to_string(),
        version: version.to_string(),
        proposition: proposition.to_string(),
        probability_bp,
        regime: regime.to_string(),
        score_history,
        compute_cost,
        cast_at: now,
    })
}

fn returns(closes: &[f64]) -> Vec<f64> {
    closes
        .windows(2)
        .filter(|w| w[0] > 0.0 && w[1].is_finite())
        .map(|w| (w[1] - w[0]) / w[0])
        .collect()
}

fn deviation(values: &[f64]) -> Option<f64> {
    if values.len() < 2 {
        return None;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    Some((values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt())
}

/// The tail threshold, in basis points, a question on a subject with these
/// returns carries: [`TAIL_SIGMAS`] of the subject's own deviations, and
/// nothing until [`MIN_TAIL_RETURNS`] returns exist — a deviation read off a
/// handful of moves would call ordinary noise a tail. A separate function so
/// the rule is testable apart from opening a question.
fn tail_threshold(returns: &[f64]) -> Option<f64> {
    if returns.len() < MIN_TAIL_RETURNS {
        return None;
    }
    deviation(returns)
        .map(|sd| TAIL_SIGMAS * sd * 10_000.0)
        .filter(|t| *t > 0.0)
}

/// The quantiles every forecast on a question states over the realised move,
/// in basis points — the value a question resolves with.
pub const MOVE_QUANTILES: [f64; 3] = [0.1, 0.5, 0.9];

/// The standard normal's 90th percentile: the spread [`MOVE_QUANTILES`] sit
/// at either side of the centre, in units of the subject's deviation.
const Z_90: f64 = 1.281_551_565_545;

/// A forecaster's distribution over the realised move, so its quantile
/// accuracy is scored beside its direction (R12-MODEL-6018).
///
/// The centre is the mean of the two-point move the claim states — up its
/// `expected_move_bps` with the forecaster's own `p_up`, down by the same with
/// the rest — so a forecaster that leans harder on the claim is centred
/// further along it, and one that disagrees is centred against it. The spread
/// is the subject's own one-period return deviation; it is not scaled to the
/// horizon, so it understates a multi-bar horizon's spread, which the pinball
/// loss then charges every forecaster for alike rather than favouring one.
///
/// `None` — no quantiles, and the forecast scored on direction alone — when
/// the claim states no finite, non-zero move (the platform reads zero where
/// no prediction carried a claim) or the subject has no measurable deviation:
/// a distribution invented without either would be scored as if someone had
/// stated it.
fn move_quantiles(p_up: f64, expected_move_bps: f64, returns: &[f64]) -> Option<Vec<(f64, f64)>> {
    let magnitude = expected_move_bps.abs();
    if !(magnitude.is_finite() && magnitude > 0.0 && p_up.is_finite()) {
        return None;
    }
    let spread = deviation(returns)
        .map(|sd| sd * 10_000.0)
        .filter(|sd| sd.is_finite() && *sd > 0.0)?;
    let centre = (2.0 * p_up.clamp(0.0, 1.0) - 1.0) * magnitude;
    Some(vec![
        (MOVE_QUANTILES[0], centre - Z_90 * spread),
        (MOVE_QUANTILES[1], centre),
        (MOVE_QUANTILES[2], centre + Z_90 * spread),
    ])
}

fn up_bp(probability: f64) -> u32 {
    // In range by construction (a probability), so the rounded value fits.
    (probability * 10_000.0).round() as u32
}

fn binary(up: u32) -> BTreeMap<String, u32> {
    BTreeMap::from([(UP.to_string(), up), (NOT_UP.to_string(), 10_000 - up)])
}

impl TournamentStage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one Brier score to `forecaster`'s history at `now`. Two
    /// resolutions at one instant are merged into their mean, because the
    /// contract wants the history strictly increasing in time and two points
    /// at one instant are one observation of the forecaster's skill.
    fn record_score(&mut self, forecaster: &str, brier: Option<f64>, now: Timestamp) {
        let Some(score) = brier
            .filter(|b| b.is_finite() && *b >= 0.0)
            .and_then(Decimal::from_f64)
        else {
            return;
        };
        let history = self.scores.entry(forecaster.to_string()).or_default();
        match history.last_mut() {
            Some(last) if last.at == now => {
                last.score = (last.score + score) / Decimal::from(2);
            }
            _ => history.push(ScorePoint { at: now, score }),
        }
        if history.len() > SCORE_HISTORY {
            let excess = history.len() - SCORE_HISTORY;
            history.drain(..excess);
        }
    }

    pub fn tournament(&self) -> &Tournament {
        &self.tournament
    }

    /// Score one resolved question for [`AggregateMeasurement`]: both the
    /// aggregate and the equal-weight mean, as each stood when the question
    /// opened. A question whose weights were not earned, or that lacks
    /// either figure, is not a measurement of influence and is skipped.
    fn measure_aggregate(&mut self, pending: &PendingQuestion, up: bool) -> bool {
        let (true, Some(weighted), Some(equal)) = (
            pending.weights_earned,
            pending.aggregate_up,
            pending.equal_up,
        ) else {
            return false;
        };
        if !(weighted.is_finite() && equal.is_finite()) {
            return false;
        }
        let y = if up { 1.0 } else { 0.0 };
        self.aggregate_tally.questions += 1;
        self.aggregate_tally.weighted += 2.0 * (weighted - y).powi(2);
        self.aggregate_tally.equal += 2.0 * (equal - y).powi(2);
        true
    }

    /// A resolution on `pending`'s subject: the live temporary specialist
    /// there, if any, learns the outcome; and where the aggregate's residual
    /// exceeds [`SPAWN_RESIDUAL`] and none is live, one is spawned, its
    /// branch recorded on the review. The aggregate is the population's
    /// forecast, so a miss by one forecaster alone does not spawn.
    fn observe_outcome(&mut self, pending: &PendingQuestion, up: bool, cycle: u64, now: Timestamp) {
        if let Some(live) = self
            .temporaries
            .get_mut(&pending.subject)
            .filter(|t| t.expires_at > now)
        {
            live.observed = live.observed.saturating_add(1);
            live.ups = live.ups.saturating_add(u32::from(up));
            return;
        }
        let Some(aggregate_up) = pending.aggregate_up.filter(|p| p.is_finite()) else {
            return;
        };
        let residual = if up { 1.0 - aggregate_up } else { aggregate_up };
        // Statistic to the spawner's decimal threshold, at four places — the
        // basis-point resolution the aggregate was recorded in.
        let (Some(residual), Some(threshold)) = (
            Decimal::from_f64((residual * 10_000.0).round() / 10_000.0),
            Decimal::from_f64(SPAWN_RESIDUAL),
        ) else {
            return;
        };
        let id = format!("{TEMPORARY_PREFIX}{}", pending.subject);
        let expires_at = now.saturating_add(TEMPORARY_LIFETIME);
        let proposed = WorldModelSpawner::new(threshold).and_then(|spawner| {
            spawner.propose_branch(
                STANDING_POPULATION,
                residual,
                &format!("{id}@{cycle}"),
                &format!(
                    "the standing forecasters left {residual} on the outcome {} took; a \
                     specialist on its own outcome frequency competes until {expires_at}",
                    pending.subject,
                ),
                now,
            )
        });
        // Below the threshold is the ordinary case and records nothing.
        if let Ok(branch) = proposed {
            self.temporaries.insert(
                pending.subject.clone(),
                TemporarySpecialist {
                    id,
                    expires_at,
                    ups: u32::from(up),
                    observed: 1,
                    branch: branch.id.clone(),
                    spawned_at: now,
                    measured: BranchTally::default(),
                },
            );
            self.review.spawned.push(branch);
        }
    }

    /// Score the live branch on `pending`'s subject against its parent, on
    /// a question its specialist forecast: the specialist's own Brier score
    /// from `scores`, and the aggregate's at opening, in the same two-outcome
    /// form [`Self::measure_aggregate`] uses. Only a question that opened
    /// strictly after the spawn counts: one opened at the spawn's instant
    /// opened earlier in that cycle, and a respawn's reused id would
    /// otherwise carry its predecessor's forecasts on questions still open.
    fn measure_branch(
        &mut self,
        pending: &PendingQuestion,
        up: bool,
        scores: &[ScoredForecast],
        now: Timestamp,
    ) {
        let Some(live) = self
            .temporaries
            .get_mut(&pending.subject)
            .filter(|t| t.expires_at > now)
            .filter(|t| {
                pending
                    .opened_at
                    .is_some_and(|opened| opened > t.spawned_at)
            })
        else {
            return;
        };
        let (Some(own), Some(parent)) = (
            scores
                .iter()
                .find(|score| score.forecaster == live.id)
                .and_then(|score| score.brier)
                .filter(|b| b.is_finite()),
            pending.aggregate_up.filter(|p| p.is_finite()),
        ) else {
            return;
        };
        let y = if up { 1.0 } else { 0.0 };
        live.measured.questions += 1;
        live.measured.branch += own;
        live.measured.parent += 2.0 * (parent - y).powi(2);
        let n = f64::from(live.measured.questions);
        let measurement = BranchMeasurement {
            branch: live.branch.clone(),
            forecaster: live.id.clone(),
            parent: STANDING_POPULATION.to_string(),
            questions: live.measured.questions,
            branch_brier: live.measured.branch / n,
            parent_brier: live.measured.parent / n,
        };
        let measurements = &mut self.review.branch_measurements;
        match measurements
            .iter_mut()
            .find(|m| m.branch == measurement.branch)
        {
            Some(existing) => *existing = measurement,
            None => measurements.push(measurement),
        }
    }

    pub fn society(&self) -> &VerifierSociety {
        &self.society
    }

    /// Forecasters the society has refused promotion to, with the reason.
    pub fn refuted(&self) -> &BTreeMap<String, String> {
        &self.refuted
    }

    /// Replace the card forecasts standing on `subject` with `offered`
    /// (ADR 0390), returning one sentence per offer refused. The caller has
    /// already judged each card against the registry
    /// ([`crate::platform::Platform::stage_card_forecasts`]); this checks
    /// what the stage can: the offer names this subject, and is a forecast.
    ///
    /// The whole set is replaced and not merged, so the population on a
    /// subject is exactly the cards the desk offered last: one it stopped
    /// offering — drifted, retired, rolled back — has left.
    pub fn stage_cards(&mut self, subject: &str, offered: Vec<CardForecast>) -> Vec<String> {
        let mut refused = Vec::new();
        let mut standing = BTreeMap::new();
        for forecast in offered {
            if forecast.subject != subject {
                refused.push(format!(
                    "{}: it forecasts {} and was offered on {subject}; a card answers only \
                     its own instrument's question",
                    forecast.reference, forecast.subject
                ));
                continue;
            }
            if let Some(reason) = forecast.shape_refusal() {
                refused.push(format!("{}: {reason}", forecast.reference));
                continue;
            }
            standing.insert(forecast.reference.clone(), forecast);
        }
        if standing.is_empty() {
            self.cards.remove(subject);
        } else {
            self.cards.insert(subject.to_string(), standing);
        }
        refused
    }

    /// The card forecasts standing on `subject`, in reference order.
    pub fn standing_cards(&self, subject: &str) -> impl Iterator<Item = &CardForecast> {
        self.cards
            .get(subject)
            .into_iter()
            .flat_map(BTreeMap::values)
    }

    /// Open a question on one hypothesis, enter every forecast on it,
    /// review the disagreement, and — where the prediction is important —
    /// put it to the verifier society. Returns a clause for REASON's detail
    /// and the society's refutation if it refuted the prediction, or `None`
    /// when the hypothesis has no directional claim to ask about.
    pub fn observe_reasoning(
        &mut self,
        observation: ReasonObservation<'_>,
        now: Timestamp,
    ) -> Option<ReasonVerdict> {
        let hypothesis = observation.hypothesis;
        // Fail closed (ADR 0170 decision 1, R12-REASON-3108): an approved
        // prediction is important by definition, and one the society cannot
        // be shown has not survived it. Each of these returns used to read
        // as "no refutation", so an approved thesis with no subject or no
        // usable close went on unattacked.
        let unattackable = |why: &str| {
            observation.approved.then(|| ReasonVerdict {
                clause: format!("; not attacked: {why}"),
                refuted: Some(format!("it could not be attacked: {why}")),
            })
        };
        let Some(subject) = hypothesis.subjects.first().map(|s| s.as_str().to_string()) else {
            return unattackable("it names no subject");
        };
        let Some(&reference) = observation.closes.last() else {
            return unattackable("no close is held for its subject");
        };
        if !(reference.is_finite() && reference > 0.0) {
            return unattackable("its subject's last close is not a positive price");
        }
        // An undirected claim (a regime shift) has no close-direction
        // question to open, but it is still a forecast the society attacks
        // when it is important, with the critics that need no side.
        let Some(sign) = hypothesis.claim.implied_sign() else {
            let confidence = hypothesis.effective_confidence().clamp(0.0, 1.0);
            if !(observation.approved || confidence >= IMPORTANCE_CONFIDENCE) {
                return None;
            }
            let rets = returns(observation.closes);
            let question = format!("q-{}", hypothesis.hypothesis_id.as_str());
            let dossier =
                self.prediction_dossier(&observation, 0.0, &subject, &BTreeMap::new(), &rets);
            let verification = self.society.attack_prediction(&dossier, now);
            let refuted = verification
                .verdict
                .is_refuted()
                .then(|| verification.verdict.summary());
            let clause = format!(
                "; undirected, no tournament question; verifier society: {}",
                verification.verdict.summary()
            );
            match challenge_exchange(&self.society, &question, &dossier, &verification, now) {
                Ok(exchange) => self.review.challenges.push(exchange),
                Err(error) => self
                    .review
                    .refused
                    .push(format!("{question}/challenge: {}", error.message())),
            }
            self.review.verifications.push(verification);
            return Some(ReasonVerdict { clause, refuted });
        };
        let question = format!("q-{}", hypothesis.hypothesis_id.as_str());
        let target = format!("{subject}/close-direction");
        let resolves_at = now.saturating_add(hypothesis.horizon);
        let rets = returns(observation.closes);
        let tail_threshold = tail_threshold(&rets);
        if let Err(error) = self.tournament.open(
            QuestionSpec {
                id: question.clone(),
                target: target.clone(),
                outcomes: vec![NOT_UP.to_string(), UP.to_string()],
                regime: observation.regime.clone(),
                resolves_at,
                tail_threshold,
            },
            now,
        ) {
            self.review
                .refused
                .push(format!("{question}: {}", error.message()));
            // Fail closed (ADR 0170 decision 1): an approved prediction is
            // important by definition, and one the society could not attack
            // has not survived it. This arm returned `refuted: None`, so an
            // approved thesis whose question the tournament refused — a
            // second hypothesis on one subject in one cycle shares the id
            // `hyp-<cycle>-<subject>` — was sized unattacked.
            return Some(ReasonVerdict {
                clause: format!("; tournament refused the question: {}", error.message()),
                refuted: observation.approved.then(|| {
                    format!(
                        "it could not be attacked: the tournament refused its question ({})",
                        error.message()
                    )
                }),
            });
        }
        self.last_regime = Some(observation.regime.clone());
        let band = LatticeHorizon::of(hypothesis.horizon);
        let mut entries: Vec<TournamentForecast> = Vec::new();
        // The version each entrant's vote names, by forecaster id.
        let mut versions: BTreeMap<String, String> = BTreeMap::new();
        versions.insert(
            format!("detector:{}", hypothesis.class),
            format!("qip-kernel-{}", env!("CARGO_PKG_VERSION")),
        );
        let confidence = hypothesis.effective_confidence().clamp(0.0, 1.0);
        let detector_up = if sign > 0.0 {
            confidence
        } else {
            1.0 - confidence
        };
        entries.push(TournamentForecast {
            forecaster: format!("detector:{}", hypothesis.class),
            forecast_id: format!("{question}/detector:{}", hypothesis.class),
            target: target.clone(),
            issued_at: now,
            probabilities_bp: Some(binary(up_bp(detector_up))),
            quantiles: move_quantiles(detector_up, observation.expected_move_bps, &rets),
            // Counted work, not money (ADR 0149 decision 13, R12-MODEL-3090):
            // the evidence items the detector examined, a deterministic step
            // count ranked only among step-metered entrants. Latency stays
            // unmetered: the hypothesis is formed at the pass instant, so a
            // measured latency is zero by construction and a wall-clock
            // reading would not replay.
            cost: Some(hypothesis.evidence.len() as f64),
            cost_meter: CostMeter::Steps,
            latency: None,
        });
        for finding in &observation.report.findings {
            if !matches!(
                finding.status,
                FindingStatus::Complete | FindingStatus::Partial
            ) {
                continue;
            }
            if !observation.roster.contains(&finding.agent_id) {
                continue;
            }
            let p_up = (0.5
                + 0.5 * finding.direction.sign() * finding.effective_conviction().clamp(0.0, 1.0))
            .clamp(0.0, 1.0);
            let run = observation
                .report
                .runs
                .iter()
                .find(|run| run.run_id == finding.run_id);
            if let Some(run) = run {
                versions.insert(
                    format!("analyst:{}", finding.agent_id),
                    format!("manifest-{}", run.manifest_digest),
                );
            }
            entries.push(TournamentForecast {
                forecaster: format!("analyst:{}", finding.agent_id),
                forecast_id: format!("{question}/analyst:{}", finding.agent_id),
                target: target.clone(),
                issued_at: now,
                probabilities_bp: Some(binary(up_bp(p_up))),
                quantiles: move_quantiles(p_up, observation.expected_move_bps, &rets),
                cost: run.map(|run| run.spend.cost_micros as f64),
                cost_meter: CostMeter::Spend,
                latency: run.map(|run| run.duration()),
            });
        }
        // A live temporary specialist on this subject answers too
        // (R12-MODEL-7198), from its own outcome frequency.
        let temporary = self
            .temporaries
            .get(&subject)
            .filter(|t| t.expires_at > now)
            .cloned();
        if let Some(temporary) = &temporary {
            let p_up = temporary.p_up();
            versions.insert(
                temporary.id.clone(),
                format!("qip-kernel-{}/outcome-frequency", env!("CARGO_PKG_VERSION")),
            );
            entries.push(TournamentForecast {
                forecaster: temporary.id.clone(),
                forecast_id: format!("{question}/{}", temporary.id),
                target: target.clone(),
                issued_at: now,
                probabilities_bp: Some(binary(up_bp(p_up))),
                quantiles: move_quantiles(p_up, observation.expected_move_bps, &rets),
                // In-cycle arithmetic over counts, unmetered.
                cost: None,
                cost_meter: CostMeter::Spend,
                latency: None,
            });
        }
        // Each fitted card the desk offered on this subject answers its own
        // instrument's question (ADR 0390, R12-MODEL-3089), from its own
        // held-out error. One whose forecast is about a different close, a
        // different span or an older bar is not entered, and the review says
        // which and why; no card is entered to make up a number.
        if let Some(cards) = self.cards.get_mut(&subject) {
            cards.retain(|_, card| now < card.from_close_time.saturating_add(card.bar));
        }
        let cards: Vec<CardForecast> = self.standing_cards(&subject).cloned().collect();
        for card in cards {
            let id = card.forecaster();
            if let Some(reason) = card.mismatch(reference, hypothesis.horizon, now) {
                self.review
                    .refused
                    .push(format!("{question}/{id}: not entered: {reason}"));
                continue;
            }
            let p_up = card.p_up();
            versions.insert(id.clone(), card.reference.clone());
            entries.push(TournamentForecast {
                forecast_id: format!("{question}/{id}"),
                forecaster: id,
                target: target.clone(),
                issued_at: now,
                probabilities_bp: Some(binary(up_bp(p_up))),
                quantiles: move_quantiles(p_up, observation.expected_move_bps, &rets),
                // Scored on the deep brain's existing capacity, off the
                // cycle: nothing metered it, so nothing is claimed.
                cost: None,
                cost_meter: CostMeter::Spend,
                latency: None,
            });
        }
        let mut forecasts = BTreeMap::new();
        for entry in entries {
            let id = entry.forecaster.clone();
            // The vote contract carries a `Decimal` compute cost with no
            // absent form, so an unmetered entrant votes at zero there; the
            // tournament's own influence arithmetic reads the `None`.
            //
            // A step count is not an amount, so a step-metered entrant votes
            // at zero there too: converting evidence items into compute money
            // is the conversion ADR 0149 decision 13 rules out.
            let cost = match entry.cost_meter {
                CostMeter::Spend => entry.cost.unwrap_or(0.0),
                CostMeter::Steps => 0.0,
            };
            let up = entry
                .probabilities_bp
                .as_ref()
                .and_then(|p| p.get(UP).copied())
                .unwrap_or(0);
            let kind = match &temporary {
                Some(t) if t.id == id => ForecasterKind::Temporary {
                    expires_at: t.expires_at,
                },
                _ => ForecasterKind::Specialist,
            };
            let admitted = self.tournament.admit(
                ForecasterSpec {
                    id: id.clone(),
                    kind,
                    horizon: band,
                    lane: band.admitted_lane(),
                },
                now,
            );
            let submitted = admitted.and_then(|()| self.tournament.submit(&question, entry, now));
            match submitted {
                Ok(()) => {
                    match (self.scores.get(&id), versions.get(&id)) {
                        (None, _) => self.review.voteless.push(format!(
                            "{question}/{id}: no score history yet, so no standing to vote"
                        )),
                        (_, None) => self.review.voteless.push(format!(
                            "{question}/{id}: the run it forecast from is not on the report, so \
                             no version to vote under"
                        )),
                        (history, Some(version)) => match model_vote(
                            &id,
                            version,
                            &format!("{target} = {UP}"),
                            up,
                            &observation.regime,
                            history,
                            cost,
                            now,
                        ) {
                            Ok(vote) => self.review.votes.push(vote),
                            Err(error) => self
                                .review
                                .voteless
                                .push(format!("{question}/{id}: {}", error.message())),
                        },
                    }
                    forecasts.insert(id, up);
                }
                Err(error) => {
                    // A temporary the tournament will not hold is not live:
                    // kept, it would block a respawn on its subject and be
                    // refused again at every question until it expired.
                    if temporary.as_ref().is_some_and(|t| t.id == id) {
                        self.temporaries.remove(&subject);
                    }
                    self.review
                        .refused
                        .push(format!("{question}/{id}: {}", error.message()));
                }
            }
        }
        if let Ok(Some(task)) = self.tournament.review_disagreement(&question, now) {
            self.review.research.push(task);
        }
        let aggregate_up = self
            .tournament
            .aggregate(&question, &observation.regime)
            .ok()
            .flatten()
            .and_then(|p| p.get(UP).copied());
        let weights_earned = self
            .tournament
            .aggregate_weights(&question, &observation.regime)
            .is_ok_and(|(_, earned)| earned);
        let equal_up = self
            .tournament
            .equal_weight_aggregate(&question)
            .ok()
            .flatten()
            .and_then(|p| p.get(UP).copied());
        if let Some(up) = aggregate_up {
            let formed = self
                .tournament
                .aggregate_weights(&question, &observation.regime)
                .and_then(|(weights, earned)| {
                    consensus(
                        &format!("{target} = {UP}"),
                        &observation.regime,
                        up,
                        &weights,
                        earned,
                        now,
                    )
                });
            match formed {
                Ok(consensus) => self.review.consensus.push(consensus),
                Err(error) => self
                    .review
                    .refused
                    .push(format!("{question}/consensus: {}", error.message())),
            }
        }

        // The society, for important predictions only.
        let important = observation.approved || confidence >= IMPORTANCE_CONFIDENCE;
        let mut society_clause = String::new();
        let mut refuted = None;
        if important {
            let dossier = self.prediction_dossier(&observation, sign, &subject, &forecasts, &rets);
            let verification = self.society.attack_prediction(&dossier, now);
            if verification.verdict.is_refuted() {
                let _ = self
                    .tournament
                    .hold_out(&question, verification.verdict.summary());
                refuted = Some(verification.verdict.summary());
            }
            society_clause = format!("; verifier society: {}", verification.verdict.summary());
            // The same attack as the §27 pair a second brain can read: the
            // request that asked every critic, and the result the contract
            // checked against its own reports (R12-REASON-9032).
            let exchange =
                challenge_exchange(&self.society, &question, &dossier, &verification, now);
            match exchange {
                Ok(exchange) => self.review.challenges.push(exchange),
                Err(error) => self
                    .review
                    .refused
                    .push(format!("{question}/challenge: {}", error.message())),
            }
            self.review.verifications.push(verification);
        }
        self.pending.insert(
            question.clone(),
            PendingQuestion {
                subject,
                reference,
                resolves_at,
                opened_at: Some(now),
                aggregate_up,
                equal_up,
                weights_earned,
            },
        );
        self.review.opened.push(OpenedQuestion {
            question,
            hypothesis: hypothesis.hypothesis_id.as_str().to_string(),
            target,
            regime: observation.regime,
            resolves_at,
            forecasts: forecasts.clone(),
            aggregate_up,
        });
        Some(ReasonVerdict {
            clause: format!(
                "; tournament question opened with {} forecast(s){society_clause}",
                forecasts.len()
            ),
            refuted,
        })
    }

    fn prediction_dossier(
        &self,
        observation: &ReasonObservation<'_>,
        sign: f64,
        subject: &str,
        forecasts: &BTreeMap<String, u32>,
        rets: &[f64],
    ) -> PredictionDossier {
        let hypothesis = observation.hypothesis;
        let evidence = hypothesis
            .evidence
            .iter()
            .filter(|e| !matches!(e.stance, Stance::Contextual))
            .map(|e| EvidenceItem {
                origin: e.origin.clone(),
                supports: matches!(e.stance, Stance::Supports),
                weight: e.weight(),
                known_at: e.known_at,
            })
            .collect();
        let panel = observation
            .report
            .findings
            .iter()
            .filter(|f| matches!(f.status, FindingStatus::Complete | FindingStatus::Partial))
            .filter(|f| observation.explaining.contains(&f.agent_id))
            .map(|f| PanelView {
                agent: f.agent_id.clone(),
                direction: f.direction.sign(),
                conviction: f.effective_conviction(),
            })
            .collect();
        let window = &rets[rets.len().saturating_sub(TAIL_WINDOW)..];
        // An undirected claim has no adverse side, so no adverse tail: the
        // tail critic abstains rather than reading every move as harmless.
        let adverse_tail_bps = if window.is_empty() || sign == 0.0 {
            None
        } else {
            Some(
                window
                    .iter()
                    .map(|r| (-sign * r * 10_000.0).max(0.0))
                    .fold(0.0, f64::max),
            )
        };
        let forecaster_failure = forecasts
            .keys()
            .filter(|name| self.tournament.last_scored(name).is_some())
            .filter(|name| self.tournament.mean_brier(name).is_some())
            .map(|name| {
                (
                    name.clone(),
                    self.tournament
                        .meta_model()
                        .predict(name, &observation.regime),
                )
            })
            .collect();
        PredictionDossier {
            hypothesis_id: hypothesis.hypothesis_id.as_str().to_string(),
            subject: subject.to_string(),
            direction: sign,
            confidence: hypothesis.effective_confidence(),
            as_of: hypothesis.as_of,
            expected_move_bps: observation.expected_move_bps,
            evidence,
            panel,
            missing_inputs: observation
                .report
                .findings
                .iter()
                .filter(|f| observation.explaining.contains(&f.agent_id))
                .flat_map(|f| {
                    f.missing_inputs
                        .iter()
                        .map(move |missing| format!("{}: {missing}", f.agent_id))
                })
                .collect(),
            coverage: observation.report.coverage(),
            regime_crossed_recently: observation.crossed_recently,
            spread_ratio: observation.spread_ratio,
            adverse_tail_bps,
            forecaster_failure,
        }
    }

    /// The tournament record of `forecaster`, as the society's model critics
    /// read it.
    fn model_record(&self, forecaster: &str, regime: Option<&str>) -> Option<ModelRecord> {
        let scorecards = self.tournament.scorecards(forecaster);
        if scorecards.is_empty() {
            return None;
        }
        let hits = self.tournament.hit_rates_by_outcome(forecaster);
        Some(ModelRecord {
            scorecards,
            up_hits: hits.get(UP).copied(),
            down_hits: hits.get(NOT_UP).copied(),
            predicted_failure_now: regime
                .map_or(0.0, |r| self.tournament.meta_model().predict(forecaster, r)),
        })
    }

    /// Attack a model the registry is asked to promote. The dossier is the
    /// card's own facts plus, where the model also competes in the
    /// tournament as `model:<reference>`, its record there.
    pub fn verify_model(
        &self,
        card: &qip_ai::registry::ModelCard,
        quarantined: BTreeSet<String>,
        latest_regime_crossing: Option<Timestamp>,
        now: Timestamp,
    ) -> Verification {
        let reference = card.reference();
        let latest = card.latest_evaluation();
        let dossier = ModelDossier {
            reference: reference.clone(),
            evaluated_at: latest.map(|e| e.evaluated_at),
            evaluation_passed: latest.map(|e| e.passed),
            evaluation_validity: card.evaluation_validity,
            drift_score: card.drift_score,
            drift_threshold: card.drift_threshold,
            training_datasets: card.training_datasets.clone(),
            quarantined_sources: quarantined,
            features: card.features.clone(),
            latest_regime_crossing,
            tournament: self
                .model_record(&format!("model:{reference}"), self.last_regime.as_deref()),
        };
        self.society.attack_model(&dossier, now)
    }

    /// LEARN: credit the attribution, resolve what is due, review
    /// promotions through the society, retire the expired, and hand back
    /// the cycle's record — or `None` when nothing happened.
    pub fn learn(
        &mut self,
        bars: &BTreeMap<String, Vec<Bar>>,
        attributed: Option<&BTreeMap<String, Decimal>>,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<TournamentReview>> {
        if let Some(attributed) = attributed {
            for (forecaster, pnl) in attributed {
                if self.tournament.is_admitted(forecaster) {
                    self.tournament.credit_attribution(forecaster, *pnl)?;
                    self.review.attributed.insert(forecaster.clone(), *pnl);
                }
            }
        }
        let due: Vec<(String, PendingQuestion)> = self
            .pending
            .iter()
            .filter(|(_, p)| p.resolves_at <= now)
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect();
        let mut scored: BTreeSet<String> = BTreeSet::new();
        let mut measured = false;
        let mut regimes: BTreeSet<String> = self
            .review
            .opened
            .iter()
            .map(|o| o.regime.clone())
            .collect();
        for (id, pending) in due {
            // The first bar that closed at or after the horizon: the answer
            // as it was first knowable, not as it reads today.
            let settling = bars.get(&pending.subject).and_then(|series| {
                series
                    .iter()
                    .filter(|bar| {
                        bar.close_time() >= pending.resolves_at && bar.close_time() <= now
                    })
                    .min_by_key(|bar| bar.close_time())
            });
            match settling {
                Some(bar) => {
                    let close = bar.close.to_f64();
                    let outcome = if close > pending.reference {
                        UP
                    } else {
                        NOT_UP
                    };
                    let move_bps = (close / pending.reference - 1.0) * 10_000.0;
                    let resolved = self.tournament.resolve(
                        &id,
                        Resolution {
                            outcome: outcome.to_string(),
                            realised_value: Some(move_bps).filter(|v| v.is_finite()),
                            observed_at: bar.close_time(),
                        },
                        now,
                    );
                    self.pending.remove(&id);
                    match resolved {
                        Ok(resolved) => {
                            for score in &resolved.scores {
                                self.record_score(&score.forecaster, score.brier, now);
                            }
                            self.measure_branch(&pending, outcome == UP, &resolved.scores, now);
                            self.observe_outcome(&pending, outcome == UP, cycle, now);
                            measured |= self.measure_aggregate(&pending, outcome == UP);
                            scored.extend(resolved.scores.iter().map(|s| s.forecaster.clone()));
                            regimes.insert(resolved.regime.clone());
                            self.review.resolved.push(resolved);
                        }
                        Err(error) => self
                            .review
                            .refused
                            .push(format!("{id}: {}", error.message())),
                    }
                }
                None if now.since(pending.resolves_at) > ABANDON_AFTER => {
                    self.tournament.abandon(&id);
                    self.pending.remove(&id);
                    self.review.abandoned.push(id);
                }
                None => {}
            }
        }
        // The society attacks every promotion candidate whose record moved
        // this cycle — attacking every candidate every cycle would restate
        // the same verdicts into the log for ever.
        for candidate in self
            .tournament
            .promotion_candidates()
            .into_iter()
            .filter(|candidate| scored.contains(candidate))
        {
            let record = self.model_record(&candidate, self.last_regime.as_deref());
            let dossier = ModelDossier {
                reference: candidate.clone(),
                evaluated_at: self.tournament.last_scored(&candidate),
                evaluation_passed: self.tournament.mean_brier(&candidate).map(|b| b < 0.5),
                evaluation_validity: RECORD_VALIDITY,
                drift_score: 0.0,
                drift_threshold: 0.0,
                training_datasets: Vec::new(),
                quarantined_sources: BTreeSet::new(),
                features: vec![candidate.clone()],
                latest_regime_crossing: None,
                tournament: record,
            };
            let verification = self.society.attack_model(&dossier, now);
            if verification.verdict.is_refuted() {
                self.refuted
                    .insert(candidate.clone(), verification.verdict.summary());
            } else {
                self.refuted.remove(&candidate);
            }
            self.review.verifications.push(verification);
        }
        self.review.promotions = self.tournament.review_promotions(&self.refuted);
        self.review.retired = self.tournament.retire_expired(now);
        // The tournament also evicts expired temporaries on its own when
        // full, so expiry is read off the clock, not off the retired list.
        // The score history goes with it: a respawn on the subject reuses the
        // id, and would otherwise vote on a record its predecessor earned.
        let scores = &mut self.scores;
        self.temporaries.retain(|_, t| {
            let live = t.expires_at > now;
            if !live {
                scores.remove(&t.id);
            }
            live
        });
        for forecaster in &self.review.retired {
            self.refuted.remove(forecaster);
        }
        for regime in &regimes {
            let influence = self.tournament.influence(regime);
            if !influence.is_empty() {
                self.review.influence.insert(regime.clone(), influence);
                self.review.compute.insert(
                    regime.clone(),
                    self.tournament.compute_allocation(regime, COMPUTE_BUDGET)?,
                );
            }
        }
        if measured && self.aggregate_tally.questions > 0 {
            let n = self.aggregate_tally.questions as f64;
            self.review.aggregate_measurement = Some(AggregateMeasurement {
                questions: self.aggregate_tally.questions,
                weighted_brier: self.aggregate_tally.weighted / n,
                equal_brier: self.aggregate_tally.equal / n,
            });
        }
        self.review.scorecards = scored
            .iter()
            .flat_map(|forecaster| self.tournament.scorecards(forecaster))
            .collect();
        // Every task the tournament queued was raised in REASON and is
        // already on this cycle's record; the queue is drained so it cannot
        // grow, not to read it twice.
        let _ = self.tournament.drain_research();
        let review = std::mem::take(&mut self.review);
        if review.is_empty() {
            return Ok(None);
        }
        Ok(Some(TournamentReview {
            cycle,
            at: Some(now),
            ..review
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::challenge::ChallengeVerdict;

    /// The threshold is two of the subject's own deviations, and is stated
    /// only once ten returns exist. Until the round-2 mutation audit this
    /// test asserted only that five closes give fewer than ten returns and
    /// that some deviation is positive — it called neither the threshold
    /// rule nor the minimum, and passed with `TAIL_SIGMAS` at one and with
    /// the minimum at one. Mutations: `TAIL_SIGMAS = 1.0` fails the 2,000 bp
    /// figure; `>= 1` (or `>`) in place of the minimum fails the nine- or
    /// ten-return arm.
    #[test]
    fn tail_threshold_needs_enough_returns_and_reads_two_deviations() {
        // Returns alternating +10% and -10% exactly: mean zero, population
        // deviation 0.1, so two deviations are 2,000 bp.
        let mut closes = vec![100.0_f64];
        for i in 0..MIN_TAIL_RETURNS {
            let last = closes[closes.len() - 1];
            closes.push(if i % 2 == 0 { last * 1.1 } else { last * 0.9 });
        }
        let enough = returns(&closes);
        assert_eq!(
            enough.len(),
            MIN_TAIL_RETURNS,
            "premise: exactly the minimum"
        );
        let threshold = tail_threshold(&enough).expect("ten returns state a threshold");
        assert!((threshold - 2_000.0).abs() < 1e-6, "{threshold}");
        let short = &enough[..MIN_TAIL_RETURNS - 1];
        assert!(
            deviation(short).is_some_and(|sd| sd > 0.0),
            "premise: the short window has a deviation to read"
        );
        assert_eq!(tail_threshold(short), None, "nine returns state no tail");
    }

    /// R12-REASON-9032: a fatal attack on a prediction leaves an exchange
    /// whose result is refuted by the critic that raised it, and whose
    /// request the result answers; the same dossier unaltered leaves a
    /// survival. Mutation: build the result from a fixed `SocietyVerdict`
    /// instead of the attack's own — the contract refuses the disagreement
    /// and the `expect` fails.
    #[test]
    fn a_fatal_attack_is_recorded_as_a_refuted_result_that_answers_its_request() {
        let at = Timestamp::from_secs(1_760_000_000);
        let society = VerifierSociety::standard();
        let sound = PredictionDossier {
            hypothesis_id: "hyp-1".to_string(),
            subject: "obj-AAA".to_string(),
            direction: 1.0,
            confidence: 0.7,
            as_of: at,
            expected_move_bps: 200.0,
            evidence: vec![
                EvidenceItem {
                    origin: "exchange".to_string(),
                    supports: true,
                    weight: 0.5,
                    known_at: at,
                },
                EvidenceItem {
                    origin: "filings".to_string(),
                    supports: true,
                    weight: 0.4,
                    known_at: at,
                },
            ],
            panel: vec![PanelView {
                agent: "valuation".to_string(),
                direction: 1.0,
                conviction: 0.6,
            }],
            missing_inputs: Vec::new(),
            coverage: 0.8,
            regime_crossed_recently: Some(false),
            spread_ratio: Some(1.1),
            adverse_tail_bps: Some(150.0),
            forecaster_failure: BTreeMap::from([("detector:momentum".to_string(), 0.2)]),
        };
        let exchange = |dossier: &PredictionDossier| {
            let verification = society.attack_prediction(dossier, at);
            challenge_exchange(&society, "q-hyp-1", dossier, &verification, at)
                .expect("the society's own verdict satisfies the contract")
        };
        assert_eq!(
            exchange(&sound).result.verdict,
            ChallengeVerdict::Survived,
            "premise: the sound prediction survives"
        );

        let mut fragile = sound;
        fragile.adverse_tail_bps = Some(900.0);
        let fatal = exchange(&fragile);

        assert_eq!(
            fatal.result.verdict,
            ChallengeVerdict::Refuted {
                by: vec!["tail_risk".to_string()]
            }
        );
        assert_eq!(fatal.request.request_id, "challenge-q-hyp-1");
        fatal.result.answers(&fatal.request).expect("it answers");
    }

    /// R12-REASON-9032: the seam that pushes the §27 exchange is
    /// `observe_reasoning`, and until this test nothing drove it with an
    /// important prediction — the cycle fixtures form a hypothesis below
    /// `IMPORTANCE_CONFIDENCE` and unapproved, so only the extracted
    /// `challenge_exchange` helper was proven and a kernel that never called
    /// it would have passed. An approved hypothesis with a directional claim
    /// opens a question, is attacked by the society, and the request and the
    /// result that answers it ride the review; the same hypothesis unapproved
    /// and under the threshold is not attacked and leaves no exchange.
    ///
    /// Mutation: delete `Ok(exchange) => self.review.challenges.push(exchange)`
    /// (replace with `Ok(_) => {}`) in `observe_reasoning` — the review holds
    /// no exchange and the first assertion fails. Verified and restored.
    #[test]
    fn an_important_prediction_journals_a_challenge_exchange_on_its_tournament_review() {
        use qip_core::ids::{EvidenceId, HypothesisId, ObjectId};
        use qip_investment_agents::chief::OrganisationReport;
        use qip_reasoning_engine::UncertaintyType;
        use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet};
        use qip_reasoning_engine::hypothesis::{
            CausalChain, CausalStep, Claim, HypothesisDraft, HypothesisStatus,
        };
        use qip_world_model::causal::Mechanism;

        let at = Timestamp::from_secs(1_760_000_000);
        let evidence = |id: &str, origin: &str| {
            Evidence::new(
                EvidenceId::from_string(id),
                EvidenceKind::Filing,
                Stance::Supports,
                format!("statement for {id}"),
                format!("rec-{id}"),
                origin,
                at,
                at,
            )
            .with_reliability(0.8)
            .with_diagnosticity(0.7)
        };
        let hypothesis = |status: HypothesisStatus| -> Hypothesis {
            let mut formed = Hypothesis::form(HypothesisDraft {
                hypothesis_id: HypothesisId::from_string("hyp-important"),
                opportunity_id: None,
                formed_at: at,
                as_of: at,
                class: "funding-cost-pass-through".to_string(),
                claim: Claim::Overvalued,
                statement: "ACME's margin guidance does not reflect its funding".to_string(),
                subjects: vec![ObjectId::from_string("obj-ACME")],
                chain: CausalChain::new(vec![CausalStep::new(
                    "funding-cost",
                    "obj-ACME",
                    Mechanism::CreditConditions,
                    "gross margin compresses",
                    Duration::from_days(20),
                    0.8,
                )]),
                evidence: EvidenceSet::from_items(vec![
                    evidence("e1", "macro-analyst"),
                    evidence("e2", "equity-analyst"),
                ]),
                prior: 0.25,
                uncertainty_type: UncertaintyType::Epistemic,
                falsifiers: vec!["the next report shows flat gross margin".to_string()],
                leading_alternative: "the market has priced the margin path".to_string(),
                horizon: Duration::from_days(60),
                contributors: Vec::new(),
                models: Vec::new(),
            })
            .expect("a well-formed draft forms");
            formed
                .apply_review(&qip_reasoning_engine::redteam::ReviewOutcome {
                    hypothesis_id: formed.hypothesis_id.as_str().to_string(),
                    reviewed_at: at,
                    challenges: Vec::new(),
                    confidence_before: 0.0,
                    confidence_after: 0.0,
                    verdict: status,
                    rationale: "a test's review".to_string(),
                })
                .expect("the test's review applies");
            formed
        };
        let report = OrganisationReport {
            as_of: at,
            question: "is ACME overvalued".to_string(),
            runs: Vec::new(),
            findings: Vec::new(),
            deferred: Vec::new(),
            failed: Vec::new(),
            missing_inputs: Vec::new(),
            backlogged: Vec::new(),
            backlog_refused: Vec::new(),
        };
        let closes: Vec<f64> = (0..40)
            .map(|i| 100.0 + (i as f64 * 0.37).sin() * 2.0 + i as f64 * 0.05)
            .collect();
        let roster = vec!["macro-analyst".to_string(), "equity-analyst".to_string()];
        let observe = |stage: &mut TournamentStage, hypothesis: &Hypothesis, approved: bool| {
            stage.observe_reasoning(
                ReasonObservation {
                    hypothesis,
                    report: &report,
                    roster: &roster,
                    regime: "calm".to_string(),
                    closes: &closes,
                    spread_ratio: Some(1.1),
                    crossed_recently: Some(false),
                    expected_move_bps: 200.0,
                    approved,
                    explaining: BTreeSet::new(),
                },
                at,
            )
        };

        // Premise: the same hypothesis, unapproved and under the threshold,
        // opens its question and is not attacked.
        let quiet = hypothesis(HypothesisStatus::Draft);
        assert!(
            quiet.effective_confidence() < IMPORTANCE_CONFIDENCE,
            "premise: {} is important on confidence alone",
            quiet.effective_confidence()
        );
        let mut unattacked = TournamentStage::new();
        observe(&mut unattacked, &quiet, false).expect("the question opens");
        // R12-MODEL-3090 (ADR 0149 decision 13): the detector enters with a
        // cost — the evidence items it examined — on the step meter, never
        // as money and never as an unmetered blank. Mutation: put back
        // `cost: None` for the detector, or meter it as `Spend`, and these
        // fail.
        assert_eq!(quiet.evidence.len(), 2, "premise: two evidence items");
        let detector = unattacked
            .tournament()
            .question(&format!("q-{}", quiet.hypothesis_id.as_str()))
            .and_then(|q| q.forecasts().get(&format!("detector:{}", quiet.class)))
            .expect("the detector's forecast is on its question");
        assert_eq!(detector.cost, Some(2.0));
        assert_eq!(detector.cost_meter, CostMeter::Steps);
        assert!(unattacked.review.challenges.is_empty());
        assert!(unattacked.review.verifications.is_empty());

        let important = hypothesis(HypothesisStatus::Approved);
        let mut stage = TournamentStage::new();
        let verdict = observe(&mut stage, &important, true).expect("the question opens");
        assert!(
            verdict.clause.contains("verifier society:"),
            "the society did not attack an approved prediction: {}",
            verdict.clause
        );
        assert_eq!(
            stage.review.challenges.len(),
            1,
            "{:?}",
            stage.review.refused
        );
        let exchange = &stage.review.challenges[0];
        assert_eq!(exchange.request.request_id, "challenge-q-hyp-important");
        exchange
            .result
            .answers(&exchange.request)
            .expect("the result answers the request it rides with");
        assert_eq!(stage.review.verifications.len(), 1);
        assert_eq!(
            matches!(exchange.result.verdict, ChallengeVerdict::Refuted { .. }),
            stage.review.verifications[0].verdict.is_refuted(),
            "the exchange and the verification disagree about the same attack"
        );
    }

    /// R12-CONTRACT-6112: a vote whose cost is not an amount is refused,
    /// not cast at zero — a free vote would win the cost factor it never
    /// paid — and a forecaster with no history has no standing. Mutation:
    /// default an unrepresentable cost to zero in `model_vote` and the first
    /// assertion fails.
    #[test]
    fn a_vote_with_a_cost_nobody_can_bill_or_no_history_is_refused() {
        let at = Timestamp::from_secs(1_760_000_000);
        let history = vec![ScorePoint {
            at,
            score: Decimal::from_f64(0.2).unwrap(),
        }];
        let vote = |history: Option<&Vec<ScorePoint>>, cost: f64| {
            model_vote(
                "analyst:a",
                "manifest-x",
                "AAA = up",
                6_000,
                "calm",
                history,
                cost,
                at,
            )
        };
        assert!(
            vote(Some(&history), 2_500.0).is_ok(),
            "premise: a billable vote is cast"
        );
        assert!(
            vote(Some(&history), f64::NAN).is_err(),
            "a vote with no cost was cast"
        );
        assert!(vote(Some(&history), f64::INFINITY).is_err());
        assert!(
            vote(None, 2_500.0).is_err(),
            "a vote with no history was cast"
        );
    }

    #[test]
    fn a_binary_view_always_sums_to_one() {
        for p in [0.0, 0.123_456, 0.5, 0.999_99, 1.0] {
            let view = binary(up_bp(p));
            assert_eq!(view.values().map(|bp| u64::from(*bp)).sum::<u64>(), 10_000);
        }
    }

    /// R12-REASON-3108, ADR 0170 decision 1: an approved prediction whose
    /// question the tournament refuses is refused, never sized unattacked.
    ///
    /// The failure this prevents: the refused-question arm returned no
    /// refutation, and the society runs only after the question opens, so an
    /// approved thesis the society never saw went on to DECIDE. A second
    /// hypothesis on one subject in one cycle reaches it in production,
    /// because both carry the id `hyp-<cycle>-<subject>`; here the same
    /// hypothesis is observed twice, which is that collision. Mutation:
    /// return `refuted: None` from the arm again — the second assertion
    /// fails.
    #[test]
    fn an_approved_prediction_whose_question_the_tournament_refuses_is_refused() {
        use qip_core::ids::{EvidenceId, HypothesisId, ObjectId};
        use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet};
        use qip_reasoning_engine::hypothesis::{
            CausalChain, CausalStep, Claim, HypothesisDraft, UncertaintyType,
        };
        use qip_world_model::causal::Mechanism;

        let at = Timestamp::from_secs(1_760_000_000);
        let hypothesis = Hypothesis::form(HypothesisDraft {
            hypothesis_id: HypothesisId::from_string("hyp-7-obj-AAA"),
            opportunity_id: None,
            formed_at: at,
            as_of: at,
            class: "mean-reversion".to_string(),
            claim: Claim::Undervalued,
            statement: "AAA fell further than its history explains".to_string(),
            subjects: vec![ObjectId::from_string("obj-AAA")],
            chain: CausalChain::new(vec![CausalStep::new(
                "order-flow",
                "obj-AAA",
                Mechanism::CreditConditions,
                "the dislocation closes as liquidity returns",
                Duration::from_days(2),
                0.8,
            )]),
            evidence: EvidenceSet::from_items(vec![
                Evidence::new(
                    EvidenceId::from_string("e1"),
                    EvidenceKind::MarketObservation,
                    Stance::Supports,
                    "a nine percent drop on the last bar".to_string(),
                    "rec-e1".to_string(),
                    "exchange",
                    at,
                    at,
                )
                .with_reliability(0.9)
                .with_diagnosticity(0.6),
            ]),
            prior: 0.4,
            uncertainty_type: UncertaintyType::Epistemic,
            falsifiers: vec!["the price keeps falling for a week".to_string()],
            leading_alternative: "the drop is news the tape has not finished pricing".to_string(),
            horizon: Duration::from_days(5),
            contributors: Vec::new(),
            models: Vec::new(),
        })
        .expect("a well-formed draft forms");
        let report = OrganisationReport {
            as_of: at,
            question: "is AAA undervalued".to_string(),
            runs: Vec::new(),
            findings: Vec::new(),
            deferred: Vec::new(),
            failed: Vec::new(),
            missing_inputs: Vec::new(),
            backlogged: Vec::new(),
            backlog_refused: Vec::new(),
        };
        let closes: Vec<f64> = (0..30).map(|i| 100.0 + f64::from(i % 3)).collect();
        let roster: Vec<String> = Vec::new();
        let observe = |stage: &mut TournamentStage, approved: bool| {
            stage
                .observe_reasoning(
                    ReasonObservation {
                        hypothesis: &hypothesis,
                        report: &report,
                        roster: &roster,
                        regime: "trend/calm".to_string(),
                        closes: &closes,
                        spread_ratio: Some(1.0),
                        crossed_recently: Some(false),
                        expected_move_bps: 100.0,
                        approved,
                        explaining: BTreeSet::new(),
                    },
                    at,
                )
                .expect("a directional claim on a subject is observed")
        };

        let mut stage = TournamentStage::new();
        let first = observe(&mut stage, true);
        assert!(
            !first.clause.contains("tournament refused the question"),
            "premise: the first question opens: {}",
            first.clause
        );
        let second = observe(&mut stage, true);
        assert!(
            second.clause.contains("tournament refused the question"),
            "premise: the colliding question is refused: {}",
            second.clause
        );
        let refusal = second
            .refuted
            .expect("an approved prediction the society could not attack was let through");
        assert!(refusal.starts_with("it could not be attacked"), "{refusal}");

        // An unapproved one is not vetoed for it: there is no thesis to stop.
        let unapproved = observe(&mut stage, false);
        assert!(
            unapproved
                .clause
                .contains("tournament refused the question")
        );
        assert_eq!(unapproved.refuted, None);
    }

    /// R12-REASON-3108: an important undirected forecast — a regime shift,
    /// which the standard detectors form — is attacked by the critics that
    /// need no side, and their refutation vetoes it; the bull and bear
    /// critics abstain rather than reading the panel's lean against a claim
    /// that never said which way. And an approved prediction whose subject
    /// has no close fails closed rather than reading as unrefuted.
    /// Mutations: restore `implied_sign()?` at the top of
    /// `observe_reasoning` — the regime shift is observed as `None` and the
    /// first expectation fails; drop the direction guard in
    /// `directional_attack` — the bull and bear critics stop abstaining;
    /// make `unattackable` return `None` — the empty-closes assertion fails.
    #[test]
    fn an_approved_regime_shift_is_attacked_without_a_side_and_a_refutation_vetoes_it() {
        use qip_core::ids::{EvidenceId, HypothesisId, ObjectId};
        use qip_learning_engine::verifier::{CriticKind, CriticVerdict};
        use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet};
        use qip_reasoning_engine::hypothesis::{
            CausalChain, CausalStep, Claim, HypothesisDraft, UncertaintyType,
        };
        use qip_world_model::causal::Mechanism;

        let at = Timestamp::from_secs(1_760_000_000);
        let hypothesis = Hypothesis::form(HypothesisDraft {
            hypothesis_id: HypothesisId::from_string("hyp-9-obj-AAA"),
            opportunity_id: None,
            formed_at: at,
            as_of: at,
            class: "regime".to_string(),
            claim: Claim::RegimeShift,
            statement: "AAA's volatility regime changed".to_string(),
            subjects: vec![ObjectId::from_string("obj-AAA")],
            chain: CausalChain::new(vec![CausalStep::new(
                "order-flow",
                "obj-AAA",
                Mechanism::CreditConditions,
                "liquidity withdrew",
                Duration::from_days(2),
                0.8,
            )]),
            evidence: EvidenceSet::from_items(vec![
                Evidence::new(
                    EvidenceId::from_string("e1"),
                    EvidenceKind::MarketObservation,
                    Stance::Supports,
                    "realised volatility doubled".to_string(),
                    "rec-e1".to_string(),
                    "exchange",
                    at,
                    at,
                )
                .with_reliability(0.9)
                .with_diagnosticity(0.6),
            ]),
            prior: 0.4,
            uncertainty_type: UncertaintyType::Epistemic,
            falsifiers: vec!["volatility reverts within the week".to_string()],
            leading_alternative: "one news day, not a regime".to_string(),
            horizon: Duration::from_days(5),
            contributors: Vec::new(),
            models: Vec::new(),
        })
        .expect("a well-formed draft forms");
        assert_eq!(
            hypothesis.claim.implied_sign(),
            None,
            "premise: the claim is undirected"
        );
        let report = OrganisationReport {
            as_of: at,
            question: "did AAA change regime".to_string(),
            runs: Vec::new(),
            findings: Vec::new(),
            deferred: Vec::new(),
            failed: Vec::new(),
            missing_inputs: Vec::new(),
            backlogged: Vec::new(),
            backlog_refused: Vec::new(),
        };
        let closes: Vec<f64> = (0..30).map(|i| 100.0 + f64::from(i % 3)).collect();
        let roster: Vec<String> = Vec::new();
        let observe = |stage: &mut TournamentStage, closes: &[f64], spread: f64| {
            stage.observe_reasoning(
                ReasonObservation {
                    hypothesis: &hypothesis,
                    report: &report,
                    roster: &roster,
                    regime: "trend/calm".to_string(),
                    closes,
                    spread_ratio: Some(spread),
                    crossed_recently: Some(false),
                    expected_move_bps: 100.0,
                    approved: true,
                    explaining: BTreeSet::new(),
                },
                at,
            )
        };

        let mut stage = TournamentStage::new();
        // No specialist formed a view, so the missing-variable critic — one
        // that needs no side — refutes it, and the refutation is the veto.
        let calm = observe(&mut stage, &closes, 1.0)
            .expect("an approved undirected forecast is attacked, not skipped");
        let veto = calm.refuted.expect("a refuted regime shift is vetoed");
        assert!(veto.contains("missing_variable"), "{veto}");
        assert!(
            !veto.contains("manipulation"),
            "premise: the spread is calm: {veto}"
        );
        let verification = stage
            .review
            .verifications
            .last()
            .expect("the attack is recorded");
        let verdict_of = |kind: CriticKind| {
            verification
                .findings
                .iter()
                .find(|f| f.critic == kind)
                .map(|f| f.verdict.clone())
                .expect("every critic answered")
        };
        for side in [CriticKind::Bull, CriticKind::Bear] {
            assert!(
                matches!(verdict_of(side), CriticVerdict::Abstained { .. }),
                "{side:?} argued a side the claim never took"
            );
        }
        assert!(
            matches!(
                verdict_of(CriticKind::StructuralBreak),
                CriticVerdict::Survived { .. }
            ),
            "the structural-break critic examined it"
        );

        let thin = observe(&mut stage, &closes, 1_000.0).expect("attacked");
        let veto = thin.refuted.expect("a refuted regime shift is vetoed");
        assert!(veto.contains("manipulation"), "{veto}");

        let unpriced = observe(&mut stage, &[], 1.0)
            .expect("an approved forecast with no close is refused, not skipped");
        let refusal = unpriced.refuted.expect("it was let through unattacked");
        assert!(refusal.starts_with("it could not be attacked"), "{refusal}");
    }

    /// R12-MODEL-6022, ADR 0149 decision 13's measurement: each LEARN that
    /// resolves a question whose weights were earned journals the
    /// influence-weighted aggregate's Brier score beside the equal-weight
    /// mean's, both as they stood when the question opened, and the record
    /// moves no weight, no allocation and no forecast.
    ///
    /// The failure this prevents: the influence weights and the compute
    /// allocation were journaled as if they meant something, and nothing
    /// measured whether weighing by reputation forecast better than weighing
    /// everyone alike — a governance record nobody could falsify. Mutation:
    /// score both aggregates with the same weights (`equal_up = aggregate_up`
    /// at opening) — the two Briers agree and the figures fail.
    #[test]
    fn a_measurement_is_journaled_and_reads_nothing_back() {
        use qip_core::ids::{EvidenceId, HypothesisId, ObjectId};
        use qip_market::bar::Interval;
        use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet};
        use qip_reasoning_engine::hypothesis::{
            CausalChain, CausalStep, Claim, HypothesisDraft, UncertaintyType,
        };
        use qip_world_model::causal::Mechanism;

        let at = Timestamp::from_secs(1_760_000_000);
        let hypothesis = |id: &str, formed: Timestamp| {
            Hypothesis::form(HypothesisDraft {
                hypothesis_id: HypothesisId::from_string(id),
                opportunity_id: None,
                formed_at: formed,
                as_of: formed,
                class: "mean-reversion".to_string(),
                claim: Claim::Undervalued,
                statement: "AAA fell further than its history explains".to_string(),
                subjects: vec![ObjectId::from_string("obj-AAA")],
                chain: CausalChain::new(vec![CausalStep::new(
                    "order-flow",
                    "obj-AAA",
                    Mechanism::CreditConditions,
                    "the dislocation closes as liquidity returns",
                    Duration::from_days(2),
                    0.8,
                )]),
                evidence: EvidenceSet::from_items(vec![
                    Evidence::new(
                        EvidenceId::from_string("e1"),
                        EvidenceKind::MarketObservation,
                        Stance::Supports,
                        "a nine percent drop on the last bar".to_string(),
                        "rec-e1".to_string(),
                        "exchange",
                        formed,
                        formed,
                    )
                    .with_reliability(0.9)
                    .with_diagnosticity(0.6),
                ]),
                prior: 0.4,
                uncertainty_type: UncertaintyType::Epistemic,
                falsifiers: vec!["the price keeps falling for a week".to_string()],
                leading_alternative: "the drop is news the tape has not finished pricing"
                    .to_string(),
                horizon: Duration::from_days(5),
                contributors: Vec::new(),
                models: Vec::new(),
            })
            .expect("a well-formed draft forms")
        };
        let report = OrganisationReport {
            as_of: at,
            question: "is AAA undervalued".to_string(),
            runs: Vec::new(),
            findings: Vec::new(),
            deferred: Vec::new(),
            failed: Vec::new(),
            missing_inputs: Vec::new(),
            backlogged: Vec::new(),
            backlog_refused: Vec::new(),
        };
        let closes: Vec<f64> = (0..30).map(|i| 100.0 + f64::from(i % 3)).collect();
        let reference = closes[closes.len() - 1];
        let roster: Vec<String> = Vec::new();
        let observe = |stage: &mut TournamentStage, hypothesis: &Hypothesis, now: Timestamp| {
            stage
                .observe_reasoning(
                    ReasonObservation {
                        hypothesis,
                        report: &report,
                        roster: &roster,
                        regime: "trend/calm".to_string(),
                        closes: &closes,
                        spread_ratio: Some(1.0),
                        crossed_recently: Some(false),
                        expected_move_bps: 100.0,
                        approved: false,
                        explaining: BTreeSet::new(),
                    },
                    now,
                )
                .expect("a directional claim on a subject is observed");
        };
        let settle = |opened: Timestamp, close: f64| {
            let px = |v: f64| Decimal::from_f64(v).expect("a price");
            BTreeMap::from([(
                "obj-AAA".to_string(),
                vec![Bar {
                    object_id: ObjectId::from_string("obj-AAA"),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: opened,
                    open: px(reference),
                    high: px(reference.max(close)),
                    low: px(reference.min(close)),
                    close: px(close),
                    volume: Decimal::from_int(1_000),
                    trade_count: 10,
                    vwap: None,
                    quality: qip_financial::quality::DataQuality::default(),
                }],
            )])
        };
        // Ten questions resolved the way the detector leans make it Active,
        // so its weight is earned; an eleventh resolved against it spawns a
        // temporary, so the next question carries two forecasts that differ.
        // Built by one closure so a second, identical stage can be poisoned.
        let day = Duration::from_days(1);
        let opened_at = |k: i64| at.saturating_add(Duration::from_days(6 * k));
        let settled_at = |k: i64| opened_at(k).saturating_add(Duration::from_days(5));
        let prepared = || {
            let mut stage = TournamentStage::new();
            let mut lean_up = None;
            for k in 0..11 {
                let id = format!("hyp-{k}-obj-AAA");
                observe(&mut stage, &hypothesis(&id, opened_at(k)), opened_at(k));
                let up = stage.review.opened[0]
                    .aggregate_up
                    .expect("premise: an aggregate");
                let leans_up = *lean_up.get_or_insert(up >= 0.5);
                let hit = k < 10;
                let close = if leans_up == hit {
                    reference * 1.1
                } else {
                    reference * 0.9
                };
                let review = stage
                    .learn(
                        &settle(settled_at(k), close),
                        None,
                        k as u64,
                        settled_at(k).saturating_add(day),
                    )
                    .expect("learns")
                    .expect("a resolution is a record");
                if k == 0 {
                    assert!(
                        review.aggregate_measurement.is_none(),
                        "a question nobody had earned weight on was measured as influence: {:?}",
                        review.aggregate_measurement
                    );
                }
            }
            assert!(
                stage.temporaries.contains_key("obj-AAA"),
                "premise: the eleventh, a miss, spawned a temporary"
            );
            observe(
                &mut stage,
                &hypothesis("hyp-11-obj-AAA", opened_at(11)),
                opened_at(11),
            );
            stage
        };
        let mut stage = prepared();
        let opened = stage.review.opened[0].clone();
        assert_eq!(opened.forecasts.len(), 2, "premise: {:?}", opened.forecasts);
        let stated: BTreeSet<u32> = opened.forecasts.values().copied().collect();
        assert_eq!(
            stated.len(),
            2,
            "premise: two distinct clusters, so pooling is the mean"
        );
        let pending = stage
            .pending
            .get(&opened.question)
            .expect("pending")
            .clone();
        assert!(
            pending.weights_earned,
            "premise: the detector's record earned it weight"
        );
        let weighted = opened.aggregate_up.expect("premise: an aggregate");
        let equal = opened
            .forecasts
            .values()
            .map(|bp| f64::from(*bp))
            .sum::<f64>()
            / 20_000.0;
        assert!(
            (weighted - equal).abs() > 1e-6,
            "premise: earned weights move the aggregate off the mean ({weighted} vs {equal})"
        );

        let before = stage.aggregate_tally.clone();
        let bars = settle(settled_at(11), reference * 1.1);
        let learned_at = settled_at(11).saturating_add(day);
        let review = stage
            .learn(&bars, None, 11, learned_at)
            .expect("learns")
            .expect("a resolution is a record");
        let measured = review
            .aggregate_measurement
            .clone()
            .expect("the cycle that scored an earned question journals the measurement");
        // This question's two scores join the earlier ones exactly once.
        let n = (before.questions + 1) as f64;
        assert_eq!(measured.questions, before.questions + 1);
        let expect_weighted = (before.weighted + 2.0 * (weighted - 1.0).powi(2)) / n;
        let expect_equal = (before.equal + 2.0 * (equal - 1.0).powi(2)) / n;
        assert!(
            (measured.weighted_brier - expect_weighted).abs() < 1e-12,
            "{measured:?}"
        );
        assert!(
            (measured.equal_brier - expect_equal).abs() < 1e-12,
            "{measured:?}"
        );

        // Reads nothing back: a stage whose tally says something absurd
        // resolves, weighs and forecasts exactly as this one does.
        let mut poisoned = prepared();
        poisoned.aggregate_tally = AggregateTally {
            questions: 1_000,
            weighted: 0.0,
            equal: 1_000.0,
        };
        let mut other = poisoned
            .learn(&bars, None, 11, learned_at)
            .expect("learns")
            .expect("a resolution is a record");
        assert_ne!(
            other.aggregate_measurement, review.aggregate_measurement,
            "premise"
        );
        other.aggregate_measurement = review.aggregate_measurement.clone();
        assert_eq!(
            other, review,
            "the measurement moved something it only records"
        );
        let next = opened_at(12);
        observe(&mut stage, &hypothesis("hyp-12-obj-AAA", next), next);
        observe(&mut poisoned, &hypothesis("hyp-12-obj-AAA", next), next);
        assert_eq!(stage.review.opened, poisoned.review.opened);
    }

    /// R12-MODEL-7198: a resolution the standing population badly missed
    /// spawns a temporary specialist on its subject, which then competes on
    /// that subject's next question with a forecast its own model made, and
    /// leaves at its expiry. One that the population called right spawns
    /// nothing.
    ///
    /// The failure this prevents: the tournament admitted
    /// `ForecasterKind::Temporary` and the world model carried a spawner, and
    /// neither had a caller — every entrant was the detector or an analyst,
    /// so the "thousands of specialist and temporary models" the tournament
    /// is capped for were none. Mutations: swap the residual's arms in
    /// `observe_outcome` — the miss spawns nothing and the hit spawns one;
    /// drop the live temporary from `observe_reasoning`'s entrants (`&&
    /// false` on its expiry filter) — the next question carries no
    /// temporary forecast.
    #[test]
    fn a_subject_the_population_missed_spawns_a_temporary_specialist_that_competes_until_it_expires()
     {
        use qip_core::ids::{EvidenceId, HypothesisId, ObjectId};
        use qip_market::bar::Interval;
        use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet};
        use qip_reasoning_engine::hypothesis::{
            CausalChain, CausalStep, Claim, HypothesisDraft, UncertaintyType,
        };
        use qip_world_model::causal::Mechanism;

        let at = Timestamp::from_secs(1_760_000_000);
        let hypothesis = |id: &str, formed: Timestamp| {
            Hypothesis::form(HypothesisDraft {
                hypothesis_id: HypothesisId::from_string(id),
                opportunity_id: None,
                formed_at: formed,
                as_of: formed,
                class: "mean-reversion".to_string(),
                claim: Claim::Undervalued,
                statement: "AAA fell further than its history explains".to_string(),
                subjects: vec![ObjectId::from_string("obj-AAA")],
                chain: CausalChain::new(vec![CausalStep::new(
                    "order-flow",
                    "obj-AAA",
                    Mechanism::CreditConditions,
                    "the dislocation closes as liquidity returns",
                    Duration::from_days(2),
                    0.8,
                )]),
                evidence: EvidenceSet::from_items(vec![
                    Evidence::new(
                        EvidenceId::from_string("e1"),
                        EvidenceKind::MarketObservation,
                        Stance::Supports,
                        "a nine percent drop on the last bar".to_string(),
                        "rec-e1".to_string(),
                        "exchange",
                        formed,
                        formed,
                    )
                    .with_reliability(0.9)
                    .with_diagnosticity(0.6),
                ]),
                prior: 0.4,
                uncertainty_type: UncertaintyType::Epistemic,
                falsifiers: vec!["the price keeps falling for a week".to_string()],
                leading_alternative: "the drop is news the tape has not finished pricing"
                    .to_string(),
                horizon: Duration::from_days(5),
                contributors: Vec::new(),
                models: Vec::new(),
            })
            .expect("a well-formed draft forms")
        };
        let report = OrganisationReport {
            as_of: at,
            question: "is AAA undervalued".to_string(),
            runs: Vec::new(),
            findings: Vec::new(),
            deferred: Vec::new(),
            failed: Vec::new(),
            missing_inputs: Vec::new(),
            backlogged: Vec::new(),
            backlog_refused: Vec::new(),
        };
        let closes: Vec<f64> = (0..30).map(|i| 100.0 + f64::from(i % 3)).collect();
        let reference = closes[closes.len() - 1];
        let roster: Vec<String> = Vec::new();
        let observe = |stage: &mut TournamentStage, hypothesis: &Hypothesis, now: Timestamp| {
            stage
                .observe_reasoning(
                    ReasonObservation {
                        hypothesis,
                        report: &report,
                        roster: &roster,
                        regime: "trend/calm".to_string(),
                        closes: &closes,
                        spread_ratio: Some(1.0),
                        crossed_recently: Some(false),
                        expected_move_bps: 100.0,
                        approved: false,
                        explaining: BTreeSet::new(),
                    },
                    now,
                )
                .expect("a directional claim on a subject is observed");
        };
        let settle = |opened: Timestamp, close: f64| {
            let px = |v: f64| Decimal::from_f64(v).expect("a price");
            BTreeMap::from([(
                "obj-AAA".to_string(),
                vec![Bar {
                    object_id: ObjectId::from_string("obj-AAA"),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: opened,
                    open: px(reference),
                    high: px(reference.max(close)),
                    low: px(reference.min(close)),
                    close: px(close),
                    volume: Decimal::from_int(1_000),
                    trade_count: 10,
                    vwap: None,
                    quality: qip_financial::quality::DataQuality::default(),
                }],
            )])
        };
        let temporary = format!("{TEMPORARY_PREFIX}obj-AAA");

        // The first question: the detector is the population.
        let mut stage = TournamentStage::new();
        observe(&mut stage, &hypothesis("hyp-1-obj-AAA", at), at);
        let aggregate = stage.review.opened[0]
            .aggregate_up
            .expect("premise: the question formed an aggregate");
        // Settle it the way the aggregate leaned least.
        let missed_up = aggregate < 0.5;
        let residual = if missed_up {
            1.0 - aggregate
        } else {
            aggregate
        };
        assert!(
            residual > SPAWN_RESIDUAL,
            "premise: the aggregate {aggregate} leans far enough to be missed"
        );
        let (miss, hit) = if missed_up {
            (reference * 1.1, reference * 0.9)
        } else {
            (reference * 0.9, reference * 1.1)
        };
        let resolved_at = at.saturating_add(Duration::from_days(6));

        // A population that called it right spawns nothing.
        let mut called = TournamentStage::new();
        observe(&mut called, &hypothesis("hyp-1-obj-AAA", at), at);
        let review = called
            .learn(
                &settle(at.saturating_add(Duration::from_days(5)), hit),
                None,
                2,
                resolved_at,
            )
            .expect("learns")
            .expect("a resolution is a record");
        assert_eq!(review.resolved.len(), 1, "premise: the question resolved");
        assert!(review.spawned.is_empty(), "{:?}", review.spawned);

        // The miss spawns one, under the standing population, on the
        // residual that triggered it.
        let review = stage
            .learn(
                &settle(at.saturating_add(Duration::from_days(5)), miss),
                None,
                2,
                resolved_at,
            )
            .expect("learns")
            .expect("a resolution is a record");
        assert_eq!(review.resolved.len(), 1, "premise: the question resolved");
        assert_eq!(review.spawned.len(), 1, "{review:?}");
        let branch = &review.spawned[0];
        assert_eq!(branch.parent_id, STANDING_POPULATION);
        assert_eq!(branch.id, format!("{temporary}@2"));
        assert!(branch.trigger_residual > branch.insufficiency_threshold);

        // It competes on the subject's next question, at its own model's
        // forecast: one outcome observed, Laplace-smoothed.
        // A minute after the spawn: in a cycle REASON opens before LEARN
        // spawns, so a question opened at the spawn's own instant opened
        // before it and is not the branch's (ADR 0347).
        let next = resolved_at.saturating_add(Duration::from_mins(1));
        observe(&mut stage, &hypothesis("hyp-3-obj-AAA", next), next);
        let opened = &stage.review.opened[0];
        let opened_aggregate = opened.aggregate_up;
        let expected = if missed_up { 6_667 } else { 3_333 };
        assert_eq!(
            opened.forecasts.get(&temporary),
            Some(&expected),
            "{:?} / {:?}",
            opened.forecasts,
            stage.review.refused
        );
        assert!(stage.tournament.is_admitted(&temporary));
        // Its question resolves, so it holds a record of its own.
        let review = stage
            .learn(
                &settle(next.saturating_add(Duration::from_days(5)), hit),
                None,
                3,
                next.saturating_add(Duration::from_days(6)),
            )
            .expect("learns")
            .expect("a resolution is a record");
        assert_eq!(review.resolved.len(), 1, "premise: its question resolved");
        assert!(
            stage.scores.contains_key(&temporary),
            "premise: the temporary was scored"
        );
        // ADR 0347 (R12-EXPAND-6043): a spawned branch's Brier is journaled
        // beside its parent's on later resolutions, and moves nothing. The
        // branch forecast 6,667 or 3,333 bp; the parent's aggregate on the
        // same question is what it opened with. Mutation: score the branch
        // with the parent's forecast — the two figures agree and the last
        // assertion fails.
        let own = review.resolved[0]
            .scores
            .iter()
            .find(|s| s.forecaster == temporary)
            .and_then(|s| s.brier)
            .expect("premise: the temporary's forecast was scored");
        let parent_up = opened_aggregate.expect("premise: the question had an aggregate");
        let y = if missed_up { 0.0 } else { 1.0 };
        assert_eq!(review.branch_measurements.len(), 1, "{review:?}");
        let measured = &review.branch_measurements[0];
        assert_eq!(measured.branch, format!("{temporary}@2"));
        assert_eq!(measured.forecaster, temporary);
        assert_eq!(measured.parent, STANDING_POPULATION);
        assert_eq!(measured.questions, 1);
        assert!((measured.branch_brier - own).abs() < 1e-12, "{measured:?}");
        assert!(
            (measured.parent_brier - 2.0 * (parent_up - y).powi(2)).abs() < 1e-12,
            "{measured:?}"
        );
        assert!(
            (measured.branch_brier - measured.parent_brier).abs() > 1e-6,
            "the branch was scored with its parent's forecast: {measured:?}"
        );

        // And leaves at its expiry: retired, and absent from the next
        // question.
        let expired = resolved_at.saturating_add(TEMPORARY_LIFETIME);
        let review = stage
            .learn(&BTreeMap::new(), None, 4, expired)
            .expect("learns")
            .expect("the retirement is a record");
        assert!(review.retired.contains(&temporary), "{review:?}");
        assert!(
            !stage.scores.contains_key(&temporary),
            "a retired temporary's score history outlived it, so a respawn would vote on it"
        );
        observe(&mut stage, &hypothesis("hyp-5-obj-AAA", expired), expired);
        assert!(
            !stage.review.opened[0].forecasts.contains_key(&temporary),
            "{:?}",
            stage.review.opened[0].forecasts
        );
    }

    /// ADR 0347's respawn guard: a respawn reuses the forecaster id, so a
    /// question opened at or before the spawn's instant carries the
    /// predecessor's forecast under that id and must not be scored as the
    /// new branch's. Only a question opened strictly after counts.
    /// Mutation: `>=` in place of `>` in `measure_branch`'s opened filter
    /// makes the same-instant arm record a measurement and the first
    /// assertion fail.
    #[test]
    fn a_question_opened_at_or_before_the_spawn_is_not_measured_as_the_branchs_own() {
        let spawned_at = Timestamp::from_secs(1_760_000_000);
        let id = format!("{TEMPORARY_PREFIX}obj-AAA");
        let mut stage = TournamentStage::new();
        stage.temporaries.insert(
            "obj-AAA".to_string(),
            TemporarySpecialist {
                id: id.clone(),
                expires_at: spawned_at.saturating_add(TEMPORARY_LIFETIME),
                ups: 1,
                observed: 1,
                branch: format!("{id}@2"),
                spawned_at,
                measured: BranchTally::default(),
            },
        );
        let pending = |opened_at: Option<Timestamp>| PendingQuestion {
            subject: "obj-AAA".to_string(),
            reference: 100.0,
            resolves_at: spawned_at.saturating_add(Duration::from_days(5)),
            opened_at,
            aggregate_up: Some(0.3),
            equal_up: None,
            weights_earned: false,
        };
        let scores = vec![ScoredForecast {
            forecaster: id,
            forecast_id: "f-1".to_string(),
            question: "q-1".to_string(),
            regime: "trend/calm".to_string(),
            outcome: UP.to_string(),
            brier: Some(0.5),
            log_loss: None,
            hit: None,
            tail: None,
            pinball: None,
            economic: None,
            information_value: None,
            calibration_points: Vec::new(),
            cost: None,
            cost_meter: CostMeter::default(),
            latency_ms: None,
        }];
        let now = spawned_at.saturating_add(Duration::from_days(1));

        // Premise: nothing is measured yet.
        assert!(stage.review.branch_measurements.is_empty());
        stage.measure_branch(&pending(Some(spawned_at)), true, &scores, now);
        stage.measure_branch(&pending(None), true, &scores, now);
        assert!(
            stage.review.branch_measurements.is_empty(),
            "a question opened at the spawn, or with no opening, was counted: {:?}",
            stage.review.branch_measurements
        );

        let after = spawned_at.saturating_add(Duration::from_mins(1));
        stage.measure_branch(&pending(Some(after)), true, &scores, now);
        assert_eq!(stage.review.branch_measurements.len(), 1);
        assert_eq!(stage.review.branch_measurements[0].questions, 1);
    }

    /// A one-bar hypothesis on `obj-AAA` and the pieces a question needs,
    /// for the card-entrant tests (ADR 0390).
    fn card_fixture(
        id: &str,
        formed: Timestamp,
        horizon: Duration,
    ) -> (Hypothesis, OrganisationReport, Vec<f64>) {
        use qip_core::ids::{EvidenceId, HypothesisId, ObjectId};
        use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet};
        use qip_reasoning_engine::hypothesis::{
            CausalChain, CausalStep, Claim, HypothesisDraft, UncertaintyType,
        };
        use qip_world_model::causal::Mechanism;

        let hypothesis = Hypothesis::form(HypothesisDraft {
            hypothesis_id: HypothesisId::from_string(id),
            opportunity_id: None,
            formed_at: formed,
            as_of: formed,
            class: "mean-reversion".to_string(),
            claim: Claim::Undervalued,
            statement: "AAA fell further than its history explains".to_string(),
            subjects: vec![ObjectId::from_string("obj-AAA")],
            chain: CausalChain::new(vec![CausalStep::new(
                "order-flow",
                "obj-AAA",
                Mechanism::CreditConditions,
                "the dislocation closes as liquidity returns",
                Duration::from_days(2),
                0.8,
            )]),
            evidence: EvidenceSet::from_items(vec![
                Evidence::new(
                    EvidenceId::from_string("e1"),
                    EvidenceKind::MarketObservation,
                    Stance::Supports,
                    "a nine percent drop on the last bar".to_string(),
                    "rec-e1".to_string(),
                    "exchange",
                    formed,
                    formed,
                )
                .with_reliability(0.9)
                .with_diagnosticity(0.6),
            ]),
            prior: 0.4,
            uncertainty_type: UncertaintyType::Epistemic,
            falsifiers: vec!["the price keeps falling for a week".to_string()],
            leading_alternative: "the drop is news the tape has not finished pricing".to_string(),
            horizon,
            contributors: Vec::new(),
            models: Vec::new(),
        })
        .expect("a well-formed draft forms");
        let report = OrganisationReport {
            as_of: formed,
            question: "is AAA undervalued".to_string(),
            runs: Vec::new(),
            findings: Vec::new(),
            deferred: Vec::new(),
            failed: Vec::new(),
            missing_inputs: Vec::new(),
            backlogged: Vec::new(),
            backlog_refused: Vec::new(),
        };
        let closes: Vec<f64> = (0..30).map(|i| 100.0 + f64::from(i % 3)).collect();
        (hypothesis, report, closes)
    }

    fn observe_card_question(
        stage: &mut TournamentStage,
        id: &str,
        horizon: Duration,
        now: Timestamp,
    ) -> f64 {
        let (hypothesis, report, closes) = card_fixture(id, now, horizon);
        let roster: Vec<String> = Vec::new();
        stage
            .observe_reasoning(
                ReasonObservation {
                    hypothesis: &hypothesis,
                    report: &report,
                    roster: &roster,
                    regime: "trend/calm".to_string(),
                    closes: &closes,
                    spread_ratio: Some(1.0),
                    crossed_recently: Some(false),
                    expected_move_bps: 100.0,
                    approved: false,
                    explaining: BTreeSet::new(),
                },
                now,
            )
            .expect("a directional claim on a subject is observed");
        closes[closes.len() - 1]
    }

    fn card(reference: &str, ups: u32, rows: u32, close: f64, at: Timestamp) -> CardForecast {
        CardForecast {
            reference: reference.to_string(),
            subject: "obj-AAA".to_string(),
            holdout_ups: ups,
            holdout_rows: rows,
            from_close: close,
            from_close_time: at,
            bar: Duration::from_days(1),
        }
    }

    /// R12-MODEL-3089, ADR 0390. Until this, the stage admitted a detector,
    /// an analyst and a temporary specialist and no fitted card, so the
    /// learning desk's models never competed and `verify_model` read a
    /// `model:<reference>` record nothing could have written.
    #[test]
    fn a_fitted_card_answers_its_own_instruments_one_bar_question_from_its_held_out_error_and_is_scored()
     {
        use qip_core::ids::ObjectId;
        use qip_market::bar::Interval;

        let at = Timestamp::from_secs(1_760_000_000);
        let day = Duration::from_days(1);
        let reference = 100.0 + f64::from(29 % 3);
        let mut stage = TournamentStage::new();
        // Seven of eight held-out errors leave the next return above zero.
        let refused = stage.stage_cards("obj-AAA", vec![card("ridge@3", 7, 8, reference, at)]);
        assert!(refused.is_empty(), "{refused:?}");
        let measured_from = observe_card_question(&mut stage, "hyp-1-obj-AAA", day, at);
        assert_eq!(
            measured_from.to_bits(),
            reference.to_bits(),
            "premise: the card forecasts from the close the question measures from"
        );
        let opened = &stage.review.opened[0];
        assert!(
            opened.forecasts.contains_key("detector:mean-reversion"),
            "premise: the detector is on the question too: {:?}",
            opened.forecasts
        );
        // (7 + 1) / (8 + 2) = 0.8, in basis points.
        assert_eq!(
            opened.forecasts.get("model:ridge@3").copied(),
            Some(8_000),
            "{:?} {:?}",
            opened.forecasts,
            stage.review.refused
        );
        assert!(
            stage.tournament.is_admitted("model:ridge@3"),
            "the card was not admitted as a forecaster"
        );

        // The next bar closes down: the card said 0.8 up and is scored on it.
        let px = |v: f64| Decimal::from_f64(v).expect("a price");
        let resolved_at = at.saturating_add(day);
        let bars = BTreeMap::from([(
            "obj-AAA".to_string(),
            vec![Bar {
                object_id: ObjectId::from_string("obj-AAA"),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: px(reference),
                high: px(reference),
                low: px(reference - 2.0),
                close: px(reference - 2.0),
                volume: Decimal::from_int(1_000),
                trade_count: 10,
                vwap: None,
                quality: qip_financial::quality::DataQuality::default(),
            }],
        )]);
        let review = stage
            .learn(&bars, None, 2, resolved_at)
            .expect("learns")
            .expect("a resolution is a record");
        let score = review.resolved[0]
            .scores
            .iter()
            .find(|score| score.forecaster == "model:ridge@3")
            .expect("the card is scored on the question it answered");
        // Brier over two outcomes: 2 * (0.8 - 0)^2.
        assert!(
            (score
                .brier
                .expect("a probabilistic forecast has a Brier score")
                - 1.28)
                .abs()
                < 1e-9,
            "{score:?}"
        );
        assert!(
            stage.model_record("model:ridge@3", None).is_some(),
            "verify_model reads this record and found none"
        );
    }

    /// ADR 0390 decisions 3 and 5: a card answers the question its fit is
    /// about and no other, and leaves when the desk stops offering it. A
    /// card entered on a five-day question, from a stale bar or from another
    /// close would be scored on something it never forecast, and its record
    /// would then weigh on its own promotion review.
    #[test]
    fn a_card_is_not_entered_on_a_question_its_fit_does_not_describe_and_leaves_when_no_longer_offered()
     {
        let at = Timestamp::from_secs(1_760_000_000);
        let day = Duration::from_days(1);
        let reference = 100.0 + f64::from(29 % 3);
        let entered = |stage: &TournamentStage| {
            stage.review.opened[0]
                .forecasts
                .contains_key("model:ridge@3")
        };

        // Premise: the same card on a one-bar question from its own close
        // is entered, so each refusal below is the mismatch and not a card
        // that never enters.
        let mut stage = TournamentStage::new();
        stage.stage_cards("obj-AAA", vec![card("ridge@3", 7, 8, reference, at)]);
        observe_card_question(&mut stage, "hyp-0-obj-AAA", day, at);
        assert!(entered(&stage), "premise: {:?}", stage.review.refused);

        // A five-day question is not the next bar's.
        let mut stage = TournamentStage::new();
        stage.stage_cards("obj-AAA", vec![card("ridge@3", 7, 8, reference, at)]);
        observe_card_question(&mut stage, "hyp-1-obj-AAA", Duration::from_days(5), at);
        assert!(!entered(&stage), "entered on a five-day question");
        assert!(
            stage
                .review
                .refused
                .iter()
                .any(|line| line.contains("model:ridge@3") && line.contains("not entered")),
            "the review does not say the card was withheld: {:?}",
            stage.review.refused
        );

        // Another close is another question.
        let mut stage = TournamentStage::new();
        stage.stage_cards("obj-AAA", vec![card("ridge@3", 7, 8, reference + 1.0, at)]);
        observe_card_question(&mut stage, "hyp-2-obj-AAA", day, at);
        assert!(
            !entered(&stage),
            "entered from a close the question does not measure from"
        );

        // A forecast older than its own bar has left, and is not kept.
        let mut stage = TournamentStage::new();
        stage.stage_cards("obj-AAA", vec![card("ridge@3", 7, 8, reference, at)]);
        observe_card_question(&mut stage, "hyp-3-obj-AAA", day, at.saturating_add(day));
        assert!(
            !entered(&stage),
            "entered from a bar that is no longer the newest"
        );
        assert_eq!(stage.standing_cards("obj-AAA").count(), 0);

        // The desk's next offer replaces the set: a card it leaves out is gone.
        let mut stage = TournamentStage::new();
        stage.stage_cards("obj-AAA", vec![card("ridge@3", 7, 8, reference, at)]);
        stage.stage_cards("obj-AAA", Vec::new());
        observe_card_question(&mut stage, "hyp-4-obj-AAA", day, at);
        assert!(
            !entered(&stage),
            "a card the desk stopped offering still competes"
        );

        // An offer that is not a forecast is refused, never repaired.
        let mut stage = TournamentStage::new();
        let mut other = card("ridge@3", 7, 8, reference, at);
        other.subject = "obj-BBB".to_string();
        let refused = stage.stage_cards(
            "obj-AAA",
            vec![
                card("none@1", 0, 0, reference, at),
                card("over@1", 9, 8, reference, at),
                other,
            ],
        );
        assert_eq!(refused.len(), 3, "{refused:?}");
        assert_eq!(stage.standing_cards("obj-AAA").count(), 0);
    }
}
