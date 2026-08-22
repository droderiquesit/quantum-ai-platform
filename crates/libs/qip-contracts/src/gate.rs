//! The evidence and approval gates a strategy passes through before it holds
//! capital, and the ones it can be pushed back through.
//!
//! The lifecycle is a ratchet in one direction only by permission and in the
//! other by anyone. Promoting takes evidence and an approver; demoting takes
//! neither, because the cost of an unnecessary demotion is a day of missed
//! opportunity and the cost of a missed one is the book.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

/// Where a strategy stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GateStage {
    /// Being researched. No capital, no orders, not even simulated.
    Candidate,
    /// Evaluated against data held out of its own fitting.
    Holdout,
    /// Simulated against live data with a simulated venue.
    Paper,
    /// Running against live data alongside production, orders computed and
    /// discarded. The last stage where being wrong is free.
    Shadow,
    /// Live with capital, deliberately limited.
    Pilot,
    /// Live at its approved size.
    Scaled,
    /// Withdrawn. Terminal — a retired strategy is re-proposed as a new
    /// candidate rather than resurrected, so its evidence is re-earned.
    Retired,
}

impl GateStage {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Holdout => "holdout",
            Self::Paper => "paper",
            Self::Shadow => "shadow",
            Self::Pilot => "pilot",
            Self::Scaled => "scaled",
            Self::Retired => "retired",
        }
    }

    pub const fn all() -> [Self; 7] {
        [
            Self::Candidate,
            Self::Holdout,
            Self::Paper,
            Self::Shadow,
            Self::Pilot,
            Self::Scaled,
            Self::Retired,
        ]
    }

    /// Whether a strategy at this stage may hold real capital.
    pub const fn holds_capital(&self) -> bool {
        matches!(self, Self::Pilot | Self::Scaled)
    }

    /// Whether a strategy at this stage may emit orders that reach a venue.
    pub const fn may_reach_a_venue(&self) -> bool {
        self.holds_capital()
    }

    /// The only stage a strategy here may be promoted to.
    ///
    /// One step at a time, and no path that skips shadow. A strategy that has
    /// never run against live data has not been tested, however good its
    /// backtest.
    pub const fn next(&self) -> Option<Self> {
        match self {
            Self::Candidate => Some(Self::Holdout),
            Self::Holdout => Some(Self::Paper),
            Self::Paper => Some(Self::Shadow),
            Self::Shadow => Some(Self::Pilot),
            Self::Pilot => Some(Self::Scaled),
            Self::Scaled | Self::Retired => None,
        }
    }

    /// Whether promotion to this stage needs a named human approver.
    ///
    /// Everything that can lose money does.
    pub const fn requires_human_approval(&self) -> bool {
        matches!(self, Self::Pilot | Self::Scaled)
    }
}

/// What a gate decided and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateOutcome {
    pub stage: GateStage,
    pub passed: bool,
    /// The checks that ran, and what each concluded.
    pub findings: Vec<(String, bool, String)>,
    pub at: Timestamp,
}

impl GateOutcome {
    pub fn new(stage: GateStage, at: Timestamp) -> Self {
        Self {
            stage,
            passed: true,
            findings: Vec::new(),
            at,
        }
    }

    /// Record a check. One failure fails the gate; there is no scoring.
    pub fn record(
        mut self,
        name: impl Into<String>,
        passed: bool,
        detail: impl Into<String>,
    ) -> Self {
        self.passed &= passed;
        self.findings.push((name.into(), passed, detail.into()));
        self
    }

    pub fn failures(&self) -> Vec<&(String, bool, String)> {
        self.findings.iter().filter(|(_, ok, _)| !ok).collect()
    }
}

/// A recorded move between stages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Promotion {
    pub from: GateStage,
    pub to: GateStage,
    pub at: Timestamp,
    /// Who approved it. `None` only for demotions, which need no authority.
    pub approver: Option<String>,
    pub rationale: String,
    pub evidence: Vec<String>,
}

impl Promotion {
    /// Whether this move increases what the strategy is allowed to do.
    pub fn is_escalation(&self) -> bool {
        self.to > self.from && self.to != GateStage::Retired
    }
}
