//! Closed-loop research prompts from failure clusters (blueprint §22, MODEL-046).
//!
//! One wrong thesis is noise; the same hypothesis class failing again and
//! again is a question nobody has asked the research side. Without a record
//! of that question the platform keeps charging the failures to the
//! components' track records and learns nothing it can act on. The ledger
//! here turns a *cluster* — [`MINIMUM_CLUSTER`] failed theses of one class —
//! into exactly one [`ResearchPrompt`] naming the episodes, and keeps the
//! prompt open until a named research task answers it, so an unanswered
//! prompt is visible rather than forgotten.
//!
//! What it deliberately does not do: start research, move capital, or change
//! any model. A prompt is a record; acting on one goes through the foundry's
//! own training, evaluation and promotion (MODEL-015).

use crate::evaluation::{Evaluation, Verdict};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Failed theses of one class that make a cluster. Below it a failure is
/// indistinguishable from the ordinary miss rate of any forecaster.
pub const MINIMUM_CLUSTER: usize = 3;

/// Open (unanswered) prompts the ledger holds. A full ledger refuses new
/// clusters rather than evicting an unanswered question.
pub const MAX_OPEN_PROMPTS: usize = 256;

/// Classes with failures collected but not yet a cluster.
pub const MAX_PENDING_CLASSES: usize = 1_024;

/// A question for the research side, raised by a cluster.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchPrompt {
    pub id: String,
    pub class: String,
    /// The hypothesis ids of the failed theses that formed the cluster.
    pub episodes: Vec<String>,
    pub raised_at: Timestamp,
    /// The research task that answered it, once closed.
    pub answered_by: Option<String>,
    pub closed_at: Option<Timestamp>,
}

impl ResearchPrompt {
    pub fn is_open(&self) -> bool {
        self.answered_by.is_none()
    }
}

/// Failure clusters in, prompts out, and each prompt tracked to its answer.
#[derive(Debug, Default)]
pub struct ResearchLedger {
    prompts: BTreeMap<String, ResearchPrompt>,
    pending: BTreeMap<String, Vec<String>>,
    seen: BTreeSet<String>,
    minted: u64,
}

impl ResearchLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold in graded theses and return the ids of prompts raised by them.
    ///
    /// Only [`Verdict::Wrong`] and [`Verdict::Falsified`] are failures:
    /// `RightForTheWrongReason` is a different lesson and `Inconclusive` is
    /// no evidence. A hypothesis id already folded in is skipped, so the
    /// whole calibration window may be offered again without one failure
    /// counting twice toward a cluster.
    pub fn observe(&mut self, evaluations: &[Evaluation], now: Timestamp) -> Result<Vec<String>> {
        let mut raised = Vec::new();
        for evaluation in evaluations {
            if !matches!(evaluation.verdict, Verdict::Wrong | Verdict::Falsified) {
                continue;
            }
            if self.seen.contains(&evaluation.hypothesis_id) {
                continue;
            }
            if !self.pending.contains_key(&evaluation.class)
                && self.pending.len() >= MAX_PENDING_CLASSES
            {
                return Err(Error::invalid(format!(
                    "failure in class {:?} not clustered: {MAX_PENDING_CLASSES} classes already \
                     hold unclustered failures; close or widen before adding another class",
                    evaluation.class
                )));
            }
            let open = self.prompts.values().filter(|p| p.is_open()).count();
            let would_mint = self
                .pending
                .get(&evaluation.class)
                .is_some_and(|ids| ids.len() + 1 >= MINIMUM_CLUSTER);
            if would_mint && open >= MAX_OPEN_PROMPTS {
                return Err(Error::invalid(format!(
                    "cluster in class {:?} not prompted: {MAX_OPEN_PROMPTS} prompts are still \
                     unanswered; close one with ResearchLedger::close first",
                    evaluation.class
                )));
            }
            self.seen.insert(evaluation.hypothesis_id.clone());
            let ids = self.pending.entry(evaluation.class.clone()).or_default();
            ids.push(evaluation.hypothesis_id.clone());
            if ids.len() >= MINIMUM_CLUSTER {
                let episodes = std::mem::take(ids);
                self.pending.remove(&evaluation.class);
                self.minted += 1;
                let id = format!("research-{:06}", self.minted);
                self.prompts.insert(
                    id.clone(),
                    ResearchPrompt {
                        id: id.clone(),
                        class: evaluation.class.clone(),
                        episodes,
                        raised_at: now,
                        answered_by: None,
                        closed_at: None,
                    },
                );
                raised.push(id);
            }
        }
        Ok(raised)
    }

    /// Record the research task that answered a prompt.
    ///
    /// Refuses an unknown prompt, an empty task id, and a prompt already
    /// closed: a second answer would overwrite the first task's trace.
    pub fn close(&mut self, prompt: &str, task: &str, now: Timestamp) -> Result<()> {
        if task.trim().is_empty() {
            return Err(Error::invalid(
                "a prompt is closed by naming the research task that answered it; none was given",
            ));
        }
        let entry = self.prompts.get_mut(prompt).ok_or_else(|| {
            Error::invalid(format!(
                "no research prompt {prompt:?}; list them with prompts()"
            ))
        })?;
        if let Some(existing) = &entry.answered_by {
            return Err(Error::denied(format!(
                "research prompt {prompt} is already answered by {existing}; open a new prompt \
                 rather than overwriting the trace"
            )));
        }
        entry.answered_by = Some(task.to_string());
        entry.closed_at = Some(now);
        Ok(())
    }

    /// Every prompt, by id.
    pub fn prompts(&self) -> impl Iterator<Item = &ResearchPrompt> {
        self.prompts.values()
    }

    pub fn prompt(&self, id: &str) -> Option<&ResearchPrompt> {
        self.prompts.get(id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // tests may unwrap: a panic is the failure report
mod tests {
    use super::*;
    use qip_core::time::Duration;

    fn failed(id: &str, class: &str, verdict: Verdict) -> Evaluation {
        Evaluation {
            hypothesis_id: id.into(),
            class: class.into(),
            subject: String::new(),
            verdict,
            expected_move_bps: 100.0,
            realised_move_bps: -80.0,
            magnitude_ratio: -0.8,
            confidence: 0.8,
            realised_pnl: -1.0,
            contributors: vec![],
            evaluated_at: Timestamp::from_secs(1),
            rationale: String::new(),
            horizon: Duration::from_secs(3600),
        }
    }

    #[test]
    fn three_failures_of_one_class_raise_one_prompt_naming_all_three_and_two_do_not() {
        let mut ledger = ResearchLedger::new();
        let now = Timestamp::from_secs(10);
        let two = [
            failed("a", "gap", Verdict::Wrong),
            failed("b", "gap", Verdict::Falsified),
        ];
        assert!(
            ledger.observe(&two, now).unwrap().is_empty(),
            "two is not a cluster"
        );
        let raised = ledger
            .observe(&[failed("c", "gap", Verdict::Wrong)], now)
            .unwrap();
        assert_eq!(raised.len(), 1);
        let prompt = ledger.prompt(&raised[0]).unwrap();
        assert_eq!(prompt.episodes, vec!["a", "b", "c"]);
        assert!(prompt.is_open());
    }

    #[test]
    fn a_failure_offered_twice_counts_once_and_other_verdicts_never_count() {
        let mut ledger = ResearchLedger::new();
        let now = Timestamp::from_secs(10);
        let batch = [
            failed("a", "x", Verdict::Wrong),
            failed("a", "x", Verdict::Wrong),
            failed("b", "x", Verdict::RightForTheWrongReason),
            failed("c", "x", Verdict::Inconclusive),
            failed("d", "x", Verdict::Vindicated),
        ];
        assert!(ledger.observe(&batch, now).unwrap().is_empty());
        assert!(ledger.observe(&batch, now).unwrap().is_empty());
        assert_eq!(ledger.prompts().count(), 0);
    }

    #[test]
    fn closing_records_the_task_once_and_refuses_a_second_answer() {
        let mut ledger = ResearchLedger::new();
        let now = Timestamp::from_secs(10);
        let batch: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|i| failed(i, "k", Verdict::Wrong))
            .collect();
        let id = ledger.observe(&batch, now).unwrap().remove(0);
        assert!(ledger.close(&id, " ", now).is_err());
        assert!(ledger.close("nope", "t", now).is_err());
        ledger.close(&id, "task-1", now).unwrap();
        assert_eq!(
            ledger.prompt(&id).unwrap().answered_by.as_deref(),
            Some("task-1")
        );
        assert!(ledger.close(&id, "task-2", now).is_err());
        assert_eq!(
            ledger.prompt(&id).unwrap().answered_by.as_deref(),
            Some("task-1")
        );
    }
}
