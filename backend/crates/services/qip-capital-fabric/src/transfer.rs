//! What it costs to move capital, and what it costs not to have moved it.
//!
//! Five things are charged, and the fifth is the one systems leave out.
//!
//! 1. **FX conversion**, where the currencies differ. Priced through
//!    [`qip_financial::costs::TransactionCostModel`], so a large conversion pays
//!    the square-root impact of being a large conversion rather than the same
//!    basis points a small one pays.
//! 2. **The wire fee**, a fixed charge per instruction. Small, known exactly,
//!    and the reason a plan of many tiny transfers is worse than one large one.
//! 3. **The funding differential**, the carry given up by holding the balance in
//!    the destination currency instead of the source one.
//! 4. **The opportunity cost of capital in flight**, which earns nothing at
//!    either end for as long as [`crate::settlement`] says it is in transit.
//! 5. **The cost of not having it where it turned out to be needed** —
//!    [`ShortfallAsymmetry`].
//!
//! # Why the asymmetry is a type and not a parameter
//!
//! Being short capital in a region costs more than being long it. Being long
//! costs carry: a number, per day, that somebody can put in a spreadsheet.
//! Being short costs a trade not done, a margin call met by liquidating
//! something at whatever price is available, or a clearing house closing a
//! position out on your behalf. Those are not the same size and they are not the
//! same kind of number.
//!
//! A symmetric model does not merely mis-price this; it systematically
//! **under-positions**, because it sizes the buffer as though the two tails were
//! equally bad and the optimum of a symmetric loss sits at the median. So
//! [`ShortfallAsymmetry::new`] refuses a symmetric configuration outright. The
//! asymmetry cannot be switched off by passing equal numbers, because a
//! symmetric setting is not a conservative default here — it is a specific,
//! wrong opinion about which way the fabric should be wrong.

use crate::forecast::DemandKind;
use crate::location::CapitalLocation;
use crate::settlement::SettlementQuote;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Days in a year used to convert an annual rate to a period charge.
///
/// Actual/365 fixed. Chosen once, here, so no two cost figures in this crate can
/// disagree by a day-count convention.
const DAYS_PER_YEAR: f64 = 365.0;

/// How much worse it is to be short than to be long.
///
/// Both rates are annualised basis points on the gap. The constructor refuses a
/// configuration in which a shortfall is no worse than a surplus.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShortfallAsymmetry {
    shortfall_bps_annual: f64,
    surplus_bps_annual: f64,
}

impl ShortfallAsymmetry {
    /// Build an asymmetry, refusing a symmetric or inverted one.
    pub fn new(shortfall_bps_annual: f64, surplus_bps_annual: f64) -> Result<Self> {
        if !shortfall_bps_annual.is_finite() || !surplus_bps_annual.is_finite() {
            return Err(Error::invalid("penalty rates must be finite"));
        }
        if surplus_bps_annual < 0.0 {
            return Err(Error::invalid(
                "holding surplus capital cannot pay you; the surplus rate must be non-negative",
            ));
        }
        if shortfall_bps_annual <= surplus_bps_annual {
            return Err(Error::invalid(format!(
                "a shortfall rate of {shortfall_bps_annual:.1}bp does not exceed the \
                 {surplus_bps_annual:.1}bp surplus rate; a symmetric penalty sizes the buffer at \
                 the median and systematically under-positions"
            )));
        }
        Ok(Self {
            shortfall_bps_annual,
            surplus_bps_annual,
        })
    }

    /// The shipped asymmetry for a kind of demand.
    ///
    /// Contractual kinds — margin and collateral — are penalised an order of
    /// magnitude harder than tradeable ones, because a shortfall there is
    /// somebody else deciding what to sell. The surplus rate is the same for
    /// both: idle capital costs what idle capital costs, whatever it was set
    /// aside for.
    pub fn for_kind(kind: DemandKind) -> Result<Self> {
        if kind.is_contractual() {
            Self::new(4_000.0, 400.0)
        } else {
            Self::new(1_200.0, 400.0)
        }
    }

    /// Annualised basis points charged on a shortfall.
    pub fn shortfall_bps_annual(&self) -> f64 {
        self.shortfall_bps_annual
    }

    /// Annualised basis points charged on a surplus.
    pub fn surplus_bps_annual(&self) -> f64 {
        self.surplus_bps_annual
    }

    /// How many times worse a shortfall is than an equal surplus.
    ///
    /// Strictly greater than one by construction.
    pub fn multiple(&self) -> f64 {
        if self.surplus_bps_annual <= 0.0 {
            return f64::INFINITY;
        }
        self.shortfall_bps_annual / self.surplus_bps_annual
    }

    /// The newsvendor critical fractile implied by the two rates.
    ///
    /// `cu / (cu + co)` — the share of the demand distribution a cost-minimising
    /// buffer would cover if the distribution were trusted. Strictly above a
    /// half, which is the asymmetry showing up as a number rather than as a
    /// sentence. The planner uses it only to set the size of a buffer *above*
    /// demand it is already confident about; see [`crate::plan`] for why it is
    /// deliberately not used to reach up into the wide part of an interval.
    pub fn critical_fractile(&self) -> f64 {
        let total = self.shortfall_bps_annual + self.surplus_bps_annual;
        if total <= 0.0 {
            return 0.5;
        }
        (self.shortfall_bps_annual / total).clamp(0.5, 0.999)
    }

    /// What being short by `gap` for `over` costs.
    pub fn shortfall_penalty(&self, gap: Decimal, over: Duration) -> Decimal {
        Self::charge(gap, self.shortfall_bps_annual, over)
    }

    /// What being long by `gap` for `over` costs.
    pub fn surplus_penalty(&self, gap: Decimal, over: Duration) -> Decimal {
        Self::charge(gap, self.surplus_bps_annual, over)
    }

    /// The penalty on a signed gap, where negative is short.
    ///
    /// The single entry point where both branches are visible together, so the
    /// asymmetry is a property of one function rather than of two call sites
    /// that happen to be configured differently.
    pub fn penalty(&self, signed_gap: Decimal, over: Duration) -> Decimal {
        if signed_gap.is_negative() {
            self.shortfall_penalty(signed_gap.abs(), over)
        } else {
            self.surplus_penalty(signed_gap, over)
        }
    }

    fn charge(gap: Decimal, bps_annual: f64, over: Duration) -> Decimal {
        let days = over.as_days_f64().max(0.0);
        if days <= 0.0 || !gap.is_positive() {
            return Decimal::ZERO;
        }
        gap.apply_bps(bps_annual * days / DAYS_PER_YEAR)
    }
}

/// Annualised funding rates per currency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FundingCurve {
    rates_bps_annual: BTreeMap<Currency, f64>,
    default_bps_annual: f64,
}

impl FundingCurve {
    /// A curve that charges `default_bps_annual` for any currency not named.
    ///
    /// Failing to a stated default rather than to zero: an unpriced currency
    /// that costs nothing to fund would make every transfer into it look free,
    /// which is the wrong direction to be wrong in.
    pub fn flat(default_bps_annual: f64) -> Result<Self> {
        if !default_bps_annual.is_finite() {
            return Err(Error::invalid("a funding rate must be finite"));
        }
        Ok(Self {
            rates_bps_annual: BTreeMap::new(),
            default_bps_annual,
        })
    }

    /// Name a currency's rate.
    pub fn with_rate(mut self, currency: Currency, bps_annual: f64) -> Result<Self> {
        if !bps_annual.is_finite() {
            return Err(Error::invalid("a funding rate must be finite"));
        }
        self.rates_bps_annual.insert(currency, bps_annual);
        Ok(self)
    }

    /// The rate for a currency, or the default.
    pub fn rate_bps_annual(&self, currency: Currency) -> f64 {
        self.rates_bps_annual
            .get(&currency)
            .copied()
            .unwrap_or(self.default_bps_annual)
    }

    /// Source rate less destination rate: the carry given up by the move.
    pub fn differential_bps_annual(&self, from: Currency, to: Currency) -> f64 {
        self.rate_bps_annual(from) - self.rate_bps_annual(to)
    }
}

/// Exchange rates against one base currency.
///
/// The fabric budgets in a single base currency because the limits it composes
/// from [`qip_capital`] are stated in one. Every amount that touches a limit is
/// converted here, once, through a rate that is an input rather than a lookup —
/// so a plan can be replayed against the rates it was actually built on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FxRates {
    base: Currency,
    rates: BTreeMap<Currency, Decimal>,
}

impl FxRates {
    /// A table in which only the base currency is priced, at one.
    pub fn new(base: Currency) -> Self {
        let mut rates = BTreeMap::new();
        rates.insert(base, Decimal::ONE);
        Self { base, rates }
    }

    /// Price a currency in units of the base.
    pub fn with_rate(mut self, currency: Currency, base_per_unit: Decimal) -> Result<Self> {
        if !base_per_unit.is_positive() {
            return Err(Error::invalid(format!(
                "the rate for {currency} must be positive"
            )));
        }
        self.rates.insert(currency, base_per_unit);
        Ok(self)
    }

    /// The base currency.
    pub fn base(&self) -> Currency {
        self.base
    }

    /// Convert an amount into the base currency.
    ///
    /// Refuses an unpriced currency rather than assuming parity. A missing rate
    /// silently treated as one is how a yen balance becomes a hundred and fifty
    /// times its real contribution to a dollar limit.
    pub fn to_base(&self, amount: Decimal, currency: Currency) -> Result<Decimal> {
        let rate = self.rates.get(&currency).copied().ok_or_else(|| {
            Error::not_found(format!(
                "no rate for {currency} against the {} base; an unpriced currency is refused \
                 rather than assumed to be at parity",
                self.base
            ))
        })?;
        amount
            .checked_mul(rate)
            .ok_or_else(|| Error::numeric(format!("converting {amount} {currency} overflowed")))
    }

    /// Convert between two priced currencies.
    pub fn convert(&self, amount: Decimal, from: Currency, to: Currency) -> Result<Decimal> {
        if from == to {
            return Ok(amount);
        }
        let in_base = self.to_base(amount, from)?;
        let target_rate = self.rates.get(&to).copied().ok_or_else(|| {
            Error::not_found(format!("no rate for {to} against the {} base", self.base))
        })?;
        in_base.checked_div(target_rate).ok_or_else(|| {
            Error::numeric(format!("converting {amount} {from} into {to} overflowed"))
        })
    }
}

/// The itemised cost of one transfer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransferCost {
    /// The amount moved, in the destination currency.
    pub amount: Decimal,
    /// Spread, fees and market impact on the FX leg. Zero within a currency.
    pub fx_conversion: Decimal,
    /// The fixed charge for the instruction.
    pub wire_fee: Decimal,
    /// Carry given up by holding the balance in the destination currency.
    ///
    /// Signed: negative where the destination currency is the better-paid one,
    /// and the move earns rather than costs. Kept signed rather than floored at
    /// zero, because a transfer that pays for part of itself is a real thing and
    /// hiding it would make the fabric refuse transfers it should make.
    pub funding_differential: Decimal,
    /// What the capital forgoes while it is in transit and usable by nobody.
    pub in_flight_opportunity: Decimal,
    /// The algebraic sum of the four.
    pub total: Decimal,
    /// The figure the planner is required to compare against.
    ///
    /// The uncertain components widened by the model's stated cost uncertainty;
    /// the wire fee is known exactly and is not inflated. A plan justified on a
    /// central cost estimate and a central benefit estimate is a plan that is
    /// right on average and loses money in the cases that matter.
    pub upper: Decimal,
    /// Calendar days the capital spends in transit. A statistic, hence `f64`.
    pub days_in_flight_stat: f64,
    /// Share of a day's FX volume the conversion would be. A statistic.
    pub fx_participation_stat: f64,
}

impl TransferCost {
    /// A one-line summary for an approval log.
    pub fn describe(&self) -> String {
        format!(
            "moving {} costs {} ({} fx at {:.2}% of daily volume, {} wire, {} funding, {} in \
             flight over {:.2} day(s)); charged at its {} upper bound",
            self.amount,
            self.total,
            self.fx_conversion,
            self.fx_participation_stat * 100.0,
            self.wire_fee,
            self.funding_differential,
            self.in_flight_opportunity,
            self.days_in_flight_stat,
            self.upper,
        )
    }
}

/// Prices transfers between locations.
#[derive(Clone, Debug, PartialEq)]
pub struct TransferCostModel {
    fx: TransactionCostModel,
    fx_liquidity: LiquidityProfile,
    funding: FundingCurve,
    wire_fee: Decimal,
    opportunity_bps_annual: f64,
    cost_uncertainty: f64,
}

impl TransferCostModel {
    /// How much the uncertain cost components are widened by default.
    ///
    /// A quarter. FX is quoted, so the conversion estimate is decent; funding
    /// and the opportunity cost of capital in flight are both forecasts of
    /// rates over a period, and a quarter is roughly what those move by inside
    /// a horizon this crate plans over.
    pub const DEFAULT_COST_UNCERTAINTY: f64 = 0.25;

    /// Build a cost model.
    ///
    /// `fx_liquidity` describes the currency market a conversion goes through,
    /// with its average daily volume expressed in the currency being bought. It
    /// is what turns a transfer's size into a participation rate and therefore
    /// into square-root impact, so a transfer that is a meaningful share of a
    /// thin cross pays for being one.
    pub fn new(
        fx: TransactionCostModel,
        fx_liquidity: LiquidityProfile,
        funding: FundingCurve,
        wire_fee: Decimal,
        opportunity_bps_annual: f64,
    ) -> Result<Self> {
        if wire_fee.is_negative() {
            return Err(Error::invalid("a wire fee cannot be negative"));
        }
        if !opportunity_bps_annual.is_finite() || opportunity_bps_annual < 0.0 {
            return Err(Error::invalid(
                "the opportunity cost of capital in flight must be a non-negative rate",
            ));
        }
        Ok(Self {
            fx,
            fx_liquidity,
            funding,
            wire_fee,
            opportunity_bps_annual,
            cost_uncertainty: Self::DEFAULT_COST_UNCERTAINTY,
        })
    }

    /// Change how far the uncertain components are widened.
    pub fn with_cost_uncertainty(mut self, uncertainty: f64) -> Result<Self> {
        if !uncertainty.is_finite() || uncertainty < 0.0 {
            return Err(Error::invalid(
                "cost uncertainty must be a non-negative fraction",
            ));
        }
        self.cost_uncertainty = uncertainty;
        Ok(self)
    }

    /// The funding curve in force.
    pub fn funding(&self) -> &FundingCurve {
        &self.funding
    }

    /// The uncertainty applied to the widened components.
    pub fn cost_uncertainty(&self) -> f64 {
        self.cost_uncertainty
    }

    /// Price moving `amount` of the destination currency from one location to
    /// another.
    ///
    /// `holding` is how long the capital sits at the destination after it
    /// arrives and before the demand it was sent for materialises. The funding
    /// differential is charged over the flight *and* the wait, because the
    /// balance is in the destination currency for both.
    pub fn price(
        &self,
        amount: Decimal,
        from: &CapitalLocation,
        to: &CapitalLocation,
        quote: &SettlementQuote,
        holding: Duration,
    ) -> Result<TransferCost> {
        if amount.is_negative() {
            return Err(Error::invalid(
                "a transfer amount cannot be negative; capital is recalled through \
                 qip_capital::RecallOrder, not by pricing a backwards transfer",
            ));
        }

        let (fx_conversion, participation_stat) = if from.requires_conversion(to) {
            let adv = self.fx_liquidity.average_daily_volume.to_f64();
            let participation = if adv > 0.0 {
                amount.to_f64() / adv
            } else {
                // No stated FX volume means no basis for an impact estimate.
                // Treating that as "no impact" would make the least liquid
                // crosses look like the cheapest ones.
                f64::INFINITY
            };
            if !participation.is_finite() {
                return Err(Error::unavailable(format!(
                    "no daily FX volume for {} into {}, so the conversion cannot be priced",
                    from.currency, to.currency
                )));
            }
            (self.fx.estimate(amount, participation), participation)
        } else {
            (Decimal::ZERO, 0.0)
        };

        let flight_days = quote.days_in_flight_stat.max(0.0);
        let exposed_days = flight_days + holding.as_days_f64().max(0.0);

        let differential_bps = self
            .funding
            .differential_bps_annual(from.currency, to.currency)
            * exposed_days
            / DAYS_PER_YEAR;
        let funding_differential = amount.apply_bps(differential_bps);
        let in_flight_opportunity =
            amount.apply_bps(self.opportunity_bps_annual * flight_days / DAYS_PER_YEAR);

        let total = fx_conversion + self.wire_fee + funding_differential + in_flight_opportunity;
        let uncertain = fx_conversion + funding_differential.abs() + in_flight_opportunity;
        let widening = Decimal::from_f64(self.cost_uncertainty)
            .and_then(|factor| uncertain.checked_mul(factor))
            .ok_or_else(|| Error::numeric("the transfer cost widening overflowed"))?;

        Ok(TransferCost {
            amount,
            fx_conversion,
            wire_fee: self.wire_fee,
            funding_differential,
            in_flight_opportunity,
            total,
            upper: total + widening,
            days_in_flight_stat: flight_days,
            fx_participation_stat: participation_stat,
        })
    }
}
