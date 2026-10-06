//! The action boundary of blueprint §1.2 and §25: what may leave research and
//! become an action, and what record each step leaves.
//!
//! Four refusals live here, each a pure function so the property can be
//! tested without a venue:
//!
//! * [`ExecutableScope::route`] (GOV-016) — an opportunity outside the scope
//!   comes back as a [`ShadowRecord`], a type with no path to
//!   [`ActionIntent`]. Re-scoring re-enters `route`; it has no other door.
//! * [`ToolRegistry`] (GOV-017) — a tool acts only if registered with all six
//!   declarations, inside its rate and budget, and not revoked. Every refusal
//!   after revocation is audited.
//! * [`check_justification`] (GOV-021) — profit and price impact are motives,
//!   not grounds; a justification with nothing else is refused.
//! * [`GoalApproval`] -> [`PlanApproval`] -> [`ExecutionRecord`] (GOV-018) —
//!   three records, each naming its own actor and time and the one before it,
//!   so each approval can be attributed alone.
//!
//! Nothing here submits anything. `ActionIntent` is a value, not a call.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::{Error, Result, Timestamp, sha256_hex};

/// Action classes, each with distinct blast radius and authority requirements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionClass {
    Capital,
    Communication,
    Product,
    Operational,
    Research,
}

impl ActionClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionClass::Capital => "capital",
            ActionClass::Communication => "communication",
            ActionClass::Product => "product",
            ActionClass::Operational => "operational",
            ActionClass::Research => "research",
        }
    }
}

/// Authority levels, each granting permission to execute actions of up to that class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuthorityLevel {
    Shadow,
    Research,
    Operational,
    Product,
    Communication,
    Capital,
}

impl AuthorityLevel {
    pub fn permits(&self, action_class: ActionClass) -> bool {
        match (self, action_class) {
            (AuthorityLevel::Shadow, _) => false,
            (AuthorityLevel::Research, ActionClass::Research) => true,
            (AuthorityLevel::Operational, ActionClass::Research | ActionClass::Operational) => true,
            (
                AuthorityLevel::Product,
                ActionClass::Research | ActionClass::Operational | ActionClass::Product,
            ) => true,
            (
                AuthorityLevel::Communication,
                ActionClass::Research
                | ActionClass::Operational
                | ActionClass::Product
                | ActionClass::Communication,
            ) => true,
            (AuthorityLevel::Capital, _) => true,
            _ => false,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorityLevel::Shadow => "shadow",
            AuthorityLevel::Research => "research",
            AuthorityLevel::Operational => "operational",
            AuthorityLevel::Product => "product",
            AuthorityLevel::Communication => "communication",
            AuthorityLevel::Capital => "capital",
        }
    }
}

/// What an executable opportunity becomes. Fields are private and the only
/// constructor is [`ExecutableScope::route`], so an intent cannot be built for
/// an instrument the scope does not name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionIntent {
    opportunity: String,
    instrument: String,
    action_class: ActionClass,
}

impl ActionIntent {
    pub fn opportunity(&self) -> &str {
        &self.opportunity
    }
    pub fn instrument(&self) -> &str {
        &self.instrument
    }
    pub fn action_class(&self) -> ActionClass {
        self.action_class
    }
}

/// An opportunity outside the executable scope: recordable, simulable,
/// scoreable, and nothing else. Deliberately no `From`/`Into` to
/// [`ActionIntent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowRecord {
    opportunity: String,
    instrument: String,
}

impl ShadowRecord {
    pub fn new(opportunity: String, instrument: String) -> Self {
        Self {
            opportunity,
            instrument,
        }
    }

    pub fn opportunity(&self) -> &str {
        &self.opportunity
    }
    pub fn instrument(&self) -> &str {
        &self.instrument
    }
}

/// CAPITAL-036: Shadow portfolio holding counterfactual allocations beside
/// every paper allocation. Aggregates alternate-size and rejected-opportunity
/// shadows for a single allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowPortfolio {
    /// Identifier for the allocation this portfolio shadows.
    pub allocation_id: String,
    /// Alternate-size shadows: smaller or larger sizes of the actual allocation.
    pub size_alternatives: Vec<ShadowRecord>,
    /// Rejected-opportunity shadows: paths that were declined but would have
    /// been profitable (regrets) or venues/hedges not taken.
    pub rejected_opportunities: Vec<ShadowRecord>,
}

impl ShadowPortfolio {
    pub fn new(allocation_id: String) -> Self {
        Self {
            allocation_id,
            size_alternatives: Vec::new(),
            rejected_opportunities: Vec::new(),
        }
    }

    /// Checks if this shadow portfolio has all required shadows for CAPITAL-036:
    /// at least one alternate-size and one rejected-opportunity shadow.
    pub fn is_complete(&self) -> bool {
        !self.size_alternatives.is_empty() && !self.rejected_opportunities.is_empty()
    }

    /// Adds a size alternative shadow.
    pub fn add_size_alternative(&mut self, shadow: ShadowRecord) {
        self.size_alternatives.push(shadow);
    }

    /// Adds a rejected opportunity shadow.
    pub fn add_rejected_opportunity(&mut self, shadow: ShadowRecord) {
        self.rejected_opportunities.push(shadow);
    }
}

/// Where an opportunity goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routed {
    Executable(ActionIntent),
    Shadow(ShadowRecord),
}

/// The instruments an opportunity may become an action in.
#[derive(Clone, Debug, Default)]
pub struct ExecutableScope {
    instruments: BTreeSet<String>,
}

impl ExecutableScope {
    pub fn new(instruments: impl IntoIterator<Item = String>) -> Self {
        Self {
            instruments: instruments.into_iter().collect(),
        }
    }

    /// Fail closed: an empty scope makes everything shadow. The score is not a
    /// parameter, because a re-score must never be able to promote.
    pub fn route(&self, opportunity: &str, instrument: &str) -> Routed {
        self.route_with_class(opportunity, instrument, ActionClass::Capital)
    }

    /// Route with an explicit action class.
    pub fn route_with_class(
        &self,
        opportunity: &str,
        instrument: &str,
        action_class: ActionClass,
    ) -> Routed {
        let (opportunity, instrument) = (opportunity.to_string(), instrument.to_string());
        if self.instruments.contains(&instrument) {
            Routed::Executable(ActionIntent {
                opportunity,
                instrument,
                action_class,
            })
        } else {
            Routed::Shadow(ShadowRecord {
                opportunity,
                instrument,
            })
        }
    }
}

/// Check that an action intent can execute with the given authority level.
pub fn check_authority(intent: &ActionIntent, authority: AuthorityLevel) -> Result<()> {
    if authority.permits(intent.action_class) {
        Ok(())
    } else {
        Err(Error::denied(format!(
            "action class {} requires authority level {} or higher, but only {} is available",
            intent.action_class.as_str(),
            match intent.action_class {
                ActionClass::Capital => "capital",
                ActionClass::Communication => "communication",
                ActionClass::Product => "product",
                ActionClass::Operational => "operational",
                ActionClass::Research => "research",
            },
            authority.as_str()
        )))
    }
}

/// The six declarations a tool must make before it can act.
#[derive(Clone, Debug)]
pub struct ToolRegistration {
    pub tool: String,
    pub acting_identity: String,
    pub permission_scope: BTreeSet<String>,
    pub disclosure_policy: String,
    pub channels: BTreeSet<String>,
    pub max_actions_per_window: u64,
    pub budget_units: u64,
    pub revocation_path: String,
}

/// One audited tool decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolAudit {
    pub tool: String,
    pub at: Timestamp,
    pub admitted: bool,
    pub reason: String,
}

#[derive(Debug)]
struct Slot {
    reg: ToolRegistration,
    revoked: bool,
    window: u64,
    in_window: u64,
    spent: u64,
}

/// Registry of action tools. Unregistered means refused.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Slot>,
    audit: Vec<ToolAudit>,
}

fn blank(s: &str) -> bool {
    s.trim().is_empty()
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuse a registration missing any of the six fields (zero limits count
    /// as missing: a zero rate or budget is a tool that can never act, which
    /// is a misconfiguration rather than a limit).
    pub fn register(&mut self, reg: ToolRegistration) -> Result<()> {
        let missing = [
            ("tool", blank(&reg.tool)),
            ("acting identity", blank(&reg.acting_identity)),
            ("permission scope", reg.permission_scope.is_empty()),
            ("disclosure policy", blank(&reg.disclosure_policy)),
            ("audience/channel rules", reg.channels.is_empty()),
            ("rate limit", reg.max_actions_per_window == 0),
            ("budget", reg.budget_units == 0),
            ("revocation path", blank(&reg.revocation_path)),
        ];
        if let Some((name, _)) = missing.iter().find(|(_, m)| *m) {
            return Err(Error::invalid(format!(
                "action tool registration lacks its {name}; declare all six of identity, scope, disclosure, channels, limits and revocation path"
            )));
        }
        self.tools.insert(
            reg.tool.clone(),
            Slot {
                reg,
                revoked: false,
                window: 0,
                in_window: 0,
                spent: 0,
            },
        );
        Ok(())
    }

    /// Revoke a tool; its next action is refused and audited.
    pub fn revoke(&mut self, tool: &str) -> Result<()> {
        let slot = self
            .tools
            .get_mut(tool)
            .ok_or_else(|| Error::not_found(format!("tool {tool} is not registered")))?;
        slot.revoked = true;
        Ok(())
    }

    /// Admit or refuse one action. `window` is the caller's rate window id;
    /// the count resets when it changes. Every outcome is audited.
    pub fn authorise(
        &mut self,
        tool: &str,
        channel: &str,
        window: u64,
        cost: u64,
        at: Timestamp,
    ) -> Result<()> {
        let verdict = match self.tools.get_mut(tool) {
            None => Err("tool is not registered".to_string()),
            Some(s) if s.revoked => Err("tool is revoked".to_string()),
            Some(s) if !s.reg.channels.contains(channel) => {
                Err(format!("channel {channel} is outside the tool's channels"))
            }
            Some(s) => {
                if s.window != window {
                    s.window = window;
                    s.in_window = 0;
                }
                if s.in_window >= s.reg.max_actions_per_window {
                    Err("rate limit reached for this window".to_string())
                } else if s.spent.saturating_add(cost) > s.reg.budget_units {
                    Err("budget would be exceeded".to_string())
                } else {
                    s.in_window += 1;
                    s.spent += cost;
                    Ok(())
                }
            }
        };
        self.audit.push(ToolAudit {
            tool: tool.to_string(),
            at,
            admitted: verdict.is_ok(),
            reason: verdict.clone().err().unwrap_or_default(),
        });
        verdict.map_err(|why| Error::denied(format!("action refused: {why}")))
    }

    pub fn audit(&self) -> &[ToolAudit] {
        &self.audit
    }
}

/// One stated ground for a market-facing action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ground {
    ExpectedProfit,
    DesiredPriceImpact,
    /// An independent, evidence-backed reason; the text names the evidence.
    Evidence(String),
}

/// Refuse a justification that consists only of profit and price impact.
pub fn check_justification(grounds: &[Ground]) -> Result<()> {
    let independent = grounds
        .iter()
        .any(|g| matches!(g, Ground::Evidence(e) if !blank(e)));
    if independent {
        Ok(())
    } else {
        Err(Error::denied(
            "justification names only expected profit or price impact, or nothing; add an independent, evidence-backed ground",
        ))
    }
}

fn digest(parts: &[&str]) -> String {
    sha256_hex(parts.join("\u{1f}").as_bytes())
}

/// Stage 1 of the intervention record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalApproval {
    pub id: String,
    pub approver: String,
    pub at: Timestamp,
    pub subject: String,
}

/// Stage 2; names the goal it relied on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanApproval {
    pub id: String,
    pub approver: String,
    pub at: Timestamp,
    pub subject: String,
    pub goal: String,
}

/// Stage 3; names the plan it relied on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionRecord {
    pub id: String,
    pub executor: String,
    pub at: Timestamp,
    pub subject: String,
    pub plan: String,
}

fn actor(who: &str, role: &str) -> Result<()> {
    if blank(who) {
        Err(Error::invalid(format!(
            "a {role} must be named; an unattributed record cannot be reviewed"
        )))
    } else {
        Ok(())
    }
}

impl GoalApproval {
    pub fn approve(approver: &str, at: Timestamp, subject: &str) -> Result<Self> {
        actor(approver, "goal approver")?;
        let id = digest(&["goal", approver, &at.as_nanos().to_string(), subject]);
        Ok(Self {
            id,
            approver: approver.into(),
            at,
            subject: subject.into(),
        })
    }
}

impl PlanApproval {
    /// A plan cannot be approved before its goal was.
    pub fn approve(
        goal: &GoalApproval,
        approver: &str,
        at: Timestamp,
        subject: &str,
    ) -> Result<Self> {
        actor(approver, "plan approver")?;
        if at < goal.at {
            return Err(Error::invalid(
                "plan approval predates the goal approval it relies on; check the clock or the order",
            ));
        }
        let id = digest(&[
            "plan",
            &goal.id,
            approver,
            &at.as_nanos().to_string(),
            subject,
        ]);
        Ok(Self {
            id,
            approver: approver.into(),
            at,
            subject: subject.into(),
            goal: goal.id.clone(),
        })
    }
}

impl ExecutionRecord {
    /// An execution cannot precede the plan approval it relies on.
    pub fn execute(
        plan: &PlanApproval,
        executor: &str,
        at: Timestamp,
        subject: &str,
    ) -> Result<Self> {
        actor(executor, "executor")?;
        if at < plan.at {
            return Err(Error::invalid(
                "execution predates the plan approval it relies on; check the clock or the order",
            ));
        }
        let id = digest(&[
            "exec",
            &plan.id,
            executor,
            &at.as_nanos().to_string(),
            subject,
        ]);
        Ok(Self {
            id,
            executor: executor.into(),
            at,
            subject: subject.into(),
            plan: plan.id.clone(),
        })
    }
}

/// AGENCY-009: Pre-action gate enforcing nine checks before external intervention.
/// Actions that can move money, change external systems or communicate publicly
/// must pass all nine checks or be refused with no adapter invoked.
#[derive(Clone, Debug)]
pub struct PreActionGate {
    /// Registered identities with verified credentials.
    identities: BTreeSet<String>,
    /// Identity → disclosure requirement mapping.
    disclosure_requirements: BTreeMap<String, String>,
    /// Authorized jurisdictions for this gate.
    jurisdictions: BTreeSet<String>,
}

impl PreActionGate {
    pub fn new(
        identities: impl IntoIterator<Item = String>,
        disclosure_requirements: impl IntoIterator<Item = (String, String)>,
        jurisdictions: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            identities: identities.into_iter().collect(),
            disclosure_requirements: disclosure_requirements.into_iter().collect(),
            jurisdictions: jurisdictions.into_iter().collect(),
        }
    }

    /// Apply all nine checks to an action before execution.
    /// Returns Ok(()) if all checks pass, or an error naming the first check that fails.
    pub fn check(
        &self,
        acting_identity: &str,
        action_class: &str,
        claimed_effect: &str,
        evidence_count: usize,
        origin: &str,
        channel: &str,
        jurisdiction: &str,
        is_technically_feasible: bool,
        disclosure_provided: bool,
    ) -> Result<()> {
        // Check 1: Acting identity must be registered and not blank.
        if blank(acting_identity) || !self.identities.contains(acting_identity) {
            return Err(Error::denied(
                "check 1 failed: acting identity is not registered",
            ));
        }

        // Check 2: Authority level must be sufficient for action class.
        // Conservative default: capital actions require identity in a capital list.
        if action_class == "capital"
            && !self
                .identities
                .contains(&format!("{acting_identity}:capital"))
        {
            return Err(Error::denied(
                "check 2 failed: authority level insufficient for action class",
            ));
        }

        // Check 3: Truthfulness — claims must be backed by evidence.
        if !claimed_effect.is_empty() && evidence_count == 0 {
            return Err(Error::denied(
                "check 3 failed: claimed effect lacks supporting evidence",
            ));
        }

        // Check 4: Provenance — action origin must be recorded and not blank.
        if blank(origin) {
            return Err(Error::denied(
                "check 4 failed: action provenance is not recorded",
            ));
        }

        // Check 5: Disclosure — identity's required disclosure must be provided.
        if let Some(requirement) = self.disclosure_requirements.get(acting_identity)
            && !requirement.is_empty()
            && !disclosure_provided
        {
            return Err(Error::denied(
                "check 5 failed: disclosure requirement not met",
            ));
        }

        // Check 6: Market-conduct rules — detect prohibited patterns.
        // Conservative: refuse any action with "wash" or "spoof" in description.
        if claimed_effect.to_lowercase().contains("wash")
            || claimed_effect.to_lowercase().contains("spoof")
            || claimed_effect.to_lowercase().contains("pump-and-dump")
        {
            return Err(Error::denied(
                "check 6 failed: action violates market-conduct rules",
            ));
        }

        // Check 7: Channel permissions — channel must not be blank.
        if blank(channel) {
            return Err(Error::denied(
                "check 7 failed: channel permissions cannot be verified",
            ));
        }

        // Check 8: Jurisdiction — must be in authorized set.
        if blank(jurisdiction) || !self.jurisdictions.contains(jurisdiction) {
            return Err(Error::denied(
                "check 8 failed: jurisdiction is not authorized",
            ));
        }

        // Check 9: Feasibility — action must be technically possible.
        if !is_technically_feasible {
            return Err(Error::denied(
                "check 9 failed: action is not technically feasible",
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_intent_of_a_class_requiring_capital_authority_is_refused_when_only_lower_authority_is_present()
     {
        let scope = ExecutableScope::new(vec!["instrument_a".into()]);
        let intent =
            match scope.route_with_class("opportunity", "instrument_a", ActionClass::Capital) {
                Routed::Executable(i) => i,
                Routed::Shadow(_) => panic!("should be executable"),
            };

        let result = check_authority(&intent, AuthorityLevel::Operational);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message()
                .contains("authority level capital")
        );
    }

    #[test]
    fn an_action_intent_of_a_class_requiring_capital_authority_is_admitted_once_the_signed_authority_is_supplied()
     {
        let scope = ExecutableScope::new(vec!["instrument_a".into()]);
        let intent =
            match scope.route_with_class("opportunity", "instrument_a", ActionClass::Capital) {
                Routed::Executable(i) => i,
                Routed::Shadow(_) => panic!("should be executable"),
            };

        let result = check_authority(&intent, AuthorityLevel::Capital);
        assert!(result.is_ok());
    }

    #[test]
    fn authority_level_permits_checks_class_hierarchy() {
        assert!(AuthorityLevel::Capital.permits(ActionClass::Capital));
        assert!(AuthorityLevel::Capital.permits(ActionClass::Communication));
        assert!(!AuthorityLevel::Communication.permits(ActionClass::Capital));
        assert!(AuthorityLevel::Operational.permits(ActionClass::Operational));
        assert!(!AuthorityLevel::Research.permits(ActionClass::Operational));
    }

    #[test]
    fn action_class_capital_requires_capital_authority() {
        let scope = ExecutableScope::new(vec!["payment".into()]);
        let capital_intent =
            match scope.route_with_class("pay_vendor", "payment", ActionClass::Capital) {
                Routed::Executable(i) => i,
                Routed::Shadow(_) => panic!("should be executable"),
            };

        assert!(check_authority(&capital_intent, AuthorityLevel::Shadow).is_err());
        assert!(check_authority(&capital_intent, AuthorityLevel::Capital).is_ok());
    }

    // AGENCY-007: Actions execute only through registered tools

    #[test]
    fn an_unregistered_tool_is_refused() {
        let mut registry = ToolRegistry::new();
        let at = Timestamp::from_nanos(1000);

        let result = registry.authorise("unregistered_tool", "channel_a", 0, 100, at);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message()
                .contains("tool is not registered")
        );
    }

    #[test]
    fn a_registered_tool_with_all_six_fields_is_admitted() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into(), "ledger".into()]
                .into_iter()
                .collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into(), "orchestrator".into()]
                .into_iter()
                .collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke/payment".into(),
        };
        assert!(registry.register(reg).is_ok());

        let at = Timestamp::from_nanos(1000);
        let result = registry.authorise("payment", "api", 0, 100, at);
        assert!(result.is_ok());
    }

    #[test]
    fn a_tool_registration_lacking_tool_name_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("tool"));
    }

    #[test]
    fn a_tool_registration_lacking_acting_identity_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("identity"));
    }

    #[test]
    fn a_tool_registration_lacking_permission_scope_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec![].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("scope"));
    }

    #[test]
    fn a_tool_registration_lacking_disclosure_policy_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("disclosure"));
    }

    #[test]
    fn a_tool_registration_lacking_channels_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec![].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("channel"));
    }

    #[test]
    fn a_tool_registration_with_zero_rate_limit_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 0,
            budget_units: 1000,
            revocation_path: "/policy/revoke".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("rate limit"));
    }

    #[test]
    fn a_tool_registration_with_zero_budget_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 0,
            revocation_path: "/policy/revoke".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("budget"));
    }

    #[test]
    fn a_tool_registration_lacking_revocation_path_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "".into(),
        };
        let result = registry.register(reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("revocation"));
    }

    #[test]
    fn a_revoked_tool_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke/payment".into(),
        };
        assert!(registry.register(reg).is_ok());

        assert!(registry.revoke("payment").is_ok());

        let at = Timestamp::from_nanos(1000);
        let result = registry.authorise("payment", "api", 0, 100, at);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("tool is revoked"));
    }

    #[test]
    fn a_tool_call_on_an_unauthorized_channel_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke/payment".into(),
        };
        assert!(registry.register(reg).is_ok());

        let at = Timestamp::from_nanos(1000);
        let result = registry.authorise("payment", "webhook", 0, 100, at);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("webhook"));
    }

    #[test]
    fn a_tool_call_exceeding_the_rate_limit_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 2,
            budget_units: 1000,
            revocation_path: "/policy/revoke/payment".into(),
        };
        assert!(registry.register(reg).is_ok());

        let at = Timestamp::from_nanos(1000);
        assert!(registry.authorise("payment", "api", 0, 100, at).is_ok());
        assert!(registry.authorise("payment", "api", 0, 100, at).is_ok());
        let result = registry.authorise("payment", "api", 0, 100, at);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("rate limit"));
    }

    #[test]
    fn a_tool_call_exceeding_the_budget_is_refused() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 100,
            budget_units: 500,
            revocation_path: "/policy/revoke/payment".into(),
        };
        assert!(registry.register(reg).is_ok());

        let at = Timestamp::from_nanos(1000);
        assert!(registry.authorise("payment", "api", 0, 300, at).is_ok());
        let result = registry.authorise("payment", "api", 0, 300, at);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("budget"));
    }

    #[test]
    fn every_tool_authorization_is_audited() {
        let mut registry = ToolRegistry::new();
        let reg = ToolRegistration {
            tool: "payment".into(),
            acting_identity: "operator_1".into(),
            permission_scope: vec!["accounts".into()].into_iter().collect(),
            disclosure_policy: "must_audit".into(),
            channels: vec!["api".into()].into_iter().collect(),
            max_actions_per_window: 10,
            budget_units: 1000,
            revocation_path: "/policy/revoke/payment".into(),
        };
        assert!(registry.register(reg).is_ok());

        let at = Timestamp::from_nanos(1000);
        assert!(registry.authorise("payment", "api", 0, 100, at).is_ok());

        let at2 = Timestamp::from_nanos(2000);
        let _ = registry.authorise("unregistered", "api", 0, 100, at2);

        let audit = registry.audit();
        assert_eq!(audit.len(), 2);
        assert!(audit[0].admitted);
        assert!(!audit[1].admitted);
    }

    // AGENCY-009: Pre-action gate with 9 checks

    #[test]
    fn check_1_fails_when_acting_identity_is_blank() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 1 failed"));
    }

    #[test]
    fn check_1_fails_when_acting_identity_is_not_registered() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "unregistered_op",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 1 failed"));
    }

    #[test]
    fn check_1_passes_when_identity_is_registered() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_2_fails_for_capital_action_without_capital_authority() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "capital",
            "deploy_capital",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 2 failed"));
    }

    #[test]
    fn check_2_passes_for_capital_action_with_capital_authority() {
        let gate = PreActionGate::new(
            vec!["operator_1".into(), "operator_1:capital".into()],
            vec![],
            vec!["US".into()],
        );
        let result = gate.check(
            "operator_1",
            "capital",
            "deploy_capital",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_3_fails_when_claim_has_no_evidence() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "this will improve returns",
            0,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 3 failed"));
    }

    #[test]
    fn check_3_passes_when_claim_is_backed_by_evidence() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "this will improve returns",
            2,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_3_passes_when_claim_is_empty() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "",
            0,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_4_fails_when_origin_is_blank() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 4 failed"));
    }

    #[test]
    fn check_4_passes_when_origin_is_recorded() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "opportunity:123",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_5_fails_when_disclosure_required_but_not_provided() {
        let gate = PreActionGate::new(
            vec!["operator_1".into()],
            vec![("operator_1".into(), "must_disclose".into())],
            vec!["US".into()],
        );
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 5 failed"));
    }

    #[test]
    fn check_5_passes_when_disclosure_not_required() {
        let gate = PreActionGate::new(
            vec!["operator_1".into()],
            vec![("operator_1".into(), "".into())],
            vec!["US".into()],
        );
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_6_fails_for_wash_trading_pattern() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "wash trading strategy",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 6 failed"));
    }

    #[test]
    fn check_6_fails_for_spoofing_pattern() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "spoof the market",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 6 failed"));
    }

    #[test]
    fn check_6_fails_for_pump_and_dump_pattern() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "pump-and-dump scheme",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 6 failed"));
    }

    #[test]
    fn check_6_passes_for_legitimate_action() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance portfolio",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_7_fails_when_channel_is_blank() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "",
            "US",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 7 failed"));
    }

    #[test]
    fn check_7_passes_when_channel_is_provided() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_8_fails_when_jurisdiction_is_blank() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 8 failed"));
    }

    #[test]
    fn check_8_fails_when_jurisdiction_not_authorized() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "CN",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 8 failed"));
    }

    #[test]
    fn check_8_passes_when_jurisdiction_is_authorized() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_9_fails_when_action_is_not_feasible() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            false,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 9 failed"));
    }

    #[test]
    fn check_9_passes_when_action_is_feasible() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "operator_1",
            "research",
            "rebalance",
            1,
            "origin",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn all_nine_checks_pass_for_valid_action() {
        let gate = PreActionGate::new(
            vec!["operator_1".into(), "operator_1:capital".into()],
            vec![("operator_1".into(), "".into())],
            vec!["US".into()],
        );
        let result = gate.check(
            "operator_1",
            "capital",
            "deploy_capital_for_returns",
            3,
            "opportunity:456",
            "api",
            "US",
            true,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn gate_refuses_unregistered_identity_before_checking_other_gates() {
        let gate = PreActionGate::new(vec!["operator_1".into()], vec![], vec!["US".into()]);
        let result = gate.check(
            "nobody",
            "capital",
            "wash trading scheme",
            0,
            "",
            "",
            "CN",
            false,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("check 1 failed"));
    }
}
