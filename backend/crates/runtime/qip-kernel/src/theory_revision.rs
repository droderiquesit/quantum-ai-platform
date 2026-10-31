//! Where the symbolic rule pack's refusals are held against what the world
//! then did (R12-EXPAND-6044).
//!
//! `symbolic_stage::rule_pack` is fixed at build time and can veto a
//! hypothesis review approved (ADR 0125 decision 2): a live ceiling, one lot
//! past the position budget, cash short of one lot. Until this module the
//! veto was the end of the matter. The hypothesis still became a prediction,
//! LEARN still graded it when its horizon closed, and a thesis the pack had
//! refused that the grader then credited was a contradiction between the
//! platform's theory and its own evidence that nothing recorded and nothing
//! turned into work. The requirement's words are that contradictions generate
//! theory-revision tasks; the platform had the contradiction and no task.
//!
//! The seam is in two halves, and the second never changes a rule:
//!
//! * REASON notes each veto under the hypothesis it refused, with the ids of
//!   the rules and invariants whose firing made it ([`SymbolicVetoNote`]).
//!   Bounded by [`SYMBOLIC_VETO_HISTORY`]; a veto that arrives while the
//!   window is full is reported, never queued, because evicting the oldest
//!   note would silently choose which contradiction goes unexamined.
//! * LEARN, at the instant a thesis is graded, reads the note back. A verdict
//!   the grader credits (`Verdict::counts_as_correct`) against a thesis the
//!   pack refused produces one [`TheoryDelta`], validated and journaled
//!   under [`THEORY_DELTA_ORIGIN`]. Any other verdict closes the note: the
//!   veto was upheld, and there is nothing to revise.
//!
//! A delta is a proposal. Nothing in this crate reads one to move a rule, the
//! pack has no setter, and the deep brain's research loop reads the journaled
//! records off the log and raises a `GapTrigger::Contradiction` gap for each,
//! which is where the revision task is queued. The guard that keeps this a
//! proposal is the same one §12.4 holds for risk rules: there is no code path
//! that assigns a pack after boot.

use crate::platform::Platform;
use qip_contracts::expansion::TheoryDelta;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_events::{EventBody, Topic};
use qip_learning_engine::evaluation::Evaluation;
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};

/// Origin of a journaled [`TheoryDeltaRecord`].
pub const THEORY_DELTA_ORIGIN: &str = "kernel/theory-delta";
const THEORY_DELTA_PREFIX: &str = "theory-delta/";

/// How many vetoed hypotheses may await grading at once. The same working
/// window as `DECLINED_HISTORY`, for the same reason: a hypothesis vetoed
/// this many vetoes ago and still not graded is one whose horizon never
/// closed, and a window that evicted it would be choosing which veto goes
/// unexamined.
pub const SYMBOLIC_VETO_HISTORY: usize = 256;

/// One hypothesis the rule pack refused, kept until LEARN grades it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolicVetoNote {
    pub hypothesis_id: String,
    pub cycle: u64,
    pub at: Timestamp,
    /// The pack and version that refused, as the derivation recorded them.
    pub pack: String,
    pub version: u32,
    /// The rule and invariant ids whose firing made the refusal, from
    /// `SymbolicDerivation::veto_rules`.
    pub rules: Vec<String>,
    /// The veto as REASON reported it.
    pub veto: String,
}

/// A journaled theory delta, with the cycle whose LEARN pass produced it.
///
/// On `hypothesis.rejected` because the delta is the rejection's post-mortem
/// — the record of a refusal the outcome contradicted — and because that
/// topic carried no other body: a second body on one topic is the decode
/// failure `references.rs` records having shipped once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TheoryDeltaRecord {
    pub delta: TheoryDelta,
    pub cycle: u64,
}

impl EventBody for TheoryDeltaRecord {
    const TOPIC: Topic = Topic::HypothesisRejected;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        // One delta per hypothesis: a thesis is graded once, so a second
        // record under the same id would be a replay and dedupes.
        Some(format!("{THEORY_DELTA_PREFIX}{}", self.delta.hypothesis_id))
    }
}

/// What one LEARN pass did with the vetoes whose theses it graded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TheoryRevision {
    /// Deltas journaled this pass.
    pub produced: usize,
    /// Notes closed because the grader did not credit the thesis.
    pub upheld: usize,
    pub problems: Vec<String>,
}

impl Platform {
    /// REASON's half: remember that the pack refused `hypothesis_id`, so
    /// LEARN can hold the refusal against the grade. A full window refuses
    /// the note and says so; see [`SYMBOLIC_VETO_HISTORY`].
    pub(crate) fn note_symbolic_veto(&mut self, note: SymbolicVetoNote) -> Option<String> {
        if self.symbolic_vetoes.contains_key(&note.hypothesis_id) {
            return None;
        }
        if self.symbolic_vetoes.len() >= SYMBOLIC_VETO_HISTORY {
            return Some(format!(
                "the rule pack's veto of {} will not be held against its grade: \
                 {SYMBOLIC_VETO_HISTORY} vetoed theses are already awaiting one",
                note.hypothesis_id
            ));
        }
        self.symbolic_vetoes
            .insert(note.hypothesis_id.clone(), note);
        None
    }

    /// LEARN's half: for every graded thesis the pack had refused, journal a
    /// [`TheoryDelta`] when the grader credited the thesis and close the note
    /// otherwise. `cycle` is the LEARN pass's own, handed in because the
    /// platform's counter is not this module's to read.
    pub(crate) fn revise_theory(
        &mut self,
        evaluations: &[Evaluation],
        cycle: u64,
        now: Timestamp,
    ) -> TheoryRevision {
        let mut revision = TheoryRevision::default();
        for evaluation in evaluations {
            let Some(note) = self.symbolic_vetoes.remove(&evaluation.hypothesis_id) else {
                continue;
            };
            if !evaluation.verdict.counts_as_correct() {
                revision.upheld += 1;
                continue;
            }
            let verdict = format!("{:?}", evaluation.verdict).to_ascii_lowercase();
            let delta = TheoryDelta {
                pack: note.pack.clone(),
                version: note.version,
                rules: note.rules.clone(),
                hypothesis_id: note.hypothesis_id.clone(),
                proposed_change: format!(
                    "review {}: the stop refused a thesis the grader then marked {verdict} \
                     ({:.0}bp realised against {:.0}bp expected); the bound it fired on is to be \
                     re-read against this outcome, by a reviewer, and is not moved here",
                    note.rules.join(", "),
                    evaluation.realised_move_bps,
                    evaluation.expected_move_bps,
                ),
                contradicting_evidence: vec![
                    format!("hypothesis:{}", note.hypothesis_id),
                    format!("veto:{}", note.veto),
                    format!("verdict:{verdict}"),
                    format!("vetoed_cycle:{}", note.cycle),
                    format!("graded_cycle:{cycle}"),
                ],
                proposed_at: now,
            };
            if let Err(error) = delta.validate() {
                revision.problems.push(format!(
                    "a theory delta for {} was indicated and refused: {}",
                    note.hypothesis_id,
                    error.message()
                ));
                continue;
            }
            match self.journal_record(TheoryDeltaRecord { delta, cycle }, THEORY_DELTA_ORIGIN, now)
            {
                Ok(()) => revision.produced += 1,
                Err(error) => revision.problems.push(format!(
                    "a theory delta for {} was computed and not journalled: {}",
                    note.hypothesis_id,
                    error.message()
                )),
            }
        }
        revision
    }

    /// Every theory delta the log holds, oldest first, as the deep brain's
    /// research loop reads them (R12-EXPAND-6044). A frame under the
    /// delta prefix that does not decode is an error, so a reader can count
    /// the refusal rather than sweep over a silently shorter list.
    pub fn theory_deltas(&self) -> Result<Vec<TheoryDelta>> {
        self.event_log()
            .by_topic(Topic::HypothesisRejected)
            .into_iter()
            .filter(|frame| {
                frame
                    .idempotency_key
                    .as_deref()
                    .is_some_and(|key| key.starts_with(THEORY_DELTA_PREFIX))
            })
            .map(|frame| {
                let envelope = StreamEnvelope::from_frame(frame)?;
                Ok(envelope.decode::<TheoryDeltaRecord>()?.body.delta)
            })
            .collect()
    }

    /// Vetoed theses still awaiting a grade, by hypothesis id.
    pub fn symbolic_vetoes_pending(&self) -> impl Iterator<Item = &SymbolicVetoNote> {
        self.symbolic_vetoes.values()
    }
}
