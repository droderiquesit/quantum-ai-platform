//! The expected absolute move of a price over an exposure window.
//!
//! One formula, held here so that two pricers can share it without sharing a
//! crate. The desk's net-edge calculator (`qip-arbitrage`) charges a cycle
//! the move it is exposed to over its venue round trip plus any cross-region
//! coordination; the peer saga (`qip-peer-link`) charges a crossing the move
//! over the round trip the wire measured to the peer. The saga may not depend
//! on the desk's crate: `qip-arbitrage` reaches the durable fabric through the
//! contracts it shares, and the acceptance suite
//! (`reflex_mesh_traffic_and_durable_fabric_traffic_share_no_crate_in_either_direction`)
//! keeps the mesh and the fabric apart so a leg's latency is never paid for a
//! durability guarantee. A second copy of the formula in the saga would be
//! the model drifting from itself, and this module is the alternative.
//!
//! Statistics are `f64` here by design (`.claude/rules/domains/core-rust.md`);
//! the caller crosses into `Decimal` where it charges money, and says so.

use crate::time::Duration;

/// Expectation of the absolute value of a standard normal, `sqrt(2/pi)`.
///
/// The factor that turns a volatility into an expected move. Without it the
/// deduction is a one-standard-deviation figure, which overstates the typical
/// case by about a quarter and makes every path look worse than it is.
pub const EXPECTED_ABSOLUTE_NORMAL: f64 = 0.797_884_560_802_865_4;

/// The expected absolute move, as a fraction of price, of an instrument with
/// annualised volatility `volatility_annual_f64` left exposed for `exposed`,
/// under a driftless normal diffusion: `volatility * sqrt(years) * sqrt(2/pi)`.
///
/// A negative volatility or a negative window is read as zero rather than
/// refused: both are a caller's arithmetic on a measurement, and a deduction
/// of zero is the conservative reading of "nothing was measured". The result
/// is never negative.
pub fn expected_absolute_move_fraction(volatility_annual_f64: f64, exposed: Duration) -> f64 {
    let years_f64 = exposed.as_years_f64().max(0.0);
    volatility_annual_f64.max(0.0) * years_f64.sqrt() * EXPECTED_ABSOLUTE_NORMAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_longer_exposure_at_the_same_volatility_is_a_larger_move_and_zero_exposure_is_none() {
        let short = expected_absolute_move_fraction(0.40, Duration::from_millis(250));
        let long = expected_absolute_move_fraction(0.40, Duration::from_millis(1000));
        assert!(short > 0.0);
        assert!(long > short, "{long} is not above {short}");
        assert!(
            expected_absolute_move_fraction(0.40, Duration::from_nanos(0)).abs() < f64::EPSILON
        );
    }

    #[test]
    fn a_negative_volatility_or_window_prices_as_nothing_rather_than_as_a_credit() {
        assert!(
            expected_absolute_move_fraction(-0.40, Duration::from_millis(250)).abs() < f64::EPSILON
        );
        assert!(
            expected_absolute_move_fraction(0.40, Duration::from_millis(-250)).abs() < f64::EPSILON
        );
    }

    #[test]
    fn the_move_is_the_volatility_scaled_by_the_square_root_of_the_years_and_the_normal_factor() {
        // One year at 40%: 0.40 * 1 * sqrt(2/pi).
        let year = Duration::from_secs(365 * 24 * 60 * 60);
        let expected = 0.40 * EXPECTED_ABSOLUTE_NORMAL;
        let actual = expected_absolute_move_fraction(0.40, year);
        assert!((actual - expected).abs() < 1e-12, "{actual} vs {expected}");
    }
}
