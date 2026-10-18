//! What an external action must clear besides the plan itself, checked in
//! front of every adapter call [`crate::engine::execute`] makes.
//!
//! R12-AGENCY-1056..1069, 6091; R12-GOV-6123, 6124, 6127. Before this module
//! the conduct gate, the tool registry, the approval chain and the
//! justification check each existed and each was tested, and `execute` called
//! an adapter consulting none of them. A control with no caller reads as
//! protection and is not. Now a plan is sent only when, for every step:
//!
//! * the tool is registered, not revoked, on a channel it may use, and the
//!   *whole plan* fits its rate and budget ([`ToolRegistry::admits_all`]);
//! * the step's public face — the [`CommunicationDraft`] the world would see
//!   — passes all seven conduct checks, and the drafts together pass the
//!   coordinated-voices check;
//! * the step's message, by its words, was not already released under
//!   another identity in an earlier pass this guard admitted or resumed
//!   ([`Voice`]; R12-AGENCY-1064). Inside one pass every draft speaks as
//!   the goal's one identity, so the coordinated-voices check cannot fire
//!   there; across passes it is the only place manufactured consensus can
//!   be seen, and until this check nothing looked;
//! * the draft speaks as the tool's registered identity, which is the goal's
//!   acting identity: a step sent under any other name is a fabricated
//!   identity (R12-AGENCY-1063) however it is authorised;
//! * the justification names an evidence ground, not profit or price impact
//!   alone (GOV-6127);
//! * a goal approval and a plan approval exist, by people other than the
//!   acting identity, in order, and before the execution instant (GOV-6124).
//!
//! All of it runs before the first call, so a refusal sends nothing. In
//! shadow the same checks run, read-only, and a step they would refuse is
//! reported withheld rather than "cleared every gate".
//!
//! # A guard is a policy plus one pass at a time
//!
//! A composition root builds the guard once, from a [`GuardPolicy`] — the
//! conduct policy and the tool registrations an operator configures — and
//! keeps it for the life of the process, so the registry's rate, budget and
//! revocations hold across every pass it runs. Each pass then begins with
//! its own [`PassDecl`] (drafts, grounds, approvals, rate window) and ends
//! with it removed: an approval is for one pass, and a guard that carried
//! the last pass's approvals into the next would approve something nobody
//! looked at. With no pass begun every preflight is refused. A rate window
//! may not go backwards, and [`ActionGuard::resume`] re-applies the tool
//! decisions a previous process journaled, so a budget is not reset by
//! running the binary again.

use crate::conduct_brain::{
    CommunicationDraft, ConductGate, ConductPolicy, EvidenceResolver, HoldingsResolver, StepConduct,
};
use crate::goal::GoalSpec;
use crate::plan::Step;
use qip_contracts::action::{
    ExecutionRecord, GoalApproval, Ground, Method, PlanApproval, ToolAudit, ToolRegistration,
    ToolRegistry, check_justification, words,
};
use qip_contracts::action_intent::{
    ActingIdentity as IntentIdentity, ActionIntent, ActionIntentFields,
    Disclosure as IntentDisclosure, PolicyDecision, Verdict,
};
use qip_core::decimal::SCALE;
use qip_core::{Decimal, Error, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

/// How many released voices a guard remembers across passes. Bounded, like
/// every working set here; the durable record is the journal, which
/// [`ActionGuard::resume_voices`] reads back on start.
pub const VOICE_RETENTION: usize = 1024;

/// One message as it was released: which tool said it, as whom, and the
/// fingerprint of its words. Kept so a later pass cannot say the same thing
/// as somebody else (R12-AGENCY-1064).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Voice {
    pub tool: String,
    pub identity: String,
    /// [`voice_fingerprint`] of the draft's content.
    pub fingerprint: String,
}

/// The fingerprint of a message: a digest of its words, lower-cased and
/// stripped of punctuation, so a changed byline or a reflowed line does not
/// make one message into two. The same normalisation the conduct gate's
/// coordinated-voices check compares by.
pub fn voice_fingerprint(content: &str) -> String {
    qip_core::sha256_hex(words(content).join(" ").as_bytes())
}

/// Who approved the goal and the plan, and when the act stage runs. Times
/// come from the caller, never a clock read here, so a pass replays.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecl {
    pub goal_approver: String,
    pub goal_approved_at: Timestamp,
    pub plan_approver: String,
    pub plan_approved_at: Timestamp,
    pub execute_at: Timestamp,
}

/// The guard as a document, carried by a request: the standing policy and
/// one pass's declarations together.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardDecl {
    pub conduct: ConductPolicy,
    pub tools: Vec<ToolRegistration>,
    /// Per tool, the public face of every step that tool takes.
    pub communications: BTreeMap<String, CommunicationDraft>,
    pub grounds: Vec<Ground>,
    pub approvals: ApprovalDecl,
    /// The caller's rate window id for this pass.
    pub rate_window: u64,
}

/// The standing half of a guard: what an operator configures once and a
/// composition root loads at start-up — the conduct policy and the tool
/// registrations. It holds no approvals, because an approval is for one
/// pass and a policy file that carried one would approve every pass the
/// process ever ran.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardPolicy {
    pub conduct: ConductPolicy,
    pub tools: Vec<ToolRegistration>,
}

/// The per-pass half: what this pass's steps would say, why, who approved
/// it, and which rate window it spends in.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassDecl {
    /// Per tool, the public face of every step that tool takes.
    pub communications: BTreeMap<String, CommunicationDraft>,
    pub grounds: Vec<Ground>,
    pub approvals: ApprovalDecl,
    /// The caller's rate window id for this pass.
    pub rate_window: u64,
}

/// The three GOV-6124 records of one pass, each attributable alone. The
/// execution record exists only once steps were sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalChain {
    pub goal: GoalApproval,
    pub plan: PlanApproval,
    pub execution: Option<ExecutionRecord>,
}

/// A step cleared to go: what the registry will be charged with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cleared {
    pub(crate) step: Step,
    pub(crate) channel: String,
    pub(crate) cost_units: u64,
    /// The voice this step would release, remembered once it is admitted.
    pub(crate) voice: Voice,
    /// The §27 record of this step as an action outside the platform
    /// (R12-CONTRACT-7082): who acts, through which channel, saying what,
    /// on what provenance and disclosure, admitted by which policies.
    pub(crate) intent: ActionIntent,
}

/// The payload an [`ActionIntent`] carries for one step: what the channel
/// would receive, so the digest the intent holds is a digest of the action
/// that was reviewed and not of a summary of it.
#[derive(Serialize)]
struct StepPayload<'a> {
    tool: &'a str,
    variable: &'a str,
    exposure: Decimal,
    channel: &'a str,
    audience: &'a str,
    content: &'a str,
}

/// The policies an intent records as having admitted it, in the order the
/// preflight consults them. An intent leaves the preflight only once every
/// one has passed: a refused step leaves a refusal, never an intent.
const INTENT_POLICIES: [&str; 4] = [
    "justification",
    "approval_chain",
    "conduct_gate",
    "tool_registry",
];

/// The built guard. Holds the registry, so a caller that keeps one guard
/// across passes keeps the rate, budget and revocation state across them.
#[derive(Debug)]
pub struct ActionGuard {
    gate: ConductGate,
    registry: ToolRegistry,
    /// The current pass's declarations; `None` between passes, when every
    /// preflight is refused.
    pass: Option<PassDecl>,
    /// The highest rate window this guard has spent or begun in. A pass may
    /// not begin below it: the registry resets a tool's count whenever the
    /// window changes, so a pass that named an old window would get a fresh
    /// allowance by going back in time.
    floor: u64,
    /// Every message admitted through [`Self::authorise`] or resumed from
    /// the journal, newest last; at most [`VOICE_RETENTION`].
    voices: VecDeque<Voice>,
}

impl ActionGuard {
    /// Build the gate and register every tool, refusing whatever either
    /// refuses, and begin the pass the document declares.
    pub fn from_decl(decl: GuardDecl) -> Result<Self, Error> {
        let mut guard = Self::from_policy(GuardPolicy {
            conduct: decl.conduct,
            tools: decl.tools,
        })?;
        guard.begin_pass(PassDecl {
            communications: decl.communications,
            grounds: decl.grounds,
            approvals: decl.approvals,
            rate_window: decl.rate_window,
        })?;
        Ok(guard)
    }

    /// Build the gate and register every tool, with no pass begun: every
    /// preflight is refused until [`Self::begin_pass`]. A policy with no
    /// tool is built, and refuses every step as unregistered; a composition
    /// root that wants to refuse such a configuration outright says so where
    /// it loads it.
    pub fn from_policy(policy: GuardPolicy) -> Result<Self, Error> {
        let gate = ConductGate::from_policy(policy.conduct)?;
        let mut registry = ToolRegistry::new();
        for tool in policy.tools {
            registry.register(tool)?;
        }
        Ok(Self {
            gate,
            registry,
            pass: None,
            floor: 0,
            voices: VecDeque::new(),
        })
    }

    /// Attach the platform's record of evidence to the conduct gate
    /// (R12-AGENCY-1056 / 1057): every citation of every draft is then
    /// resolved against it, and one that resolves to nothing, to another
    /// source or to another statement withholds the plan.
    pub fn with_evidence(mut self, resolver: Arc<dyn EvidenceResolver>) -> Self {
        self.gate = self.gate.with_resolver(resolver);
        self
    }

    /// Attach what the platform holds to the conduct gate (R12-AGENCY-1066):
    /// a draft naming a held instrument then needs a `position` disclosure.
    pub fn with_holdings(mut self, holdings: Arc<dyn HoldingsResolver>) -> Self {
        self.gate = self.gate.with_holdings(holdings);
        self
    }

    /// Re-apply the voices a previous process released, so a message said
    /// under one identity before a restart is still a message said under
    /// it. Refused once a pass has begun, for the same reason as
    /// [`Self::resume`].
    pub fn resume_voices(&mut self, voices: Vec<Voice>) -> Result<(), Error> {
        if self.pass.is_some() {
            return Err(Error::guard(
                "resume the guard's voices before the first pass begins, not during one",
            ));
        }
        for voice in voices {
            self.remember(voice);
        }
        Ok(())
    }

    fn remember(&mut self, voice: Voice) {
        if self.voices.len() >= VOICE_RETENTION {
            self.voices.pop_front();
        }
        self.voices.push_back(voice);
    }

    /// Every voice this guard remembers, oldest first.
    pub fn voices(&self) -> impl Iterator<Item = &Voice> {
        self.voices.iter()
    }

    /// Re-apply the tool decisions a previous process journaled, so the rate
    /// and budget they spent are spent here too and the rate-window floor is
    /// where it was left. Refused once a pass has begun: resuming under a
    /// pass would charge history to it.
    pub fn resume(&mut self, decisions: Vec<ToolAudit>) -> Result<(), Error> {
        if self.pass.is_some() {
            return Err(Error::guard(
                "resume the guard before the first pass begins, not during one",
            ));
        }
        for decision in decisions {
            self.floor = self.floor.max(decision.window);
            self.registry.resume(decision)?;
        }
        Ok(())
    }

    /// Begin a pass on its own declarations. Refuses a rate window below the
    /// highest already spent or begun.
    pub fn begin_pass(&mut self, pass: PassDecl) -> Result<(), Error> {
        if pass.rate_window < self.floor {
            return Err(Error::denied(format!(
                "the pass names rate window {}, below window {} already spent; a window is not \
                 reopened, because reopening one resets every tool's count",
                pass.rate_window, self.floor
            )));
        }
        self.floor = pass.rate_window;
        self.pass = Some(pass);
        Ok(())
    }

    /// End the current pass. Its approvals, drafts and grounds go with it:
    /// an approval for one pass is not an approval for the next.
    pub fn end_pass(&mut self) {
        self.pass = None;
    }

    /// Whether a pass is begun.
    pub fn in_pass(&self) -> bool {
        self.pass.is_some()
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// The revocation path: a revoked tool's next step is refused, and the
    /// refusal is audited if it reaches the registry's `authorise`.
    pub fn revoke(&mut self, tool: &str) -> Result<(), Error> {
        self.registry.revoke(tool)
    }

    /// Restore a revoked tool on `operator`'s authority
    /// ([`ToolRegistry::reauthorise`]): the only way back from
    /// [`Self::revoke`].
    pub fn reauthorise(&mut self, tool: &str, operator: &str) -> Result<(), Error> {
        self.registry.reauthorise(tool, operator)
    }

    /// Move to a new rate window and execution instant, keeping the current
    /// pass's drafts and grounds. With no pass begun, begins one with none,
    /// which every preflight refuses on its grounds.
    pub fn advance(&mut self, rate_window: u64, approvals: ApprovalDecl) {
        self.floor = self.floor.max(rate_window);
        match self.pass.as_mut() {
            Some(pass) => {
                pass.rate_window = rate_window;
                pass.approvals = approvals;
            }
            None => {
                self.pass = Some(PassDecl {
                    communications: BTreeMap::new(),
                    grounds: Vec::new(),
                    approvals,
                    rate_window,
                });
            }
        }
    }

    pub fn audit(&self) -> &[ToolAudit] {
        self.registry.audit()
    }

    fn current(&self) -> Result<&PassDecl, Error> {
        self.pass.as_ref().ok_or_else(|| {
            Error::denied(
                "no pass is begun on this guard: approvals, drafts and grounds are declared per \
                 pass, and none has been",
            )
        })
    }

    /// Every check, read-only. `priced` is each step beside its cost and
    /// the method its affordance edge declares. On success, the goal and plan approvals and the
    /// steps as the registry will be charged for them.
    pub(crate) fn preflight(
        &self,
        goal: &GoalSpec,
        priced: &[(Step, Decimal, Method)],
    ) -> Result<(ApprovalChain, Vec<Cleared>), Error> {
        let pass = self.current()?;
        check_justification(&pass.grounds)?;
        let steps: Vec<Step> = priced.iter().map(|(step, _, _)| step.clone()).collect();
        let chain = self.approve(goal, &steps)?;

        // A step's checks, then its intent: the intent is built only once
        // every check has passed, so a refusal names the check that refused
        // rather than whatever the §27 record would have refused first.
        struct Staged {
            step: Step,
            channel: String,
            cost_units: u64,
            voice: Voice,
        }
        let mut cleared: Vec<Staged> = Vec::with_capacity(priced.len());
        let mut drafts: BTreeMap<&str, &CommunicationDraft> = BTreeMap::new();
        let mut faces: Vec<StepConduct<'_>> = Vec::with_capacity(priced.len());
        for (step, cost, method) in priced {
            let registration = self.registry.registration(&step.tool).ok_or_else(|| {
                Error::denied(format!(
                    "tool `{}` is not registered; an action acts only through a registered tool",
                    step.tool
                ))
            })?;
            let draft = pass.communications.get(&step.tool).ok_or_else(|| {
                Error::denied(format!(
                    "step `{}` has no drafted public face; an external action nobody drafted \
                     cannot be conduct-checked, so it is not sent",
                    step.tool
                ))
            })?;
            if draft.sender_identity != registration.acting_identity
                || registration.acting_identity != goal.acting_identity
            {
                return Err(Error::denied(format!(
                    "step `{}` would speak as `{}`, the tool is registered to `{}` and the goal \
                     acts as `{}`; an action under any name but the goal's is a fabricated identity",
                    step.tool,
                    draft.sender_identity,
                    registration.acting_identity,
                    goal.acting_identity
                )));
            }
            cleared.push(Staged {
                step: step.clone(),
                channel: draft.channel.clone(),
                cost_units: budget_units(&step.tool, *cost)?,
                voice: Voice {
                    tool: step.tool.clone(),
                    identity: draft.sender_identity.clone(),
                    fingerprint: voice_fingerprint(&draft.content),
                },
            });
            drafts.insert(step.tool.as_str(), draft);
            faces.push(StepConduct {
                tool: step.tool.as_str(),
                method: *method,
                draft,
            });
        }

        let released: Vec<&CommunicationDraft> = drafts.values().copied().collect();
        let mut failures = self.gate.campaign_failures(&released);
        if let Some(step) = self.gate.step_failure(&faces) {
            failures.push(("*".to_string(), step));
        }
        if !failures.is_empty() {
            let reasons: Vec<String> = failures
                .iter()
                .map(|(id, check)| {
                    format!(
                        "{id}: {} ({})",
                        check.check_name,
                        check.reason.as_deref().unwrap_or("failed")
                    )
                })
                .collect();
            return Err(Error::denied(format!(
                "the conduct gate refuses: {}",
                reasons.join("; ")
            )));
        }
        // R12-AGENCY-1064: the same words already out under another name.
        for c in &cleared {
            if let Some(prior) = self
                .voices
                .iter()
                .find(|v| v.fingerprint == c.voice.fingerprint && v.identity != c.voice.identity)
            {
                return Err(Error::denied(format!(
                    "fake consensus across passes: step `{}` would release as `{}` the same \
                     message `{}` already released as `{}` in an earlier pass; one message under \
                     two identities is one voice posing as several",
                    c.step.tool, c.voice.identity, prior.tool, prior.identity
                )));
            }
        }
        let actions: Vec<(&str, &str, u64)> = cleared
            .iter()
            .map(|c| (c.step.tool.as_str(), c.channel.as_str(), c.cost_units))
            .collect();
        self.registry.admits_all(pass.rate_window, &actions)?;
        let mut admitted = Vec::with_capacity(cleared.len());
        for (index, (staged, face)) in cleared.into_iter().zip(&faces).enumerate() {
            admitted.push(Cleared {
                intent: intent(goal, &chain, pass, index, &staged.step, face.draft)?,
                step: staged.step,
                channel: staged.channel,
                cost_units: staged.cost_units,
                voice: staged.voice,
            });
        }
        Ok((chain, admitted))
    }

    /// The execution record, written before the first call so a refused or
    /// failed call still has an attributable execution behind it.
    pub(crate) fn execution(
        &self,
        chain: &mut ApprovalChain,
        executor: &str,
        steps: &[Step],
    ) -> Result<(), Error> {
        chain.execution = Some(ExecutionRecord::execute(
            &chain.plan,
            executor,
            self.current()?.approvals.execute_at,
            &subject(steps),
        )?);
        Ok(())
    }

    /// The mutating, audited admission of one step, immediately before its
    /// call.
    pub(crate) fn authorise(&mut self, cleared: &Cleared) -> Result<(), Error> {
        let (window, at) = {
            let pass = self.current()?;
            (pass.rate_window, pass.approvals.execute_at)
        };
        self.registry.authorise(
            &cleared.step.tool,
            &cleared.channel,
            window,
            cleared.cost_units,
            at,
        )?;
        self.remember(cleared.voice.clone());
        Ok(())
    }

    fn approve(&self, goal: &GoalSpec, steps: &[Step]) -> Result<ApprovalChain, Error> {
        let a = &self.current()?.approvals;
        for (role, who) in [("goal", &a.goal_approver), ("plan", &a.plan_approver)] {
            if who == &goal.acting_identity {
                return Err(Error::denied(format!(
                    "the {role} approver is the acting identity `{who}`; an identity approving \
                     its own action is one record wearing two names"
                )));
            }
        }
        if a.execute_at < a.plan_approved_at {
            return Err(Error::invalid(
                "execution would precede the plan approval it relies on; check the clock or the order",
            ));
        }
        let goal_approval =
            GoalApproval::approve(&a.goal_approver, a.goal_approved_at, &goal.target_state)?;
        let plan = PlanApproval::approve(
            &goal_approval,
            &a.plan_approver,
            a.plan_approved_at,
            &subject(steps),
        )?;
        Ok(ApprovalChain {
            goal: goal_approval,
            plan,
            execution: None,
        })
    }
}

/// The §27 intent for the `index`th step of a pass, built only once the
/// step has a drafted face and an approval chain. Validated by
/// [`ActionIntent::new`], so a payload whose digest does not match, a
/// blank name or a missing provenance is refused here rather than carried.
fn intent(
    goal: &GoalSpec,
    chain: &ApprovalChain,
    pass: &PassDecl,
    index: usize,
    step: &Step,
    draft: &CommunicationDraft,
) -> Result<ActionIntent, Error> {
    let payload = serde_json::to_string(&StepPayload {
        tool: &step.tool,
        variable: &step.variable,
        exposure: step.exposure,
        channel: &draft.channel,
        audience: &draft.target_audience,
        content: &draft.content,
    })
    .map_err(|e| Error::invalid(format!("step `{}` payload does not encode: {e}", step.tool)))?;
    let payload_digest = ActionIntentFields::digest_payload(&payload);
    let at = pass.approvals.execute_at;
    let horizon = i64::try_from(goal.time_horizon_secs)
        .map_err(|_| Error::numeric("the goal's horizon does not fit a timestamp"))?;
    let mut provenance: Vec<String> = draft
        .evidence_citations
        .iter()
        .map(|c| c.evidence_id.clone())
        .collect();
    provenance.push(chain.goal.id.clone());
    provenance.push(chain.plan.id.clone());
    ActionIntent::new(ActionIntentFields {
        schema_version: 1,
        intent_id: format!("intent-{}-{index}", chain.plan.id),
        identity: IntentIdentity {
            actor: goal.acting_identity.clone(),
            on_behalf_of: chain.goal.approver.clone(),
        },
        channel: draft.channel.clone(),
        idempotency_key: qip_core::sha256_hex(
            format!("{}|{index}|{payload_digest}", chain.plan.id).as_bytes(),
        ),
        payload,
        payload_digest,
        provenance,
        disclosure: IntentDisclosure::Required {
            statements: draft.disclosures.iter().map(|d| d.text.clone()).collect(),
        },
        policy_decisions: INTENT_POLICIES
            .iter()
            .map(|policy| PolicyDecision {
                policy: (*policy).to_string(),
                verdict: Verdict::Allowed,
                decided_at: at,
            })
            .collect(),
        created_at: at,
        expires_at: Timestamp::from_secs(at.as_secs().saturating_add(horizon)),
        audit_lineage: vec![chain.goal.id.clone(), chain.plan.id.clone()],
    })
}

/// What a plan approval and an execution record are about: the tools, in
/// order, so the two records name the same thing.
fn subject(steps: &[Step]) -> String {
    let tools: Vec<&str> = steps.iter().map(|s| s.tool.as_str()).collect();
    format!("steps: {}", tools.join(" -> "))
}

/// The graph states a tool's cost as a `Decimal`; the registry's budget is
/// whole units. A fractional cost is refused rather than rounded, because
/// rounding down is a budget that under-charges every call.
fn budget_units(tool: &str, cost: Decimal) -> Result<u64, Error> {
    let raw = cost.raw();
    if raw < 0 || raw % SCALE != 0 {
        return Err(Error::invalid(format!(
            "tool `{tool}` costs {cost}; a registry budget is charged in whole non-negative units"
        )));
    }
    u64::try_from(raw / SCALE)
        .map_err(|_| Error::numeric(format!("tool `{tool}` costs more units than fit a u64")))
}
