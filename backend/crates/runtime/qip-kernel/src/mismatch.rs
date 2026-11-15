//! One paper-only consumer per kind of world/market mismatch (R12-TICK-2084;
//! ADR 0609).
//!
//! `fusion::fuse` has long named three kinds of mismatch. Before this module
//! the tick desk counted them (`TickRound::mismatches`) and dropped them, so
//! a market that disagreed with the world model raised nothing: the
//! requirement's "raises an investigation, an arbitrage/relative-value
//! hypothesis or an evidence-quality challenge" held only as a number.
//!
//! Each kind goes to exactly one consumer, and none of them can size or
//! submit anything:
//!
//! - [`Mismatch::Investigation`] becomes a [`ResearchQuestion`], which a
//!   discovery scout looks for evidence on.
//! - [`Mismatch::EvidenceQualityChallenge`] becomes an [`EvidenceChallenge`]:
//!   it asks discovery for a better source, and its only effect on a weight
//!   is [`EvidenceChallenge::lowered`], which can never raise one.
//! - [`Mismatch::RelativeValueHypothesis`] becomes a [`ProposedHypothesis`]
//!   entering `falsification::Book` as proposed. The type has no quantity,
//!   price or side field, so nothing downstream can size from it.

use crate::fusion::{FusedState, Mismatch};
use serde::{Deserialize, Serialize};

/// An expected reaction was absent and the evidence was sound: find out why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchQuestion {
    pub event: String,
    pub instrument: String,
    pub reason: String,
    /// The world model's probability that the instrument reacts, as the
    /// fusion held it. The scout rank is derived from it, not chosen.
    pub prior: f64,
}

/// An expected reaction was absent and the evidence was weak.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceChallenge {
    pub event: String,
    pub evidence_quality: f64,
    pub reason: String,
    /// The instruments the event was watched through: what a better source
    /// would be looked for about.
    pub instruments: Vec<String>,
}

impl EvidenceChallenge {
    /// The weight of the challenged evidence after the challenge. A
    /// challenge may only lower a weight: this is its one operation, and it
    /// is `min`, so a weight already below the measured quality stays.
    pub fn lowered(&self, weight: f64) -> f64 {
        weight.min(self.evidence_quality)
    }
}

/// A hypothesis's standing. One variant: nothing here promotes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    Proposed,
}

/// The market moved against the expected direction: a relative-value
/// hypothesis, proposed and nothing more. Deliberately without a quantity,
/// price or side, so it cannot be mistaken for an order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposedHypothesis {
    pub event: String,
    pub instrument: String,
    pub expected_direction: f64,
    pub measured_bps: f64,
    pub status: HypothesisStatus,
}

/// Where one fused state's mismatches went.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MismatchRouting {
    pub questions: Vec<ResearchQuestion>,
    pub challenges: Vec<EvidenceChallenge>,
    pub proposals: Vec<ProposedHypothesis>,
}

impl MismatchRouting {
    /// How many mismatches were routed, which equals the fused state's own
    /// count: a mismatch is never dropped.
    pub fn total(&self) -> usize {
        self.questions.len() + self.challenges.len() + self.proposals.len()
    }
}

/// Route every mismatch of `fused` to its consumer's record.
pub fn route(fused: &FusedState) -> MismatchRouting {
    let mut routing = MismatchRouting::default();
    for event in &fused.events {
        for mismatch in &event.mismatches {
            match mismatch {
                Mismatch::Investigation { instrument, reason } => {
                    let prior = event
                        .priors
                        .iter()
                        .find(|p| &p.instrument == instrument)
                        .map_or(0.0, |p| p.probability);
                    routing.questions.push(ResearchQuestion {
                        event: event.event.clone(),
                        instrument: instrument.clone(),
                        reason: reason.clone(),
                        prior,
                    });
                }
                Mismatch::EvidenceQualityChallenge {
                    evidence_quality,
                    reason,
                } => routing.challenges.push(EvidenceChallenge {
                    event: event.event.clone(),
                    evidence_quality: *evidence_quality,
                    reason: reason.clone(),
                    instruments: event.watch.instruments.keys().cloned().collect(),
                }),
                Mismatch::RelativeValueHypothesis {
                    instrument,
                    expected_direction,
                    measured_bps,
                } => routing.proposals.push(ProposedHypothesis {
                    event: event.event.clone(),
                    instrument: instrument.clone(),
                    expected_direction: *expected_direction,
                    measured_bps: *measured_bps,
                    status: HypothesisStatus::Proposed,
                }),
            }
        }
    }
    routing
}

/// Proposed hypotheses a falsification book holds at most.
pub const PROPOSED_CAPACITY: usize = 256;
