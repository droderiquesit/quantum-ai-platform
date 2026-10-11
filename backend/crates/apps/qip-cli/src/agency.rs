//! `qip agency shadow --policy <path> --journal <path> --request <path>...`:
//! passes of the agency loop, under a conduct policy the operator
//! configured, journaled on the hash-chained log.
//!
//! An operator hands it the agency's standing policy (the conduct rules, the
//! registered tools, the legal-eligibility register and the action policies)
//! and one request document per pass (a goal, the affordance graph, the
//! acting identity, the evidence on each lever, the proposals or a
//! structural model to generate them from, and the pass's drafts, grounds and
//! approvals). It is told, per pass, what the engine did and why.
//!
//! # What this binary refuses that it used to run
//!
//! Until R12-AGENCY-1056..1069's round this command ran each request with no
//! conduct policy and no guard unless the request happened to carry one, and
//! returned the approval records in a report it printed and dropped. So the
//! conduct gate, the tool registry, the approval chain and the justification
//! check were a library this binary *could* call and usually did not. Now:
//!
//! * **No policy, no pass.** The policy comes from `--policy` or
//!   `QIP_AGENCY_POLICY_FILE` (read in `main`, nowhere else); without one the
//!   command refuses before reading a request, as it does a policy that
//!   registers no tool. A request that brings its own `guards` is refused:
//!   the guard is the operator's, not the requester's.
//! * **One guard across every pass.** Built once from the policy, kept for
//!   the invocation, so rate, budget and revocation hold across passes; each
//!   pass begins with its own declarations and ends with them gone. On start
//!   the guard re-applies every tool decision already in the journal, so
//!   running the binary again does not refill a budget.
//! * **Every pass is journaled.** `--journal` (or `QIP_AGENCY_JOURNAL`) is
//!   required, opened as the platform's hash-chained [`EventLog`], verified
//!   before anything is resumed from it, and each pass's approvals, tool
//!   decisions and verdict are appended through [`qip_agency::journal`]
//!   before the pass is reported.
//! * **Eligibility is asked, not declared.** A pass's candidates are legally
//!   eligible only if every subject it touches is eligible in every
//!   jurisdiction the goal names, by the policy's register.
//! * **Widening is evidenced.** A pass may ask to widen one action class's
//!   policy; it is granted only when an estimator run here on the supplied
//!   data reproduces the attribution's identification and effect, and the
//!   approver holds the authority (`qip_agency::engine::widen`). Granted or
//!   refused, it is journaled.
//!
//! # What it still cannot do
//!
//! Act. External action stays refused (ADR 0114). The adapter this binary
//! holds refuses every call, so a pass that clears every gate at
//! narrow-reversible authority writes its execution record, is charged by
//! the registry, and is then refused at the adapter — and that attempt is
//! what is journaled. If the engine ever reports a step as executed this
//! command refuses to print it.
//!
//! The exit code is the verdict, as for `registrations` and `replay`: zero
//! when every pass cleared every gate, [`WITHHELD`] when any pass was
//! declined or refused, and one when a document could not be read at all.

use qip_agency::attribution::EffectAttribution;
use qip_agency::autonomy::ActionPolicy;
use qip_agency::engine::{Adapter, Governance, Outcome, Report, Request, run_governed, widen};
use qip_agency::estimate::EstimateInput;
use qip_agency::guard::{ActionGuard, GuardPolicy};
use qip_agency::journal::{self, PassEntry, Provenance, WideningOutcome};
use qip_agency::plan::{ActingIdentity, Step};
use qip_compliance::eligibility::{EligibilityRecord, EligibilityRegister};
use qip_contracts::action::Method;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Decimal, SystemClock, Timestamp};
use qip_events::EventLog;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Every pass cleared every gate.
pub const ADMITTED: u8 = 0;

/// Some pass was declined: no action, abstain, gather evidence, propose an
/// experiment, a stage that stopped it, or an attempt the adapter refused.
pub const WITHHELD: u8 = 3;

/// The variable naming the policy file when `--policy` is absent.
pub const POLICY_VARIABLE: &str = "QIP_AGENCY_POLICY_FILE";

/// The variable naming the journal when `--journal` is absent.
pub const JOURNAL_VARIABLE: &str = "QIP_AGENCY_JOURNAL";

const USAGE: &str = "usage: qip agency shadow [--policy <path>] [--journal <path>] --request <path> \
                     [--request <path> ...]";

/// The adapter this binary has: none (ADR 0114).
struct Unwired;

impl Adapter for Unwired {
    fn call(&mut self, step: &Step) -> Result<()> {
        Err(Error::denied(format!(
            "no action adapter is wired into this binary (ADR 0114); `{}` was not sent",
            step.tool
        )))
    }
}

/// One action class's policy as configured: what it predicts. Every policy
/// starts at shadow in every process; authority is never configured.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPolicyDecl {
    pub class: Method,
    pub uplift_low: Decimal,
    pub uplift_high: Decimal,
    pub predicted_side_effects: BTreeSet<String>,
}

/// The agency's standing configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgencyPolicy {
    pub guard: GuardPolicy,
    /// Legal eligibility of venues, products, strategies, entities and
    /// jurisdictions. Required; an empty list admits nothing a pass touches.
    pub eligibility: Vec<EligibilityRecord>,
    #[serde(default)]
    pub action_policies: Vec<ActionPolicyDecl>,
    /// Registered tools the operator has withdrawn (R12-GOV-6123). Applied
    /// to the guard after it resumes from the journal, so every pass of this
    /// invocation finds the tool revoked and is refused at the registry with
    /// the refusal journaled. A name that is not a registered tool is
    /// refused rather than ignored: a revocation that withdrew nothing is a
    /// typo that leaves the tool live. Each pass's journal entry carries the
    /// policy document's hash, so which revocations stood is on the record.
    #[serde(default)]
    pub revoked_tools: Vec<String>,
}

/// A request to widen one class's policy, with the data to check it on.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WideningDecl {
    pub class: Method,
    pub approver: ActingIdentity,
    pub attribution: EffectAttribution,
    pub evidence: EstimateInput,
}

/// One pass's document: the engine's request, and what this binary adds.
#[derive(Debug, Clone, Deserialize)]
pub struct PassDocument {
    #[serde(flatten)]
    pub request: Request,
    /// Ask to widen a class's policy before this pass runs.
    #[serde(default)]
    pub widening: Option<WideningDecl>,
    /// The action class this pass acts under, if its policy holds the
    /// authority; otherwise the pass is shadow.
    #[serde(default)]
    pub act_as: Option<Method>,
}

/// The configuration `main` resolved: where the policy and journal are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub policy: String,
    pub journal: String,
    pub requests: Vec<String>,
}

/// Resolve the flags, falling back to the two variables `main` reads.
pub fn paths(arguments: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<Paths> {
    let mut policy = None;
    let mut journal = None;
    let mut requests = Vec::new();
    let mut rest = arguments.iter();
    while let Some(flag) = rest.next() {
        let value = rest
            .next()
            .ok_or_else(|| Error::invalid(format!("{flag} needs a value; {USAGE}")))?;
        match flag.as_str() {
            "--policy" => policy = Some(value.clone()),
            "--journal" => journal = Some(value.clone()),
            "--request" => requests.push(value.clone()),
            other => return Err(Error::invalid(format!("unknown flag {other}; {USAGE}"))),
        }
    }
    let policy = policy.or_else(|| env(POLICY_VARIABLE)).ok_or_else(|| {
        Error::invalid(format!(
            "no conduct policy is configured: pass --policy or set {POLICY_VARIABLE}. The agency \
             does not run a pass that no conduct gate, tool registry or eligibility register \
             stands in front of"
        ))
    })?;
    let journal = journal.or_else(|| env(JOURNAL_VARIABLE)).ok_or_else(|| {
        Error::invalid(format!(
            "no journal is configured: pass --journal or set {JOURNAL_VARIABLE}. A pass's \
             approvals and decisions are recorded on the hash-chained log or the pass is not run"
        ))
    })?;
    if requests.is_empty() {
        return Err(Error::invalid(USAGE));
    }
    Ok(Paths {
        policy,
        journal,
        requests,
    })
}

/// What this build is, for every record it journals.
fn code_version() -> String {
    match option_env!("QIP_SOURCE_COMMIT") {
        Some(commit) => format!("qip-cli {} ({commit})", env!("CARGO_PKG_VERSION")),
        None => format!("qip-cli {}", env!("CARGO_PKG_VERSION")),
    }
}

/// The session one invocation holds across its passes.
#[derive(Debug)]
pub struct Session {
    guard: ActionGuard,
    register: EligibilityRegister,
    policies: BTreeMap<Method, ActionPolicy>,
    provenance: Provenance,
    next_pass: u64,
}

impl Session {
    /// Build the guard, the register and the policies from the policy
    /// document, and resume the guard from the journal. Refused: a policy
    /// that does not parse or validate, one registering no tool, two
    /// policies for one class, and a journal whose chain does not verify.
    pub fn open(policy_document: &str, journal: &EventLog) -> Result<Self> {
        let policy: AgencyPolicy = serde_json::from_str(policy_document)
            .map_err(|error| Error::invalid(format!("the agency policy is refused: {error}")))?;
        if policy.guard.tools.is_empty() {
            return Err(Error::invalid(
                "the agency policy registers no tool; every step would be refused as \
                 unregistered, so declare the tools the agency may use",
            ));
        }
        let mut guard = ActionGuard::from_policy(policy.guard)?;
        let register = EligibilityRegister::from_records(policy.eligibility)?;
        let mut policies = BTreeMap::new();
        for decl in policy.action_policies {
            let built = ActionPolicy::new(
                decl.class,
                decl.uplift_low,
                decl.uplift_high,
                decl.predicted_side_effects,
            )?;
            if policies.insert(decl.class, built).is_some() {
                return Err(Error::invalid(format!(
                    "two action policies are declared for {:?}; declare one",
                    decl.class
                )));
            }
        }
        let resumed = journal::resume(journal)?;
        guard.resume(resumed.decisions)?;
        for tool in &policy.revoked_tools {
            guard.revoke(tool).map_err(|error| {
                Error::invalid(format!(
                    "the agency policy revokes `{tool}`, which it does not register; a \
                     revocation that withdraws nothing leaves the tool it meant live: {}",
                    error.message()
                ))
            })?;
        }
        Ok(Self {
            guard,
            register,
            policies,
            provenance: Provenance {
                code_version: code_version(),
                policy_version: format!(
                    "sha256:{}",
                    qip_core::sha256_hex(policy_document.as_bytes())
                ),
            },
            next_pass: resumed.next_pass,
        })
    }

    /// The guard this session holds.
    pub fn guard(&self) -> &ActionGuard {
        &self.guard
    }

    fn widen(&mut self, decl: WideningDecl) -> WideningOutcome {
        let refused = |reason: String| WideningOutcome::Refused {
            class: decl.class,
            approver: decl.approver.name.clone(),
            reason,
        };
        let Some(policy) = self.policies.get_mut(&decl.class) else {
            return refused(format!(
                "no action policy is configured for {:?}; there is nothing to widen",
                decl.class
            ));
        };
        let estimate = match decl.evidence.estimate() {
            Ok(estimate) => estimate,
            Err(error) => return refused(error.message().to_string()),
        };
        if let Err(error) = estimate.supports(&decl.attribution) {
            return refused(error.message().to_string());
        }
        match widen(policy, &decl.attribution, &decl.approver) {
            Ok(widening) => WideningOutcome::Granted {
                class: widening.class,
                goal_id: widening.goal_id,
                intervention_id: widening.intervention_id,
                action_id: widening.action_id,
                approver: widening.approver,
                estimator: estimate.kind(),
            },
            Err(error) => refused(error.message().to_string()),
        }
    }

    /// Run one pass, journal it, and return the lines to print and its
    /// verdict.
    pub fn pass(
        &mut self,
        document: &str,
        journal: &mut EventLog,
        now: Timestamp,
    ) -> Result<(Vec<String>, u8)> {
        let refused_document = |error: serde_json::Error| {
            Error::invalid(format!("the request document is refused: {error}"))
        };
        // Refused on the key, not on a `guards` that happens to parse. Checked
        // only after the typed parse, a smuggled guard missing one per-pass
        // field was refused as "missing field `communications`" — an answer
        // that tells the caller how to complete the smuggled guard rather
        // than that a request may not bring one at all.
        let raw: serde_json::Value = serde_json::from_str(document).map_err(refused_document)?;
        if raw.get("guards").is_some() {
            return Err(Error::invalid(
                "the request carries its own `guards`; this binary's guard is the configured \
                 policy, and a request does not bring its own",
            ));
        }
        // A present key — `null` included — has been refused above, so the
        // typed `request.guards` is `None` from here on.
        let document: PassDocument = serde_json::from_value(raw).map_err(refused_document)?;
        if document.request.pass.is_none() {
            return Err(Error::invalid(
                "the request declares no `pass`: its drafts, grounds, approvals and rate window \
                 are what the guard checks, and a pass without them is not run",
            ));
        }
        let number = self.next_pass;
        let widening = document.widening.map(|decl| self.widen(decl));
        let policy = document
            .act_as
            .and_then(|class| self.policies.get(&class))
            .filter(|p| p.authority() == qip_agency::autonomy::Authority::NarrowReversible);
        let goal_target = document.request.goal.target_state.clone();
        let acting = document.request.goal.acting_identity.clone();
        let audit_from = self.guard.audit().len();
        let report = run_governed(
            document.request,
            Governance {
                policy,
                guard: Some(&mut self.guard),
                eligibility: Some((&self.register, now)),
            },
            &mut Unwired,
        )?;
        journal::record_pass(
            journal,
            PassEntry {
                pass: number,
                provenance: &self.provenance,
                report: &report,
                widening: widening.as_ref(),
                audit_from,
                goal_target: &goal_target,
                acting_identity: &acting,
                at: now,
            },
        )?;
        self.next_pass += 1;
        render(number, &report, widening.as_ref())
    }
}

fn render(
    number: u64,
    report: &Report,
    widening: Option<&WideningOutcome>,
) -> Result<(Vec<String>, u8)> {
    let mut lines = vec![
        format!("agency pass {number}: no adapter is wired, so nothing below was sent"),
        format!("stages passed: {:?}", report.passed),
    ];
    match widening {
        Some(WideningOutcome::Granted {
            class, estimator, ..
        }) => lines.push(format!("widened {class:?} on a {estimator:?} estimate")),
        Some(WideningOutcome::Refused { class, reason, .. }) => {
            lines.push(format!("widening {class:?} refused: {reason}"))
        }
        None => {}
    }
    if let Some(stage) = report.failed {
        lines.push(format!("stage failed: {stage:?}"));
    }
    lines.push(format!("levers: {}", report.levers.join(", ")));
    for refusal in &report.eligibility {
        lines.push(format!("ineligible: {refusal}"));
    }
    for simulated in &report.simulations {
        for result in &simulated.results {
            lines.push(format!(
                "simulated {} under {} / {}: effect {}",
                simulated.label, result.model, result.scenario, result.effect
            ));
        }
    }
    for (index, refusal) in &report.refused {
        let label = report
            .candidates
            .get(*index)
            .cloned()
            .unwrap_or_else(|| format!("proposal {}", index + 1));
        lines.push(format!("refused {label}: {}", refusal.message()));
    }
    if let Some(chain) = &report.approvals {
        lines.push(format!(
            "approvals journaled: goal by {}, plan by {}{}",
            chain.goal.approver,
            chain.plan.approver,
            chain
                .execution
                .as_ref()
                .map(|e| format!(", execution by {}", e.executor))
                .unwrap_or_default()
        ));
    }
    let code = match &report.outcome {
        Outcome::Shadowed(steps) => {
            lines.push("outcome: shadowed; these steps cleared every gate".to_string());
            for step in steps {
                lines.push(format!(
                    "  {} on {}, exposure {}",
                    step.tool, step.variable, step.exposure
                ));
            }
            ADMITTED
        }
        Outcome::NoAction => {
            lines.push("outcome: no action; no proposal beat doing nothing".to_string());
            WITHHELD
        }
        Outcome::Abstain(levers) => {
            lines.push(format!(
                "outcome: abstain; weakly identified and not open to experiment: {}",
                levers.join(", ")
            ));
            WITHHELD
        }
        Outcome::GatherEvidence(levers) => {
            lines.push(format!(
                "outcome: gather evidence; nothing is measured about: {}",
                levers.join(", ")
            ));
            WITHHELD
        }
        Outcome::ProposeExperiment(levers) => {
            lines.push(format!(
                "outcome: propose an experiment on: {}",
                levers.join(", ")
            ));
            WITHHELD
        }
        Outcome::Stopped(why) => {
            lines.push(format!("outcome: stopped; {why}"));
            WITHHELD
        }
        Outcome::Refused { reason, attempted } => {
            lines.push(format!(
                "outcome: attempted and refused; {reason} ({} step(s) attempted, none sent)",
                attempted.len()
            ));
            WITHHELD
        }
        Outcome::Executed(_) => {
            return Err(Error::guard(
                "a pass reported an executed step from a binary with no adapter; refusing to \
                 print it",
            ));
        }
    };
    Ok((lines, code))
}

/// Run every request in order under one session over one journal.
pub fn shadow(
    policy_document: &str,
    requests: &[String],
    journal: &mut EventLog,
    clock: &dyn Clock,
) -> Result<(Vec<String>, u8)> {
    let mut session = Session::open(policy_document, journal)?;
    let mut lines = Vec::new();
    let mut code = ADMITTED;
    for document in requests {
        let (pass, verdict) = session.pass(document, journal, clock.now())?;
        lines.extend(pass);
        code = code.max(verdict);
    }
    Ok((lines, code))
}

/// The `agency` family. One subcommand.
pub fn run(arguments: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<u8> {
    let Some((action, rest)) = arguments.split_first() else {
        return Err(Error::invalid(USAGE));
    };
    if action != "shadow" {
        return Err(Error::invalid(USAGE));
    }
    let paths = paths(rest, env)?;
    let policy = std::fs::read_to_string(&paths.policy).map_err(|error| {
        Error::io(format!(
            "could not read the agency policy {}: {error}",
            paths.policy
        ))
    })?;
    let mut requests = Vec::with_capacity(paths.requests.len());
    for path in &paths.requests {
        requests.push(
            std::fs::read_to_string(path)
                .map_err(|error| Error::io(format!("could not read {path}: {error}")))?,
        );
    }
    let mut journal = EventLog::open(&paths.journal)?;
    let (lines, code) = shadow(&policy, &requests, &mut journal, &SystemClock)?;
    for line in lines {
        println!("{line}");
    }
    Ok(code)
}
