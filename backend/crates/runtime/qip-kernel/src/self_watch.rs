//! The platform watching its own telemetry for level shifts (OBS-003).
//!
//! `qip_observability::aiops::detect_level_shift` was a library nothing
//! called. A detector nobody feeds reads as a platform that notices its own
//! regressions and is not one, so [`SelfWatch`] holds a bounded tail of each
//! series the cycle records about itself and runs the detector as each point
//! arrives.
//!
//! A series shorter than the detector's minimum is **not yet observable**, and
//! this returns `Ok(None)` for it. That is the one place "no anomaly" and "could
//! not look" share an answer, and it is bounded: [`MIN_POINTS`] points, after
//! which the detector either sees or refuses.
//!
//! One shift is reported once: a later detection whose window overlaps the last
//! reported one is the same step. The window that contains a step stays
//! containing it for `HALF` more points, and re-reporting it every cycle would
//! teach an operator that the alert repeats itself.

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_observability::aiops::{Anomaly, detect_level_shift};
use std::collections::{BTreeMap, VecDeque};

/// Samples either side of a candidate step.
const HALF: usize = 8;
/// Points before the detector can see anything.
pub const MIN_POINTS: usize = 2 * HALF;
/// Retained per series. Bounded: a process that ran for a year must not hold a
/// year of points to look for a shift that is visible in a few dozen.
const RETAINED: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct SelfWatch {
    series: BTreeMap<&'static str, VecDeque<(Timestamp, f64)>>,
    reported: BTreeMap<&'static str, Timestamp>,
}

impl SelfWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Points currently held for `series`.
    pub fn len(&self, series: &str) -> usize {
        self.series.get(series).map_or(0, VecDeque::len)
    }

    /// Record one point and return a shift that has not been reported before.
    pub fn observe(
        &mut self,
        series: &'static str,
        at: Timestamp,
        value: f64,
    ) -> Result<Option<Anomaly>> {
        // Refused before it is stored: a stored NaN would make the detector
        // refuse every later call on this series until it aged out.
        if !value.is_finite() {
            return Err(Error::numeric(
                "a non-finite sample reached the self-watch; drop it at the source",
            ));
        }
        let tail = self.series.entry(series).or_default();
        tail.push_back((at, value));
        while tail.len() > RETAINED {
            tail.pop_front();
        }
        if tail.len() < MIN_POINTS {
            return Ok(None);
        }
        let points: Vec<(Timestamp, f64)> = tail.iter().copied().collect();
        let Some(anomaly) = detect_level_shift(series, &points, HALF)? else {
            return Ok(None);
        };
        if self
            .reported
            .get(series)
            .is_some_and(|end| anomaly.window_start <= *end)
        {
            return Ok(None);
        }
        self.reported.insert(series, anomaly.window_end);
        Ok(Some(anomaly))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(i: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + i)
    }

    #[test]
    fn a_step_is_reported_once_and_a_flat_series_never() {
        let mut watch = SelfWatch::new();
        for i in 0..MIN_POINTS as i64 {
            assert_eq!(
                watch.observe("s", at(i), 10.0 + (i % 2) as f64).ok(),
                Some(None)
            );
        }
        let mut hits = 0;
        for i in 16..40 {
            if watch
                .observe("s", at(i), 40.0 + (i % 2) as f64)
                .ok()
                .flatten()
                .is_some()
            {
                hits += 1;
            }
        }
        assert_eq!(hits, 1, "one step, one report");
    }

    #[test]
    fn a_non_finite_sample_is_refused_and_not_stored() {
        let mut watch = SelfWatch::new();
        assert!(watch.observe("s", at(0), f64::NAN).is_err());
        assert_eq!(watch.len("s"), 0);
    }
}
