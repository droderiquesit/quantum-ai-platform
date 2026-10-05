//! The API's four golden signals, recorded around every request (OBS-018).
//!
//! Until this wrapper the API recorded nothing about its own requests: the
//! only `metrics.count` calls in this crate were the OpenObserve drain's
//! post counters. Every series `/metrics` served described the platform
//! behind the API, so an API that was refusing every caller, or answering in
//! seconds, or at its connection limit, scraped exactly like a healthy one.
//!
//! It is the outermost handler, so what it times is what the caller waited
//! for the handler chain to answer — authorisation, the statement and fabric
//! refreshes, and the route. It records into the registry the platform
//! records into, so the series reach `/metrics` and the drain with no wiring
//! of their own.

use crate::http::{Handler, Request, Response, StreamDecision};
use qip_core::error::{Error, Result};
use qip_observability::golden::{GoldenSignals, Outcome};
use qip_observability::metrics::Metrics;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Wraps a handler and records latency, traffic, errors and saturation.
pub struct GoldenHandler {
    inner: Arc<dyn Handler>,
    signals: GoldenSignals,
    in_flight: AtomicU64,
    /// The server's `max_concurrent`: the denominator of saturation.
    capacity: usize,
}

impl std::fmt::Debug for GoldenHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoldenHandler")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl GoldenHandler {
    /// `capacity` is the connection limit the server is bound with. Zero is
    /// refused: a server that admits nothing has no share of capacity to
    /// report, and the gauge would be a division by zero.
    pub fn new(inner: Arc<dyn Handler>, metrics: Arc<Metrics>, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "configuration: the golden-signal wrapper needs the server's connection limit, \
                 and it was zero; pass ServerLimits::max_concurrent",
            ));
        }
        Ok(Self {
            inner,
            signals: GoldenSignals::new(metrics),
            in_flight: AtomicU64::new(0),
            capacity,
        })
    }

    /// Write the saturation gauge for `in_flight` requests.
    fn record_saturation(&self, in_flight: u64) {
        // u64 to f64: exact far past any connection limit a process could
        // hold. `capacity` was proven positive at construction, so the only
        // refusal `saturation` has left cannot occur and its result is unused.
        let _ = self
            .signals
            .saturation(in_flight as f64, self.capacity as f64);
    }
}

/// Counts one request in for as long as it lives, and out again when dropped
/// — including when the inner handler panics, so a panicking route cannot
/// leave the gauge reading one request fuller for the life of the process.
struct InFlight<'a>(&'a GoldenHandler);

impl<'a> InFlight<'a> {
    fn enter(handler: &'a GoldenHandler) -> Self {
        let now = handler.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
        handler.record_saturation(now);
        Self(handler)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let now = self
            .0
            .in_flight
            .fetch_sub(1, Ordering::Relaxed)
            .saturating_sub(1);
        self.0.record_saturation(now);
    }
}

fn millis_since(began: Instant) -> f64 {
    began.elapsed().as_secs_f64() * 1000.0
}

impl Handler for GoldenHandler {
    fn handle(&self, request: &Request) -> Response {
        let began = Instant::now();
        let _in_flight = InFlight::enter(self);
        let response = self.inner.handle(request);
        self.signals
            .finished(millis_since(began), Outcome::of_status(response.status));
        response
    }

    /// A refused stream is a request answered, and is recorded as one. An
    /// accepted stream is not: its duration is the client's patience rather
    /// than this service's latency, and timing it would put minutes-long
    /// observations in a histogram of request latency. `NotAStream` comes
    /// back through [`Self::handle`], which records it there, once.
    fn stream(&self, request: &Request) -> StreamDecision {
        let began = Instant::now();
        let decision = self.inner.stream(request);
        if let StreamDecision::Refused(response) = &decision {
            self.signals
                .finished(millis_since(began), Outcome::of_status(response.status));
        }
        decision
    }
}
