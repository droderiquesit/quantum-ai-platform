//! Agent governance manifests.
//!
//! Every agent in the organisation is declared here before it can run: what it
//! is for, what it may touch, what it may spend, who owns it, and when its
//! authorisation expires. An agent without a valid manifest does not run —
//! there is no implicit default, because an implicit default is how an
//! unreviewed agent acquires order-submission rights.
//!
//! The rules in [`AgentManifest::validate`] are the platform's separation of
//! duties written down. They are deliberately stated as prohibitions on
//! combinations rather than as a whitelist of roles, so a new role cannot
//! accidentally inherit an unsafe pairing.

use crate::budget::Budget;
use crate::capability::{Capability, CapabilitySet};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The function an agent performs in the organisation.
///
/// Role is not decoration: [`AgentManifest::validate`] uses it to decide which
/// capability combinations are permissible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Reads, analyses and publishes findings. Never touches the market.
    Research,
    /// Attacks the findings of other agents. Deliberately adversarial.
    Adversarial,
    /// Turns approved theses into a target portfolio.
    Construction,
    /// Independent control function with veto authority.
    Control,
    /// Interacts with venues and brokers.
    Execution,
    /// Measures what happened and feeds it back.
    Learning,
    /// Coordinates the others. Decides nothing on its own.
    Coordination,
}

impl AgentRole {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Adversarial => "adversarial",
            Self::Construction => "construction",
            Self::Control => "control",
            Self::Execution => "execution",
            Self::Learning => "learning",
            Self::Coordination => "coordination",
        }
    }

    /// Roles that may never hold a market-touching capability.
    pub const fn is_research_side(&self) -> bool {
        matches!(
            self,
            Self::Research | Self::Adversarial | Self::Learning | Self::Coordination
        )
    }
}

impl fmt::Display for AgentRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an agent does when it cannot finish within its budget or authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationPolicy {
    /// Return what it has, marked incomplete. The default: a partial answer
    /// that says it is partial is safe; a truncated answer that claims
    /// completeness is not.
    ReturnPartial,
    /// Hand the work to the named agent.
    Delegate,
    /// Stop and require a human.
    RequireHuman,
}

/// The declaration that authorises an agent to run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentManifest {
    /// Stable machine identifier, e.g. `macro-analyst`.
    pub id: String,
    /// Human name used in the UI and in decision records.
    pub name: String,
    pub role: AgentRole,
    /// What the agent is for, in one sentence. Recorded in every decision that
    /// the agent contributed to.
    pub purpose: String,
    /// The questions this agent is competent to answer. Anything outside this
    /// list is a reason to defer, and the reasoning engine records the deferral
    /// rather than accepting an out-of-scope opinion.
    pub competencies: Vec<String>,
    /// What the agent explicitly cannot do, recorded so the limitation appears
    /// in the audit trail rather than living in someone's head.
    pub limitations: Vec<String>,
    pub capabilities: CapabilitySet,
    pub budget: Budget,
    pub escalation: EscalationPolicy,
    /// The agent this one escalates to, required by `Delegate`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub escalates_to: Option<String>,
    /// The team accountable for the agent's behaviour.
    pub owner: String,
    /// When the authorisation was last reviewed.
    pub reviewed_at: Timestamp,
    /// How long a review is valid for. An expired manifest does not run.
    pub review_interval: Duration,
    /// Model references this agent depends on, checked against the model
    /// registry before a decision is accepted.
    pub models: Vec<String>,
}

impl AgentManifest {
    /// A research agent with read-only capabilities and a conservative budget.
    ///
    /// This is the shape almost every agent should have; anything more has to
    /// be argued for explicitly at the call site.
    pub fn research(
        id: impl Into<String>,
        name: impl Into<String>,
        purpose: impl Into<String>,
        reviewed_at: Timestamp,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            role: AgentRole::Research,
            purpose: purpose.into(),
            competencies: Vec::new(),
            limitations: Vec::new(),
            capabilities: CapabilitySet::read_only().with(Capability::PublishHypothesis),
            budget: Budget::research_default(),
            escalation: EscalationPolicy::ReturnPartial,
            escalates_to: None,
            owner: "investment-research".to_string(),
            reviewed_at,
            review_interval: Duration::from_days(90),
            models: Vec::new(),
        }
    }

    pub fn with_role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    pub fn with_capabilities(mut self, capabilities: CapabilitySet) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities = self.capabilities.with(capability);
        self
    }

    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_competencies(mut self, competencies: Vec<String>) -> Self {
        self.competencies = competencies;
        self
    }

    pub fn with_limitations(mut self, limitations: Vec<String>) -> Self {
        self.limitations = limitations;
        self
    }

    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = owner.into();
        self
    }

    pub fn escalating_to(mut self, agent: impl Into<String>) -> Self {
        self.escalation = EscalationPolicy::Delegate;
        self.escalates_to = Some(agent.into());
        self
    }

    pub fn requiring_human(mut self) -> Self {
        self.escalation = EscalationPolicy::RequireHuman;
        self
    }

    /// When the current authorisation lapses.
    pub fn expires_at(&self) -> Timestamp {
        self.reviewed_at.saturating_add(self.review_interval)
    }

    pub fn is_expired(&self, now: Timestamp) -> bool {
        now >= self.expires_at()
    }

    /// Check the manifest is internally coherent and safe.
    ///
    /// Every rule here exists because the combination it forbids would let one
    /// component both make and check the same decision.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::invalid("agent manifest has no id"));
        }
        if !self
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(Error::invalid(format!(
                "agent id {} must be lowercase kebab-case",
                self.id
            )));
        }
        if self.purpose.trim().is_empty() {
            return Err(Error::invalid(format!(
                "agent {} declares no purpose; an agent nobody can describe is an agent nobody reviews",
                self.id
            )));
        }
        if self.owner.trim().is_empty() {
            return Err(Error::invalid(format!("agent {} has no owner", self.id)));
        }
        if self.capabilities.is_empty() {
            return Err(Error::invalid(format!(
                "agent {} is granted nothing and cannot do anything",
                self.id
            )));
        }
        if self.review_interval.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "agent {} has a non-positive review interval",
                self.id
            )));
        }
        self.budget.validate().map_err(|e| {
            Error::invalid(format!("agent {} has an invalid budget: {}", self.id, e))
        })?;

        if matches!(self.escalation, EscalationPolicy::Delegate) && self.escalates_to.is_none() {
            return Err(Error::invalid(format!(
                "agent {} delegates on escalation but names no delegate",
                self.id
            )));
        }
        if self.escalates_to.as_deref() == Some(self.id.as_str()) {
            return Err(Error::invalid(format!(
                "agent {} escalates to itself",
                self.id
            )));
        }

        self.validate_separation_of_duties()
    }

    /// The prohibitions. Each is a combination that would collapse a control.
    fn validate_separation_of_duties(&self) -> Result<()> {
        let has = |c: Capability| self.capabilities.contains(c);
        let deny = |reason: &str| -> Result<()> {
            Err(Error::denied(format!(
                "agent {} violates separation of duties: {reason}",
                self.id
            )))
        };

        // A research-side agent may never reach the market. This is the rule
        // that keeps a hallucinating analyst from becoming a trading loss.
        if self.role.is_research_side() {
            for capability in self.capabilities.iter() {
                if capability.touches_market() {
                    return deny(&format!("role {} may not hold {capability}", self.role));
                }
            }
        }

        // Proposing and approving the same trade is one person signing their
        // own expense claim.
        if has(Capability::ProposeTrade) && has(Capability::ApproveProposal) {
            return deny("proposing and approving trades cannot be the same agent");
        }

        // A veto is only independent if the vetoing agent has no stake in the
        // proposal it is judging.
        if has(Capability::VetoProposal) && has(Capability::ProposeTrade) {
            return deny("an agent that proposes trades cannot also hold veto authority");
        }

        // The control function checks limits; it must not be able to move them.
        if has(Capability::OverrideRiskLimit) && !matches!(self.role, AgentRole::Control) {
            return deny("only a control-role agent may hold override_risk_limit");
        }
        if has(Capability::OverrideRiskLimit) && has(Capability::SubmitOrder) {
            return deny(
                "overriding a limit and submitting the order it blocked cannot be the same agent",
            );
        }

        // Autonomy level and the kill switch are operator controls. No agent
        // raises its own authority, and only a control-role agent may stop the
        // platform — a research agent tripping the kill switch is a denial of
        // service with a plausible rationale attached.
        if has(Capability::ChangeAutonomyLevel) {
            return deny(
                "no agent may change the autonomy level; that is an authenticated operator action",
            );
        }
        if has(Capability::TriggerKillSwitch) && !matches!(self.role, AgentRole::Control) {
            return deny("only a control-role agent may trigger the kill switch");
        }

        // Only the execution role reaches a venue, and it must not originate
        // the intent it executes.
        if has(Capability::SubmitOrder) && !matches!(self.role, AgentRole::Execution) {
            return deny("only an execution-role agent may submit orders");
        }
        if has(Capability::SubmitOrder) && has(Capability::PublishHypothesis) {
            return deny("an execution agent must not originate the theses it trades");
        }

        // An adversarial agent that can publish its own theses stops being a
        // check on the others and becomes another one of them.
        if matches!(self.role, AgentRole::Adversarial) && has(Capability::PublishHypothesis) {
            return deny("an adversarial agent challenges hypotheses, it does not publish them");
        }

        Ok(())
    }

    /// Whether the agent may run at `now`: valid, unexpired, authorised.
    pub fn authorisation(&self, now: Timestamp) -> Result<()> {
        self.validate()?;
        if self.is_expired(now) {
            return Err(Error::denied(format!(
                "agent {} authorisation expired at {}; last reviewed {}",
                self.id,
                self.expires_at(),
                self.reviewed_at
            )));
        }
        Ok(())
    }
}
