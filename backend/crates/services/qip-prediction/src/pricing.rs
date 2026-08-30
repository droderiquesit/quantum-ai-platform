//! Implied probability, with the fee and the spread backed out.
//!
//! "The contract trades at 0.62, so the market thinks it is 62% likely" is
//! wrong in a way that matters. A buyer at 0.62 pays 0.62 plus a taker fee and
//! collects the payoff less a settlement fee, so the probability at which that
//! purchase breaks even is strictly higher than 0.62. The seller's break-even
//! is strictly lower. What the price tells you is a band, and the width of the
//! band is exactly the cost of having an opinion.
//!
//! Everything inside the band is unreachable: a forecast more accurate than
//! the market's, but by less than the band is wide, produces no profitable
//! trade. That is the number a strategy needs, and it is not the price.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_market::book::OrderBook;
use serde::{Deserialize, Serialize};

use crate::market::FeeSchedule;

/// A probability, refusing anything outside `[0, 1]`.
///
/// A price implying more than certainty is not a belief that needs clamping,
/// it is an arbitrage that needs taking, and clamping it silently is how one
/// gets missed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Probability(Decimal);

impl Probability {
    pub const ZERO: Self = Self(Decimal::ZERO);
    pub const ONE: Self = Self(Decimal::ONE);

    pub fn new(value: Decimal) -> Result<Self> {
        if value.is_negative() || value > Decimal::ONE {
            return Err(Error::invalid(format!(
                "{value} is not a probability; it lies outside [0, 1]"
            )));
        }
        Ok(Self(value))
    }

    pub const fn value(&self) -> Decimal {
        self.0
    }

    pub fn complement(&self) -> Self {
        Self(Decimal::ONE - self.0)
    }

    /// As an `f64`, for statistics and calibration reporting only.
    pub fn as_f64(&self) -> f64 {
        self.0.to_f64()
    }
}

/// The interval a price and its fees leave the true probability in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbabilityBand {
    lower: Probability,
    upper: Probability,
}

impl ProbabilityBand {
    /// Refuses an inverted band, which would report a crossed book as an
    /// opportunity of negative width.
    pub fn new(lower: Probability, upper: Probability) -> Result<Self> {
        if lower.value() > upper.value() {
            return Err(Error::invalid(format!(
                "a probability band cannot run from {} down to {}",
                lower.value(),
                upper.value()
            )));
        }
        Ok(Self { lower, upper })
    }

    /// The probability below which selling is profitable.
    pub const fn lower(&self) -> Probability {
        self.lower
    }

    /// The probability above which buying is profitable.
    pub const fn upper(&self) -> Probability {
        self.upper
    }

    /// How much better than the market a forecast must be to be worth trading.
    pub fn width(&self) -> Decimal {
        self.upper.value() - self.lower.value()
    }

    pub fn midpoint(&self) -> Result<Probability> {
        Probability::new(
            (self.lower.value() + self.upper.value())
                .checked_div(Decimal::from_int(2))
                .ok_or_else(|| Error::numeric("a band midpoint is undefined"))?,
        )
    }

    /// Whether a forecast is outside the band and therefore actionable.
    pub fn admits(&self, forecast: Probability) -> bool {
        forecast.value() > self.upper.value() || forecast.value() < self.lower.value()
    }
}

/// The naive reading: price divided by payoff, fees ignored.
///
/// Provided so the difference against [`implied_from_ask`] can be measured
/// rather than argued about. Nothing in this crate trades on it.
pub fn naive_probability(price: Decimal, payoff: Decimal) -> Result<Probability> {
    Probability::new(
        price
            .checked_div(payoff)
            .ok_or_else(|| Error::invalid("a market with a zero payoff implies nothing"))?,
    )
}

/// The probability at which buying at `ask` breaks even.
///
/// Above this, buying is profitable. It is the upper edge of the band.
pub fn implied_from_ask(ask: Decimal, fees: &FeeSchedule, payoff: Decimal) -> Result<Probability> {
    if ask.is_negative() {
        return Err(Error::invalid("an ask price cannot be negative"));
    }
    let cost = fees.gross_up(ask);
    let net_payoff = fees.net_payoff(payoff);
    let implied = cost
        .checked_div(net_payoff)
        .ok_or_else(|| Error::invalid("a market whose payoff nets to zero implies nothing"))?;
    Probability::new(implied).map_err(|_| {
        Error::invalid(format!(
            "an ask of {ask} costs {cost} against a net payoff of {net_payoff}: that is an arbitrage, not a probability"
        ))
    })
}

/// The probability at which selling at `bid` breaks even.
///
/// Below this, selling is profitable. It is the lower edge of the band.
pub fn implied_from_bid(bid: Decimal, fees: &FeeSchedule, payoff: Decimal) -> Result<Probability> {
    if bid.is_negative() {
        return Err(Error::invalid("a bid price cannot be negative"));
    }
    let proceeds = fees.net_down(bid);
    let implied = proceeds
        .checked_div(payoff)
        .ok_or_else(|| Error::invalid("a market with a zero payoff implies nothing"))?;
    Probability::new(implied).map_err(|_| {
        Error::invalid(format!(
            "a bid of {bid} nets {proceeds} against a payoff of {payoff}: that is an arbitrage, not a probability"
        ))
    })
}

/// The band a two-sided book implies.
///
/// The touch is used deliberately: this is what the market believes, not what
/// a given size can be done at. Sizing is [`crate::arbitrage`]'s problem.
pub fn implied_band(
    book: &OrderBook,
    fees: &FeeSchedule,
    payoff: Decimal,
) -> Result<ProbabilityBand> {
    let ask = book
        .best_ask()
        .ok_or_else(|| Error::not_found("the book has no offer to imply a probability from"))?;
    let bid = book
        .best_bid()
        .ok_or_else(|| Error::not_found("the book has no bid to imply a probability from"))?;
    ProbabilityBand::new(
        implied_from_bid(bid.price, fees, payoff)?,
        implied_from_ask(ask.price, fees, payoff)?,
    )
}

/// How far a set of implied probabilities is from summing to one.
///
/// A persistent deviation is information: positive is the venue's overround,
/// which is where its edge lives, and negative is either a mispricing or a fee
/// model that has been got wrong. Reporting the deviation rather than
/// normalising it away is the only way to tell which.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SumDeviation {
    /// Sum of the probabilities implied by the offers.
    pub ask_sum: Decimal,
    /// Sum of the probabilities implied by the bids.
    pub bid_sum: Decimal,
}

impl SumDeviation {
    /// How much more than certainty the offers cost. Positive is the normal
    /// state of a fee-charging venue.
    pub fn overround(&self) -> Decimal {
        self.ask_sum - Decimal::ONE
    }

    /// How far the bids fall short of certainty.
    pub fn underround(&self) -> Decimal {
        Decimal::ONE - self.bid_sum
    }

    /// Buying every outcome is cheaper than the payoff: a locked profit if the
    /// depth is there.
    pub fn offers_are_arbitrageable(&self) -> bool {
        self.ask_sum < Decimal::ONE
    }

    /// Selling every outcome raises more than the payoff.
    pub fn bids_are_arbitrageable(&self) -> bool {
        self.bid_sum > Decimal::ONE
    }
}
