//! R12-GOV-6124 / R12-AGENCY-1056..1069: every agency pass leaves its
//! approvals, its tool decisions and its verdict on the hash-chained event
//! log, not only in a [`Report`] a caller may drop.
//!
//! Before this module [`crate::engine::run`] returned the goal approval, the
//! plan approval and the execution record in its report, and the one binary
//! that ran it printed a few lines and exited: an approval nobody could
//! attribute after the process ended, and a guard decision nobody could
//! replay. Product direction puts reproducibility from the log first, so
//! these are now [`AgencyRecord`]s appended to the platform's own
//! [`EventLog`] — the same chain `qip replay` verifies.
//!
//! # Shape
//!
//! One pass writes, in order and linked by lineage (each record's causation
//! is the one before it, all sharing the pass's correlation id):
//!
//! 1. [`AgencyRecord::PassOpened`] — the goal, the acting identity, and the
//!    code and policy versions the pass ran under (R12-GOV-6121: a decision
//!    nobody can say what produced cannot be reproduced);
//! 2. a widening, if one was asked for, granted or refused;
//! 3. [`AgencyRecord::GoalApproved`], [`AgencyRecord::PlanApproved`] and —
//!    only once an adapter call was attempted — [`AgencyRecord::Executed`]:
//!    GOV-6124's three records, each with its own actor and time;
//! 4. every tool decision the registry audited during the pass, with what it
//!    charged, which is what [`resume`] re-applies on the next start;
//! 5. what the pass learned, when the caller offered an outcome:
//!    [`AgencyRecord::Learned`] carries the outcome as the memory kept it
//!    (its [`crate::memory::Observed`] says whether it was measured or the
//!    pass's own simulation), the attribution, the effect the policy was
//!    held to, the interval and side effects it was held to, and the
//!    authority before and after — everything a reader needs to recompute
//!    the update from the log alone; [`AgencyRecord::LearningWithheld`]
//!    carries the reason nothing was learned, because an outcome that is
//!    absent is recorded as absent, not invented;
//! 6. [`AgencyRecord::PassConcluded`] — the outcome, the stages (through
//!    `Update` when the pass learned), the steps, the eligibility refusals
//!    and the simulated branches.
//!
//! # Topic
//!
//! `compliance.evaluated`, told apart from the kernel's and the capital
//! fabric's records on the same topic by the producer [`AGENCY_ORIGIN`]. A
//! conduct, eligibility and approval verdict is a compliance evaluation, the
//! topic sits in the Decide group whose retention class is irreplaceable, so
//! the log refuses an append rather than evicting one of these; and a new
//! topic would have to be added to the fabric's bindings, routing table and
//! schema lock, which are another lane's. If the agency ever publishes on the
//! fabric, it gets its own topic then.

use crate::attribution::EffectAttribution;
use crate::autonomy::Authority;
use crate::causal::SimulationResult;
use crate::engine::{Learning, LearningOutcome, Outcome, Report};
use crate::estimate::EstimatorKind;
use crate::guard::Voice;
use crate::memory::ActionOutcome;
use qip_contracts::action::{ExecutionRecord, GoalApproval, Method, PlanApproval, ToolAudit};
use qip_contracts::action_intent::ActionIntent;
use qip_core::lineage::Lineage;
use qip_core::{CorrelationId, Decimal, Error, EventId, Timestamp};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The producer every agency record carries.
pub const AGENCY_ORIGIN: &str = "agency/act-stage";

/// What the code and the policy were when a pass ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// The binary's version, and the source commit when the build stamped
    /// one.
    pub code_version: String,
    /// A digest of the policy document the guard was built from.
    pub policy_version: String,
}

/// One simulated branch, as journaled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulatedBranch {
    pub candidate: String,
    pub model: String,
    pub scenario: String,
    pub effect: f64,
}

/// One agency record on the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum AgencyRecord {
    PassOpened {
        pass: u64,
        goal_target: String,
        acting_identity: String,
        provenance: Provenance,
    },
    Widened {
        pass: u64,
        class: Method,
        goal_id: String,
        intervention_id: String,
        action_id: String,
        approver: String,
        estimator: EstimatorKind,
    },
    WideningRefused {
        pass: u64,
        class: Method,
        approver: String,
        reason: String,
    },
    GoalApproved {
        pass: u64,
        approval: GoalApproval,
    },
    PlanApproved {
        pass: u64,
        approval: PlanApproval,
    },
    Executed {
        pass: u64,
        record: ExecutionRecord,
    },
    ToolDecision {
        pass: u64,
        decision: ToolAudit,
    },
    /// A message the guard admitted and the adapter was handed: what was
    /// said, by which tool, as whom (R12-AGENCY-1064). Resumed into the
    /// guard on start so a restart does not forget who said what.
    VoiceReleased {
        pass: u64,
        voice: Voice,
    },
    /// The §27 intent of one step the guard cleared, shadowed or sent
    /// (R12-CONTRACT-7082): the record another brain or an auditor reads
    /// to know what was intended, through which channel, on whose
    /// authority. Decoded strictly, so an edited payload on the journal is
    /// refused when it is read back.
    IntentRecorded {
        pass: u64,
        intent: ActionIntent,
    },
    /// The observe-effect, attribute and update stages ran on this outcome.
    Learned {
        pass: u64,
        outcome: Box<ActionOutcome>,
        attribution: Box<EffectAttribution>,
        /// `actual_observations - no_action_counterfactual`: what the policy
        /// was held to.
        effect: Decimal,
        /// The interval and side effects the policy predicted, so `after`
        /// follows from `effect` by the drift rule and nothing else.
        predicted_uplift_low: Decimal,
        predicted_uplift_high: Decimal,
        predicted_side_effects: Vec<String>,
        authority_before: Authority,
        authority_after: Authority,
    },
    /// Nothing was learned from this pass, and why.
    LearningWithheld {
        pass: u64,
        /// The learn stage that refused, when one was reached.
        stage_failed: Option<String>,
        reason: String,
    },
    PassConcluded {
        pass: u64,
        /// `shadowed`, `executed`, `refused`, `stopped`, `no_action`,
        /// `abstain`, `gather_evidence` or `propose_experiment`.
        outcome: String,
        detail: String,
        stages_passed: Vec<String>,
        failed: Option<String>,
        steps: Vec<String>,
        eligibility_refusals: Vec<String>,
        simulations: Vec<SimulatedBranch>,
        provenance: Provenance,
    },
}

impl AgencyRecord {
    pub fn pass(&self) -> u64 {
        match self {
            Self::PassOpened { pass, .. }
            | Self::Widened { pass, .. }
            | Self::WideningRefused { pass, .. }
            | Self::GoalApproved { pass, .. }
            | Self::PlanApproved { pass, .. }
            | Self::Executed { pass, .. }
            | Self::ToolDecision { pass, .. }
            | Self::VoiceReleased { pass, .. }
            | Self::IntentRecorded { pass, .. }
            | Self::Learned { pass, .. }
            | Self::LearningWithheld { pass, .. }
            | Self::PassConcluded { pass, .. } => *pass,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::PassOpened { .. } => "opened",
            Self::Widened { .. } => "widened",
            Self::WideningRefused { .. } => "widening-refused",
            Self::GoalApproved { .. } => "goal-approved",
            Self::PlanApproved { .. } => "plan-approved",
            Self::Executed { .. } => "executed",
            Self::ToolDecision { .. } => "tool",
            Self::VoiceReleased { .. } => "voice",
            Self::IntentRecorded { .. } => "intent",
            Self::Learned { .. } => "learned",
            Self::LearningWithheld { .. } => "learning-withheld",
            Self::PassConcluded { .. } => "concluded",
        }
    }
}

impl EventBody for AgencyRecord {
    /// See the module documentation for why this topic.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// A widening asked for during a pass, and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub enum WideningOutcome {
    Granted {
        class: Method,
        goal_id: String,
        intervention_id: String,
        action_id: String,
        approver: String,
        estimator: EstimatorKind,
    },
    Refused {
        class: Method,
        approver: String,
        reason: String,
    },
}

/// Appends one pass's records, chained by lineage.
struct Writer<'a> {
    log: &'a mut EventLog,
    pass: u64,
    lineage: Lineage,
    previous: Option<EventId>,
    count: u64,
    at: Timestamp,
}

impl Writer<'_> {
    fn write(&mut self, record: AgencyRecord, occurred_at: Timestamp) -> Result<(), Error> {
        let event_id = EventId::from_string(format!(
            "evt-agency-{}-{}-{}",
            self.pass,
            self.count,
            record.kind()
        ));
        let lineage = match &self.previous {
            Some(parent) => self.lineage.caused_by(parent, AGENCY_ORIGIN),
            None => self.lineage.clone(),
        };
        let event =
            Envelope::new(event_id.clone(), occurred_at, self.at, lineage, record).erase()?;
        self.log.append(&event)?;
        self.previous = Some(event_id);
        self.count += 1;
        Ok(())
    }
}

fn outcome_label(outcome: &Outcome) -> (&'static str, String, Vec<String>) {
    let steps = |s: &[crate::plan::Step]| {
        s.iter()
            .map(|s| format!("{} on {}, exposure {}", s.tool, s.variable, s.exposure))
            .collect::<Vec<_>>()
    };
    match outcome {
        Outcome::Shadowed(s) => ("shadowed", String::new(), steps(s)),
        Outcome::Executed(s) => ("executed", String::new(), steps(s)),
        Outcome::Refused { reason, attempted } => ("refused", reason.clone(), steps(attempted)),
        Outcome::Stopped(why) => ("stopped", why.clone(), Vec::new()),
        Outcome::NoAction => ("no_action", String::new(), Vec::new()),
        Outcome::Abstain(l) => ("abstain", l.join(", "), Vec::new()),
        Outcome::GatherEvidence(l) => ("gather_evidence", l.join(", "), Vec::new()),
        Outcome::ProposeExperiment(l) => ("propose_experiment", l.join(", "), Vec::new()),
    }
}

/// One pass, as [`record_pass`] journals it.
#[derive(Debug, Clone, Copy)]
pub struct PassEntry<'a> {
    pub pass: u64,
    pub provenance: &'a Provenance,
    pub report: &'a Report,
    pub widening: Option<&'a WideningOutcome>,
    /// The length of the guard's audit before the pass, so only the
    /// decisions this pass made are written.
    pub audit_from: usize,
    pub goal_target: &'a str,
    pub acting_identity: &'a str,
    /// What the pass learned, or why it did not, when the caller runs the
    /// learn stages. `None` is a caller that offers no outcome at all and
    /// is journaled as such: no learning record, and the stages stop where
    /// the pass stopped.
    pub learning: Option<&'a Learning>,
    /// When the platform recorded the pass.
    pub at: Timestamp,
}

/// Journal one pass.
pub fn record_pass(log: &mut EventLog, entry: PassEntry<'_>) -> Result<(), Error> {
    let PassEntry {
        pass,
        provenance,
        report,
        widening,
        audit_from,
        goal_target,
        acting_identity,
        learning,
        at,
    } = entry;
    let mut w = Writer {
        log,
        pass,
        lineage: Lineage::root(
            CorrelationId::from_string(format!("agency-pass-{pass}")),
            AGENCY_ORIGIN,
        ),
        previous: None,
        count: 0,
        at,
    };
    w.write(
        AgencyRecord::PassOpened {
            pass,
            goal_target: goal_target.to_string(),
            acting_identity: acting_identity.to_string(),
            provenance: provenance.clone(),
        },
        at,
    )?;
    match widening {
        Some(WideningOutcome::Granted {
            class,
            goal_id,
            intervention_id,
            action_id,
            approver,
            estimator,
        }) => w.write(
            AgencyRecord::Widened {
                pass,
                class: *class,
                goal_id: goal_id.clone(),
                intervention_id: intervention_id.clone(),
                action_id: action_id.clone(),
                approver: approver.clone(),
                estimator: *estimator,
            },
            at,
        )?,
        Some(WideningOutcome::Refused {
            class,
            approver,
            reason,
        }) => w.write(
            AgencyRecord::WideningRefused {
                pass,
                class: *class,
                approver: approver.clone(),
                reason: reason.clone(),
            },
            at,
        )?,
        None => {}
    }
    if let Some(chain) = &report.approvals {
        w.write(
            AgencyRecord::GoalApproved {
                pass,
                approval: chain.goal.clone(),
            },
            chain.goal.at,
        )?;
        w.write(
            AgencyRecord::PlanApproved {
                pass,
                approval: chain.plan.clone(),
            },
            chain.plan.at,
        )?;
        if let Some(execution) = &chain.execution {
            w.write(
                AgencyRecord::Executed {
                    pass,
                    record: execution.clone(),
                },
                execution.at,
            )?;
        }
    }
    for intent in &report.intents {
        w.write(
            AgencyRecord::IntentRecorded {
                pass,
                intent: intent.clone(),
            },
            intent.created_at,
        )?;
    }
    for decision in report.audit.iter().skip(audit_from) {
        w.write(
            AgencyRecord::ToolDecision {
                pass,
                decision: decision.clone(),
            },
            decision.at,
        )?;
    }
    for voice in &report.released {
        w.write(
            AgencyRecord::VoiceReleased {
                pass,
                voice: voice.clone(),
            },
            at,
        )?;
    }
    match learning.map(|l| &l.outcome) {
        Some(LearningOutcome::Learned(learned)) => w.write(
            AgencyRecord::Learned {
                pass,
                outcome: Box::new(learned.outcome.clone()),
                attribution: Box::new(learned.attribution.clone()),
                effect: learned.effect,
                predicted_uplift_low: learned.predicted_uplift.0,
                predicted_uplift_high: learned.predicted_uplift.1,
                predicted_side_effects: learned.predicted_side_effects.iter().cloned().collect(),
                authority_before: learned.before,
                authority_after: learned.after,
            },
            at,
        )?,
        Some(LearningOutcome::Withheld(reason)) => w.write(
            AgencyRecord::LearningWithheld {
                pass,
                stage_failed: learning
                    .and_then(|l| l.failed)
                    .filter(|stage| report.failed != Some(*stage))
                    .map(|stage| format!("{stage:?}")),
                reason: reason.clone(),
            },
            at,
        )?,
        None => {}
    }
    // The stages are the learning's when there was one: it continued the
    // pass's log, so it holds the pass's stages and whatever came after.
    let (passed, failed) = match learning {
        Some(learning) => (&learning.passed, learning.failed),
        None => (&report.passed, report.failed),
    };
    let (outcome, detail, steps) = outcome_label(&report.outcome);
    let simulations = report
        .simulations
        .iter()
        .flat_map(|s| {
            s.results
                .iter()
                .map(|r: &SimulationResult| SimulatedBranch {
                    candidate: s.label.clone(),
                    model: r.model.clone(),
                    scenario: r.scenario.clone(),
                    effect: r.effect,
                })
        })
        .collect();
    w.write(
        AgencyRecord::PassConcluded {
            pass,
            outcome: outcome.to_string(),
            detail,
            stages_passed: passed.iter().map(|s| format!("{s:?}")).collect(),
            failed: failed.map(|s| format!("{s:?}")),
            steps,
            eligibility_refusals: report.eligibility.clone(),
            simulations,
            provenance: provenance.clone(),
        },
        at,
    )
}

/// Every agency record in the log, in log order. A record under the agency
/// producer that does not decode is refused, not skipped: a journal this
/// build cannot read is not one it may resume from.
pub fn records(log: &EventLog) -> Result<Vec<AgencyRecord>, Error> {
    let mut out = Vec::new();
    for event in log.events() {
        if event.topic != Topic::ComplianceEvaluated || event.lineage.producer != AGENCY_ORIGIN {
            continue;
        }
        out.push(event.decode::<AgencyRecord>()?.body);
    }
    Ok(out)
}

/// What a restarted composition root needs from the journal: the next pass
/// number, and every tool decision to re-apply to a fresh guard.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resumed {
    pub next_pass: u64,
    pub decisions: Vec<ToolAudit>,
    /// Every voice released, oldest first, for
    /// [`crate::guard::ActionGuard::resume_voices`].
    pub voices: Vec<Voice>,
}

/// Read back what [`record_pass`] wrote. Refuses a log whose chain does not
/// verify: resuming budgets from a journal that was edited would resume the
/// edit.
pub fn resume(log: &EventLog) -> Result<Resumed, Error> {
    log.verify_retained_chain().map_err(|sequence| {
        Error::denied(format!(
            "the journal's hash chain breaks at sequence {sequence}; an agency cannot resume \
             its budgets from a record that was altered"
        ))
    })?;
    let mut resumed = Resumed {
        next_pass: 1,
        decisions: Vec::new(),
        voices: Vec::new(),
    };
    for record in records(log)? {
        resumed.next_pass = resumed.next_pass.max(record.pass().saturating_add(1));
        match record {
            AgencyRecord::ToolDecision { decision, .. } => resumed.decisions.push(decision),
            AgencyRecord::VoiceReleased { voice, .. } => resumed.voices.push(voice),
            _ => {}
        }
    }
    Ok(resumed)
}

/// One pass's lineage, folded from the log (R12-OBS-7131): the goal the
/// pass opened on, its approvals, the execution record, what the registry
/// charged, what was said as whom, what was learned, and how it concluded.
/// Every field is what the records say; [`PassLineage::verify`] is what
/// makes them one graph rather than a pile.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct PassLineage {
    pub pass: u64,
    pub goal_target: Option<String>,
    pub acting_identity: Option<String>,
    pub goal_approval: Option<GoalApproval>,
    pub plan_approval: Option<PlanApproval>,
    pub execution: Option<ExecutionRecord>,
    pub tool_decisions: Vec<ToolAudit>,
    pub released: Vec<Voice>,
    /// The §27 intents of the pass's cleared steps (R12-CONTRACT-7082).
    pub intents: Vec<ActionIntent>,
    pub attribution: Option<EffectAttribution>,
    /// The concluded outcome label, once the pass concluded.
    pub outcome: Option<String>,
}

impl PassLineage {
    /// Whether the records form one connected chain: opened and concluded,
    /// a plan approval only over the goal approval it cites, an execution
    /// only over the plan approval it cites, nothing charged or released
    /// without an execution record, and nothing learned about a plan nobody
    /// approved. The first break is the error, naming the link.
    ///
    /// The failure prevented is a journal that holds every record and
    /// answers no question: a plan approval whose goal is missing reads, one
    /// record at a time, like an approved plan.
    pub fn verify(&self) -> Result<(), Error> {
        let pass = self.pass;
        let broken = |what: &str| {
            Err(Error::invalid(format!(
                "pass {pass}'s lineage breaks at {what}; the records do not form one chain"
            )))
        };
        if self.goal_target.is_none() {
            return broken("the opening: no PassOpened record names the goal");
        }
        if self.outcome.is_none() {
            return broken("the conclusion: no PassConcluded record");
        }
        match (&self.goal_approval, &self.plan_approval) {
            (None, Some(_)) => return broken("the plan approval: it cites no goal approval"),
            (Some(goal), Some(plan)) if plan.goal != goal.id => {
                return broken("the plan approval: it cites a goal approval the pass did not make");
            }
            _ => {}
        }
        match (&self.plan_approval, &self.execution) {
            (None, Some(_)) => return broken("the execution: it cites no plan approval"),
            (Some(plan), Some(execution)) if execution.plan != plan.id => {
                return broken("the execution: it cites a plan approval the pass did not make");
            }
            _ => {}
        }
        if self.execution.is_none() && !self.tool_decisions.is_empty() {
            return broken("a tool decision: the registry was charged with no execution record");
        }
        if self.execution.is_none() && !self.released.is_empty() {
            return broken("a released voice: a message went out with no execution record");
        }
        if let Some(intent) = self.intents.iter().find(|intent| {
            self.plan_approval
                .as_ref()
                .is_none_or(|plan| !intent.audit_lineage.contains(&plan.id))
        }) {
            return broken(&format!(
                "intent `{}`: it cites no plan approval this pass made",
                intent.intent_id
            ));
        }
        if self.attribution.is_some()
            && (self.goal_approval.is_none() || self.plan_approval.is_none())
        {
            return broken("the attribution: an effect is attributed to a plan nobody approved");
        }
        Ok(())
    }
}

/// Every pass's lineage in the log, by pass number.
pub fn lineages(log: &EventLog) -> Result<BTreeMap<u64, PassLineage>, Error> {
    let mut out: BTreeMap<u64, PassLineage> = BTreeMap::new();
    for record in records(log)? {
        let pass = record.pass();
        let entry = out.entry(pass).or_insert_with(|| PassLineage {
            pass,
            ..PassLineage::default()
        });
        match record {
            AgencyRecord::PassOpened {
                goal_target,
                acting_identity,
                ..
            } => {
                entry.goal_target = Some(goal_target);
                entry.acting_identity = Some(acting_identity);
            }
            AgencyRecord::GoalApproved { approval, .. } => entry.goal_approval = Some(approval),
            AgencyRecord::PlanApproved { approval, .. } => entry.plan_approval = Some(approval),
            AgencyRecord::Executed { record, .. } => entry.execution = Some(record),
            AgencyRecord::ToolDecision { decision, .. } => entry.tool_decisions.push(decision),
            AgencyRecord::VoiceReleased { voice, .. } => entry.released.push(voice),
            AgencyRecord::IntentRecorded { intent, .. } => entry.intents.push(intent),
            AgencyRecord::Learned { attribution, .. } => entry.attribution = Some(*attribution),
            AgencyRecord::PassConcluded { outcome, .. } => entry.outcome = Some(outcome),
            AgencyRecord::Widened { .. }
            | AgencyRecord::WideningRefused { .. }
            | AgencyRecord::LearningWithheld { .. } => {}
        }
    }
    Ok(out)
}

/// The lineage of one pass, or `None` when the log holds no record of it.
pub fn lineage_of(log: &EventLog, pass: u64) -> Result<Option<PassLineage>, Error> {
    Ok(lineages(log)?.remove(&pass))
}
