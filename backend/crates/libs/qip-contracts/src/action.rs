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

/// What an executable opportunity becomes. Fields are private and the only
/// constructor is [`ExecutableScope::route`], so an intent cannot be built for
/// an instrument the scope does not name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionIntent {
    opportunity: String,
    instrument: String,
}

impl ActionIntent {
    pub fn opportunity(&self) -> &str {
        &self.opportunity
    }
    pub fn instrument(&self) -> &str {
        &self.instrument
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
        let (opportunity, instrument) = (opportunity.to_string(), instrument.to_string());
        if self.instruments.contains(&instrument) {
            Routed::Executable(ActionIntent {
                opportunity,
                instrument,
            })
        } else {
            Routed::Shadow(ShadowRecord {
                opportunity,
                instrument,
            })
        }
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
