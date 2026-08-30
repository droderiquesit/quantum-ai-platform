//! Currency-tagged amounts.
//!
//! [`Money`] refuses to add two different currencies. Cross-currency arithmetic
//! must go through an explicit FX conversion, which is where the rate, its
//! source and its timestamp get recorded.

use crate::decimal::Decimal;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// ISO 4217 alphabetic code, plus digital-asset tickers.
///
/// Stored as a fixed 4-byte buffer so `Currency` stays `Copy` and cheap to pass
/// through hot paths such as position keying.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Currency {
    code: [u8; 4],
    len: u8,
}

impl Currency {
    pub const USD: Self = Self::literal(b"USD");
    pub const EUR: Self = Self::literal(b"EUR");
    pub const GBP: Self = Self::literal(b"GBP");
    pub const JPY: Self = Self::literal(b"JPY");
    pub const CHF: Self = Self::literal(b"CHF");
    pub const CNY: Self = Self::literal(b"CNY");
    pub const AUD: Self = Self::literal(b"AUD");
    pub const CAD: Self = Self::literal(b"CAD");

    const fn literal(code: &[u8; 3]) -> Self {
        Self {
            code: [code[0], code[1], code[2], 0],
            len: 3,
        }
    }

    /// Parse a 2-to-4 character alphanumeric code, upper-cased.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.len() < 2 || s.len() > 4 || !s.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::invalid(format!("invalid currency code: {s}")));
        }
        let mut code = [0u8; 4];
        for (i, b) in s.bytes().enumerate() {
            code[i] = b.to_ascii_uppercase();
        }
        Ok(Self {
            code,
            len: s.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.code[..self.len as usize]).unwrap_or("???")
    }

    /// Conventional minor-unit digits used for display and settlement rounding.
    pub fn display_digits(&self) -> u32 {
        match self.as_str() {
            "JPY" | "KRW" | "CLP" | "ISK" => 0,
            "BTC" | "ETH" => 8,
            _ => 2,
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Default for Currency {
    fn default() -> Self {
        Self::USD
    }
}

impl Serialize for Currency {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as DeError;
        let s = String::deserialize(d)?;
        Currency::parse(&s).map_err(|e| D::Error::custom(e.to_string()))
    }
}

/// An exact amount in a specific currency.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub amount: Decimal,
    pub currency: Currency,
}

impl Money {
    pub fn new(amount: Decimal, currency: Currency) -> Self {
        Self { amount, currency }
    }

    pub fn zero(currency: Currency) -> Self {
        Self {
            amount: Decimal::ZERO,
            currency,
        }
    }

    pub fn usd(amount: Decimal) -> Self {
        Self {
            amount,
            currency: Currency::USD,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.amount.is_zero()
    }

    /// Add, rejecting a currency mismatch instead of silently coercing.
    pub fn checked_add(self, rhs: Self) -> Result<Self> {
        self.require_same(rhs)?;
        Ok(Self {
            amount: self.amount + rhs.amount,
            currency: self.currency,
        })
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self> {
        self.require_same(rhs)?;
        Ok(Self {
            amount: self.amount - rhs.amount,
            currency: self.currency,
        })
    }

    /// Scale by a dimensionless factor (a quantity, a weight, a ratio).
    pub fn scale(self, factor: Decimal) -> Self {
        Self {
            amount: self.amount * factor,
            currency: self.currency,
        }
    }

    /// Flip the sign. Named to avoid shadowing `std::ops::Neg::neg`, which
    /// `Money` deliberately does not implement — negating an amount should be
    /// an explicit act, not an operator that hides a sign error.
    pub fn negated(self) -> Self {
        Self {
            amount: -self.amount,
            currency: self.currency,
        }
    }

    pub fn abs(self) -> Self {
        Self {
            amount: self.amount.abs(),
            currency: self.currency,
        }
    }

    /// Convert at an explicit rate quoted as `to` units per one `self.currency`.
    pub fn convert(self, to: Currency, rate: Decimal) -> Self {
        Self {
            amount: self.amount * rate,
            currency: to,
        }
    }

    /// Round to the currency's conventional minor units.
    pub fn round_to_minor(self) -> Self {
        Self {
            amount: self.amount.round_dp(self.currency.display_digits()),
            currency: self.currency,
        }
    }

    fn require_same(self, rhs: Self) -> Result<()> {
        if self.currency != rhs.currency {
            return Err(Error::invalid(format!(
                "currency mismatch: {} vs {}",
                self.currency, rhs.currency
            )));
        }
        Ok(())
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            self.amount.round_dp(self.currency.display_digits()),
            self.currency
        )
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Money({self})")
    }
}
