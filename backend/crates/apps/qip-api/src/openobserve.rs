//! Draining this process's telemetry to OpenObserve over OTLP/JSON.
//!
//! ADR 0028 adopts OpenObserve as the platform's metrics, logs and traces
//! backend, over OTLP rather than Prometheus remote-write, and names this
//! mechanism as "the same shape ADR 0026 already designed for spans" —
//! ADR 0026's Option (b): a producer records into the bounded ring that
//! already exists, a drain thread in each composition root takes what it
//! holds on an interval and POSTs OTLP/JSON to a collector, blocking, with an
//! explicit timeout, on the pattern every other outbound call in this
//! workspace follows.
//!
//! `qip-api` hosts it rather than `qip-fastbrain` or `qip-deepbrain` because
//! it is already this platform's one process with an established outbound
//! HTTP pattern to a peer it does not control — [`crate::mesh`]'s capital
//! dispatcher and policy courier, both built on the same
//! [`qip_transport::HttpClient`] this module uses, both bounded by an
//! explicit `ClientLimits`, both composed here and nowhere else. Reusing that
//! pattern rather than inventing a second one is the point; `qip-fastbrain`
//! and `qip-deepbrain` are candidates for the same wiring later, on their own
//! evidence, not assumed here.
//!
//! # What is and is not proven here
//!
//! `Snapshot::to_otlp_metrics` (`qip-observability`) is new code this task
//! adds, encoding OTLP's own published JSON mapping; `Tracer::export`
//! already existed and, per ADR 0026, produces only the *top-level* OTLP
//! shape (`resourceSpans` → `scopeSpans` → `spans`) — the leaf `Span` objects
//! inside it serialise this crate's own field names (`trace_id`, `span_id`,
//! `start`, …), not OTLP's (`traceId`, `spanId`, `startTimeUnixNano`, …), and
//! `attributes` is a plain map rather than OTLP's `KeyValue` array. This
//! module posts that shape unmodified, because changing `Tracer::export`'s
//! wire format was not in this task's scope and doing it as a side effect
//! here would risk breaking `trace_export_has_the_otlp_shape`, the test that
//! already depends on today's shape. **The exact endpoint paths this module
//! posts to — `/api/{org}/v1/metrics` and `/api/{org}/traces` — are taken
//! from ADR 0028's own citation of OpenObserve's published API reference,
//! confirmed against that document but not against a live OpenObserve
//! instance from this sandbox; ADR 0028 itself names this as the one thing
//! only a real deployment can close.**
//!
//! # Principle 10, applied to telemetry about telemetry
//!
//! A collector that is down must not crash the process reporting to it, and
//! must not do so *silently* either — a failed POST that leaves no trace
//! behind is indistinguishable from one that never happened. Every attempt
//! is counted on the telemetry surface it is itself describing
//! ([`qip_observability::metrics::names::TELEMETRY_EXPORT_ATTEMPTS`],
//! [`TELEMETRY_EXPORT_FAILURES`]) and logged at `Warn` on failure, and the
//! loop always sleeps and tries again rather than stopping.
//!
//! [`TELEMETRY_EXPORT_FAILURES`]: qip_observability::metrics::names::TELEMETRY_EXPORT_FAILURES

use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method, Url};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;

/// The gate. Unset means the drain thread never starts and this process's
/// telemetry stays local, exactly as it always has — absence is a decision
/// an operator can make honestly, not a misconfiguration.
pub const URL_VARIABLE: &str = "QIP_OPENOBSERVE_URL";
/// The OpenObserve organisation the ingestion paths are scoped under. Set is
/// required whenever the URL is set: OpenObserve's own ingestion paths carry
/// the organisation in the path (`/api/{org}/...`), so there is no default
/// that is not a guess about a deployment this process cannot see.
pub const ORG_VARIABLE: &str = "QIP_OPENOBSERVE_ORG";
/// The credential this process presents, read through [`qip_core::secret`]
/// so the deployment may mount it as a file rather than an environment
/// value. Carries the whole `Authorization` header value as the operator
/// wants it sent (`Basic base64(user:pass)` or `Bearer <token>`) — this
/// module accepts it verbatim rather than picking a scheme, because
/// OpenObserve accepts either depending on deployment and guessing one would
/// be exactly the "clamp an invalid input instead of refusing it" this
/// workspace forbids. Unset means the POST carries no `Authorization` header,
/// which is a valid OpenObserve configuration (basic auth disabled) and not
/// refused here.
pub const CREDENTIAL_VARIABLE: &str = "QIP_OPENOBSERVE_AUTHORIZATION";
/// How often the drain thread exports, in whole seconds. Unset takes
/// [`DEFAULT_INTERVAL`].
pub const INTERVAL_VARIABLE: &str = "QIP_OPENOBSERVE_INTERVAL_SECS";

/// The default drain interval: frequent enough that a dashboard reads as
/// live, infrequent enough that a collector outage does not turn into a
/// tight retry loop against a peer that is already down.
pub const DEFAULT_INTERVAL: StdDuration = StdDuration::from_secs(15);

/// What one export attempt posted, and what came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalOutcome {
    /// The peer answered with a 2xx status.
    Sent { status: u16 },
    /// The peer answered with a non-2xx status — a response, not a socket
    /// failure, so the detail is the status and a bounded excerpt of the
    /// body OpenObserve sent back explaining why.
    Refused { status: u16, detail: String },
    /// No response reached this process at all: a connect failure, a
    /// timeout, or a malformed reply. [`qip_transport::HttpError`]'s own
    /// message names which.
    Failed { detail: String },
}

impl SignalOutcome {
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Sent { .. })
    }
}

/// What one drain pass did, for the thread loop to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPass {
    pub metrics: SignalOutcome,
    pub traces: SignalOutcome,
}

/// Configuration for the drain thread, already validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenObserveConfig {
    base: Url,
    org: String,
    credential: Option<String>,
    interval: StdDuration,
}

impl OpenObserveConfig {
    /// Read the configuration from the environment, or `None` when this
    /// process exports nothing. Absence is not a misconfiguration — see
    /// [`URL_VARIABLE`].
    pub fn from_env() -> Result<Option<Self>> {
        let Some(url) = std::env::var(URL_VARIABLE)
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let org = std::env::var(ORG_VARIABLE).ok();
        let interval = std::env::var(INTERVAL_VARIABLE).ok();
        let credential = qip_core::secret::from_environment(CREDENTIAL_VARIABLE)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
        Self::parse(&url, org.as_deref(), interval.as_deref(), credential).map(Some)
    }

    /// The parse behind [`Self::from_env`], separated so a test can hand it
    /// strings without touching the process environment.
    pub fn parse(
        url: &str,
        org: Option<&str>,
        interval_secs: Option<&str>,
        credential: Option<String>,
    ) -> Result<Self> {
        let base = Url::parse(url).map_err(|error| {
            Error::invalid(format!(
                "configuration: {URL_VARIABLE} is not a usable URL: {error}"
            ))
        })?;
        let org = org
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: {URL_VARIABLE} is set and {ORG_VARIABLE} is not. \
                     OpenObserve's ingestion paths are scoped by organisation \
                     (/api/{{org}}/...) and there is no default that is not a guess about a \
                     deployment this process cannot see; name the organisation explicitly"
                ))
            })?
            .to_string();
        let interval = match interval_secs {
            None => DEFAULT_INTERVAL,
            Some(value) => {
                let seconds: u64 = value.trim().parse().map_err(|_| {
                    Error::invalid(format!(
                        "configuration: {INTERVAL_VARIABLE} is not a whole number of seconds: \
                         {value}"
                    ))
                })?;
                if seconds == 0 {
                    return Err(Error::invalid(format!(
                        "configuration: {INTERVAL_VARIABLE} is zero, which would export in a \
                         tight loop against a peer this process does not control; set an \
                         interval of at least one second"
                    )));
                }
                StdDuration::from_secs(seconds)
            }
        };
        Ok(Self {
            base,
            org,
            credential,
            interval,
        })
    }

    fn metrics_url(&self) -> Result<Url> {
        self.base
            .with_path(&format!("/api/{}/v1/metrics", self.org))
            .map_err(|error| Error::invalid(format!("{URL_VARIABLE}: {error}")))
    }

    fn traces_url(&self) -> Result<Url> {
        self.base
            .with_path(&format!("/api/{}/traces", self.org))
            .map_err(|error| Error::invalid(format!("{URL_VARIABLE}: {error}")))
    }

    /// A one-line description for the start-up banner.
    pub fn describe(&self) -> String {
        format!(
            "{} org={} every {}s{}",
            self.base,
            self.org,
            self.interval.as_secs(),
            if self.credential.is_some() {
                ", authenticated"
            } else {
                ", unauthenticated"
            }
        )
    }
}

/// Client limits for the drain's POSTs. A collector on the other side of the
/// egress proxy is not this process's own loopback peer the way the mesh's
/// capital feed is, so the timeouts are looser than [`crate::mesh`]'s
/// loopback dispatch — but still bounded, and still short enough that a
/// stalled collector costs one drain pass rather than the thread.
fn drain_limits() -> ClientLimits {
    ClientLimits {
        connect_timeout: StdDuration::from_secs(2),
        read_timeout: StdDuration::from_secs(5),
        write_timeout: StdDuration::from_secs(5),
        ..ClientLimits::default()
    }
}

/// POST one document to one OpenObserve ingestion endpoint.
fn post(
    client: &HttpClient,
    url: &Url,
    credential: Option<&str>,
    body: &serde_json::Value,
) -> SignalOutcome {
    let bytes = match serde_json::to_vec(body) {
        Ok(bytes) => bytes,
        // Encoding a `serde_json::Value` this module built itself fails only
        // if it somehow contains non-finite floats or invalid map keys —
        // neither of which `Snapshot::to_otlp_metrics` or `Tracer::export`
        // produce. Counted as a failure rather than reached with `expect`:
        // a defect in the encoder must not become a panic in the drain
        // thread that discovers it.
        Err(error) => {
            return SignalOutcome::Failed {
                detail: format!("the document could not be serialised: {error}"),
            };
        }
    };
    let request = match HttpRequest::json(Method::Post, &url.to_string(), bytes) {
        Ok(request) => request,
        Err(error) => {
            return SignalOutcome::Failed {
                detail: error.to_string(),
            };
        }
    };
    let request = match credential {
        Some(value) => request.with_header("authorization", value),
        None => request,
    };
    match client.send(&request) {
        Ok(response) if response.is_success() => SignalOutcome::Sent {
            status: response.status,
        },
        Ok(response) => SignalOutcome::Refused {
            status: response.status,
            detail: response.body_excerpt(),
        },
        Err(error) => SignalOutcome::Failed {
            detail: error.to_string(),
        },
    }
}

/// One export pass: snapshot this process's metrics and spans as they stand
/// right now, and POST each to its OpenObserve endpoint.
///
/// Takes `now` rather than reading a clock, so a test drives it
/// deterministically and the drain thread is the only caller that reaches
/// for a real one.
pub fn export_once(
    telemetry: &Telemetry,
    client: &HttpClient,
    config: &OpenObserveConfig,
    now: Timestamp,
) -> Result<ExportPass> {
    let metrics_url = config.metrics_url()?;
    let traces_url = config.traces_url()?;
    let metrics_body = telemetry.metrics.snapshot().to_otlp_metrics(now.as_nanos());
    let traces_body = telemetry.tracer.export();
    Ok(ExportPass {
        metrics: post(
            client,
            &metrics_url,
            config.credential.as_deref(),
            &metrics_body,
        ),
        traces: post(
            client,
            &traces_url,
            config.credential.as_deref(),
            &traces_body,
        ),
    })
}

/// Record one pass's outcome onto the telemetry surface it describes, and
/// log a failure loudly. Never panics: this is principle 10 applied to the
/// export mechanism itself, and a defect here reaching the process that is
/// only trying to report a defect elsewhere would be the exact failure this
/// module exists to avoid — so failure detail is read out of the `Option`
/// the match below produces rather than matched a second time against an
/// arm (`Sent`) that structurally cannot occur there.
///
/// Public so the crate's integration tests can call the exact function the
/// drain thread calls, rather than a copy of its three lines kept in a test
/// file where it could drift from what actually ships.
pub fn record(telemetry: &Telemetry, pass: &ExportPass) {
    for (signal, outcome) in [("metrics", &pass.metrics), ("traces", &pass.traces)] {
        telemetry.metrics.count(
            names::TELEMETRY_EXPORT_ATTEMPTS,
            labels([("signal", signal)]),
        );
        let failure_detail = match outcome {
            SignalOutcome::Sent { .. } => None,
            SignalOutcome::Refused { status, detail } => Some(format!(
                "OpenObserve refused the {signal} export with {status}: {detail}"
            )),
            SignalOutcome::Failed { detail } => Some(format!(
                "the {signal} export to OpenObserve failed: {detail}"
            )),
        };
        if let Some(detail) = failure_detail {
            telemetry.metrics.count(
                names::TELEMETRY_EXPORT_FAILURES,
                labels([("signal", signal)]),
            );
            telemetry.logger.warn(detail);
        }
    }
}

/// The running drain thread. Dropping it stops the loop before its next
/// sleep ends, the same shape [`crate::mesh::MeshListener`] uses.
#[derive(Debug)]
pub struct DrainHandle {
    shutdown: Arc<AtomicBool>,
}

impl Drop for DrainHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// Start the drain thread. One thread for the life of the process: no pool,
/// because there is exactly one collector this process reports to and one
/// interval to keep.
pub fn spawn(
    telemetry: Telemetry,
    config: OpenObserveConfig,
    clock: Arc<dyn Clock>,
) -> Result<DrainHandle> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = shutdown.clone();
    std::thread::Builder::new()
        .name("qip-openobserve-drain".to_string())
        .spawn(move || {
            let client = HttpClient::new(drain_limits());
            while !thread_shutdown.load(Ordering::Relaxed) {
                let now = clock.now();
                match export_once(&telemetry, &client, &config, now) {
                    Ok(pass) => record(&telemetry, &pass),
                    // A URL that fails to build from an already-validated
                    // base and organisation cannot happen in production —
                    // `OpenObserveConfig::parse` proved both — but the drain
                    // loop still must not stop over it: logged and skipped,
                    // exactly like a POST failure.
                    Err(error) => telemetry.logger.warn(format!(
                        "the OpenObserve export could not build its endpoint URLs: {}",
                        error.message()
                    )),
                }
                std::thread::sleep(config.interval);
            }
        })
        .map_err(|error| {
            Error::io(format!(
                "cannot start the OpenObserve drain thread: {error}"
            ))
        })?;
    Ok(DrainHandle { shutdown })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_with_no_org_is_refused_rather_than_defaulted() {
        let error = OpenObserveConfig::parse("http://collector:5080", None, None, None)
            .expect_err("an unset organisation was accepted");
        assert!(error.message().contains(ORG_VARIABLE));
    }

    #[test]
    fn a_zero_interval_is_refused_because_it_is_a_tight_loop_not_a_disable_switch() {
        let error =
            OpenObserveConfig::parse("http://collector:5080", Some("default"), Some("0"), None)
                .expect_err("a zero interval was accepted");
        assert!(error.message().contains(INTERVAL_VARIABLE));
    }

    #[test]
    fn a_well_formed_configuration_builds_the_endpoints_the_adr_names() {
        let config =
            OpenObserveConfig::parse("http://collector:5080", Some("qip"), Some("30"), None)
                .expect("a well-formed configuration was refused");
        assert_eq!(
            config.metrics_url().unwrap().to_string(),
            "http://collector:5080/api/qip/v1/metrics"
        );
        assert_eq!(
            config.traces_url().unwrap().to_string(),
            "http://collector:5080/api/qip/traces"
        );
        assert_eq!(config.interval, StdDuration::from_secs(30));
    }
}
