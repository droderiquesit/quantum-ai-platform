//! Transaction costs.
//!
//! A backtest without costs is a backtest of a strategy nobody can run. But a
//! backtest with *its own* costs is worse: it is a second opinion about what an
//! order costs, and the desk only ever hears the louder one.
//!
//! **There is one transaction-cost model on this platform and it is
//! [`TransactionCostModel`], in `qip-financial`.** Everything below is a
//! simulation *policy* wrapped around it, and every currency figure this module
//! produces comes out of that type's own arithmetic. This file used to hold a
//! complete second model — commission, half-spread, square-root impact and
//! borrow, in `f64` — and the two had already drifted apart in public:
//!
//! * a different half-spread on the shipped default, 3.0bp here against 2.5bp
//!   there, so a backtest paid twenty per cent more spread than the pre-trade
//!   check would have quoted for the same order;
//! * a different impact law, `k · σ_daily · sqrt(p) · 10⁴` here against
//!   `k_bps · sqrt(min(p, 4))` there, which at a two per cent daily volatility
//!   priced impact at five times the pre-trade figure;
//! * `f64` here and [`Decimal`] there, so the two also disagreed about what
//!   money is.
//!
//! A strategy that looked profitable in simulation and was not in the pre-trade
//! path is the failure this platform's whole evidence chain exists to prevent,
//! and two cost models are how it happens.
//!
//! # What the simulation still states for itself, and why
//!
//! Two things, and the line between them is deliberate: **the simulation keeps
//! only what it alone can measure, and nothing that is a term of a broker's
//! contract.**
//!
//! * [`CostModel::maximum_participation`] — a refusal, not a price. The
//!   square-root law is calibrated on modest participation, and beyond this the
//!   simulation reports the order as unfillable rather than quoting a number
//!   that looks like an answer. [`TransactionCostModel`] has no notion of
//!   declining to quote, because a pre-trade check that refused would be a risk
//!   control living in the wrong crate.
//! * [`CostModel::reference_daily_volatility`] — the daily volatility at which
//!   [`TransactionCostModel::impact_coefficient_bps`] is quoted. A backtest is
//!   the one caller that has measured the instrument's realised volatility over
//!   the bars it is about to trade on, so it scales that one coefficient before
//!   handing it back to the shared law. The law itself — the square root, the
//!   participation cap, the guards — is not restated here.
//!
//! Both are stated assumptions rather than measurements, and
//! [`CostModel::describe`] says so, because a cost model presented as fact is
//! how a strategy that only works at zero impact reaches production.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_financial::costs::TransactionCostModel;
use serde::{Deserialize, Serialize};

/// Trading days in a year, for restating an annualised volatility as a daily
/// one. The same figure `RiskMetrics` annualises with.
const TRADING_DAYS_PER_YEAR: f64 = 252.0;

/// The annualised volatility a liquid listed equity is conventionally taken to
/// have, and therefore the volatility [`TransactionCostModel`]'s impact
/// coefficient is read as being quoted at.
const REFERENCE_ANNUAL_VOLATILITY: f64 = 0.20;

/// The daily volatility [`TransactionCostModel::impact_coefficient_bps`] is
/// quoted at.
///
/// `qip-financial` documents that coefficient as "roughly the impact of trading
/// 100% of a day's volume", with liquid-equity estimates clustering at 30-60bp,
/// and states no volatility alongside it. It has to be quoted at *some*
/// volatility — impact is a price move and a price move is measured in units of
/// volatility — so the simulation names the one implied by that sentence rather
/// than leaving it implicit in a coefficient nobody can interpret.
///
/// This is a stated assumption. It is not a measurement, and it is the figure
/// to change if the desk calibrates impact properly. What it must never become
/// again is *unstated*: the coefficient this file shipped before, `k = 1.0`
/// against a law that multiplied by `10⁴`, implied a reference of 0.4% a day —
/// about six per cent a year, which no listed equity has — and nothing in the
/// file said so or could have been checked.
pub fn reference_daily_volatility() -> f64 {
    REFERENCE_ANNUAL_VOLATILITY / TRADING_DAYS_PER_YEAR.sqrt()
}

/// Cost parameters. Every field is a stated assumption.
///
/// The six pricing fields are [`TransactionCostModel`]'s own, carried by value
/// because `qip-twin` hands this type out of a `const fn` and so needs it
/// `Copy`, which [`TransactionCostModel`] is not. They are never *restated*:
/// every constructor below builds one from a `qip-financial` constructor and
/// copies it in through [`CostModel::of`], and
/// `the_simulations_default_costs_are_the_platforms_own` fails if the two ever
/// disagree again.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostModel {
    /// Commission and exchange fees, in basis points of notional.
    pub commission_bps: f64,
    /// Fixed cost per order, in the instrument currency.
    pub fixed_fee: Decimal,
    /// Half-spread paid per side, in basis points.
    pub half_spread_bps: f64,
    /// Impact of trading a full day's volume, in basis points, at
    /// [`Self::reference_daily_volatility`].
    pub impact_coefficient_bps: f64,
    /// Taxes and levies, in basis points.
    pub tax_bps: f64,
    /// Borrow cost for a short position, in basis points per year.
    pub short_borrow_bps_annual: f64,
    /// The daily volatility [`Self::impact_coefficient_bps`] is quoted at.
    ///
    /// See [`reference_daily_volatility`] for why this is stated rather than
    /// folded into the coefficient.
    pub reference_daily_volatility: f64,
    /// Participation above which the model refuses to quote a cost.
    ///
    /// The square-root law is calibrated on modest participation. Extrapolating
    /// it to a third of a day's volume produces a number that looks like an
    /// answer and is not one, so beyond this the simulation reports the order
    /// as unfillable rather than pricing it.
    pub maximum_participation: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self::liquid_equity()
    }
}

impl CostModel {
    /// Wrap the platform's cost model in the simulation's own policy.
    ///
    /// Every constructor below goes through here, so no pricing figure in this
    /// crate is *written* in this crate: each one is copied off a
    /// `qip-financial` model. The fields stay public because a
    /// [`crate::backtest::BacktestConfig`] is a configuration record that serde
    /// round-trips, and a run that states its own costs must be able to say so
    /// — but a run that states nothing inherits `qip-financial`'s figures
    /// rather than a second set that looks like them.
    fn of(pricing: &TransactionCostModel, maximum_participation: f64) -> Self {
        Self {
            commission_bps: pricing.commission_bps,
            fixed_fee: pricing.fixed_fee,
            half_spread_bps: pricing.half_spread_bps,
            impact_coefficient_bps: pricing.impact_coefficient_bps,
            tax_bps: pricing.tax_bps,
            short_borrow_bps_annual: pricing.short_borrow_bps_annual,
            reference_daily_volatility: reference_daily_volatility(),
            maximum_participation,
        }
    }

    /// Costs an institution would face in a liquid market.
    ///
    /// Exactly [`TransactionCostModel::default`] — not a restatement of it —
    /// refusing orders above a fifth of a day's volume.
    pub fn liquid_equity() -> Self {
        Self::of(&TransactionCostModel::default(), 0.20)
    }

    /// A less liquid market: wider spreads, more impact, harder borrow.
    ///
    /// A 50bp quote, so `qip-financial`'s own `listed` constructor sets the
    /// half-spread; the three figures that depart from a liquid name are named
    /// here and nothing else is.
    pub fn small_cap() -> Self {
        Self::of(
            &TransactionCostModel {
                commission_bps: 2.0,
                // 1.8x the liquid-equity coefficient, the ratio this profile
                // has always carried.
                impact_coefficient_bps: TransactionCostModel::default().impact_coefficient_bps
                    * 1.8,
                short_borrow_bps_annual: 400.0,
                ..TransactionCostModel::listed(50.0)
            },
            0.10,
        )
    }

    /// Zero costs. Available for isolating a signal's raw predictive content,
    /// and never for a result presented as achievable.
    pub fn frictionless() -> Self {
        Self::of(
            &TransactionCostModel {
                commission_bps: 0.0,
                fixed_fee: Decimal::ZERO,
                half_spread_bps: 0.0,
                impact_coefficient_bps: 0.0,
                tax_bps: 0.0,
                short_borrow_bps_annual: 0.0,
            },
            1.0,
        )
    }

    /// The platform's cost model, as this simulation states it.
    ///
    /// Exposed so a caller — or a test — can put the backtest's parameters and
    /// the pre-trade check's side by side and see that they are one object.
    ///
    /// # Why this is fallible, when a struct literal never was
    ///
    /// A [`TransactionCostModel`] that arrives by deserialisation goes through
    /// [`TransactionCostModel::checked`], and one that arrives on a reference
    /// record goes through `FinancialObject::validate`. **A struct literal goes
    /// through neither**, and this function and [`Self::pricing_at`] were the
    /// literals: `qip-financial`'s own doc names them as the hole its wire
    /// guard could not close, because a computed model is not a file and not a
    /// record and no check inside that type can see it. So the check happens
    /// here, at the seam where the model is computed, and the two ways in are
    /// held to one bound rather than to whichever one the value happened to
    /// take.
    ///
    /// [`Self::validate`] is not that check. It refuses a figure that is not a
    /// finite non-negative number and stops there; it has no ceiling, so a
    /// coefficient of 50,000bp — five times the whole notional per trade —
    /// passes it and would be priced.
    pub fn pricing(&self) -> Result<TransactionCostModel> {
        TransactionCostModel {
            commission_bps: self.commission_bps,
            fixed_fee: self.fixed_fee,
            half_spread_bps: self.half_spread_bps,
            impact_coefficient_bps: self.impact_coefficient_bps,
            tax_bps: self.tax_bps,
            short_borrow_bps_annual: self.short_borrow_bps_annual,
        }
        .checked()
    }

    /// The platform's cost model as it applies to an instrument whose measured
    /// daily volatility is `daily_volatility`.
    ///
    /// The *only* thing the simulation changes is the impact coefficient, and
    /// it changes it in proportion: an instrument twice as volatile as the
    /// reference moves twice as far for the same participation. The square-root
    /// law, its participation cap and its guards stay in `qip-financial`, so
    /// there is one impact formula on this platform and this is not a second
    /// one.
    ///
    /// A volatility that is missing, zero or not finite yields a zero
    /// coefficient rather than a guessed one. Substituting the reference would
    /// charge a figure nobody measured, and charging the reference is worse
    /// than charging nothing: a zero impact is visibly a zero in
    /// [`crate::backtest::BacktestResult::total_impact`], where a substituted
    /// reference reads as a measurement.
    ///
    /// # The scaled coefficient is held to the same ceiling as a wire record
    ///
    /// This multiplication is the one place in the crate that can *manufacture*
    /// a figure `qip-financial` would have refused on load: the scale is a
    /// measured volatility over a stated reference and nothing bounded it, so a
    /// volatility series with one bad bar in it produced a coefficient no
    /// document could have carried, and `apply_bps` priced whatever came out.
    ///
    /// It is checked against `MAX_TRADE_COST_BPS` — the reference bound, not a
    /// wider one invented for computed models — and the reason is arithmetic
    /// rather than deference. That bound is 10,000bp: one component of one
    /// trade costing the entire notional. Impact is a share of the notional
    /// whatever volatility produced it, so there is no volatility at which
    /// "this trade costs more than the thing being traded" becomes a modelling
    /// requirement rather than a data error. The headroom is not tight: at the
    /// default 40bp coefficient the ceiling is a scale of 250, which is a
    /// **315% daily** volatility. A genuine crisis — 12.6% a day, ten times the
    /// reference — scales to 400bp and prices, which is what
    /// `a_volatility_a_crisis_could_produce_still_prices` holds. A distinct,
    /// wider bound for computed models would therefore buy no legitimate
    /// simulation anything, and would cost the property that the backtest and
    /// the pre-trade check refuse the same figures.
    pub fn pricing_at(&self, daily_volatility: f64) -> Result<TransactionCostModel> {
        let pricing = self.pricing()?;
        TransactionCostModel {
            impact_coefficient_bps: pricing.impact_coefficient_bps
                * self.volatility_scale(daily_volatility),
            ..pricing
        }
        .checked()
    }

    /// Measured volatility as a multiple of the reference it is quoted against.
    fn volatility_scale(&self, daily_volatility: f64) -> f64 {
        let reference = self.reference_daily_volatility;
        if !(daily_volatility.is_finite()
            && daily_volatility > 0.0
            && reference.is_finite()
            && reference > 0.0)
        {
            return 0.0;
        }
        daily_volatility / reference
    }

    /// Market impact in basis points, from the one square-root law.
    ///
    /// Refused rather than answered where [`Self::pricing_at`] refuses: an
    /// impact figure taken from a model the platform would not load is a number
    /// with no model behind it.
    pub fn impact_bps(&self, participation: f64, daily_volatility: f64) -> Result<f64> {
        Ok(self.pricing_at(daily_volatility)?.impact_bps(participation))
    }

    pub fn is_frictionless(&self) -> bool {
        self.commission_bps == 0.0
            && self.tax_bps == 0.0
            && self.half_spread_bps == 0.0
            && self.impact_coefficient_bps == 0.0
            && self.fixed_fee.is_zero()
    }

    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("commission_bps", self.commission_bps),
            ("half_spread_bps", self.half_spread_bps),
            ("impact_coefficient_bps", self.impact_coefficient_bps),
            ("tax_bps", self.tax_bps),
            ("short_borrow_bps_annual", self.short_borrow_bps_annual),
        ] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(Error::invalid(format!(
                    "{name} must be a finite, non-negative number of basis points; it was {value}"
                )));
            }
        }
        if self.fixed_fee.is_negative() {
            return Err(Error::invalid("a fixed fee cannot be negative"));
        }
        if !(self.reference_daily_volatility.is_finite() && self.reference_daily_volatility > 0.0) {
            // Zero would make every impact figure a division by zero wearing a
            // multiplication's clothes: the coefficient would scale to infinity
            // and the model would refuse every order for a reason that named
            // participation.
            return Err(Error::invalid(
                "the reference daily volatility must be finite and positive; the impact coefficient is quoted against it",
            ));
        }
        if !(0.0..=1.0).contains(&self.maximum_participation) {
            return Err(Error::invalid(
                "maximum participation must be a fraction of daily volume",
            ));
        }
        Ok(())
    }

    pub fn describe(&self) -> String {
        if self.is_frictionless() {
            return "frictionless: this measures a signal's raw content, not an achievable return"
                .to_string();
        }
        format!(
            "{:.1}bp commission, {:.1}bp half-spread, {:.1}bp tax, impact {:.1}bp at full participation and {:.2}% daily volatility on the square-root law (a calibrated assumption, not a measurement), {:.2}% borrow, refusing orders above {:.0}% of daily volume",
            self.commission_bps,
            self.half_spread_bps,
            self.tax_bps,
            self.impact_coefficient_bps,
            self.reference_daily_volatility * 100.0,
            self.short_borrow_bps_annual / 100.0,
            self.maximum_participation * 100.0
        )
    }
}

/// The cost of one order, decomposed.
///
/// In [`Decimal`], because this is money and the book is charged exactly what
/// is in here. It was `f64`, and [`crate::backtest::Backtester`] converted the
/// total back to a [`Decimal`] with an `unwrap_or(Decimal::ZERO)` behind it, so
/// a cost the money type could not represent was charged to the portfolio as
/// nothing at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TradeCost {
    pub commission: Decimal,
    pub spread: Decimal,
    pub impact: Decimal,
    /// Notional the costs were computed on.
    pub notional: Decimal,
}

impl TradeCost {
    /// What the book is charged.
    pub fn charged(&self) -> Decimal {
        self.commission + self.spread + self.impact
    }

    /// The same figure as a statistic.
    ///
    /// **This is the crossing point between money and `f64`**: a cost drag and
    /// a Sharpe ratio are not money, and everything downstream of here is
    /// arithmetic on returns. Anything that debits a book uses
    /// [`Self::charged`].
    pub fn total(&self) -> f64 {
        self.charged().to_f64()
    }

    /// Total cost in basis points of notional.
    pub fn total_bps(&self) -> f64 {
        // A `Decimal` is always finite, so this comparison has no NaN arm to
        // worry about; zero notional is the only degenerate case and it has no
        // basis points.
        let notional = self.notional.abs().to_f64();
        if notional <= 0.0 {
            return 0.0;
        }
        self.total() / notional * 10_000.0
    }
}

/// Why an order could not be filled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Unfillable {
    /// The order is larger than the model will price.
    ExceedsParticipation { requested: f64, limit: f64 },
    /// There was no volume to trade against.
    NoVolume,
    /// The price required to compute a cost was missing.
    NoPrice,
    /// Quantity times price is larger than the platform's money type can hold.
    NotRepresentable,
    /// The cost model itself could not price anything: a rate outside what
    /// `qip-financial` will hold on a record, or a charge the money type cannot
    /// represent.
    ///
    /// Distinct from [`Self::NotRepresentable`], which is about the *order*.
    /// This one is about the *model*, and the remedy is a different one: the
    /// order is fine and the parameters it was priced with are not. Carrying
    /// the refusal's own words rather than a code, because the field and the
    /// value are what the operator has to correct and only the model knows
    /// which they were.
    Unpriceable { reason: String },
}

impl Unfillable {
    pub fn describe(&self) -> String {
        match self {
            Self::ExceedsParticipation { requested, limit } => format!(
                "order is {:.1}% of daily volume against a {:.1}% limit; the impact model is not calibrated that far and would return a number rather than an answer",
                requested * 100.0,
                limit * 100.0
            ),
            Self::NoVolume => {
                "no volume traded in the fill window, or the volume reported was not a number"
                    .to_string()
            }
            Self::NoPrice => "no price available to compute a cost against".to_string(),
            Self::NotRepresentable => {
                "the order's notional is larger than the platform's money type can represent, so no cost can be stated for it"
                    .to_string()
            }
            Self::Unpriceable { reason } => format!(
                "the cost model cannot price this order: {reason}; correct the model's parameters — \
                 the order itself is fillable"
            ),
        }
    }
}

impl CostModel {
    /// Cost of trading `quantity` units at `price`.
    ///
    /// `daily_volume` sets the participation and `daily_volatility` scales the
    /// impact coefficient; the arithmetic is
    /// [`TransactionCostModel::estimate`]'s, term for term, so
    /// [`TradeCost::charged`] and the pre-trade estimate for the same order are
    /// the same number rather than two opinions about it.
    ///
    /// Returning an error rather than a large number when participation is
    /// excessive is deliberate: a backtest that quietly prices a 40%-of-volume
    /// order has told you nothing about whether the strategy is implementable.
    pub fn cost_of(
        &self,
        quantity: Decimal,
        price: Decimal,
        daily_volume: f64,
        daily_volatility: f64,
    ) -> std::result::Result<TradeCost, Unfillable> {
        if !price.is_positive() {
            return Err(Unfillable::NoPrice);
        }
        let magnitude = quantity.abs();
        if magnitude.is_zero() {
            return Ok(TradeCost::default());
        }
        // `is_finite` as well as positive: a NaN volume passes every `<= 0.0`
        // guard ever written, and the participation, the impact and the cost
        // computed from it are all NaN by the time anything notices.
        if !(daily_volume.is_finite() && daily_volume > 0.0) {
            return Err(Unfillable::NoVolume);
        }

        let participation = magnitude.to_f64() / daily_volume;
        if participation > self.maximum_participation {
            return Err(Unfillable::ExceedsParticipation {
                requested: participation,
                limit: self.maximum_participation,
            });
        }

        let Some(notional) = magnitude.checked_mul(price) else {
            return Err(Unfillable::NotRepresentable);
        };

        // The volatility-scaled model, checked. A backtest runs thousands of
        // instruments and one of them has a bad volatility bar; refusing that
        // instrument and naming it is the behaviour that lets the other
        // thousand finish, where `apply_bps`'s panic would end the run — and
        // in a release build, which is `panic = "abort"`, end the process.
        let pricing =
            self.pricing_at(daily_volatility)
                .map_err(|error| Unfillable::Unpriceable {
                    reason: error.to_string(),
                })?;
        // The three terms of `TransactionCostModel::estimate`, split so a
        // result can be decomposed into what the strategy earned and what the
        // market took. They sum to `estimate` exactly, and
        // `a_simulated_fill_is_charged_exactly_what_the_pre_trade_model_quotes`
        // is what keeps it that way.
        //
        // `checked_apply_bps` and not `apply_bps` even though `pricing` is
        // checked: the ceiling bounds the *rate*, and the product is a rate
        // against a notional this function did not choose. A `None` here is
        // the platform declining to charge a number it could not compute,
        // never the zero this function used to be able to return.
        let charge = |bps: f64, term: &str| {
            notional
                .checked_apply_bps(bps)
                .ok_or_else(|| Unfillable::Unpriceable {
                    reason: format!(
                        "{bps}bp of {term} on a notional of {notional} is not representable as \
                         money; reduce the order or correct the {term} rate"
                    ),
                })
        };
        let commission = charge(
            pricing.commission_bps + pricing.tax_bps,
            "commission and tax",
        )? + pricing.fixed_fee;
        let spread = charge(pricing.half_spread_bps, "half-spread")?;
        let impact = charge(pricing.impact_bps(participation), "impact")?;

        Ok(TradeCost {
            commission,
            spread,
            impact,
            notional,
        })
    }

    /// Commission and levies on a filled notional.
    ///
    /// The same two terms [`Self::cost_of`] charges and
    /// [`TransactionCostModel::estimate`] quotes. A notional too large for a
    /// [`Decimal`] never reaches here through [`Self::cost_of`], which refuses
    /// it as [`Unfillable::NotRepresentable`] before any fee is computed — but
    /// this is a `pub fn` and that is a fact about one caller, not about this
    /// one, so the rate goes through the same check the model does and the
    /// product is taken as data.
    pub fn commission_on(&self, notional: Decimal) -> Result<Decimal> {
        if !notional.is_positive() {
            return Ok(Decimal::ZERO);
        }
        let pricing = self.pricing()?;
        let rate = pricing.commission_bps + pricing.tax_bps;
        notional
            .checked_apply_bps(rate)
            .map(|fees| fees + pricing.fixed_fee)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "commission of {rate}bp on a notional of {notional} is not representable as \
                     money; charge it on a smaller fill or correct the commission and tax rates"
                ))
            })
    }

    /// Financing cost of holding a short for `days`, on an ACT/365 basis.
    pub fn borrow_cost(&self, notional: f64, days: f64) -> f64 {
        if notional >= 0.0 || days <= 0.0 {
            return 0.0;
        }
        notional.abs() * self.short_borrow_bps_annual / 10_000.0 * days / 365.0
    }
}
