//! On-chain integer amounts and their conversion to [`Decimal`].
//!
//! A chain reports balances as integers scaled by the token's own decimal
//! exponent — eighteen for most ERC-20s, six for the common stablecoins, eight
//! for wrapped Bitcoin. [`Decimal`] carries nine fractional digits, so an
//! eighteen-decimal amount has nine digits that cannot be represented.
//!
//! Those nine digits are almost always dust and almost never nothing. A
//! conversion that drops them silently turns an accounting identity into an
//! approximate one, and the discrepancy surfaces days later as an unexplained
//! position break. Every conversion here therefore returns a [`Conversion`]
//! that carries the residual, and a caller must decide — in code, visibly —
//! whether to demand exactness or accept the rounding.

use qip_core::decimal::SCALE_DIGITS;
use qip_core::error::{Error, Result};
use qip_core::Decimal;
use serde::{Deserialize, Serialize};

/// The largest decimal exponent a token may declare.
///
/// Beyond this the raw integer cannot be held in `i128` at any useful
/// magnitude, so accepting it would only defer the failure.
pub const MAX_TOKEN_DECIMALS: u8 = 30;

/// An integer amount as the chain reports it, with the token's exponent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TokenAmount {
    raw: i128,
    decimals: u8,
}

impl TokenAmount {
    /// Wrap a raw integer amount for a token with `decimals` fractional digits.
    pub fn new(raw: i128, decimals: u8) -> Result<Self> {
        if decimals > MAX_TOKEN_DECIMALS {
            return Err(Error::invalid(format!(
                "token declares {decimals} decimals, more than the {MAX_TOKEN_DECIMALS} supported"
            )));
        }
        Ok(Self { raw, decimals })
    }

    /// An eighteen-decimal amount, the ERC-20 default.
    pub fn wei(raw: i128) -> Self {
        Self { raw, decimals: 18 }
    }

    pub const fn raw(&self) -> i128 {
        self.raw
    }

    pub const fn decimals(&self) -> u8 {
        self.decimals
    }

    pub const fn is_zero(&self) -> bool {
        self.raw == 0
    }

    /// Convert to a [`Decimal`], reporting what could not be represented.
    ///
    /// Never fails for a representable magnitude; a magnitude beyond
    /// [`Decimal`]'s range is a numeric error rather than a saturated value,
    /// because a saturated balance is worse than no balance at all.
    pub fn to_decimal(&self) -> Result<Conversion> {
        let exponent = i32::from(self.decimals) - SCALE_DIGITS as i32;
        if exponent <= 0 {
            let multiplier = pow10(exponent.unsigned_abs())?;
            let scaled = self.raw.checked_mul(multiplier).ok_or_else(|| {
                Error::numeric(format!(
                    "{} at {} decimals exceeds the decimal range",
                    self.raw, self.decimals
                ))
            })?;
            return Ok(Conversion {
                truncated: Decimal::from_raw(scaled),
                rounded: Decimal::from_raw(scaled),
                residual: 0,
                divisor: 1,
                decimals: self.decimals,
            });
        }

        let divisor = pow10(exponent.unsigned_abs())?;
        let truncated = self.raw / divisor;
        let residual = self.raw % divisor;
        // Half away from zero, matching the rounding Decimal itself uses.
        let bump = if residual.abs() * 2 >= divisor {
            self.raw.signum()
        } else {
            0
        };
        Ok(Conversion {
            truncated: Decimal::from_raw(truncated),
            rounded: Decimal::from_raw(truncated + bump),
            residual,
            divisor,
            decimals: self.decimals,
        })
    }

    /// Express a [`Decimal`] in this token's raw units, reporting the residual.
    ///
    /// The inverse direction loses precision whenever the token carries fewer
    /// than nine decimals — a USDC amount of 0.0000005 is not payable.
    pub fn quantise(value: Decimal, decimals: u8) -> Result<Quantised> {
        let exponent = i32::from(decimals) - SCALE_DIGITS as i32;
        if exponent >= 0 {
            let multiplier = pow10(exponent.unsigned_abs())?;
            let raw = value.raw().checked_mul(multiplier).ok_or_else(|| {
                Error::numeric(format!("{value} at {decimals} decimals overflows an i128"))
            })?;
            return Ok(Quantised {
                amount: TokenAmount::new(raw, decimals)?,
                residual: Decimal::ZERO,
            });
        }
        let divisor = pow10(exponent.unsigned_abs())?;
        let raw = value.raw() / divisor;
        let residual = value.raw() % divisor;
        Ok(Quantised {
            amount: TokenAmount::new(raw, decimals)?,
            residual: Decimal::from_raw(residual),
        })
    }
}

/// The result of narrowing a chain integer into a [`Decimal`].
///
/// There is deliberately no plain accessor for the value: a caller reaches it
/// either through [`Conversion::require_exact`], which refuses to hand back an
/// amount it had to shorten, or through [`Conversion::rounded`] /
/// [`Conversion::truncated`], which name the compromise being made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversion {
    truncated: Decimal,
    rounded: Decimal,
    residual: i128,
    divisor: i128,
    decimals: u8,
}

impl Conversion {
    /// Whether every digit of the original amount survived.
    pub const fn is_exact(&self) -> bool {
        self.residual == 0
    }

    /// The raw sub-nanounit remainder that could not be represented.
    pub const fn residual(&self) -> i128 {
        self.residual
    }

    /// The residual as a fraction of one unit in the last representable place.
    ///
    /// A statistic, used for reporting how much precision a feed is costing.
    pub fn residual_fraction(&self) -> f64 {
        if self.divisor == 0 {
            return 0.0;
        }
        self.residual as f64 / self.divisor as f64
    }

    /// The value, refusing to answer when digits were dropped.
    pub fn require_exact(&self) -> Result<Decimal> {
        if self.is_exact() {
            return Ok(self.truncated);
        }
        Err(Error::numeric(format!(
            "a {}-decimal amount left a residual of {} raw units, which nine decimal digits cannot represent",
            self.decimals, self.residual
        )))
    }

    /// The value rounded half away from zero, accepting the loss.
    pub const fn rounded(&self) -> Decimal {
        self.rounded
    }

    /// The value truncated toward zero, accepting the loss.
    ///
    /// The right choice when the amount is something the platform must be able
    /// to deliver: rounding a payable up creates a shortfall of exactly the
    /// kind nobody notices until settlement.
    pub const fn truncated(&self) -> Decimal {
        self.truncated
    }
}

/// The result of widening a [`Decimal`] into a chain integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quantised {
    amount: TokenAmount,
    residual: Decimal,
}

impl Quantised {
    pub const fn is_exact(&self) -> bool {
        self.residual.raw() == 0
    }

    /// What the token's precision cannot carry, in the source scale.
    pub const fn residual(&self) -> Decimal {
        self.residual
    }

    /// The amount, refusing when the token could not carry every digit.
    pub fn require_exact(&self) -> Result<TokenAmount> {
        if self.is_exact() {
            return Ok(self.amount);
        }
        Err(Error::numeric(format!(
            "a {}-decimal token cannot carry a residual of {}",
            self.amount.decimals(),
            self.residual
        )))
    }

    /// The amount truncated toward zero, accepting the loss.
    pub const fn truncated(&self) -> TokenAmount {
        self.amount
    }
}

fn pow10(exponent: u32) -> Result<i128> {
    10i128
        .checked_pow(exponent)
        .ok_or_else(|| Error::numeric(format!("10^{exponent} does not fit in an i128")))
}
