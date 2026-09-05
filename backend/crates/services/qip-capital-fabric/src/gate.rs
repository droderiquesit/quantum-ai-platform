//! The transfer gate: seven deterministic checks, each of which can only veto.
//!
//! Blueprint §37.3 puts a veto-only gate between a machine-generated transfer
//! intent and anything that would act on it. This module is that gate with
//! nothing behind it. [`TransferGate::assess`] takes an intent, the corridor
//! it claims, the allowlist, the balances, the velocity state and the
//! kill-switch state — every one of them supplied by the caller, with the
//! platform clock — and returns either an [`Approved`] record or a [`Vetoed`]
//! record naming the check that failed and what would satisfy it.
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
use crate::custody::CustodyPolicy;
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
        for transfer in &carried {
            if !transfer.amount.is_positive() {
                return Err(Error::invalid(format!(
                    "a carried transfer at {} of {} is not positive; history records what \
                     left, and nothing else",
                    transfer.at, transfer.amount
                )));
            }
        }
        let mut carried = carried;
        carried.sort_by_key(|transfer| transfer.at);
        Ok(Self { carried })
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
        for (name, value) in [
            ("reserved", reserved),
            ("in_flight_settlement", in_flight_settlement),
            ("commitments", commitments),
        ] {
            if value.is_negative() {
                return Err(Error::invalid(format!(
                    "{name} is {value}; a claim on a balance cannot be negative"
                )));
            }
        }
        Ok(Self {
            balance,
            reserved,
            in_flight_settlement,
            commitments,
        })
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

/// The seven checks of §37.3, in the order the gate runs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateCheck {
    /// Corridor active, signature record present and covering the current
    /// definition, destination allowlisted and usable.
    CorridorAuthority,
    /// Within per-transfer, hourly, daily and cumulative caps, and inside
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

        // 1. Corridor active, signature valid, destination allowlisted.
        let signature_reference =
            Self::corridor_authority(intent, corridor, registry, custody, now)
                .map_err(|reason| veto(GateCheck::CorridorAuthority, reason))?;

        // 2. Within per-transfer, hourly, daily, cumulative caps and hours.
        Self::caps(intent, corridor, history, now)
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

        // 5. Source balance sufficient after every claim.
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
        })
    }

    /// Check 1. Returns the filing reference of the signature relied on.
    fn corridor_authority(
        intent: &TransferIntent,
        corridor: &Corridor,
        registry: &DestinationRegistry,
        custody: &CustodyPolicy,
        now: Timestamp,
    ) -> std::result::Result<String, String> {
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
        Ok(signed.signature.reference.clone())
    }

    /// Check 2.
    fn caps(
        intent: &TransferIntent,
        corridor: &Corridor,
        history: &TransferHistory,
        now: Timestamp,
    ) -> std::result::Result<(), String> {
        let caps = corridor.caps();
        let amount = intent.amount();
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
