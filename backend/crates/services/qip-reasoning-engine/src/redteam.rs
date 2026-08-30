//! Adversarial review.
//!
//! Every hypothesis is attacked before it can be acted on. The attacks here
//! are deterministic checks against the hypothesis's own structure and
//! evidence, not a model being asked whether it is convinced — a model asked
//! to critique a well-written thesis will usually find it persuasive, which is
//! the failure mode this component exists to avoid.
//!
//! Each [`Challenge`] names the specific flaw, states what would resolve it,
//! and carries a severity. Severity aggregates into a verdict by explicit
//! rules: one fatal challenge rejects, and enough serious ones do too.
//!
//! A red team that never rejects anything is decoration, so
//! [`ReviewOutcome::rejection_rate`] is tracked and the acceptance criteria are
//! stated in one place where they can be argued with.

use crate::evidence::{EvidenceKind, Stance};
use crate::hypothesis::{Hypothesis, HypothesisStatus};
use qip_core::ids::ChallengeId;
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;

/// How damaging a challenge is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth recording; does not block.
    Note,
    /// Weakens the thesis and reduces its confidence.
    Serious,
    /// The thesis cannot stand as written.
    Fatal,
}

impl Severity {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Serious => "serious",
            Self::Fatal => "fatal",
        }
    }

    /// Multiplicative confidence penalty when the challenge stands.
    ///
    /// Calibrated against [`ReviewPolicy::serious_tolerance`]: two serious
    /// challenges cost 28% of the confidence, which a strong thesis survives
    /// and an ordinary one does not. A harsher per-challenge penalty would
    /// make the tolerance meaningless, since the first challenge would already
    /// take every thesis below the floor.
    pub const fn confidence_penalty(&self) -> f64 {
        match self {
            Self::Note => 0.97,
            Self::Serious => 0.85,
            Self::Fatal => 0.0,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kinds of attack the red team runs.
///
/// Named rather than free-form so the same weaknesses are checked every time
/// and so a gap in coverage is visible as a missing variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeKind {
    /// The supporting evidence traces to one origin.
    CorrelatedEvidence,
    /// One item carries almost all the weight.
    SingleFactDependency,
    /// No primary source.
    NoPrimarySource,
    /// The causal chain is long, weak, or both.
    WeakMechanism,
    /// The chain's sign does not survive to the conclusion.
    SignError,
    /// The claim would already be in the price.
    AlreadyPriced,
    /// The prior looks chosen to suit the conclusion.
    PriorNotJustified,
    /// The evidence supports a different explanation at least as well.
    AlternativeExplanation,
    /// The horizon is too short for the mechanism's own lags.
    HorizonMismatch,
    /// The evidence is stale relative to the as-of time.
    StaleEvidence,
    /// The claim rests on data whose licence forbids this use.
    DataLicensing,
    /// Contradicting evidence exists and was not addressed.
    UnaddressedContradiction,
}

impl ChallengeKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::CorrelatedEvidence => "correlated_evidence",
            Self::SingleFactDependency => "single_fact_dependency",
            Self::NoPrimarySource => "no_primary_source",
            Self::WeakMechanism => "weak_mechanism",
            Self::SignError => "sign_error",
            Self::AlreadyPriced => "already_priced",
            Self::PriorNotJustified => "prior_not_justified",
            Self::AlternativeExplanation => "alternative_explanation",
            Self::HorizonMismatch => "horizon_mismatch",
            Self::StaleEvidence => "stale_evidence",
            Self::DataLicensing => "data_licensing",
            Self::UnaddressedContradiction => "unaddressed_contradiction",
        }
    }
}

impl fmt::Display for ChallengeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One attack on a hypothesis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Challenge {
    pub challenge_id: ChallengeId,
    pub kind: ChallengeKind,
    pub severity: Severity,
    /// The specific problem, with the numbers that show it.
    pub finding: String,
    /// What would answer the challenge. A challenge with no resolution is a
    /// complaint, so this is required.
    pub resolution: String,
    pub raised_at: Timestamp,
}

impl Challenge {
    pub fn new(
        challenge_id: ChallengeId,
        kind: ChallengeKind,
        severity: Severity,
        finding: impl Into<String>,
        resolution: impl Into<String>,
        raised_at: Timestamp,
    ) -> Self {
        Self {
            challenge_id,
            kind,
            severity,
            finding: finding.into(),
            resolution: resolution.into(),
            raised_at,
        }
    }
}

/// Thresholds the red team applies.
///
/// Every number here is a judgement call, so they are collected in one place
/// with the reasoning attached rather than scattered through the checks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewPolicy {
    /// Supporting-weight concentration above which evidence is called
    /// correlated. At 0.70 one origin is carrying the thesis.
    pub concentration_limit: f64,
    /// Share of belief movement from one item above which the thesis is
    /// really about that item.
    pub dominance_limit: f64,
    /// Chain confidence below which the mechanism is called weak. A three-step
    /// chain at 0.75 per step lands at 0.42, which is about the point where
    /// the story stops being convincing.
    pub weak_chain_confidence: f64,
    /// Chain length beyond which the story is too long to check.
    pub maximum_chain_length: usize,
    /// Serious challenges tolerated before rejection.
    pub serious_tolerance: usize,
    /// Confidence a hypothesis must retain after penalties.
    ///
    /// Set against the scale [`crate::hypothesis::Hypothesis`] actually
    /// produces: a well-evidenced thesis with a sound two-step mechanism lands
    /// near 0.55, so a floor of 0.50 admits that thesis, admits a strong one
    /// carrying two serious challenges, and rejects an ordinary one carrying
    /// even a single structural flaw.
    pub minimum_surviving_confidence: f64,
    /// How stale supporting evidence may be, as a fraction of the horizon.
    /// Evidence older than the horizon it is meant to predict over is usually
    /// already reflected in the price.
    pub staleness_limit: f64,
}

impl Default for ReviewPolicy {
    fn default() -> Self {
        Self {
            concentration_limit: 0.70,
            dominance_limit: 0.75,
            weak_chain_confidence: 0.40,
            maximum_chain_length: 4,
            serious_tolerance: 2,
            minimum_surviving_confidence: 0.50,
            staleness_limit: 2.0,
        }
    }
}

/// The result of a review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewOutcome {
    pub hypothesis_id: String,
    pub reviewed_at: Timestamp,
    pub challenges: Vec<Challenge>,
    /// Confidence before review.
    pub confidence_before: f64,
    /// Confidence after the surviving challenges are applied.
    pub confidence_after: f64,
    pub verdict: HypothesisStatus,
    /// Why, in a sentence, for the decision record.
    pub rationale: String,
}

impl ReviewOutcome {
    pub fn approved(&self) -> bool {
        self.verdict == HypothesisStatus::Approved
    }

    pub fn fatal(&self) -> Vec<&Challenge> {
        self.challenges
            .iter()
            .filter(|c| c.severity == Severity::Fatal)
            .collect()
    }

    pub fn serious(&self) -> Vec<&Challenge> {
        self.challenges
            .iter()
            .filter(|c| c.severity == Severity::Serious)
            .collect()
    }

    /// The kinds raised, for tracking which weaknesses recur.
    pub fn kinds(&self) -> Vec<ChallengeKind> {
        let mut kinds: Vec<ChallengeKind> = self.challenges.iter().map(|c| c.kind).collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }
}

/// The adversarial reviewer.
#[derive(Clone, Debug, Default)]
pub struct RedTeam {
    policy: ReviewPolicy,
    reviewed: usize,
    rejected: usize,
}

impl RedTeam {
    pub fn new(policy: ReviewPolicy) -> Self {
        Self {
            policy,
            reviewed: 0,
            rejected: 0,
        }
    }

    pub fn policy(&self) -> &ReviewPolicy {
        &self.policy
    }

    pub fn reviewed(&self) -> usize {
        self.reviewed
    }

    /// Share of reviews that rejected.
    ///
    /// Watched deliberately. A rejection rate near zero means the review is
    /// not doing anything; near one means the hypothesis generator is.
    pub fn rejection_rate(&self) -> Option<f64> {
        if self.reviewed == 0 {
            return None;
        }
        Some(self.rejected as f64 / self.reviewed as f64)
    }

    /// Attack a hypothesis and return a verdict.
    ///
    /// `market_priced_in` is the platform's own estimate of how much of the
    /// claim the price already reflects, in `[0, 1]`, computed elsewhere from
    /// market data. Passed in rather than guessed here, because guessing it is
    /// exactly the mistake the check exists to catch.
    pub fn review(
        &mut self,
        hypothesis: &Hypothesis,
        market_priced_in: Option<f64>,
        at: Timestamp,
        next_id: &mut impl FnMut() -> ChallengeId,
    ) -> ReviewOutcome {
        let mut challenges = Vec::new();
        let mut raise = |kind: ChallengeKind,
                         severity: Severity,
                         finding: String,
                         resolution: &str,
                         challenges: &mut Vec<Challenge>| {
            challenges.push(Challenge::new(
                next_id(),
                kind,
                severity,
                finding,
                resolution,
                at,
            ));
        };

        // --- evidence structure ---
        let concentration = hypothesis.evidence.concentration();
        if concentration > self.policy.concentration_limit && hypothesis.evidence.len() > 1 {
            raise(
                ChallengeKind::CorrelatedEvidence,
                Severity::Serious,
                format!(
                    "{:.0}% of the supporting weight comes from a single origin across {} distinct origins",
                    concentration * 100.0,
                    hypothesis.evidence.origins().len()
                ),
                "find corroboration from an independent origin, or restate the thesis as resting on one source",
                &mut challenges,
            );
        }

        let dominance = hypothesis.belief.dominance();
        if dominance > self.policy.dominance_limit && hypothesis.evidence.len() > 1 {
            raise(
                ChallengeKind::SingleFactDependency,
                Severity::Serious,
                format!(
                    "{:.0}% of the belief movement comes from one item; the thesis is a bet on that item being right",
                    dominance * 100.0
                ),
                "verify the dominant item directly, or add evidence that does not depend on it",
                &mut challenges,
            );
        }

        if !hypothesis.evidence.has_primary_source() {
            raise(
                ChallengeKind::NoPrimarySource,
                Severity::Serious,
                "no filing, official statistic or market observation supports the claim"
                    .to_string(),
                "locate a primary source, or mark the thesis as resting on secondary reporting",
                &mut challenges,
            );
        }

        // A thesis built on rumour is not a thesis.
        let rumour_only = !hypothesis.evidence.is_empty()
            && hypothesis
                .evidence
                .with_stance(Stance::Supports)
                .iter()
                .all(|e| e.kind == EvidenceKind::Rumour);
        if rumour_only {
            raise(
                ChallengeKind::NoPrimarySource,
                Severity::Fatal,
                "every supporting item is an unattributed report".to_string(),
                "obtain attributable evidence before this can be acted on",
                &mut challenges,
            );
        }

        let contradicting = hypothesis.evidence.with_stance(Stance::Contradicts);
        if !contradicting.is_empty() {
            let strongest = contradicting
                .iter()
                .map(|e| e.weight())
                .fold(0.0_f64, f64::max);
            let support = hypothesis.evidence.independent_weight(Stance::Supports);
            if strongest > 0.5 * support {
                raise(
                    ChallengeKind::UnaddressedContradiction,
                    Severity::Serious,
                    format!(
                        "contradicting evidence carries weight {strongest:.2} against supporting weight {support:.2}"
                    ),
                    "explain why the contradicting evidence does not apply, or revise the claim",
                    &mut challenges,
                );
            }
        }

        // --- mechanism ---
        let chain_confidence = hypothesis.chain.confidence();
        if chain_confidence < self.policy.weak_chain_confidence {
            raise(
                ChallengeKind::WeakMechanism,
                Severity::Serious,
                format!(
                    "the {}-step mechanism holds with probability {:.2}",
                    hypothesis.chain.len(),
                    chain_confidence
                ),
                "shorten the chain, or evidence the weakest step directly",
                &mut challenges,
            );
        }
        if hypothesis.chain.len() > self.policy.maximum_chain_length {
            raise(
                ChallengeKind::WeakMechanism,
                Severity::Serious,
                format!(
                    "{} causal steps is more than can be checked; each is an opportunity to be wrong",
                    hypothesis.chain.len()
                ),
                "reduce the chain to the steps that carry the claim",
                &mut challenges,
            );
        }

        // The claim's direction must match what the chain actually transmits.
        if let Some(sign) = hypothesis.claim.implied_sign() {
            let chain_sign = if hypothesis.chain.preserves_sign() {
                1.0
            } else {
                -1.0
            };
            // A chain that flips the sign supports the opposite claim, so a
            // positive claim on a flipping chain is an error of direction.
            if sign > 0.0 && chain_sign < 0.0 {
                raise(
                    ChallengeKind::SignError,
                    Severity::Fatal,
                    "the mechanism inverts the sign an odd number of times, so it supports the opposite claim"
                        .to_string(),
                    "correct the claim direction, or the mechanism",
                    &mut challenges,
                );
            }
        }

        // --- timing ---
        let chain_lag = hypothesis.chain.total_lag();
        if chain_lag > hypothesis.horizon {
            raise(
                ChallengeKind::HorizonMismatch,
                Severity::Fatal,
                format!(
                    "the mechanism's own lags total {chain_lag:?} but the horizon is {:?}; the claim cannot resolve in time",
                    hypothesis.horizon
                ),
                "extend the horizon past the mechanism's lags, or find a faster mechanism",
                &mut challenges,
            );
        }

        if let Some(latest) = hypothesis.evidence.latest_known_at() {
            let age = hypothesis.as_of.since(latest);
            let limit = hypothesis.horizon * 2;
            if age > limit && self.policy.staleness_limit > 0.0 {
                raise(
                    ChallengeKind::StaleEvidence,
                    Severity::Serious,
                    format!(
                        "the newest supporting evidence is {age:?} old against a {:?} horizon; the market has had time to price it",
                        hypothesis.horizon
                    ),
                    "refresh the evidence, or explain why the market has not acted on it",
                    &mut challenges,
                );
            }
        }

        // --- pricing ---
        match market_priced_in {
            Some(priced) if priced > 0.8 => {
                raise(
                    ChallengeKind::AlreadyPriced,
                    Severity::Fatal,
                    format!(
                        "{:.0}% of the claim is already in the price",
                        priced * 100.0
                    ),
                    "identify what the market has not priced, or drop the thesis",
                    &mut challenges,
                );
            }
            Some(priced) if priced > 0.5 => {
                raise(
                    ChallengeKind::AlreadyPriced,
                    Severity::Serious,
                    format!(
                        "{:.0}% of the claim appears to be in the price",
                        priced * 100.0
                    ),
                    "size the position to the unpriced remainder",
                    &mut challenges,
                );
            }
            None => {
                raise(
                    ChallengeKind::AlreadyPriced,
                    Severity::Note,
                    "no estimate of how much is already priced was supplied".to_string(),
                    "compute the priced-in fraction from market data before acting",
                    &mut challenges,
                );
            }
            Some(_) => {}
        }

        // --- reasoning hygiene ---
        if hypothesis.leading_alternative.trim().len() < 20 {
            raise(
                ChallengeKind::AlternativeExplanation,
                Severity::Serious,
                "the alternative explanation is stated too thinly to have been considered"
                    .to_string(),
                "state the strongest competing explanation and why the evidence favours this one",
                &mut challenges,
            );
        }
        if hypothesis.prior > 0.5 {
            raise(
                ChallengeKind::PriorNotJustified,
                Severity::Note,
                format!(
                    "the prior of {:.2} assumes the claim is more likely than not before any evidence",
                    hypothesis.prior
                ),
                "justify the prior from the class base rate",
                &mut challenges,
            );
        }

        // --- verdict ---
        let confidence_before = hypothesis.effective_confidence();
        let penalty: f64 = challenges
            .iter()
            .map(|c| c.severity.confidence_penalty())
            .product();
        let confidence_after = confidence_before * penalty;

        let fatal = challenges
            .iter()
            .filter(|c| c.severity == Severity::Fatal)
            .count();
        let serious = challenges
            .iter()
            .filter(|c| c.severity == Severity::Serious)
            .count();

        let (verdict, rationale) = if fatal > 0 {
            (
                HypothesisStatus::Rejected,
                format!(
                    "{fatal} fatal challenge(s): {}",
                    challenges
                        .iter()
                        .filter(|c| c.severity == Severity::Fatal)
                        .map(|c| c.kind.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        } else if serious > self.policy.serious_tolerance {
            (
                HypothesisStatus::Rejected,
                format!(
                    "{serious} serious challenges exceed the tolerance of {}",
                    self.policy.serious_tolerance
                ),
            )
        } else if confidence_after < self.policy.minimum_surviving_confidence {
            (
                HypothesisStatus::Rejected,
                format!(
                    "confidence fell from {confidence_before:.3} to {confidence_after:.3}, below the {:.3} required",
                    self.policy.minimum_surviving_confidence
                ),
            )
        } else {
            (
                HypothesisStatus::Approved,
                format!(
                    "survived {} challenge(s) with confidence {confidence_after:.3}",
                    challenges.len()
                ),
            )
        };

        self.reviewed += 1;
        if verdict == HypothesisStatus::Rejected {
            self.rejected += 1;
        }

        ReviewOutcome {
            hypothesis_id: hypothesis.hypothesis_id.as_str().to_string(),
            reviewed_at: at,
            challenges,
            confidence_before,
            confidence_after,
            verdict,
            rationale,
        }
    }
}
