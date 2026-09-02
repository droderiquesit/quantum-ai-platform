//! What an agent is allowed to do.
//!
//! Capabilities are the platform's separation of duties. A research agent may
//! read market data and propose a thesis; it may not submit an order, and no
//! configuration short of editing this file and its tests can give it one.
//!
//! The enforcement point is [`crate::runtime::AgentContext`], not the agent:
//! an agent asks the context for a facility and the context refuses if the
//! manifest did not grant it. An agent that forgot to check is therefore still
//! contained, which is the only containment worth having.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// One thing an agent may be permitted to do.
///
/// Ordered from the least to the most consequential, which is also the order
/// [`Capability::sensitivity`] uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    // --- observation: read-only, no market impact ---
    ReadMarketData,
    ReadReferenceData,
    ReadFundamentals,
    ReadNews,
    ReadAlternativeData,
    ReadWorldModel,
    ReadResearchMemory,
    ReadPortfolio,
    ReadRiskState,
    ReadExecutionHistory,

    // --- computation: expensive but side-effect free ---
    CallLanguageModel,
    RunBacktest,
    RunSimulation,
    RunOptimization,
    RunQuantumOptimization,

    // --- production: writes something other components consume ---
    WriteResearchMemory,
    PublishHypothesis,
    ChallengeHypothesis,
    ProposeTrade,

    // --- authority: changes what the platform will actually do ---
    ApproveProposal,
    VetoProposal,
    SubmitOrder,
    CancelOrder,
    OverrideRiskLimit,
    ChangeAutonomyLevel,
    TriggerKillSwitch,
}

impl Capability {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ReadMarketData => "read_market_data",
            Self::ReadReferenceData => "read_reference_data",
            Self::ReadFundamentals => "read_fundamentals",
            Self::ReadNews => "read_news",
            Self::ReadAlternativeData => "read_alternative_data",
            Self::ReadWorldModel => "read_world_model",
            Self::ReadResearchMemory => "read_research_memory",
            Self::ReadPortfolio => "read_portfolio",
            Self::ReadRiskState => "read_risk_state",
            Self::ReadExecutionHistory => "read_execution_history",
            Self::CallLanguageModel => "call_language_model",
            Self::RunBacktest => "run_backtest",
            Self::RunSimulation => "run_simulation",
            Self::RunOptimization => "run_optimization",
            Self::RunQuantumOptimization => "run_quantum_optimization",
            Self::WriteResearchMemory => "write_research_memory",
            Self::PublishHypothesis => "publish_hypothesis",
            Self::ChallengeHypothesis => "challenge_hypothesis",
            Self::ProposeTrade => "propose_trade",
            Self::ApproveProposal => "approve_proposal",
            Self::VetoProposal => "veto_proposal",
            Self::SubmitOrder => "submit_order",
            Self::CancelOrder => "cancel_order",
            Self::OverrideRiskLimit => "override_risk_limit",
            Self::ChangeAutonomyLevel => "change_autonomy_level",
            Self::TriggerKillSwitch => "trigger_kill_switch",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Self::all()
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| Error::invalid(format!("unknown capability: {s}")))
    }

    /// Every capability, in declaration order.
    pub fn all() -> Vec<Self> {
        vec![
            Self::ReadMarketData,
            Self::ReadReferenceData,
            Self::ReadFundamentals,
            Self::ReadNews,
            Self::ReadAlternativeData,
            Self::ReadWorldModel,
            Self::ReadResearchMemory,
            Self::ReadPortfolio,
            Self::ReadRiskState,
            Self::ReadExecutionHistory,
            Self::CallLanguageModel,
            Self::RunBacktest,
            Self::RunSimulation,
            Self::RunOptimization,
            Self::RunQuantumOptimization,
            Self::WriteResearchMemory,
            Self::PublishHypothesis,
            Self::ChallengeHypothesis,
            Self::ProposeTrade,
            Self::ApproveProposal,
            Self::VetoProposal,
            Self::SubmitOrder,
            Self::CancelOrder,
            Self::OverrideRiskLimit,
            Self::ChangeAutonomyLevel,
            Self::TriggerKillSwitch,
        ]
    }

    /// How much damage misuse could do, from 0 (read-only) to 3 (authority).
    pub const fn sensitivity(&self) -> u8 {
        match self {
            Self::ReadMarketData
            | Self::ReadReferenceData
            | Self::ReadFundamentals
            | Self::ReadNews
            | Self::ReadAlternativeData
            | Self::ReadWorldModel
            | Self::ReadResearchMemory
            | Self::ReadPortfolio
            | Self::ReadRiskState
            | Self::ReadExecutionHistory => 0,
            Self::CallLanguageModel
            | Self::RunBacktest
            | Self::RunSimulation
            | Self::RunOptimization
            | Self::RunQuantumOptimization => 1,
            Self::WriteResearchMemory
            | Self::PublishHypothesis
            | Self::ChallengeHypothesis
            | Self::ProposeTrade => 2,
            Self::ApproveProposal
            | Self::VetoProposal
            | Self::SubmitOrder
            | Self::CancelOrder
            | Self::OverrideRiskLimit
            | Self::ChangeAutonomyLevel
            | Self::TriggerKillSwitch => 3,
        }
    }

    /// Whether exercising this capability can move money or change exposure.
    ///
    /// These are the capabilities that require an authorised human or an
    /// explicitly configured autonomy level, never an agent's own judgement.
    pub const fn touches_market(&self) -> bool {
        matches!(
            self,
            Self::SubmitOrder | Self::CancelOrder | Self::OverrideRiskLimit
        )
    }

    /// Whether the capability writes anything at all.
    pub const fn is_read_only(&self) -> bool {
        self.sensitivity() == 0
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A set of capabilities, with the checks that make it a security boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet {
    granted: BTreeSet<Capability>,
}

impl CapabilitySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn of(capabilities: impl IntoIterator<Item = Capability>) -> Self {
        Self {
            granted: capabilities.into_iter().collect(),
        }
    }

    /// Every read-only capability: the standard grant for a research agent.
    pub fn read_only() -> Self {
        Self::of(
            Capability::all()
                .into_iter()
                .filter(Capability::is_read_only),
        )
    }

    pub fn with(mut self, capability: Capability) -> Self {
        self.granted.insert(capability);
        self
    }

    pub fn contains(&self, capability: Capability) -> bool {
        self.granted.contains(&capability)
    }

    pub fn len(&self) -> usize {
        self.granted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.granted.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.granted.iter().copied()
    }

    /// Refuse unless the capability is granted.
    ///
    /// The error names the agent and the capability, because the common
    /// operational question after a denial is "which manifest do I fix".
    pub fn require(&self, capability: Capability, agent: &str) -> Result<()> {
        if self.granted.contains(&capability) {
            return Ok(());
        }
        Err(Error::denied(format!(
            "agent {agent} is not granted {capability}"
        )))
    }

    /// Whether this set is contained by `other` — used to check that a
    /// delegated sub-task cannot exceed its delegator.
    pub fn is_subset_of(&self, other: &Self) -> bool {
        self.granted.is_subset(&other.granted)
    }

    /// The capabilities in `self` that `other` does not have.
    pub fn excess_over(&self, other: &Self) -> Vec<Capability> {
        self.granted.difference(&other.granted).copied().collect()
    }
}
