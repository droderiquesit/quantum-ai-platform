//! The adversarial verifier society (blueprint §9.11; R12-MODEL-7201,
//! R12-REASON-3108, -3109).
//!
//! A conclusion earns authority only by surviving an attack from somebody
//! who did not produce it. The platform's `RedTeam` already runs twelve
//! structural checks inside the reasoning engine — the same engine that wrote
//! the hypothesis — so it is a self-review, however rigorous. This society is
//! the independent half: eight critics, each named for the failure §9.11
//! says it hunts, each handed the same [`PredictionDossier`] or
//! [`ModelDossier`] and nothing else — not each other's verdicts, not the
//! author's rationale — and each able to refute.
//!
//! # What a verdict means
//!
//! * [`CriticVerdict::Refuted`] — the critic found the failure it hunts, and
//!   says what it found. One refutation refutes the target.
//! * [`CriticVerdict::Survived`] — the critic examined the target and found
//!   nothing.
//! * [`CriticVerdict::Abstained`] — the dossier lacked what the critic needs.
//!   **Never read as survival.** A society in which most critics abstained has
//!   not verified anything, and [`SocietyVerdict::Unverified`] says so rather
//!   than reporting a pass nobody earned.
//!
//! # Everything is evidence the platform already holds
//!
//! No critic invents a figure. Each reads a field of the dossier, compares it
//! against a stated constant, and a field the caller could not fill is an
//! abstention. The thresholds are named below so a reviewer can argue with
//! them.
//!
//! # Advisory, except at promotion
//!
//! The society never touches an order. Its verdict on a prediction holds the
//! question out of the forecast market's aggregate; its verdict on a model
//! blocks that model's promotion — in the tournament and at the model
//! registry. Blocking a promotion is a refusal to *add* authority, never a
//! way to create it.

use crate::tournament::{Measure, Scorecard};
use qip_contracts::challenge::{
    ChallengeRequest, ChallengeRequestFields, ChallengeTarget, ChallengeVerdict, CriticOutcome,
    CriticReport, VerificationResult, VerificationResultFields,
};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Critics that must examine (not abstain) before a survival is a verdict.
pub const QUORUM: usize = 5;

/// A spread this many times its own median makes the price an opinion — the
/// kernel's own illiquid-regime multiple.
pub const MANIPULATION_SPREAD_MULTIPLE: f64 = 3.0;

/// An adverse move seen in the window this many times the claimed edge
/// makes the claim a bet against its own tail.
pub const TAIL_MULTIPLE: f64 = 2.0;

/// Predicted failure probability at which a forecaster's view is a model
/// risk rather than a contribution.
pub const MODEL_FAILURE_BAR: f64 = 0.5;

/// Share of supporting weight from one origin above which the support is one
/// source counted several times.
pub const SINGLE_ORIGIN_SHARE: f64 = 0.8;

/// Panel coverage below which the explanation was formed without most of the
/// specialists the question needed.
pub const MINIMUM_COVERAGE: f64 = 0.5;

/// Resolved forecasts per direction a directional-bias finding needs.
pub const BIAS_SAMPLE: usize = 10;

/// Hit-rate gap between up and down outcomes that is a bias, not noise.
pub const BIAS_GAP: f64 = 0.3;

/// Tail Brier above which a model is worse than a coin flip in the tails.
pub const TAIL_BRIER_BAR: f64 = 0.5;

/// The eight critics §9.11 names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriticKind {
    Bull,
    Bear,
    StructuralBreak,
    Manipulation,
    TailRisk,
    ModelRisk,
    DataPoisoning,
    MissingVariable,
}

impl CriticKind {
    pub const ALL: [Self; 8] = [
        Self::Bull,
        Self::Bear,
        Self::StructuralBreak,
        Self::Manipulation,
        Self::TailRisk,
        Self::ModelRisk,
        Self::DataPoisoning,
        Self::MissingVariable,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bull => "bull",
            Self::Bear => "bear",
            Self::StructuralBreak => "structural_break",
            Self::Manipulation => "manipulation",
            Self::TailRisk => "tail_risk",
            Self::ModelRisk => "model_risk",
            Self::DataPoisoning => "data_poisoning",
            Self::MissingVariable => "missing_variable",
        }
    }
}

/// One critic's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum CriticVerdict {
    Survived { examined: String },
    Refuted { reason: String },
    Abstained { missing: String },
}

/// One critic's verdict, attributed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriticFinding {
    pub critic: CriticKind,
    pub verdict: CriticVerdict,
}

/// The society's verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum SocietyVerdict {
    /// At least [`QUORUM`] critics examined it and none refuted it.
    Survived,
    /// At least one critic refuted it.
    Refuted {
        by: Vec<CriticKind>,
        reasons: Vec<String>,
    },
    /// No critic refuted it, and too few could examine it to call that a
    /// survival.
    Unverified { examined: usize },
}

impl SocietyVerdict {
    pub const fn is_refuted(&self) -> bool {
        matches!(self, Self::Refuted { .. })
    }

    /// One line, for a refusal message or a stage detail.
    pub fn summary(&self) -> String {
        match self {
            Self::Survived => "survived the verifier society".to_string(),
            Self::Refuted { by, reasons } => format!(
                "refuted by the {} critic(s): {}",
                by.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", "),
                reasons.join("; ")
            ),
            Self::Unverified { examined } => format!(
                "unverified: only {examined} of {} critics could examine it (quorum {QUORUM})",
                CriticKind::ALL.len()
            ),
        }
    }
}

/// What was attacked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Target {
    Prediction {
        hypothesis_id: String,
        subject: String,
    },
    Model {
        reference: String,
    },
}

/// The full record of one attack, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub target: Target,
    pub at: Timestamp,
    pub findings: Vec<CriticFinding>,
    pub verdict: SocietyVerdict,
}

impl Verification {
    /// This record as the §27 [`VerificationResult`] answering `request_id`.
    ///
    /// The contract cross-checks the verdict against the findings, so a
    /// society whose `conclude` ever disagreed with its own critics would be
    /// refused here rather than released.
    pub fn as_result(&self, request_id: &str) -> Result<VerificationResult> {
        let target = match &self.target {
            Target::Prediction {
                hypothesis_id,
                subject,
            } => ChallengeTarget::Prediction {
                hypothesis_id: hypothesis_id.clone(),
                subject: subject.clone(),
            },
            Target::Model { reference } => ChallengeTarget::Model {
                reference: reference.clone(),
            },
        };
        let reports = self
            .findings
            .iter()
            .map(|finding| CriticReport {
                critic: finding.critic.as_str().to_string(),
                outcome: match &finding.verdict {
                    CriticVerdict::Survived { examined } => CriticOutcome::Survived {
                        examined: examined.clone(),
                    },
                    CriticVerdict::Refuted { reason } => CriticOutcome::Refuted {
                        reason: reason.clone(),
                    },
                    CriticVerdict::Abstained { missing } => CriticOutcome::Abstained {
                        missing: missing.clone(),
                    },
                },
            })
            .collect();
        let verdict = match &self.verdict {
            SocietyVerdict::Survived => ChallengeVerdict::Survived,
            SocietyVerdict::Refuted { by, .. } => ChallengeVerdict::Refuted {
                by: by.iter().map(|kind| kind.as_str().to_string()).collect(),
            },
            SocietyVerdict::Unverified { examined } => ChallengeVerdict::Unverified {
                examined: u32::try_from(*examined).map_err(|_| {
                    Error::invalid("examined count does not fit the contract's u32")
                })?,
            },
        };
        VerificationResult::new(VerificationResultFields {
            schema_version: 1,
            request_id: request_id.to_string(),
            target,
            reports,
            verdict,
            concluded_at: self.at,
        })
    }
}

/// One item of evidence, as the critics see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub origin: String,
    pub supports: bool,
    /// Reliability times diagnosticity: how much the item moved the belief.
    pub weight: f64,
    pub known_at: Timestamp,
}

/// One specialist's view on the question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelView {
    pub agent: String,
    /// `+1`, `-1` or `0`.
    pub direction: f64,
    pub conviction: f64,
}

/// Everything the critics may read about a prediction, captured when it was
/// made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PredictionDossier {
    pub hypothesis_id: String,
    pub subject: String,
    /// `+1` for a claim the subject rises, `-1` for a fall.
    pub direction: f64,
    pub confidence: f64,
    /// The instant the hypothesis reasons as of.
    pub as_of: Timestamp,
    pub expected_move_bps: f64,
    pub evidence: Vec<EvidenceItem>,
    pub panel: Vec<PanelView>,
    pub missing_inputs: Vec<String>,
    /// Share of the specialists asked that produced an informative view.
    pub coverage: f64,
    /// Whether the subject crossed a regime boundary recently, where the
    /// platform tracks its regime at all.
    pub regime_crossed_recently: Option<bool>,
    /// Latest quoted spread over its own median, where measured.
    pub spread_ratio: Option<f64>,
    /// The largest single adverse move in the recent window, in basis points
    /// against the claim's direction, where measured.
    pub adverse_tail_bps: Option<f64>,
    /// The meta-model's predicted failure probability, in the regime in force,
    /// for each forecaster behind the claim.
    pub forecaster_failure: BTreeMap<String, f64>,
}

/// A model's record in the tournament, where it competes there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRecord {
    pub scorecards: Vec<Scorecard>,
    /// Hit rate and sample on questions that resolved up, and on those that
    /// did not.
    pub up_hits: Option<(f64, usize)>,
    pub down_hits: Option<(f64, usize)>,
    pub predicted_failure_now: f64,
}

/// Everything the critics may read about a model at promotion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelDossier {
    pub reference: String,
    pub evaluated_at: Option<Timestamp>,
    pub evaluation_passed: Option<bool>,
    pub evaluation_validity: Duration,
    pub drift_score: f64,
    pub drift_threshold: f64,
    pub training_datasets: Vec<String>,
    /// Datasets the catalogue holds quarantined, and every dataset produced
    /// from one. Matched exactly: a substring match would quarantine
    /// `bars:AAAB` for a quarantine of `bars:AAA`.
    pub quarantined_sources: BTreeSet<String>,
    pub features: Vec<String>,
    /// The latest regime crossing anywhere the platform watches, if any.
    pub latest_regime_crossing: Option<Timestamp>,
    pub tournament: Option<ModelRecord>,
}

/// One critic.
pub trait Critic: std::fmt::Debug {
    fn kind(&self) -> CriticKind;
    fn attack_prediction(&self, dossier: &PredictionDossier) -> CriticVerdict;
    fn attack_model(&self, dossier: &ModelDossier, now: Timestamp) -> CriticVerdict;
}

fn survived(examined: impl Into<String>) -> CriticVerdict {
    CriticVerdict::Survived {
        examined: examined.into(),
    }
}

fn refuted(reason: impl Into<String>) -> CriticVerdict {
    CriticVerdict::Refuted {
        reason: reason.into(),
    }
}

fn abstained(missing: impl Into<String>) -> CriticVerdict {
    CriticVerdict::Abstained {
        missing: missing.into(),
    }
}

/// Conviction-weighted lean of the panel, in `[-1, 1]`.
fn panel_lean(panel: &[PanelView]) -> Option<f64> {
    let total: f64 = panel.iter().map(|v| v.conviction.abs()).sum();
    if total <= 0.0 {
        return None;
    }
    Some(
        panel
            .iter()
            .map(|v| v.direction * v.conviction.abs())
            .sum::<f64>()
            / total,
    )
}

/// The weight of evidence for and against.
fn evidence_balance(evidence: &[EvidenceItem]) -> (f64, f64) {
    evidence.iter().fold((0.0, 0.0), |(for_, against), item| {
        if item.supports {
            (for_ + item.weight.max(0.0), against)
        } else {
            (for_, against + item.weight.max(0.0))
        }
    })
}

/// The directional critics share one argument with the sign flipped: argue
/// the side opposite the claim, from the panel and the evidence.
fn directional_attack(dossier: &PredictionDossier, side: f64, name: &str) -> CriticVerdict {
    if dossier.direction * side > 0.0 {
        return survived(format!(
            "the claim is already {name}; the {name} critic attacks the opposite claim"
        ));
    }
    let lean = panel_lean(&dossier.panel);
    let (for_, against) = evidence_balance(&dossier.evidence);
    if lean.is_none() && for_ + against <= 0.0 {
        return abstained("no panel view and no weighted evidence to argue from");
    }
    if let Some(lean) = lean
        && lean * side > 0.0
    {
        return refuted(format!(
            "the panel leans {name} ({lean:+.2}) against a claim the other way"
        ));
    }
    if against > for_ {
        return refuted(format!(
            "contrary evidence weighs {against:.2} against {for_:.2} in support"
        ));
    }
    survived(format!(
        "panel lean {} and evidence {for_:.2} for / {against:.2} against",
        lean.map_or("none".to_string(), |l| format!("{l:+.2}"))
    ))
}

fn directional_model_attack(
    dossier: &ModelDossier,
    bias_against_up: bool,
    name: &str,
) -> CriticVerdict {
    let Some(record) = &dossier.tournament else {
        return abstained("the model has no tournament record to read a directional bias from");
    };
    let (Some((up, up_n)), Some((down, down_n))) = (record.up_hits, record.down_hits) else {
        return abstained("the tournament has not resolved the model on both directions");
    };
    if up_n < BIAS_SAMPLE || down_n < BIAS_SAMPLE {
        return abstained(format!(
            "{up_n} up and {down_n} down resolutions; {BIAS_SAMPLE} of each are needed"
        ));
    }
    let gap = if bias_against_up {
        down - up
    } else {
        up - down
    };
    if gap > BIAS_GAP {
        return refuted(format!(
            "the model is right {:.0}% on up moves and {:.0}% on down moves; it misses the \
             {name} case",
            up * 100.0,
            down * 100.0
        ));
    }
    survived(format!("hit rates {up:.2} up and {down:.2} down"))
}

#[derive(Debug)]
struct BullCritic;
impl Critic for BullCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::Bull
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        directional_attack(d, 1.0, "bull")
    }
    fn attack_model(&self, d: &ModelDossier, _now: Timestamp) -> CriticVerdict {
        directional_model_attack(d, true, "bull")
    }
}

#[derive(Debug)]
struct BearCritic;
impl Critic for BearCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::Bear
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        directional_attack(d, -1.0, "bear")
    }
    fn attack_model(&self, d: &ModelDossier, _now: Timestamp) -> CriticVerdict {
        directional_model_attack(d, false, "bear")
    }
}

#[derive(Debug)]
struct StructuralBreakCritic;
impl Critic for StructuralBreakCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::StructuralBreak
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        match d.regime_crossed_recently {
            None => abstained("the subject's regime is not tracked"),
            Some(true) => refuted(format!(
                "{} crossed a regime boundary recently; the evidence describes the regime it left",
                d.subject
            )),
            Some(false) => survived("no recent regime crossing on the subject"),
        }
    }
    fn attack_model(&self, d: &ModelDossier, _now: Timestamp) -> CriticVerdict {
        let Some(evaluated) = d.evaluated_at else {
            return abstained("the model has no evaluation to date the regime against");
        };
        match d.latest_regime_crossing {
            Some(crossing) if crossing > evaluated => refuted(format!(
                "the market crossed a regime boundary at {crossing}, after the model was last \
                 evaluated at {evaluated}"
            )),
            _ => survived("no regime crossing since the latest evaluation"),
        }
    }
}

#[derive(Debug)]
struct ManipulationCritic;
impl Critic for ManipulationCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::Manipulation
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        match d.spread_ratio {
            None => abstained("no quoted spread history for the subject"),
            Some(ratio) if ratio >= MANIPULATION_SPREAD_MULTIPLE => refuted(format!(
                "the quoted spread is {ratio:.1}× its median; a price this thin can be pushed, \
                 and the move the claim reads may be someone else's order"
            )),
            Some(ratio) => survived(format!("spread at {ratio:.1}× its median")),
        }
    }
    fn attack_model(&self, _d: &ModelDossier, _now: Timestamp) -> CriticVerdict {
        abstained(
            "a model has no tape of its own; manipulation is examined on the predictions it makes",
        )
    }
}

#[derive(Debug)]
struct TailRiskCritic;
impl Critic for TailRiskCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::TailRisk
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        let Some(adverse) = d.adverse_tail_bps else {
            return abstained("no recent return window for the subject");
        };
        let edge = d.expected_move_bps.abs();
        if edge > 0.0 && adverse >= TAIL_MULTIPLE * edge {
            return refuted(format!(
                "a single adverse move of {adverse:.0} bp was seen recently against a claimed \
                 edge of {edge:.0} bp"
            ));
        }
        survived(format!(
            "largest adverse move {adverse:.0} bp against an edge of {edge:.0} bp"
        ))
    }
    fn attack_model(&self, d: &ModelDossier, _now: Timestamp) -> CriticVerdict {
        let Some(record) = &d.tournament else {
            return abstained("the model has no tournament record to read its tails from");
        };
        let worst = record
            .scorecards
            .iter()
            .filter_map(|c| c.tail_brier.value())
            .fold(None, |worst: Option<f64>, v| {
                Some(worst.map_or(v, |w| w.max(v)))
            });
        match worst {
            None => abstained("no tail outcome has resolved against the model"),
            Some(brier) if brier > TAIL_BRIER_BAR => refuted(format!(
                "tail Brier {brier:.3} is worse than a coin flip on tail outcomes"
            )),
            Some(brier) => survived(format!("worst tail Brier {brier:.3}")),
        }
    }
}

#[derive(Debug)]
struct ModelRiskCritic;
impl Critic for ModelRiskCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::ModelRisk
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        if d.forecaster_failure.is_empty() {
            return abstained("no forecaster behind the claim has a meta-model record");
        }
        let failing: Vec<String> = d
            .forecaster_failure
            .iter()
            .filter(|(_, p)| **p >= MODEL_FAILURE_BAR)
            .map(|(name, p)| format!("{name} ({p:.2})"))
            .collect();
        if failing.is_empty() {
            survived("every forecaster behind the claim is predicted to hold in this regime")
        } else {
            refuted(format!(
                "the meta-model predicts failure in this regime for {}",
                failing.join(", ")
            ))
        }
    }
    fn attack_model(&self, d: &ModelDossier, now: Timestamp) -> CriticVerdict {
        let (Some(evaluated), Some(passed)) = (d.evaluated_at, d.evaluation_passed) else {
            return refuted("the model has never been evaluated");
        };
        if !passed {
            return refuted("the latest evaluation did not pass");
        }
        if now.since(evaluated) > d.evaluation_validity {
            return refuted(format!(
                "the latest evaluation at {evaluated} is past its validity window"
            ));
        }
        if d.drift_score > d.drift_threshold {
            return refuted(format!(
                "drift {:.3} exceeds the threshold {:.3}",
                d.drift_score, d.drift_threshold
            ));
        }
        if let Some(record) = &d.tournament
            && record.predicted_failure_now >= MODEL_FAILURE_BAR
        {
            return refuted(format!(
                "the meta-model predicts failure {:.2} in the regime now in force",
                record.predicted_failure_now
            ));
        }
        survived("evaluated, passing, inside validity, below its drift threshold")
    }
}

#[derive(Debug)]
struct DataPoisoningCritic;
impl Critic for DataPoisoningCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::DataPoisoning
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        if let Some(late) = d.evidence.iter().find(|e| e.known_at > d.as_of) {
            return refuted(format!(
                "evidence from {} became knowable at {}, after the hypothesis reasons as of {}",
                late.origin, late.known_at, d.as_of
            ));
        }
        let supporting: Vec<&EvidenceItem> = d.evidence.iter().filter(|e| e.supports).collect();
        let total: f64 = supporting.iter().map(|e| e.weight.max(0.0)).sum();
        if supporting.len() < 2 || total <= 0.0 {
            return abstained("fewer than two weighted supporting items to test for concentration");
        }
        let mut by_origin: BTreeMap<&str, f64> = BTreeMap::new();
        for item in &supporting {
            *by_origin.entry(item.origin.as_str()).or_insert(0.0) += item.weight.max(0.0);
        }
        if let Some((origin, weight)) = by_origin
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
            && weight / total > SINGLE_ORIGIN_SHARE
        {
            return refuted(format!(
                "{origin} carries {:.0}% of the supporting weight; one source counted several times \
                 is the shape a poisoned feed takes",
                weight / total * 100.0
            ));
        }
        survived(format!("support spread over {} origin(s)", by_origin.len()))
    }
    fn attack_model(&self, d: &ModelDossier, _now: Timestamp) -> CriticVerdict {
        if d.training_datasets.is_empty() {
            return abstained("the card records no training lineage to examine");
        }
        let poisoned: Vec<&String> = d
            .training_datasets
            .iter()
            .filter(|dataset| d.quarantined_sources.contains(dataset.as_str()))
            .collect();
        if poisoned.is_empty() {
            survived(format!(
                "{} training dataset(s), none quarantined",
                d.training_datasets.len()
            ))
        } else {
            refuted(format!(
                "trained on quarantined data: {}",
                poisoned
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

#[derive(Debug)]
struct MissingVariableCritic;
impl Critic for MissingVariableCritic {
    fn kind(&self) -> CriticKind {
        CriticKind::MissingVariable
    }
    fn attack_prediction(&self, d: &PredictionDossier) -> CriticVerdict {
        if !d.missing_inputs.is_empty() {
            return refuted(format!(
                "the panel reported missing inputs the explanation went ahead without: {}",
                d.missing_inputs.join("; ")
            ));
        }
        if d.coverage < MINIMUM_COVERAGE {
            return refuted(format!(
                "only {:.0}% of the specialists asked formed a view",
                d.coverage * 100.0
            ));
        }
        survived(format!(
            "coverage {:.0}%, no missing inputs reported",
            d.coverage * 100.0
        ))
    }
    fn attack_model(&self, d: &ModelDossier, _now: Timestamp) -> CriticVerdict {
        if d.features.is_empty() {
            return refuted("the card names no features; a model reading nothing explains nothing");
        }
        survived(format!("{} feature(s) named", d.features.len()))
    }
}

/// The society: eight independent critics.
#[derive(Debug)]
pub struct VerifierSociety {
    critics: Vec<Box<dyn Critic + Send + Sync>>,
}

impl Default for VerifierSociety {
    fn default() -> Self {
        Self::standard()
    }
}

impl VerifierSociety {
    /// All eight §9.11 critics.
    pub fn standard() -> Self {
        Self {
            critics: vec![
                Box::new(BullCritic),
                Box::new(BearCritic),
                Box::new(StructuralBreakCritic),
                Box::new(ManipulationCritic),
                Box::new(TailRiskCritic),
                Box::new(ModelRiskCritic),
                Box::new(DataPoisoningCritic),
                Box::new(MissingVariableCritic),
            ],
        }
    }

    pub fn kinds(&self) -> Vec<CriticKind> {
        self.critics.iter().map(|c| c.kind()).collect()
    }

    /// Attack a prediction.
    pub fn attack_prediction(&self, dossier: &PredictionDossier, at: Timestamp) -> Verification {
        let findings = self
            .critics
            .iter()
            .map(|critic| CriticFinding {
                critic: critic.kind(),
                verdict: critic.attack_prediction(dossier),
            })
            .collect();
        Self::conclude(
            Target::Prediction {
                hypothesis_id: dossier.hypothesis_id.clone(),
                subject: dossier.subject.clone(),
            },
            findings,
            at,
        )
    }

    /// The request that asks every critic in the society to attack
    /// `dossier`'s prediction (R12-REASON-9032).
    pub fn request_for_prediction(
        &self,
        request_id: impl Into<String>,
        dossier: &PredictionDossier,
        requested_by: impl Into<String>,
        at: Timestamp,
    ) -> Result<ChallengeRequest> {
        ChallengeRequest::new(ChallengeRequestFields {
            schema_version: 1,
            request_id: request_id.into(),
            target: ChallengeTarget::Prediction {
                hypothesis_id: dossier.hypothesis_id.clone(),
                subject: dossier.subject.clone(),
            },
            critics: self
                .critics
                .iter()
                .map(|critic| critic.kind().as_str().to_string())
                .collect(),
            requested_by: requested_by.into(),
            requested_at: at,
        })
    }

    /// Answer a [`ChallengeRequest`] over a prediction: only the critics it
    /// names attack, and what comes back is the contract result, checked
    /// against its own reports and against the request.
    ///
    /// Refuses a request that targets a different hypothesis or subject from
    /// the dossier, or that names a critic this society does not hold — a
    /// request answered by fewer critics than it asked would read as a wider
    /// examination than took place.
    pub fn challenge_prediction(
        &self,
        request: &ChallengeRequest,
        dossier: &PredictionDossier,
        at: Timestamp,
    ) -> Result<VerificationResult> {
        let expected = ChallengeTarget::Prediction {
            hypothesis_id: dossier.hypothesis_id.clone(),
            subject: dossier.subject.clone(),
        };
        if request.target != expected {
            return Err(Error::invalid(format!(
                "challenge `{}` targets {:?}, not the dossier's {expected:?}; build the request \
                 from the dossier it is answered with",
                request.request_id, request.target
            )));
        }
        let held: BTreeSet<&str> = self.critics.iter().map(|c| c.kind().as_str()).collect();
        if let Some(unknown) = request
            .critics
            .iter()
            .find(|name| !held.contains(name.as_str()))
        {
            return Err(Error::invalid(format!(
                "challenge `{}` asks for critic `{unknown}`, which this society does not hold; \
                 ask for one of {held:?}",
                request.request_id
            )));
        }
        let findings: Vec<CriticFinding> = self
            .critics
            .iter()
            .filter(|critic| request.critics.iter().any(|n| n == critic.kind().as_str()))
            .map(|critic| CriticFinding {
                critic: critic.kind(),
                verdict: critic.attack_prediction(dossier),
            })
            .collect();
        let verification = Self::conclude(
            Target::Prediction {
                hypothesis_id: dossier.hypothesis_id.clone(),
                subject: dossier.subject.clone(),
            },
            findings,
            at,
        );
        let result = verification.as_result(&request.request_id)?;
        result.answers(request)?;
        Ok(result)
    }

    /// Attack a model at promotion.
    pub fn attack_model(&self, dossier: &ModelDossier, at: Timestamp) -> Verification {
        let findings = self
            .critics
            .iter()
            .map(|critic| CriticFinding {
                critic: critic.kind(),
                verdict: critic.attack_model(dossier, at),
            })
            .collect();
        Self::conclude(
            Target::Model {
                reference: dossier.reference.clone(),
            },
            findings,
            at,
        )
    }

    fn conclude(target: Target, findings: Vec<CriticFinding>, at: Timestamp) -> Verification {
        let mut by = Vec::new();
        let mut reasons = Vec::new();
        let mut examined = 0;
        for finding in &findings {
            match &finding.verdict {
                CriticVerdict::Refuted { reason } => {
                    examined += 1;
                    by.push(finding.critic);
                    reasons.push(format!("{}: {reason}", finding.critic.as_str()));
                }
                CriticVerdict::Survived { .. } => examined += 1,
                CriticVerdict::Abstained { .. } => {}
            }
        }
        let verdict = if !by.is_empty() {
            SocietyVerdict::Refuted { by, reasons }
        } else if examined < QUORUM {
            SocietyVerdict::Unverified { examined }
        } else {
            SocietyVerdict::Survived
        };
        Verification {
            target,
            at,
            findings,
            verdict,
        }
    }
}

/// Whether a measure reads at all — a helper for callers building a
/// [`ModelRecord`] from scorecards.
pub fn measured(measure: &Measure) -> bool {
    measure.value().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + secs)
    }

    /// A prediction every critic can examine and none can refute.
    fn sound() -> PredictionDossier {
        PredictionDossier {
            hypothesis_id: "hyp-1".to_string(),
            subject: "obj-AAA".to_string(),
            direction: 1.0,
            confidence: 0.7,
            as_of: at(0),
            expected_move_bps: 200.0,
            evidence: vec![
                EvidenceItem {
                    origin: "exchange".to_string(),
                    supports: true,
                    weight: 0.5,
                    known_at: at(-10),
                },
                EvidenceItem {
                    origin: "filings".to_string(),
                    supports: true,
                    weight: 0.4,
                    known_at: at(-10),
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
        }
    }

    fn refuting(verification: &Verification) -> Vec<CriticKind> {
        match &verification.verdict {
            SocietyVerdict::Refuted { by, .. } => by.clone(),
            _ => Vec::new(),
        }
    }

    /// R12-REASON-3108: each of the eight critics runs on a hypothesis and
    /// can refuse it. Premise first: the sound dossier survives all eight.
    /// Then each mutation of the dossier is refuted by exactly the critic
    /// that hunts that failure. Mutation of the implementation: removing any
    /// critic from `standard()` (or making one always survive) fails its row.
    #[test]
    fn each_of_the_eight_critics_can_refute_a_prediction_on_its_own_failure() {
        let society = VerifierSociety::standard();
        assert_eq!(
            society.kinds(),
            CriticKind::ALL.to_vec(),
            "a critic is missing"
        );
        let clean = society.attack_prediction(&sound(), at(0));
        assert_eq!(
            clean.verdict,
            SocietyVerdict::Survived,
            "{:?}",
            clean.findings
        );

        let mut cases: Vec<(CriticKind, PredictionDossier)> = Vec::new();
        let mut bearish = sound();
        bearish.direction = -1.0;
        cases.push((CriticKind::Bull, bearish));
        let mut opposed = sound();
        opposed.panel = vec![PanelView {
            agent: "macro".to_string(),
            direction: -1.0,
            conviction: 0.9,
        }];
        cases.push((CriticKind::Bear, opposed));
        let mut broken = sound();
        broken.regime_crossed_recently = Some(true);
        cases.push((CriticKind::StructuralBreak, broken));
        let mut thin = sound();
        thin.spread_ratio = Some(4.0);
        cases.push((CriticKind::Manipulation, thin));
        let mut fragile = sound();
        fragile.adverse_tail_bps = Some(900.0);
        cases.push((CriticKind::TailRisk, fragile));
        let mut failing = sound();
        failing
            .forecaster_failure
            .insert("analyst:x".to_string(), 0.8);
        cases.push((CriticKind::ModelRisk, failing));
        let mut poisoned = sound();
        poisoned.evidence[0].weight = 5.0;
        cases.push((CriticKind::DataPoisoning, poisoned));
        let mut gap = sound();
        gap.missing_inputs = vec!["inventory data".to_string()];
        cases.push((CriticKind::MissingVariable, gap));

        assert_eq!(cases.len(), 8);
        for (critic, dossier) in cases {
            let verification = society.attack_prediction(&dossier, at(0));
            assert_eq!(
                refuting(&verification),
                vec![critic],
                "the {} failure was refuted by the wrong critics",
                critic.as_str()
            );
        }
    }

    /// Mutation: counting abstentions toward the quorum turns this into a
    /// survival nobody earned.
    #[test]
    fn a_target_most_critics_could_not_examine_is_unverified_not_survived() {
        let society = VerifierSociety::standard();
        let mut empty = sound();
        empty.evidence.clear();
        empty.panel.clear();
        empty.regime_crossed_recently = None;
        empty.spread_ratio = None;
        empty.adverse_tail_bps = None;
        empty.forecaster_failure.clear();
        let verification = society.attack_prediction(&empty, at(0));
        assert!(matches!(
            verification.verdict,
            SocietyVerdict::Unverified { .. }
        ));
    }

    /// Look-ahead in the evidence is poisoning. Mutation: dropping the
    /// `known_at > as_of` check lets it survive.
    #[test]
    fn evidence_knowable_only_after_the_hypothesis_is_refuted_as_poisoning() {
        let society = VerifierSociety::standard();
        let mut leaked = sound();
        leaked.evidence[1].known_at = at(60);
        let verification = society.attack_prediction(&leaked, at(0));
        assert_eq!(refuting(&verification), vec![CriticKind::DataPoisoning]);
    }

    /// A model the market moved past is refuted at promotion. Mutation:
    /// comparing the crossing against `now` instead of the evaluation lets it
    /// through.
    #[test]
    fn a_model_evaluated_before_a_regime_break_is_refuted_at_promotion() {
        let society = VerifierSociety::standard();
        let dossier = ModelDossier {
            reference: "bar-linear@1".to_string(),
            evaluated_at: Some(at(0)),
            evaluation_passed: Some(true),
            evaluation_validity: Duration::from_days(90),
            drift_score: 0.0,
            drift_threshold: 0.2,
            training_datasets: vec!["bars:AAA".to_string()],
            quarantined_sources: BTreeSet::new(),
            features: vec!["return_1d".to_string()],
            latest_regime_crossing: Some(at(30)),
            tournament: None,
        };
        let verification = society.attack_model(&dossier, at(60));
        assert_eq!(refuting(&verification), vec![CriticKind::StructuralBreak]);
        let mut fresh = dossier;
        fresh.latest_regime_crossing = None;
        assert!(!society.attack_model(&fresh, at(60)).verdict.is_refuted());
    }

    /// R12-REASON-9032: a fatal finding comes back as the contract result,
    /// refuted by the critic that raised it and no other, and the result
    /// answers the request it was asked under. Premise first: the same
    /// dossier unaltered survives, so the refutation is the alteration's.
    /// Mutation: map `SocietyVerdict::Refuted` to `ChallengeVerdict::Survived`
    /// in `Verification::as_result` — the contract refuses the result
    /// ("survived while tail_risk refuted it") and `expect` fails.
    #[test]
    fn a_fatal_challenge_comes_back_as_a_contract_result_refuted_by_the_critic_that_raised_it() {
        let society = VerifierSociety::standard();
        let ask = |dossier: &PredictionDossier| {
            let request = society
                .request_for_prediction("challenge-1", dossier, "kernel/test", at(0))
                .expect("a request");
            let result = society
                .challenge_prediction(&request, dossier, at(0))
                .expect("the contract accepts the society's own verdict");
            (request, result)
        };
        let (request, clean) = ask(&sound());
        assert_eq!(request.critics.len(), 8, "premise: every critic is asked");
        assert_eq!(clean.verdict, ChallengeVerdict::Survived, "{clean:?}");

        let mut fragile = sound();
        fragile.adverse_tail_bps = Some(900.0);
        let (request, result) = ask(&fragile);

        assert_eq!(
            result.verdict,
            ChallengeVerdict::Refuted {
                by: vec!["tail_risk".to_string()]
            }
        );
        result
            .answers(&request)
            .expect("it answers its own request");
    }

    /// A request is answered by exactly the critics it names, and a request
    /// naming one the society does not hold is refused rather than answered
    /// by fewer critics than it asked. Mutation: drop the `held` check in
    /// `challenge_prediction` — the unknown critic is silently skipped and the
    /// refusal assertion fails.
    #[test]
    fn a_challenge_is_answered_by_the_critics_it_names_and_refuses_one_the_society_lacks() {
        let society = VerifierSociety::standard();
        let dossier = sound();
        let mut fields = society
            .request_for_prediction("challenge-2", &dossier, "kernel/test", at(0))
            .expect("a request")
            .into_fields();
        fields.critics = vec!["bull".to_string(), "bear".to_string()];
        let narrow = ChallengeRequest::new(fields.clone()).expect("two critics");

        let result = society
            .challenge_prediction(&narrow, &dossier, at(0))
            .expect("two named critics answer");
        assert_eq!(result.reports.len(), 2, "{result:?}");

        fields.critics.push("astrologer".to_string());
        let unknown = ChallengeRequest::new(fields).expect("well-formed, if unanswerable");
        let refusal = society.challenge_prediction(&unknown, &dossier, at(0));
        assert!(
            refusal.is_err(),
            "a critic nobody holds was answered for: {refusal:?}"
        );
    }
}
