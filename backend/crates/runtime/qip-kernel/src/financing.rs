//! The valuation plane's financing half: FX forwards, repos and the spot
//! rates a balance is converted at.
//!
//! # What this closes
//!
//! [`covered_interest_forward`] priced an FX forward and had no production
//! caller: nothing supplied it a tenor or the two currencies' rates, so the
//! `ForeignExchange` record's claim that a forward is "spot carried along the
//! two curves" was true of a function and of no instrument. A repo was an
//! instrument nothing valued. And the kernel converted balances through an
//! [`FxRates`] table that priced the dollar and nothing else, so a balance in
//! any other currency was refused however many spot pairs the universe held.
//!
//! This register is the caller. It is derived at assembly from the universe
//! and the [`CreditRegister`]'s sovereign curves, and the UNDERSTAND stage
//! reports it — including, as stage problems, every forward and repo it could
//! not value and why. A register that valued what it could and stayed silent
//! about the rest would read as coverage of the whole class.
//!
//! # Where every figure comes from
//!
//! Nothing here is a constant of this module. The spot is the price of the
//! `FxSpot` instrument on the same pair; the tenor is the forward's own
//! `settlement_date`; each currency's rate is read off that currency's
//! sovereign curve at that tenor; a repo's rate, haircut, maturity and
//! collateral are its own [`RepoDetails`], and the collateral's value is the
//! collateral instrument's price. A figure the universe does not carry is a
//! refusal naming the missing record, never a default.
//!
//! # Money and statistics
//!
//! A curve's zero rate is a statistic and is `f64`. A forward outright, a
//! repo's cash and its interest are money and are [`Decimal`]. The crossing
//! points are [`simple_rate`] and [`fraction`], commented at each.

use std::collections::BTreeMap;

use qip_capital_fabric::FxRates;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, ObjectId, Timestamp};
use qip_financial::asset_class::InstrumentType;
use qip_financial::extensions::{Constituent, Extension, FxDetails, PerpetualDetails, RepoDetails};
use qip_financial::object::FinancialObject;
use qip_financial::universe::Universe;

use crate::asset_class_registry::covered_interest_forward;
use crate::valuation::CreditRegister;

/// Quarter-days in a year on the 365.25-day convention the platform's tenors
/// use (`valuation.rs` builds every sovereign curve on it).
///
/// A rate read off a curve built on 365.25 days must accrue over a year of
/// 365.25 days, or the forward disagrees with the curve it was priced from.
/// Held in quarter-days because [`covered_interest_forward`] takes its day
/// count and basis as whole numbers and 365.25 is not one.
const QUARTER_DAYS_PER_YEAR: u32 = 1461;

const MILLIS_PER_DAY: i64 = 86_400_000;

/// One FX forward, valued by covered interest parity from the universe's own
/// records.
#[derive(Clone, Debug, PartialEq)]
pub struct FxForwardValuation {
    pub base: Currency,
    pub quote: Currency,
    /// The `FxSpot` instrument the spot was read from.
    pub spot_object: String,
    pub spot: Decimal,
    /// Whole days from the assembly instant to the forward's settlement.
    pub days: u32,
    /// Simple rate of the quote (domestic) currency over the tenor.
    pub domestic_rate: Decimal,
    /// Simple rate of the base (foreign) currency over the tenor.
    pub foreign_rate: Decimal,
    /// Units of quote per unit of base for settlement at the tenor.
    pub outright: Decimal,
}

/// One repo, valued per unit of its collateral.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoValuation {
    pub collateral_object: String,
    pub counterparty: String,
    /// True when the platform lends cash and takes the collateral.
    pub is_reverse: bool,
    /// The collateral instrument's price, in `currency`.
    pub collateral_value: Decimal,
    pub currency: Currency,
    /// Cash exchanged on the near leg: collateral value less the haircut.
    pub cash: Decimal,
    /// Whole days from the assembly instant to the repo's maturity.
    pub days: u32,
    /// Interest on `cash` at the repo rate to maturity.
    pub interest: Decimal,
    /// Cash exchanged back on the far leg: `cash + interest`.
    pub repurchase: Decimal,
}

/// One perpetual's funding, per contract and per funding interval (ADR 0395).
///
/// The rate is the premium of the perpetual's own price over its
/// underlying's price, both read from the universe. No venue-published
/// funding rate is read because no source in the tree supplies one, and one
/// is not assumed. This is a model's figure: it is reported by the cycle and
/// is not booked to the ledger, for the reason ADR 0153 declines a modelled
/// margin there.
#[derive(Clone, Debug, PartialEq)]
pub struct PerpetualFunding {
    pub underlying_object: String,
    /// The perpetual's price, in `currency`.
    pub mark: Decimal,
    /// The underlying's price, in `currency`.
    pub index: Decimal,
    pub currency: Currency,
    /// Hours between funding exchanges, from the perpetual's own record.
    pub interval_hours: u32,
    /// `(mark − index) / index` for one interval. Positive: the long pays.
    pub rate: Decimal,
    /// `rate × index × contract_size`: what one long contract pays one short
    /// contract at the next exchange. Negative: the long receives.
    pub payment_per_contract: Decimal,
}

/// One index or basket, valued from the constituents its own record states
/// (ADR 0397).
///
/// Reported by the cycle and read by nothing that sizes or decides: the
/// composite's own quote stays its mark. What this adds is the figure the
/// quote can be checked against, and the exposure behind one unit.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeValuation {
    /// Constituent id to the value it contributes to one unit of the
    /// composite: `units × price`, before any divisor. The look-through
    /// exposure of one unit.
    pub exposure: BTreeMap<String, Decimal>,
    /// The sum of `exposure`, divided by the index divisor where the record
    /// states one.
    pub value: Decimal,
    /// The composite's own price in the universe.
    pub quoted: Decimal,
    pub currency: Currency,
}

/// The financing valuations of one universe, as it stood at assembly.
///
/// `BTreeMap` throughout: the summary and the problems are decided by
/// iterating these, and a register that reordered would tell a replay a
/// different sentence from the same facts.
#[derive(Clone, Debug, Default)]
pub struct FinancingRegister {
    /// `(base, quote)` to the spot instrument's id and price.
    spots: BTreeMap<(Currency, Currency), (String, Decimal)>,
    forwards: BTreeMap<String, FxForwardValuation>,
    repos: BTreeMap<String, RepoValuation>,
    perpetuals: BTreeMap<String, PerpetualFunding>,
    composites: BTreeMap<String, CompositeValuation>,
    /// Object id to why it could not be valued. Held rather than dropped: an
    /// unvalued forward that vanished would read as a universe with none.
    refusals: BTreeMap<String, String>,
}

impl FinancingRegister {
    /// Derive the register from the universe as it stood at `as_of`, reading
    /// currency rates off `credit`'s sovereign curves.
    ///
    /// Takes no clock, for the reason [`CreditRegister::from_universe`] gives:
    /// a replay builds the same register from the same universe.
    pub fn from_universe(universe: &Universe, credit: &CreditRegister, as_of: Timestamp) -> Self {
        let mut register = Self::default();
        for object in universe.iter() {
            if object.instrument_type != InstrumentType::FxSpot {
                continue;
            }
            // A spot pair names its two currencies in its own record. One
            // that does not is not guessed from its symbol.
            if let Extension::Fx(fx) = &object.extension
                && object.price.is_positive()
            {
                register
                    .spots
                    .entry((fx.base_currency, fx.quote_currency))
                    .or_insert_with(|| (object.object_id.as_str().to_string(), object.price));
            }
        }
        for object in universe.iter() {
            let id = object.object_id.as_str().to_string();
            match (&object.instrument_type, &object.extension) {
                (InstrumentType::FxForward, Extension::Fx(fx)) => {
                    match register.value_forward(fx, credit, as_of) {
                        Ok(valuation) => {
                            register.forwards.insert(id, valuation);
                        }
                        Err(error) => {
                            register.refusals.insert(id, error.to_string());
                        }
                    }
                }
                (InstrumentType::Repo, Extension::Repo(repo)) => {
                    match value_repo(repo, universe, as_of) {
                        Ok(valuation) => {
                            register.repos.insert(id, valuation);
                        }
                        Err(error) => {
                            register.refusals.insert(id, error.to_string());
                        }
                    }
                }
                (InstrumentType::PerpetualFuture, Extension::Perpetual(perpetual)) => {
                    match fund_perpetual(object, perpetual, universe) {
                        Ok(funding) => {
                            register.perpetuals.insert(id, funding);
                        }
                        Err(error) => {
                            register.refusals.insert(id, error.to_string());
                        }
                    }
                }
                (InstrumentType::Index, Extension::Index(index)) => {
                    match value_composite(
                        object,
                        &index.constituents,
                        Some(index.divisor),
                        universe,
                    ) {
                        Ok(valuation) => {
                            register.composites.insert(id, valuation);
                        }
                        Err(error) => {
                            register.refusals.insert(id, error.to_string());
                        }
                    }
                }
                (InstrumentType::SyntheticBasket, Extension::Structured(basket)) => {
                    match value_composite(object, &basket.constituents, None, universe) {
                        Ok(valuation) => {
                            register.composites.insert(id, valuation);
                        }
                        Err(error) => {
                            register.refusals.insert(id, error.to_string());
                        }
                    }
                }
                // A perpetual whose record carries no perpetual terms has no
                // funding interval to read. Held as a refusal: dropped, it
                // would read as a universe with no perpetual in it.
                (InstrumentType::PerpetualFuture, other) => {
                    register.refusals.insert(
                        id,
                        format!(
                            "a perpetual future carries a {} extension and no perpetual terms; \
                             state its PerpetualDetails, a funding interval is not assumed",
                            other.label()
                        ),
                    );
                }
                _ => {}
            }
        }
        register
    }

    fn value_forward(
        &self,
        fx: &FxDetails,
        credit: &CreditRegister,
        as_of: Timestamp,
    ) -> Result<FxForwardValuation> {
        let settlement = fx.settlement_date.ok_or_else(|| {
            Error::invalid(
                "the forward states no settlement date, so it has no tenor to carry spot over; \
                 state `settlement_date` on its FX details",
            )
        })?;
        let days = whole_days(as_of, settlement).ok_or_else(|| {
            Error::invalid(
                "the forward settles less than a whole day after the valuation instant, which is \
                 a spot and not a forward; value it from the spot pair",
            )
        })?;
        let (spot_object, spot) = self
            .spots
            .get(&(fx.base_currency, fx.quote_currency))
            .ok_or_else(|| {
                Error::not_found(format!(
                    "no FxSpot instrument on {}/{} is in the universe, so there is no spot to \
                     carry; add the spot pair, a forward is not priced from an assumed spot",
                    fx.base_currency, fx.quote_currency
                ))
            })?;
        let domestic_rate = simple_rate(credit, fx.quote_currency, days)?;
        let foreign_rate = simple_rate(credit, fx.base_currency, days)?;
        let outright = covered_interest_forward(
            *spot,
            domestic_rate,
            foreign_rate,
            quarter_days(days)?,
            QUARTER_DAYS_PER_YEAR,
        )?;
        Ok(FxForwardValuation {
            base: fx.base_currency,
            quote: fx.quote_currency,
            spot_object: spot_object.clone(),
            spot: *spot,
            days,
            domestic_rate,
            foreign_rate,
            outright,
        })
    }

    /// The forward valued under `object_id`, if it could be.
    pub fn forward(&self, object_id: &str) -> Option<&FxForwardValuation> {
        self.forwards.get(object_id)
    }

    /// The repo valued under `object_id`, if it could be.
    pub fn repo(&self, object_id: &str) -> Option<&RepoValuation> {
        self.repos.get(object_id)
    }

    /// The funding valued for the perpetual under `object_id`, if it could be.
    pub fn perpetual(&self, object_id: &str) -> Option<&PerpetualFunding> {
        self.perpetuals.get(object_id)
    }

    pub fn perpetuals(&self) -> impl Iterator<Item = (&String, &PerpetualFunding)> {
        self.perpetuals.iter()
    }

    /// The index or basket valued under `object_id`, if it could be.
    pub fn composite(&self, object_id: &str) -> Option<&CompositeValuation> {
        self.composites.get(object_id)
    }

    pub fn composites(&self) -> impl Iterator<Item = (&String, &CompositeValuation)> {
        self.composites.iter()
    }

    pub fn forwards(&self) -> impl Iterator<Item = (&String, &FxForwardValuation)> {
        self.forwards.iter()
    }

    pub fn repos(&self) -> impl Iterator<Item = (&String, &RepoValuation)> {
        self.repos.iter()
    }

    /// Object id to why it could not be valued.
    pub fn refusals(&self) -> impl Iterator<Item = (&String, &String)> {
        self.refusals.iter()
    }

    /// The universe's spot pairs as a conversion table against `base`.
    ///
    /// Only pairs quoted directly against `base` are priced: a pair with
    /// `base` as its quote currency prices its base currency at the spot, and
    /// a pair with `base` as its base currency prices its quote currency at
    /// the reciprocal. A cross is not triangulated here, because a rate built
    /// through a third currency is a figure no instrument in the universe
    /// quotes; a currency with no direct pair stays unpriced and
    /// [`FxRates::to_base`] refuses it, as it did before this existed.
    pub fn fx_rates(&self, base: Currency) -> Result<FxRates> {
        let mut rates = FxRates::new(base);
        for ((pair_base, pair_quote), (object, spot)) in &self.spots {
            if *pair_quote == base && *pair_base != base {
                rates = rates.with_rate(*pair_base, *spot)?;
            } else if *pair_base == base && *pair_quote != base {
                let reciprocal = Decimal::ONE.checked_div(*spot).ok_or_else(|| {
                    Error::numeric(format!(
                        "the reciprocal of spot {spot} on {object} overflowed"
                    ))
                })?;
                rates = rates.with_rate(*pair_quote, reciprocal)?;
            }
        }
        Ok(rates)
    }

    /// One clause for the UNDERSTAND stage's detail; empty when the universe
    /// holds no forward and no repo.
    pub fn summary(&self) -> String {
        if self.forwards.is_empty()
            && self.repos.is_empty()
            && self.perpetuals.is_empty()
            && self.composites.is_empty()
            && self.refusals.is_empty()
        {
            return String::new();
        }
        format!(
            "; financing register values {} FX forward(s) by covered interest parity and {} \
             repo(s) and funds {} perpetual(s) from the premium over the underlying and values {} \
             index(es) and basket(s) from their constituents, {} unvalued",
            self.forwards.len(),
            self.repos.len(),
            self.perpetuals.len(),
            self.composites.len(),
            self.refusals.len()
        )
    }

    /// Every forward and repo that could not be valued, as a stage problem.
    pub fn problems(&self) -> Vec<String> {
        self.refusals
            .iter()
            .map(|(id, reason)| format!("financing instrument {id} is unvalued: {reason}"))
            .collect()
    }
}

/// Value one index or basket from the constituents its record states.
///
/// Refused rather than guessed: a record stating no constituents, a
/// constituent named twice, units that are not positive, a constituent that
/// is absent, unpriced or priced in another currency, and an index divisor
/// that is not positive.
fn value_composite(
    object: &FinancialObject,
    constituents: &[Constituent],
    divisor: Option<Decimal>,
    universe: &Universe,
) -> Result<CompositeValuation> {
    if constituents.is_empty() {
        return Err(Error::invalid(
            "the record states no constituents, so there is nothing to value it from; state \
             each constituent and its units, a composition is not assumed",
        ));
    }
    let overflow = || Error::numeric("the composite's value arithmetic overflowed");
    let mut exposure = BTreeMap::new();
    let mut total = Decimal::ZERO;
    for constituent in constituents {
        if !constituent.units.is_positive() {
            return Err(Error::invalid(format!(
                "constituent {} is held at {} units; state a positive holding or drop it",
                constituent.object_id, constituent.units
            )));
        }
        let member = universe
            .get(&ObjectId::from_string(constituent.object_id.as_str()))
            .ok_or_else(|| {
                Error::not_found(format!(
                    "constituent {} is not in the universe, so its price is unknown; add it, a \
                     composite is not valued around a missing member",
                    constituent.object_id
                ))
            })?;
        if !member.price.is_positive() {
            return Err(Error::invalid(format!(
                "constituent {} is priced at {}, which values nothing",
                constituent.object_id, member.price
            )));
        }
        if member.currency != object.currency {
            return Err(Error::invalid(format!(
                "constituent {} is priced in {} and the composite in {}; a sum across two \
                 currencies is not a value",
                constituent.object_id, member.currency, object.currency
            )));
        }
        let contributed = constituent
            .units
            .checked_mul(member.price)
            .ok_or_else(overflow)?;
        if exposure
            .insert(constituent.object_id.clone(), contributed)
            .is_some()
        {
            return Err(Error::invalid(format!(
                "constituent {} is stated twice; state its whole holding once",
                constituent.object_id
            )));
        }
        total = total.checked_add(contributed).ok_or_else(overflow)?;
    }
    let value = match divisor {
        Some(divisor) if !divisor.is_positive() => {
            return Err(Error::invalid(format!(
                "the index divisor is {divisor}; a level is not divided by a non-positive figure"
            )));
        }
        Some(divisor) => total.checked_div(divisor).ok_or_else(overflow)?,
        None => total,
    };
    Ok(CompositeValuation {
        exposure,
        value,
        quoted: object.price,
        currency: object.currency,
    })
}

/// Fund one perpetual from its own terms and the universe's two prices.
///
/// Refused rather than defaulted: a funding interval of zero, a contract
/// size that is not positive, an underlying that is absent, either price
/// not positive, and an underlying priced in another currency (a premium
/// across two currencies is an FX move, not a funding rate).
fn fund_perpetual(
    object: &FinancialObject,
    perpetual: &PerpetualDetails,
    universe: &Universe,
) -> Result<PerpetualFunding> {
    if perpetual.funding_interval_hours == 0 {
        return Err(Error::invalid(
            "the funding interval is zero hours, so funding is never exchanged; state the \
             contract's interval, a contract that never funds is not a perpetual",
        ));
    }
    if !perpetual.contract_size.is_positive() {
        return Err(Error::invalid(format!(
            "a contract size of {} funds nothing; state the units of underlying per contract",
            perpetual.contract_size
        )));
    }
    let underlying = universe
        .get(&ObjectId::from_string(
            perpetual.underlying_object_id.as_str(),
        ))
        .ok_or_else(|| {
            Error::not_found(format!(
                "underlying {} is not in the universe, so there is no price to take a premium \
                 over; add the underlying, funding is not valued against an assumed price",
                perpetual.underlying_object_id
            ))
        })?;
    if !underlying.price.is_positive() {
        return Err(Error::invalid(format!(
            "underlying {} is priced at {}, and a premium over that is not a rate",
            perpetual.underlying_object_id, underlying.price
        )));
    }
    if !object.price.is_positive() {
        return Err(Error::invalid(format!(
            "the perpetual is priced at {}, so it has no mark to take a premium from",
            object.price
        )));
    }
    if underlying.currency != object.currency {
        return Err(Error::invalid(format!(
            "the perpetual is priced in {} and underlying {} in {}; a premium across two \
             currencies is an FX move and is not read as funding",
            object.currency, perpetual.underlying_object_id, underlying.currency
        )));
    }
    let overflow = || Error::numeric("the perpetual's funding arithmetic overflowed");
    let rate = object
        .price
        .checked_sub(underlying.price)
        .and_then(|premium| premium.checked_div(underlying.price))
        .ok_or_else(overflow)?;
    let payment_per_contract = rate
        .checked_mul(underlying.price)
        .and_then(|per_unit| per_unit.checked_mul(perpetual.contract_size))
        .ok_or_else(overflow)?;
    Ok(PerpetualFunding {
        underlying_object: perpetual.underlying_object_id.clone(),
        mark: object.price,
        index: underlying.price,
        currency: object.currency,
        interval_hours: perpetual.funding_interval_hours,
        rate,
        payment_per_contract,
    })
}

/// Whole days from `from` to `to`; `None` when there is not one.
fn whole_days(from: Timestamp, to: Timestamp) -> Option<u32> {
    if to <= from {
        return None;
    }
    let days = to.since(from).as_millis() / MILLIS_PER_DAY;
    u32::try_from(days).ok().filter(|days| *days > 0)
}

fn quarter_days(days: u32) -> Result<u32> {
    days.checked_mul(4)
        .ok_or_else(|| Error::numeric(format!("a tenor of {days} days overflowed")))
}

/// A statistic entering money arithmetic, refused when it is not a number.
///
/// The crossing point for a haircut, a repo rate and a curve rate: each is an
/// `f64` in its record and is applied to a [`Decimal`] amount here.
fn fraction(value: f64, what: &str) -> Result<Decimal> {
    if !value.is_finite() {
        return Err(Error::invalid(format!(
            "the {what} is {value}, which is not a number; state it as a fraction"
        )));
    }
    Decimal::from_f64(value)
        .ok_or_else(|| Error::numeric(format!("the {what} of {value} does not fit a decimal")))
}

/// The simple rate equal to `currency`'s sovereign zero rate over `days`.
///
/// The curve's rate is continuously compounded
/// ([`qip_market::curve::TermStructure::discount_factor`]) and
/// [`covered_interest_forward`] takes simple money-market rates, so the rate
/// is restated as the simple rate that grows a deposit by the same factor:
/// `(1 / discount_factor − 1) / t`. Passing the zero rate through unchanged
/// would misprice every forward by the difference between the two
/// compoundings. This is the `f64`-to-`Decimal` crossing for a curve rate.
fn simple_rate(credit: &CreditRegister, currency: Currency, days: u32) -> Result<Decimal> {
    let curve = credit.curve(currency).ok_or_else(|| {
        Error::not_found(format!(
            "no {currency} sovereign curve is in the universe, so there is no {currency} rate at \
             the forward's tenor; add {currency} sovereign benchmarks, a rate is not assumed"
        ))
    })?;
    let years = f64::from(days) * 4.0 / f64::from(QUARTER_DAYS_PER_YEAR);
    let discount = curve.discount_factor(years);
    if !(discount.is_finite() && discount > 0.0) {
        return Err(Error::numeric(format!(
            "the {currency} curve's discount factor at {days} days is {discount}"
        )));
    }
    fraction((1.0 / discount - 1.0) / years, "simple rate off the curve")
}

/// Interest on `principal` lent at `repo_rate` from `opened` to `maturity`
/// (ADR 0398): `principal × repo_rate × days / 365.25`, the convention
/// [`value_repo`] accrues on, over the whole days the cash was lent. Zero
/// when less than a whole day separates the two. The far leg of a repo is
/// repaid on this; it is money, so the rate crosses from `f64` here and is
/// refused when it is not a number.
pub(crate) fn repo_interest(
    principal: Decimal,
    repo_rate: f64,
    opened: Timestamp,
    maturity: Timestamp,
) -> Result<Decimal> {
    let Some(days) = whole_days(opened, maturity) else {
        return Ok(Decimal::ZERO);
    };
    let overflow = || Error::numeric("the repo's interest arithmetic overflowed");
    let year_fraction = Decimal::from_int(i64::from(quarter_days(days)?))
        .checked_div(Decimal::from_int(i64::from(QUARTER_DAYS_PER_YEAR)))
        .ok_or_else(overflow)?;
    principal
        .checked_mul(fraction(repo_rate, "repo rate")?)
        .and_then(|annual| annual.checked_mul(year_fraction))
        .ok_or_else(overflow)
}

/// Value one repo per unit of its collateral, from its own terms.
///
/// `cash = collateral price × (1 − haircut)`, and
/// `interest = cash × repo_rate × days / 365.25` on the platform's tenor
/// convention — [`RepoDetails`] states no day count of its own, and the
/// convention used is the one named beside [`QUARTER_DAYS_PER_YEAR`] rather
/// than a market convention this record was never shown.
///
/// Refused rather than clamped: a haircut outside `[0, 1)` (at one the repo
/// lends nothing), a rate that is not a number, a maturity not after the
/// valuation instant, and collateral that is not in the universe or has no
/// positive price.
fn value_repo(repo: &RepoDetails, universe: &Universe, as_of: Timestamp) -> Result<RepoValuation> {
    if !(repo.haircut.is_finite() && (0.0..1.0).contains(&repo.haircut)) {
        return Err(Error::invalid(format!(
            "a haircut of {} is outside [0, 1); state it as the fraction of collateral value \
             withheld",
            repo.haircut
        )));
    }
    let days = whole_days(as_of, repo.maturity).ok_or_else(|| {
        Error::invalid(
            "the repo matures less than a whole day after the valuation instant, so there is no \
             term to accrue over; a matured repo is settled, not valued",
        )
    })?;
    let collateral: &FinancialObject = universe
        .get(&ObjectId::from_string(repo.collateral_object.as_str()))
        .ok_or_else(|| {
            Error::not_found(format!(
                "collateral {} is not in the universe, so its value is unknown; add the \
                 collateral instrument, a repo is not valued against an assumed price",
                repo.collateral_object
            ))
        })?;
    if !collateral.price.is_positive() {
        return Err(Error::invalid(format!(
            "collateral {} is priced at {}, which secures nothing",
            repo.collateral_object, collateral.price
        )));
    }
    let overflow = || Error::numeric("the repo's cash arithmetic overflowed");
    let lent_fraction = Decimal::ONE
        .checked_sub(fraction(repo.haircut, "haircut")?)
        .ok_or_else(overflow)?;
    let cash = collateral
        .price
        .checked_mul(lent_fraction)
        .ok_or_else(overflow)?;
    let year_fraction = Decimal::from_int(i64::from(quarter_days(days)?))
        .checked_div(Decimal::from_int(i64::from(QUARTER_DAYS_PER_YEAR)))
        .ok_or_else(overflow)?;
    let interest = cash
        .checked_mul(fraction(repo.repo_rate, "repo rate")?)
        .and_then(|annual| annual.checked_mul(year_fraction))
        .ok_or_else(overflow)?;
    let repurchase = cash.checked_add(interest).ok_or_else(overflow)?;
    Ok(RepoValuation {
        collateral_object: repo.collateral_object.clone(),
        counterparty: repo.counterparty.clone(),
        is_reverse: repo.is_reverse,
        collateral_value: collateral.price,
        currency: collateral.currency,
        cash,
        days,
        interest,
        repurchase,
    })
}
