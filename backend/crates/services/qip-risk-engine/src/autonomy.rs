//! Autonomy levels and the kill switch.
//!
//! The platform runs at one of six levels, and the shipped default is
//! [`AutonomyLevel::PaperTrading`]. Live trading is off unless an operator
//! turns it on, and turning it on is not something any agent, model or
//! configuration file can do on its own: [`AutonomyController::request_change`]
//! requires an authenticated operator identity and refuses every unauthenticated
//! path.
//!
//! Two properties are worth stating plainly because the whole safety argument
//! rests on them:
//!
//! * **Nothing escalates itself.** There is no code path from a model output,
//!   an agent finding or a config value to a higher autonomy level. Every
//!   raise is an explicit call carrying an operator identity and a reason.
//! * **The kill switch is one-way until cleared.** Tripping it needs no
//!   authority beyond a component noticing something wrong; clearing it needs
//!   an operator. That asymmetry is deliberate: stopping should be easy and
//!   restarting should not.

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// How much the platform may do without a human.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    /// Observe only. Nothing is proposed, nothing is traded.
    Observation,
    /// Research and propose. Proposals are recorded, never executed.
    Advisory,
    /// Execute against a simulated broker. The shipped default.
    PaperTrading,
    /// Execute live, but every proposal needs a human release.
    SupervisedLive,
    /// Execute live within pre-approved limits; a human is notified.
    LimitedAutonomousLive,
    /// Execute live autonomously within the mandate.
    AutonomousLive,
}

impl AutonomyLevel {
    /// The level the platform starts at.
    ///
    /// Paper trading: the full loop runs, orders are worked against a
    /// simulated venue, and nothing reaches a real market. A platform that
    /// defaulted any higher would be one where forgetting to configure
    /// something is the dangerous case.
    pub const DEFAULT: Self = Self::PaperTrading;

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Advisory => "advisory",
            Self::PaperTrading => "paper_trading",
            Self::SupervisedLive => "supervised_live",
            Self::LimitedAutonomousLive => "limited_autonomous_live",
            Self::AutonomousLive => "autonomous_live",
        }
    }

    pub const fn ordinal(&self) -> u8 {
        match self {
            Self::Observation => 0,
            Self::Advisory => 1,
            Self::PaperTrading => 2,
            Self::SupervisedLive => 3,
            Self::LimitedAutonomousLive => 4,
            Self::AutonomousLive => 5,
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Self::all()
            .into_iter()
            .find(|level| level.as_str() == s)
            .ok_or_else(|| Error::invalid(format!("unknown autonomy level: {s}")))
    }

    /// The ceiling a *deployment* may be started at.
    ///
    /// [`Self::parse`] answers "is this one of the six levels", which is the
    /// right question for a domain model that has six. This answers "may a
    /// process be started here", and the answer for the three live levels is
    /// no, whatever the configuration says.
    ///
    /// The ladder keeps all six because the levels are real and the code that
    /// reasons about them — [`Self::is_live`], [`Self::requires_human_release`],
    /// the controller's escalation rules — has to keep working. What this
    /// removes is the path from a configuration value to a running process at
    /// one of them. A composition root that reads its ceiling through here
    /// cannot be talked into live trading by an environment variable, a
    /// ConfigMap, or a Terraform apply that slipped through review.
    ///
    /// Absent configuration is [`Self::PaperTrading`], not an error: a
    /// deployment that says nothing about its ceiling gets the shipped
    /// default, and a deployment that says something forbidden is told so
    /// rather than quietly lowered — a process that silently ignores the
    /// configuration it was given is one whose operator believes something
    /// false about it.
    ///
    /// This is the innermost of the layers that refuse a live ceiling. The
    /// outermost is a `terraform plan` validation, which stops a live value
    /// before it is ever written into the ConfigMap the workloads read. That
    /// one catches the reviewed, committed mistake; this one catches the
    /// unreviewed live edit that Terraform never sees.
    pub fn deployable(configured: Option<&str>) -> Result<Self> {
        let Some(configured) = configured else {
            return Ok(Self::PaperTrading);
        };
        let level = Self::parse(configured)?;
        if level.is_live() {
            return Err(Error::denied(format!(
                "this deployment is configured at autonomy level '{}', at which orders \
                 reach a real venue. This platform is paper-trading only and will not \
                 start there. Set the ceiling to paper_trading, advisory or observation",
                level.as_str()
            )));
        }
        Ok(level)
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Observation,
            Self::Advisory,
            Self::PaperTrading,
            Self::SupervisedLive,
            Self::LimitedAutonomousLive,
            Self::AutonomousLive,
        ]
    }

    /// Whether orders reach a real venue at this level.
    pub const fn is_live(&self) -> bool {
        matches!(
            self,
            Self::SupervisedLive | Self::LimitedAutonomousLive | Self::AutonomousLive
        )
    }

    /// Whether any order is produced at all.
    pub const fn executes(&self) -> bool {
        self.ordinal() >= Self::PaperTrading.ordinal()
    }

    /// Whether a human must release each proposal individually.
    pub const fn requires_human_release(&self) -> bool {
        matches!(self, Self::SupervisedLive)
    }

    /// Whether a human must be notified of each release.
    pub const fn requires_notification(&self) -> bool {
        matches!(self, Self::LimitedAutonomousLive)
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Self::Observation => "observes and records; forms no proposals",
            Self::Advisory => "researches and proposes; nothing is executed",
            Self::PaperTrading => "executes against a simulated broker; nothing reaches a market",
            Self::SupervisedLive => "executes live, one human release per proposal",
            Self::LimitedAutonomousLive => {
                "executes live within pre-approved limits, notifying a human"
            }
            Self::AutonomousLive => "executes live autonomously within the mandate",
        }
    }
}

impl Default for AutonomyLevel {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for AutonomyLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who authorised an autonomy change.
///
/// Constructing one is the authentication boundary. There is deliberately no
/// `Default`, no way to build one from a string, and no way for an agent to
/// produce one: a change requires an authenticated operator, and this type is
/// the evidence of that.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorIdentity {
    /// The operator's identifier from the authentication system.
    subject: String,
    /// The authentication method, e.g. `oidc`, `hardware-token`.
    method: String,
    /// When the operator authenticated.
    authenticated_at: Timestamp,
    /// Whether a second operator approved. Required to reach a live level.
    second_approver: Option<String>,
}

impl OperatorIdentity {
    /// Build from a verified authentication.
    ///
    /// The caller is asserting that it has already verified the credential.
    /// Everything downstream trusts this, so it should be constructed in
    /// exactly one place: the API's authentication middleware.
    pub fn verified(
        subject: impl Into<String>,
        method: impl Into<String>,
        authenticated_at: Timestamp,
    ) -> Self {
        Self {
            subject: subject.into(),
            method: method.into(),
            authenticated_at,
            second_approver: None,
        }
    }

    /// Record a second operator's approval.
    pub fn with_second_approver(mut self, approver: impl Into<String>) -> Self {
        let approver = approver.into();
        // A second approver who is the same person is not a second approver.
        if approver != self.subject {
            self.second_approver = Some(approver);
        }
        self
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn second_approver(&self) -> Option<&str> {
        self.second_approver.as_deref()
    }

    pub fn has_second_approver(&self) -> bool {
        self.second_approver.is_some()
    }

    /// Whether the authentication is recent enough to authorise a change.
    ///
    /// A session token from three days ago is not evidence that anyone is at
    /// the keyboard now.
    pub fn is_fresh(&self, now: Timestamp, maximum_age: qip_core::Duration) -> bool {
        now >= self.authenticated_at && now.since(self.authenticated_at) <= maximum_age
    }
}

/// Why the platform stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillSwitchTrip {
    pub at: Timestamp,
    /// The component that tripped it.
    pub tripped_by: String,
    pub reason: String,
    /// Scope: `None` is global, otherwise the strategy or instrument halted.
    pub scope: Option<String>,
}

/// One recorded clearing of a halt.
///
/// A halt that can be lifted without a record is a control with no
/// accountability: the incident review can say what stopped the platform but
/// not who started it again, which is the more consequential of the two.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KillSwitchClearance {
    pub at: Timestamp,
    /// The operator who lifted it.
    pub operator: String,
    /// How that operator authenticated.
    pub method: String,
    /// Scope: `None` is the global stop, otherwise the scope lifted.
    pub scope: Option<String>,
    /// The trip that was lifted, so the record carries why it was halted.
    pub cleared: KillSwitchTrip,
}

/// How stale an operator's credential may be when lifting a halt.
///
/// The same window the autonomy controller uses for a level change. Restarting
/// a platform that stopped itself is at least as consequential, so it is not
/// held to a looser standard.
const MAXIMUM_CLEARANCE_CREDENTIAL_AGE: qip_core::Duration = qip_core::Duration::from_mins(15);

/// The kill switch.
///
/// Tripping requires no authority: any component that notices something wrong
/// can stop the platform, because the cost of a false stop is far below the
/// cost of a missed one.
///
/// Clearing requires an authenticated operator with a fresh credential, and
/// leaves a [`KillSwitchClearance`] naming them. Both halves of the record
/// matter: the trip says why the platform stopped, the clearance says who
/// decided it was safe to continue.
#[derive(Clone, Debug, Default)]
pub struct KillSwitch {
    global: Option<KillSwitchTrip>,
    scoped: BTreeMap<String, KillSwitchTrip>,
    history: Vec<KillSwitchTrip>,
    clearances: Vec<KillSwitchClearance>,
}

impl KillSwitch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop everything.
    pub fn trip_global(
        &mut self,
        at: Timestamp,
        tripped_by: impl Into<String>,
        reason: impl Into<String>,
    ) {
        let trip = KillSwitchTrip {
            at,
            tripped_by: tripped_by.into(),
            reason: reason.into(),
            scope: None,
        };
        self.history.push(trip.clone());
        // A later trip does not overwrite an earlier one: the first reason is
        // the one that matters for an incident review.
        if self.global.is_none() {
            self.global = Some(trip);
        }
    }

    /// Stop one strategy or instrument.
    pub fn trip_scope(
        &mut self,
        scope: impl Into<String>,
        at: Timestamp,
        tripped_by: impl Into<String>,
        reason: impl Into<String>,
    ) {
        let scope = scope.into();
        let trip = KillSwitchTrip {
            at,
            tripped_by: tripped_by.into(),
            reason: reason.into(),
            scope: Some(scope.clone()),
        };
        self.history.push(trip.clone());
        self.scoped.entry(scope).or_insert(trip);
    }

    pub fn is_globally_tripped(&self) -> bool {
        self.global.is_some()
    }

    /// Whether trading is halted for a given scope, globally or specifically.
    pub fn is_halted(&self, scope: &str) -> bool {
        self.global.is_some() || self.scoped.contains_key(scope)
    }

    pub fn global_trip(&self) -> Option<&KillSwitchTrip> {
        self.global.as_ref()
    }

    pub fn scoped_trip(&self, scope: &str) -> Option<&KillSwitchTrip> {
        self.scoped.get(scope)
    }

    /// Every trip that has ever happened, including cleared ones.
    pub fn history(&self) -> &[KillSwitchTrip] {
        &self.history
    }

    /// Clear the global stop.
    ///
    /// Refuses a stale credential and records who lifted it. Clearing a stop
    /// that is not set is not an error — it is the idempotent case an operator
    /// retrying a request will hit — but it records nothing either, because
    /// nothing happened.
    pub fn clear_global(&mut self, operator: &OperatorIdentity, at: Timestamp) -> Result<()> {
        let Some(trip) = self.global.clone() else {
            return Ok(());
        };
        Self::check_credential(operator, at)?;
        self.global = None;
        self.clearances.push(KillSwitchClearance {
            at,
            operator: operator.subject().to_string(),
            method: operator.method().to_string(),
            scope: None,
            cleared: trip,
        });
        Ok(())
    }

    /// Clear one scope. Same rules as [`KillSwitch::clear_global`].
    pub fn clear_scope(
        &mut self,
        scope: &str,
        operator: &OperatorIdentity,
        at: Timestamp,
    ) -> Result<()> {
        let Some(trip) = self.scoped.get(scope).cloned() else {
            return Ok(());
        };
        Self::check_credential(operator, at)?;
        self.scoped.remove(scope);
        self.clearances.push(KillSwitchClearance {
            at,
            operator: operator.subject().to_string(),
            method: operator.method().to_string(),
            scope: Some(scope.to_string()),
            cleared: trip,
        });
        Ok(())
    }

    /// Every halt that has been lifted, and by whom.
    pub fn clearances(&self) -> &[KillSwitchClearance] {
        &self.clearances
    }

    fn check_credential(operator: &OperatorIdentity, at: Timestamp) -> Result<()> {
        if operator.is_fresh(at, MAXIMUM_CLEARANCE_CREDENTIAL_AGE) {
            return Ok(());
        }
        Err(Error::denied(format!(
            "operator {} authenticated too long ago to lift a halt; \
             re-authenticate and try again",
            operator.subject()
        )))
    }

    /// Scopes currently halted, excluding a global stop.
    pub fn halted_scopes(&self) -> Vec<&str> {
        self.scoped.keys().map(String::as_str).collect()
    }
}

/// One recorded change of autonomy level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutonomyChange {
    pub at: Timestamp,
    pub from: AutonomyLevel,
    pub to: AutonomyLevel,
    pub operator: String,
    pub second_approver: Option<String>,
    pub reason: String,
}

/// Holds and guards the autonomy level.
#[derive(Debug)]
pub struct AutonomyController {
    level: AutonomyLevel,
    /// The highest level this deployment is configured to permit.
    ///
    /// A separate, lower ceiling from the enum's maximum, so a deployment can
    /// be provably incapable of live trading regardless of what an operator
    /// asks for.
    ceiling: AutonomyLevel,
    kill_switch: KillSwitch,
    history: Vec<AutonomyChange>,
    /// How recently an operator must have authenticated.
    maximum_credential_age: qip_core::Duration,
}

impl Default for AutonomyController {
    fn default() -> Self {
        Self::new()
    }
}

impl AutonomyController {
    /// A controller at the default level with a paper-trading ceiling.
    ///
    /// The ceiling starts at paper trading, so a deployment that has not
    /// explicitly been configured for live trading cannot reach it even with
    /// an authenticated operator. Raising the ceiling is a deployment-time
    /// decision, not a runtime one.
    pub fn new() -> Self {
        Self {
            level: AutonomyLevel::DEFAULT,
            ceiling: AutonomyLevel::PaperTrading,
            kill_switch: KillSwitch::new(),
            history: Vec::new(),
            maximum_credential_age: qip_core::Duration::from_mins(15),
        }
    }

    /// A controller whose ceiling permits live trading.
    ///
    /// Only a deployment configuration should call this, and the fact that it
    /// is a distinct constructor is the point: a live-capable platform is
    /// visibly different from a default one at the call site.
    pub fn with_live_ceiling(ceiling: AutonomyLevel) -> Self {
        Self {
            ceiling,
            ..Self::new()
        }
    }

    pub fn level(&self) -> AutonomyLevel {
        // A tripped kill switch overrides the configured level. This is
        // checked on read rather than by mutating the level, so clearing the
        // switch restores exactly what was there before.
        if self.kill_switch.is_globally_tripped() {
            return AutonomyLevel::Observation;
        }
        self.level
    }

    /// The level as configured, ignoring the kill switch.
    pub fn configured_level(&self) -> AutonomyLevel {
        self.level
    }

    pub fn ceiling(&self) -> AutonomyLevel {
        self.ceiling
    }

    pub fn kill_switch(&self) -> &KillSwitch {
        &self.kill_switch
    }

    pub fn kill_switch_mut(&mut self) -> &mut KillSwitch {
        &mut self.kill_switch
    }

    pub fn history(&self) -> &[AutonomyChange] {
        &self.history
    }

    /// Whether execution is permitted for a scope right now.
    pub fn may_execute(&self, scope: &str) -> bool {
        !self.kill_switch.is_halted(scope) && self.level().executes()
    }

    /// Whether orders would reach a real venue right now.
    pub fn is_live(&self) -> bool {
        self.level().is_live()
    }

    /// Change the level. The only path, and it requires an operator.
    ///
    /// Every guard here exists because its absence would be a way for the
    /// platform to end up trading live without anyone deciding that it should.
    pub fn request_change(
        &mut self,
        to: AutonomyLevel,
        operator: &OperatorIdentity,
        reason: impl Into<String>,
        now: Timestamp,
    ) -> Result<()> {
        let reason = reason.into();
        if reason.trim().len() < 10 {
            return Err(Error::denied(
                "an autonomy change needs a stated reason; the audit trail is the point",
            ));
        }
        if !operator.is_fresh(now, self.maximum_credential_age) {
            return Err(Error::denied(format!(
                "operator {} authenticated more than {:?} ago; re-authenticate to change the autonomy level",
                operator.subject(),
                self.maximum_credential_age
            )));
        }
        if to > self.ceiling {
            return Err(Error::denied(format!(
                "this deployment's ceiling is {}, so {} cannot be reached; raising the ceiling is a deployment change, not a runtime one",
                self.ceiling, to
            )));
        }
        // Going live needs two people. One operator with a compromised session
        // should not be able to turn on live trading.
        if to.is_live() && !self.level.is_live() && !operator.has_second_approver() {
            return Err(Error::denied(format!(
                "moving to {to} requires a second approver; one operator cannot enable live trading alone"
            )));
        }
        // A raise is refused while the kill switch is tripped: clear the stop
        // first, deliberately, then raise.
        if self.kill_switch.is_globally_tripped() && to > self.level {
            return Err(Error::denied(format!(
                "the kill switch is tripped ({}); clear it before raising the autonomy level",
                self.kill_switch
                    .global_trip()
                    .map(|t| t.reason.as_str())
                    .unwrap_or("no reason recorded")
            )));
        }

        self.history.push(AutonomyChange {
            at: now,
            from: self.level,
            to,
            operator: operator.subject().to_string(),
            second_approver: operator.second_approver().map(str::to_string),
            reason,
        });
        self.level = to;
        Ok(())
    }

    /// Drop to a lower level. Needs no operator and no reason beyond the one
    /// given, because reducing autonomy is always safe.
    pub fn reduce_to(&mut self, to: AutonomyLevel, reason: impl Into<String>, now: Timestamp) {
        if to >= self.level {
            return;
        }
        self.history.push(AutonomyChange {
            at: now,
            from: self.level,
            to,
            operator: "system".to_string(),
            second_approver: None,
            reason: reason.into(),
        });
        self.level = to;
    }
}
