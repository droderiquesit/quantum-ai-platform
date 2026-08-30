//! Exact wide-integer arithmetic for pool maths.
//!
//! A constant-product quote is `reserve_out * amount_in / (reserve_in +
//! amount_in)`. At realistic pool sizes the numerator overflows 128 bits long
//! before either operand does, and evaluating it in `f64` gives an answer that
//! disagrees with the chain in the last few digits — which is precisely where
//! the arbitrage either exists or does not. Every product here is therefore
//! carried at full 256-bit width and divided back down, so the result is the
//! same integer the contract would compute.

const MASK64: u128 = u64::MAX as u128;

/// The 256-bit product of two 128-bit values, as `(high, low)`.
pub fn wide_mul(a: u128, b: u128) -> (u128, u128) {
    let (a_hi, a_lo) = (a >> 64, a & MASK64);
    let (b_hi, b_lo) = (b >> 64, b & MASK64);

    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;

    let mid = (ll >> 64) + (lh & MASK64) + (hl & MASK64);
    let lo = (ll & MASK64) | (mid << 64);
    let hi = hh + (lh >> 64) + (hl >> 64) + (mid >> 64);
    (hi, lo)
}

/// Add a 128-bit value to a 256-bit one, reporting overflow rather than wrapping.
fn wide_add(hi: u128, lo: u128, addend: u128) -> Option<(u128, u128)> {
    let (sum, carried) = lo.overflowing_add(addend);
    if carried {
        Some((hi.checked_add(1)?, sum))
    } else {
        Some((hi, sum))
    }
}

/// Divide a 256-bit value by a 128-bit divisor, returning `(quotient, remainder)`.
///
/// `None` when the divisor is zero or the quotient does not fit in 128 bits.
/// Both are caller errors — a saturated quotient here would silently become a
/// price, so the failure is returned instead of approximated.
pub fn wide_div(hi: u128, lo: u128, divisor: u128) -> Option<(u128, u128)> {
    if divisor == 0 || hi >= divisor {
        return None;
    }
    let mut remainder = hi;
    let mut quotient = 0u128;
    for bit in (0..128).rev() {
        // The shifted remainder can exceed 128 bits; because `remainder` is
        // always below `divisor` beforehand, the doubled value is below `2 *
        // divisor` and the subtraction brings it back into range.
        let overflowed = remainder >> 127 == 1;
        remainder = (remainder << 1) | ((lo >> bit) & 1);
        if overflowed || remainder >= divisor {
            remainder = remainder.wrapping_sub(divisor);
            quotient |= 1 << bit;
        }
    }
    Some((quotient, remainder))
}

/// `floor(a * b / divisor)`, exact at 256-bit intermediate width.
pub fn mul_div_floor(a: u128, b: u128, divisor: u128) -> Option<u128> {
    let (hi, lo) = wide_mul(a, b);
    wide_div(hi, lo, divisor).map(|(quotient, _)| quotient)
}

/// `ceil(a * b / divisor)`, exact at 256-bit intermediate width.
///
/// The rounding direction is not cosmetic: an input requirement rounded down
/// is an input that does not buy the output it was computed for.
pub fn mul_div_ceil(a: u128, b: u128, divisor: u128) -> Option<u128> {
    let (hi, lo) = wide_mul(a, b);
    let (quotient, remainder) = wide_div(hi, lo, divisor)?;
    if remainder == 0 {
        Some(quotient)
    } else {
        quotient.checked_add(1)
    }
}

/// `floor((a * b + addend) / divisor)`, exact at 256-bit intermediate width.
pub fn mul_add_div_floor(a: u128, b: u128, addend: u128, divisor: u128) -> Option<u128> {
    let (hi, lo) = wide_mul(a, b);
    let (hi, lo) = wide_add(hi, lo, addend)?;
    wide_div(hi, lo, divisor).map(|(quotient, _)| quotient)
}

/// Compare `a * b` with `c * d` without losing a bit to overflow.
///
/// The constant-product invariant is checked this way rather than by dividing:
/// a division would round, and the whole point of the check is that nothing
/// rounds in the pool's favour by accident.
pub fn cmp_products(a: u128, b: u128, c: u128, d: u128) -> std::cmp::Ordering {
    wide_mul(a, b).cmp(&wide_mul(c, d))
}
