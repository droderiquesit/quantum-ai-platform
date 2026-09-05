//! Corridors: where capital may move, under what caps, and where each one is
//! in the life §37.1 gives it.
//!
//! A corridor is `source -> destination | asset | allowlisted address` plus
//! six caps: per transfer, per hour, per day, cumulative, minimum interval and
//! permitted hours. Humans approve a corridor once, slowly, with a signature
//! and a day's delay; machines then decide continuously, inside the caps, when
//! money moves along it. Nothing in this module moves anything. It is the
//! record the [`crate::gate`] checks a transfer intent against — "the signed
//! definition, not a cached copy" — and the transition table that stops a
//! revoked corridor from being walked back to active by a mis-ordered event.
//!
//! # The delay applies to destinations, not caps (§37.2)
//!
//! Tightening a cap on an active corridor is immediate: it cannot change where
//! money goes, only how much, and a human who wants less to move should get it
//! at once. Loosening a cap re-enters the delay here, which is stricter than
//! the blueprint's "two approvals, no delay". That choice is deliberate and
//! stated: a looser cap widens the blast radius of a compromised engine, this
//! platform fails closed by rule, and relaxing that to the blueprint's letter
//! is a decision for whoever approves step 10 of ADR 0023, not for this crate.

use crate::custody::{CorridorKind, CustodyClass};
use crate::destination::{ACTIVATION_DELAY, Approver, DestinationKey, SignatureRecord, describe};
use crate::location::CapitalLocation;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Names a corridor in the ledger and in every refusal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CorridorId(String);

impl CorridorId {
    /// Name a corridor. Refuses an empty name, which would make every refusal
    /// about it unattributable.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid("a corridor needs a name"));
        }
        Ok(Self(name))
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CorridorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The hours of the day (UTC) during which a corridor may carry a transfer.
///
/// A half-open window `[start, end)` in whole hours. Refuses an empty or
/// inverted window, because a corridor that permits no hour is a corridor
/// that can never fire and would still read as a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermittedHours {
    start: u32,
    end: u32,
}

impl PermittedHours {
    /// Every hour of the day.
    pub const ALL_DAY: Self = Self { start: 0, end: 24 };

    /// A window from `start` (inclusive) to `end` (exclusive), both in `0..=24`.
    pub fn new(start: u32, end: u32) -> Result<Self> {
        if end > 24 {
            return Err(Error::invalid(format!(
                "permitted hours end at {end}, past the 24 hours a day has"
            )));
        }
        if start >= end {
            return Err(Error::invalid(format!(
                "permitted hours {start}..{end} is empty or inverted; a corridor that permits \
                 no hour can never carry a transfer"
            )));
        }
        Ok(Self { start, end })
    }

    /// First permitted hour.
    pub fn start(&self) -> u32 {
        self.start
    }

    /// First hour after the window.
    pub fn end(&self) -> u32 {
        self.end
    }

    /// Whether `at` falls inside the window.
    pub fn permits(&self, at: Timestamp) -> bool {
        let (hour, _, _, _) = at.civil_time();
        hour >= self.start && hour < self.end
    }

    /// Whether this window admits an hour the other does not.
    pub fn is_wider_than(&self, other: &Self) -> bool {
        self.start < other.start || self.end > other.end
    }
}

impl fmt::Display for PermittedHours {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:00-{:02}:00 UTC", self.start, self.end)
    }
}

/// The six caps a corridor's signature covers.
///
/// Money is [`Decimal`]; the interval is a [`Duration`]. Validated on
/// construction so a corridor cannot carry a cap set in which one limit can
/// never fire: a per-transfer cap above the hourly cap, say, reads as a
/// control and is not one, which is the template `risk-and-execution.md`
/// names as the thing not to add.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorridorCaps {
    max_per_transfer: Decimal,
    max_per_hour: Decimal,
    max_per_day: Decimal,
    max_cumulative: Decimal,
    min_interval: Duration,
    permitted_hours: PermittedHours,
}

impl CorridorCaps {
    /// Build a cap set, refusing by name any value that is not positive or any
    /// pair that is out of order.
    ///
    /// The required order is per-transfer ≤ hourly ≤ daily ≤ cumulative. Each
    /// inequality is what makes the next limit reachable: an hourly cap below
    /// the per-transfer cap means no single permitted transfer ever fits in an
    /// hour, and a daily cap below the hourly one means the hourly cap can
    /// never be the binding one.
    pub fn new(
        max_per_transfer: Decimal,
        max_per_hour: Decimal,
        max_per_day: Decimal,
        max_cumulative: Decimal,
        min_interval: Duration,
        permitted_hours: PermittedHours,
    ) -> Result<Self> {
        for (name, value) in [
            ("max_per_transfer", max_per_transfer),
            ("max_per_hour", max_per_hour),
            ("max_per_day", max_per_day),
            ("max_cumulative", max_cumulative),
        ] {
            if !value.is_positive() {
                return Err(Error::invalid(format!(
                    "{name} is {value}; every corridor cap must be positive, because a cap of \
                     nothing is a corridor that can never fire and still reads as a control"
                )));
            }
        }
        if min_interval < Duration::ZERO {
            return Err(Error::invalid(format!(
                "min_interval is {min_interval:?}; a negative interval is not an interval"
            )));
        }
        for (lower_name, lower, upper_name, upper) in [
            (
                "max_per_transfer",
                max_per_transfer,
                "max_per_hour",
                max_per_hour,
            ),
            ("max_per_hour", max_per_hour, "max_per_day", max_per_day),
            ("max_per_day", max_per_day, "max_cumulative", max_cumulative),
        ] {
            if lower > upper {
                return Err(Error::invalid(format!(
                    "{lower_name} ({lower}) exceeds {upper_name} ({upper}); caps must satisfy \
                     per-transfer <= hourly <= daily <= cumulative, or the wider one can never \
                     be the binding limit"
                )));
            }
        }
        Ok(Self {
            max_per_transfer,
            max_per_hour,
            max_per_day,
            max_cumulative,
            min_interval,
            permitted_hours,
        })
    }

    /// The most one transfer may carry.
    pub fn max_per_transfer(&self) -> Decimal {
        self.max_per_transfer
    }

    /// The most a rolling hour may carry.
    pub fn max_per_hour(&self) -> Decimal {
        self.max_per_hour
    }

    /// The most a rolling day may carry.
    pub fn max_per_day(&self) -> Decimal {
        self.max_per_day
    }

    /// The most the corridor may ever carry.
    pub fn max_cumulative(&self) -> Decimal {
        self.max_cumulative
    }

    /// The least time between two transfers.
    pub fn min_interval(&self) -> Duration {
        self.min_interval
    }

    /// The hours during which a transfer may be assessed.
    pub fn permitted_hours(&self) -> PermittedHours {
        self.permitted_hours
    }

    /// Whether any dimension of `self` admits more than `other` does.
    ///
    /// Any: a cap set that raises one limit and lowers five is a loosening,
    /// because the raised limit is the one an attacker would use. Only a set
    /// that admits nothing more in every dimension counts as tight.
    pub fn is_looser_than(&self, other: &Self) -> bool {
        self.max_per_transfer > other.max_per_transfer
            || self.max_per_hour > other.max_per_hour
            || self.max_per_day > other.max_per_day
            || self.max_cumulative > other.max_cumulative
            || self.min_interval < other.min_interval
            || self.permitted_hours.is_wider_than(&other.permitted_hours)
    }
}

/// Where a corridor sits in §37.1's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorridorStage {
    /// Full parameters and a stated purpose, from the optimiser or a human.
    Proposed,
    /// A human verified the destination out of band with the institution.
    Reviewed,
    /// A hardware-key signature record covers the destination and every cap.
    Signed,
    /// The system is holding it for [`ACTIVATION_DELAY`] before activation.
    TimeDelayed,
    /// Every transfer is checked against the signed definition.
    Active,
    /// Halted instantly by any anomaly or any human. Reactivation needs
    /// approval; suspension does not.
    Suspended,
    /// Permanent and immediate. Terminal.
    Revoked,
}

impl CorridorStage {
    /// The stage name, for refusals and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Reviewed => "reviewed",
            Self::Signed => "signed",
            Self::TimeDelayed => "time_delayed",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Revoked => "revoked",
        }
    }

    /// Move to `next`, refusing anything not on the legal-edge table.
    ///
    /// One `matches!` so the legal moves read as a table, the discipline
    /// `qip_portfolio`'s position lifecycle uses. The refusals are the point:
    /// a revoked corridor that a late, mis-ordered event walks back to active
    /// is a corridor money can leave through after a human said it could not.
    ///
    /// `Active -> TimeDelayed` is the loosened-cap edge from §37.2 as this
    /// crate reads it; `Suspended -> Active` is reactivation and needs the
    /// delay to have elapsed, which [`Corridor::reactivate`] checks because
    /// the table cannot see a clock.
    pub fn transition(&self, next: Self) -> Result<Self> {
        let legal = matches!(
            (self, &next),
            (Self::Proposed, Self::Reviewed)
                | (Self::Proposed, Self::Revoked)
                | (Self::Reviewed, Self::Signed)
                | (Self::Reviewed, Self::Revoked)
                | (Self::Signed, Self::TimeDelayed)
                | (Self::Signed, Self::Revoked)
                | (Self::TimeDelayed, Self::Active)
                | (Self::TimeDelayed, Self::Suspended)
                | (Self::TimeDelayed, Self::Revoked)
                | (Self::Active, Self::TimeDelayed)
                | (Self::Active, Self::Suspended)
                | (Self::Active, Self::Revoked)
                | (Self::Suspended, Self::Active)
                | (Self::Suspended, Self::Revoked)
        );
        if !legal {
            return Err(Error::invalid(format!(
                "corridor lifecycle cannot move from {} to {}",
                self.as_str(),
                next.as_str()
            )));
        }
        Ok(next)
    }
}

/// What a signature covered: the destination and every cap, at signing time.
///
/// Kept beside the signature record rather than reconstructed from the
/// corridor, so the gate can check a transfer against what was signed rather
/// than against whatever the corridor's caps say now. A tightened cap is
/// still inside the signed one; a loosened cap without a fresh signature is
/// the discrepancy this exists to expose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDefinition {
    /// The destination the signature covers.
    pub destination: DestinationKey,
    /// The caps the signature covers.
    pub caps: CorridorCaps,
    /// The record of the signature itself.
    pub signature: SignatureRecord,
}

/// One stage change, kept so the corridor's history is in the record rather
/// than in whoever remembers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageChange {
    /// The stage left.
    pub from: CorridorStage,
    /// The stage entered.
    pub to: CorridorStage,
    /// Who caused it, or `None` where the system did on its own clock.
    pub by: Option<Approver>,
    /// When.
    pub at: Timestamp,
    /// Why, in the actor's words.
    pub reason: String,
}

/// A corridor: the record of where money may go, and under what caps.
///
/// Every mutation is one edge of [`CorridorStage::transition`] plus whatever
/// that edge requires — a reviewer, a signature record, an elapsed delay — and
/// every refusal names what would satisfy it. There is no method here that
/// moves, signs or calls out; the corridor is what the gate reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Corridor {
    id: CorridorId,
    source: CapitalLocation,
    source_class: CustodyClass,
    kind: CorridorKind,
    destination: DestinationKey,
    caps: CorridorCaps,
    purpose: String,
    stage: CorridorStage,
    proposed_by: Approver,
    proposed_at: Timestamp,
    reviewed: Option<(Approver, Timestamp)>,
    signed: Option<SignedDefinition>,
    activation_at: Option<Timestamp>,
    history: Vec<StageChange>,
}

impl Corridor {
    /// Propose a corridor with full parameters and a stated purpose.
    ///
    /// Refuses an empty purpose: §37.1 requires one at proposal, and a
    /// corridor nobody could say the reason for is one nobody will notice is
    /// wrong.
    ///
    /// `source_class` and `kind` are what [`crate::custody::CustodyPolicy`]
    /// is asked about at every assessment: which §37.4 class the source
    /// balance is held in, and which kind of corridor this is. Recorded at
    /// proposal so the gate checks the pairing a human reviewed rather than
    /// one the caller names at assessment time.
    pub fn propose(
        id: CorridorId,
        source: CapitalLocation,
        source_class: CustodyClass,
        kind: CorridorKind,
        destination: DestinationKey,
        caps: CorridorCaps,
        purpose: impl Into<String>,
        by: Approver,
        at: Timestamp,
    ) -> Result<Self> {
        let purpose = purpose.into();
        if purpose.trim().is_empty() {
            return Err(Error::invalid(format!(
                "corridor {id} needs a stated purpose; §37.1 requires one at proposal"
            )));
        }
        Ok(Self {
            id,
            source,
            source_class,
            kind,
            destination,
            caps,
            purpose,
            stage: CorridorStage::Proposed,
            proposed_by: by,
            proposed_at: at,
            reviewed: None,
            signed: None,
            activation_at: None,
            history: Vec::new(),
        })
    }

    /// The corridor's name.
    pub fn id(&self) -> &CorridorId {
        &self.id
    }

    /// Where money would leave from.
    pub fn source(&self) -> &CapitalLocation {
        &self.source
    }

    /// The §37.4 custody class the source balance is held in.
    pub fn source_class(&self) -> CustodyClass {
        self.source_class
    }

    /// Which kind of corridor this is, in §37.4's terms.
    pub fn kind(&self) -> CorridorKind {
        self.kind
    }

    /// Where it would go.
    pub fn destination(&self) -> &DestinationKey {
        &self.destination
    }

    /// The caps in force now — which may be tighter than the signed ones.
    pub fn caps(&self) -> &CorridorCaps {
        &self.caps
    }

    /// The purpose stated at proposal.
    pub fn purpose(&self) -> &str {
        &self.purpose
    }

    /// Where it is in its life.
    pub fn stage(&self) -> CorridorStage {
        self.stage
    }

    /// Who proposed it, and when.
    pub fn proposed(&self) -> (&Approver, Timestamp) {
        (&self.proposed_by, self.proposed_at)
    }

    /// Who reviewed it, and when, if anyone has.
    pub fn reviewed(&self) -> Option<(&Approver, Timestamp)> {
        self.reviewed.as_ref().map(|(by, at)| (by, *at))
    }

    /// What was signed, if anything has been.
    pub fn signed(&self) -> Option<&SignedDefinition> {
        self.signed.as_ref()
    }

    /// The instant the current delay ends, if one is or was running.
    pub fn activation_at(&self) -> Option<Timestamp> {
        self.activation_at
    }

    /// Every stage change so far, oldest first.
    pub fn history(&self) -> &[StageChange] {
        &self.history
    }

    /// Record that a human verified the destination out of band.
    ///
    /// Refuses the proposer as reviewer: the review exists to put a second
    /// person between a proposal and a signature, and one person filling both
    /// roles is no review.
    pub fn review(&mut self, by: Approver, at: Timestamp) -> Result<()> {
        if by == self.proposed_by {
            return Err(Error::denied(format!(
                "corridor {} was proposed by {by}, who cannot also review it; the review is a \
                 second person's out-of-band check",
                self.id
            )));
        }
        self.step(CorridorStage::Reviewed, Some(by.clone()), at, "reviewed")?;
        self.reviewed = Some((by, at));
        Ok(())
    }

    /// Record a hardware-key signature covering the destination and every cap.
    ///
    /// The record's coverage is snapshotted from the corridor as it is now, so
    /// the gate later compares a transfer against exactly what was signed.
    pub fn record_signature(&mut self, signature: SignatureRecord) -> Result<()> {
        let at = signature.signed_at;
        if let Some((_, reviewed_at)) = &self.reviewed
            && at < *reviewed_at
        {
            return Err(Error::invalid(format!(
                "corridor {} was reviewed at {reviewed_at} but the signature is dated {at}; \
                 a signature that predates review covered an unreviewed destination",
                self.id
            )));
        }
        self.step(
            CorridorStage::Signed,
            Some(signature.signer.clone()),
            at,
            "signature recorded",
        )?;
        self.signed = Some(SignedDefinition {
            destination: self.destination.clone(),
            caps: self.caps.clone(),
            signature,
        });
        Ok(())
    }

    /// Start the activation delay. System action, on the platform clock.
    pub fn begin_delay(&mut self, now: Timestamp) -> Result<Timestamp> {
        let activation_at = now.saturating_add(ACTIVATION_DELAY);
        self.step(
            CorridorStage::TimeDelayed,
            None,
            now,
            format!("delay of {} begun", describe(ACTIVATION_DELAY)),
        )?;
        self.activation_at = Some(activation_at);
        Ok(activation_at)
    }

    /// Activate, refusing while the delay is still running.
    ///
    /// System action. Refuses rather than waits: the caller supplies the
    /// clock, and a corridor that activated itself early because a test
    /// passed a generous `now` is exactly what a replay would catch — so the
    /// refusal names the instant it would have accepted.
    pub fn activate(&mut self, now: Timestamp) -> Result<()> {
        let activation_at = self.activation_at.ok_or_else(|| {
            Error::invalid(format!(
                "corridor {} has no activation time; the delay has not begun",
                self.id
            ))
        })?;
        if now < activation_at {
            return Err(Error::denied(format!(
                "corridor {} activates at {activation_at}, and it is {now}; the {} delay after \
                 signing has not elapsed",
                self.id,
                describe(ACTIVATION_DELAY)
            )));
        }
        self.step(CorridorStage::Active, None, now, "delay elapsed")
    }

    /// Suspend instantly. Any anomaly, or any human; no approval needed.
    pub fn suspend(
        &mut self,
        by: Option<Approver>,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<()> {
        self.step(CorridorStage::Suspended, by, at, reason)
    }

    /// Reactivate a suspended corridor, with approval.
    ///
    /// Refuses while the delay that was running at suspension has still not
    /// elapsed: a corridor suspended during its delay must not use
    /// reactivation as a way around it.
    pub fn reactivate(&mut self, by: Approver, now: Timestamp) -> Result<()> {
        if let Some(activation_at) = self.activation_at
            && now < activation_at
        {
            return Err(Error::denied(format!(
                "corridor {} was suspended during its delay, which ends at {activation_at}; \
                 reactivation cannot shorten it and it is {now}",
                self.id
            )));
        }
        self.step(
            CorridorStage::Active,
            Some(by),
            now,
            "reactivated with approval",
        )
    }

    /// Revoke permanently. Any human, from any stage but revoked.
    pub fn revoke(&mut self, by: Approver, reason: impl Into<String>, at: Timestamp) -> Result<()> {
        self.step(CorridorStage::Revoked, Some(by), at, reason)
    }

    /// Replace the caps with a set that admits nothing more in any dimension.
    /// Immediate, from any live stage, with one human.
    ///
    /// Refuses a set that loosens anything, naming [`Corridor::loosen_caps`]
    /// as the path that does. A tightening that silently accepted one raised
    /// limit would be the delay-free path an attacker with one credential
    /// would use.
    pub fn tighten_caps(&mut self, caps: CorridorCaps, by: Approver, at: Timestamp) -> Result<()> {
        if caps.is_looser_than(&self.caps) {
            return Err(Error::denied(format!(
                "corridor {}: the proposed caps admit more than the current ones in at least \
                 one dimension; a loosening needs a fresh signature record and re-enters the \
                 delay through loosen_caps",
                self.id
            )));
        }
        self.require_live("tighten caps on")?;
        self.caps = caps;
        self.history.push(StageChange {
            from: self.stage,
            to: self.stage,
            by: Some(by),
            at,
            reason: "caps tightened".to_string(),
        });
        Ok(())
    }

    /// Replace the caps with a set that admits more in at least one dimension.
    ///
    /// Requires a fresh signature record covering the new caps and re-enters
    /// the delay: the corridor moves from active to time-delayed and activates
    /// only once [`ACTIVATION_DELAY`] has passed on the platform clock. Refuses
    /// a set that loosens nothing, because calling this with a tightening
    /// would spend a signature and a day on a change that needed neither, and
    /// the mismatch means the caller is confused about which change it made.
    pub fn loosen_caps(
        &mut self,
        caps: CorridorCaps,
        signature: SignatureRecord,
        now: Timestamp,
    ) -> Result<Timestamp> {
        if !caps.is_looser_than(&self.caps) {
            return Err(Error::invalid(format!(
                "corridor {}: the proposed caps admit nothing more than the current ones; a \
                 tightening is immediate through tighten_caps and needs no signature",
                self.id
            )));
        }
        if self.stage != CorridorStage::Active {
            return Err(Error::denied(format!(
                "corridor {} is {} and only an active corridor's caps can be loosened",
                self.id,
                self.stage.as_str()
            )));
        }
        let activation_at = now.saturating_add(ACTIVATION_DELAY);
        self.step(
            CorridorStage::TimeDelayed,
            Some(signature.signer.clone()),
            now,
            format!(
                "caps loosened; delay of {} re-entered",
                describe(ACTIVATION_DELAY)
            ),
        )?;
        self.caps = caps.clone();
        self.signed = Some(SignedDefinition {
            destination: self.destination.clone(),
            caps,
            signature,
        });
        self.activation_at = Some(activation_at);
        Ok(activation_at)
    }

    fn require_live(&self, action: &str) -> Result<()> {
        match self.stage {
            CorridorStage::Revoked => Err(Error::denied(format!(
                "cannot {action} corridor {}: it is revoked, and revocation is permanent",
                self.id
            ))),
            _ => Ok(()),
        }
    }

    fn step(
        &mut self,
        to: CorridorStage,
        by: Option<Approver>,
        at: Timestamp,
        reason: impl Into<String>,
    ) -> Result<()> {
        let from = self.stage;
        self.stage = from
            .transition(to)
            .map_err(|err| Error::invalid(format!("corridor {}: {}", self.id, err.message())))?;
        self.history.push(StageChange {
            from,
            to,
            by,
            at,
            reason: reason.into(),
        });
        Ok(())
    }
}
