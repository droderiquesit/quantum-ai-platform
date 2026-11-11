//! The limit in force: the operator's ceiling narrowed by a risk tolerance
//! (ADR 0405).
//!
//! The limit set a composition root reads at boot is a *ceiling*. The desk
//! mandate states a risk tolerance, a fraction in `[0, 1]`, and the limit an
//! order is actually checked against is the ceiling multiplied by it. Before
//! this module the tolerance bounded how much capital a user could put at one
//! strategy and nothing else, so a desk that stated a tolerance of one half
//! was checked against the same bounds as a desk that stated one.
//!
//! Three properties hold here, in code and not in a comment:
//!
//! - **Narrowing only.** A maximum is multiplied by a fraction no greater
//!   than one and the result is compared against the ceiling before it is
//!   returned. A minimum is not touched: a floor multiplied by a fraction
//!   would be a *looser* floor.
//! - **Refused, not clamped.** A tolerance outside `[0, 1]` is an error, and
//!   so is a ceiling the multiplication could loosen.
//! - **One is the ceiling.** At a tolerance of one the result equals the
//!   input, with no mark on it.
//!
//! This is plain arithmetic on two numbers a person wrote down. It asks no
//! model and reads no state.

use crate::limits::{Limit, LimitKind, LimitSet, Narrowing};
use qip_core::Decimal;
use qip_core::error::{Error, Result};

/// The tolerance as the `f64` the statistic-bounded kinds are multiplied by.
///
/// **This is where the tolerance crosses from `Decimal` to `f64`.** It is
/// held as a `Decimal` because the ledger multiplies money by it; every
/// limit but the two notional ones is a ratio or a statistic carried as
/// `f64`, so the crossing is into the statistics lane and the product is a
/// bound an observation is compared against, never a figure money is
/// computed from. The two notional kinds do not use this value at all.
///
/// The range is checked on both sides of the crossing. The `Decimal` check
/// is the one a caller can trip. The `f64` check exists because the
/// multiplication below trusts the fraction completely: `limit * NaN` is
/// `NaN`, which [`Limit`]'s assessment reports as uncomparable on every
/// order, and `limit * 1.0000001` is a bound above its ceiling.
fn fraction(tolerance: Decimal) -> Result<f64> {
    if tolerance.is_negative() || tolerance > Decimal::ONE {
        return Err(Error::invalid(format!(
            "a risk tolerance must be a fraction in [0, 1], not {tolerance}; correct the \
             risk_tolerance of the desk mandate (if this was a percentage, divide by 100) — a \
             limit is not narrowed by a figure outside that range and the figure is not clamped \
             into it"
        )));
    }
    after_crossing(tolerance.to_f64(), tolerance)
}

/// The second half of [`fraction`]: the range, asked again of the `f64`.
fn after_crossing(crossed: f64, tolerance: Decimal) -> Result<f64> {
    if !crossed.is_finite() || !(0.0..=1.0).contains(&crossed) {
        return Err(Error::numeric(format!(
            "the risk tolerance {tolerance} became {crossed} when carried as the fraction a \
             limit is multiplied by, which is not a finite number in [0, 1]; state the \
             tolerance to at most nine decimal places between 0 and 1"
        )));
    }
    Ok(crossed)
}

impl LimitKind {
    /// The same kind with its bound multiplied by `tolerance`, when it is a
    /// maximum; the same kind unchanged when it is a minimum.
    ///
    /// Unlike [`Self::with_bound`] this admits a result of zero. A
    /// recalibration to zero is nobody's intent, but a tolerance of zero is a
    /// desk saying it will carry no risk, and a maximum of zero is how the
    /// limit set says the same thing: every order that leaves a positive
    /// figure under it is refused.
    ///
    /// Refuses a maximum whose ceiling is negative or not finite. Multiplying
    /// a negative bound by a fraction moves it toward zero, which for a
    /// maximum is looser, and a bound that is not finite is one
    /// [`Limit`]'s assessment already refuses to compare against.
    ///
    /// The match is exhaustive with no wildcard, so a new kind cannot be
    /// added without someone deciding whether a tolerance narrows it.
    pub fn narrowed_by(&self, tolerance: Decimal) -> Result<Self> {
        let fraction = fraction(tolerance)?;
        let scale = |ceiling: f64| -> Result<f64> {
            if !ceiling.is_finite() || ceiling < 0.0 {
                return Err(self.unnarrowable(&ceiling.to_string()));
            }
            Ok(ceiling * fraction)
        };
        // The two notional kinds stay in `Decimal` end to end: ceiling and
        // tolerance are both `Decimal`, so nothing crosses.
        let money = |ceiling: Decimal| -> Result<Decimal> {
            if ceiling.is_negative() {
                return Err(self.unnarrowable(&ceiling.to_string()));
            }
            ceiling.checked_mul(tolerance).ok_or_else(|| {
                Error::numeric(format!(
                    "the {} ceiling of {ceiling} could not be multiplied by a risk tolerance \
                     of {tolerance} without overflowing; lower the ceiling in the limits file",
                    self.label()
                ))
            })
        };
        Ok(match self {
            Self::MinLiquidity { .. }
            | Self::MinCashBuffer { .. }
            | Self::MinFundingBuffer { .. } => self.clone(),
            Self::MaxOrderNotional { limit } => Self::MaxOrderNotional {
                limit: money(*limit)?,
            },
            Self::MaxPositionNotional { limit } => Self::MaxPositionNotional {
                limit: money(*limit)?,
            },
            Self::MaxPositionWeight { limit } => Self::MaxPositionWeight {
                limit: scale(*limit)?,
            },
            Self::MaxLeverage { limit } => Self::MaxLeverage {
                limit: scale(*limit)?,
            },
            Self::MaxNetExposure { limit } => Self::MaxNetExposure {
                limit: scale(*limit)?,
            },
            Self::MaxConcentration { axis, limit } => Self::MaxConcentration {
                axis: axis.clone(),
                limit: scale(*limit)?,
            },
            Self::MaxAxisWeight { axis, limit } => Self::MaxAxisWeight {
                axis: axis.clone(),
                limit: scale(*limit)?,
            },
            Self::MaxBucketExposure {
                axis,
                bucket,
                limit,
            } => Self::MaxBucketExposure {
                axis: axis.clone(),
                bucket: bucket.clone(),
                limit: scale(*limit)?,
            },
            Self::MaxVolatility { limit } => Self::MaxVolatility {
                limit: scale(*limit)?,
            },
            Self::MaxValueAtRisk { confidence, limit } => Self::MaxValueAtRisk {
                confidence: *confidence,
                limit: scale(*limit)?,
            },
            Self::MaxExpectedShortfall { confidence, limit } => Self::MaxExpectedShortfall {
                confidence: *confidence,
                limit: scale(*limit)?,
            },
            Self::MaxDrawdown { limit } => Self::MaxDrawdown {
                limit: scale(*limit)?,
            },
            Self::MaxDailyLoss { limit } => Self::MaxDailyLoss {
                limit: scale(*limit)?,
            },
            Self::MaxDaysToLiquidate { limit } => Self::MaxDaysToLiquidate {
                limit: scale(*limit)?,
            },
            Self::MaxCounterpartyExposure { limit } => Self::MaxCounterpartyExposure {
                limit: scale(*limit)?,
            },
            Self::MaxVenueExposure { venue, limit } => Self::MaxVenueExposure {
                venue: venue.clone(),
                limit: scale(*limit)?,
            },
            Self::MaxVenueNotionalRate { venue, limit } => Self::MaxVenueNotionalRate {
                venue: venue.clone(),
                limit: scale(*limit)?,
            },
            Self::MaxSurvivalGross { limit } => Self::MaxSurvivalGross {
                limit: scale(*limit)?,
            },
            Self::MaxMarginUtilisation { limit } => Self::MaxMarginUtilisation {
                limit: scale(*limit)?,
            },
        })
    }

    fn unnarrowable(&self, ceiling: &str) -> Error {
        Error::invalid(format!(
            "the {} ceiling of {ceiling} cannot be narrowed by a risk tolerance: a maximum must \
             be a finite number that is not negative, because a fraction of a negative bound is \
             a looser bound; correct the bound in the limits file",
            self.label()
        ))
    }
}

impl Limit {
    /// This limit as it is in force under `tolerance`.
    ///
    /// At a tolerance of exactly one, and for every minimum, the result is
    /// this limit unchanged and unmarked: the limit in force *is* the
    /// operator's. Otherwise the bound is the ceiling multiplied by the
    /// tolerance and the limit carries a [`Narrowing`] saying so.
    ///
    /// The warning threshold and the critical multiple are not changed. Each
    /// is a fraction or a multiple of the bound the limit is checked at, so
    /// the whole severity ladder moves with the limit in force; had it stayed
    /// on the ceiling, an order past the limit in force would have been
    /// reported as a warning and sent.
    ///
    /// Refuses a limit that is already narrowed. Narrowing is always from the
    /// operator's ceiling; applied twice it would square the tolerance and
    /// record the first result as though the operator had written it.
    pub fn narrowed_by(&self, tolerance: Decimal) -> Result<Self> {
        if let Some(narrowing) = &self.narrowing {
            return Err(Error::invalid(format!(
                "the limit {} is already narrowed from a ceiling of {} by a tolerance of {}; \
                 narrow the operator's ceiling set, not a set that is already in force",
                self.name, narrowing.ceiling, narrowing.tolerance
            )));
        }
        let kind = self.kind.narrowed_by(tolerance)?;
        if self.kind.is_minimum() || tolerance == Decimal::ONE {
            return Ok(self.clone());
        }
        let ceiling = self.kind.bound();
        let in_force = kind.bound();
        // Checked on the result rather than argued from the inputs: this is
        // the one property the whole change exists to hold, and a future
        // kind, or a change to how a product rounds, must fail here and not
        // at a venue.
        if in_force > ceiling {
            return Err(Error::numeric(format!(
                "narrowing the limit {} by a tolerance of {tolerance} produced {in_force}, \
                 which is above its ceiling of {ceiling}; nothing may raise a limit above the \
                 operator's bound, so the limit set is refused rather than used",
                self.name
            )));
        }
        Ok(Self {
            kind,
            narrowing: Some(Narrowing { ceiling, tolerance }),
            ..self.clone()
        })
    }
}

impl LimitSet {
    /// The limit set in force under `tolerance`: every maximum multiplied by
    /// the fraction, every minimum where the operator put it.
    ///
    /// `self` is the ceiling and is not changed. At a tolerance of one the
    /// result equals `self`. The name is kept, because rules are looked up
    /// and journalled by name and the rule is the same rule at a tighter
    /// bound.
    pub fn narrowed_by(&self, tolerance: Decimal) -> Result<Self> {
        Ok(Self {
            name: self.name.clone(),
            limits: self
                .limits
                .iter()
                .map(|limit| limit.narrowed_by(tolerance))
                .collect::<Result<Vec<_>>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Decimal` cannot hold a value that is not finite, so the second range
    /// check is unreachable from [`fraction`] today. It is tested directly
    /// because it is the only thing between a changed `to_f64` and a bound of
    /// `NaN` on every maximum, which would refuse nothing by comparison and
    /// everything as uncomparable.
    #[test]
    fn a_tolerance_that_is_not_a_finite_fraction_after_the_crossing_is_refused() {
        for crossed in [f64::NAN, f64::INFINITY, -0.000_001, 1.000_001] {
            let error = after_crossing(crossed, Decimal::ONE)
                .expect_err("a fraction outside [0, 1] must not be multiplied into a limit");
            assert!(
                error.message().contains("not a finite number in [0, 1]"),
                "{crossed}: {}",
                error.message()
            );
        }
        // The premise: the same function admits the two ends of the range.
        assert_eq!(after_crossing(0.0, Decimal::ZERO).ok(), Some(0.0));
        assert_eq!(after_crossing(1.0, Decimal::ONE).ok(), Some(1.0));
    }
}
