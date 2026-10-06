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
    pub fn opportunity(&self) -> &str {
        &self.opportunity
    }
    pub fn instrument(&self) -> &str {
        &self.instrument
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
}
