//! Reflex journal contract (SLICE-14): decision record format.
//!
//! Every reflex decision is journaled with its evidence, reasoning, and
//! outcomes. The journal is the record of what the cell decided, what it
//! knew when, and what the consequences were.

use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// The reason a decision was made (or declined)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    /// Model predicted an opportunity
    ModelSignal,

    /// Ambient signal triggered activation
    AmbientSignal,

    /// Arbitrage desk identified a cycle
    ArbitrageOpportunity,

    /// Risk or capital constraint was hit
    ConstraintBinding,

    /// Decision was explicitly declined by a control
    ExplicitlyDeclined,

    /// Unknown or other reason
    Other,
}

impl DecisionReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ModelSignal => "model_signal",
            Self::AmbientSignal => "ambient_signal",
            Self::ArbitrageOpportunity => "arbitrage_opportunity",
            Self::ConstraintBinding => "constraint_binding",
            Self::ExplicitlyDeclined => "explicitly_declined",
            Self::Other => "other",
        }
    }
}

/// Decision outcome: whether the action was taken and what happened
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    /// Action was taken as planned
    Executed,

    /// Action was refused by a risk control
    RefusedByControl,

    /// Action was declined by an external factor (e.g., no venue connectivity)
    EnvironmentFail,

    /// Action was rejected by the market (e.g., order was rejected)
    MarketReject,

    /// Unknown outcome
    Unknown,
}

impl DecisionOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Executed => "executed",
            Self::RefusedByControl => "refused_by_control",
            Self::EnvironmentFail => "environment_fail",
            Self::MarketReject => "market_reject",
            Self::Unknown => "unknown",
        }
    }
}

/// One decision the reflex cell made, with its reasoning
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflexDecision {
    /// Unique ID for this decision within the cell
    pub decision_id: String,

    /// Cell that made the decision
    pub cell_id: String,

    /// When the decision was made
    pub decided_at: Timestamp,

    /// Why the decision was made
    pub reason: DecisionReason,

    /// The action(s) being considered (e.g., order details)
    pub proposed_action: String,

    /// Model/strategy that suggested this action
    pub source_model: String,

    /// Confidence in the model's recommendation (0.0 - 1.0)
    pub model_confidence: f64,

    /// What actually happened
    pub outcome: DecisionOutcome,

    /// PnL impact if known
    pub pnl: Option<Decimal>,

    /// When the outcome was known
    pub outcome_at: Option<Timestamp>,
}

impl ReflexDecision {
    /// Whether this decision was beneficial (profitable or as intended)
    pub fn was_beneficial(&self) -> bool {
        matches!(self.outcome, DecisionOutcome::Executed)
            || (self.pnl.is_some_and(|p| p > Decimal::ZERO))
    }
}

/// Summary of a reflex cell's decision history over a period
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReflexJournalSummary {
    /// Cell identifier
    pub cell_id: String,

    /// Total number of decisions made
    pub decision_count: u64,

    /// Number executed successfully
    pub executed_count: u64,

    /// Number refused by risk controls
    pub refused_count: u64,

    /// Number profitable
    pub profitable_count: u64,

    /// Total PnL from executed decisions
    pub total_pnl: Decimal,

    /// Average model confidence across all decisions
    pub avg_model_confidence: f64,

    /// Period start time
    pub period_start: Timestamp,

    /// Period end time
    pub period_end: Timestamp,
}

impl ReflexJournalSummary {
    /// Create a new summary
    pub fn new(cell_id: String, period_start: Timestamp, period_end: Timestamp) -> Self {
        Self {
            cell_id,
            decision_count: 0,
            executed_count: 0,
            refused_count: 0,
            profitable_count: 0,
            total_pnl: Decimal::ZERO,
            avg_model_confidence: 0.0,
            period_start,
            period_end,
        }
    }

    /// Record a decision in the summary
    pub fn record_decision(&mut self, decision: &ReflexDecision) {
        self.decision_count += 1;

        match decision.outcome {
            DecisionOutcome::Executed => {
                self.executed_count += 1;
            }
            DecisionOutcome::RefusedByControl => {
                self.refused_count += 1;
            }
            _ => {}
        }

        if decision.was_beneficial() {
            self.profitable_count += 1;
        }

        // Update running average of confidence
        self.avg_model_confidence = (self.avg_model_confidence * (self.decision_count - 1) as f64
            + decision.model_confidence)
            / self.decision_count as f64;

        if let Some(pnl) = decision.pnl {
            self.total_pnl += pnl;
        }
    }

    /// Execution rate: fraction of decisions that were executed
    pub fn execution_rate(&self) -> f64 {
        if self.decision_count == 0 {
            0.0
        } else {
            self.executed_count as f64 / self.decision_count as f64
        }
    }

    /// Win rate: fraction of executed decisions that were profitable
    pub fn win_rate(&self) -> f64 {
        if self.executed_count == 0 {
            0.0
        } else {
            self.profitable_count as f64 / self.executed_count as f64
        }
    }

    /// Average PnL per executed decision
    pub fn avg_pnl_per_execution(&self) -> f64 {
        if self.executed_count == 0 {
            0.0
        } else {
            (self.total_pnl / Decimal::from(self.executed_count as i64)).to_f64()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_with_positive_pnl_is_beneficial() {
        let decision = ReflexDecision {
            decision_id: "d1".to_string(),
            cell_id: "c1".to_string(),
            decided_at: Timestamp::from_secs(0),
            reason: DecisionReason::ModelSignal,
            proposed_action: "buy".to_string(),
            source_model: "ml_v1".to_string(),
            model_confidence: 0.8,
            outcome: DecisionOutcome::Executed,
            pnl: Some(Decimal::from(100)),
            outcome_at: Some(Timestamp::from_secs(60)),
        };
        assert!(decision.was_beneficial());
    }

    #[test]
    fn a_summary_computes_correct_rates() {
        let mut summary = ReflexJournalSummary::new(
            "c1".to_string(),
            Timestamp::from_secs(0),
            Timestamp::from_secs(3600),
        );

        for i in 0..10 {
            let decision = ReflexDecision {
                decision_id: format!("d{i}"),
                cell_id: "c1".to_string(),
                decided_at: Timestamp::from_secs(i as i64 * 60),
                reason: DecisionReason::ModelSignal,
                proposed_action: "test".to_string(),
                source_model: "ml_v1".to_string(),
                model_confidence: 0.7,
                outcome: if i % 2 == 0 {
                    DecisionOutcome::Executed
                } else {
                    DecisionOutcome::RefusedByControl
                },
                pnl: if i % 2 == 0 {
                    Some(Decimal::from(100))
                } else {
                    None
                },
                outcome_at: Some(Timestamp::from_secs((i + 1) as i64 * 60)),
            };
            summary.record_decision(&decision);
        }

        assert_eq!(summary.decision_count, 10);
        assert_eq!(summary.executed_count, 5);
        assert_eq!(summary.refused_count, 5);
    }
}
