//! Service level objectives.
//!
//! Each critical service declares its objective here rather than in a
//! dashboard, so the target lives beside the code that has to meet it and can
//! be asserted on in tests.

use serde::{Deserialize, Serialize};

/// Evaluation window for an objective.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SloWindow {
    Hour,
    Day,
    Week,
    Month,
}

impl SloWindow {
    pub fn hours(&self) -> f64 {
        match self {
            Self::Hour => 1.0,
            Self::Day => 24.0,
            Self::Week => 168.0,
            Self::Month => 720.0,
        }
    }
}

/// One service level objective.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slo {
    pub name: String,
    pub service: String,
    pub description: String,
    /// Target as a fraction, e.g. 0.999.
    pub target: f64,
    pub window: SloWindow,
    /// Latency threshold in milliseconds, for latency objectives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_threshold_ms: Option<f64>,
}

impl Slo {
    /// Availability objective: fraction of successful operations.
    pub fn availability(
        name: impl Into<String>,
        service: impl Into<String>,
        target: f64,
        window: SloWindow,
    ) -> Self {
        Self {
            name: name.into(),
            service: service.into(),
            description: "fraction of operations completing without error".into(),
            target,
            window,
            latency_threshold_ms: None,
        }
    }

    /// Latency objective: fraction of operations under a threshold.
    pub fn latency(
        name: impl Into<String>,
        service: impl Into<String>,
        target: f64,
        threshold_ms: f64,
        window: SloWindow,
    ) -> Self {
        Self {
            name: name.into(),
            service: service.into(),
            description: format!("fraction of operations completing within {threshold_ms}ms"),
            target,
            window,
            latency_threshold_ms: Some(threshold_ms),
        }
    }

    /// Total error budget for the window, as a fraction.
    pub fn error_budget(&self) -> f64 {
        (1.0 - self.target).max(0.0)
    }

    /// Evaluate against observed counts.
    pub fn evaluate(&self, good: u64, total: u64) -> SloStatus {
        if total == 0 {
            return SloStatus {
                slo: self.clone(),
                achieved: 1.0,
                budget_consumed: 0.0,
                is_met: true,
                observations: 0,
            };
        }
        let achieved = good as f64 / total as f64;
        let budget = self.error_budget();
        let consumed = if budget <= 0.0 {
            if achieved >= 1.0 { 0.0 } else { 1.0 }
        } else {
            ((1.0 - achieved) / budget).clamp(0.0, f64::INFINITY)
        };
        SloStatus {
            slo: self.clone(),
            achieved,
            budget_consumed: consumed,
            is_met: achieved >= self.target,
            observations: total,
        }
    }
}

/// Result of evaluating an objective.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SloStatus {
    pub slo: Slo,
    /// Fraction actually achieved.
    pub achieved: f64,
    /// Fraction of the error budget used; above 1.0 means the objective is missed.
    pub budget_consumed: f64,
    pub is_met: bool,
    pub observations: u64,
}

impl SloStatus {
    /// Whether the burn rate warrants paging rather than a ticket.
    ///
    /// Consuming most of a window's budget means the remaining margin is gone,
    /// which is the condition that should wake somebody up.
    pub fn is_page_worthy(&self) -> bool {
        self.budget_consumed >= 0.9 && self.observations >= 20
    }
}

/// The objectives the platform ships with.
///
/// Fast Brain paths get latency objectives measured in milliseconds; Deep Brain
/// paths get availability objectives, because a research run taking longer is
/// not an incident but silently failing is.
pub fn default_slos() -> Vec<Slo> {
    vec![
        Slo::latency(
            "market-ingestion-latency",
            "market-ingestion",
            0.999,
            50.0,
            SloWindow::Day,
        ),
        Slo::latency(
            "risk-precheck-latency",
            "risk-engine",
            0.9995,
            10.0,
            SloWindow::Day,
        ),
        Slo::latency(
            "execution-submit-latency",
            "execution-engine",
            0.999,
            100.0,
            SloWindow::Day,
        ),
        Slo::availability(
            "event-log-durability",
            "event-log",
            0.99999,
            SloWindow::Month,
        ),
        Slo::availability("world-model-queries", "world-model", 0.999, SloWindow::Day),
        Slo::availability(
            "reasoning-completion",
            "reasoning-engine",
            0.99,
            SloWindow::Week,
        ),
        Slo::availability(
            "optimization-completion",
            "optimization-engine",
            0.995,
            SloWindow::Week,
        ),
        Slo::availability("api-availability", "api", 0.999, SloWindow::Day),
    ]
}
