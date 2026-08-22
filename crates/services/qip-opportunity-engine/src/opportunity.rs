//! The opportunity record.

use qip_core::{Duration, ObjectId, OpportunityId, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

use crate::detector::Anomaly;

/// What kind of work an opportunity needs before it can become a thesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Investigation {
    /// Confirm the observation is real and not a data error.
    DataVerification,
    /// Establish what happened and why.
    CausalAnalysis,
    /// Value the affected instruments.
    Valuation,
    /// Assess the credit or balance-sheet implications.
    CreditAnalysis,
    /// Work out the macro transmission.
    MacroAnalysis,
    /// Examine order books, flow and liquidity.
    MicrostructureAnalysis,
    /// Model the derivative and volatility implications.
    DerivativesAnalysis,
    /// Trace supply-chain and second-order effects.
    SupplyChainAnalysis,
    /// Backtest whether the pattern has paid historically.
    HistoricalValidation,
}

impl Investigation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DataVerification => "data_verification",
            Self::CausalAnalysis => "causal_analysis",
            Self::Valuation => "valuation",
            Self::CreditAnalysis => "credit_analysis",
            Self::MacroAnalysis => "macro_analysis",
            Self::MicrostructureAnalysis => "microstructure_analysis",
            Self::DerivativesAnalysis => "derivatives_analysis",
            Self::SupplyChainAnalysis => "supply_chain_analysis",
            Self::HistoricalValidation => "historical_validation",
        }
    }

    /// Rough cost of the investigation, in agent-minutes.
    ///
    /// Carried so the ranking can weigh what an opportunity is worth against
    /// what it costs to find out.
    pub fn estimated_cost(&self) -> f64 {
        match self {
            Self::DataVerification => 2.0,
            Self::MicrostructureAnalysis => 5.0,
            Self::CausalAnalysis => 15.0,
            Self::Valuation | Self::CreditAnalysis => 25.0,
            Self::MacroAnalysis | Self::DerivativesAnalysis => 20.0,
            Self::SupplyChainAnalysis => 30.0,
            Self::HistoricalValidation => 40.0,
        }
    }
}

/// Where an opportunity sits in the queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpportunityRank {
    /// The score the queue is ordered by.
    pub score: f64,
    /// How much the observation matters if real, in `[0, 1]`.
    pub importance: f64,
    /// How much this differs from what has recently been seen, in `[0, 1]`.
    pub novelty: f64,
    /// Confidence the observation is real rather than noise or bad data.
    pub confidence: f64,
    /// Total estimated investigation cost, in agent-minutes.
    pub cost: f64,
}

impl OpportunityRank {
    /// Expected value per unit of investigation cost.
    ///
    /// What the queue is actually ordered by: an important finding that takes
    /// an hour to confirm can be worth less than three quick ones.
    pub fn value_per_cost(&self) -> f64 {
        if self.cost <= 1e-9 {
            return self.score;
        }
        self.score / self.cost.sqrt()
    }
}

/// Something worth investigating.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opportunity {
    pub opportunity_id: OpportunityId,
    pub detected_at: Timestamp,
    /// A one-line statement of what was noticed.
    pub headline: String,
    /// Instruments the observation concerns.
    pub affected_objects: Vec<ObjectId>,
    /// Entities the observation concerns.
    pub affected_entities: Vec<String>,
    /// The anomalies that produced it.
    pub anomalies: Vec<Anomaly>,
    /// Supporting record ids: documents, events, features.
    pub evidence: Vec<String>,
    /// How often something like this has been seen before, and what followed.
    pub historical_context: String,
    pub rank: OpportunityRank,
    /// Horizon over which the implication would play out.
    pub horizon: Duration,
    /// What must be done before this can become a thesis.
    pub required_investigation: Vec<Investigation>,
    /// Detectors that contributed.
    pub detectors: Vec<String>,
    /// When the opportunity stops being worth acting on.
    pub expires_at: Timestamp,
}

impl Opportunity {
    /// Whether the opportunity is still live at `now`.
    pub fn is_live(&self, now: Timestamp) -> bool {
        now < self.expires_at
    }

    /// Total estimated investigation cost.
    pub fn investigation_cost(&self) -> f64 {
        self.required_investigation
            .iter()
            .map(Investigation::estimated_cost)
            .sum()
    }

    /// Whether the opportunity clears the bar for an agent's time.
    pub fn is_worth_investigating(&self, threshold: f64) -> bool {
        self.rank.score >= threshold && self.rank.confidence >= 0.4
    }

    /// A description an agent can act on.
    pub fn brief(&self) -> String {
        let anomalies: Vec<String> = self
            .anomalies
            .iter()
            .map(|a| format!("{} ({:+.1} sigma)", a.kind.as_str(), a.z_score))
            .collect();
        format!(
            "{}\nDetected {} across {} instrument(s). Signals: {}. {}. \
             Investigation required: {}.",
            self.headline,
            self.detected_at.to_rfc3339(),
            self.affected_objects.len(),
            anomalies.join(", "),
            self.historical_context,
            self.required_investigation
                .iter()
                .map(|i| i.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl EventBody for Opportunity {
    const TOPIC: Topic = Topic::OpportunityDetected;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(self.opportunity_id.as_str().to_string())
    }
}
