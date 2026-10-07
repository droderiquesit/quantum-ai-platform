//! Causal mechanisms for reasoning engine integration
//!
//! This module provides causal mechanism types that describe the economic
//! reasoning behind investment hypotheses. A mechanism explains *why* a
//! market phenomenon is expected to occur.

use serde::{Deserialize, Serialize};

/// A causal mechanism explaining an investment hypothesis
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mechanism {
    /// Credit conditions changing
    CreditConditions,
    /// Supply-demand imbalance
    SupplyDemand,
    /// Policy or regulatory change
    PolicyChange,
    /// Earnings revision
    EarningsRevision,
    /// Valuation mean reversion
    MeanReversion,
    /// Momentum or trend continuation
    Momentum,
    /// Flow-driven pricing
    FlowDriven,
    /// Sentiment shift
    SentimentShift,
    /// Macroeconomic cycle
    MacroCycle,
    /// Liquidity event
    Liquidity,
}

impl Mechanism {
    /// Human-readable name of the mechanism
    pub fn name(&self) -> &'static str {
        match self {
            Self::CreditConditions => "Credit Conditions",
            Self::SupplyDemand => "Supply/Demand",
            Self::PolicyChange => "Policy Change",
            Self::EarningsRevision => "Earnings Revision",
            Self::MeanReversion => "Mean Reversion",
            Self::Momentum => "Momentum",
            Self::FlowDriven => "Flow-Driven",
            Self::SentimentShift => "Sentiment Shift",
            Self::MacroCycle => "Macro Cycle",
            Self::Liquidity => "Liquidity",
        }
    }

    /// Describes what would falsify this mechanism
    pub fn describe(&self) -> &'static str {
        match self {
            Self::CreditConditions => "Credit conditions remain unchanged",
            Self::SupplyDemand => "Supply and demand remain balanced",
            Self::PolicyChange => "No policy or regulatory changes occur",
            Self::EarningsRevision => "Earnings forecasts remain constant",
            Self::MeanReversion => "Asset continues to diverge from historical mean",
            Self::Momentum => "Trend reverses or consolidates",
            Self::FlowDriven => "Investment flows reverse",
            Self::SentimentShift => "Market sentiment remains unchanged",
            Self::MacroCycle => "Macroeconomic cycle phase does not change",
            Self::Liquidity => "Liquidity conditions remain normal",
        }
    }

    /// Whether this mechanism preserves the sign of returns (directional)
    pub fn preserves_sign(&self) -> bool {
        matches!(
            self,
            Self::Momentum | Self::CreditConditions | Self::PolicyChange | Self::EarningsRevision
        )
    }
}

impl std::fmt::Display for Mechanism {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
