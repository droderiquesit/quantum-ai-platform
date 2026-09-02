//! The REASON stage.
//!
//! Takes the findings of the agent organisation and produces reviewed
//! hypotheses. The pipeline is: synthesise a draft from the findings, form it
//! (which computes the confidence), attack it, and record the verdict.
//!
//! Two things this deliberately does not do:
//!
//! * It does not average agent convictions into a house view. Averaging turns
//!   one agent's strong, well-evidenced objection into a small adjustment.
//!   Disagreement is carried through to the hypothesis as contradicting
//!   evidence and let the red team weigh it.
//! * It does not ask a language model whether the thesis is any good. Every
//!   number here comes from the evidence set, and a model's role — where one
//!   is configured at all — is narrative, not judgement.

use crate::evidence::{Evidence, EvidenceKind, EvidenceSet, Stance};
use crate::hypothesis::{CausalChain, Claim, Hypothesis, HypothesisDraft, HypothesisStatus};
use crate::redteam::{RedTeam, ReviewOutcome, ReviewPolicy};
use qip_agents::finding::{AgentFinding, Direction, FindingStatus};
use qip_core::error::{Error, Result};
use qip_core::ids::{ChallengeId, EvidenceId, HypothesisId, ObjectId, OpportunityId};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// Everything the engine needs to form one hypothesis.
#[derive(Clone, Debug)]
pub struct SynthesisInput {
    pub hypothesis_id: HypothesisId,
    pub opportunity_id: Option<OpportunityId>,
    /// The point in time the reasoning happens as of.
    pub as_of: Timestamp,
    /// When the engine is running. Must not precede `as_of`.
    pub now: Timestamp,
    /// The hypothesis class, which determines the prior.
    pub class: String,
    pub claim: Claim,
    pub statement: String,
    pub subjects: Vec<ObjectId>,
    pub chain: CausalChain,
    /// Findings from the agent organisation.
    pub findings: Vec<AgentFinding>,
    /// Evidence gathered outside the agent runs — filings, prices, computations.
    pub direct_evidence: EvidenceSet,
    /// The base rate for the class. Supplied rather than chosen here, so it
    /// cannot be tuned to the conclusion.
    pub prior: f64,
    pub falsifiers: Vec<String>,
    pub leading_alternative: String,
    pub horizon: Duration,
    /// The platform's estimate of how much of the claim is already priced.
    pub market_priced_in: Option<f64>,
    pub models: Vec<String>,
}

/// What the REASON stage produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReasoningOutcome {
    pub hypothesis: Hypothesis,
    pub review: ReviewOutcome,
    /// Agents that declined the question, recorded so a thesis nobody
    /// competent looked at is visible as such.
    pub deferred_by: Vec<String>,
    /// Agents that disagreed, by id. Kept prominent: a thesis three agents
    /// argued against reads differently from one nobody questioned.
    pub dissenters: Vec<String>,
    /// Agents that reported missing data, and what was missing.
    pub missing_inputs: Vec<String>,
}

impl ReasoningOutcome {
    pub fn is_actionable(&self) -> bool {
        self.hypothesis.status.is_actionable()
    }

    /// A one-paragraph account of the reasoning, for the decision record.
    pub fn narrate(&self) -> String {
        let mut parts = vec![
            self.hypothesis.statement.clone(),
            format!("Mechanism: {}.", self.hypothesis.chain.narrate()),
            format!(
                "Confidence {:.2} from a prior of {:.2} on {} pieces of evidence across {} independent origin(s).",
                self.hypothesis.effective_confidence(),
                self.hypothesis.prior,
                self.hypothesis.evidence.len(),
                self.hypothesis.evidence.origins().len()
            ),
        ];
        if !self.dissenters.is_empty() {
            parts.push(format!("Dissent from: {}.", self.dissenters.join(", ")));
        }
        if !self.missing_inputs.is_empty() {
            parts.push(format!("Missing: {}.", self.missing_inputs.join("; ")));
        }
        parts.push(format!(
            "Review: {} — {}.",
            self.review.verdict.as_str(),
            self.review.rationale
        ));
        parts.join(" ")
    }
}

/// The REASON stage.
#[derive(Debug)]
pub struct ReasoningEngine {
    red_team: RedTeam,
    /// Counter for deterministic evidence and challenge ids.
    sequence: u64,
    /// Confidence a hypothesis must reach to be worth proposing.
    action_bar: f64,
}

impl ReasoningEngine {
    pub fn new(policy: ReviewPolicy) -> Self {
        Self {
            red_team: RedTeam::new(policy),
            sequence: 0,
            action_bar: 0.60,
        }
    }

    pub fn red_team(&self) -> &RedTeam {
        &self.red_team
    }

    /// Form and review one hypothesis.
    pub fn reason(&mut self, input: SynthesisInput) -> Result<ReasoningOutcome> {
        if input.now < input.as_of {
            return Err(Error::invalid(format!(
                "reasoning at {} for an as-of time of {}, which is in its future",
                input.now, input.as_of
            )));
        }

        let mut evidence = input.direct_evidence.as_of(input.as_of);
        let mut deferred_by = Vec::new();
        let mut dissenters = Vec::new();
        let mut missing_inputs = Vec::new();

        for finding in &input.findings {
            if finding.as_of > input.as_of {
                return Err(Error::invalid(format!(
                    "finding from {} reasons as of {}, after this hypothesis's {}",
                    finding.agent_id, finding.as_of, input.as_of
                )));
            }
            for missing in &finding.missing_inputs {
                missing_inputs.push(format!("{}: {missing}", finding.agent_id));
            }
            if finding.status == FindingStatus::Deferred {
                deferred_by.push(finding.agent_id.clone());
                continue;
            }
            let Some(stance) = stance_of(input.claim, finding) else {
                continue;
            };
            if stance == Stance::Contradicts {
                dissenters.push(finding.agent_id.clone());
            }
            self.sequence += 1;
            evidence.push(evidence_from_finding(
                finding,
                stance,
                EvidenceId::from_string(format!("ev-{}-{}", finding.agent_id, self.sequence)),
            ));
        }

        let hypothesis = Hypothesis::form(HypothesisDraft {
            hypothesis_id: input.hypothesis_id,
            opportunity_id: input.opportunity_id,
            formed_at: input.now,
            as_of: input.as_of,
            class: input.class,
            claim: input.claim,
            statement: input.statement,
            subjects: input.subjects,
            chain: input.chain,
            evidence,
            prior: input.prior,
            falsifiers: input.falsifiers,
            leading_alternative: input.leading_alternative,
            horizon: input.horizon,
            contributors: input
                .findings
                .iter()
                .map(|f| f.run_id.as_str().to_string())
                .collect(),
            models: input.models,
        })?;

        let base = self.sequence;
        let mut counter = 0u64;
        let mut next_id = || {
            counter += 1;
            ChallengeId::from_string(format!("ch-{base}-{counter}"))
        };
        let review =
            self.red_team
                .review(&hypothesis, input.market_priced_in, input.now, &mut next_id);
        self.sequence += counter;

        let mut hypothesis = hypothesis;
        hypothesis.status = review.verdict;

        Ok(ReasoningOutcome {
            hypothesis,
            review,
            deferred_by,
            dissenters,
            missing_inputs,
        })
    }

    /// Whether an outcome clears the bar to be proposed to a portfolio.
    ///
    /// Returns the reason for refusal rather than a bare boolean: "did not
    /// meet the bar" is not something anyone can act on.
    pub fn clears_action_bar(&self, outcome: &ReasoningOutcome) -> std::result::Result<(), String> {
        if outcome.hypothesis.status != HypothesisStatus::Approved {
            return Err(format!(
                "hypothesis was {}: {}",
                outcome.hypothesis.status.as_str(),
                outcome.review.rationale
            ));
        }
        outcome.hypothesis.meets_action_bar(self.action_bar)
    }
}

/// Which way a finding cuts for a claim.
///
/// A finding's direction is about the world, not about the hypothesis, so it
/// has to be interpreted against what the hypothesis claims. `None` means the
/// finding says nothing either way and should not be counted at all — the
/// alternative, counting it as neutral evidence, would dilute the real
/// evidence with silence.
fn stance_of(claim: Claim, finding: &AgentFinding) -> Option<Stance> {
    if finding.status == FindingStatus::NoView || finding.direction == Direction::Neutral {
        return None;
    }
    if finding.direction == Direction::Ambiguous {
        // A genuinely two-sided finding is context: it bears on the situation
        // without deciding it.
        return Some(Stance::Contextual);
    }
    let claim_sign = claim.implied_sign()?;
    let finding_sign = finding.direction.sign();
    if claim_sign * finding_sign > 0.0 {
        Some(Stance::Supports)
    } else {
        Some(Stance::Contradicts)
    }
}

/// Turn an agent finding into evidence.
///
/// The origin is the agent, so two findings from the same agent are correctly
/// treated as correlated. Reliability comes from the agent's own effective
/// conviction — which is already discounted for a partial answer — and the
/// kind is `Computation`, because that is what an agent's analysis is.
fn evidence_from_finding(
    finding: &AgentFinding,
    stance: Stance,
    evidence_id: EvidenceId,
) -> Evidence {
    Evidence::new(
        evidence_id,
        EvidenceKind::Computation,
        stance,
        finding.claim.clone(),
        finding.run_id.as_str().to_string(),
        finding.agent_id.clone(),
        finding.as_of,
        finding.produced_at,
    )
    .with_reliability(finding.effective_conviction())
    // Diagnosticity comes from how much the agent actually grounded the view:
    // a finding citing five records bears on the question more than a bare
    // assertion, and this is the only place that difference gets expressed.
    .with_diagnosticity(diagnosticity_of(finding))
}

fn diagnosticity_of(finding: &AgentFinding) -> f64 {
    let grounding = finding.evidence.len() + finding.facts.len();
    match grounding {
        0 => 0.20,
        1 => 0.40,
        2 => 0.55,
        3..=4 => 0.70,
        _ => 0.80,
    }
}
