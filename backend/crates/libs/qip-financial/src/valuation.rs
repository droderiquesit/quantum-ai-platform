//! Illiquid valuation — blueprint §16.2 and §16.3.
//!
//! Roughly sixty percent of global investable wealth has no continuous price.
//! This module is how such a thing gets a mark, and — far more importantly —
//! how it fails to get one.
//!
//! **A mark without a method is an assertion.** Every [`AssetValuation`]
//! carries the [`ValuationMethod`] it was arrived at by, the inputs it was
//! derived from with their own confidence, when it was struck, and when it
//! must be refreshed. A portfolio holding both a quoted equity and a venture
//! position is then not summing two numbers that mean different things without
//! saying so.
//!
//! **There is no constructor that invents a mark.** Every entry point on
//! [`IlliquidValuator`] takes evidence and refuses when the evidence is
//! absent: no comparables, no cashflows, no round, no cost — no valuation, and
//! an error naming what to supply. Returning a number nobody could observe,
//! presented as a valuation, is the specific failure this plane must not have,
//! because a fabricated mark is indistinguishable downstream from an observed
//! one and will support leverage on its own authority. Reading a mark back from
//! a document is the second way to obtain one, and it runs the same check: a
//! plain `#[derive(Deserialize)]` wrote straight to the private fields and was
//! the constructor that invented a mark, until `serde(try_from)` put it through
//! [`IlliquidValuator::assemble`] like everything else.
//!
//! **Marks decay, from the instant the evidence was observed.** A last-round
//! valuation six months old carries less confidence than one six days old, and
//! [`AssetValuation::confidence_at`] reduces its weight rather than treating it
//! as equally true. The clock starts at [`AssetValuation::as_of`], which for
//! [`IlliquidValuator::mark_object`] is the record's own observation instant
//! and never the instant the platform happened to assemble itself: a mark
//! struck at the assembly instant decays with process uptime, so byte-identical
//! catalogue data would size differently on a host up six months and on one
//! restarted this morning, and a restart would refresh a mark from 2010.
//! Confidence is `f64` because it is a statistic; the mark is [`Decimal`]
//! because it is money. The crossing point is marked where a confidence
//! multiplies a value.
//!
//! **Every input is checked for knowability.** An input stamped after the
//! valuation instant is refused. A mark struck as of January from a comparable
//! transaction that printed in March is a point-in-time leak, and a backtest
//! built on it is better than reality by exactly the information it should not
//! have had.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::cashflow::CashflowForecast;
use crate::extensions::{Extension, PrivateAssetDetails};
use crate::object::FinancialObject;

/// How a mark was arrived at.
///
/// The order of the variants is the blueprint's order of descending
/// confidence, and [`Self::base_confidence`] is the table in §16.3 made
/// arithmetic rather than prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValuationMethod {
    /// A continuous two-sided market exists.
    Quoted,
    /// Similar instruments quote and this one is interpolated from them.
    Matrix,
    /// Observable transactions in similar assets, adjusted.
    Comparables,
    /// Forecast cashflows discounted at a rate reflecting risk.
    DiscountedCashflow,
    /// A pricing model with observable inputs.
    Model,
    /// The most recent primary transaction price.
    LastRound,
    /// Acquisition cost, absent anything better. An admission of ignorance.
    Cost,
}

impl ValuationMethod {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Quoted => "quoted",
            Self::Matrix => "matrix",
            Self::Comparables => "comparables",
            Self::DiscountedCashflow => "discounted_cashflow",
            Self::Model => "model",
            Self::LastRound => "last_round",
            Self::Cost => "cost",
        }
    }

    /// Confidence the method carries on the day it is struck.
    ///
    /// A statistic, not money. The values are ordered strictly so that no two
    /// methods are interchangeable: a mark that fell back from comparables to
    /// cost must be visibly worse, or the fallback is free and will be taken.
    pub const fn base_confidence(self) -> f64 {
        match self {
            Self::Quoted => 0.99,
            Self::Matrix => 0.85,
            Self::Comparables => 0.65,
            Self::DiscountedCashflow => 0.60,
            Self::Model => 0.55,
            Self::LastRound => 0.40,
            Self::Cost => 0.20,
        }
    }

    /// How long the mark keeps half its confidence.
    ///
    /// A quote is worth what it was worth a moment ago and almost nothing a
    /// year later; a cost basis was never worth much and does not get worse
    /// nearly as fast. The half-lives encode that difference so a stale quote
    /// does not outrank a fresh appraisal.
    pub const fn decay_half_life(self) -> Duration {
        match self {
            Self::Quoted => Duration::from_days(1),
            Self::Matrix => Duration::from_days(7),
            Self::Comparables => Duration::from_days(90),
            Self::DiscountedCashflow => Duration::from_days(120),
            Self::Model => Duration::from_days(60),
            Self::LastRound => Duration::from_days(180),
            Self::Cost => Duration::from_days(365),
        }
    }

    /// How long a mark by this method may stand before it must be refreshed.
    pub const fn review_interval(self) -> Duration {
        match self {
            Self::Quoted => Duration::from_days(1),
            Self::Matrix => Duration::from_days(30),
            Self::Comparables => Duration::from_days(180),
            Self::DiscountedCashflow => Duration::from_days(180),
            Self::Model => Duration::from_days(90),
            Self::LastRound => Duration::from_days(365),
            Self::Cost => Duration::from_days(365),
        }
    }
}

/// One piece of evidence a mark was derived from, with its own confidence.
///
/// Fields are private and deserialisation is routed through [`Self::new`] by
/// `serde(try_from)`. The plain derive was a second way in that wrote straight
/// to the private fields, and an input is not an inert record: its confidence
/// is what [`IlliquidValuator::from_comparables`] scales the whole mark by, and
/// its `known_at` is what [`IlliquidValuator::assemble`] tests for
/// point-in-time leakage. An input a document invented is evidence the mark
/// then claims to rest on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ValuationInputWire")]
pub struct ValuationInput {
    label: String,
    value: Decimal,
    /// How much this input is itself to be trusted. A statistic.
    confidence: f64,
    /// When this input became knowable to the platform.
    known_at: Timestamp,
}

/// The on-disk shape. Deserialising goes through [`ValuationInput::new`], so an
/// unlabelled input or one believed at a confidence of 50 is refused at load
/// and not discovered after it has already scaled a mark.
#[derive(Deserialize)]
struct ValuationInputWire {
    label: String,
    value: Decimal,
    confidence: f64,
    known_at: Timestamp,
}

impl TryFrom<ValuationInputWire> for ValuationInput {
    type Error = Error;

    fn try_from(wire: ValuationInputWire) -> Result<Self> {
        Self::new(wire.label, wire.value, wire.confidence, wire.known_at)
    }
}

impl ValuationInput {
    /// Refuses an unlabelled input, a non-positive value and a confidence
    /// outside `(0, 1]`.
    ///
    /// The label is required because an input nobody can name is an input
    /// nobody can check, and the whole point of carrying inputs is that a
    /// person can re-derive the mark from them.
    pub fn new(
        label: impl Into<String>,
        value: Decimal,
        confidence: f64,
        known_at: Timestamp,
    ) -> Result<Self> {
        let label = label.into();
        if label.trim().is_empty() {
            return Err(Error::invalid(
                "a valuation input needs a label naming what it is; an unlabelled input cannot be \
                 checked by the person the mark has to convince",
            ));
        }
        if !value.is_positive() {
            return Err(Error::invalid(format!(
                "the valuation input {label} is {value}; supply a strictly positive observation, \
                 and omit the input rather than recording a zero"
            )));
        }
        if !confidence.is_finite() || confidence <= 0.0 || confidence > 1.0 {
            return Err(Error::invalid(format!(
                "the valuation input {label} carries a confidence of {confidence}; supply one in \
                 (0, 1] — an input believed with zero confidence is not evidence"
            )));
        }
        Ok(Self {
            label,
            value,
            confidence,
            known_at,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn value(&self) -> Decimal {
        self.value
    }

    pub const fn confidence(&self) -> f64 {
        self.confidence
    }

    pub const fn known_at(&self) -> Timestamp {
        self.known_at
    }
}

/// A mark on an asset, carrying how it was arrived at and how much it is to be
/// believed.
///
/// Named `AssetValuation` rather than `Valuation` deliberately:
/// `qip_portfolio::Valuation` is a whole-book snapshot, and two types with one
/// name across the crates that both use them is how a reviewer reads the wrong
/// invariant into a diff.
///
/// Fields are private and there is no public constructor. A mark is either
/// struck by [`IlliquidValuator`] or read back from a document, and
/// `serde(try_from)` routes the second through [`IlliquidValuator::assemble`],
/// which is the first's own evidence check — so both mints run one check
/// rather than two that can drift apart.
///
/// **`#[derive(Deserialize)]` on its own was the second constructor this doc
/// used to deny existed.** It wrote straight to the private fields, and a
/// document stating a confidence of 50, a value of zero, an empty asset id, no
/// inputs at all, or a review date centuries out was accepted as a mark. The
/// last is the worst of them: `next_review` is the only thing
/// [`Self::is_stale`] consults, so a review date nobody computed is a mark that
/// never falls due and goes on supporting leverage at full confidence forever.
/// The wire is therefore required to carry the review date the method mandates
/// rather than having it recomputed — a document that disagrees with the method
/// it names is a corrupt document, and silently replacing its figure would hide
/// that.
///
/// **What this does not establish, and no check inside this type could.** The
/// mark's `value` is not re-derived from its `inputs`: only
/// [`IlliquidValuator::from_comparables`] computes the value from them, the
/// others take it as the observation itself, and a read-path check the write
/// path does not run would refuse marks this platform produced. Nor is `asset`
/// known to name an object in any universe, or `as_of` known to be when anyone
/// looked. A document is trusted exactly as far as the log it was read from is,
/// and what makes that trustworthy is the hash chain on the event log, not this
/// type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AssetValuationWire")]
pub struct AssetValuation {
    asset: String,
    value: Decimal,
    method: ValuationMethod,
    /// The evidence, keyed by label so a replay reports it in one order.
    inputs: BTreeMap<String, ValuationInput>,
    /// Confidence on the day the mark was struck. A statistic.
    confidence: f64,
    as_of: Timestamp,
    next_review: Timestamp,
}

/// The on-disk shape. Every field arrives unchecked and none of them reaches an
/// [`AssetValuation`] without passing [`IlliquidValuator::assemble`].
#[derive(Deserialize)]
struct AssetValuationWire {
    asset: String,
    value: Decimal,
    method: ValuationMethod,
    inputs: BTreeMap<String, ValuationInput>,
    confidence: f64,
    as_of: Timestamp,
    next_review: Timestamp,
}

impl TryFrom<AssetValuationWire> for AssetValuation {
    type Error = Error;

    fn try_from(wire: AssetValuationWire) -> Result<Self> {
        // The map key and the input's own label are two claims about the same
        // fact, and a document is the one place they can disagree: `assemble`
        // builds the key from the label, so a mark that files `quote` under
        // `acquisition_cost` is a mark whose evidence is reported under a name
        // nobody can reconcile it by.
        for (key, input) in &wire.inputs {
            if key != input.label() {
                return Err(Error::invalid(format!(
                    "the {} mark on {} files the input {} under the key {key}; key each input by \
                     its own label, because the key is what a person re-deriving the mark looks \
                     it up by",
                    wire.method.label(),
                    wire.asset,
                    input.label()
                )));
            }
        }
        let declared_review = wire.next_review;
        let mark = IlliquidValuator::assemble(
            wire.asset,
            wire.value,
            wire.method,
            wire.inputs.into_values().collect(),
            wire.confidence,
            wire.as_of,
        )?;
        if mark.next_review != declared_review {
            return Err(Error::invalid(format!(
                "the {} mark on {} falls due for review at {} but the record says {}; a {} mark is \
                 reviewed {} days after it is struck, and a review date nobody computed is a mark \
                 that never goes stale",
                mark.method.label(),
                mark.asset,
                mark.next_review.to_rfc3339(),
                declared_review.to_rfc3339(),
                mark.method.label(),
                mark.method.review_interval().as_days_f64()
            )));
        }
        Ok(mark)
    }
}

impl AssetValuation {
    pub fn asset(&self) -> &str {
        &self.asset
    }

    pub const fn value(&self) -> Decimal {
        self.value
    }

    pub const fn method(&self) -> ValuationMethod {
        self.method
    }

    pub fn inputs(&self) -> impl Iterator<Item = &ValuationInput> {
        self.inputs.values()
    }

    /// Confidence as struck, before any decay.
    pub const fn struck_confidence(&self) -> f64 {
        self.confidence
    }

    pub const fn as_of(&self) -> Timestamp {
        self.as_of
    }

    pub const fn next_review(&self) -> Timestamp {
        self.next_review
    }

    /// Whether the mark has passed the instant it had to be refreshed by.
    pub fn is_stale(&self, now: Timestamp) -> bool {
        now > self.next_review
    }

    /// Confidence at `now`, halved once per the method's half-life.
    ///
    /// Refuses an instant before the mark was struck: a confidence that grew
    /// backwards in time would make an old mark look better the earlier you
    /// asked about it.
    pub fn confidence_at(&self, now: Timestamp) -> Result<f64> {
        if now < self.as_of {
            return Err(Error::invalid(format!(
                "the mark on {} was struck at {} and its confidence cannot be read as of {}; a \
                 mark carries no weight before it exists",
                self.asset,
                self.as_of.to_rfc3339(),
                now.to_rfc3339()
            )));
        }
        let age = now.since(self.as_of).as_days_f64();
        let half_life = self.method.decay_half_life().as_days_f64();
        if half_life <= 0.0 {
            return Err(Error::numeric(format!(
                "method {} declares a half-life of {half_life} days, which cannot decay a mark",
                self.method.label()
            )));
        }
        Ok(self.confidence * 0.5_f64.powf(age / half_life))
    }

    /// The mark reduced to the part of it the platform is willing to stand
    /// behind at `now`.
    ///
    /// **Statistic meets money here.** Confidence is `f64`; the mark is
    /// [`Decimal`]. The confidence crosses into `Decimal` once, at this
    /// conversion, and the product is exact decimal arithmetic — a
    /// risk-bearing figure is never carried through binary floating point.
    /// This is the arithmetic behind §16.3's rule that an uncertain mark
    /// cannot silently support leverage.
    pub fn supportable_value(&self, now: Timestamp) -> Result<Decimal> {
        let confidence = self.confidence_at(now)?;
        let weight = Decimal::from_f64(confidence).ok_or_else(|| {
            Error::numeric(format!(
                "the decayed confidence {confidence} on {} cannot be represented at decimal scale",
                self.asset
            ))
        })?;
        self.value.checked_mul(weight).ok_or_else(|| {
            Error::numeric(format!(
                "weighting the mark {} on {} by {confidence} overflows",
                self.value, self.asset
            ))
        })
    }

    /// A haircut on a notional, taken at this mark's decayed confidence.
    ///
    /// Money in, money out: the haircut is applied to a [`Decimal`] notional
    /// and the result is [`Decimal`]. The confidence is the only `f64` and it
    /// crosses in [`Self::supportable_value`]'s sibling conversion below.
    pub fn haircut(&self, notional: Decimal, now: Timestamp) -> Result<Decimal> {
        let confidence = self.confidence_at(now)?;
        let weight = Decimal::from_f64(confidence).ok_or_else(|| {
            Error::numeric(format!(
                "the decayed confidence {confidence} on {} cannot be represented at decimal scale",
                self.asset
            ))
        })?;
        let supported = notional
            .checked_mul(weight)
            .ok_or_else(|| Error::numeric("the haircut computation overflows".to_string()))?;
        Ok(notional - supported)
    }
}

/// Produces marks from evidence, and refuses when there is none.
///
/// A unit struct rather than free functions so that the refusal surface is one
/// named thing a reader can go and audit in full.
#[derive(Clone, Copy, Debug, Default)]
pub struct IlliquidValuator;

impl IlliquidValuator {
    /// Common construction, after a method's own evidence check has passed.
    ///
    /// Also the path deserialisation takes, so a mark read back from a document
    /// meets the same refusals as one struck here rather than a weaker set
    /// written twice.
    fn assemble(
        asset: String,
        value: Decimal,
        method: ValuationMethod,
        inputs: Vec<ValuationInput>,
        confidence: f64,
        as_of: Timestamp,
    ) -> Result<AssetValuation> {
        if asset.trim().is_empty() {
            return Err(Error::invalid(
                "a valuation needs the object id it marks; an unattributed mark cannot be \
                 reconciled against the position it prices",
            ));
        }
        if !value.is_positive() {
            return Err(Error::invalid(format!(
                "the {} mark on {asset} is {value}; a mark must be strictly positive — an asset \
                 worth nothing is written off, not marked",
                method.label()
            )));
        }
        if !confidence.is_finite() || confidence <= 0.0 || confidence > 1.0 {
            return Err(Error::invalid(format!(
                "the {} mark on {asset} carries a confidence of {confidence}; supply one in \
                 (0, 1] — a mark believed with zero confidence is not a mark",
                method.label()
            )));
        }
        // Every constructor above supplies at least one input, so this refuses
        // nothing this platform strikes. It exists because the deserialisation
        // path reaches here too, and a mark with an empty evidence set is the
        // fabrication the module opens by refusing: a number wearing a method
        // label, with nothing behind it a person could go and check.
        if inputs.is_empty() {
            return Err(Error::invalid(format!(
                "the {} mark on {asset} names no input it was derived from; supply the observation \
                 it was struck from — a mark carrying no evidence cannot be re-derived by the \
                 person it has to convince",
                method.label()
            )));
        }
        let mut keyed = BTreeMap::new();
        for input in inputs {
            if input.known_at() > as_of {
                return Err(Error::invalid(format!(
                    "the input {} for the {} mark on {asset} became knowable at {} , after the \
                     valuation instant {}; mark {asset} as of {} or later, because a mark that \
                     reads the future is a point-in-time leak however good the backtest looks",
                    input.label(),
                    method.label(),
                    input.known_at().to_rfc3339(),
                    as_of.to_rfc3339(),
                    input.known_at().to_rfc3339()
                )));
            }
            if keyed.contains_key(input.label()) {
                return Err(Error::invalid(format!(
                    "the {} mark on {asset} names the input {} twice; label each observation \
                     distinctly so the mark can be re-derived from what it says it used",
                    method.label(),
                    input.label()
                )));
            }
            keyed.insert(input.label().to_string(), input);
        }
        Ok(AssetValuation {
            asset,
            value,
            method,
            inputs: keyed,
            confidence,
            as_of,
            next_review: as_of.saturating_add(method.review_interval()),
        })
    }

    /// A quoted mark: a continuous two-sided market exists.
    pub fn from_quote(
        asset: impl Into<String>,
        price: Decimal,
        quoted_at: Timestamp,
        as_of: Timestamp,
    ) -> Result<AssetValuation> {
        let asset = asset.into();
        let input = ValuationInput::new("quote", price, 1.0, quoted_at)?;
        Self::assemble(
            asset,
            price,
            ValuationMethod::Quoted,
            vec![input],
            ValuationMethod::Quoted.base_confidence(),
            as_of,
        )
    }

    /// A comparables mark: the mean of observed transactions in similar
    /// assets, each adjusted before it is handed in.
    ///
    /// Refuses an empty comparable set by name. This is the refusal the module
    /// exists for: with no observable comparable there is no mark, and the
    /// alternative — returning the caller's own guess wearing the
    /// `Comparables` label — is a fabricated number that will be believed at
    /// 0.65 confidence by everything downstream.
    pub fn from_comparables(
        asset: impl Into<String>,
        comparables: Vec<ValuationInput>,
        as_of: Timestamp,
    ) -> Result<AssetValuation> {
        let asset = asset.into();
        if comparables.is_empty() {
            return Err(Error::invalid(format!(
                "{asset} has no observable comparable transaction, so no comparables mark exists; \
                 supply at least one adjusted comparable, or mark it by another method — this \
                 plane does not invent a mark"
            )));
        }
        let count = i64::try_from(comparables.len()).map_err(|_| {
            Error::numeric(format!(
                "{asset} names more comparables than can be counted"
            ))
        })?;
        let mut total = Decimal::ZERO;
        let mut weakest = 1.0_f64;
        for comparable in &comparables {
            total = total
                .checked_add(comparable.value())
                .ok_or_else(|| Error::numeric("the comparable total overflows".to_string()))?;
            weakest = weakest.min(comparable.confidence());
        }
        let value = total.checked_div(Decimal::from_int(count)).ok_or_else(|| {
            Error::numeric("averaging the comparables divides by zero".to_string())
        })?;
        // Statistic meets statistic: the method's own confidence is scaled by
        // the weakest input's, because a mark is no better than the worst
        // observation it leans on. No money crosses here.
        let confidence = ValuationMethod::Comparables.base_confidence() * weakest;
        Self::assemble(
            asset,
            value,
            ValuationMethod::Comparables,
            comparables,
            confidence,
            as_of,
        )
    }

    /// A discounted-cashflow mark: the present value of a forecast stream.
    ///
    /// Refuses a forecast that was not knowable at `as_of`, a non-positive
    /// present value, and — through [`CashflowForecast::present_value`] — an
    /// empty schedule and a discount factor that is not positive.
    pub fn from_discounted_cashflow(
        asset: impl Into<String>,
        forecast: &CashflowForecast,
        discount_rate: f64,
        as_of: Timestamp,
    ) -> Result<AssetValuation> {
        let asset = asset.into();
        if forecast.subject() != asset {
            return Err(Error::invalid(format!(
                "the forecast for {} cannot mark {asset}; discount the subject's own schedule",
                forecast.subject()
            )));
        }
        let present_value = forecast.present_value(as_of, discount_rate)?;
        if !present_value.is_positive() {
            return Err(Error::invalid(format!(
                "discounting {asset} at {discount_rate} gives a present value of {present_value}; \
                 a stream whose expected calls exceed its expected distributions is a liability, \
                 not a mark — record it as a commitment instead"
            )));
        }
        let input = ValuationInput::new(
            "discounted_cashflow",
            present_value,
            1.0,
            forecast.known_at(),
        )?;
        Self::assemble(
            asset,
            present_value,
            ValuationMethod::DiscountedCashflow,
            vec![input],
            ValuationMethod::DiscountedCashflow.base_confidence(),
            as_of,
        )
    }

    /// A last-round mark: the most recent primary transaction price.
    ///
    /// Refuses a round dated after the valuation instant. A February round
    /// used to mark a January book is the point-in-time leak in its purest
    /// form — the mark is right, and the platform could not have known it.
    pub fn from_last_round(
        asset: impl Into<String>,
        price: Decimal,
        round_at: Timestamp,
        as_of: Timestamp,
    ) -> Result<AssetValuation> {
        let asset = asset.into();
        let input = ValuationInput::new("last_round", price, 1.0, round_at)?;
        Self::assemble(
            asset,
            price,
            ValuationMethod::LastRound,
            vec![input],
            ValuationMethod::LastRound.base_confidence(),
            as_of,
        )
    }

    /// A cost mark: held at acquisition cost absent anything better.
    ///
    /// The lowest confidence in the table, and an admission of ignorance
    /// rather than a valuation. It exists so that "we do not know" has a
    /// representation other than a confident number.
    pub fn at_cost(
        asset: impl Into<String>,
        cost: Decimal,
        acquired_at: Timestamp,
        as_of: Timestamp,
    ) -> Result<AssetValuation> {
        let asset = asset.into();
        let input = ValuationInput::new("acquisition_cost", cost, 1.0, acquired_at)?;
        Self::assemble(
            asset,
            cost,
            ValuationMethod::Cost,
            vec![input],
            ValuationMethod::Cost.base_confidence(),
            as_of,
        )
    }

    /// Mark a private-asset record from what the object model already carries,
    /// in descending order of evidence, and refuse when it carries none.
    ///
    /// The ladder is the §16.3 table applied to the only observations the
    /// record actually holds:
    ///
    /// 1. **Discounted cashflow** where the manager reports a residual value,
    ///    the record carries a required yield, and the lockup has not run out
    ///    — a single distribution at the end of the lockup, discounted. Every
    ///    input is on the record; nothing is assumed.
    /// 2. **Last round** where a residual value is reported but no rate exists
    ///    to discount it at. The manager's own figure, believed as a primary
    ///    report and decayed at the last-round half-life.
    /// 3. **Cost** where capital has been called and not returned, and no
    ///    residual is reported.
    /// 4. **Refusal**, naming what to supply. A private position with no
    ///    residual, no net cost and no schedule cannot be marked, and this
    ///    returns an error rather than a zero — a zero would be summed into
    ///    book equity as though somebody had observed it.
    ///
    /// **Three instants, and confusing any two of them is a defect.**
    ///
    /// * `observed_at` — when the evidence was true. Every rung of the ladder
    ///   is struck as of this instant, so the mark's confidence decays with
    ///   the age of the administrator's report and its review falls due a
    ///   fixed interval after that report, exactly as §16.3 says. It is the
    ///   only instant on the mark, and it comes off the record.
    /// * `known_at` — when the platform could first have read the record.
    ///   Bounds readability rather than dating the evidence.
    /// * `read_at` — the instant the caller is asking as of. It buys one
    ///   refusal and nothing else: a record not yet knowable at `read_at` is
    ///   not marked, because a mark readable before it was knowable is a
    ///   point-in-time leak and a backtest resting on it is better than
    ///   reality by exactly the information it should not have had.
    ///
    /// `observed_at` after `known_at` is refused rather than reconciled. Such
    /// a record claims its evidence was true after the platform wrote it down,
    /// and there is no instant to date the mark from that is not a guess —
    /// taking the later would let a vendor field make a 2010 report decay as
    /// though it were current, and taking the earlier would silently rewrite
    /// the vendor's own claim.
    pub fn mark_private_asset(
        asset: impl Into<String>,
        details: &PrivateAssetDetails,
        origin: Timestamp,
        observed_at: Timestamp,
        known_at: Timestamp,
        required_yield: Option<f64>,
        read_at: Timestamp,
    ) -> Result<AssetValuation> {
        let asset = asset.into();
        if observed_at > known_at {
            return Err(Error::invalid(format!(
                "{asset} reports evidence observed at {} but became knowable at {}; correct the \
                 record — a mark cannot be dated from an observation the platform recorded before \
                 it happened, and this plane will not pick one of the two instants for you",
                observed_at.to_rfc3339(),
                known_at.to_rfc3339()
            )));
        }
        if known_at > read_at {
            return Err(Error::invalid(format!(
                "{asset} became knowable at {} and cannot be marked as of {}; mark it at {} or \
                 later, because a mark that reads the future is a point-in-time leak however good \
                 the backtest looks",
                known_at.to_rfc3339(),
                read_at.to_rfc3339(),
                known_at.to_rfc3339()
            )));
        }
        if details.residual_value.is_positive() {
            let lockup_end = origin.saturating_add(details.lockup()?);
            if let Some(rate) = required_yield.filter(|r| r.is_finite() && *r > -1.0)
                && lockup_end > observed_at
            {
                let forecast = CashflowForecast::new(asset.clone(), origin, observed_at)?
                    .with_flow(crate::cashflow::ForecastCashflow::new(
                        crate::cashflow::CashflowKind::Distribution,
                        lockup_end,
                        details.residual_value,
                        1.0,
                    )?)?;
                return Self::from_discounted_cashflow(asset, &forecast, rate, observed_at);
            }
            return Self::from_last_round(asset, details.residual_value, observed_at, observed_at);
        }
        let net_cost = details.called_capital - details.distributed_capital;
        if net_cost.is_positive() {
            return Self::at_cost(asset, net_cost, observed_at, observed_at);
        }
        Err(Error::invalid(format!(
            "{asset} reports no residual value and no capital called beyond what has been \
             distributed, so there is nothing observable to mark it from; supply the manager's \
             reported residual value or a transaction price — this plane refuses to invent a mark"
        )))
    }

    /// Mark a financial object that has no continuous price.
    ///
    /// Returns `Ok(None)` for an object this engine has no business marking —
    /// one that is not a private asset — so a caller sweeping a universe can
    /// tell "not my instrument" from "your instrument cannot be marked",
    /// which are different facts and must not share a representation.
    ///
    /// **The decay origin is `provenance.event_time`, not `updated_at` and
    /// certainly not `read_at`.** `event_time` is when the fact was true —
    /// the administrator's reporting date — and staleness is a statement about
    /// the evidence, so that is the instant to measure from. `updated_at` is
    /// when this platform last rewrote its copy, which a nightly re-ingestion
    /// moves without any new evidence existing; marking from it would refresh
    /// a decade-old report every night. `read_at` is worse still: it is the
    /// clock, and a mark struck from the clock decays with process uptime.
    ///
    /// `event_time` is trustworthy here in the one way that matters:
    /// `ObjectBuilder::build` refuses an object whose `ingestion_time`
    /// precedes its `event_time` ("record was ingested before it happened"),
    /// so no object in a universe carries a fact dated after the platform
    /// ingested it. It can still be dated after `updated_at`, which is a
    /// separate stamp nothing reconciles against it, and
    /// [`Self::mark_private_asset`] refuses that case rather than choosing.
    pub fn mark_object(
        object: &FinancialObject,
        origin: Timestamp,
        read_at: Timestamp,
    ) -> Result<Option<AssetValuation>> {
        let Extension::PrivateAsset(details) = &object.extension else {
            return Ok(None);
        };
        Self::mark_private_asset(
            object.object_id.as_str().to_string(),
            details,
            origin,
            object.provenance.event_time,
            object.updated_at,
            object.yield_rate,
            read_at,
        )
        .map(Some)
    }
}
