//! Investment hypotheses.
//!
//! A [`Hypothesis`] is the platform's unit of investment reasoning, and it is
//! deliberately hard to construct. It must state what it claims, by what
//! mechanism, over what horizon, on what evidence, and — the part that is
//! usually missing — what would make it wrong.
//!
//! [`Hypothesis::validate`] refuses anything that cannot be reviewed:
//! no mechanism, no falsifier, no evidence, or a confidence that does not
//! follow from the evidence. That last check is the important one. Confidence
//! here is not an opinion; it is the output of [`crate::bayes::update`] over
//! the evidence set, and a hypothesis whose stated confidence has drifted from
//! that arithmetic is rejected rather than quietly corrected.

use crate::bayes::{BeliefUpdate, EvidenceStrength, attenuate, update};
use crate::evidence::{EvidenceSet, Stance};
use qip_core::error::{Error, Result};
use qip_core::ids::{HypothesisId, ObjectId, OpportunityId};
use qip_core::time::{Duration, Timestamp};
use qip_world_model::causal::Mechanism;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// One step in the causal story a hypothesis tells.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CausalStep {
    pub from: String,
    pub to: String,
    pub mechanism: Mechanism,
    /// What the platform expects to see if this step is real, so the step can
    /// be checked rather than merely asserted.
    pub observable: String,
    /// Expected delay before that observable appears.
    pub lag: Duration,
    /// Confidence in this step alone, in `[0, 1]`.
    pub confidence: f64,
}

impl CausalStep {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        mechanism: Mechanism,
        observable: impl Into<String>,
        lag: Duration,
        confidence: f64,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            mechanism,
            observable: observable.into(),
            lag,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} affects {} via {}; expect {} within {:?}",
            self.from,
            self.to,
            self.mechanism.describe(),
            self.observable,
            self.lag
        )
    }
}

/// The mechanism a hypothesis proposes, as an ordered chain.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CausalChain {
    pub steps: Vec<CausalStep>,
}

impl CausalChain {
    pub fn new(steps: Vec<CausalStep>) -> Self {
        Self { steps }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Product of the per-step confidences.
    ///
    /// Multiplicative because every step must hold. A five-step chain of
    /// individually plausible links is not a plausible chain, and this is the
    /// arithmetic that says so.
    pub fn confidence(&self) -> f64 {
        if self.steps.is_empty() {
            return 0.0;
        }
        self.steps.iter().map(|s| s.confidence).product()
    }

    /// Total expected lag along the chain.
    pub fn total_lag(&self) -> Duration {
        self.steps
            .iter()
            .fold(Duration::ZERO, |acc, step| acc + step.lag)
    }

    /// Whether the chain's sign survives to the end.
    ///
    /// A substitution step flips the sign; two flip it back. Getting this
    /// wrong is how a supply-chain thesis ends up long the wrong name.
    pub fn preserves_sign(&self) -> bool {
        self.steps
            .iter()
            .filter(|s| !s.mechanism.preserves_sign())
            .count()
            % 2
            == 0
    }

    /// The chain, as prose, for a decision record.
    pub fn narrate(&self) -> String {
        if self.steps.is_empty() {
            return "no mechanism proposed".to_string();
        }
        self.steps
            .iter()
            .map(CausalStep::describe)
            .collect::<Vec<_>>()
            .join("; then ")
    }

    /// Steps whose confidence is below `threshold` — where a chain is weakest
    /// and where a red team should look first.
    pub fn weakest_links(&self, threshold: f64) -> Vec<&CausalStep> {
        self.steps
            .iter()
            .filter(|s| s.confidence < threshold)
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        if self.steps.is_empty() {
            return Err(Error::invalid(
                "a hypothesis with no mechanism is a correlation with an opinion attached",
            ));
        }
        for (i, step) in self.steps.iter().enumerate() {
            if step.observable.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "causal step {i} ({} -> {}) names nothing observable, so it cannot be checked",
                    step.from, step.to
                )));
            }
        }
        // The chain must actually connect: step n's effect is step n+1's cause.
        for pair in self.steps.windows(2) {
            if pair[0].to != pair[1].from {
                return Err(Error::invalid(format!(
                    "causal chain is broken: {} -> {} does not continue into {} -> {}",
                    pair[0].from, pair[0].to, pair[1].from, pair[1].to
                )));
            }
        }
        Ok(())
    }
}

/// What a hypothesis claims will happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Claim {
    /// The instrument is mispriced upward relative to fundamentals.
    Overvalued,
    Undervalued,
    /// Volatility is mispriced.
    VolatilityUnderpriced,
    VolatilityOverpriced,
    /// A relationship between two instruments has broken or will revert.
    SpreadWidens,
    SpreadNarrows,
    /// A regime change is under way.
    RegimeShift,
    /// An event will or will not occur.
    EventOccurs,
}

impl Claim {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Overvalued => "overvalued",
            Self::Undervalued => "undervalued",
            Self::VolatilityUnderpriced => "volatility_underpriced",
            Self::VolatilityOverpriced => "volatility_overpriced",
            Self::SpreadWidens => "spread_widens",
            Self::SpreadNarrows => "spread_narrows",
            Self::RegimeShift => "regime_shift",
            Self::EventOccurs => "event_occurs",
        }
    }

    /// The direction a position implementing this claim would take, where
    /// meaningful. `RegimeShift` and `EventOccurs` have no inherent direction:
    /// what to do about them depends on the book.
    pub const fn implied_sign(&self) -> Option<f64> {
        match self {
            Self::Undervalued | Self::VolatilityUnderpriced | Self::SpreadNarrows => Some(1.0),
            Self::Overvalued | Self::VolatilityOverpriced | Self::SpreadWidens => Some(-1.0),
            Self::RegimeShift | Self::EventOccurs => None,
        }
    }
}

impl fmt::Display for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a hypothesis is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    /// Formed but not yet reviewed.
    Draft,
    /// Passed adversarial review.
    Approved,
    /// Failed adversarial review.
    Rejected,
    /// Approved and now expressed in the book.
    Active,
    /// The horizon passed. The outcome is what the learning stage reads.
    Resolved,
    /// Superseded by a better-evidenced version.
    Superseded,
}

impl HypothesisStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Active => "active",
            Self::Resolved => "resolved",
            Self::Superseded => "superseded",
        }
    }

    /// Whether a portfolio may act on it.
    pub const fn is_actionable(&self) -> bool {
        matches!(self, Self::Approved | Self::Active)
    }
}

/// An investment hypothesis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hypothesis {
    pub hypothesis_id: HypothesisId,
    /// The opportunity that prompted it, when there was one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub opportunity_id: Option<OpportunityId>,
    pub formed_at: Timestamp,
    /// The point in time the hypothesis reasons as of.
    pub as_of: Timestamp,
    pub status: HypothesisStatus,
    /// Hypothesis class, used to look up the base rate. Deliberately a
    /// declared class rather than free text so priors cannot be picked to fit.
    pub class: String,
    pub claim: Claim,
    /// The claim in a sentence.
    pub statement: String,
    /// Instruments the claim is about.
    pub subjects: Vec<ObjectId>,
    pub chain: CausalChain,
    pub evidence: EvidenceSet,
    /// The base rate the belief started from.
    pub prior: f64,
    /// Confidence after updating on evidence. Recomputed, never assigned.
    pub confidence: f64,
    /// The full update, so the confidence can be audited step by step.
    pub belief: BeliefUpdate,
    /// The self-model factor each evidence origin's supporting weight was
    /// scaled by, for exactly the origins present in the evidence and only
    /// where the self-model had a sufficient sample to say. An absent origin
    /// was left at full weight, and an origin's contrary evidence is never
    /// scaled (see `strengths`), so a factor recorded here for an origin
    /// that only dissented was recorded and had no effect — kept anyway,
    /// because what a replay must reproduce is the rule and its inputs, not
    /// a guess at which inputs turned out to matter.
    ///
    /// Recorded on the hypothesis rather than looked up at replay time
    /// because the self-model moves every cycle: a replay that read today's
    /// factors would recompute a different confidence from the same evidence,
    /// and [`Hypothesis::validate`] would then reject the record as drifted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub origin_factors: BTreeMap<String, f64>,
    /// Observations that would falsify the hypothesis. Required.
    pub falsifiers: Vec<String>,
    /// The competing explanation the author considered most credible, and why
    /// it was set aside. Required, because a hypothesis with no considered
    /// alternative has not been thought about.
    pub leading_alternative: String,
    /// Horizon over which the claim should resolve.
    pub horizon: Duration,
    /// Agent runs that contributed.
    pub contributors: Vec<String>,
    /// Model references the hypothesis depended on.
    pub models: Vec<String>,
}

/// Everything a hypothesis needs, checked on the way in.
#[derive(Clone, Debug)]
pub struct HypothesisDraft {
    pub hypothesis_id: HypothesisId,
    pub opportunity_id: Option<OpportunityId>,
    pub formed_at: Timestamp,
    pub as_of: Timestamp,
    pub class: String,
    pub claim: Claim,
    pub statement: String,
    pub subjects: Vec<ObjectId>,
    pub chain: CausalChain,
    pub evidence: EvidenceSet,
    pub prior: f64,
    pub falsifiers: Vec<String>,
    pub leading_alternative: String,
    pub horizon: Duration,
    pub contributors: Vec<String>,
    pub models: Vec<String>,
}

impl Hypothesis {
    /// Form a hypothesis, computing its confidence from its evidence.
    ///
    /// There is no constructor that accepts a confidence. The only way to a
    /// higher number is more or better evidence.
    pub fn form(draft: HypothesisDraft) -> Result<Self> {
        Self::form_with_factors(draft, &BTreeMap::new())
    }

    /// Form a hypothesis with each evidence origin's weight scaled by the
    /// self-model's estimate of that origin's accuracy.
    ///
    /// `factors` is keyed by evidence origin — the detector name on a market
    /// observation, the agent id on a finding — and an origin it does not
    /// name is left at full weight. Only the factors for origins actually
    /// present in the evidence are recorded on the hypothesis, so the record
    /// is what was applied and not the whole table the platform held.
    ///
    /// The factor multiplies the item's signed weight before the log-odds
    /// update, so a component estimated at 0.6 accuracy contributes sixty
    /// percent of the evidence it would have at full weight, in either
    /// direction. It is a scaling of *contribution* and not of confidence:
    /// the prior, the review bar and the action bar are untouched, and a
    /// hypothesis with every origin discounted falls back toward its base
    /// rate, not toward zero.
    pub fn form_with_factors(
        draft: HypothesisDraft,
        factors: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        // Evidence is filtered to the as-of time before anything is computed,
        // so a hypothesis cannot be built on facts it could not have known.
        let evidence = draft.evidence.as_of(draft.as_of);
        evidence.validate()?;

        let origin_factors: BTreeMap<String, f64> = evidence
            .origins()
            .into_iter()
            .filter_map(|origin| {
                factors
                    .get(origin)
                    .map(|factor| (origin.to_string(), *factor))
            })
            .collect();
        validate_factors(&origin_factors)?;

        let belief = update(draft.prior, &strengths(&evidence, &origin_factors));
        let chain_weight = draft.chain.confidence().max(MINIMUM_CHAIN_WEIGHT);
        let hypothesis = Self {
            hypothesis_id: draft.hypothesis_id,
            opportunity_id: draft.opportunity_id,
            formed_at: draft.formed_at,
            as_of: draft.as_of,
            status: HypothesisStatus::Draft,
            class: draft.class,
            claim: draft.claim,
            statement: draft.statement,
            subjects: draft.subjects,
            chain: draft.chain,
            evidence,
            prior: draft.prior,
            // The mechanism gates how much the evidence is allowed to move
            // the belief: a weak causal story means the evidence tells you
            // less about *this* claim, so confidence falls back toward the
            // base rate rather than toward zero.
            confidence: attenuate(draft.prior, belief.posterior, chain_weight),
            belief,
            origin_factors,
            falsifiers: draft.falsifiers,
            leading_alternative: draft.leading_alternative,
            horizon: draft.horizon,
            contributors: draft.contributors,
            models: draft.models,
        };
        hypothesis.validate()?;
        Ok(hypothesis)
    }

    /// Recompute confidence after the evidence set changed.
    pub fn revise(&mut self) {
        self.belief = update(self.prior, &strengths(&self.evidence, &self.origin_factors));
        let chain = self.chain.confidence().max(MINIMUM_CHAIN_WEIGHT);
        self.confidence = attenuate(self.prior, self.belief.posterior, chain);
    }

    /// Add evidence and recompute. The only way confidence changes.
    pub fn add_evidence(&mut self, evidence: crate::evidence::Evidence) -> Result<()> {
        evidence.validate()?;
        if !evidence.was_knowable_at(self.as_of) {
            return Err(Error::invalid(format!(
                "evidence {} became knowable at {} which is after the hypothesis as-of {}",
                evidence.evidence_id.as_str(),
                evidence.known_at,
                self.as_of
            )));
        }
        self.evidence.push(evidence);
        self.revise();
        Ok(())
    }

    pub fn expires_at(&self) -> Timestamp {
        self.as_of.saturating_add(self.horizon)
    }

    /// Whether the horizon has passed and the claim can be scored.
    pub fn is_resolvable(&self, now: Timestamp) -> bool {
        now >= self.expires_at()
    }

    /// Confidence adjusted for how concentrated the supporting evidence is.
    ///
    /// A thesis whose support all traces to one origin is one source's opinion
    /// however many documents restate it.
    pub fn effective_confidence(&self) -> f64 {
        let concentration = self.evidence.concentration();
        // Full weight up to half the support from one origin, falling linearly
        // to 60% when it all comes from one.
        let penalty = if concentration <= 0.5 {
            1.0
        } else {
            1.0 - 0.8 * (concentration - 0.5)
        };
        self.confidence * penalty
    }

    /// Whether the hypothesis meets the bar to be proposed to a portfolio.
    ///
    /// Deliberately several independent conditions rather than one score: a
    /// thesis that fails any of them fails for a reason that can be stated.
    pub fn meets_action_bar(&self, minimum_confidence: f64) -> std::result::Result<(), String> {
        if !self.status.is_actionable() {
            return Err(format!("status is {}", self.status.as_str()));
        }
        if self.effective_confidence() < minimum_confidence {
            return Err(format!(
                "effective confidence {:.3} below the {:.3} required",
                self.effective_confidence(),
                minimum_confidence
            ));
        }
        if !self.evidence.has_primary_source() {
            return Err("no primary source supports the claim".to_string());
        }
        if self.belief.dominance() > 0.75 && self.evidence.len() > 1 {
            return Err(format!(
                "{:.0}% of the evidence weight rests on one item",
                self.belief.dominance() * 100.0
            ));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.statement.trim().is_empty() {
            return Err(Error::invalid("hypothesis has no statement"));
        }
        if self.class.trim().is_empty() {
            return Err(Error::invalid(format!(
                "hypothesis {} declares no class, so its prior cannot be checked against a base rate",
                self.hypothesis_id.as_str()
            )));
        }
        if self.subjects.is_empty() {
            return Err(Error::invalid(format!(
                "hypothesis {} is about nothing in particular",
                self.hypothesis_id.as_str()
            )));
        }
        self.chain.validate()?;
        if self.falsifiers.is_empty() {
            return Err(Error::invalid(format!(
                "hypothesis {} states no falsifier; an unfalsifiable claim cannot be reviewed or learned from",
                self.hypothesis_id.as_str()
            )));
        }
        if self.leading_alternative.trim().is_empty() {
            return Err(Error::invalid(format!(
                "hypothesis {} considers no alternative explanation",
                self.hypothesis_id.as_str()
            )));
        }
        if self.evidence.with_stance(Stance::Supports).is_empty() {
            return Err(Error::invalid(format!(
                "hypothesis {} has no supporting evidence",
                self.hypothesis_id.as_str()
            )));
        }
        if self.formed_at < self.as_of {
            return Err(Error::invalid(format!(
                "hypothesis {} was formed at {} for an as-of time of {}, which is in its future",
                self.hypothesis_id.as_str(),
                self.formed_at,
                self.as_of
            )));
        }
        if self.horizon.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "hypothesis {} has no horizon, so it can never be scored",
                self.hypothesis_id.as_str()
            )));
        }
        // A factor outside (0, 1] is not an accuracy: above one it would let
        // a component raise its own weight, and at or below zero it would
        // silence or invert the evidence rather than discount it.
        validate_factors(&self.origin_factors)?;
        // The stated confidence must be the one the evidence implies. This is
        // what stops a confidence being edited in isolation. The recorded
        // factors are part of that arithmetic, which is why a replay carries
        // them rather than re-reading a self-model that has since moved.
        let expected = attenuate(
            self.prior,
            self.belief.posterior,
            self.chain.confidence().max(MINIMUM_CHAIN_WEIGHT),
        );
        if (self.confidence - expected).abs() > 1e-9 {
            return Err(Error::invalid(format!(
                "hypothesis {} states confidence {:.6} but its evidence implies {expected:.6}",
                self.hypothesis_id.as_str(),
                self.confidence
            )));
        }
        Ok(())
    }
}

/// Floor on how much a weak causal chain can discount the evidence.
///
/// Without a floor a long chain would suppress the evidence entirely and every
/// such hypothesis would sit exactly at its base rate, which throws away real
/// information rather than discounting it.
const MINIMUM_CHAIN_WEIGHT: f64 = 0.30;

/// Refuse a self-model factor that is not an accuracy in `(0, 1]`.
fn validate_factors(factors: &BTreeMap<String, f64>) -> Result<()> {
    for (origin, factor) in factors {
        if !(factor.is_finite() && *factor > 0.0 && *factor <= 1.0) {
            return Err(Error::invalid(format!(
                "self-model factor {factor} for origin {origin:?} is not an accuracy in (0, 1]; \
                 a factor above one would let a component raise its own weight and one at zero \
                 would silence it rather than discount it"
            )));
        }
    }
    Ok(())
}

/// The evidence as the belief updater sees it, each *supporting* item's
/// weight scaled by its origin's self-model factor where one was recorded.
///
/// Contrary evidence is never discounted. A factor is an origin's measured
/// accuracy, and applying it to a dissent as well as to a claim would let a
/// poorly-calibrated dissenter be talked down: an analyst measured at 0.15
/// would have its objection cut to fifteen percent, the posterior would
/// *rise* relative to the unweighted computation, and a hypothesis could
/// cross the action bar because the voice against it had a bad record. The
/// self-model exists so that being wrong costs a component influence over
/// what the platform believes; it must not buy a hypothesis the benefit of
/// its critics' doubt. So the conservative reading: a factor can only ever
/// lower a posterior, and the worst a discounted origin can do to a thesis
/// it opposes is oppose it at full weight.
fn strengths(evidence: &EvidenceSet, factors: &BTreeMap<String, f64>) -> Vec<EvidenceStrength> {
    evidence
        .iter()
        .filter(|e| e.stance != Stance::Contextual)
        .map(|e| {
            let signed_weight = e.signed_weight();
            let factor = if signed_weight > 0.0 {
                factors.get(&e.origin).copied().unwrap_or(1.0)
            } else {
                1.0
            };
            EvidenceStrength {
                id: e.evidence_id.as_str().to_string(),
                signed_weight: signed_weight * factor,
                origin: e.origin.clone(),
            }
        })
        .collect()
}
