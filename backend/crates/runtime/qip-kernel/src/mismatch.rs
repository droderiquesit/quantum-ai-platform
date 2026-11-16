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

/// The hypothesis family every mismatch proposal is charged to. One family,
/// not one per event: the ledger tracks a bounded number of families, and a
/// family per event would let proposals alone fill it.
pub const PROPOSAL_FAMILY: &str = "world_market_mismatch/relative_value";

/// Proposed hypotheses a falsification book's working set holds before it
/// evicts the oldest. **Provisional**: no figure was given, so this is chosen
/// in the restrictive direction (half the family trial budget of 64) and is
/// named here so it can be changed by decision, not found. Evictions are
/// counted and journaled, never silent.
pub const PROPOSED_CAPACITY: usize = 32;

/// What `falsification::Book::propose` did with one proposal.
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalOutcome {
    /// Charged to the family and held; `evicted` is the oldest proposal the
    /// working set let go to make room, if it was full.
    Admitted {
        spent: usize,
        budget: usize,
        evicted: Option<ProposedHypothesis>,
    },
    /// The event and instrument were already proposed; nothing was charged.
    Duplicate,
    /// The family had no trial budget left; nothing was charged or held.
    BudgetExhausted { spent: usize, budget: usize },
}

/// What happened to a proposal, as the log records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAction {
    Admitted,
    Evicted,
    RefusedOnBudget,
}

/// One proposal event on the log (ADR 0609): the admission of a hypothesis,
/// the eviction of an older one, or a refusal because the family's budget was
/// spent. Journaled under `Topic::HypothesisCreated`, keyed so a re-delivery
/// is suppressed. The log is what the working set is rebuilt from
/// ([`replay`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MismatchProposalJournal {
    pub action: ProposalAction,
    pub hypothesis: ProposedHypothesis,
    pub family: String,
    /// The family's trials spent after this event.
    pub spent: usize,
    pub budget: usize,
    /// A sequence the journaling platform assigns, strictly increasing, so two
    /// events about one idea have distinct keys.
    pub sequence: u64,
}

impl qip_events::EventBody for MismatchProposalJournal {
    const TOPIC: qip_events::Topic = qip_events::Topic::HypothesisCreated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("mismatch-proposal:{}", self.sequence))
    }
}

/// The origin the kernel journals mismatch proposals under.
pub const PROPOSAL_ORIGIN: &str = "kernel/mismatch-proposal";

/// The working set and the trials spent, rebuilt from the journal alone, in
/// sequence order. Equal to the book's own state when the journal is whole;
/// the test that says so is what keeps the log, not the book, the record.
pub fn replay(entries: &[MismatchProposalJournal]) -> (Vec<ProposedHypothesis>, usize) {
    let mut sorted: Vec<&MismatchProposalJournal> = entries.iter().collect();
    sorted.sort_by_key(|e| e.sequence);
    let mut live: Vec<ProposedHypothesis> = Vec::new();
    let mut spent = 0;
    for entry in sorted {
        match entry.action {
            ProposalAction::Admitted => {
                live.push(entry.hypothesis.clone());
                spent = entry.spent;
            }
            ProposalAction::Evicted => {
                live.retain(|h| {
                    !(h.event == entry.hypothesis.event
                        && h.instrument == entry.hypothesis.instrument)
                });
            }
            ProposalAction::RefusedOnBudget => {}
        }
    }
    (live, spent)
}

/// What one `Platform::admit_proposed_hypotheses` call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MismatchAdmission {
    pub admitted: usize,
    pub duplicates: usize,
    /// Refused because the family's trial budget was spent.
    pub refused_on_budget: usize,
    /// Older proposals evicted to make room; each is journaled.
    pub evicted: usize,
}

/// Every proposal event the log holds, oldest first. A frame under the
/// proposal origin that does not decode is an error, so a reader counts the
/// refusal rather than sweeping over a shorter list.
pub fn from_log(
    log: &qip_events::EventLog,
) -> qip_core::error::Result<Vec<MismatchProposalJournal>> {
    use qip_streaming::envelope::StreamEnvelope;
    log.events()
        .filter(|frame| {
            frame.topic == <MismatchProposalJournal as qip_events::EventBody>::TOPIC
                && frame.lineage.producer == PROPOSAL_ORIGIN
        })
        .map(|frame| {
            let envelope = StreamEnvelope::from_frame(frame)?;
            Ok(envelope.decode::<MismatchProposalJournal>()?.body)
        })
        .collect()
}

/// The falsification book and the next journal sequence, rebuilt from the log
/// at assembly (R12-TICK-2084). The book gets the working set and the trials
/// spent that [`replay`] derives; the sequence continues from the highest the
/// log holds, so a key `mismatch-proposal:{sequence}` is never reused by a
/// later process over the same log.
pub fn resume(
    log: &qip_events::EventLog,
) -> qip_core::error::Result<(crate::falsification::Book, u64)> {
    let entries = from_log(log)?;
    let (live, spent) = replay(&entries);
    let count = |a: ProposalAction| entries.iter().filter(|e| e.action == a).count() as u64;
    let sequence = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
    let book = crate::falsification::Book::restored(
        live,
        spent,
        count(ProposalAction::Evicted),
        count(ProposalAction::RefusedOnBudget),
    )?;
    Ok((book, sequence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use crate::platform::Platform;
    use qip_core::Context;
    use qip_core::time::Timestamp;
    use qip_financial::universe::Universe;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Platform {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(at(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    fn proposal(n: usize) -> ProposedHypothesis {
        ProposedHypothesis {
            event: format!("evt-{n}"),
            instrument: format!("INST{n}"),
            expected_direction: 1.0,
            measured_bps: -10.0,
            status: HypothesisStatus::Proposed,
        }
    }

    /// Mutation: delete the `charge_proposal` call in `Book::propose` — the
    /// family's spent count stays 0 and the first assertion fails.
    /// Mutation: skip the journaling loop in `admit_proposed_hypotheses` —
    /// the journal is empty and the replay assertion fails.
    #[test]
    fn a_proposal_is_charged_to_its_family_journaled_and_rebuilt_from_the_log() {
        let mut platform = platform();
        let ledger_spent = |p: &Platform| p.falsification().ledger().spent(PROPOSAL_FAMILY);
        assert_eq!(ledger_spent(&platform), 0, "premise: nothing spent");
        let admission = platform
            .admit_proposed_hypotheses(vec![proposal(1), proposal(1)], at())
            .expect("admits");
        assert_eq!((admission.admitted, admission.duplicates), (1, 1));
        assert_eq!(ledger_spent(&platform), 1, "the proposal was not charged");
        let log = from_log(platform.event_log()).expect("decodes");
        assert_eq!(
            log.len(),
            1,
            "a duplicate was journaled or an admission was not"
        );
        let (live, spent) = replay(&log);
        assert_eq!(live, platform.falsification().proposed());
        assert_eq!(spent, 1);
    }

    /// Mutation: in `Book::propose` drop the `refused_on_budget` branch's
    /// early return — the 65th proposal is admitted and the budget assertion
    /// fails. Mutation: drop the `evicted` count — the counter assertion fails.
    #[test]
    fn the_cap_and_the_budget_count_and_journal_what_they_drop() {
        let mut platform = platform();
        let budget = platform.falsification().ledger().budget();
        assert!(PROPOSED_CAPACITY < budget, "premise: eviction is reachable");
        let total = budget + 6;
        let admission = platform
            .admit_proposed_hypotheses((0..total).map(proposal).collect(), at())
            .expect("admits");
        assert_eq!(admission.admitted, budget);
        assert_eq!(admission.refused_on_budget, 6);
        assert_eq!(admission.evicted, budget - PROPOSED_CAPACITY);
        let book = platform.falsification();
        assert_eq!(book.proposals_refused_on_budget(), 6);
        assert_eq!(
            book.proposals_evicted() as usize,
            budget - PROPOSED_CAPACITY
        );
        assert_eq!(book.proposed().len(), PROPOSED_CAPACITY);
        let log = from_log(platform.event_log()).expect("decodes");
        let count = |a: ProposalAction| log.iter().filter(|e| e.action == a).count();
        assert_eq!(count(ProposalAction::Admitted), budget);
        assert_eq!(count(ProposalAction::Evicted), budget - PROPOSED_CAPACITY);
        assert_eq!(count(ProposalAction::RefusedOnBudget), 6);
        let (live, spent) = replay(&log);
        assert_eq!(
            live,
            book.proposed(),
            "the log does not rebuild the working set"
        );
        assert_eq!(spent, budget);
    }
}
