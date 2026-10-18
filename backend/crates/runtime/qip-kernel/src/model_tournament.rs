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
//! Detectors are not metered, so their forecasts carry zero cost and zero
//! latency. That favours them on those two factors and is stated here so the
//! factor is not read as measured for them.
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
//! created.

use qip_agents::finding::FindingStatus;
use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_investment_agents::chief::OrganisationReport;
use qip_learning_engine::tournament::{
    ForecasterKind, ForecasterSpec, InfluenceWeight, LatticeHorizon, PromotionDecision,
    QuestionSpec, ResearchTask, Resolution, ResolvedQuestion, Scorecard, Tournament,
    TournamentForecast,
};
use qip_learning_engine::verifier::{
    EvidenceItem, ModelDossier, ModelRecord, PanelView, PredictionDossier, Verification,
    VerifierSociety,
};
use qip_market::bar::Bar;
use qip_reasoning_engine::evidence::Stance;
use qip_reasoning_engine::hypothesis::Hypothesis;
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingQuestion {
    subject: String,
    reference: f64,
    resolves_at: Timestamp,
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

/// The tournament's cycle, as the log keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TournamentReview {
    pub cycle: u64,
    pub at: Option<Timestamp>,
    pub opened: Vec<OpenedQuestion>,
    pub refused: Vec<String>,
    pub verifications: Vec<Verification>,
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
            && self.research.is_empty()
            && self.resolved.is_empty()
            && self.abandoned.is_empty()
            && self.promotions.is_empty()
            && self.retired.is_empty()
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

    pub fn tournament(&self) -> &Tournament {
        &self.tournament
    }

    pub fn society(&self) -> &VerifierSociety {
        &self.society
    }

    /// Forecasters the society has refused promotion to, with the reason.
    pub fn refuted(&self) -> &BTreeMap<String, String> {
        &self.refuted
    }

    /// Open a question on one hypothesis, enter every forecast on it,
    /// review the disagreement, and — where the prediction is important —
    /// put it to the verifier society. Returns a clause for REASON's detail,
    /// or `None` when the hypothesis has no directional claim to ask about.
    pub fn observe_reasoning(
        &mut self,
        observation: ReasonObservation<'_>,
        now: Timestamp,
    ) -> Option<String> {
        let hypothesis = observation.hypothesis;
        let sign = hypothesis.claim.implied_sign()?;
        let subject = hypothesis.subjects.first()?.as_str().to_string();
        let reference = *observation.closes.last()?;
        if !(reference.is_finite() && reference > 0.0) {
            return None;
        }
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
            return Some(format!(
                "; tournament refused the question: {}",
                error.message()
            ));
        }
        self.last_regime = Some(observation.regime.clone());
        let band = LatticeHorizon::of(hypothesis.horizon);
        let mut entries: Vec<TournamentForecast> = Vec::new();
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
            cost: 0.0,
            latency: Duration::from_nanos(0),
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
            entries.push(TournamentForecast {
                forecaster: format!("analyst:{}", finding.agent_id),
                forecast_id: format!("{question}/analyst:{}", finding.agent_id),
                target: target.clone(),
                issued_at: now,
                probabilities_bp: Some(binary(up_bp(p_up))),
                quantiles: move_quantiles(p_up, observation.expected_move_bps, &rets),
                cost: run.map_or(0.0, |run| run.spend.cost_micros as f64),
                latency: run.map_or(Duration::from_nanos(0), |run| run.duration()),
            });
        }
        let mut forecasts = BTreeMap::new();
        for entry in entries {
            let id = entry.forecaster.clone();
            let up = entry
                .probabilities_bp
                .as_ref()
                .and_then(|p| p.get(UP).copied())
                .unwrap_or(0);
            let admitted = self.tournament.admit(
                ForecasterSpec {
                    id: id.clone(),
                    kind: ForecasterKind::Specialist,
                    horizon: band,
                    lane: band.admitted_lane(),
                },
                now,
            );
            let submitted = admitted.and_then(|()| self.tournament.submit(&question, entry, now));
            match submitted {
                Ok(()) => {
                    forecasts.insert(id, up);
                }
                Err(error) => self
                    .review
                    .refused
                    .push(format!("{question}/{id}: {}", error.message())),
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

        // The society, for important predictions only.
        let important = observation.approved || confidence >= IMPORTANCE_CONFIDENCE;
        let mut society_clause = String::new();
        if important {
            let dossier = self.prediction_dossier(&observation, sign, &subject, &forecasts, &rets);
            let verification = self.society.attack_prediction(&dossier, now);
            if verification.verdict.is_refuted() {
                let _ = self
                    .tournament
                    .hold_out(&question, verification.verdict.summary());
            }
            society_clause = format!("; verifier society: {}", verification.verdict.summary());
            self.review.verifications.push(verification);
        }
        self.pending.insert(
            question.clone(),
            PendingQuestion {
                subject,
                reference,
                resolves_at,
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
        Some(format!(
            "; tournament question opened with {} forecast(s){society_clause}",
            forecasts.len()
        ))
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
            .map(|f| PanelView {
                agent: f.agent_id.clone(),
                direction: f.direction.sign(),
                conviction: f.effective_conviction(),
            })
            .collect();
        let window = &rets[rets.len().saturating_sub(TAIL_WINDOW)..];
        let adverse_tail_bps = if window.is_empty() {
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
            missing_inputs: observation.report.missing_inputs.clone(),
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

    #[test]
    fn a_binary_view_always_sums_to_one() {
        for p in [0.0, 0.123_456, 0.5, 0.999_99, 1.0] {
            let view = binary(up_bp(p));
            assert_eq!(view.values().map(|bp| u64::from(*bp)).sum::<u64>(), 10_000);
        }
    }
}
