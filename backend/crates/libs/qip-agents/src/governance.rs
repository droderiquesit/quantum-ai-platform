//! Organisation-level governance.
//!
//! A manifest can be individually valid while the roster as a whole is unsafe:
//! two agents each holding half of a control, nobody holding a veto, or every
//! research agent owned by the same team that owns execution. Those are
//! properties of the set, so they are checked on the set.
//!
//! [`Roster::validate`] is run at start-up and in CI. A platform whose roster
//! does not validate does not start, because the alternative is discovering
//! the gap from an incident report.

use crate::capability::Capability;
use crate::manifest::{AgentManifest, AgentRole};
use crate::runtime::{AgentRunRecord, RunStatus};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A governance problem found in the roster.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernanceFinding {
    /// `error` blocks start-up; `warning` is reported and allowed.
    pub severity: Severity,
    pub rule: String,
    pub detail: String,
    /// Agents involved, so the fix has an address.
    pub agents: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

/// The declared agent organisation.
#[derive(Clone, Debug, Default)]
pub struct Roster {
    manifests: Vec<AgentManifest>,
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_manifests(manifests: Vec<AgentManifest>) -> Self {
        Self { manifests }
    }

    pub fn add(&mut self, manifest: AgentManifest) {
        self.manifests.push(manifest);
    }

    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentManifest> {
        self.manifests.iter()
    }

    pub fn get(&self, id: &str) -> Option<&AgentManifest> {
        self.manifests.iter().find(|m| m.id == id)
    }

    pub fn with_role(&self, role: AgentRole) -> Vec<&AgentManifest> {
        self.manifests.iter().filter(|m| m.role == role).collect()
    }

    pub fn holding(&self, capability: Capability) -> Vec<&AgentManifest> {
        self.manifests
            .iter()
            .filter(|m| m.capabilities.contains(capability))
            .collect()
    }

    /// Check the roster as a whole.
    ///
    /// Returns every finding rather than the first, because fixing governance
    /// one error per CI run is how a roster stays broken for a week.
    pub fn review(&self, now: Timestamp) -> Vec<GovernanceFinding> {
        let mut findings = Vec::new();
        let error = |rule: &str, detail: String, agents: Vec<String>| GovernanceFinding {
            severity: Severity::Error,
            rule: rule.to_string(),
            detail,
            agents,
        };
        let warn = |rule: &str, detail: String, agents: Vec<String>| GovernanceFinding {
            severity: Severity::Warning,
            rule: rule.to_string(),
            detail,
            agents,
        };

        // Individual validity first: a roster of invalid manifests has no
        // meaningful collective properties.
        for manifest in &self.manifests {
            if let Err(e) = manifest.validate() {
                findings.push(error(
                    "manifest-valid",
                    e.message().to_string(),
                    vec![manifest.id.clone()],
                ));
            }
            if manifest.is_expired(now) {
                findings.push(error(
                    "authorisation-current",
                    format!(
                        "authorisation expired at {}; agents run on reviewed permissions only",
                        manifest.expires_at()
                    ),
                    vec![manifest.id.clone()],
                ));
            }
        }

        // Duplicate ids would make the audit trail ambiguous about which agent
        // did what.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for manifest in &self.manifests {
            if !seen.insert(manifest.id.as_str()) {
                findings.push(error(
                    "unique-id",
                    "two agents share an id; run records could not be attributed".to_string(),
                    vec![manifest.id.clone()],
                ));
            }
        }

        // Escalation targets must exist, or an escalation is a silent drop.
        for manifest in &self.manifests {
            if let Some(target) = &manifest.escalates_to
                && self.get(target).is_none()
            {
                findings.push(error(
                    "escalation-target-exists",
                    format!("escalates to {target}, which is not in the roster"),
                    vec![manifest.id.clone()],
                ));
            }
        }
        findings.extend(self.escalation_cycles());

        // Somebody must be able to say no. A roster with proposers and no veto
        // has a risk function in name only.
        let vetoers = self.holding(Capability::VetoProposal);
        if vetoers.is_empty() && !self.holding(Capability::ProposeTrade).is_empty() {
            findings.push(error(
                "veto-exists",
                "the roster proposes trades but no agent holds veto authority".to_string(),
                Vec::new(),
            ));
        }
        for manifest in &vetoers {
            if manifest.role != AgentRole::Control {
                findings.push(error(
                    "veto-is-control",
                    format!(
                        "holds veto authority with role {}; a veto is only independent from a control-role agent",
                        manifest.role
                    ),
                    vec![manifest.id.clone()],
                ));
            }
        }

        // Execution must be reachable but narrow.
        let submitters = self.holding(Capability::SubmitOrder);
        if submitters.len() > 1 {
            findings.push(warn(
                "single-execution-path",
                format!(
                    "{} agents may submit orders; every additional path is another place a limit can be bypassed",
                    submitters.len()
                ),
                submitters.iter().map(|m| m.id.clone()).collect(),
            ));
        }

        // An adversarial function that reports to the people it reviews is not
        // adversarial. Ownership is the practical proxy for independence.
        let adversarial_owners: BTreeSet<&str> = self
            .with_role(AgentRole::Adversarial)
            .iter()
            .map(|m| m.owner.as_str())
            .collect();
        let research_owners: BTreeSet<&str> = self
            .with_role(AgentRole::Research)
            .iter()
            .map(|m| m.owner.as_str())
            .collect();
        let shared: Vec<&&str> = adversarial_owners.intersection(&research_owners).collect();
        if !shared.is_empty() {
            findings.push(warn(
                "adversarial-independence",
                format!(
                    "adversarial and research agents share owner(s): {}",
                    shared.iter().map(|s| **s).collect::<Vec<_>>().join(", ")
                ),
                Vec::new(),
            ));
        }
        if self.with_role(AgentRole::Adversarial).is_empty() && !self.manifests.is_empty() {
            findings.push(error(
                "adversarial-exists",
                "no agent is charged with attacking the organisation's own conclusions".to_string(),
                Vec::new(),
            ));
        }

        // The control function must not be owned by the desk it constrains.
        for control in self.with_role(AgentRole::Control) {
            for executor in self.with_role(AgentRole::Execution) {
                if control.owner == executor.owner {
                    findings.push(error(
                        "control-independence",
                        format!(
                            "control agent {} and execution agent {} share owner {}",
                            control.id, executor.id, control.owner
                        ),
                        vec![control.id.clone(), executor.id.clone()],
                    ));
                }
            }
        }

        findings
    }

    /// Validate, failing on any error-severity finding.
    pub fn validate(&self, now: Timestamp) -> Result<Vec<GovernanceFinding>> {
        let findings = self.review(now);
        let errors: Vec<&GovernanceFinding> = findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        if errors.is_empty() {
            return Ok(findings);
        }
        let detail = errors
            .iter()
            .map(|f| {
                format!(
                    "[{}] {}{}",
                    f.rule,
                    if f.agents.is_empty() {
                        String::new()
                    } else {
                        format!("{}: ", f.agents.join(", "))
                    },
                    f.detail
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        Err(Error::denied(format!("roster governance failed: {detail}")))
    }

    /// Escalation chains that loop, which would hang an escalation forever.
    fn escalation_cycles(&self) -> Vec<GovernanceFinding> {
        let mut findings = Vec::new();
        for manifest in &self.manifests {
            let mut visited = BTreeSet::new();
            let mut current = manifest.id.as_str();
            visited.insert(current);
            let mut chain = vec![current.to_string()];
            while let Some(next) = self.get(current).and_then(|m| m.escalates_to.as_deref()) {
                chain.push(next.to_string());
                if !visited.insert(next) {
                    findings.push(GovernanceFinding {
                        severity: Severity::Error,
                        rule: "escalation-terminates".to_string(),
                        detail: format!("escalation cycle: {}", chain.join(" -> ")),
                        agents: vec![manifest.id.clone()],
                    });
                    break;
                }
                current = next;
            }
        }
        findings
    }
}

/// The append-only record of what agents did.
///
/// Separate from the event log because it answers a different question: the
/// event log says what the platform decided, this says which agent touched
/// what to get there, including the attempts that were refused.
#[derive(Clone, Debug, Default)]
pub struct AuditTrail {
    records: Vec<AgentRunRecord>,
}

impl AuditTrail {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, run: AgentRunRecord) {
        self.records.push(run);
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &[AgentRunRecord] {
        &self.records
    }

    /// Runs for one agent, oldest first.
    pub fn for_agent(&self, agent_id: &str) -> Vec<&AgentRunRecord> {
        self.records
            .iter()
            .filter(|r| r.agent_id == agent_id)
            .collect()
    }

    /// Every run in one correlation chain — the answer to "show me everything
    /// that happened because of this observation".
    pub fn for_correlation(&self, correlation: &str) -> Vec<&AgentRunRecord> {
        self.records
            .iter()
            .filter(|r| r.lineage.correlation_id.as_str() == correlation)
            .collect()
    }

    /// Runs where an agent attempted something it was not granted.
    ///
    /// Empty is the expected state. Non-empty is either a bug or an agent
    /// behaving in a way its manifest did not anticipate; both need looking at.
    pub fn permission_violations(&self) -> Vec<&AgentRunRecord> {
        self.records
            .iter()
            .filter(|r| !r.denied_accesses().is_empty())
            .collect()
    }

    /// Runs that ended in failure or refusal.
    pub fn failures(&self) -> Vec<&AgentRunRecord> {
        self.records
            .iter()
            .filter(|r| {
                matches!(
                    r.status,
                    RunStatus::Failed { .. } | RunStatus::Refused { .. }
                )
            })
            .collect()
    }

    /// Per-agent budget utilisation, for spotting an agent that is always at
    /// the edge of its allowance and about to start failing.
    pub fn utilisation_by_agent(&self) -> BTreeMap<String, f64> {
        let mut totals: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        for record in &self.records {
            let entry = totals.entry(record.agent_id.clone()).or_insert((0.0, 0));
            entry.0 += record.utilisation;
            entry.1 += 1;
        }
        totals
            .into_iter()
            .map(|(agent, (sum, count))| (agent, if count == 0 { 0.0 } else { sum / count as f64 }))
            .collect()
    }

    /// Success rate per agent over recorded runs.
    pub fn success_rate(&self, agent_id: &str) -> Option<f64> {
        let runs = self.for_agent(agent_id);
        if runs.is_empty() {
            return None;
        }
        let ok = runs.iter().filter(|r| r.status.is_success()).count();
        Some(ok as f64 / runs.len() as f64)
    }
}
