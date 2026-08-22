//! Automated market makers: reserves, curves and computed prices.
//!
//! A pool does not quote. It holds reserves and a curve, and the price is
//! whatever those two imply for the size being traded — so a "price" from a
//! pool is a function of size, not a number, and the fee and the impact are
//! part of it rather than adjustments applied afterwards.
//!
//! All of it is integer arithmetic at the same width the contract uses. A
//! float approximation would agree to eight or nine digits, which sounds like
//! plenty until the whole opportunity is four basis points wide and the leg
//! planner is deciding whether to send.

use qip_contracts::{BookSide, VenueId};
use qip_core::decimal::SCALE;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

use crate::block::BlockNumber;
use crate::math::{cmp_products, mul_add_div_floor, mul_div_ceil, mul_div_floor};

/// Newton iteration ceiling for the stable-swap solves.
const STABLE_MAX_ITERATIONS: usize = 255;

/// A pool's on-chain identity, usually its address.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PoolId(String);

impl PoolId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PoolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A swap fee in basis points of the amount it is charged on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FeeBps(u32);

impl FeeBps {
    /// Refuses a fee of a whole turn or more, which would make every quote
    /// zero and every inverse quote unbounded.
    pub fn new(bps: u32) -> Result<Self> {
        if bps >= 10_000 {
            return Err(Error::invalid(format!(
                "a swap fee of {bps}bp is a hundred percent or more"
            )));
        }
        Ok(Self(bps))
    }

    pub const fn bps(&self) -> u32 {
        self.0
    }

    /// The complement, in basis points: what survives the fee.
    const fn retained(&self) -> u128 {
        (10_000 - self.0) as u128
    }
}

/// Which curve the pool's reserves sit on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoolCurve {
    /// `x · y = k`. Prices every size, badly for large ones.
    ConstantProduct,
    /// The stable-swap invariant, which is nearly flat near balance and
    /// degrades to constant product away from it.
    ///
    /// `amplification` is the `A` of the published curve: higher means flatter
    /// near the peg and a sharper cliff once the pool tips.
    StableSwap { amplification: u32 },
}

impl PoolCurve {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ConstantProduct => "constant_product",
            Self::StableSwap { .. } => "stable_swap",
        }
    }
}

/// Which leg of the swap the fee is taken from.
///
/// Not a detail: constant-product pools skim the input before it touches the
/// curve, stable-swap pools take it out of the output afterwards, and a caller
/// reconciling a fill against a quote has to know which.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeeSide {
    Input,
    Output,
}

/// The value of a pool's curve, at the width it actually needs.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PoolInvariant {
    /// The product of the reserves, which does not fit in a [`Decimal`].
    ConstantProduct { high: u128, low: u128 },
    /// The stable-swap `D`, which is of the order of the reserve total.
    StableSwap { d: Decimal },
}

impl PartialOrd for PoolInvariant {
    /// Two invariants are only comparable when they come from the same curve.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (
                Self::ConstantProduct { high, low },
                Self::ConstantProduct {
                    high: other_high,
                    low: other_low,
                },
            ) => Some((high, low).cmp(&(other_high, other_low))),
            (Self::StableSwap { d }, Self::StableSwap { d: other_d }) => Some(d.cmp(other_d)),
            _ => None,
        }
    }
}

/// A computed swap: what goes in, what comes out, and what it costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SwapQuote {
    pub pool: PoolId,
    /// The side the taker lifted. [`BookSide::Ask`] means the taker bought
    /// base, so the input is denominated in quote and the output in base.
    pub taker: BookSide,
    pub amount_in: Decimal,
    pub amount_out: Decimal,
    pub fee: Decimal,
    pub fee_side: FeeSide,
    /// Quote per unit of base, actually achieved over the whole size.
    pub effective_price: Decimal,
    /// Quote per unit of base at the margin, before the trade.
    pub spot_price: Decimal,
    /// Adverse move from spot to effective, as a fraction of spot. Never
    /// negative: a pool cannot fill better than its own marginal price.
    pub price_impact: Decimal,
    pub reserve_base_after: Decimal,
    pub reserve_quote_after: Decimal,
}

impl SwapQuote {
    /// Price impact in basis points, as a statistic for logs and thresholds.
    pub fn price_impact_bps(&self) -> f64 {
        self.price_impact.to_f64() * 10_000.0
    }
}

/// A pool's state as of a block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pool {
    pub id: PoolId,
    pub venue: VenueId,
    pub base: ObjectId,
    pub quote: ObjectId,
    pub curve: PoolCurve,
    pub fee: FeeBps,
    reserve_base: Decimal,
    reserve_quote: Decimal,
    /// The block these reserves are from. A pool state without a block cannot
    /// be given a finality, and a reserve figure of unknown age is a guess.
    pub as_of: BlockNumber,
}

impl Pool {
    /// Build a pool state, refusing reserves that cannot price anything.
    pub fn new(
        id: PoolId,
        venue: VenueId,
        base: ObjectId,
        quote: ObjectId,
        curve: PoolCurve,
        fee: FeeBps,
        reserve_base: Decimal,
        reserve_quote: Decimal,
        as_of: BlockNumber,
    ) -> Result<Self> {
        if !reserve_base.is_positive() || !reserve_quote.is_positive() {
            return Err(Error::invalid(format!(
                "pool {id} has reserves {reserve_base}/{reserve_quote}; a pool with an empty side has no price"
            )));
        }
        if let PoolCurve::StableSwap { amplification } = curve
            && amplification == 0
        {
            return Err(Error::invalid(format!(
                "pool {id} declares a stable-swap amplification of zero"
            )));
        }
        Ok(Self {
            id,
            venue,
            base,
            quote,
            curve,
            fee,
            reserve_base,
            reserve_quote,
            as_of,
        })
    }

    pub const fn reserve_base(&self) -> Decimal {
        self.reserve_base
    }

    pub const fn reserve_quote(&self) -> Decimal {
        self.reserve_quote
    }

    /// Overwrite the reserves, as a sync from a node would.
    pub fn set_reserves(&mut self, base: Decimal, quote: Decimal, as_of: BlockNumber) -> Result<()> {
        if !base.is_positive() || !quote.is_positive() {
            return Err(Error::invalid(format!(
                "pool {} cannot hold reserves {base}/{quote}",
                self.id
            )));
        }
        self.reserve_base = base;
        self.reserve_quote = quote;
        self.as_of = as_of;
        Ok(())
    }

    /// The value of the curve at the current reserves.
    pub fn invariant(&self) -> Result<PoolInvariant> {
        let base = positive_raw(self.reserve_base, "reserve")?;
        let quote = positive_raw(self.reserve_quote, "reserve")?;
        match self.curve {
            PoolCurve::ConstantProduct => {
                let (high, low) = crate::math::wide_mul(base, quote);
                Ok(PoolInvariant::ConstantProduct { high, low })
            }
            PoolCurve::StableSwap { amplification } => {
                let d = stable_d(base, quote, ann(amplification))?;
                Ok(PoolInvariant::StableSwap {
                    d: to_decimal(d)?,
                })
            }
        }
    }

    /// Quote per base at the margin, with no size and no fee.
    ///
    /// The number to compare a book's mid against; it is not obtainable, since
    /// any executable size moves along the curve.
    pub fn spot_price(&self) -> Result<Decimal> {
        let base = positive_raw(self.reserve_base, "reserve")?;
        let quote = positive_raw(self.reserve_quote, "reserve")?;
        let raw = match self.curve {
            PoolCurve::ConstantProduct => mul_div_floor(quote, SCALE as u128, base),
            PoolCurve::StableSwap { amplification } => {
                stable_marginal_price(base, quote, ann(amplification))
            }
        }
        .ok_or_else(|| Error::numeric(format!("pool {} spot price overflowed", self.id)))?;
        to_decimal(raw)
    }

    /// The instrument a taker pays in, on this side.
    pub fn input_object(&self, taker: BookSide) -> &ObjectId {
        match taker {
            BookSide::Ask => &self.quote,
            BookSide::Bid => &self.base,
        }
    }

    /// The instrument a taker receives, on this side.
    pub fn output_object(&self, taker: BookSide) -> &ObjectId {
        match taker {
            BookSide::Ask => &self.base,
            BookSide::Bid => &self.quote,
        }
    }

    /// The output for an exact input, fee and price impact included.
    pub fn quote_exact_in(&self, taker: BookSide, amount_in: Decimal) -> Result<SwapQuote> {
        let input = positive_raw(amount_in, "swap input")?;
        let (reserve_in, reserve_out) = self.directed_reserves(taker)?;
        let (output, fee, fee_side) = match self.curve {
            PoolCurve::ConstantProduct => {
                let charged = mul_div_floor(input, self.fee.retained(), 10_000)
                    .ok_or_else(|| Error::numeric("swap fee overflowed"))?;
                let out = mul_div_floor(
                    reserve_out,
                    charged,
                    reserve_in
                        .checked_add(charged)
                        .ok_or_else(|| Error::numeric("pool reserve overflowed"))?,
                )
                .ok_or_else(|| Error::numeric("swap output overflowed"))?;
                (out, input - charged, FeeSide::Input)
            }
            PoolCurve::StableSwap { amplification } => {
                let ann = ann(amplification);
                let d = stable_d(reserve_in, reserve_out, ann)?;
                let new_in = reserve_in
                    .checked_add(input)
                    .ok_or_else(|| Error::numeric("pool reserve overflowed"))?;
                let new_out = stable_y(new_in, d, ann)?;
                // One raw unit is left in the pool so integer rounding can
                // never make the invariant fall.
                let gross = reserve_out.saturating_sub(new_out).saturating_sub(1);
                let net = mul_div_floor(gross, self.fee.retained(), 10_000)
                    .ok_or_else(|| Error::numeric("swap fee overflowed"))?;
                (net, gross - net, FeeSide::Output)
            }
        };
        if output == 0 {
            return Err(Error::invalid(format!(
                "an input of {amount_in} to pool {} buys nothing at these reserves",
                self.id
            )));
        }
        if output >= reserve_out {
            return Err(Error::numeric(format!(
                "pool {} cannot deliver {output} against a reserve of {reserve_out}",
                self.id
            )));
        }
        self.build_quote(taker, input, output, fee, fee_side)
    }

    /// The input required for an exact output — what a leg planner needs when
    /// the size is set by the other leg rather than by this one.
    pub fn quote_exact_out(&self, taker: BookSide, amount_out: Decimal) -> Result<SwapQuote> {
        let output = positive_raw(amount_out, "swap output")?;
        let (reserve_in, reserve_out) = self.directed_reserves(taker)?;
        if output >= reserve_out {
            return Err(Error::invalid(format!(
                "pool {} holds {reserve_out} and cannot deliver {amount_out}",
                self.id
            )));
        }
        let (input, fee, fee_side) = match self.curve {
            PoolCurve::ConstantProduct => {
                let charged = mul_div_ceil(reserve_in, output, reserve_out - output)
                    .ok_or_else(|| Error::numeric("swap input overflowed"))?;
                let gross = mul_div_ceil(charged, 10_000, self.fee.retained())
                    .ok_or_else(|| Error::numeric("swap fee overflowed"))?;
                (gross, gross - charged, FeeSide::Input)
            }
            PoolCurve::StableSwap { amplification } => {
                let ann = ann(amplification);
                let d = stable_d(reserve_in, reserve_out, ann)?;
                let gross_out = mul_div_ceil(output, 10_000, self.fee.retained())
                    .ok_or_else(|| Error::numeric("swap fee overflowed"))?;
                if gross_out >= reserve_out {
                    return Err(Error::invalid(format!(
                        "pool {} cannot deliver {amount_out} net of fee",
                        self.id
                    )));
                }
                let new_out = reserve_out - gross_out;
                let new_in = stable_y(new_out, d, ann)?;
                let input = new_in
                    .checked_sub(reserve_in)
                    .ok_or_else(|| Error::numeric("stable-swap inverse went backwards"))?
                    .checked_add(1)
                    .ok_or_else(|| Error::numeric("stable-swap input overflowed"))?;
                (input, gross_out - output, FeeSide::Output)
            }
        };
        self.build_quote(taker, input, output, fee, fee_side)
    }

    /// Move the reserves as the swap would, so a caller can walk a route.
    pub fn apply(&mut self, quote: &SwapQuote) -> Result<()> {
        if quote.pool != self.id {
            return Err(Error::invalid(format!(
                "quote for pool {} applied to pool {}",
                quote.pool, self.id
            )));
        }
        self.reserve_base = quote.reserve_base_after;
        self.reserve_quote = quote.reserve_quote_after;
        Ok(())
    }

    fn directed_reserves(&self, taker: BookSide) -> Result<(u128, u128)> {
        let base = positive_raw(self.reserve_base, "reserve")?;
        let quote = positive_raw(self.reserve_quote, "reserve")?;
        Ok(match taker {
            BookSide::Ask => (quote, base),
            BookSide::Bid => (base, quote),
        })
    }

    fn build_quote(
        &self,
        taker: BookSide,
        input: u128,
        output: u128,
        fee: u128,
        fee_side: FeeSide,
    ) -> Result<SwapQuote> {
        let amount_in = to_decimal(input)?;
        let amount_out = to_decimal(output)?;
        let (base_after, quote_after, base_amount, quote_amount) = match taker {
            BookSide::Ask => (
                self.reserve_base - amount_out,
                self.reserve_quote + amount_in,
                amount_out,
                amount_in,
            ),
            BookSide::Bid => (
                self.reserve_base + amount_in,
                self.reserve_quote - amount_out,
                amount_in,
                amount_out,
            ),
        };
        let effective_price = quote_amount
            .checked_div(base_amount)
            .ok_or_else(|| Error::numeric("swap produced no base amount to price"))?;
        let spot_price = self.spot_price()?;
        // The pool cannot fill better than its marginal price, so the adverse
        // direction is the only one; a negative figure here would be a
        // rounding artefact reported as free money.
        let adverse = match taker {
            BookSide::Ask => effective_price - spot_price,
            BookSide::Bid => spot_price - effective_price,
        };
        let price_impact = adverse
            .max(Decimal::ZERO)
            .checked_div(spot_price)
            .ok_or_else(|| Error::numeric("pool spot price is zero"))?;
        Ok(SwapQuote {
            pool: self.id.clone(),
            taker,
            amount_in,
            amount_out,
            fee: to_decimal(fee)?,
            fee_side,
            effective_price,
            spot_price,
            price_impact,
            reserve_base_after: base_after,
            reserve_quote_after: quote_after,
        })
    }
}

/// Whether the reserves after a swap still satisfy the constant-product
/// invariant, given the input that reached the curve.
///
/// Exposed because "the pool never loses" is the property that has to hold for
/// every quote this module produces, and checking it needs the full-width
/// comparison rather than a division.
pub fn constant_product_holds(
    reserve_in_before: Decimal,
    reserve_out_before: Decimal,
    input_after_fee: Decimal,
    output: Decimal,
) -> Result<bool> {
    let before_in = positive_raw(reserve_in_before, "reserve")?;
    let before_out = positive_raw(reserve_out_before, "reserve")?;
    let input = positive_raw(input_after_fee, "input")?;
    let output = positive_raw(output, "output")?;
    let after_in = before_in
        .checked_add(input)
        .ok_or_else(|| Error::numeric("reserve overflowed"))?;
    let after_out = before_out
        .checked_sub(output)
        .ok_or_else(|| Error::numeric("output exceeds the reserve"))?;
    Ok(cmp_products(after_in, after_out, before_in, before_out) != Ordering::Less)
}

const fn ann(amplification: u32) -> u128 {
    // A · n^n for the two-coin case.
    amplification as u128 * 4
}

/// Solve the stable-swap invariant `D` for two reserves by Newton iteration.
fn stable_d(x: u128, y: u128, ann: u128) -> Result<u128> {
    let sum = x
        .checked_add(y)
        .ok_or_else(|| Error::numeric("stable-swap reserves overflowed"))?;
    if sum == 0 {
        return Ok(0);
    }
    let mut d = sum;
    for _ in 0..STABLE_MAX_ITERATIONS {
        let step = mul_div_floor(d, d, x.saturating_mul(2))
            .and_then(|partial| mul_div_floor(partial, d, y.saturating_mul(2)))
            .ok_or_else(|| Error::numeric("stable-swap D iteration overflowed"))?;
        let previous = d;
        let numerator = ann
            .checked_mul(sum)
            .and_then(|term| term.checked_add(step.checked_mul(2)?))
            .ok_or_else(|| Error::numeric("stable-swap D numerator overflowed"))?;
        let denominator = (ann - 1)
            .checked_mul(d)
            .and_then(|term| term.checked_add(step.checked_mul(3)?))
            .ok_or_else(|| Error::numeric("stable-swap D denominator overflowed"))?;
        d = mul_div_floor(numerator, d, denominator)
            .ok_or_else(|| Error::numeric("stable-swap D iteration overflowed"))?;
        if d.abs_diff(previous) <= 1 {
            return Ok(d);
        }
    }
    Err(Error::numeric(
        "the stable-swap invariant did not converge in 255 iterations",
    ))
}

/// Solve for the reserve on the other side, given one reserve and `D`.
///
/// The two-coin invariant is symmetric, so the same solve serves both
/// directions and the inverse quote does not need a second implementation to
/// disagree with the first.
fn stable_y(x: u128, d: u128, ann: u128) -> Result<u128> {
    if x == 0 {
        return Err(Error::invalid("stable-swap solve needs a positive reserve"));
    }
    let c = mul_div_floor(d, d, x.saturating_mul(2))
        .and_then(|partial| mul_div_floor(partial, d, ann.saturating_mul(2)))
        .ok_or_else(|| Error::numeric("stable-swap y constant overflowed"))?;
    let b = x
        .checked_add(d / ann)
        .ok_or_else(|| Error::numeric("stable-swap y constant overflowed"))?;
    let mut y = d;
    for _ in 0..STABLE_MAX_ITERATIONS {
        let previous = y;
        let denominator = y
            .checked_mul(2)
            .and_then(|term| term.checked_add(b))
            .and_then(|term| term.checked_sub(d))
            .ok_or_else(|| Error::numeric("stable-swap y iteration left the domain"))?;
        y = mul_add_div_floor(y, y, c, denominator)
            .ok_or_else(|| Error::numeric("stable-swap y iteration overflowed"))?;
        if y.abs_diff(previous) <= 1 {
            return Ok(y);
        }
    }
    Err(Error::numeric(
        "the stable-swap solve did not converge in 255 iterations",
    ))
}

/// The marginal price `dy/dx` on the stable-swap curve, as a scaled ratio.
///
/// Taken from the derivative of the invariant rather than from a small probe
/// trade: a probe would fold its own price impact into the answer, which is
/// the one thing a spot price must not contain.
fn stable_marginal_price(x: u128, y: u128, ann: u128) -> Option<u128> {
    let d = stable_d(x, y, ann).ok()?;
    let scale = SCALE as u128;
    // D³ / (4·x²·y) and D³ / (4·x·y²), each carried at Decimal scale so the
    // amplification term and the curvature term are in the same units.
    let dx = mul_div_floor(d, d, x.checked_mul(2)?)
        .and_then(|partial| mul_div_floor(partial, d, x.checked_mul(2)?))
        .and_then(|partial| mul_div_floor(partial, scale, y))?;
    let dy = mul_div_floor(d, d, y.checked_mul(2)?)
        .and_then(|partial| mul_div_floor(partial, d, y.checked_mul(2)?))
        .and_then(|partial| mul_div_floor(partial, scale, x))?;
    let numerator = ann.checked_mul(scale)?.checked_add(dx)?;
    let denominator = ann.checked_mul(scale)?.checked_add(dy)?;
    mul_div_floor(numerator, scale, denominator)
}

fn positive_raw(value: Decimal, what: &str) -> Result<u128> {
    if !value.is_positive() {
        return Err(Error::invalid(format!("{what} must be positive, got {value}")));
    }
    Ok(value.raw().unsigned_abs())
}

fn to_decimal(raw: u128) -> Result<Decimal> {
    i128::try_from(raw)
        .map(Decimal::from_raw)
        .map_err(|_| Error::numeric(format!("{raw} exceeds the decimal range")))
}
