//! The transfer gate: seven deterministic checks, each of which can only veto.
//!
//! Blueprint §37.3 puts a veto-only gate between a machine-generated transfer
//! intent and anything that would act on it. This module is that gate with
//! nothing behind it. [`TransferGate::assess`] takes an intent, the corridor
//! it claims, the allowlist, the custody table, the three enforcement points'
//! attestations, the Intelligence layer's ruling on the corridor, the
//! balances, the velocity state and the kill-switch state — every one of them
//! supplied by the caller, with the platform clock — and returns either an
//! [`Approved`] record or a [`Vetoed`] record naming the check that failed and
//! what would satisfy it.
//!
//! Blueprint §2 gives the Intelligence layer "sets risk and corridor policy",
//! and this gate is where that policy stops being a document. A
//! [`CorridorFunding`] is what that layer derived from the rungs the corridor's
//! strategies stand on; the gate refuses a suspended corridor in check 1 and
//! holds a narrowed one to its ceiling in check 2. The fabric does not derive
//! it — the lifecycle is a different service, and the two meet in `qip-kernel`.
//! Before this input existed, the fabric held corridors as records that nothing
//! measured against a policy, which is a control with nothing to control.
//!
//! §37.4's closing rule rides inside check 1 rather than becoming an eighth
//! check, for the same reason the custody table already does: §37.3 names
//! seven checks, and the question "may this corridor carry this at all" is
//! [`GateCheck::CorridorAuthority`]'s whether the answer comes from the
//! corridor's own signature, from the allowlist, from the custody table, or
//! from who attested to it. §37.4's unconditional rules about the table
//! itself ([`crate::custody::CustodyPolicy::conforms`]) are asked there too,
//! for the reason that method gives: every input to this gate arrives
//! deserialised on a replay, so a rule only a constructor holds is a rule the
//! replay never re-derives.
//!
//! An [`Approved`] carries no way to execute. There is no transfer engine in
//! this crate, no method that takes an `Approved` and does something with it,
//! and ADR 0021 refuses building one. A gate with no engine behind it is not
//! a stub; it is the control, and the control is the half worth having. It
//! can be exercised against the simulator, produces evidence a person can
//! check, and cannot cause a payment.
//!
//! # Why each input is a value the caller supplies
//!
//! The gate reads no ledger, no clock and no switch of its own. What it knows
//! about the world arrives as arguments, so a replay from the event log can
//! hand it the same arguments and get the same veto — and so that no path
//! exists by which the gate could learn something the log did not record.

use crate::corridor::{Corridor, CorridorStage};
use crate::custody::{Agreement, CustodyPolicy, TransferAuthority};
use crate::destination::{DestinationKey, DestinationRegistry};
use crate::location::CapitalLocation;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The rolling hour a corridor's hourly cap is measured over.
const HOUR: Duration = Duration::from_hours(1);
/// The rolling day a corridor's daily cap is measured over.
const DAY: Duration = Duration::from_days(1);

/// The reason a transfer is proposed, as arithmetic rather than prose.
///
/// §37.3 requires that a transfer reduce deviation from the optimiser's
/// target, and vetoes one that does not with "no transfer without a stated
/// purpose". The purpose is therefore two numbers the caller computed from
/// its target — the deviation now and the deviation the transfer would leave —
/// and the gate checks the second is strictly smaller. A purpose stated as a
/// sentence could say anything; a purpose stated as a reduction can be false.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatedPurpose {
    deviation_before: Decimal,
    deviation_after: Decimal,
}

impl StatedPurpose {
    /// State a purpose. Both deviations are distances from target and must be
    /// non-negative; a negative distance is a sign error in the caller.
    pub fn new(deviation_before: Decimal, deviation_after: Decimal) -> Result<Self> {
        if deviation_before.is_negative() || deviation_after.is_negative() {
            return Err(Error::invalid(format!(
                "a deviation from target is a distance and cannot be negative (before \
                 {deviation_before}, after {deviation_after})"
            )));
        }
        Ok(Self {
            deviation_before,
            deviation_after,
        })
    }

    /// Distance from target before the transfer.
    pub fn deviation_before(&self) -> Decimal {
        self.deviation_before
    }

    /// Distance from target the transfer would leave.
    pub fn deviation_after(&self) -> Decimal {
        self.deviation_after
    }

    /// Whether the transfer would bring the book strictly closer to target.
    pub fn reduces_deviation(&self) -> bool {
        self.deviation_after < self.deviation_before
    }
}

/// A machine-generated request to move capital. A record, never a movement.
///
/// Carries where from, where to, how much and why. It has no method that
/// does anything, and the only thing that reads it is [`TransferGate`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferIntent {
    source: CapitalLocation,
    destination: DestinationKey,
    amount: Decimal,
    purpose: StatedPurpose,
}

impl TransferIntent {
    /// Record an intent. Refuses a non-positive amount: there is no such thing
    /// as a transfer of nothing, and a negative one is a recall wearing the
    /// wrong type.
    pub fn new(
        source: CapitalLocation,
        destination: DestinationKey,
        amount: Decimal,
        purpose: StatedPurpose,
    ) -> Result<Self> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "a transfer intent needs a positive amount, not {amount}; capital is recalled \
                 through qip_capital::RecallOrder, not by a backwards transfer"
            )));
        }
        Ok(Self {
            source,
            destination,
            amount,
            purpose,
        })
    }

    /// Where from.
    pub fn source(&self) -> &CapitalLocation {
        &self.source
    }

    /// Where to.
    pub fn destination(&self) -> &DestinationKey {
        &self.destination
    }

    /// How much, in the destination's asset.
    pub fn amount(&self) -> Decimal {
        self.amount
    }

    /// Why.
    pub fn purpose(&self) -> StatedPurpose {
        self.purpose
    }
}

/// One transfer the corridor has already carried, as the caller's ledger
/// records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedTransfer {
    /// When it was carried.
    pub at: Timestamp,
    /// How much.
    pub amount: Decimal,
}

/// What a corridor has carried so far, for the rolling caps and the interval.
///
/// Supplied by the caller from its ledger rather than accumulated here, so
/// the gate holds no state that could drift from the record and a replay
/// assesses each intent against the history the log says existed at the time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferHistory {
    carried: Vec<CarriedTransfer>,
}

impl TransferHistory {
    /// A corridor that has carried nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a history. Refuses a non-positive amount, which would make the
    /// cumulative cap count a refund it never saw.
    pub fn new(carried: Vec<CarriedTransfer>) -> Result<Self> {
        let mut carried = carried;
        carried.sort_by_key(|transfer| transfer.at);
        let history = Self { carried };
        history.well_formed().map_err(Error::invalid)?;
        Ok(history)
    }

    /// Whether this history is one the caps and the interval can be measured
    /// against: every amount positive, and oldest first.
    ///
    /// Re-derived by [`TransferGate::assess`] for the reason
    /// [`crate::custody::CustodyPolicy::conforms`] gives at length — a
    /// `TransferHistory` arrives inside a [`crate::journal::GateCommand`]
    /// deserialised off the log, where [`TransferHistory::new`] never runs.
    /// Both halves are load-bearing and neither is cosmetic: a non-positive
    /// amount makes [`TransferHistory::carried_total`] under-count, so the
    /// cumulative cap admits a transfer that exhausts it, and an
    /// out-of-order list makes [`TransferHistory::last_carried_at`] name a
    /// transfer that is not the last one, so the minimum-interval check
    /// measures from the wrong instant and passes.
    pub fn well_formed(&self) -> std::result::Result<(), String> {
        let mut previous: Option<Timestamp> = None;
        for transfer in &self.carried {
            if !transfer.amount.is_positive() {
                return Err(format!(
                    "a carried transfer at {} of {} is not positive; history records what \
                     left, and nothing else",
                    transfer.at, transfer.amount
                ));
            }
            if let Some(previous) = previous
                && transfer.at < previous
            {
                return Err(format!(
                    "a carried transfer at {} follows one at {previous}; a history is oldest \
                     first, and out of order the last transfer is not the latest one",
                    transfer.at
                ));
            }
            previous = Some(transfer.at);
        }
        Ok(())
    }

    /// Everything carried at or after `since`.
    pub fn carried_since(&self, since: Timestamp) -> Decimal {
        self.carried
            .iter()
            .filter(|transfer| transfer.at >= since)
            .map(|transfer| transfer.amount)
            .sum()
    }

    /// Everything ever carried.
    pub fn carried_total(&self) -> Decimal {
        self.carried.iter().map(|transfer| transfer.amount).sum()
    }

    /// When the corridor last carried anything.
    pub fn last_carried_at(&self) -> Option<Timestamp> {
        self.carried.last().map(|transfer| transfer.at)
    }

    /// The transfers, oldest first.
    pub fn carried(&self) -> &[CarriedTransfer] {
        &self.carried
    }
}

/// The source's balance and every claim already on it.
///
/// §37.3 checks sufficiency *after* reservations, in-flight settlement and
/// commitments, because a balance that ignores them is money promised twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBalances {
    /// What the ledger says is there.
    pub balance: Decimal,
    /// Reserved against open orders and margin.
    pub reserved: Decimal,
    /// Instructed out and not yet settled.
    pub in_flight_settlement: Decimal,
    /// Committed to something not yet instructed.
    pub commitments: Decimal,
}

impl SourceBalances {
    /// Build a balance picture. Refuses a negative claim, which would add to
    /// the free balance rather than subtract from it.
    pub fn new(
        balance: Decimal,
        reserved: Decimal,
        in_flight_settlement: Decimal,
        commitments: Decimal,
    ) -> Result<Self> {
        let balances = Self {
            balance,
            reserved,
            in_flight_settlement,
            commitments,
        };
        balances.well_formed().map_err(Error::invalid)?;
        Ok(balances)
    }

    /// Whether every claim on the balance is a claim rather than a credit.
    ///
    /// Re-derived by [`TransferGate::assess`], because a `SourceBalances`
    /// arrives inside a [`crate::journal::GateCommand`] deserialised off the
    /// log and [`SourceBalances::new`] never runs on that path. A negative
    /// claim is subtracted in [`SourceBalances::free`] and therefore *adds* to
    /// the free balance: the sufficiency check would then admit a transfer of
    /// money the source does not have, which is the one thing check 5 exists
    /// to refuse.
    pub fn well_formed(&self) -> std::result::Result<(), String> {
        for (name, value) in [
            ("reserved", self.reserved),
            ("in_flight_settlement", self.in_flight_settlement),
            ("commitments", self.commitments),
        ] {
            if value.is_negative() {
                return Err(format!(
                    "{name} is {value}; a claim on a balance cannot be negative"
                ));
            }
        }
        Ok(())
    }

    /// What is actually free after every claim.
    pub fn free(&self) -> Decimal {
        self.balance - self.reserved - self.in_flight_settlement - self.commitments
    }
}

/// Whether the velocity breaker has tripped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VelocityBreaker {
    /// Within bounds.
    Armed,
    /// Tripped; nothing moves until a human resets it.
    Tripped,
}

/// Whether the anomaly detector has raised a flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyFlag {
    /// Nothing raised.
    Clear,
    /// Raised; nothing moves until a human clears it.
    Raised,
}

/// The velocity breaker and the anomaly detector, together, as the caller
/// last read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VelocityState {
    /// The breaker.
    pub breaker: VelocityBreaker,
    /// The detector.
    pub anomaly: AnomalyFlag,
}

impl VelocityState {
    /// Breaker armed, detector clear.
    pub const CLEAR: Self = Self {
        breaker: VelocityBreaker::Armed,
        anomaly: AnomalyFlag::Clear,
    };
}

/// The kill switch, as the caller last read it.
///
/// Its own type rather than a `bool` so that a caller cannot pass `true`
/// meaning "yes, proceed" into an argument that reads `true` as "tripped".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillSwitchState {
    /// Not tripped.
    Armed,
    /// Tripped. Vetoes everything.
    Tripped,
}

/// Where the Intelligence layer says the strategies behind a corridor stand.
///
/// The fabric does not derive this and cannot: the rungs live in the strategy
/// lifecycle, which is a different service, and a corridor's standing is a
/// statement about the strategies it funds rather than about the corridor's
/// own record. It arrives as a value the caller derived, like every other
/// input to this gate, and `qip-kernel` is where the two services meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FundingStanding {
    /// Every strategy the corridor funds is on a rung that holds capital at
    /// full size. The corridor may carry up to its stated ceiling.
    Permitted,
    /// The weakest strategy it funds is deliberately limited, so the corridor
    /// is held to a smaller stated ceiling. Not a fault.
    Narrowed,
    /// Something it funds holds no capital at all, so the corridor carries
    /// nothing. **This is the control working, not failing** — the platform
    /// has stopped funding a strategy that no longer holds capital, and an
    /// operator who reads it as a corridor fault will look for the wrong
    /// problem.
    Suspended,
}

impl FundingStanding {
    /// The standing's name, for vetoes and logs.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Permitted => "permitted",
            Self::Narrowed => "narrowed",
            Self::Suspended => "suspended",
        }
    }
}

impl fmt::Display for FundingStanding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The Intelligence layer's ruling on a corridor, as the caller derived it.
///
/// Two figures and the sentence that produced them: what the corridor may
/// carry, and why that is the number. The ceiling is *stated*, never computed
/// here — the fabric would be inventing a cap with no owner, and a veto with
/// no owner is unarguable for the wrong reason.
///
/// Nothing is validated at construction alone, for the reason
/// [`crate::custody::TransferAuthority`] gives at length: this type travels
/// inside [`crate::journal::GateCommand`] and so arrives deserialised off the
/// event log on every replay, where a constructor is a check nothing runs.
/// [`Self::well_formed`] states the rules and [`TransferGate::assess`] asks
/// them again on every assessment, live and replayed alike.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorridorFunding {
    standing: FundingStanding,
    permitted: Decimal,
    reason: String,
}

impl CorridorFunding {
    /// Record a ruling. Refuses what [`Self::well_formed`] refuses, so a
    /// caller building one by hand hears about it at the seam rather than at
    /// the gate.
    pub fn new(
        standing: FundingStanding,
        permitted: Decimal,
        reason: impl Into<String>,
    ) -> Result<Self> {
        let funding = Self {
            standing,
            permitted,
            reason: reason.into(),
        };
        funding.well_formed().map_err(Error::invalid)?;
        Ok(funding)
    }

    /// Whether this ruling is one the gate can act on.
    ///
    /// Every rule here is load-bearing and none is cosmetic. A negative
    /// ceiling would make the funding check in [`TransferGate::caps`] admit
    /// nothing and read as a cap, which is a suspension wearing a cap's
    /// clothes — suspend the corridor instead and say so. A suspended
    /// standing carrying a positive ceiling is two claims about the same fact,
    /// and the louder one would be whichever check happened to be asked
    /// first. An unexplained ruling is a cap an operator cannot trace to a
    /// named rung, which is exactly the veto nobody can argue with.
    ///
    /// A ceiling of **zero** under any standing but [`FundingStanding::Suspended`]
    /// is the same suspension in the same clothes, and refusing only the
    /// negatives left the gap open at the one value a person actually types.
    /// It passed every branch here, passed check 1 — the corridor is not
    /// suspended, so it is admitted — and then refused every transfer ever
    /// proposed at check 2 with "exceeds the narrowed ceiling of 0", which
    /// tells an operator to promote a strategy when the cause is a zero in a
    /// declaration. `CorridorSubject::new` in `qip-lifecycle` refuses a
    /// zero ceiling at the seam a person writes, and this asks the same
    /// question again for the reason the whole type documents: a ruling
    /// reaches this gate deserialised off the event log, where no constructor
    /// runs. Both, not either.
    pub fn well_formed(&self) -> std::result::Result<(), String> {
        if self.permitted.is_negative() {
            return Err(format!(
                "the corridor is {} at {}; a ceiling is a non-negative amount, and a corridor \
                 that may carry nothing is suspended rather than capped below zero",
                self.standing, self.permitted
            ));
        }
        if self.standing != FundingStanding::Suspended && !self.permitted.is_positive() {
            return Err(format!(
                "the corridor is {} and carries a ceiling of {}; a standing that is not suspended \
                 says the corridor may carry something, and a ceiling of zero says it may not. \
                 Suspend it, or state the ceiling the strategies it funds have earned — as it \
                 stands the corridor would be admitted and then refuse every transfer through it",
                self.standing, self.permitted
            ));
        }
        if self.standing == FundingStanding::Suspended && self.permitted.is_positive() {
            return Err(format!(
                "the corridor is suspended and carries a ceiling of {}; a suspended corridor \
                 carries nothing, and the two figures disagree about which is true",
                self.permitted
            ));
        }
        if self.reason.trim().is_empty() {
            return Err(format!(
                "the corridor is {} at {} with no reason given; name the strategy and the rung \
                 that decided it, because a cap an operator cannot trace to one named rung is a \
                 cap nobody can argue with",
                self.standing, self.permitted
            ));
        }
        Ok(())
    }

    /// Permitted, narrowed or suspended.
    pub fn standing(&self) -> FundingStanding {
        self.standing
    }

    /// The most the corridor may carry in one transfer under this ruling.
    /// Zero when suspended, and — since [`Self::well_formed`] refuses the
    /// alternative — only then.
    ///
    /// Per transfer, and deliberately not against the corridor's lifetime
    /// total: a ruling narrows when a strategy's rung moves, and a ceiling
    /// measured against what the corridor has already carried would re-decide
    /// history — a corridor narrowed this morning would refuse everything for
    /// ever because of transfers a wider ruling admitted last year. The
    /// lifetime total is what [`crate::corridor::CorridorCaps::max_cumulative`]
    /// is for, and it is the desk's signed figure rather than a derived one.
    pub fn permitted(&self) -> Decimal {
        self.permitted
    }

    /// Why, in the deriving layer's own words.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The seven checks of §37.3, in the order the gate runs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateCheck {
    /// Corridor active, signature record present and covering the current
    /// definition, destination allowlisted and usable, the custody table
    /// conforming to §37.4's unconditional rules and permitting the class
    /// through this kind of corridor, and §37.4's three enforcement points
    /// agreeing under three identities none of which trades, the venue's own
    /// allowlist mirrored against *this* destination where the class's row
    /// demands it, and the Intelligence layer's ruling on the corridor not
    /// being [`FundingStanding::Suspended`].
    CorridorAuthority,
    /// Within the Intelligence layer's ceiling for the corridor, and within
    /// the per-transfer, hourly, daily and cumulative caps, and inside
    /// permitted hours.
    Caps,
    /// Minimum interval elapsed since the corridor last carried anything.
    MinimumInterval,
    /// Reduces deviation from the optimiser target.
    StatedPurpose,
    /// Source balance sufficient after reservations, in-flight settlement and
    /// commitments.
    SourceBalance,
    /// Velocity breaker not tripped; anomaly detector clear.
    VelocityAndAnomaly,
    /// Kill switch not tripped.
    KillSwitch,
}

impl GateCheck {
    /// Every check, in assessment order.
    pub const ALL: [Self; 7] = [
        Self::CorridorAuthority,
        Self::Caps,
        Self::MinimumInterval,
        Self::StatedPurpose,
        Self::SourceBalance,
        Self::VelocityAndAnomaly,
        Self::KillSwitch,
    ];

    /// The check's name, for refusals and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CorridorAuthority => "corridor_authority",
            Self::Caps => "caps",
            Self::MinimumInterval => "minimum_interval",
            Self::StatedPurpose => "stated_purpose",
            Self::SourceBalance => "source_balance",
            Self::VelocityAndAnomaly => "velocity_and_anomaly",
            Self::KillSwitch => "kill_switch",
        }
    }

    /// Whether §37.3 pairs this check's veto with an alert.
    ///
    /// A corridor failure means something reached the gate that should not
    /// have been generated; a breaker or anomaly means the world changed
    /// under the book. Both are for a person, not just for the log.
    pub fn alerts(&self) -> bool {
        matches!(self, Self::CorridorAuthority | Self::VelocityAndAnomaly)
    }
}

impl fmt::Display for GateCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refused transfer: which check refused it, and what would satisfy it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vetoed {
    /// The check that fired. Checks after it were not run; a veto is a veto.
    pub check: GateCheck,
    /// Why, naming the figures and what would change the answer.
    pub reason: String,
    /// Whether §37.3 pairs this veto with an alert to a person.
    pub alert: bool,
    /// When it was assessed.
    pub assessed_at: Timestamp,
}

impl fmt::Display for Vetoed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vetoed by {}: {}", self.check, self.reason)
    }
}

/// A transfer every check admitted.
///
/// **This type carries no way to execute.** It is a record that, at
/// `assessed_at`, the intent passed all seven checks against the corridor's
/// signed definition — nothing more. There is no method on it, and no
/// function in this crate taking it, that moves capital, signs anything or
/// calls anything outside the process. Under ADR 0021 that is not a gap
/// awaiting an engine; it is the shape the control is required to have.
/// Anyone adding a consumer of this type is building the thing the ADR
/// refuses, and `no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
/// in the acceptance suite is the test that will notice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approved {
    intent: TransferIntent,
    corridor: crate::corridor::CorridorId,
    signature_reference: String,
    assessed_at: Timestamp,
    checks_passed: [GateCheck; 7],
    authority: Agreement,
    funding: CorridorFunding,
}

impl Approved {
    /// The intent that was assessed.
    pub fn intent(&self) -> &TransferIntent {
        &self.intent
    }

    /// The corridor it was assessed against.
    pub fn corridor(&self) -> &crate::corridor::CorridorId {
        &self.corridor
    }

    /// The filing reference of the signature the corridor was checked against,
    /// so the approval can be traced to the signed definition.
    pub fn signature_reference(&self) -> &str {
        &self.signature_reference
    }

    /// When.
    pub fn assessed_at(&self) -> Timestamp {
        self.assessed_at
    }

    /// The checks, in the order they passed. Always all seven.
    pub fn checks_passed(&self) -> &[GateCheck; 7] {
        &self.checks_passed
    }

    /// The three attestations §37.4 required, as the gate found them.
    ///
    /// Kept rather than discarded so an admitted assessment names *which*
    /// three identities agreed. A gate that checked the rule and recorded
    /// only "it held" would leave an operator asking who authorised a
    /// movement with nothing but the gate's own word for it.
    pub fn authority(&self) -> &Agreement {
        &self.authority
    }

    /// The Intelligence layer's ruling the assessment was made under.
    ///
    /// Kept for the reason [`Approved::authority`] is kept: an admitted
    /// assessment names the rung that permitted it, so an operator reading an
    /// approval afterwards can see which standing the corridor was on at the
    /// time rather than which one it is on now.
    pub fn funding(&self) -> &CorridorFunding {
        &self.funding
    }
}

/// The deterministic, veto-only gate.
///
/// A unit struct rather than something with state, so that there is nothing
/// in it a caller could configure to skip a check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransferGate;

impl TransferGate {
    /// Run the seven checks of §37.3 in order and stop at the first veto.
    ///
    /// Stopping at the first is deliberate: the checks are ordered from the
    /// one that means "this should never have been generated" to the one that
    /// means "nothing moves today", and the first failure is the one the
    /// operator needs to hear about. A gate that ran all seven and reported
    /// the kill switch beside a missing signature would bury the finding.
    ///
    /// Every argument is supplied by the caller. The gate reads nothing on
    /// its own.
    #[allow(clippy::too_many_arguments)]
    pub fn assess(
        intent: &TransferIntent,
        corridor: &Corridor,
        registry: &DestinationRegistry,
        custody: &CustodyPolicy,
        authority: &TransferAuthority,
        funding: &CorridorFunding,
        history: &TransferHistory,
        balances: &SourceBalances,
        velocity: VelocityState,
        kill_switch: KillSwitchState,
        now: Timestamp,
    ) -> std::result::Result<Approved, Vetoed> {
        let veto = |check: GateCheck, reason: String| Vetoed {
            alert: check.alerts(),
            check,
            reason,
            assessed_at: now,
        };

        // 1. Corridor active, signature valid, destination allowlisted,
        //    custody table permitting, three enforcement points agreeing, and
        //    the Intelligence layer not having suspended the corridor.
        let (signature_reference, agreement) =
            Self::corridor_authority(intent, corridor, registry, custody, authority, funding, now)
                .map_err(|reason| veto(GateCheck::CorridorAuthority, reason))?;

        // 2. Within the derived ceiling and within the per-transfer, hourly,
        //    daily, cumulative caps and hours.
        Self::caps(intent, corridor, funding, history, now)
            .map_err(|reason| veto(GateCheck::Caps, reason))?;

        // 3. Minimum interval elapsed.
        Self::minimum_interval(corridor, history, now)
            .map_err(|reason| veto(GateCheck::MinimumInterval, reason))?;

        // 4. Reduces deviation from optimiser target.
        let purpose = intent.purpose();
        if !purpose.reduces_deviation() {
            return Err(veto(
                GateCheck::StatedPurpose,
                format!(
                    "no transfer without a stated purpose: deviation from target would be {} \
                     after against {} before, which is not a reduction; a transfer must bring \
                     the book strictly closer to the optimiser's target",
                    purpose.deviation_after(),
                    purpose.deviation_before()
                ),
            ));
        }

        // 5. Source balance sufficient after every claim. The claims are
        //    re-checked for sign here rather than trusted from
        //    `SourceBalances::new`, which a replayed record never calls: a
        //    negative claim is subtracted and so raises the free balance.
        balances
            .well_formed()
            .map_err(|reason| veto(GateCheck::SourceBalance, reason))?;
        let free = balances.free();
        if intent.amount() > free {
            return Err(veto(
                GateCheck::SourceBalance,
                format!(
                    "source {} has {free} free after {} reserved, {} in flight and {} \
                     committed against a balance of {}, and the transfer is {}; wait for \
                     settlement or reduce the amount",
                    intent.source(),
                    balances.reserved,
                    balances.in_flight_settlement,
                    balances.commitments,
                    balances.balance,
                    intent.amount()
                ),
            ));
        }

        // 6. Velocity breaker not tripped; anomaly detector clear.
        if velocity.breaker == VelocityBreaker::Tripped {
            return Err(veto(
                GateCheck::VelocityAndAnomaly,
                "the velocity breaker is tripped; all transfers are vetoed until a human \
                 resets it"
                    .to_string(),
            ));
        }
        if velocity.anomaly == AnomalyFlag::Raised {
            return Err(veto(
                GateCheck::VelocityAndAnomaly,
                "the anomaly detector has raised a flag; all transfers are vetoed until a \
                 human clears it"
                    .to_string(),
            ));
        }

        // 7. Kill switch.
        if kill_switch == KillSwitchState::Tripped {
            return Err(veto(
                GateCheck::KillSwitch,
                "the kill switch is tripped; all transfers are vetoed".to_string(),
            ));
        }

        Ok(Approved {
            intent: intent.clone(),
            corridor: corridor.id().clone(),
            signature_reference,
            assessed_at: now,
            checks_passed: GateCheck::ALL,
            authority: agreement,
            funding: funding.clone(),
        })
    }

    /// Check 1. Returns the filing reference of the signature relied on and
    /// the three attestations §37.4 required.
    fn corridor_authority(
        intent: &TransferIntent,
        corridor: &Corridor,
        registry: &DestinationRegistry,
        custody: &CustodyPolicy,
        authority: &TransferAuthority,
        funding: &CorridorFunding,
        now: Timestamp,
    ) -> std::result::Result<(String, Agreement), String> {
        if intent.source() != corridor.source() || intent.destination() != corridor.destination() {
            return Err(format!(
                "the intent is {} -> {} but corridor {} runs {} -> {}; an intent is assessed \
                 only against the corridor it names",
                intent.source(),
                intent.destination(),
                corridor.id(),
                corridor.source(),
                corridor.destination()
            ));
        }
        if corridor.stage() != CorridorStage::Active {
            return Err(format!(
                "corridor {} is {}, not active{}",
                corridor.id(),
                corridor.stage().as_str(),
                match corridor.stage() {
                    CorridorStage::TimeDelayed => corridor
                        .activation_at()
                        .map(|at| format!("; it activates at {at}"))
                        .unwrap_or_default(),
                    CorridorStage::Suspended => "; reactivation needs approval".to_string(),
                    CorridorStage::Revoked => "; revocation is permanent".to_string(),
                    _ => "; it has not completed review, signature and delay".to_string(),
                }
            ));
        }
        // §37.4's closing rule is not the only answer to "may this corridor
        // carry anything at all". A corridor exists to fund strategies, and
        // the layer that sets corridor policy has the last word on whether the
        // ones behind this corridor still hold capital. Asked here, in check
        // 1, for the reason the custody table is asked here: it is the same
        // question, and the answer's source does not change which check it
        // belongs to.
        //
        // The ruling is re-derived rather than trusted, because a
        // `CorridorFunding` arrives inside a `GateCommand` deserialised off
        // the log on every replay and `CorridorFunding::new` never runs on
        // that path. A malformed one — a suspended corridor carrying a
        // positive ceiling — would otherwise pass this check and then be
        // measured against its own contradiction in check 2.
        funding.well_formed().map_err(|refusal| {
            format!(
                "corridor {} would be assessed against a funding ruling that contradicts itself, \
                 and {refusal}; correct the ruling rather than the corridor",
                corridor.id()
            )
        })?;
        if funding.standing() == FundingStanding::Suspended {
            return Err(format!(
                "corridor {} is suspended by the layer that sets corridor policy and carries \
                 nothing: {}. This is that control working rather than a corridor fault; the \
                 corridor reopens when the strategies it funds hold capital again, not by being \
                 re-signed",
                corridor.id(),
                funding.reason()
            ));
        }
        let signed = corridor.signed().ok_or_else(|| {
            format!(
                "corridor {} is active but has no signature record; that is a corrupt record, \
                 and the corridor must be suspended and re-signed",
                corridor.id()
            )
        })?;
        if signed.destination != *corridor.destination() {
            return Err(format!(
                "corridor {}'s signature covers destination {} but the corridor now names {}; \
                 the destination changed without a signature",
                corridor.id(),
                signed.destination,
                corridor.destination()
            ));
        }
        if corridor.caps().is_looser_than(&signed.caps) {
            return Err(format!(
                "corridor {}'s caps admit more than the signed definition does; a loosened cap \
                 needs a fresh signature record and the delay, through loosen_caps",
                corridor.id()
            ));
        }
        registry
            .usable(intent.destination(), now)
            .map_err(|err| err.message().to_string())?;
        // §37.4's unconditional rules, asked of the table before the table is
        // asked anything. A `CustodyPolicy` reaches the gate deserialised —
        // from a `GateCommand` on the event log on every replay — and serde
        // does not call `from_constraints`, so a table that constructor
        // refuses could otherwise reach `permits` and answer *yes* for
        // collateral, the one class §37.4 says never moves at all. The failure
        // prevented is a custody policy that reads as a boundary and is only a
        // record: the veto has to be re-derived by the control the replay
        // re-runs, not asserted by a constructor the replay never calls.
        custody.conforms().map_err(|refusal| {
            format!(
                "corridor {} would be assessed against a custody table that contradicts §37.4, \
                 and the {refusal}; correct the table rather than the corridor",
                corridor.id()
            )
        })?;
        // §37.4: the custody policy is the second of the three enforcement
        // points, and a corridor a human signed for a class the policy says
        // never transfers — collateral, say — must still be refused here.
        // The signature proves a person approved it; the policy is what says
        // whether the class may leave at all.
        custody
            .permits(corridor.source_class(), corridor.kind())
            .map_err(|refusal| {
                format!(
                    "corridor {} carries {} through {}, and the {refusal}",
                    corridor.id(),
                    corridor.source_class(),
                    corridor.kind()
                )
            })?;
        // §37.4's closing rule, and the last thing check 1 asks: the three
        // points that just answered — this gate, the allowlist and the
        // custody table — must have attested under three identities, none of
        // them the one that trades. Asked here, in the control, rather than
        // of whoever assembled the attestations, because a `TransferAuthority`
        // arrives deserialised from the log on every replay and a constructor
        // that refused would be a check no replay runs. The failure prevented
        // is §37.4's own: one service identity behind two points is one
        // decision counted twice, and a trading process that can attest to
        // its own capital movement needs no second compromise.
        let agreement = authority.agreement().map_err(|refusal| {
            format!(
                "corridor {} would carry capital on three enforcement points' agreement, and \
                 the {refusal}",
                corridor.id()
            )
        })?;
        // And what the venue's allowlist agreed *to*, where the class demands
        // its own allowlist be mirrored. `all_agree` proves the point spoke;
        // it does not read the reference, so before this the venue-allowlist
        // attestation for one address admitted a corridor running to any
        // other. `ClassConstraints::venue_allowlist_mirrored` had until now no
        // reader at all — a documented precondition enforced nowhere, which is
        // the failure `risk-and-execution.md` names by its other instance.
        custody
            .mirrors_the_venue_allowlist(
                corridor.source_class(),
                corridor.destination(),
                &agreement,
            )
            .map_err(|refusal| {
                format!(
                    "corridor {} runs to {}, and the {refusal}",
                    corridor.id(),
                    corridor.destination()
                )
            })?;
        Ok((signed.signature.reference.clone(), agreement))
    }

    /// Check 2.
    fn caps(
        intent: &TransferIntent,
        corridor: &Corridor,
        funding: &CorridorFunding,
        history: &TransferHistory,
        now: Timestamp,
    ) -> std::result::Result<(), String> {
        let caps = corridor.caps();
        let amount = intent.amount();
        // The two well-formedness rules `TransferIntent::new` and
        // `TransferHistory::new` hold, re-derived here because neither
        // constructor runs on a record replayed off the log. Both are
        // conditions for the caps below meaning anything: a non-positive
        // amount is under every cap vacuously, and a mis-ordered or
        // negative-amount history under-counts the ones that are cumulative.
        if !amount.is_positive() {
            return Err(format!(
                "the intent's amount is {amount}, and every cap admits it vacuously; a transfer \
                 of nothing is not a transfer, and capital comes back through \
                 qip_capital::RecallOrder rather than through a negative one"
            ));
        }
        history.well_formed()?;
        // The derived ceiling before the signed ones, because it is the only
        // cap here that can be *below* what the desk signed, and it is the one
        // that names a strategy rung. Reporting the signed per-transfer cap
        // when the binding constraint was the narrowing would send an operator
        // to loosen a cap that is not what refused the transfer — and loosening
        // a signed cap costs a fresh signature and a delay, spent on the wrong
        // control.
        if amount > funding.permitted() {
            return Err(format!(
                "{amount} exceeds the {} ceiling of {} the layer that sets corridor policy \
                 derived for this corridor: {}. Lower the amount; the ceiling moves when the \
                 strategies this corridor funds do",
                funding.standing(),
                funding.permitted(),
                funding.reason()
            ));
        }
        if amount > caps.max_per_transfer() {
            return Err(format!(
                "{amount} exceeds the per-transfer cap of {}; split it across the minimum \
                 interval or lower the amount",
                caps.max_per_transfer()
            ));
        }
        let hourly = history.carried_since(now.saturating_sub(HOUR)) + amount;
        if hourly > caps.max_per_hour() {
            return Err(format!(
                "{amount} would bring the rolling hour to {hourly}, over the hourly cap of {}; \
                 wait for the hour to roll",
                caps.max_per_hour()
            ));
        }
        let daily = history.carried_since(now.saturating_sub(DAY)) + amount;
        if daily > caps.max_per_day() {
            return Err(format!(
                "{amount} would bring the rolling day to {daily}, over the daily cap of {}; \
                 wait for the day to roll",
                caps.max_per_day()
            ));
        }
        let cumulative = history.carried_total() + amount;
        if cumulative > caps.max_cumulative() {
            return Err(format!(
                "{amount} would bring the corridor's lifetime total to {cumulative}, over the \
                 cumulative cap of {}; the corridor is exhausted and a new one must be signed",
                caps.max_cumulative()
            ));
        }
        let hours = caps.permitted_hours();
        if !hours.permits(now) {
            return Err(format!(
                "{now} is outside the corridor's permitted hours of {hours}"
            ));
        }
        Ok(())
    }

    /// Check 3.
    fn minimum_interval(
        corridor: &Corridor,
        history: &TransferHistory,
        now: Timestamp,
    ) -> std::result::Result<(), String> {
        let Some(last) = history.last_carried_at() else {
            return Ok(());
        };
        let min_interval = corridor.caps().min_interval();
        let elapsed = now.since(last);
        if elapsed < min_interval {
            return Err(format!(
                "the corridor last carried a transfer at {last}, {elapsed:?} ago, and its \
                 minimum interval is {min_interval:?}; the next transfer is permitted at {}",
                last.saturating_add(min_interval)
            ));
        }
        Ok(())
    }
}
