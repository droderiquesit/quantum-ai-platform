//! The four golden signals — latency, traffic, errors, saturation — in one
//! shape for every service (OBS-018).
//!
//! Each binary already had rich series about its own domain and none of them
//! lined up: the brains had a cycle histogram and a problem counter under
//! their own names, the API had no request series at all, and nothing
//! anywhere measured how full a service was. An operator asking "which
//! service is slow, failing or full" had four different questions to learn.
//! This recorder is the same four names on every one, fed with whatever that
//! service's unit of work is — a request for the API, a cycle for a brain, a
//! pass for a cell.
//!
//! It holds an `Arc<Metrics>` it is given and performs no I/O: every call is
//! a write to an in-memory registry behind a mutex, the same cost as any
//! other recording site.

use crate::metrics::{Labels, Metrics, labels, names};
use qip_core::error::{Error, Result};
use std::sync::Arc;

/// How one unit of work ended.
///
/// Three values and not a boolean, because "the caller asked for something
/// it may not have" and "the service broke" page different people, and a
/// single error count that mixes them teaches an operator to ignore it the
/// first time a scanner walks the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Completed as asked.
    Served,
    /// Refused or not found: the caller's request was the problem.
    CallerError,
    /// The service failed to do what it was validly asked.
    Failed,
}

impl Outcome {
    /// The outcome an HTTP status reports: `5xx` failed, `4xx` is the
    /// caller's, everything else was served.
    pub const fn of_status(status: u16) -> Self {
        match status {
            500..=599 => Self::Failed,
            400..=499 => Self::CallerError,
            _ => Self::Served,
        }
    }

    /// The `class` label an error is counted under, or `None` when the work
    /// was served and no error is counted. A fixed pair of literals, so the
    /// label is bounded by this enum.
    const fn error_class(self) -> Option<&'static str> {
        match self {
            Self::Served => None,
            Self::CallerError => Some("caller"),
            Self::Failed => Some("service"),
        }
    }
}

/// Records the four golden signals for one service.
#[derive(Clone, Debug)]
pub struct GoldenSignals {
    metrics: Arc<Metrics>,
}

impl GoldenSignals {
    /// A recorder on `metrics`, with all four series described.
    ///
    /// Describing creates nothing: a service that has served no work exports
    /// none of the four, which is the honest reading of a process that has
    /// done nothing yet.
    pub fn new(metrics: Arc<Metrics>) -> Self {
        metrics.describe(
            names::SERVICE_REQUESTS,
            "units of work this service finished: requests, cycles or passes (traffic)",
        );
        metrics.describe(
            names::SERVICE_ERRORS,
            "units of work that did not complete as asked, by class: caller or service (errors)",
        );
        metrics.describe(
            names::SERVICE_LATENCY_MS,
            "milliseconds one unit of work took, measured on a monotonic clock (latency)",
        );
        metrics.describe(
            names::SERVICE_SATURATION,
            "the share of this service's capacity in use; above one means it is over capacity \
             (saturation)",
        );
        Self { metrics }
    }

    /// One unit of work finished: counts it, times it, and counts an error
    /// when it did not complete as asked.
    pub fn finished(&self, millis: f64, outcome: Outcome) {
        self.metrics.count(names::SERVICE_REQUESTS, Labels::new());
        self.metrics
            .observe_latency_ms(names::SERVICE_LATENCY_MS, Labels::new(), millis);
        if let Some(class) = outcome.error_class() {
            self.metrics
                .count(names::SERVICE_ERRORS, labels([("class", class)]));
        }
    }

    /// How full the service is: `used` of `capacity`, as a ratio.
    ///
    /// Not clamped at one. A cycle that took 150 ms of a 100 ms interval is
    /// 1.5, and reporting that as 1.0 would hide exactly the overrun the
    /// gauge exists to show. What is refused is a capacity that is not
    /// positive or a figure that is not finite: a ratio over zero is not a
    /// measurement, and writing it would chart `inf` or `NaN` as a reading.
    pub fn saturation(&self, used: f64, capacity: f64) -> Result<()> {
        if !used.is_finite() || !capacity.is_finite() || capacity <= 0.0 || used < 0.0 {
            return Err(Error::invalid(format!(
                "saturation needs a non-negative finite use and a positive finite capacity, got \
                 {used} of {capacity}; pass the service's real capacity"
            )));
        }
        self.metrics
            .gauge(names::SERVICE_SATURATION, Labels::new(), used / capacity);
        Ok(())
    }
}
