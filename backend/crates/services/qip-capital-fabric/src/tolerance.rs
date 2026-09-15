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
//! * A rate above [`ToleranceBasis::MAX_INTERVAL_RATE`]. One interval cannot
//!   accrue a tenth of the balance, let alone the whole of it, and a tolerance
//!   of the whole book is a halt that can never fire — the
//!   `MaxExpectedShortfall` defect wearing the opposite sign. The ceiling was
//!   `< 1` until a review pointed out that `0.999999999` passed it, which is
//!   the same defect one unit in the last place away from the bound that was
//!   supposed to stop it.
//! * A rate on a class §38.3 gives no accrual to. Crypto spot settles
//!   instantly and equity's unsettled positions are already in the timeline;
//!   the section's words are "dust floor only" and "zero beyond dust". A
//!   fraction of the book smuggled onto either arm is not a dust floor, and
//!   [`ToleranceBasis::new`] refuses it by name.
//! * A tolerance that is not strictly smaller than the balance it judges, at
//!   the instant it is evaluated. Every refusal above is on the
//!   *declaration*, and a declaration cannot see the balance it will be
//!   applied to; a dust floor of ten million on a book of ten million is
//!   three strictly-positive figures that pass all of them and still admit
//!   the entire balance vanishing as noise. [`ToleranceBasis::evaluate`]
//!   refuses it where the balance is finally in scope.
//!
//! None of these is clamped. A tolerance quietly corrected into range is a
//! caller's mistake that survives into the record as though it were a
//! decision.
//!
//! # One rate now has a feed, and the record says which books it reaches
//!
//! This section used to read "the rate has no feed yet", and that is no longer
//! true of every class. The platform now holds one published rate: the euro
//! area's **deposit facility rate**, fetched from the ECB's own data portal by
//! the `ecb-key-interest-rates` connector, admitted by `qip-data-finder`'s
//! licensing catalogue before the socket, and stamped with the date it applied
//! to and the instant it became readable. [`SourcedIntervalRate`] is how it
//! reaches this module, [`PolicyRateTable`] is where the kernel holds it, and
//! [`ToleranceBasis::from_sourced`] is the only door a non-zero rate enters by
//! from data rather than from a caller's literal.
//!
//! What has **not** changed, and must not be read as having changed:
//!
//! * **A funding rate and a mark interval still have no feed.** §38.3's
//!   perpetual, dated-future and private-position rows have a rule and no
//!   data, and [`SourcedIntervalRate::from_percent_per_annum`] refuses to be
//!   the one that supplies it: an annual percentage becomes a day without a
//!   second assumption and becomes a funding interval only with one.
//! * **The deposit facility rate governs euro balances and nothing else.** An
//!   issuer sets a rate for a currency, so [`PolicyRateTable::governing`] is
//!   keyed by asset as well as class. The one class this platform's kernel can
//!   attest without being told is the desk's own cash at its broker, and that
//!   book is in dollars — so no basis in a default deployment carries a
//!   non-zero rate today, and the reason is the currency rather than the
//!   absence of any feed at all. That distinction is the point: `rate: 0`
//!   reads identically whether no issuer publishes for the currency, the rate
//!   held is for another row, the publisher has gone quiet, or the class
//!   accrues nothing, and [`RateLookup`] makes those four separate findings.
//!
//! Everything a class has no rate for stays at zero, which is the fail-closed
//! direction — the accrual term is nil, so the tolerance is the dust floor and
//! nothing wider — and it stays **visible**: every outcome carries its
//! [`EvaluatedTolerance`], so a reader sees `class: undeclared, rate: 0` and
//! knows no row of §38.3's table was applied, rather than reading a number and
//! assuming one was.

use crate::wallet::{Asset, VenueAsset};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
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
///
/// # Why this is not a plain derived `Deserialize`
///
/// It was one, and that made every refusal in [`Self::new`] a refusal of the
/// *constructor* rather than of the type. A basis is journalled and replayed
/// — [`crate::journal::WalletCommand::Reconcile`] carries a whole schedule of
/// them — and a replay re-runs the real control against whatever the record
/// decoded to. So a hand-edited record naming `{"class":"equity","rate":"0.9"}`
/// — a combination `new` refuses by name, because §38.3 gives equity no
/// accrual at all — rebuilt a basis that had never been checked, and a
/// shortfall recorded as a halt replayed as within tolerance with a
/// [`EvaluatedTolerance::derivation`] that reads like a configured control.
/// A validating constructor beside a derived `Deserialize` is not a control;
/// it is a control and a door beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "RawBasis", try_from = "RawBasis")]
pub struct ToleranceBasis {
    class: ToleranceClass,
    dust: Decimal,
    rate: Decimal,
}

/// A basis's three fields as they travel in a record, before anything has
/// been checked about them.
///
/// Private, and the only way in or out of [`ToleranceBasis`]'s wire form, so
/// that the decode path cannot skip [`ToleranceBasis::new`]. The field names
/// and their order are the derived ones this replaced, so a record written
/// before the door was shut still decodes — and is now checked.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct RawBasis {
    class: ToleranceClass,
    dust: Decimal,
    rate: Decimal,
}

impl From<ToleranceBasis> for RawBasis {
    fn from(basis: ToleranceBasis) -> Self {
        Self {
            class: basis.class,
            dust: basis.dust,
            rate: basis.rate,
        }
    }
}

impl TryFrom<RawBasis> for ToleranceBasis {
    type Error = Error;

    /// Every field a record supplies, through the same refusals a caller
    /// constructing one faces. There is deliberately no second statement of
    /// those rules here: two statements of one rule disagree eventually, and
    /// the one that disagreed would be the one guarding the replayed record.
    fn try_from(raw: RawBasis) -> Result<Self> {
        Self::new(raw.class, raw.dust, raw.rate)
    }
}

impl ToleranceBasis {
    /// The widest fractional movement one interval of any §38.3 class may be
    /// declared to accrue: a tenth of the balance.
    ///
    /// Argued rather than round, because the bound this replaced was argued
    /// too and still stopped nothing. It was `rate < 1`, on the reasoning that
    /// one interval cannot accrue the entire balance — true, and it admits
    /// `0.999999999`, which *is* the entire balance to within one unit in the
    /// last place. A ceiling one unit away from the value it exists to refuse
    /// is the `MaxExpectedShortfall` defect wearing a bound's clothes.
    ///
    /// A tenth is above every interval the section names by more than an order
    /// of magnitude. One day's interest on fiat at a hundred percent a year is
    /// under three tenths of a percent. A perpetual funding interval is a few
    /// basis points, and the widest cap venues publish is three quarters of a
    /// percent. A mark-to-market interval on a dated future or a margin book
    /// is one session's move. The widest of the six is a private position's
    /// mark confidence band at statement cadence, and the headroom a tenth
    /// leaves that row is deliberate rather than accidental. Above a tenth,
    /// one interval is not accruing on the balance — it is a different
    /// balance, and that is a halt to investigate rather than a tolerance to
    /// widen.
    pub const MAX_INTERVAL_RATE: Decimal = Decimal::from_raw(100_000_000);

    /// Declare a basis, refusing a dust floor that is not strictly positive,
    /// a negative rate, a rate above [`Self::MAX_INTERVAL_RATE`], and any rate
    /// at all on a class §38.3 gives no accrual to.
    ///
    /// What it cannot refuse is a floor too wide for the balance it will
    /// judge, because no balance is in scope here. [`Self::evaluate`] holds
    /// that half.
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
        if rate > Self::MAX_INTERVAL_RATE {
            return Err(Error::invalid(format!(
                "the interval rate for a {class} tolerance is {rate}, above the ceiling of {}; \
                 {} cannot accrue a tenth of the balance, and a tolerance approaching the \
                 whole book is a halt that can never fire — state the fraction one interval \
                 actually moves",
                Self::MAX_INTERVAL_RATE,
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

    /// Declare a basis whose accrual term came from a figure a named
    /// institution published.
    ///
    /// The only way a non-zero rate enters this type from data rather than
    /// from a caller's literal. It goes through [`Self::new`] unchanged, so a
    /// sourced rate faces every refusal a declared one does — including
    /// [`Self::MAX_INTERVAL_RATE`], which a vendor serving a mis-scaled figure
    /// would meet. A gate a sourced number skipped would be a gate that guards
    /// only the numbers nobody worried about.
    ///
    /// The class comes from the rate rather than from the caller. Two claims
    /// about which row of §38.3 a figure is one interval of would disagree
    /// eventually, and the one that disagreed would be the one behind the
    /// halt.
    pub fn from_sourced(dust: Decimal, sourced: &SourcedIntervalRate) -> Result<Self> {
        Self::new(sourced.class(), dust, sourced.rate())
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
    ///
    /// # The refusal this is the only place that can make
    ///
    /// A tolerance must be strictly smaller than the balance it judges, and
    /// this is the first moment both figures exist. [`Self::new`] bounds the
    /// rate and demands a positive floor, and neither check can see a balance;
    /// three strictly-positive figures that pass all of them still produce a
    /// control that cannot fire, because `|delta|` on a venue-asset that
    /// simply *vanished* is exactly the expected magnitude. If the tolerance
    /// reaches that, the whole balance disappearing records as within
    /// tolerance, and the record reads like a deliberately wide control rather
    /// than a broken one.
    ///
    /// This is reachable today with no attacker. A statement file whose
    /// `tolerance` and `quantity` fields were transposed — ten million of
    /// each — passes the parser (strictly positive), passes `new` (strictly
    /// positive), and from that cycle on nothing at that venue-asset can
    /// halt. So it is refused here, naming both figures, rather than clamped:
    /// a floor quietly narrowed to fit the book is the operator's transposed
    /// field surviving into the record as a decision.
    ///
    /// A zero expectation is exempt, and must be. A venue-asset the ledger
    /// does not book has an expectation of zero, every positive floor exceeds
    /// it, and refusing there would refuse the pass instead of letting
    /// `BreakCause::UnrecordedByLedger` halt it — a refusal standing in front
    /// of the halt it was written to protect.
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
        if !basis_quantity.is_zero() && tolerance >= basis_quantity {
            return Err(Error::invalid(format!(
                "a {} tolerance of {tolerance} (dust floor {} + accrual {accrual}) is not \
                 smaller than the expected magnitude {basis_quantity} it judges; the whole \
                 balance disappearing would record as within tolerance, so this is a halt \
                 that cannot fire — state a floor below the balance it guards, and check \
                 whether a quantity and a tolerance have been transposed",
                self.class, self.dust
            )));
        }
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

/// One interval's accrual for a §38.3 class, derived from a figure a named
/// institution published, with both instants it carries.
///
/// # Why a type rather than a `Decimal`
///
/// The module documentation above says the rate arm has no feed and that the
/// kernel declares every basis at zero, "because a rate it invented would
/// widen a halt by a number with no owner". The owner is the whole point: a
/// bare `Decimal` reaching [`ToleranceBasis::new`] is indistinguishable from
/// one somebody typed, and the record a halt leaves would read the same
/// either way. This type is the difference between the two — it cannot be
/// built without naming the source, the instant the published figure was true
/// of, and the instant it became knowable.
///
/// # The two instants, and the one that is not optional
///
/// A reconciliation tolerance is evaluated against a book at a moment. A rate
/// that was *true* on a date but not *knowable* until sixteen hours later
/// cannot be used to judge a book in between, and a rate whose knowable
/// instant precedes its true instant is a stamp nobody should believe. Both
/// are refused rather than reordered; see [`Self::from_percent_per_annum`].
///
/// # The derivation is stated, because the licence says it must be
///
/// The ECB's copyright statement — read for `qip-data-finder`'s catalogue
/// entry — permits free use of its published figures on three conditions, one
/// of which is that a *modification* of the figure be stated explicitly, and
/// it names "calculation of growth rates" as the example. Dividing an annual
/// percentage by a day-count basis is exactly such a calculation. So
/// [`Self::derivation`] spells out the arithmetic in words, and it is the
/// sentence that travels beside the number rather than a comment in this file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcedIntervalRate {
    class: ToleranceClass,
    asset: Asset,
    source_id: String,
    /// The figure as the institution published it, in percent per annum.
    published_percent_per_annum: Decimal,
    /// One interval's fractional accrual, derived from the figure above.
    rate: Decimal,
    /// The instant the published figure was true of.
    true_at: Timestamp,
    /// The instant it became knowable — never earlier than `true_at`.
    knowable_at: Timestamp,
}

impl SourcedIntervalRate {
    /// The day-count basis one day's interest is taken on.
    ///
    /// 360, not 365, and the difference is the issuer's rather than this
    /// platform's preference: euro money-market interest, including the
    /// remuneration of the ECB's own deposit facility, accrues actual/360. A
    /// figure divided by the convention its publisher uses is the publisher's
    /// arithmetic; a figure divided by a rounder number would be this
    /// platform's, and §38.3's tolerance is the last place to prefer a rounder
    /// number.
    pub const DAY_COUNT_BASIS: Decimal = Decimal::from_raw(360_000_000_000);

    /// A published percentage is a percentage: one hundred of them is one.
    const PERCENT: Decimal = Decimal::from_raw(100_000_000_000);

    /// How stale a published figure may be before it stops being evidence
    /// about today's book.
    ///
    /// Seven days. A central bank's key rates change only at a scheduled
    /// meeting, and the daily series carries a level for every calendar day,
    /// so a level more than a week old does not mean the rate has not moved —
    /// it means the feed stopped, and a tolerance computed from a figure
    /// nobody is still publishing is a control whose input has gone dark while
    /// the control kept reporting. Refused rather than extrapolated:
    /// [`PolicyRateTable::governing`] answers [`RateLookup::NotCurrent`] and
    /// the caller falls back to the dust floor, which is the tightest the
    /// formula goes.
    pub const MAX_AGE: Duration = Duration::from_days(7);

    /// Derive one day's accrual from an annual percentage an institution
    /// published.
    ///
    /// # What is refused, and never corrected
    ///
    /// * A class §38.3 does not measure in days. The section gives the fiat
    ///   row "one day's interest accrual" and gives the other accruing rows a
    ///   funding interval, a mark-to-market interval and a statement cadence;
    ///   an annual percentage becomes a day without a second assumption and
    ///   becomes any of the others only with one. A per-annum figure pressed
    ///   onto a funding interval would be a number nobody computed wearing a
    ///   citation.
    /// * A knowable instant earlier than the instant the figure was true of.
    ///   That is point-in-time leakage in its purest form: a rate readable
    ///   before it was knowable makes every backtest that touched it
    ///   worthless, however good it looked.
    /// * A figure whose derived rate is not one [`ToleranceBasis`] will
    ///   accept. That bound is not restated here — the derived rate meets
    ///   [`ToleranceBasis::MAX_INTERVAL_RATE`] in
    ///   [`ToleranceBasis::from_sourced`], and two statements of one rule
    ///   disagree eventually.
    ///
    /// # Why the magnitude
    ///
    /// A published policy rate can be negative, and the ECB's deposit facility
    /// sat at -0.50 from 2019 to 2022. One day's accrual on a negative rate is
    /// a balance that *shrinks*, and a tolerance allows for a movement of that
    /// size whichever way it points — so the accrual is taken on the
    /// magnitude. Signing it would produce a negative interval rate, which
    /// [`ToleranceBasis::new`] refuses by name because it would pull the
    /// tolerance below the dust floor an operator set. Three real years of
    /// published policy would then have no usable rate at all, which is a
    /// worse answer than the correct one.
    pub fn from_percent_per_annum(
        class: ToleranceClass,
        asset: Asset,
        source_id: impl Into<String>,
        published_percent_per_annum: Decimal,
        true_at: Timestamp,
        knowable_at: Timestamp,
    ) -> Result<Self> {
        let source_id = source_id.into();
        if class != ToleranceClass::FiatAtBrokerOrBank {
            return Err(Error::invalid(format!(
                "a figure in percent per annum was offered as one interval of a {class} \
                 tolerance, and §38.3 gives that class {}. Only the fiat row's interval is a \
                 day, which is the one an annual figure converts to without a second assumption \
                 nobody made",
                class.interval()
            )));
        }
        if knowable_at < true_at {
            return Err(Error::invalid(format!(
                "a rate published by `{source_id}` is stamped true at {} and knowable at {}, \
                 which is earlier. A figure readable before it was knowable is point-in-time \
                 leakage, and a tolerance built on one judges a book against something nobody \
                 could have read",
                true_at.to_rfc3339(),
                knowable_at.to_rfc3339()
            )));
        }
        let rate = published_percent_per_annum
            .abs()
            .checked_div(Self::PERCENT)
            .and_then(|fraction| fraction.checked_div(Self::DAY_COUNT_BASIS))
            .ok_or_else(|| {
                Error::numeric(format!(
                    "one day's accrual could not be derived from {published_percent_per_annum} \
                     percent per annum published by `{source_id}`"
                ))
            })?;
        Ok(Self {
            class,
            asset,
            source_id,
            published_percent_per_annum,
            rate,
            true_at,
            knowable_at,
        })
    }

    pub const fn class(&self) -> ToleranceClass {
        self.class
    }

    /// The asset the publishing institution sets this rate for.
    ///
    /// The field that stops a euro rate judging a dollar book. The ECB sets
    /// the euro area's rates and nobody else's, so a lookup for any other
    /// asset must answer that nothing is held rather than reach for the
    /// nearest number.
    pub const fn asset(&self) -> &Asset {
        &self.asset
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// One interval's fractional accrual.
    pub const fn rate(&self) -> Decimal {
        self.rate
    }

    /// The figure as published, before this platform divided it.
    pub const fn published_percent_per_annum(&self) -> Decimal {
        self.published_percent_per_annum
    }

    pub const fn true_at(&self) -> Timestamp {
        self.true_at
    }

    pub const fn knowable_at(&self) -> Timestamp {
        self.knowable_at
    }

    /// Whether this figure was knowable by `now` and has not gone stale.
    fn usable_at(&self, now: Timestamp) -> bool {
        now >= self.knowable_at && now.since(self.knowable_at) <= Self::MAX_AGE
    }

    /// The arithmetic in words: the published figure, the division, and the
    /// two instants.
    ///
    /// This sentence is the platform's statement that it modified the
    /// publisher's figure, which the ECB's terms require explicitly and which
    /// no class label can make. It is also the only way a reader of a halt can
    /// re-derive the number rather than trust it.
    pub fn derivation(&self) -> String {
        format!(
            "{} per annum for {} published by `{}`, true at {} and knowable at {}, divided by \
             100 and by an actual/{} day count to one day's accrual of {}",
            self.published_percent_per_annum,
            self.asset,
            self.source_id,
            self.true_at.to_rfc3339(),
            self.knowable_at.to_rfc3339(),
            Self::DAY_COUNT_BASIS,
            self.rate
        )
    }
}

/// What a lookup in a [`PolicyRateTable`] found, as four findings rather than
/// one `Option`.
///
/// An `Option` would collapse the four into "no rate", and they are different
/// facts about a control. "Nothing is published for dollars" is a gap in the
/// sources this build carries; "the rate held is for a class this venue-asset
/// is not" is a §38.3 question; "the feed stopped a fortnight ago" is an
/// outage. A caller that could not tell them apart would report a book judged
/// at its dust floor without being able to say why — which is the state the
/// tolerance module was in before this type, and the reason its records say
/// `rate: 0` and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RateLookup {
    /// A published rate governs this asset and this class, and is current.
    Governed(Box<SourcedIntervalRate>),
    /// No source in this build publishes a rate for this asset.
    NoneHeld,
    /// A rate is held for this asset and governs a different §38.3 class.
    ClassNotGoverned {
        held: ToleranceClass,
        asked: ToleranceClass,
    },
    /// A rate is held and is not evidence about this instant: either it was
    /// not yet knowable, or its publisher has gone quiet.
    NotCurrent {
        source_id: String,
        knowable_at: Timestamp,
    },
}

impl RateLookup {
    /// The sentence a record carries beside the basis, saying which of the
    /// four happened.
    pub fn describe(&self, asset: &Asset, now: Timestamp) -> String {
        match self {
            Self::Governed(rate) => rate.derivation(),
            Self::NoneHeld => format!(
                "no source in this build publishes an interval rate for {asset}, so the dust \
                 floor is the whole tolerance"
            ),
            Self::ClassNotGoverned { held, asked } => format!(
                "the rate held for {asset} is one interval of a {held} tolerance and this \
                 venue-asset is {asked}, so the dust floor is the whole tolerance"
            ),
            Self::NotCurrent {
                source_id,
                knowable_at,
            } => format!(
                "the rate held for {asset} from `{source_id}` became knowable at {} and is not \
                 evidence about {}, so the dust floor is the whole tolerance",
                knowable_at.to_rfc3339(),
                now.to_rfc3339()
            ),
        }
    }

    /// The rate, when one governs.
    pub fn rate(&self) -> Option<&SourcedIntervalRate> {
        match self {
            Self::Governed(rate) => Some(rate),
            Self::NoneHeld | Self::ClassNotGoverned { .. } | Self::NotCurrent { .. } => None,
        }
    }
}

/// Every published interval rate this process holds, keyed by the asset its
/// publisher sets it for.
///
/// Bounded, like every working set here: a table fed from a feed is a table
/// that grows, and [`Self::MAX_ASSETS`] is the ceiling. Keyed by asset and not
/// by class, because an institution sets a rate for a currency and §38.3
/// decides which row of its table that rate can be one interval of — two
/// different questions, and folding them into one key would let a rate
/// published for euros answer for a dollar book whose class happened to match.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRateTable {
    per_asset: BTreeMap<Asset, SourcedIntervalRate>,
}

impl PolicyRateTable {
    /// How many assets' rates may be held at once.
    ///
    /// Sixteen. There are not sixteen institutions publishing a policy rate
    /// whose terms this platform has evaluated; the bound is here so that a
    /// feed which started minting assets could not grow the table without
    /// limit, and it refuses the seventeenth by name rather than evicting one
    /// nobody chose.
    pub const MAX_ASSETS: usize = 16;

    pub fn new() -> Self {
        Self::default()
    }

    /// Record a published rate, or refuse.
    ///
    /// Refuses a figure older than the one already held for that asset. A
    /// re-poll serving the same date is idempotent and lands on the same
    /// value; a figure stamped *earlier* than the one held is either a replay
    /// or a vendor serving history, and taking it would move the tolerance
    /// backwards onto a rate that has since been superseded — the same
    /// backwards-instant defect `StandingAdmission::check` refuses in the
    /// licensing gate, in a place where the consequence is a halt rather than
    /// a licence.
    pub fn record(&mut self, rate: SourcedIntervalRate) -> Result<()> {
        if let Some(held) = self.per_asset.get(rate.asset()) {
            if rate.true_at() < held.true_at() {
                return Err(Error::invalid(format!(
                    "a rate for {} stamped true at {} was offered against one already held for \
                     {}, which is later. A tolerance moved backwards onto a superseded rate is a \
                     control judging today's book by a figure that has been replaced",
                    rate.asset(),
                    rate.true_at().to_rfc3339(),
                    held.true_at().to_rfc3339()
                )));
            }
        } else if self.per_asset.len() >= Self::MAX_ASSETS {
            return Err(Error::denied(format!(
                "a rate for {} would be the {}th asset held against a bound of {}; retire one \
                 before adding another",
                rate.asset(),
                self.per_asset.len() + 1,
                Self::MAX_ASSETS
            )));
        }
        self.per_asset.insert(rate.asset().clone(), rate);
        Ok(())
    }

    /// What this table has to say about one venue-asset's class at one instant.
    pub fn governing(&self, asset: &Asset, class: ToleranceClass, now: Timestamp) -> RateLookup {
        let Some(held) = self.per_asset.get(asset) else {
            return RateLookup::NoneHeld;
        };
        if held.class() != class {
            return RateLookup::ClassNotGoverned {
                held: held.class(),
                asked: class,
            };
        }
        if !held.usable_at(now) {
            return RateLookup::NotCurrent {
                source_id: held.source_id().to_string(),
                knowable_at: held.knowable_at(),
            };
        }
        RateLookup::Governed(Box::new(held.clone()))
    }

    /// Every asset a rate is held for, in stable order.
    pub fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.per_asset.keys()
    }

    pub fn len(&self) -> usize {
        self.per_asset.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_asset.is_empty()
    }
}
