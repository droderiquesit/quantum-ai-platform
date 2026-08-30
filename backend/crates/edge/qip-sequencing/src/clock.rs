//! Clock discipline: what the venue's timestamps mean in this cell's time.
//!
//! Every message carries a venue time and a capture time, and the difference
//! between them is not a latency measurement — it is a latency *plus* whatever
//! the two clocks disagree by. Separating those is what this type does, and how
//! well it can be done depends entirely on what the feed gives you.
//!
//! **With a one-way feed the offset and the path delay are not separable.** PTP
//! separates them by timing a round trip; a market data multicast offers no
//! round trip, so what is estimated here is `offset + one-way path delay`. The
//! estimator is the classical one: over a window of observations, the smallest
//! `capture - venue` is the one that queued least, so it is the closest to the
//! true constant term. A [`ClockDiscipline::with_path_delay`] hint subtracts a
//! separately measured propagation delay where one is known; without it, the
//! estimate is a consistent relative correction rather than an absolute one, and
//! the documentation says so rather than the code pretending otherwise.
//!
//! **The uncertainty is published, not hidden.** The spread of the observations
//! bounds how well the minimum can be trusted, because any of them could have
//! been the least-delayed. Below a configured sample count or above a configured
//! spread the estimate is marked untrustworthy, and an untrustworthy estimate is
//! never applied — an unreliable correction is worse than no correction, because
//! it is invisible.
//!
//! **A correction never moves a timestamp backwards.** Time going backwards
//! breaks every ordering assumption downstream — event logs, bitemporal reads,
//! watermarks — and it does so quietly. So [`ClockDiscipline::discipline`] clamps
//! against the last value it returned and counts how often it had to.

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// One observation: what the venue said, and when this cell saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockObservation {
    /// What the venue said the time was.
    pub venue_time: Timestamp,
    /// When this cell's hardware saw the message.
    pub capture_time: Timestamp,
}

impl ClockObservation {
    /// One observation of the two clocks.
    pub fn new(venue_time: Timestamp, capture_time: Timestamp) -> Self {
        Self {
            venue_time,
            capture_time,
        }
    }

    /// `capture - venue`: the offset, the path delay and the queuing, together.
    pub fn raw(&self) -> Duration {
        self.capture_time.since(self.venue_time)
    }
}

/// The current estimate and how much of it to believe.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockEstimate {
    /// Best estimate of how far this cell's clock runs ahead of the venue's,
    /// less any configured path delay.
    pub offset: Duration,
    /// How fast the offset is changing, in nanoseconds per second. A statistic,
    /// hence `f64`.
    pub drift_nanos_per_sec_f64: f64,
    /// How tightly the observations bound the offset, in nanoseconds.
    pub uncertainty_nanos_f64: f64,
    /// Observations the estimate was drawn from.
    pub samples: usize,
    /// Whether the estimate clears both the sample-count and spread thresholds.
    pub trustworthy: bool,
}

impl ClockEstimate {
    /// Where a venue timestamp falls in this cell's clock, before any clamping.
    pub fn corrected(&self, venue_time: Timestamp) -> Timestamp {
        venue_time.saturating_add(self.offset)
    }
}

/// Estimates the offset between a venue's clock and this cell's.
#[derive(Debug)]
pub struct ClockDiscipline {
    window: usize,
    observations: VecDeque<ClockObservation>,
    min_samples: usize,
    max_uncertainty_nanos_f64: f64,
    path_delay: Duration,
    /// The last value [`ClockDiscipline::discipline`] returned, so it can never
    /// return anything earlier.
    last_emitted: Option<Timestamp>,
    clamped: u64,
}

impl ClockDiscipline {
    /// `window` observations, `min_samples` before any estimate is trusted, and
    /// a spread above `max_uncertainty` marking the estimate unusable.
    pub fn new(window: usize, min_samples: usize, max_uncertainty: Duration) -> Result<Self> {
        if window == 0 || min_samples == 0 {
            return Err(qip_core::error::Error::invalid(
                "clock discipline needs a non-zero window and sample threshold",
            ));
        }
        Ok(Self {
            window,
            observations: VecDeque::new(),
            min_samples: min_samples.min(window),
            max_uncertainty_nanos_f64: max_uncertainty.as_nanos() as f64,
            path_delay: Duration::ZERO,
            last_emitted: None,
            clamped: 0,
        })
    }

    /// A separately measured one-way propagation delay, subtracted from the
    /// estimate so that what remains is closer to a true clock offset.
    pub fn with_path_delay(mut self, path_delay: Duration) -> Self {
        self.path_delay = path_delay;
        self
    }

    /// How many times a correction would have moved a timestamp backwards.
    ///
    /// Non-zero means the estimate is moving around more than the message rate
    /// tolerates, which is a reason to widen the window rather than to ignore it.
    pub fn clamped(&self) -> u64 {
        self.clamped
    }

    /// Observations currently in the window.
    pub fn samples(&self) -> usize {
        self.observations.len()
    }

    /// Record an observation, discarding the oldest once the window is full.
    pub fn observe(&mut self, observation: ClockObservation) {
        if self.observations.len() == self.window {
            self.observations.pop_front();
        }
        self.observations.push_back(observation);
    }

    /// The current estimate, or `None` before any observation.
    pub fn estimate(&self) -> Option<ClockEstimate> {
        if self.observations.is_empty() {
            return None;
        }
        let raw_nanos: Vec<f64> = self
            .observations
            .iter()
            .map(|observation| observation.raw().as_nanos() as f64)
            .collect();
        let minimum = raw_nanos.iter().copied().fold(f64::INFINITY, f64::min);

        // The spread between the quartiles: robust to one absurd sample, and it
        // is the honest bound on where the true constant term sits, since any
        // observation could have been the least delayed one.
        let uncertainty_nanos_f64 =
            (stats::quantile(&raw_nanos, 0.75) - stats::quantile(&raw_nanos, 0.25)).max(0.0);

        // Drift needs at least three points for a fit with an intercept; below
        // that it is reported as zero rather than as a number invented from two
        // observations.
        let drift_nanos_per_sec_f64 = if self.observations.len() >= 3 {
            // Seconds since the window's first observation, not since the epoch.
            // A regressor of 1.7e9 with a spread of a few seconds loses the
            // spread to rounding, and the fitted slope comes back as noise.
            let base = self
                .observations
                .front()
                .map_or(0, |observation| observation.capture_time.as_nanos());
            let seconds: Vec<f64> = self
                .observations
                .iter()
                .map(|observation| (observation.capture_time.as_nanos() - base) as f64 / 1e9)
                .collect();
            stats::linear_fit(&seconds, &raw_nanos)
                .ok()
                .and_then(|fit| fit.coefficients.get(1).copied())
                .unwrap_or(0.0)
        } else {
            0.0
        };

        let offset_nanos = minimum - self.path_delay.as_nanos() as f64;
        let samples = self.observations.len();
        Some(ClockEstimate {
            offset: Duration::from_nanos(offset_nanos as i64),
            drift_nanos_per_sec_f64,
            uncertainty_nanos_f64,
            samples,
            trustworthy: samples >= self.min_samples
                && uncertainty_nanos_f64 <= self.max_uncertainty_nanos_f64,
        })
    }

    /// Map a venue timestamp into this cell's clock.
    ///
    /// Returns the input unchanged when the estimate is not trustworthy: an
    /// unreliable correction silently displaces every downstream ordering, and
    /// no correction at least leaves the error where an operator can see it.
    /// The result is never earlier than the previous result, whatever the
    /// estimate says.
    pub fn discipline(&mut self, venue_time: Timestamp) -> Timestamp {
        let corrected = match self.estimate() {
            Some(estimate) if estimate.trustworthy => estimate.corrected(venue_time),
            _ => venue_time,
        };
        let emitted = match self.last_emitted {
            Some(previous) if corrected < previous => {
                self.clamped += 1;
                previous
            }
            _ => corrected,
        };
        self.last_emitted = Some(emitted);
        emitted
    }

    /// Forget the history, after a venue's clock has been stepped.
    ///
    /// A step is a discontinuity, not drift, and averaging across one produces an
    /// estimate that describes neither side of it. The monotonic floor survives
    /// the reset deliberately: the reason for the step does not make it
    /// acceptable for time to run backwards downstream.
    pub fn reset_history(&mut self) {
        self.observations.clear();
    }
}
