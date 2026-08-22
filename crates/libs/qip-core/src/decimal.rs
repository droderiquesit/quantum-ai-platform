//! Exact fixed-point decimal arithmetic for money, prices and quantities.
//!
//! Backed by `i128` scaled by 10^9 (nine fractional digits). That range covers
//! every instrument the platform models — from a 0.00000001 BTC quantity to a
//! multi-trillion notional — without the representation error that makes `f64`
//! unfit for balances. Statistics (returns, volatility, risk) deliberately stay
//! in `f64`; the boundary is crossed explicitly via [`Decimal::to_f64`] and
//! [`Decimal::from_f64`].
//!
//! Arithmetic operators panic on overflow rather than wrapping, matching Rust's
//! debug-mode integer semantics. Call the `checked_*` methods where a caller
//! must handle overflow as data.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Number of fractional decimal digits.
pub const SCALE_DIGITS: u32 = 9;
/// 10^[`SCALE_DIGITS`].
pub const SCALE: i128 = 1_000_000_000;
const SCALE_U: u128 = SCALE as u128;

/// A fixed-point decimal with nine fractional digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Decimal(i128);

impl Decimal {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(SCALE);
    pub const NEG_ONE: Self = Self(-SCALE);
    pub const MAX: Self = Self(i128::MAX);
    pub const MIN: Self = Self(i128::MIN + 1);

    /// Construct from a raw scaled integer (units of 10^-9).
    pub const fn from_raw(raw: i128) -> Self {
        Self(raw)
    }

    /// The underlying scaled integer.
    pub const fn raw(self) -> i128 {
        self.0
    }

    /// Exact construction from a whole number.
    pub fn from_int(v: i64) -> Self {
        Self(i128::from(v) * SCALE)
    }

    /// Construct from `mantissa * 10^-exp`, e.g. `(12345, 2)` is `123.45`.
    pub fn from_scaled(mantissa: i128, exp: u32) -> Option<Self> {
        if exp > SCALE_DIGITS {
            // Round the excess precision away rather than silently truncating.
            let drop = exp - SCALE_DIGITS;
            let divisor = 10i128.checked_pow(drop)?;
            let q = mantissa / divisor;
            let r = (mantissa % divisor).abs();
            let bump = if r * 2 >= divisor {
                if mantissa < 0 { -1 } else { 1 }
            } else {
                0
            };
            return Some(Self(q + bump));
        }
        let mul = 10i128.checked_pow(SCALE_DIGITS - exp)?;
        mantissa.checked_mul(mul).map(Self)
    }

    /// Lossy conversion from `f64`. Rejects NaN and infinities.
    pub fn from_f64(v: f64) -> Option<Self> {
        if !v.is_finite() {
            return None;
        }
        let scaled = v * SCALE as f64;
        if scaled.abs() >= i128::MAX as f64 {
            return None;
        }
        Some(Self(scaled.round() as i128))
    }

    /// Lossy conversion to `f64`, for statistics and reporting only.
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / SCALE as f64
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
    pub fn is_positive(self) -> bool {
        self.0 > 0
    }
    pub fn is_negative(self) -> bool {
        self.0 < 0
    }
    pub fn signum(self) -> i32 {
        match self.0.cmp(&0) {
            Ordering::Greater => 1,
            Ordering::Less => -1,
            Ordering::Equal => 0,
        }
    }

    pub fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }

    pub fn min(self, other: Self) -> Self {
        if self.0 <= other.0 { self } else { other }
    }

    pub fn max(self, other: Self) -> Self {
        if self.0 >= other.0 { self } else { other }
    }

    /// Clamp into `[lo, hi]`. `lo` must not exceed `hi`.
    pub fn clamp(self, lo: Self, hi: Self) -> Self {
        debug_assert!(lo <= hi, "clamp bounds inverted");
        self.max(lo).min(hi)
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0).map(Self)
    }

    /// Multiply with half-away-from-zero rounding at the 9th decimal.
    ///
    /// Splits the left operand into integral and fractional halves so the
    /// intermediate product stays inside `u128` for realistic magnitudes.
    pub fn checked_mul(self, rhs: Self) -> Option<Self> {
        let negative = (self.0 < 0) != (rhs.0 < 0);
        let a = self.0.unsigned_abs();
        let b = rhs.0.unsigned_abs();

        let a_int = a / SCALE_U;
        let a_frac = a % SCALE_U;

        let hi = a_int.checked_mul(b)?;
        let lo_full = a_frac.checked_mul(b)?;
        let lo = lo_full / SCALE_U;
        let rem = lo_full % SCALE_U;
        let round = u128::from(rem * 2 >= SCALE_U);

        let magnitude = hi.checked_add(lo)?.checked_add(round)?;
        if magnitude > i128::MAX as u128 {
            return None;
        }
        let signed = magnitude as i128;
        Some(Self(if negative { -signed } else { signed }))
    }

    /// Divide with half-away-from-zero rounding. `None` when `rhs` is zero.
    pub fn checked_div(self, rhs: Self) -> Option<Self> {
        if rhs.0 == 0 {
            return None;
        }
        let negative = (self.0 < 0) != (rhs.0 < 0);
        let a = self.0.unsigned_abs();
        let b = rhs.0.unsigned_abs();

        let q = a / b;
        let r = a % b;

        let hi = q.checked_mul(SCALE_U)?;
        let scaled_r = r.checked_mul(SCALE_U)?;
        let lo = scaled_r / b;
        let rem = scaled_r % b;
        let round = u128::from(rem.checked_mul(2)? >= b);

        let magnitude = hi.checked_add(lo)?.checked_add(round)?;
        if magnitude > i128::MAX as u128 {
            return None;
        }
        let signed = magnitude as i128;
        Some(Self(if negative { -signed } else { signed }))
    }

    /// Round to `digits` fractional digits, half away from zero.
    pub fn round_dp(self, digits: u32) -> Self {
        if digits >= SCALE_DIGITS {
            return self;
        }
        let factor = 10i128.pow(SCALE_DIGITS - digits);
        let q = self.0 / factor;
        let r = (self.0 % factor).abs();
        let bump = if r * 2 >= factor {
            i128::from(self.signum())
        } else {
            0
        };
        Self((q + bump) * factor)
    }

    /// Truncate toward zero to `digits` fractional digits.
    pub fn truncate_dp(self, digits: u32) -> Self {
        if digits >= SCALE_DIGITS {
            return self;
        }
        let factor = 10i128.pow(SCALE_DIGITS - digits);
        Self((self.0 / factor) * factor)
    }

    /// Round the magnitude down to a multiple of `step` (lot / tick sizing).
    ///
    /// Rounds toward zero, so a short quantity shrinks toward flat rather than
    /// growing — never round a position further from flat by accident.
    pub fn floor_to_step(self, step: Self) -> Self {
        if step.0 <= 0 {
            return self;
        }
        Self((self.0 / step.0) * step.0)
    }

    /// Scale by a basis-point figure: `notional.apply_bps(2.5)` is 2.5bp of it.
    pub fn apply_bps(self, bps: f64) -> Self {
        let factor = Self::from_f64(bps / 10_000.0).unwrap_or(Self::ZERO);
        self.checked_mul(factor).unwrap_or(Self::ZERO)
    }

    /// Parse a decimal string such as `-1234.5678`. Rejects anything else.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (negative, body) = match s.as_bytes()[0] {
            b'-' => (true, &s[1..]),
            b'+' => (false, &s[1..]),
            _ => (false, s),
        };
        if body.is_empty() {
            return None;
        }
        let mut parts = body.splitn(2, '.');
        let int_part = parts.next()?;
        let frac_part = parts.next().unwrap_or("");
        if int_part.is_empty() && frac_part.is_empty() {
            return None;
        }
        if !int_part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if !frac_part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }

        let mut value: i128 = 0;
        for c in int_part.bytes() {
            value = value.checked_mul(10)?.checked_add(i128::from(c - b'0'))?;
        }
        value = value.checked_mul(SCALE)?;

        // Consume up to SCALE_DIGITS fractional digits, rounding on the next one.
        let frac_bytes = frac_part.as_bytes();
        let mut frac: i128 = 0;
        for i in 0..SCALE_DIGITS as usize {
            let digit = frac_bytes.get(i).map_or(0, |c| i128::from(c - b'0'));
            frac = frac * 10 + digit;
        }
        if let Some(&next) = frac_bytes.get(SCALE_DIGITS as usize)
            && next - b'0' >= 5
        {
            frac += 1;
        }
        value = value.checked_add(frac)?;
        Some(Self(if negative { -value } else { value }))
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let negative = self.0 < 0;
        let magnitude = self.0.unsigned_abs();
        let int_part = magnitude / SCALE_U;
        let frac_part = magnitude % SCALE_U;
        let frac = format!("{frac_part:09}");
        let frac = frac.trim_end_matches('0');
        if negative {
            f.write_str("-")?;
        }
        if frac.is_empty() {
            write!(f, "{int_part}")
        } else {
            write!(f, "{int_part}.{frac}")
        }
    }
}

impl fmt::Debug for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Decimal({self})")
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

macro_rules! binop {
    ($trait:ident, $method:ident, $checked:ident, $label:literal) => {
        impl $trait for Decimal {
            type Output = Decimal;
            fn $method(self, rhs: Decimal) -> Decimal {
                self.$checked(rhs)
                    .unwrap_or_else(|| panic!(concat!("decimal ", $label, " overflow")))
            }
        }
    };
}

binop!(Add, add, checked_add, "add");
binop!(Sub, sub, checked_sub, "sub");
binop!(Mul, mul, checked_mul, "mul");
binop!(Div, div, checked_div, "div");

impl Neg for Decimal {
    type Output = Decimal;
    fn neg(self) -> Decimal {
        Decimal(-self.0)
    }
}

impl AddAssign for Decimal {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Decimal {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl std::iter::Sum for Decimal {
    fn sum<I: Iterator<Item = Decimal>>(iter: I) -> Decimal {
        iter.fold(Decimal::ZERO, |a, b| a + b)
    }
}

impl<'a> std::iter::Sum<&'a Decimal> for Decimal {
    fn sum<I: Iterator<Item = &'a Decimal>>(iter: I) -> Decimal {
        iter.fold(Decimal::ZERO, |a, b| a + *b)
    }
}

impl From<i64> for Decimal {
    fn from(v: i64) -> Self {
        Self::from_int(v)
    }
}

impl From<i32> for Decimal {
    fn from(v: i32) -> Self {
        Self::from_int(i64::from(v))
    }
}

/// Serialized as a JSON string so no precision is lost in transit.
impl Serialize for Decimal {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as DeError;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Str(String),
            Int(i64),
            Float(f64),
        }
        match Repr::deserialize(d)? {
            Repr::Str(s) => {
                Decimal::parse(&s).ok_or_else(|| D::Error::custom(format!("invalid decimal: {s}")))
            }
            Repr::Int(i) => Ok(Decimal::from_int(i)),
            Repr::Float(f) => {
                Decimal::from_f64(f).ok_or_else(|| D::Error::custom("non-finite decimal"))
            }
        }
    }
}

/// Convenience constructor: `dec!("123.45")`.
#[macro_export]
macro_rules! dec {
    ($s:literal) => {
        $crate::decimal::Decimal::parse($s).expect("invalid decimal literal")
    };
}
