//! The agent runtime.
//!
//! An [`Agent`] never touches a facility directly. It is handed an
//! [`AgentContext`], and every facility it can reach is wrapped in a
//! [`Gated`]. `Gated`'s inner value is private and its only accessor takes
//! `&mut AgentContext`, so reaching a facility necessarily goes through the
//! capability check, the budget charge and the audit entry. An agent that
//! forgets to check its own permissions is therefore still contained, which is
//! the only kind of containment worth building.
//!
//! Runs are recorded whether they succeed or fail. A failed run that left no
//! trace is indistinguishable from one that never happened, and the difference
//! matters when reconstructing why the organisation missed something.

use crate::budget::{Budget, BudgetLedger, Spend};
use crate::capability::Capability;
use crate::finding::{AgentBrief, AgentFinding};
use crate::manifest::{AgentManifest, EscalationPolicy};
use qip_ai::language::{Completion, LanguageModel, ModelRequest};
use qip_core::error::{Error, Result};
use qip_core::ids::AgentRunId;
use qip_core::lineage::Lineage;
use qip_core::rng::Xoshiro256;
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A facility that can only be reached with a capability.
///
/// The inner value is unreachable except through [`Gated::get`], which needs
/// the run's context. Wrapping a facility is therefore sufficient to enforce
/// its permission, with no cooperation required from the agent.
#[derive(Debug)]
pub struct Gated<T> {
    value: T,
    required: Capability,
    /// What the facility is, for the audit entry and the denial message.
    label: &'static str,
}

impl<T> Gated<T> {
    pub fn new(value: T, required: Capability, label: &'static str) -> Self {
        Self {
            value,
            required,
            label,
        }
    }

    pub const fn required(&self) -> Capability {
        self.required
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    /// Borrow the facility, charging the run and recording the access.
    pub fn get<'a>(&'a self, ctx: &mut AgentContext) -> Result<&'a T> {
        ctx.authorise(self.required, self.label)?;
        Ok(&self.value)
    }

    /// Whether the context could reach this facility, without charging for it.
    ///
    /// For an agent that wants to adapt rather than fail — checking first is
    /// how an agent produces an honest partial answer instead of an error.
    pub fn is_available(&self, ctx: &AgentContext) -> bool {
        ctx.manifest.capabilities.contains(self.required)
    }
}

/// One recorded capability invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccessRecord {
    pub at: Timestamp,
    pub capability: Capability,
    /// The facility the capability was used to reach.
    pub facility: String,
    /// `false` when the access was refused.
    pub granted: bool,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    /// The agent stopped early but produced something usable.
    Escalated {
        policy: EscalationPolicy,
        to: Option<String>,
    },
    Failed {
        error_code: String,
        message: String,
    },
    /// Refused before the agent ran: an invalid or expired manifest.
    Refused {
        reason: String,
    },
}

impl RunStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// The permanent record of one agent run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRunRecord {
    pub run_id: AgentRunId,
    pub agent_id: String,
    /// Hash of the manifest the run executed under, so a later manifest change
    /// cannot silently rewrite what a past run was allowed to do.
    pub manifest_digest: String,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub status: RunStatus,
    pub spend: Spend,
    /// Fraction of the tightest budget line used.
    pub utilisation: f64,
    pub accesses: Vec<AccessRecord>,
    pub lineage: Lineage,
    /// The brief, so the run can be replayed exactly.
    pub brief: AgentBrief,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finding: Option<AgentFinding>,
}

impl AgentRunRecord {
    pub fn duration(&self) -> Duration {
        self.finished_at.since(self.started_at)
    }

    /// Capabilities the run actually exercised, deduplicated and ordered.
    pub fn capabilities_used(&self) -> Vec<Capability> {
        let mut used: Vec<Capability> = self
            .accesses
            .iter()
            .filter(|a| a.granted)
            .map(|a| a.capability)
            .collect();
        used.sort_unstable();
        used.dedup();
        used
    }

    /// Accesses the runtime refused. Non-empty means an agent tried to do
    /// something its manifest does not permit, which is worth alerting on even
    /// though it was blocked.
    pub fn denied_accesses(&self) -> Vec<&AccessRecord> {
        self.accesses.iter().filter(|a| !a.granted).collect()
    }
}

/// Everything an agent may reach during one run.
///
/// Created by [`AgentHost::run`] and destroyed when the run ends, so no handle
/// outlives the authorisation it was created under.
#[derive(Debug)]
pub struct AgentContext {
    manifest: AgentManifest,
    run_id: AgentRunId,
    started_at: Timestamp,
    now: Timestamp,
    ledger: BudgetLedger,
    accesses: Vec<AccessRecord>,
    lineage: Lineage,
    rng: Xoshiro256,
    language_model: Option<Arc<dyn LanguageModel>>,
    /// Notes the agent wants in the run record regardless of outcome.
    notes: Vec<String>,
}

impl AgentContext {
    pub fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    pub fn run_id(&self) -> &AgentRunId {
        &self.run_id
    }

    pub fn now(&self) -> Timestamp {
        self.now
    }

    pub fn lineage(&self) -> &Lineage {
        &self.lineage
    }

    pub fn spend(&self) -> Spend {
        self.ledger.spend()
    }

    pub fn budget(&self) -> &Budget {
        self.ledger.budget()
    }

    /// A deterministic RNG stream for this run.
    pub fn rng(&mut self) -> &mut Xoshiro256 {
        &mut self.rng
    }

    /// Whether the run can still do useful work — the loop condition for an
    /// agent that wants to stop cleanly rather than be cut off.
    pub fn has_headroom(&self) -> bool {
        self.ledger.has_headroom()
    }

    pub fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Advance the run's clock. Called by the host, and by a simulation that
    /// wants an agent to experience elapsed time deterministically.
    pub fn advance(&mut self, to: Timestamp) -> Result<()> {
        if to > self.now {
            self.now = to;
        }
        self.ledger.observe_elapsed(self.now.since(self.started_at))
    }

    /// The single enforcement point: check, charge, record.
    ///
    /// A refusal is recorded before the error is returned, so an agent probing
    /// for capabilities it does not have leaves evidence.
    pub fn authorise(&mut self, capability: Capability, facility: &str) -> Result<()> {
        let permitted = self.manifest.capabilities.contains(capability);
        self.accesses.push(AccessRecord {
            at: self.now,
            capability,
            facility: facility.to_string(),
            granted: permitted,
        });
        if !permitted {
            return Err(Error::denied(format!(
                "agent {} is not granted {capability} (needed for {facility})",
                self.manifest.id
            )));
        }
        self.ledger.charge_tool_call()
    }

    /// Complete a language-model request under the run's budget.
    ///
    /// Structured completions are validated and stripped of numbers by
    /// [`qip_ai::language::NumericGuard`] before they reach the agent, so a
    /// model cannot hand an agent a quantity even if it tries.
    pub fn complete(&mut self, request: &ModelRequest) -> Result<Completion> {
        self.authorise(Capability::CallLanguageModel, "language-model")?;
        let Some(model) = self.language_model.clone() else {
            return Err(Error::unavailable(format!(
                "agent {} has no language model configured",
                self.manifest.id
            )));
        };
        // Charge the estimate before the call so an over-large prompt is
        // refused rather than billed.
        let estimate = request.estimated_input_tokens();
        self.ledger.charge_language_model(estimate, 0)?;
        let completion = model.complete_structured(request, self.now)?;
        // Charge the difference between the estimate and what was actually
        // used. The output tokens were not knowable in advance.
        let actual = completion.total_tokens();
        let extra = actual.saturating_sub(estimate);
        if extra > 0 {
            self.ledger.charge_tokens_only(extra)?;
        }
        Ok(completion)
    }

    pub fn accesses(&self) -> &[AccessRecord] {
        &self.accesses
    }
}

/// An agent the organisation can run.
///
/// Deliberately object-safe and uniform: the coordinator dispatches to every
/// specialist through this one signature, which is what makes the roster a
/// roster rather than eighteen bespoke call sites.
pub trait Agent: Send + Sync + std::fmt::Debug {
    fn manifest(&self) -> &AgentManifest;

    /// Whether this agent can say anything useful about the brief.
    ///
    /// Answering `false` is respectable, and cheaper than a deferred run.
    fn accepts(&self, brief: &AgentBrief) -> bool {
        let _ = brief;
        true
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding>;
}

/// Runs agents under their manifests.
#[derive(Debug)]
pub struct AgentHost {
    language_model: Option<Arc<dyn LanguageModel>>,
    seed: u64,
}

impl AgentHost {
    pub fn new(seed: u64) -> Self {
        Self {
            language_model: None,
            seed,
        }
    }

    pub fn with_language_model(mut self, model: Arc<dyn LanguageModel>) -> Self {
        self.language_model = Some(model);
        self
    }

    /// Run one agent, producing a record whatever happens.
    ///
    /// `started_at` is the run's clock origin; the host does not read an
    /// ambient clock, so a replay produces the same record.
    pub fn run(
        &self,
        agent: &dyn Agent,
        brief: &AgentBrief,
        started_at: Timestamp,
        lineage: Lineage,
        run_id: AgentRunId,
    ) -> AgentRunRecord {
        let manifest = agent.manifest().clone();
        let digest = manifest_digest(&manifest);

        let refuse = |reason: String| AgentRunRecord {
            run_id: run_id.clone(),
            agent_id: manifest.id.clone(),
            manifest_digest: digest.clone(),
            started_at,
            finished_at: started_at,
            status: RunStatus::Refused { reason },
            spend: Spend::default(),
            utilisation: 0.0,
            accesses: Vec::new(),
            lineage: lineage.clone(),
            brief: brief.clone(),
            finding: None,
        };

        if let Err(error) = manifest.authorisation(started_at) {
            return refuse(error.message().to_string());
        }
        // An agent must not reason about a future it has not reached. This is
        // the look-ahead guard at the agent boundary; the data layer has its
        // own, and both are cheap.
        if brief.as_of > started_at {
            return refuse(format!(
                "brief as-of {} is after the run start {started_at}",
                brief.as_of
            ));
        }

        let mut ctx = AgentContext {
            manifest: manifest.clone(),
            run_id: run_id.clone(),
            started_at,
            now: started_at,
            ledger: BudgetLedger::new(manifest.budget, manifest.id.clone()),
            accesses: Vec::new(),
            lineage: lineage.clone(),
            rng: Xoshiro256::seeded(self.seed).fork(run_id.as_str()),
            language_model: self.language_model.clone(),
            notes: Vec::new(),
        };

        let outcome = agent.analyse(&mut ctx, brief);
        let finished_at = ctx.now;
        let spend = ctx.spend();
        let utilisation = ctx.ledger.utilisation();
        let accesses = ctx.accesses.clone();

        let (status, finding) = match outcome {
            Ok(finding) => match finding.validate() {
                Ok(()) => (RunStatus::Succeeded, Some(finding)),
                Err(error) => (
                    RunStatus::Failed {
                        error_code: error.code().to_string(),
                        message: format!("finding failed validation: {}", error.message()),
                    },
                    None,
                ),
            },
            // Escalate only when the error the agent actually returned *is*
            // the budget exhaustion (`Error::Guard`, raised solely by
            // `BudgetLedger::exceed`) and the ledger confirms it. Matching
            // on `ctx.ledger.exhausted().is_some()` alone was wrong: that
            // flag never clears once set, so an agent that hit a budget
            // limit, absorbed the refusal, and then failed for an unrelated
            // reason later in the same run — a numeric fault, a bad input —
            // had that real error discarded and replaced with "escalated:
            // ran out of budget", which is a false record of what happened.
            Err(error) if matches!(error, Error::Guard(_)) && ctx.ledger.exhausted().is_some() => (
                RunStatus::Escalated {
                    policy: manifest.escalation,
                    to: manifest.escalates_to.clone(),
                },
                None,
            ),
            Err(error) => (
                RunStatus::Failed {
                    error_code: error.code().to_string(),
                    message: error.message().to_string(),
                },
                None,
            ),
        };

        AgentRunRecord {
            run_id,
            agent_id: manifest.id,
            manifest_digest: digest,
            started_at,
            finished_at,
            status,
            spend,
            utilisation,
            accesses,
            lineage,
            brief: brief.clone(),
            finding,
        }
    }
}

/// Digest of the manifest a run executed under.
///
/// Serialised canonically through `serde_json`, whose object key order is the
/// declaration order of the struct, so the digest is stable across runs.
pub fn manifest_digest(manifest: &AgentManifest) -> String {
    let encoded = serde_json::to_vec(manifest).unwrap_or_default();
    qip_core::hash::sha256_hex(&encoded)
}
