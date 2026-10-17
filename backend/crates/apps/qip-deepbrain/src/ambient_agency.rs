//! The deep brain's run loop reaching the agency (R12-AGENCY-7029,
//! R12-COVERAGE-7190): every ambient signal the research sweep routed and the
//! attention router activated is offered to `qip-agency`'s one door, promoted,
//! cleared by the action guard and written to an internal outbox for a person
//! to read.
//!
//! # Why this exists
//!
//! The ambient mesh ended in an advisory and the agency began at an operator's
//! request document, and nothing joined them. The claim that authority and
//! conduct gates sit between detection and execution was therefore a claim
//! about a path that did not run. This module is the path, called from
//! [`crate::node::run_with`] on the same cycle that journals the sweep.
//!
//! # What it may not do
//!
//! Act. The only effect reachable from here is `qip_agency::outbox`'s append
//! (ADR 0114 Amendment 1, owner decision D5). No `Adapter` is implemented in
//! this file and none is named; the door owns its adapter. Publishing,
//! outreach, pricing and operational change stay refused.
//!
//! # The approvers are policy, not people
//!
//! The promotion approver and the plan approver are identities this module
//! names ([`PROMOTION_APPROVER`], [`PLAN_APPROVER`]); the guard enforces that
//! neither is the acting identity. They record that the standing intake policy
//! admitted the draft, which is all a draft that only a person can act on
//! needs: the person who reads the outbox is the approval that matters. A
//! change that let this path act would need a named human approver here first.
//!
//! # Failure
//!
//! A sweep's routing never stops the node. An opportunity the door refuses is
//! counted by reason in [`AgencyRound`]; an outbox write that fails is held
//! and retried on the next cycle, bounded (see `qip_agency::ambient_intake`).

use crate::ambient_journal::JournalDestination;
use crate::research::ResearchRound;
use qip_agency::ambient_intake::{AmbientIntake, AmbientOpportunity, Intake, IntakeConfig};
use qip_contracts::ambient::Disposition;
use qip_core::error::{Error, Result};
use qip_core::{CorrelationId, Timestamp};
use qip_events::EventLog;
use std::collections::BTreeMap;

/// The variable naming the outbox's file.
pub const AGENCY_OUTBOX_VARIABLE: &str = "QIP_DEEPBRAIN_AGENCY_OUTBOX";

/// The identity every draft is sent under.
pub const ACTING_IDENTITY: &str = "deepbrain/ambient-drafter";

/// Who promotes an ambient output to the agency.
pub const PROMOTION_APPROVER: &str = "deepbrain/ambient-promotion-policy";

/// Who approves the plan of a draft.
pub const PLAN_APPROVER: &str = "deepbrain/ambient-plan-policy";

/// What one cycle's routing did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgencyRound {
    /// Activated attention events the sweep produced.
    pub offered: usize,
    pub drafted: usize,
    pub duplicate: usize,
    /// Refused by the guard, by reason.
    pub withheld: BTreeMap<String, usize>,
    /// Outbox writes held for a retry, and abandoned after retries.
    pub deferred: usize,
    pub abandoned: usize,
    /// Refused before the guard (promotion, provenance, expiry), by reason.
    pub refused: BTreeMap<String, usize>,
    /// Retried drafts that landed this cycle.
    pub retried_drafted: usize,
}

impl AgencyRound {
    pub fn describe(&self) -> String {
        format!(
            "agency: {} offered, {} drafted ({} retried), {} duplicate, {} held for retry, {} \
             abandoned, withheld {:?}, refused {:?}; drafts only, nothing sent",
            self.offered,
            self.drafted,
            self.retried_drafted,
            self.duplicate,
            self.deferred,
            self.abandoned,
            self.withheld,
            self.refused
        )
    }

    fn tally(&mut self, outcome: Intake, retried: bool) {
        match outcome {
            Intake::Drafted { .. } => {
                self.drafted += 1;
                if retried {
                    self.retried_drafted += 1;
                }
            }
            Intake::Duplicate { .. } => self.duplicate += 1,
            Intake::Withheld { reason } => *self.withheld.entry(reason).or_default() += 1,
            Intake::Deferred { .. } => self.deferred += 1,
            Intake::Abandoned { .. } => self.abandoned += 1,
        }
    }
}

/// Where the outbox is written: beside the platform's log when that is a file.
pub fn destination(
    value: Option<String>,
    platform_log: &qip_kernel::EventLogDestination,
) -> Result<JournalDestination> {
    match value {
        Some(raw) if raw.trim().is_empty() => Err(Error::invalid(format!(
            "{AGENCY_OUTBOX_VARIABLE} is set but blank; unset it to write beside the event log, \
             or name a file"
        ))),
        Some(raw) => Ok(JournalDestination::File(raw.into())),
        None => Ok(match platform_log {
            qip_kernel::EventLogDestination::InMemory => JournalDestination::InMemory,
            qip_kernel::EventLogDestination::File { path, .. } => {
                let mut name = path.as_os_str().to_owned();
                name.push(".outbox.jsonl");
                JournalDestination::File(name.into())
            }
        }),
    }
}

/// The run loop's handle on the agency door.
#[derive(Debug)]
pub struct AmbientAgency {
    intake: AmbientIntake,
}

impl AmbientAgency {
    /// Open the door on `destination`, refusing an outbox whose chain breaks.
    pub fn open(destination: &JournalDestination) -> Result<Self> {
        let log = match destination {
            JournalDestination::InMemory => EventLog::in_memory(),
            JournalDestination::File(path) => EventLog::open(path)?,
        };
        Self::over(log)
    }

    /// Open the door on a log the caller built (a test holds a small one).
    pub fn over(log: EventLog) -> Result<Self> {
        let intake = AmbientIntake::open(IntakeConfig::new(ACTING_IDENTITY, PLAN_APPROVER), log)?;
        Ok(Self { intake })
    }

    pub fn intake(&self) -> &AmbientIntake {
        &self.intake
    }

    pub fn intake_mut(&mut self) -> &mut AmbientIntake {
        &mut self.intake
    }

    /// Route one sweep: retry what was held, then offer each activated
    /// attention event whose signal this sweep produced. `correlation` is the
    /// ambient journal's for the sweep; without a journal the draft is filed
    /// under a correlation of its own naming the cycle. The rate window is the
    /// cycle, which never goes backwards.
    pub fn route(
        &mut self,
        cycle: u64,
        round: &ResearchRound,
        correlation: Option<&CorrelationId>,
        now: Timestamp,
    ) -> Result<AgencyRound> {
        let mut done = AgencyRound::default();
        for outcome in self.intake.retry(now, cycle)? {
            done.tally(outcome, true);
        }
        let own;
        let correlation = match correlation {
            Some(shared) => shared,
            None => {
                own = CorrelationId::from_string(format!("ambient-sweep-{cycle}"));
                &own
            }
        };
        for event in &round.attention_events {
            if event.disposition != Disposition::Activated {
                continue;
            }
            done.offered += 1;
            let Some(signal) = round.signals.iter().find(|s| s.id() == event.signal_id) else {
                // A signal deferred in an earlier sweep: this round no longer
                // holds it, and a draft built without the signal would invent
                // its provenance.
                *done
                    .refused
                    .entry("signal not in this sweep".to_string())
                    .or_default() += 1;
                continue;
            };
            let admitted = AmbientOpportunity::advise(signal, event, correlation)
                .and_then(|advisory| self.intake.admit(advisory, PROMOTION_APPROVER, now, cycle));
            match admitted {
                Ok(outcome) => done.tally(outcome, false),
                Err(error) => {
                    *done.refused.entry(error.message().to_string()).or_default() += 1;
                }
            }
        }
        Ok(done)
    }
}
