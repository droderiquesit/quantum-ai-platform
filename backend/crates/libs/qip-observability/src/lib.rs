//! `qip-observability` — traces, metrics, structured logs and SLOs.
//!
//! Semantics follow OpenTelemetry (trace/span ids, span kinds, attribute
//! naming, histogram buckets) so the exported JSON maps onto an OTLP collector
//! without translation, but the implementation is in-tree: the collector is a
//! deployment concern, and a platform that cannot report on itself when the
//! collector is unreachable is not observable.

pub mod aiops;
pub mod critical_paths;
pub mod golden;
pub mod logs;
pub mod metrics;
pub mod sampling;
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

    /// The surface a deployed process is built on: [`Self::new`], with every
    /// log record also written to stderr (OBS-020).
    ///
    /// A workload's stderr is what Cloud Logging ingests, and until the
    /// composition roots used this the structured logger never left process
    /// memory: its records — today the telemetry drain's failure warnings,
    /// which are exactly the lines an operator needs when telemetry stops
    /// arriving — sat in a ring buffer nothing read. A constructor rather
    /// than a second call each root must remember, so a root that builds its
    /// telemetry at all builds it echoing.
    pub fn foreground(service: impl Into<String>, clock: Arc<dyn qip_core::Clock>) -> Self {
        let telemetry = Self::new(service, clock);
        telemetry.logger.set_echo(true);
        telemetry
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
