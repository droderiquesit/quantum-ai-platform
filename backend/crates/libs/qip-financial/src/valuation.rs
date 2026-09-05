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
//! one and will support leverage on its own authority.
//!
//! **Marks decay.** A last-round valuation six months old carries less
//! confidence than one six days old, and [`AssetValuation::confidence_at`]
//! reduces its weight rather than treating it as equally true. Confidence is
//! `f64` because it is a statistic; the mark is [`Decimal`] because it is
//! money. The crossing point is marked where a confidence multiplies a value.
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValuationInput {
    label: String,
    value: Decimal,
    /// How much this input is itself to be trusted. A statistic.
    confidence: f64,
    /// When this input became knowable to the platform.
    known_at: Timestamp,
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
/// Fields are private. There is no way to construct one except through
/// [`IlliquidValuator`], and therefore no way to produce a mark that did not
/// pass an evidence check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// `known_at` is the instant the record became knowable, and every derived
    /// input is stamped with it, so [`Self::assemble`]'s knowability check
    /// refuses a mark struck before the record existed.
    pub fn mark_private_asset(
        asset: impl Into<String>,
        details: &PrivateAssetDetails,
        origin: Timestamp,
        known_at: Timestamp,
        required_yield: Option<f64>,
        as_of: Timestamp,
    ) -> Result<AssetValuation> {
        let asset = asset.into();
        if details.residual_value.is_positive() {
            let lockup_end =
                origin.saturating_add(Duration::from_days((details.lockup_years * 365.0) as i64));
            if let Some(rate) = required_yield.filter(|r| r.is_finite() && *r > -1.0)
                && lockup_end > as_of
            {
                let forecast = CashflowForecast::new(asset.clone(), origin, known_at)?.with_flow(
                    crate::cashflow::ForecastCashflow::new(
                        crate::cashflow::CashflowKind::Distribution,
                        lockup_end,
                        details.residual_value,
                        1.0,
                    )?,
                )?;
                return Self::from_discounted_cashflow(asset, &forecast, rate, as_of);
            }
            return Self::from_last_round(asset, details.residual_value, known_at, as_of);
        }
        let net_cost = details.called_capital - details.distributed_capital;
        if net_cost.is_positive() {
            return Self::at_cost(asset, net_cost, known_at, as_of);
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
    pub fn mark_object(
        object: &FinancialObject,
        origin: Timestamp,
        as_of: Timestamp,
    ) -> Result<Option<AssetValuation>> {
        let Extension::PrivateAsset(details) = &object.extension else {
            return Ok(None);
        };
        Self::mark_private_asset(
            object.object_id.as_str().to_string(),
            details,
            origin,
            object.updated_at,
            object.yield_rate,
            as_of,
        )
        .map(Some)
    }
}
