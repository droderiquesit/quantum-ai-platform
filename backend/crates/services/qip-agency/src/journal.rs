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
//! 5. [`AgencyRecord::PassConcluded`] — the outcome, the stages, the steps,
//!    the eligibility refusals and the simulated branches.
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

use crate::causal::SimulationResult;
use crate::engine::{Outcome, Report};
use crate::estimate::EstimatorKind;
use qip_contracts::action::{ExecutionRecord, GoalApproval, Method, PlanApproval, ToolAudit};
use qip_core::lineage::Lineage;
use qip_core::{CorrelationId, Error, EventId, Timestamp};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};

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
    for decision in report.audit.iter().skip(audit_from) {
        w.write(
            AgencyRecord::ToolDecision {
                pass,
                decision: decision.clone(),
            },
            decision.at,
        )?;
    }
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
            stages_passed: report.passed.iter().map(|s| format!("{s:?}")).collect(),
            failed: report.failed.map(|s| format!("{s:?}")),
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
    };
    for record in records(log)? {
        resumed.next_pass = resumed.next_pass.max(record.pass().saturating_add(1));
        if let AgencyRecord::ToolDecision { decision, .. } = record {
            resumed.decisions.push(decision);
        }
    }
    Ok(resumed)
}
