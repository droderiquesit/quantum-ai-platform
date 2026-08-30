//! Exact arithmetic that reports overflow instead of aborting.
//!
//! The `Decimal` operators panic on overflow, which is right for a balance
//! computed from values already known to be sane. A path multiplier is not that
//! — it is a product of a caller's rates over a cycle a search proposed, and an
//! overflow there is data about a malformed graph rather than a bug in this
//! crate. These wrappers turn it into an error a scan can record and move past.

use qip_core::Decimal;
use qip_core::error::{Error, Result};

pub(crate) fn mul(a: Decimal, b: Decimal, what: &str) -> Result<Decimal> {
    a.checked_mul(b)
        .ok_or_else(|| Error::numeric(format!("{what} overflowed: {a} x {b}")))
}

pub(crate) fn div(a: Decimal, b: Decimal, what: &str) -> Result<Decimal> {
    a.checked_div(b)
        .ok_or_else(|| Error::numeric(format!("{what} is undefined: {a} / {b}")))
}

/// Convert a statistic back into an exact quantity.
///
/// The only sanctioned crossing from `f64` to `Decimal` in this crate, and it
/// refuses rather than silently substituting zero — a volatility that came out
/// as NaN must stop a net edge, not quietly make one of its deductions free.
pub(crate) fn from_statistic(value: f64, what: &str) -> Result<Decimal> {
    if !value.is_finite() {
        return Err(Error::numeric(format!("{what} is not a finite number")));
    }
    Decimal::from_f64(value)
        .ok_or_else(|| Error::numeric(format!("{what} does not fit an exact decimal: {value}")))
}
