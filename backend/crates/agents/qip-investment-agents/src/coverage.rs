//! What the organisation's findings cover, by scope.
//!
//! A specialist brain is declared in the blueprint by the scopes it covers —
//! queue dynamics, source poisoning, risk forecasting — and the integration
//! test every one of those rows names is "removing it drops its scope from
//! coverage". [`OrganisationReport::coverage`](crate::OrganisationReport::coverage)
//! cannot answer that: it is a ratio of findings to runs, and an organisation
//! with one agent removed has one run fewer and the same ratio.
//!
//! So a scope is covered here only by a number. Each [`Scope`] names the
//! exact fact labels that evidence it, and a report covers the scope when an
//! informative finding carries one of those facts. A manifest's competency
//! list is a promise and is deliberately not consulted: an agent that claims
//! queue dynamics in its manifest and reports no queue figure has not covered
//! queue dynamics, and an agent removed from the organisation reports nothing
//! at all.
//!
//! Labels are matched whole, never by prefix or substring. `drawdown` is a
//! substring of `max_drawdown`, and a scope that matched it would be covered
//! by an agent that never computed it.

use qip_agents::finding::AgentFinding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One scope a specialist brain is declared to cover.
///
/// Only scopes some agent actually evidences are listed. A variant whose
/// labels no agent emits would be a coverage claim about nothing, and the
/// organisation tests assert every variant is covered by the standard
/// organisation on a populated desk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// How fast the resting queue at the touch is worked through
    /// (R12-REASON-3014).
    QueueDynamics,
    /// Coordinated campaigns, contradicted and spoofed sources in the
    /// evidence fabric (R12-REASON-3023).
    SourcePoisoning,
    /// The book's forecast tail: value at risk and expected shortfall
    /// (R12-RISK-3021).
    RiskForecast,
    /// A generated stress scenario's loss (R12-RISK-3021).
    StressScenario,
    /// How much risk to shed (R12-RISK-3021).
    RiskRecommendation,
    /// The held book: its exposure, the positions held with no mandate and
    /// the ones being divested (R12-ASSET-3019).
    PortfolioAsset,
}

impl Scope {
    /// Every scope, in declaration order.
    pub const ALL: [Scope; 6] = [
        Scope::QueueDynamics,
        Scope::SourcePoisoning,
        Scope::RiskForecast,
        Scope::StressScenario,
        Scope::RiskRecommendation,
        Scope::PortfolioAsset,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueueDynamics => "queue_dynamics",
            Self::SourcePoisoning => "source_poisoning",
            Self::RiskForecast => "risk_forecast",
            Self::StressScenario => "stress_scenario",
            Self::RiskRecommendation => "risk_recommendation",
            Self::PortfolioAsset => "portfolio_asset",
        }
    }

    /// The fact labels whose presence in an informative finding evidences
    /// this scope.
    pub fn evidenced_by(self) -> &'static [&'static str] {
        match self {
            Self::QueueDynamics => &[
                "bid_queue_depletion_secs",
                "ask_queue_depletion_secs",
                "traded_rate_per_sec",
            ],
            Self::SourcePoisoning => &["poisoning_findings"],
            Self::RiskForecast => &["forecast_value_at_risk", "forecast_expected_shortfall"],
            Self::StressScenario => &["stress_loss_3sd_day"],
            Self::RiskRecommendation => &["recommended_gross_reduction"],
            Self::PortfolioAsset => &[
                "book_gross_exposure_ratio",
                "positions_flagged_without_mandate",
                "positions_unwinding",
            ],
        }
    }

    /// Whether `finding` carries a fact evidencing this scope.
    pub fn is_evidenced_in(self, finding: &AgentFinding) -> bool {
        let labels = self.evidenced_by();
        finding
            .facts
            .iter()
            .any(|fact| labels.contains(&fact.label.as_str()))
    }
}

/// The scopes `findings` cover, ignoring any finding that carries no
/// information.
pub fn covered_by<'a>(findings: impl IntoIterator<Item = &'a AgentFinding>) -> BTreeSet<Scope> {
    let findings: Vec<&AgentFinding> = findings
        .into_iter()
        .filter(|finding| finding.status.is_informative())
        .collect();
    Scope::ALL
        .into_iter()
        .filter(|scope| findings.iter().any(|f| scope.is_evidenced_in(f)))
        .collect()
}
