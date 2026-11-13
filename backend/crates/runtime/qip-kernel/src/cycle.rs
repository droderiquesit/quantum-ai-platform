//! One turn of the intelligence loop.
//!
//! [`CycleReport`] is what a single pass through SENSE → UNDERSTAND → DISCOVER
//! → REASON → SIMULATE → DECIDE → ACT → LEARN produced, with the count and the
//! reason at every stage. A cycle that produced nothing should be as legible as
//! one that traded: "no opportunity cleared the bar" and "the risk function
//! vetoed the proposal" are different outcomes, and a report that showed both
//! as zero trades would hide the difference.

use qip_capital::exposure::CrowdedPosition;
use qip_core::Decimal;
use qip_core::lineage::CorrelationId;
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// What the Risk Brain found this cycle that no single cell and no single
    /// instrument shows: names several cells hold at once, and the factors
    /// and causal drivers the book leans on (R12-RISK-4094).
    ///
    /// Read off the same aggregate and the same risk state the gate reads,
    /// after the stages ran, and never written back into either: it is
    /// advice about the book, and a field the pre-trade veto consulted would
    /// be a second gate nobody had reviewed as one.
    pub risk_discovery: RiskDiscovery,
}

/// Hidden concentration and emerging risk, as one cycle found them.
///
/// Before this existed the two surfaces were real and unreported: crowding
/// came back only from [`crate::platform::Platform::ingest_cell_report`], per
/// report, and the factor and causal-driver levels lived only inside the risk
/// state, so an operator reading a cycle could not see that three cells had
/// piled into one name or that five "diversified" holdings followed one
/// driver.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RiskDiscovery {
    /// Instruments at least the central plane's crowding threshold of cells
    /// hold, as `AggregateExposure::crowded` returns them.
    pub crowded: Vec<CrowdedPosition>,
    /// Gross charged to each systematic factor. `None` when the level was not
    /// measured this cycle — not the same fact as measured and empty.
    pub factor: Option<BTreeMap<String, Decimal>>,
    /// Gross charged to each causal driver the world model's graph names.
    /// `None` when not measured, for the same reason.
    pub causal_drivers: Option<BTreeMap<String, Decimal>>,
    /// What is new or larger than the cycle before: the emerging half.
    pub emerging: Vec<EmergingRisk>,
}

/// One concentration that appeared or grew since the previous cycle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmergingRisk {
    /// `crowded`, `factor` or `causal_driver`.
    pub axis: String,
    pub bucket: String,
    /// The previous cycle's gross, `None` when the bucket did not exist then.
    pub previous: Option<Decimal>,
    pub current: Decimal,
}

impl RiskDiscovery {
    /// Compare this cycle's findings against the last cycle's and record what
    /// emerged. A bucket that shrank or held is not emerging; a bucket that
    /// vanished is not reported here, because absence is already visible in
    /// the maps themselves.
    pub fn with_emerging_since(mut self, previous: Option<&RiskDiscovery>) -> Self {
        let mut emerging = Vec::new();
        let prior_crowded: BTreeMap<&str, Decimal> = previous
            .map(|p| {
                p.crowded
                    .iter()
                    .map(|c| (c.instrument.as_str(), c.gross))
                    .collect()
            })
            .unwrap_or_default();
        for crowd in &self.crowded {
            let before = prior_crowded.get(crowd.instrument.as_str()).copied();
            if before.is_none_or(|b| crowd.gross > b) {
                emerging.push(EmergingRisk {
                    axis: "crowded".to_string(),
                    bucket: crowd.instrument.clone(),
                    previous: before,
                    current: crowd.gross,
                });
            }
        }
        let levels = [
            (
                qip_risk::shared_cause::FACTOR_AXIS,
                self.factor.as_ref(),
                previous.and_then(|p| p.factor.as_ref()),
            ),
            (
                qip_risk::shared_cause::CAUSAL_DRIVER_AXIS,
                self.causal_drivers.as_ref(),
                previous.and_then(|p| p.causal_drivers.as_ref()),
            ),
        ];
        for (axis, now, before) in levels {
            for (bucket, current) in now.into_iter().flatten() {
                let prior = before.and_then(|b| b.get(bucket)).copied();
                if current.is_positive() && prior.is_none_or(|p| *current > p) {
                    emerging.push(EmergingRisk {
                        axis: axis.to_string(),
                        bucket: bucket.clone(),
                        previous: prior,
                        current: *current,
                    });
                }
            }
        }
        self.emerging = emerging;
        self
    }
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
