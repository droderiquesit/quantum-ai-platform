//! `qip-observability` — traces, metrics, structured logs and SLOs.
//!
//! Semantics follow OpenTelemetry (trace/span ids, span kinds, attribute
//! naming, histogram buckets) so the exported JSON maps onto an OTLP collector
//! without translation, but the implementation is in-tree: the collector is a
//! deployment concern, and a platform that cannot report on itself when the
//! collector is unreachable is not observable.

pub mod logs;
pub mod metrics;
pub mod slo;
pub mod trace;

pub use logs::{LogRecord, Logger, Severity};
pub use metrics::{Histogram, MetricKind, MetricValue, Metrics, Snapshot};
pub use slo::{Slo, SloStatus, SloWindow};
pub use trace::{Span, SpanKind, SpanStatus, Tracer};

use std::sync::Arc;

/// The observability surface handed to every component.
#[derive(Clone, Debug)]
pub struct Telemetry {
    pub metrics: Arc<Metrics>,
    pub tracer: Arc<Tracer>,
    pub logger: Arc<Logger>,
}

impl Telemetry {
    pub fn new(service: impl Into<String>, clock: Arc<dyn qip_core::Clock>) -> Self {
        let service = service.into();
        Self {
            metrics: Arc::new(Metrics::new(service.clone())),
            tracer: Arc::new(Tracer::new(service.clone(), clock.clone())),
            logger: Arc::new(Logger::new(service, clock)),
        }
    }

    /// A telemetry surface that discards everything, for tests that do not
    /// assert on it.
    pub fn silent() -> Self {
        let clock: Arc<dyn qip_core::Clock> = Arc::new(qip_core::SystemClock);
        let telemetry = Self::new("test", clock);
        telemetry.logger.set_minimum_severity(Severity::Error);
        telemetry
    }
}
