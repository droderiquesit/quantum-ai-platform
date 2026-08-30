//! Control 6 — kill switches and incident response.
//!
//! This mirrors `qip_risk_engine::autonomy::KillSwitch` and must not diverge
//! from it. The asymmetry is the whole design: **tripping requires no
//! authority** because the cost of a false stop is a day of missed opportunity
//! and the cost of a missed one is the book; **clearing requires a named
//! operator with a fresh credential** and leaves a record. Stopping should be
//! easy and restarting should not.
//!
//! What this adds on top of the risk engine's switch is the mapping from
//! [`Severity`] to what actually halts, and the incident record the halt came
//! from. A clearance here also demands a stated reason, which the risk
//! engine's does not: that is a strictly higher bar, not a different rule, so
//! a deployment satisfying this one satisfies both.
//!
//! The credential window is the same fifteen minutes used by
//! `qip_risk_engine::autonomy` and by [`crate::approval::ApprovalChain`].

use crate::approval::{MAXIMUM_CREDENTIAL_AGE, OperatorCredential};
use qip_contracts::governance::Severity;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a response halts.
///
/// There is no variant meaning "halt something, details to follow". A response
/// that cannot say what it stops cannot be executed, and the enum not having
/// that shape is what forces the incident to carry a scope or a cell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HaltScope {
    /// Recorded, nothing stops.
    Nothing,
    /// One strategy or instrument stops.
    Scope(String),
    /// One cell stops.
    Cell(String),
    /// Everything stops.
    Everything,
}

impl HaltScope {
    pub fn halts_something(&self) -> bool {
        !matches!(self, Self::Nothing)
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Nothing => "nothing halts".to_string(),
            Self::Scope(s) => format!("scope `{s}` halts"),
            Self::Cell(c) => format!("cell `{c}` halts"),
            Self::Everything => "everything halts".to_string(),
        }
    }
}

/// Something that went wrong.
///
/// The constructor refuses an incident whose severity names a target it does
/// not carry — a `Scoped` incident with no scope, a `Cell` incident with no
/// cell. Without that check the response mapping would have a case with
/// nothing to halt, and the safe reading of that case ("halt everything") and
/// the convenient one ("halt nothing") are very far apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    id: String,
    at: Timestamp,
    severity: Severity,
    /// The component or person that noticed. Not an authority — anything may
    /// raise an incident — but the record needs to say who did.
    detected_by: String,
    summary: String,
    scope: Option<String>,
    cell: Option<String>,
}

impl Incident {
    pub fn new(
        id: impl Into<String>,
        at: Timestamp,
        severity: Severity,
        detected_by: impl Into<String>,
        summary: impl Into<String>,
        scope: Option<String>,
        cell: Option<String>,
    ) -> Result<Self> {
        let id = id.into();
        let detected_by = detected_by.into();
        let summary = summary.into();
        if id.trim().is_empty() {
            return Err(Error::invalid("an incident must have an id"));
        }
        if detected_by.trim().is_empty() {
            return Err(Error::invalid("an incident must record what detected it"));
        }
        if summary.trim().len() < 10 {
            return Err(Error::invalid(format!(
                "incident {id} must summarise what happened; the summary is what the person \
                 deciding whether to clear it reads first"
            )));
        }
        if severity == Severity::Scoped && scope.as_ref().is_none_or(|s| s.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "incident {id} is scoped but names no scope to halt"
            )));
        }
        if severity == Severity::Cell && cell.as_ref().is_none_or(|c| c.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "incident {id} is cell-level but names no cell to halt"
            )));
        }
        Ok(Self {
            id,
            at,
            severity,
            detected_by,
            summary,
            scope,
            cell,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn at(&self) -> Timestamp {
        self.at
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn detected_by(&self) -> &str {
        &self.detected_by
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    pub fn cell(&self) -> Option<&str> {
        self.cell.as_deref()
    }
}

/// Which severity halts what.
///
/// The mapping is fixed. The only configuration is a floor, and a floor can
/// only raise a response, never lower one — a policy that could soften the
/// response to a global incident is not a policy, it is a way to disable the
/// control from a config file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponsePolicy {
    floor: Severity,
}

impl Default for ResponsePolicy {
    fn default() -> Self {
        Self::standard()
    }
}

impl ResponsePolicy {
    /// Observation records, scoped halts a scope, cell halts a cell, global
    /// halts everything.
    pub const fn standard() -> Self {
        Self {
            floor: Severity::Observation,
        }
    }

    /// Treat every incident as at least this severe.
    ///
    /// For a deployment in a heightened state — a fresh strategy, a venue with
    /// a known problem — where an observation should still stop a scope.
    pub const fn with_floor(floor: Severity) -> Self {
        Self { floor }
    }

    pub const fn floor(&self) -> Severity {
        self.floor
    }

    /// The severity actually applied: the incident's, or the floor if higher.
    pub fn effective_severity(&self, incident: &Incident) -> Severity {
        if self.floor > incident.severity() {
            self.floor
        } else {
            incident.severity()
        }
    }

    /// What this incident halts.
    ///
    /// A raised floor that names a target the incident does not carry falls
    /// back to the widest halt that is certainly correct. Halting more than
    /// necessary is the safe direction; halting nothing because the incident
    /// did not name a cell is not.
    pub fn response_to(&self, incident: &Incident) -> HaltScope {
        match self.effective_severity(incident) {
            Severity::Observation => HaltScope::Nothing,
            Severity::Scoped => match incident.scope() {
                Some(scope) => HaltScope::Scope(scope.to_string()),
                None => match incident.cell() {
                    Some(cell) => HaltScope::Cell(cell.to_string()),
                    None => HaltScope::Everything,
                },
            },
            Severity::Cell => match incident.cell() {
                Some(cell) => HaltScope::Cell(cell.to_string()),
                None => HaltScope::Everything,
            },
            Severity::Global => HaltScope::Everything,
        }
    }
}

/// A halt in force, and the incident that caused it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Halt {
    pub scope: HaltScope,
    pub since: Timestamp,
    pub incident: Incident,
}

/// One recorded lifting of a halt.
///
/// A halt that can be lifted without a record is a control with no
/// accountability: the review can say what stopped the platform but not who
/// started it again, which is the more consequential of the two.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clearance {
    pub at: Timestamp,
    pub operator: String,
    pub method: String,
    pub reason: String,
    pub scope: HaltScope,
    /// The incident that had halted it, so the record carries both halves.
    pub cleared: Incident,
}

/// Incidents, the halts they caused, and who lifted them.
#[derive(Debug, Default)]
pub struct IncidentLog {
    policy: ResponsePolicy,
    incidents: Vec<Incident>,
    global: Option<Halt>,
    scopes: BTreeMap<String, Halt>,
    cells: BTreeMap<String, Halt>,
    clearances: Vec<Clearance>,
}

impl IncidentLog {
    pub fn new(policy: ResponsePolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    pub fn policy(&self) -> ResponsePolicy {
        self.policy
    }

    /// Record an incident and apply the policy. Requires no authority at all.
    ///
    /// A later incident does not overwrite the halt an earlier one caused: the
    /// first reason is the one that matters when somebody reconstructs why the
    /// platform stopped.
    pub fn record(&mut self, incident: Incident) -> HaltScope {
        let response = self.policy.response_to(&incident);
        let halt = Halt {
            scope: response.clone(),
            since: incident.at(),
            incident: incident.clone(),
        };
        match &response {
            HaltScope::Nothing => {}
            HaltScope::Scope(scope) => {
                self.scopes.entry(scope.clone()).or_insert(halt);
            }
            HaltScope::Cell(cell) => {
                self.cells.entry(cell.clone()).or_insert(halt);
            }
            HaltScope::Everything => {
                if self.global.is_none() {
                    self.global = Some(halt);
                }
            }
        }
        self.incidents.push(incident);
        response
    }

    pub fn incidents(&self) -> &[Incident] {
        &self.incidents
    }

    pub fn is_globally_halted(&self) -> bool {
        self.global.is_some()
    }

    pub fn global_halt(&self) -> Option<&Halt> {
        self.global.as_ref()
    }

    /// Whether a scope running on a cell may proceed.
    ///
    /// A global halt stops everything, a cell halt stops everything on that
    /// cell, and a scope halt stops that scope wherever it runs.
    pub fn is_halted(&self, scope: &str, cell: &str) -> bool {
        self.global.is_some() || self.cells.contains_key(cell) || self.scopes.contains_key(scope)
    }

    pub fn halted_scopes(&self) -> Vec<&str> {
        self.scopes.keys().map(String::as_str).collect()
    }

    pub fn halted_cells(&self) -> Vec<&str> {
        self.cells.keys().map(String::as_str).collect()
    }

    /// Every halt lifted, by whom and why.
    pub fn clearances(&self) -> &[Clearance] {
        &self.clearances
    }

    /// Lift the global halt.
    ///
    /// Clearing a halt that is not in force is not an error — it is the
    /// idempotent case an operator retrying a request hits — but it records
    /// nothing either, because nothing happened.
    pub fn clear_global(
        &mut self,
        operator: &OperatorCredential,
        at: Timestamp,
        reason: impl Into<String>,
    ) -> Result<()> {
        let Some(halt) = self.global.clone() else {
            return Ok(());
        };
        let reason = Self::check(operator, at, reason.into())?;
        self.global = None;
        self.clearances.push(Clearance {
            at,
            operator: operator.subject().to_string(),
            method: operator.method().to_string(),
            reason,
            scope: HaltScope::Everything,
            cleared: halt.incident,
        });
        Ok(())
    }

    /// Lift one scope's halt. Same rules as [`IncidentLog::clear_global`].
    pub fn clear_scope(
        &mut self,
        scope: &str,
        operator: &OperatorCredential,
        at: Timestamp,
        reason: impl Into<String>,
    ) -> Result<()> {
        let Some(halt) = self.scopes.get(scope).cloned() else {
            return Ok(());
        };
        let reason = Self::check(operator, at, reason.into())?;
        self.scopes.remove(scope);
        self.clearances.push(Clearance {
            at,
            operator: operator.subject().to_string(),
            method: operator.method().to_string(),
            reason,
            scope: HaltScope::Scope(scope.to_string()),
            cleared: halt.incident,
        });
        Ok(())
    }

    /// Lift one cell's halt. Same rules as [`IncidentLog::clear_global`].
    pub fn clear_cell(
        &mut self,
        cell: &str,
        operator: &OperatorCredential,
        at: Timestamp,
        reason: impl Into<String>,
    ) -> Result<()> {
        let Some(halt) = self.cells.get(cell).cloned() else {
            return Ok(());
        };
        let reason = Self::check(operator, at, reason.into())?;
        self.cells.remove(cell);
        self.clearances.push(Clearance {
            at,
            operator: operator.subject().to_string(),
            method: operator.method().to_string(),
            reason,
            scope: HaltScope::Cell(cell.to_string()),
            cleared: halt.incident,
        });
        Ok(())
    }

    /// The two things a clearance needs: a fresh credential and a reason.
    fn check(operator: &OperatorCredential, at: Timestamp, reason: String) -> Result<String> {
        if !operator.is_fresh(at, MAXIMUM_CREDENTIAL_AGE) {
            return Err(Error::denied(format!(
                "operator {} authenticated at {} and is stale at {at}; re-authenticate to lift \
                 a halt",
                operator.subject(),
                operator.authenticated_at()
            )));
        }
        if reason.trim().len() < 10 {
            return Err(Error::denied(
                "lifting a halt requires a stated reason; the record of why somebody decided it \
                 was safe to continue is the point of the control",
            ));
        }
        Ok(reason)
    }
}
