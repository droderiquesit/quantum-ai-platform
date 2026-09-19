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
    /// Floor a measured magnitude must reach, for objectives that bound a
    /// ratio rather than a latency or a success rate.
    ///
    /// Carried as a number rather than left in the description because a
    /// figure that lives only in prose is a figure nothing can check. §49.1's
    /// "netting ratio above 1.5" is the case: the 1.5 is the objective, and a
    /// later edit loosening it inside a sentence would move a target with
    /// nothing to notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio_floor: Option<f64>,
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
            ratio_floor: None,
        }
    }

    /// Objective bounding a measured magnitude from below: the fraction of
    /// observations whose ratio reached `floor`.
    ///
    /// §49.1 states two targets in this shape — "netting ratio above 1.5" and
    /// "effective breadth above the allocator's floor" — and neither is a
    /// latency or a success rate. Forcing them into one of those would have
    /// put the only copy of the number in a description string.
    pub fn ratio_at_least(
        name: impl Into<String>,
        service: impl Into<String>,
        floor: f64,
        target: f64,
        window: SloWindow,
    ) -> Self {
        Self {
            name: name.into(),
            service: service.into(),
            description: format!("fraction of observations whose ratio reached {floor}"),
            target,
            window,
            latency_threshold_ms: None,
            ratio_floor: Some(floor),
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
            ratio_floor: None,
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

    /// Whether anything was actually measured.
    ///
    /// [`Slo::evaluate`] reports `is_met` for a window with no observations,
    /// and that is deliberate — a quiet hour is not an outage and must not
    /// page. But it means `is_met` alone cannot tell an objective that was
    /// achieved from one nothing ever measured, and a caller that reports the
    /// second as the first is publishing a control that cannot fire. That
    /// failure has already shipped here once, in a different place:
    /// `MaxExpectedShortfall` sat in every default limit set reading as
    /// protection while the state it consulted was always empty.
    ///
    /// Three of §49.1's targets are ratios over wall-clock time on a deployed
    /// node and cannot be measured in this process at all, so they will read
    /// as unobserved until something runs. Anything summarising objectives
    /// must say so rather than counting them as met.
    pub fn is_observed(&self) -> bool {
        self.observations > 0
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

/// The names of §49.1 objectives that **cannot** be measured without a
/// deployed, running system, and the reason each cannot.
///
/// Every one of these is a ratio over wall-clock time on a node that has to
/// exist: availability during market hours, the share of cycles that
/// completed, the share of the time a mirror sat inside its band. No amount of
/// in-process fixture work produces one, because the denominator is elapsed
/// time rather than operations, and there is nothing deployed
/// (`execution_nodes = {}` in every environment).
///
/// This list exists so the gap is a value rather than a sentence. A summary
/// that folded these into the rest would report fifteen objectives of which
/// three were `is_met` on zero observations, which is exactly the shape
/// [`SloStatus::is_observed`] documents.
pub const BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT: &[&str] = &[
    "node-availability-market-hours",
    "cycle-completion-path-1",
    "cycle-completion-path-2",
    "mirror-drift-inside-soft-band",
];

/// §49.1's targets, as values.
///
/// Until 2026-09-19 §49.1 existed here only as prose in the blueprint. The
/// module shipped eight objectives, `default_slos` had no caller at all, and
/// **not one of the eight was a §49.1 target** — so the section that states
/// what this platform must achieve was measured by nothing and contradicted by
/// nothing either. A set of objectives nobody can evaluate reads as
/// measurement and is not.
///
/// Fourteen rows become **fifteen** objectives: "Cycle completion — Path 1 /
/// Path 2" states two different targets, 97 and 93 percent, and collapsing
/// them into one would have lost whichever was stricter. The count is asserted
/// in `qip-acceptance/tests/slo.rs` so the discrepancy stays visible rather
/// than looking like a miscount.
///
/// Each target is the blueprint's own figure and nothing here rounds one.
/// Where §49.1 states a bound without a number — "within tolerance across all
/// belief classes", "inside every venue's requirement with headroom", "above
/// the allocator's floor" — the objective is the fraction of observations
/// satisfying whatever bound the owning component supplies, so the target is
/// 1.0 and the bound stays where it is computed. Inventing a number for one of
/// those would have created a second source of truth for a figure another
/// component already owns.
pub fn blueprint_slos() -> Vec<Slo> {
    vec![
        // "Node availability during market hours — 99.9 percent".
        Slo::availability(
            "node-availability-market-hours",
            "edge-node",
            0.999,
            SloWindow::Month,
        ),
        // "Strategy evaluation, p99 — under 90 µs". p99 is the 0.99 target;
        // 90 µs is 0.09 ms, and the unit conversion is here rather than at a
        // call site so there is one place to get it wrong.
        Slo::latency(
            "strategy-evaluation-p99",
            "strategy",
            0.99,
            0.09,
            SloWindow::Hour,
        ),
        // "Wire to first order, p99 internal — under 1.3 ms".
        Slo::latency(
            "wire-to-first-order-p99",
            "edge-node",
            0.99,
            1.3,
            SloWindow::Hour,
        ),
        // "Belief calibration error — within tolerance across all belief
        // classes". Every class, so the target admits no exceptions.
        Slo::availability(
            "belief-calibration-within-tolerance",
            "kernel",
            1.0,
            SloWindow::Week,
        ),
        // "Netting ratio — above 1.5".
        Slo::ratio_at_least("netting-ratio", "edge", 1.5, 1.0, SloWindow::Day),
        // "Cycle completion — Path 1 / Path 2 — above 97 / 93 percent". Two
        // objectives, deliberately.
        Slo::availability("cycle-completion-path-1", "edge", 0.97, SloWindow::Day),
        Slo::availability("cycle-completion-path-2", "edge", 0.93, SloWindow::Day),
        // "Arrival dispersion after equalisation — under 1.5 ms p99".
        Slo::latency("arrival-dispersion-p99", "edge", 0.99, 1.5, SloWindow::Hour),
        // "Quote message-to-trade ratio — inside every venue's requirement
        // with headroom". The requirement is the venue's and lives with the
        // quoting lane; this asks that no observation sat outside it.
        Slo::availability(
            "quote-message-to-trade-within-venue-requirement",
            "edge",
            1.0,
            SloWindow::Day,
        ),
        // "Mirror drift inside soft band — 95 percent of the time".
        Slo::availability(
            "mirror-drift-inside-soft-band",
            "edge",
            0.95,
            SloWindow::Day,
        ),
        // "Live-versus-holdout consistency, funded strategies — within band
        // for 80 percent".
        Slo::availability(
            "live-versus-holdout-consistency",
            "kernel",
            0.80,
            SloWindow::Week,
        ),
        // "Mark staleness — zero positions past limit supporting leverage".
        // Zero is a target of 1.0 and an error budget of nothing: one such
        // position misses the objective outright.
        Slo::availability(
            "mark-staleness-zero-past-limit",
            "portfolio",
            1.0,
            SloWindow::Day,
        ),
        // "Effective breadth — above the allocator's floor". The floor is the
        // allocator's own, so this counts allocations that reached it.
        Slo::availability(
            "effective-breadth-above-floor",
            "kernel",
            1.0,
            SloWindow::Day,
        ),
        // "Reconciliation breaks — zero unexplained. Any is severity one".
        Slo::availability(
            "reconciliation-breaks-zero-unexplained",
            "kernel",
            1.0,
            SloWindow::Day,
        ),
        // "Unauthorised transfer attempts reaching execution — zero. Any is a
        // security incident".
        Slo::availability(
            "unauthorised-transfers-reaching-execution-zero",
            "execution-engine",
            1.0,
            SloWindow::Day,
        ),
    ]
}
