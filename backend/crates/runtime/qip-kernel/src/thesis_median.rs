//! The median return a thesis is sized on, refused rather than clamped
//! (R12-WORLD-10187, ADR 0612).
//!
//! `Platform::thesis_from` used to take `(expected - observed).clamp(-0.5, 0.5)`.
//! An extreme print then became a *finite, plausible* median: a displacement
//! of 3.0 was recorded as 0.5, the forecast's lineage said the model had
//! argued for 0.5, and nobody could tell from the record that the measurement
//! was out of range. The workspace principle is refuse, do not clamp: a value
//! silently corrected is a caller bug that survives. Refusing is also the
//! restrictive direction, because a thesis that is not built is not sized and
//! commits no capital.

use qip_core::error::{Error, Result};

/// The widest magnitude of reversion, as a fraction of price, a thesis may
/// state. Provisional: it is the bound the old clamp used (mandate-scale
/// range construction validates), kept unchanged so no thesis that was
/// admissible before is refused now and none that was clamped is admitted.
pub(crate) const MAX_REVERSION_MEDIAN: f64 = 0.5;

/// The reversion an anomaly implies: expectation minus observation.
///
/// Refused, naming the figures, when either input is not finite or the
/// difference lies outside `[-MAX_REVERSION_MEDIAN, MAX_REVERSION_MEDIAN]`.
pub(crate) fn reversion_median(expected: f64, observed: f64) -> Result<f64> {
    let median = expected - observed;
    if !median.is_finite() {
        return Err(Error::invalid(format!(
            "the anomaly's expected {expected} and observed {observed} give no finite reversion; \
             the thesis is not sized"
        )));
    }
    if median.abs() > MAX_REVERSION_MEDIAN {
        return Err(Error::invalid(format!(
            "the anomaly's reversion {median} (expected {expected}, observed {observed}) is \
             outside +/-{MAX_REVERSION_MEDIAN}; it is a bad print or a broken expectation, not a \
             forecast, and is refused rather than clamped"
        )));
    }
    Ok(median)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reversion_inside_the_bound_is_passed_through_unchanged() {
        assert_eq!(reversion_median(0.30, 0.10).ok(), Some(0.30 - 0.10));
        assert_eq!(reversion_median(0.10, 0.30).ok(), Some(0.10 - 0.30));
        // The bound itself is admitted: refusing it would be tighter than the
        // clamp it replaces, which admitted exactly 0.5.
        assert_eq!(reversion_median(0.5, 0.0).ok(), Some(0.5));
    }

    #[test]
    fn a_reversion_beyond_the_bound_is_refused_in_both_directions_not_clamped() {
        // Premise: the old clamp would have returned a value here.
        assert!((3.0_f64 - 0.0).clamp(-0.5, 0.5).abs() > 0.0);
        let high = reversion_median(3.0, 0.0);
        assert!(
            high.is_err(),
            "an extreme positive print was admitted: {high:?}"
        );
        let low = reversion_median(0.0, 3.0);
        assert!(
            low.is_err(),
            "an extreme negative print was admitted: {low:?}"
        );
        assert!(reversion_median(0.51, 0.0).is_err());
    }

    #[test]
    fn a_non_finite_measurement_is_refused() {
        assert!(reversion_median(f64::NAN, 0.0).is_err());
        assert!(reversion_median(0.0, f64::INFINITY).is_err());
    }
}
