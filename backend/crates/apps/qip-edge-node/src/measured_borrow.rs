//! A borrow rate the cell measured, laid over the one the centre stated
//! (R12-MESH-5055, ADR 0681).
//!
//! The centre ships a stated `borrow_rate_annual` on each conversion and
//! holds no funding state. The cell does: `qip_edge::funding::FundingState`
//! carries the rate the venue last reported per instrument. A net edge priced
//! on the stated figure alone would keep charging a borrow the venue stopped
//! charging, or fail to charge one it raised.
//!
//! # Which figure wins
//!
//! The higher of the two. A stated rate is the operator's signed floor and a
//! measured rate is evidence the cost is at least that; taking the lower would
//! let a stale or missing measurement relax a signed cost, which is the
//! unrestrictive direction. A measured rate below the stated one therefore
//! changes nothing, and that is a choice, not a clamp: both inputs are valid.
//!
//! # What is refused
//!
//! A measured rate of one or more (100% a year), read as a fraction, is far
//! more likely a percentage figure than a real borrow cost; it is refused
//! rather than charged, because a net edge priced on a mislabelled unit
//! would reject every path. A negative measured rate (the holder is paid) is
//! not a borrow cost, and [`EdgeCosts`] refuses a negative one, so it leaves
//! the stated figure in place.

use qip_arbitrage::graph::EdgeCosts;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_edge::funding::FundingState;

/// `stated` with its borrow rate raised to the rate `funding` holds for
/// `instrument`, when that is higher. An instrument the funding state does
/// not know leaves `stated` as it was.
pub fn with_measured_borrow(
    stated: EdgeCosts,
    funding: &FundingState,
    instrument: &str,
) -> Result<EdgeCosts> {
    let Some(measured) = funding.funding_rate(instrument) else {
        return Ok(stated);
    };
    let rate = measured.rate();
    if rate >= Decimal::ONE {
        return Err(Error::invalid(format!(
            "the funding rate measured for {instrument} is {rate}, which as an annual fraction \
             is a borrow of 100% or more; it is probably stated as a percentage. Report it as \
             a fraction, for example 0.04 for 4%"
        )));
    }
    if rate <= stated.borrow_rate_annual {
        return Ok(stated);
    }
    Ok(EdgeCosts {
        borrow_rate_annual: rate,
        ..stated
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_edge::funding::FundingRate;

    fn stated(rate: Decimal) -> EdgeCosts {
        EdgeCosts {
            borrow_rate_annual: rate,
            ..EdgeCosts::default()
        }
    }

    fn funding(instrument: &str, rate: Decimal) -> FundingState {
        let mut state = FundingState::new();
        state.set_funding_rate(instrument.to_string(), FundingRate::new(rate));
        state
    }

    /// A measured rate above the stated one is the one charged; a lower one,
    /// a negative one and an unmeasured instrument leave the stated figure.
    /// Premise first: the two figures differ.
    ///
    /// Mutation: not run (nothing was compiled, owner instruction 2026-11-15).
    #[test]
    fn the_higher_of_the_stated_and_the_measured_borrow_is_charged() {
        let base = stated(dec!("0.04"));
        let raised = with_measured_borrow(base.clone(), &funding("ABC", dec!("0.09")), "ABC")
            .expect("a valid rate");
        assert_eq!(raised.borrow_rate_annual, dec!("0.09"));
        let lower = with_measured_borrow(base.clone(), &funding("ABC", dec!("0.01")), "ABC")
            .expect("a valid rate");
        assert_eq!(lower.borrow_rate_annual, dec!("0.04"));
        let paid = with_measured_borrow(base.clone(), &funding("ABC", dec!("-0.02")), "ABC")
            .expect("a valid rate");
        assert_eq!(paid.borrow_rate_annual, dec!("0.04"));
        let unknown = with_measured_borrow(base.clone(), &funding("ABC", dec!("0.09")), "XYZ")
            .expect("an unmeasured instrument is not an error");
        assert_eq!(unknown.borrow_rate_annual, dec!("0.04"));
    }

    /// A rate of 100% or more is refused, not charged and not clamped.
    ///
    /// Mutation: not run (nothing was compiled, owner instruction 2026-11-15).
    #[test]
    fn a_measured_rate_that_reads_as_a_percentage_is_refused() {
        let refusal = with_measured_borrow(stated(dec!("0.04")), &funding("ABC", dec!("4")), "ABC");
        assert!(refusal.is_err(), "a 400% borrow was charged");
    }
}
