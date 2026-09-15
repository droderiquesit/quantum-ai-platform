//! §38.3's reconciliation tolerance, as the formula the section asks for
//! rather than the constant the platform shipped.
//!
//! # The failure this module exists to stop
//!
//! Reconciliation halted a venue-asset when `|delta| >= tolerance`, and
//! `tolerance` was one strictly-positive number per *asset*, handed in with
//! the operator's statement. Three things were wrong with that, and the
//! blueprint names the first of them in one sentence — "Tolerance is a
//! formula, not a constant":
//!
//! 1. **A constant is too tight for one book and too loose for another.** A
//!    dollar of drift on a hundred dollars is a break; a dollar of drift on a
//!    hundred million is a rounding artefact. One number cannot be right for
//!    both, so whichever book it was chosen for, the other is mis-judged —
//!    and the mis-judgement is silent in both directions.
//! 2. **It was keyed by asset, not by venue-asset.** Two venues reporting USD
//!    shared one tolerance, and the last statement handed in silently
//!    overwrote the first venue's. A tolerance a venue never chose is a
//!    control nobody set.
//! 3. **It had no class.** §38.3's table gives six asset classes six
//!    different rules — dust for crypto spot, a funding interval's accrual
//!    for a perpetual, a mark-to-market interval for a dated future, a day's
//!    interest for fiat, dust for equity, the mark's own confidence band for
//!    a private position. A single number records which of those six was
//!    meant: none of them.
//!
//! # The formula
//!
//! ```text
//! basis_quantity = |expected|                       (§38.3's expectation)
//! accrual        = rate x basis_quantity            (one interval of the class)
//! tolerance      = dust + accrual
//! ```
//!
//! `dust` is the absolute floor, in the asset's own units — the smallest
//! delta anybody at that venue would call a difference. `rate` is one
//! interval's fractional movement for the class, and *which* interval is the
//! class's business: [`ToleranceClass::interval`] states it in words and that
//! sentence travels in the record, so an operator reading a halt can see what
//! interval the tolerance allowed for without consulting this file.
//!
//! The formula scales with all three things a constant could not: the
//! quantity (through `basis_quantity`), the venue (the schedule is keyed by
//! venue-asset, so two venues holding one asset are judged separately) and
//! the instrument (through the class and its rate).
//!
//! # What is refused rather than corrected
//!
//! * A dust floor that is not strictly positive. Zero halts every reconciled
//!   balance and a negative figure halts none; both read as a configured
//!   control and are neither.
//! * A negative rate, which would shrink the tolerance below the dust floor
//!   the operator set.
//! * A rate of one or more. One interval cannot accrue the entire balance,
//!   and a tolerance of the whole book is a halt that can never fire — the
//!   `MaxExpectedShortfall` defect wearing the opposite sign.
//! * A rate on a class §38.3 gives no accrual to. Crypto spot settles
//!   instantly and equity's unsettled positions are already in the timeline;
//!   the section's words are "dust floor only" and "zero beyond dust". A
//!   fraction of the book smuggled onto either arm is not a dust floor, and
//!   [`ToleranceBasis::new`] refuses it by name.
//!
//! None of these is clamped. A tolerance quietly corrected into range is a
//! caller's mistake that survives into the record as though it were a
//! decision.
//!
//! # The rate has no feed yet, and the record says so
//!
//! Nothing in this platform holds a funding rate, a deposit rate or a mark
//! interval: there is no rate field in the kernel's configuration, none on
//! the statement an operator hands in, and no venue this process may ask. So
//! the kernel declares every basis with a rate of zero and, for every
//! venue-asset except the desk's own cash, the class
//! [`ToleranceClass::Undeclared`].
//!
//! That is deliberate and it is the fail-closed direction — the accrual term
//! is nil, so the tolerance is the dust floor and nothing wider. What matters
//! is that it is **visible**: every outcome carries its
//! [`EvaluatedTolerance`], so a reader sees `class: undeclared, rate: 0` and
//! knows no row of §38.3's table was applied, rather than reading a number
//! and assuming one was. A module that says nothing when it has no subject
//! reaches no surface in the only state a deployment is ever in.

use crate::wallet::{Asset, VenueAsset};
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Which row of §38.3's tolerance table governs a venue-asset.
///
/// Six rows from the section, plus [`Self::Undeclared`] for a venue-asset
/// nobody has classified. The seventh is not a default dressed as a class: it
/// carries no accrual at all, so an unclassified holding is judged at its dust
/// floor — the tightest the formula can be — and the record names it, so that
/// "we never classified this" and "this class has no accrual" are two
/// findings rather than one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToleranceClass {
    /// Crypto spot. "Dust floor only. Instant settlement; any non-dust delta
    /// is a real break."
    CryptoSpot,
    /// Perpetual futures. "One funding interval's accrual at the current rate."
    PerpetualFuture,
    /// Dated futures and margin. "One mark-to-market interval."
    DatedFutureOrMargin,
    /// Fiat at a broker or a bank. "One day's interest accrual."
    FiatAtBrokerOrBank,
    /// Equity. "Zero beyond dust. Unsettled positions are already in the
    /// timeline."
    Equity,
    /// Private positions. "Statement cadence. A mark is compared to the
    /// administrator's NAV, and a divergence beyond the mark's own confidence
    /// band is flagged."
    PrivatePosition,
    /// No class has been stated for this venue-asset.
    Undeclared,
}

impl ToleranceClass {
    /// Every class, for a caller enumerating the table.
    pub const ALL: [Self; 7] = [
        Self::CryptoSpot,
        Self::PerpetualFuture,
        Self::DatedFutureOrMargin,
        Self::FiatAtBrokerOrBank,
        Self::Equity,
        Self::PrivatePosition,
        Self::Undeclared,
    ];

    /// A stable label for records, alerts and metrics.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::CryptoSpot => "crypto_spot",
            Self::PerpetualFuture => "perpetual_future",
            Self::DatedFutureOrMargin => "dated_future_or_margin",
            Self::FiatAtBrokerOrBank => "fiat_at_broker_or_bank",
            Self::Equity => "equity",
            Self::PrivatePosition => "private_position",
            Self::Undeclared => "undeclared",
        }
    }

    /// Whether §38.3 gives this class an accrual term at all.
    ///
    /// False for the two rows the section writes as "dust floor only" and
    /// "zero beyond dust", and for [`Self::Undeclared`]. A class that does not
    /// accrue may not carry a rate, and [`ToleranceBasis::new`] enforces that
    /// rather than ignoring the rate — an ignored rate is a number an operator
    /// set and the platform did not use.
    pub const fn accrues(&self) -> bool {
        match self {
            Self::PerpetualFuture
            | Self::DatedFutureOrMargin
            | Self::FiatAtBrokerOrBank
            | Self::PrivatePosition => true,
            Self::CryptoSpot | Self::Equity | Self::Undeclared => false,
        }
    }

    /// What one interval of this class is, in the words of §38.3.
    ///
    /// Carried into the record so a halt explains what movement its tolerance
    /// was allowing for, without the reader holding the blueprint open.
    pub const fn interval(&self) -> &'static str {
        match self {
            Self::CryptoSpot => {
                "settlement is instant, so no interval accrues and any non-dust delta is a \
                 real break"
            }
            Self::PerpetualFuture => "one funding interval's accrual at the current rate",
            Self::DatedFutureOrMargin => "one mark-to-market interval",
            Self::FiatAtBrokerOrBank => "one day's interest accrual",
            Self::Equity => {
                "zero beyond dust, because unsettled positions are already in the timeline"
            }
            Self::PrivatePosition => "the mark's own confidence band, at the statement cadence",
            Self::Undeclared => {
                "no class has been stated, so no interval accrues and the dust floor is the \
                 whole tolerance"
            }
        }
    }
}

impl fmt::Display for ToleranceClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The inputs §38.3's formula needs for one venue-asset: which row of the
/// table, the dust floor beneath it, and one interval's rate.
///
/// Immutable after construction, and every constructor is fallible, so a
/// basis that exists is a basis whose three parts were checked against each
/// other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToleranceBasis {
    class: ToleranceClass,
    dust: Decimal,
    rate: Decimal,
}

impl ToleranceBasis {
    /// Declare a basis, refusing a dust floor that is not strictly positive,
    /// a negative rate, a rate of one or more, and any rate at all on a class
    /// §38.3 gives no accrual to.
    pub fn new(class: ToleranceClass, dust: Decimal, rate: Decimal) -> Result<Self> {
        if !dust.is_positive() {
            return Err(Error::invalid(format!(
                "the dust floor for a {class} tolerance is {dust}; it must be strictly \
                 positive — zero halts every reconciled balance and a negative figure halts \
                 none, so state the smallest delta that is a difference at this venue"
            )));
        }
        if rate.is_negative() {
            return Err(Error::invalid(format!(
                "the interval rate for a {class} tolerance is {rate}; a negative rate would \
                 pull the tolerance below the dust floor that was set for it — state zero if \
                 no interval accrues"
            )));
        }
        if rate >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "the interval rate for a {class} tolerance is {rate}; {} cannot accrue the \
                 whole balance, and a tolerance of the entire book is a halt that can never \
                 fire — state the fraction one interval actually moves",
                class.interval()
            )));
        }
        if !class.accrues() && !rate.is_zero() {
            return Err(Error::invalid(format!(
                "a {class} tolerance was given an interval rate of {rate}, and §38.3 gives \
                 that class {} — a fraction of the book is not a dust floor, so state the \
                 floor and leave the rate at zero",
                class.interval()
            )));
        }
        Ok(Self { class, dust, rate })
    }

    /// Declare a basis with no accrual term: the dust floor is the whole
    /// tolerance.
    ///
    /// The honest constructor for a class whose rate nobody holds, and the one
    /// the kernel uses today. It is the tightest the formula goes, so a
    /// missing rate feed cannot widen a tolerance by accident.
    pub fn dust_only(class: ToleranceClass, dust: Decimal) -> Result<Self> {
        Self::new(class, dust, Decimal::ZERO)
    }

    /// Which row of §38.3's table this is.
    pub const fn class(&self) -> ToleranceClass {
        self.class
    }

    /// The absolute floor, in the asset's own units.
    pub const fn dust(&self) -> Decimal {
        self.dust
    }

    /// One interval's fractional accrual.
    pub const fn rate(&self) -> Decimal {
        self.rate
    }

    /// Evaluate the formula against what the ledger expected.
    ///
    /// `expected` may be negative — a margin account in debit is a real
    /// balance — so the accrual is taken on its magnitude. Checked
    /// throughout: a tolerance that saturated would be a number nobody
    /// computed governing a halt, which is the same class of defect as a
    /// tolerance nobody chose.
    ///
    /// The product rounds half-away-from-zero at the ninth decimal, which can
    /// widen the tolerance by at most one unit in the last place. That is
    /// stated rather than corrected: a truncating multiply written here would
    /// be a second statement of `Decimal`'s rounding rule, and two statements
    /// of one rule disagree eventually.
    pub fn evaluate(&self, expected: Decimal) -> Result<EvaluatedTolerance> {
        let basis_quantity = expected.abs();
        let accrual = self.rate.checked_mul(basis_quantity).ok_or_else(|| {
            Error::numeric(format!(
                "one interval's accrual overflowed at rate {} on an expected balance of \
                 {expected}",
                self.rate
            ))
        })?;
        let tolerance = self.dust.checked_add(accrual).ok_or_else(|| {
            Error::numeric(format!(
                "a {} tolerance overflowed from a dust floor of {} plus an accrual of {accrual}",
                self.class, self.dust
            ))
        })?;
        Ok(EvaluatedTolerance {
            class: self.class,
            dust: self.dust,
            rate: self.rate,
            basis_quantity,
            accrual,
            tolerance,
        })
    }
}

/// §38.3's formula as it was evaluated for one venue-asset on one pass, with
/// every term kept.
///
/// The working, not the answer alone. A halt that reported only "tolerance
/// 12.5" cannot be argued with: nobody reading it can tell whether the figure
/// came from a dust floor somebody set, an accrual on a balance that has since
/// moved, or a class that was never declared. Each term is here so the number
/// can be re-derived from the record rather than trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluatedTolerance {
    /// The row of §38.3's table that governed.
    pub class: ToleranceClass,
    /// The absolute floor beneath the formula.
    pub dust: Decimal,
    /// One interval's fractional accrual for the class.
    pub rate: Decimal,
    /// `|expected|` — what the accrual was taken on.
    pub basis_quantity: Decimal,
    /// `rate x basis_quantity`.
    pub accrual: Decimal,
    /// `dust + accrual` — the figure the delta was judged against.
    pub tolerance: Decimal,
}

impl EvaluatedTolerance {
    /// Whether the accrual term contributed anything.
    ///
    /// False whenever the class does not accrue, and also whenever it does and
    /// the rate is zero because nobody holds one. Both mean the halt fired at
    /// the dust floor, and a reader who assumed a class term was in play would
    /// be reading a control that is tighter than they think — which is the
    /// safe direction to be wrong in, and still worth knowing.
    pub fn accrual_applied(&self) -> bool {
        !self.accrual.is_zero()
    }

    /// A sentence naming how this tolerance was arrived at.
    pub fn derivation(&self) -> String {
        format!(
            "{} ({}): dust {} + rate {} x expected magnitude {} = {}",
            self.class,
            self.class.interval(),
            self.dust,
            self.rate,
            self.basis_quantity,
            self.tolerance
        )
    }
}

/// One venue-asset's basis, as it travels in a record.
///
/// The wire form of [`ToleranceSchedule`] is a sequence of these rather than a
/// JSON object, because a venue-asset is two fields and a JSON key is a
/// string: flattening it to `"venue/asset"` would make an asset named with a
/// slash decode as a different venue-asset than it encoded as. The sequence is
/// emitted in venue-asset order, so two schedules holding the same bases
/// serialise to the same bytes and hash the same in the chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// The venue-asset the basis governs.
    pub venue_asset: VenueAsset,
    /// Its §38.3 basis.
    pub basis: ToleranceBasis,
}

/// Every venue-asset's basis, for one reconciliation pass.
///
/// Keyed by venue-asset rather than by asset. Two venues holding USD are two
/// books with two dust floors, and a schedule keyed by asset let the second
/// statement handed in silently overwrite the first venue's control.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Vec<ScheduleEntry>", try_from = "Vec<ScheduleEntry>")]
pub struct ToleranceSchedule {
    per_venue_asset: BTreeMap<VenueAsset, ToleranceBasis>,
}

impl From<ToleranceSchedule> for Vec<ScheduleEntry> {
    fn from(schedule: ToleranceSchedule) -> Self {
        schedule
            .per_venue_asset
            .into_iter()
            .map(|(venue_asset, basis)| ScheduleEntry { venue_asset, basis })
            .collect()
    }
}

impl TryFrom<Vec<ScheduleEntry>> for ToleranceSchedule {
    type Error = Error;

    /// Rebuild a schedule from a record, refusing two bases for one
    /// venue-asset.
    ///
    /// A duplicate is two claims about one control. Taking the last would make
    /// the tolerance depend on the order a record happened to list its
    /// entries, which is the same defect as the per-asset key this replaced —
    /// and it would be invisible, because the rebuilt schedule would look
    /// exactly like a well-formed one.
    fn try_from(entries: Vec<ScheduleEntry>) -> Result<Self> {
        let mut per_venue_asset = BTreeMap::new();
        for entry in entries {
            let key = entry.venue_asset.clone();
            if per_venue_asset
                .insert(entry.venue_asset, entry.basis)
                .is_some()
            {
                return Err(Error::invalid(format!(
                    "two tolerance bases were recorded for {key}; a schedule holds one per \
                     venue-asset, and taking either would make the control depend on the \
                     order the record listed them in"
                )));
            }
        }
        Ok(Self { per_venue_asset })
    }
}

/// What a schedule has to say about itself before anything is reconciled
/// against it.
///
/// An empty schedule is the state a deployment sits in until the first
/// statement arrives, and it is the state in which a module that returns
/// nothing reaches nobody. [`ToleranceSchedule::state`] makes the emptiness a
/// value with a sentence in it rather than an absence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleState {
    /// Nothing has been declared. Reconciliation against this refuses.
    Idle {
        /// Why an empty schedule is not a permissive one.
        reason: &'static str,
    },
    /// Bases are declared, and this is how many, and how many of them carry a
    /// class §38.3 names.
    Declared {
        /// How many venue-assets have a basis.
        venue_assets: usize,
        /// How many of those carry a class other than
        /// [`ToleranceClass::Undeclared`].
        classified: usize,
        /// How many of those evaluate an accrual — a non-zero rate on a class
        /// that accrues. Zero everywhere means every tolerance in this pass is
        /// its dust floor.
        accruing: usize,
    },
}

impl ToleranceSchedule {
    /// Why an empty schedule refuses rather than admitting everything.
    pub const IDLE_REASON: &'static str = "no venue-asset has a tolerance basis, so no balance can be judged; an empty \
         schedule reconciles nothing rather than reconciling everything";

    /// A schedule with nothing declared.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare one venue-asset's basis, replacing any basis it already had.
    ///
    /// Consuming rather than `&mut self` so a schedule cannot be edited after
    /// it has been handed to a reconciliation — the pass is judged against the
    /// schedule it was given, and a caller that wants a different one builds a
    /// different one.
    pub fn with_basis(mut self, key: VenueAsset, basis: ToleranceBasis) -> Self {
        self.per_venue_asset.insert(key, basis);
        self
    }

    /// The basis for a venue-asset, or `None` when none was declared.
    pub fn basis_for(&self, key: &VenueAsset) -> Option<&ToleranceBasis> {
        self.per_venue_asset.get(key)
    }

    /// Every venue-asset with a basis, in stable order.
    pub fn venue_assets(&self) -> impl Iterator<Item = &VenueAsset> {
        self.per_venue_asset.keys()
    }

    /// How many bases are declared.
    pub fn len(&self) -> usize {
        self.per_venue_asset.len()
    }

    /// Whether nothing is declared.
    pub fn is_empty(&self) -> bool {
        self.per_venue_asset.is_empty()
    }

    /// What the schedule has to say about itself, including when it has
    /// nothing in it.
    pub fn state(&self) -> ScheduleState {
        if self.per_venue_asset.is_empty() {
            return ScheduleState::Idle {
                reason: Self::IDLE_REASON,
            };
        }
        let classified = self
            .per_venue_asset
            .values()
            .filter(|basis| basis.class() != ToleranceClass::Undeclared)
            .count();
        let accruing = self
            .per_venue_asset
            .values()
            .filter(|basis| basis.class().accrues() && !basis.rate().is_zero())
            .count();
        ScheduleState::Declared {
            venue_assets: self.per_venue_asset.len(),
            classified,
            accruing,
        }
    }
}

/// The class the kernel can attest for a venue-asset without being told.
///
/// Exactly one: the desk's own cash at its broker, which is fiat at a broker
/// by the same fact that makes it the ledger's cash row. Everything else a
/// statement names is a venue-asset this process knows nothing about beyond
/// its name, and guessing a class from an asset string — "BTC looks like
/// crypto spot" — would put a §38.3 row nobody chose behind a halt.
///
/// Here rather than in the kernel because the mapping is the section's, not
/// the composition's, and the test that pins it belongs beside the table it
/// reads.
pub fn class_for_desk_cash(
    venue: &VenueId,
    asset: &Asset,
    desk_venue: &VenueId,
    desk_asset: &Asset,
) -> ToleranceClass {
    if venue == desk_venue && asset == desk_asset {
        ToleranceClass::FiatAtBrokerOrBank
    } else {
        ToleranceClass::Undeclared
    }
}
