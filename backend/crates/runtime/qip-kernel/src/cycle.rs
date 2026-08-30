//! One turn of the intelligence loop.
//!
//! [`CycleReport`] is what a single pass through SENSE → UNDERSTAND → DISCOVER
//! → REASON → SIMULATE → DECIDE → ACT → LEARN produced, with the count and the
//! reason at every stage. A cycle that produced nothing should be as legible as
//! one that traded: "no opportunity cleared the bar" and "the risk function
//! vetoed the proposal" are different outcomes, and a report that showed both
//! as zero trades would hide the difference.

use qip_core::lineage::CorrelationId;
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// The stages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Sense,
    Understand,
    Discover,
    Reason,
    Simulate,
    Decide,
    Act,
    Learn,
}

impl Stage {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Sense => "sense",
            Self::Understand => "understand",
            Self::Discover => "discover",
            Self::Reason => "reason",
            Self::Simulate => "simulate",
            Self::Decide => "decide",
            Self::Act => "act",
            Self::Learn => "learn",
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Sense,
            Self::Understand,
            Self::Discover,
            Self::Reason,
            Self::Simulate,
            Self::Decide,
            Self::Act,
            Self::Learn,
        ]
    }
}

/// How one stage went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageOutcome {
    pub stage: Stage,
    /// Whether the stage ran at all.
    pub ran: bool,
    /// Items the stage produced: events absorbed, opportunities found, orders
    /// sent — whatever the stage's unit is.
    pub produced: usize,
    /// How long it took, on the injected clock.
    pub elapsed: Duration,
    /// What happened, in a sentence. Never empty, including on a quiet stage:
    /// "nothing happened" and "nothing was attempted" are different, and a
    /// report that could not distinguish them would be no use at three in the
    /// morning.
    pub detail: String,
    /// Anything that went wrong without stopping the cycle.
    pub problems: Vec<String>,
}

impl StageOutcome {
    pub fn ran(stage: Stage, produced: usize, detail: impl Into<String>) -> Self {
        Self {
            stage,
            ran: true,
            produced,
            elapsed: Duration::ZERO,
            detail: detail.into(),
            problems: Vec::new(),
        }
    }

    pub fn skipped(stage: Stage, reason: impl Into<String>) -> Self {
        Self {
            stage,
            ran: false,
            produced: 0,
            elapsed: Duration::ZERO,
            detail: reason.into(),
            problems: Vec::new(),
        }
    }

    pub fn with_problem(mut self, problem: impl Into<String>) -> Self {
        self.problems.push(problem.into());
        self
    }

    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }
}

/// What a cycle produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CycleReport {
    pub cycle: u64,
    /// The correlation id every event in this cycle carries, so the whole
    /// cycle can be reconstructed from the event log by one key.
    pub correlation_id: CorrelationId,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub stages: Vec<StageOutcome>,
    /// Events appended to the log during the cycle.
    pub events_logged: usize,
    /// Whether the platform was halted at any point in the cycle.
    pub halted: bool,
}

impl CycleReport {
    pub fn duration(&self) -> Duration {
        self.finished_at.since(self.started_at)
    }

    pub fn stage(&self, stage: Stage) -> Option<&StageOutcome> {
        self.stages.iter().find(|outcome| outcome.stage == stage)
    }

    /// Whether every stage ran.
    ///
    /// The milestone the platform is built around: an observation reaching all
    /// eight stages in one pass.
    pub fn traversed_every_stage(&self) -> bool {
        Stage::all()
            .into_iter()
            .all(|stage| self.stage(stage).is_some_and(|outcome| outcome.ran))
    }

    /// Stages that reported a problem.
    pub fn problems(&self) -> Vec<(Stage, &str)> {
        self.stages
            .iter()
            .flat_map(|outcome| {
                outcome
                    .problems
                    .iter()
                    .map(move |problem| (outcome.stage, problem.as_str()))
            })
            .collect()
    }

    /// A line per stage, for an operator reading a log at three in the morning.
    pub fn summarise(&self) -> String {
        let mut lines = vec![format!(
            "cycle {} [{}] {:?}",
            self.cycle,
            self.correlation_id.as_str(),
            self.duration()
        )];
        for outcome in &self.stages {
            lines.push(format!(
                "  {:<10} {:>4}  {}",
                outcome.stage.as_str(),
                if outcome.ran {
                    outcome.produced.to_string()
                } else {
                    "-".to_string()
                },
                outcome.detail
            ));
            for problem in &outcome.problems {
                lines.push(format!("             !  {problem}"));
            }
        }
        lines.join("\n")
    }
}
