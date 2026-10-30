//! Pushing a process's metrics to Cloud Monitoring (ADR 0121, R12-OBS-9143).
//!
//! Nothing scrapes a Cloud Run service: the managed-Prometheus sidecar that
//! would have is refused by the platform's own vulnerability gate, and ADR
//! 0121 replaced the wait for a fixed image with a push from the process.
//! This module is the I/O half of that push — the thread, the bearer token,
//! the HTTP call. The encoding is a pure function of a registry snapshot in
//! `qip-observability` ([`qip_observability::cloud_monitoring`]); the process
//! environment is read only in the composition roots, which hand
//! [`CloudMonitoringConfig::read`] a lookup over [`VARIABLES`].
//!
//! # Off unless named, and refused when named badly
//!
//! [`PROJECT_VARIABLE`] unset means no exporter thread exists and the
//! process behaves exactly as it did before this module. That is the default
//! everywhere, deliberately: a push needs a route out (the egress proxy has
//! no listener for `monitoring.googleapis.com`, and the fast brain has no
//! proxy at all, ADR 0024), the service account needs
//! `roles/monitoring.metricWriter`, and the alert policies must be re-pointed
//! at the pushed types — all infrastructure this change does not make. Set,
//! every other value is required and checked at start-up, and a malformed
//! one stops the process rather than starting an exporter that would push to
//! the wrong place or nowhere.
//!
//! # Two routes, and why plaintext is only to loopback
//!
//! The endpoint is either `http://127.0.0.1:<port>` — the egress proxy, which
//! terminates TLS to Google (`.claude/rules/domains/data-and-streaming.md`) —
//! or `https://…`, reached directly through [`qip_transport::TlsHttpClient`]
//! (ADR 0112) with the pinned Google roots the composition root supplies.
//! A plaintext endpoint anywhere but loopback is refused by
//! [`qip_transport::http::require_loopback_egress`]: the request carries a
//! bearer token, and a token sent in the clear off the instance is a token
//! anyone on the path may replay for an hour.
//!
//! # Bounded, counted, never in the way
//!
//! One thread, off every request and cycle thread; it reads the same
//! registry `/metrics` reads, holding its lock only for the snapshot. Each
//! pass sends at most [`MAX_REQUESTS_PER_PASS`] requests of at most
//! [`MAX_SERIES_PER_REQUEST`] series, each under [`push_limits`]' explicit
//! timeouts; a transport failure abandons the rest of the pass rather than
//! paying the timeout again on a peer already shown to be down. Nothing is
//! queued: a point that did not land is superseded by the next pass's
//! cumulative value, so a retry queue would hold only stale copies of a
//! number the registry still has. Every failure is counted on the registry it
//! is pushing ([`names::METRICS_EXPORT_FAILURES`] by request,
//! [`names::METRICS_EXPORT_SERIES_DROPPED`] by series) and logged, and the
//! wait after a pass in which nothing landed doubles up to
//! [`crate::BACKOFF_CEILING`].
//!
//! [`MAX_SERIES_PER_REQUEST`]: qip_observability::cloud_monitoring::MAX_SERIES_PER_REQUEST

use crate::{next_delay, sleep_until};
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_observability::Telemetry;
use qip_observability::cloud_monitoring::MonitoredTask;
use qip_observability::metrics::{labels, names};
use qip_transport::kms::BearerSource;
use qip_transport::rustls;
use qip_transport::{
    ClientLimits, HttpClient, HttpError, HttpRequest, HttpResponse, Method, TlsHttpClient, Url,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;

/// Every variable the exporter reads, so a composition root can hand
/// [`CloudMonitoringConfig::read`] a lookup that answers these and nothing
/// else.
pub const VARIABLES: [&str; 4] = [
    PROJECT_VARIABLE,
    ENDPOINT_VARIABLE,
    LOCATION_VARIABLE,
    INTERVAL_VARIABLE,
];

/// The gate: the project the series are written into. Unset means no
/// exporter. Set, it must be a well-formed project id.
pub const PROJECT_VARIABLE: &str = "QIP_METRICS_EXPORT_PROJECT";
/// Where the API is reached: `http://127.0.0.1:<port>` (the egress proxy)
/// or `https://monitoring.googleapis.com` (direct, over TLS). Required when
/// the project is set, because neither route exists by default and a guessed
/// one would fail every push without saying why.
pub const ENDPOINT_VARIABLE: &str = "QIP_METRICS_EXPORT_ENDPOINT";
/// The region the process runs in: `generic_task`'s `location` label.
/// Required when the project is set; the process cannot see its own region
/// without a further metadata call, and a wrong one files every series under
/// a place it is not.
pub const LOCATION_VARIABLE: &str = "QIP_METRICS_EXPORT_LOCATION";
/// Seconds between pushes. Unset takes [`DEFAULT_INTERVAL`].
pub const INTERVAL_VARIABLE: &str = "QIP_METRICS_EXPORT_INTERVAL_SECS";

/// ADR 0121's interval.
pub const DEFAULT_INTERVAL: StdDuration = StdDuration::from_secs(60);
/// The shortest interval accepted. Cloud Monitoring refuses a point written
/// to a series less than five seconds after the previous one; ten leaves
/// room for a pass that started late.
pub const MIN_INTERVAL: StdDuration = StdDuration::from_secs(10);
/// The most requests one pass sends: two thousand series. The registry's
/// labels are bounded where they are recorded, so a registry past this is a
/// cardinality defect, and the surplus is counted as `over_budget` rather
/// than turning one pass into an unbounded run of requests.
pub const MAX_REQUESTS_PER_PASS: usize = 10;
/// The `generic_task` namespace every series is written under.
pub const NAMESPACE: &str = "qip";

/// Why a request did not land. The `reason` label of
/// [`names::METRICS_EXPORT_FAILURES`]; an enum so the label is bounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureReason {
    /// No usable bearer token; no socket was opened.
    Token,
    /// The connect or the read outlived its timeout.
    Timeout,
    /// Any other way of getting no response.
    Transport,
    /// The API answered with a non-2xx status.
    Refused,
    /// The body could not be serialised or the request could not be built.
    Encoding,
}

impl FailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Token => "token",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::Refused => "refused",
            Self::Encoding => "encoding",
        }
    }

    fn of(error: &HttpError) -> Self {
        match error {
            HttpError::ConnectTimeout { .. } | HttpError::ReadTimeout { .. } => Self::Timeout,
            HttpError::InvalidUrl { .. } | HttpError::UnsupportedScheme { .. } => Self::Encoding,
            _ => Self::Transport,
        }
    }
}

/// What one request did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestOutcome {
    Sent {
        status: u16,
        series: usize,
    },
    Failed {
        reason: FailureReason,
        detail: String,
        series: usize,
    },
}

/// What one pass did, for [`record_push`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushPass {
    pub requests: Vec<RequestOutcome>,
    /// Series the API would refuse, left out by the encoder.
    pub unencodable: usize,
    /// Series past [`MAX_REQUESTS_PER_PASS`].
    pub over_budget: usize,
    /// Series in requests not attempted after a transport failure.
    pub abandoned: usize,
}

impl PushPass {
    /// Whether any request landed. A pass with nothing to send has not
    /// failed.
    pub fn landed_or_idle(&self) -> bool {
        self.requests.is_empty()
            || self
                .requests
                .iter()
                .any(|outcome| matches!(outcome, RequestOutcome::Sent { .. }))
    }
}

/// The route to the API, as parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Endpoint {
    /// Plaintext to the egress proxy on loopback.
    Egress(Url),
    /// TLS straight to the API.
    Direct(Url),
}

/// The exporter's configuration, validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudMonitoringConfig {
    project: String,
    location: String,
    endpoint: Endpoint,
    interval: StdDuration,
}

impl CloudMonitoringConfig {
    /// The configuration through `lookup`, or `None` when
    /// [`PROJECT_VARIABLE`] is unset or blank and this process pushes
    /// nothing. A lookup and not the environment, so a test can reach every
    /// branch: `std::env::set_var` is `unsafe` in the 2024 edition.
    pub fn read(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let Some(project) = lookup(PROJECT_VARIABLE).filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        Self::parse(
            &project,
            lookup(ENDPOINT_VARIABLE).as_deref(),
            lookup(LOCATION_VARIABLE).as_deref(),
            lookup(INTERVAL_VARIABLE).as_deref(),
        )
        .map(Some)
    }

    /// The parse behind [`Self::read`].
    pub fn parse(
        project: &str,
        endpoint: Option<&str>,
        location: Option<&str>,
        interval_secs: Option<&str>,
    ) -> Result<Self> {
        if !is_project_id(project) {
            return Err(Error::invalid(format!(
                "configuration: {PROJECT_VARIABLE} is {project:?}, which is not a Google Cloud \
                 project id (6 to 30 characters, a lowercase letter first, then lowercase \
                 letters, digits and hyphens, not ending in a hyphen). Name the project the \
                 series are written into, or unset it to push nothing"
            )));
        }
        let endpoint = endpoint
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: {PROJECT_VARIABLE} is set and {ENDPOINT_VARIABLE} is not. \
                     Name the route: http://127.0.0.1:<port> for the egress proxy, or \
                     https://monitoring.googleapis.com directly; neither exists by default"
                ))
            })?;
        let endpoint = parse_endpoint(endpoint)?;
        let location = location
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 64
                    && value
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            })
            .ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: {PROJECT_VARIABLE} is set and {LOCATION_VARIABLE} is unset \
                     or not a region name (lowercase letters, digits and hyphens, such as \
                     europe-west2). Every series is filed under it"
                ))
            })?
            .to_string();
        let interval = match interval_secs {
            None => DEFAULT_INTERVAL,
            Some(value) => {
                let seconds: u64 = value.trim().parse().map_err(|_| {
                    Error::invalid(format!(
                        "configuration: {INTERVAL_VARIABLE} is not a whole number of seconds: \
                         {value:?}"
                    ))
                })?;
                let interval = StdDuration::from_secs(seconds);
                if interval < MIN_INTERVAL {
                    return Err(Error::invalid(format!(
                        "configuration: {INTERVAL_VARIABLE} is {seconds}, below the {} second \
                         floor: Cloud Monitoring refuses a point written to a series within \
                         five seconds of the last, so a shorter interval is a stream of refused \
                         writes rather than a faster chart",
                        MIN_INTERVAL.as_secs()
                    )));
                }
                interval
            }
        };
        Ok(Self {
            project: project.to_string(),
            location,
            endpoint,
            interval,
        })
    }

    /// Whether the endpoint is reached over TLS, so the composition root
    /// knows to load the Google roots before calling [`MonitoringClient::new`].
    pub const fn needs_tls(&self) -> bool {
        matches!(self.endpoint, Endpoint::Direct(_))
    }

    pub fn interval(&self) -> StdDuration {
        self.interval
    }

    /// The `generic_task` a process with this configuration writes as.
    pub fn task(&self, job: &str, task_id: &str) -> MonitoredTask {
        MonitoredTask {
            project_id: self.project.clone(),
            location: self.location.clone(),
            namespace: NAMESPACE.to_string(),
            job: job.to_string(),
            task_id: task_id.to_string(),
        }
    }

    fn time_series_url(&self) -> std::result::Result<Url, HttpError> {
        let path = format!("/v3/projects/{}/timeSeries", self.project);
        match &self.endpoint {
            Endpoint::Egress(base) | Endpoint::Direct(base) => base.with_path(&path),
        }
    }

    /// A one-line description for the start-up banner.
    pub fn describe(&self) -> String {
        let (route, base) = match &self.endpoint {
            Endpoint::Egress(base) => ("through the egress proxy at", base),
            Endpoint::Direct(base) => ("over TLS to", base),
        };
        format!(
            "project {} ({}) {route} {base} every {}s",
            self.project,
            self.location,
            self.interval.as_secs()
        )
    }
}

/// Google's rule for a project id.
fn is_project_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (6..=30).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn parse_endpoint(raw: &str) -> Result<Endpoint> {
    let is_https = raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
    let (url, label) = if is_https {
        (Url::parse_https(raw).map(Endpoint::Direct), "an https")
    } else {
        qip_transport::http::require_loopback_egress(raw).map_err(|error| {
            Error::invalid(format!(
                "configuration: {ENDPOINT_VARIABLE}: {}. The request carries a bearer token, \
                 so plaintext is accepted only to the egress proxy on loopback",
                error.message()
            ))
        })?;
        (Url::parse(raw).map(Endpoint::Egress), "an http")
    };
    let endpoint = url.map_err(|error| {
        Error::invalid(format!(
            "configuration: {ENDPOINT_VARIABLE} is not {label} URL this exporter can open: {error}"
        ))
    })?;
    let (Endpoint::Egress(base) | Endpoint::Direct(base)) = &endpoint;
    if !matches!(base.target(), "" | "/") {
        return Err(Error::invalid(format!(
            "configuration: {ENDPOINT_VARIABLE} carries a path ({}); name the scheme, host and \
             port only — the exporter appends /v3/projects/<project>/timeSeries, and a base path \
             would be replaced rather than kept",
            base.target()
        )));
    }
    Ok(endpoint)
}

/// The limits on every push. A body is at most two hundred series, a few
/// hundred kilobytes, so the write budget is generous; the read budget is
/// what bounds a peer that accepts the request and says nothing.
pub fn push_limits() -> ClientLimits {
    ClientLimits {
        max_body: 1024 * 1024,
        connect_timeout: StdDuration::from_secs(2),
        read_timeout: StdDuration::from_secs(10),
        write_timeout: StdDuration::from_secs(10),
        ..ClientLimits::default()
    }
}

/// The client a push goes out on, matched to the endpoint's scheme.
#[derive(Clone, Debug)]
pub enum MonitoringClient {
    Plain(HttpClient),
    Tls(TlsHttpClient),
}

impl MonitoringClient {
    /// The client for `config`. `tls` is the server-authenticated config
    /// built from the pinned Google roots, and is required exactly when the
    /// endpoint is `https`: refused when missing, and refused when supplied
    /// for a plaintext endpoint, because a root set nobody uses is a
    /// configuration somebody believes is protecting a route it is not on.
    pub fn new(
        config: &CloudMonitoringConfig,
        limits: ClientLimits,
        tls: Option<Arc<rustls::ClientConfig>>,
    ) -> Result<Self> {
        match (&config.endpoint, tls) {
            (Endpoint::Egress(_), None) => Ok(Self::Plain(HttpClient::new(limits))),
            (Endpoint::Direct(_), Some(tls)) => Ok(Self::Tls(TlsHttpClient::new(limits, tls))),
            (Endpoint::Direct(_), None) => Err(Error::invalid(format!(
                "configuration: {ENDPOINT_VARIABLE} is https and no Google TLS roots were \
                 supplied; set the roots path, or point the endpoint at the egress proxy"
            ))),
            (Endpoint::Egress(_), Some(_)) => Err(Error::invalid(format!(
                "configuration: {ENDPOINT_VARIABLE} is plaintext to the egress proxy and TLS \
                 roots were supplied for it; the proxy terminates TLS, so the roots would \
                 protect nothing"
            ))),
        }
    }

    fn request(&self, url: &Url, body: Vec<u8>) -> std::result::Result<HttpRequest, HttpError> {
        match self {
            Self::Plain(_) => HttpRequest::json(Method::Post, &url.to_string(), body),
            Self::Tls(_) => HttpRequest::json_tls(Method::Post, &url.to_string(), body),
        }
    }

    fn send(&self, request: &HttpRequest) -> std::result::Result<HttpResponse, HttpError> {
        match self {
            Self::Plain(client) => client.send(request),
            Self::Tls(client) => client.send(request),
        }
    }
}

/// One push: snapshot the registry, encode it, and POST each body with the
/// bearer token. Takes both instants rather than reading a clock, so a test
/// drives it deterministically.
pub fn push_once(
    telemetry: &Telemetry,
    client: &MonitoringClient,
    config: &CloudMonitoringConfig,
    task: &MonitoredTask,
    bearer: &dyn BearerSource,
    start: Timestamp,
    now: Timestamp,
) -> PushPass {
    let encoding = telemetry
        .metrics
        .snapshot()
        .to_cloud_monitoring_time_series(task, start, now);
    let mut bodies = encoding.request_bodies();
    let series_in = |body: &serde_json::Value| body["timeSeries"].as_array().map_or(0, Vec::len);
    let mut pass = PushPass {
        unencodable: encoding.unencodable.len(),
        ..PushPass::default()
    };
    if bodies.len() > MAX_REQUESTS_PER_PASS {
        pass.over_budget = bodies[MAX_REQUESTS_PER_PASS..].iter().map(series_in).sum();
        bodies.truncate(MAX_REQUESTS_PER_PASS);
    }
    if bodies.is_empty() {
        return pass;
    }
    let total: usize = bodies.iter().map(series_in).sum();
    // Once per pass, before any socket: a token that cannot be had fails the
    // whole pass the same way, and asking again per body would only multiply
    // the metadata-server calls.
    let token = match bearer.bearer() {
        Ok(token) if !token.trim().is_empty() && !token.chars().any(char::is_control) => token,
        Ok(_) => {
            pass.requests.push(RequestOutcome::Failed {
                reason: FailureReason::Token,
                detail: "the bearer source returned a blank token or one carrying a control \
                         character"
                    .to_string(),
                series: total,
            });
            return pass;
        }
        Err(error) => {
            pass.requests.push(RequestOutcome::Failed {
                reason: FailureReason::Token,
                detail: error.message().to_string(),
                series: total,
            });
            return pass;
        }
    };
    let authorization = format!("Bearer {token}");
    let url = match config.time_series_url() {
        Ok(url) => url,
        Err(error) => {
            pass.requests.push(RequestOutcome::Failed {
                reason: FailureReason::Encoding,
                detail: error.to_string(),
                series: total,
            });
            return pass;
        }
    };
    for (index, body) in bodies.iter().enumerate() {
        let series = series_in(body);
        let outcome = send_body(client, &url, &authorization, body, series);
        let abandon = matches!(
            outcome,
            RequestOutcome::Failed {
                reason: FailureReason::Timeout | FailureReason::Transport,
                ..
            }
        );
        pass.requests.push(outcome);
        if abandon {
            pass.abandoned = bodies[index + 1..].iter().map(series_in).sum();
            break;
        }
    }
    pass
}

/// POST one body and say what came back.
fn send_body(
    client: &MonitoringClient,
    url: &Url,
    authorization: &str,
    body: &serde_json::Value,
    series: usize,
) -> RequestOutcome {
    let request = match serde_json::to_vec(body)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            client
                .request(url, bytes)
                .map_err(|error| error.to_string())
        }) {
        Ok(request) => request.with_header("authorization", authorization),
        Err(detail) => {
            return RequestOutcome::Failed {
                reason: FailureReason::Encoding,
                detail,
                series,
            };
        }
    };
    match client.send(&request) {
        Ok(response) if response.is_success() => RequestOutcome::Sent {
            status: response.status,
            series,
        },
        Ok(response) => RequestOutcome::Failed {
            reason: FailureReason::Refused,
            detail: format!("HTTP {}: {}", response.status, response.body_excerpt()),
            series,
        },
        Err(error) => RequestOutcome::Failed {
            reason: FailureReason::of(&error),
            detail: error.to_string(),
            series,
        },
    }
}

/// Count one pass onto the registry it pushed, and log its failures.
/// Public so the crate's tests call the function the thread calls.
pub fn record_push(telemetry: &Telemetry, pass: &PushPass) {
    let metrics = &telemetry.metrics;
    let mut series_failed = 0usize;
    for outcome in &pass.requests {
        metrics.count(names::METRICS_EXPORT_REQUESTS, labels([]));
        if let RequestOutcome::Failed {
            reason,
            detail,
            series,
        } = outcome
        {
            series_failed = series_failed.saturating_add(*series);
            metrics.count(
                names::METRICS_EXPORT_FAILURES,
                labels([("reason", reason.as_str())]),
            );
            telemetry.logger.warn(format!(
                "a Cloud Monitoring push of {series} series failed ({}): {detail}",
                reason.as_str()
            ));
        }
    }
    for (reason, series) in [
        ("unencodable", pass.unencodable),
        ("over_budget", pass.over_budget),
        (
            "request_failed",
            series_failed.saturating_add(pass.abandoned),
        ),
    ] {
        if series > 0 {
            metrics.increment(
                names::METRICS_EXPORT_SERIES_DROPPED,
                labels([("reason", reason)]),
                u64::try_from(series).unwrap_or(u64::MAX),
            );
        }
    }
    if pass.unencodable > 0 || pass.over_budget > 0 {
        telemetry.logger.warn(format!(
            "the Cloud Monitoring push left out {} series the API would refuse and {} past the \
             per-pass budget of {MAX_REQUESTS_PER_PASS} requests",
            pass.unencodable, pass.over_budget
        ));
    }
}

/// The running exporter thread. Dropping it stops the loop before its next
/// sleep ends.
#[derive(Debug)]
pub struct PushHandle {
    shutdown: Arc<AtomicBool>,
}

impl Drop for PushHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// Start the exporter thread.
///
/// The thread sleeps one interval before its first push, so the first point
/// has an interval longer than zero and a registry the process has had time
/// to fill. The start time every cumulative series carries is read here,
/// once, and the task id is this process's id with that start time: unique
/// among replicas, and new on a restart, which is what a counter reset is.
pub fn spawn_push(
    telemetry: Telemetry,
    config: CloudMonitoringConfig,
    client: MonitoringClient,
    bearer: Arc<dyn BearerSource>,
    clock: Arc<dyn Clock>,
) -> Result<PushHandle> {
    let start = clock.now();
    let task = config.task(
        telemetry.metrics.service(),
        &format!("{}-{}", std::process::id(), start.as_millis()),
    );
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = shutdown.clone();
    std::thread::Builder::new()
        .name("qip-cloud-monitoring-push".to_string())
        .spawn(move || {
            let mut consecutive_failures = 0u32;
            loop {
                sleep_until(
                    &thread_shutdown,
                    next_delay(config.interval, consecutive_failures),
                );
                if thread_shutdown.load(Ordering::Relaxed) {
                    return;
                }
                let pass = push_once(
                    &telemetry,
                    &client,
                    &config,
                    &task,
                    bearer.as_ref(),
                    start,
                    clock.now(),
                );
                record_push(&telemetry, &pass);
                consecutive_failures = if pass.landed_or_idle() {
                    0
                } else {
                    consecutive_failures.saturating_add(1)
                };
            }
        })
        .map_err(|error| {
            Error::io(format!(
                "cannot start the Cloud Monitoring push thread: {error}"
            ))
        })?;
    Ok(PushHandle { shutdown })
}

/// The Google roots variable, read only when the endpoint is `https`. The
/// same name every other Google API client in the workspace reads, so one
/// mounted root set serves them all.
pub const ROOTS_VARIABLE: &str = qip_transport::cas::GOOGLE_TLS_ROOTS_VARIABLE;

/// Whether `name` is a variable this exporter reads, for the lookup a
/// composition root hands [`start`].
pub fn reads(name: &str) -> bool {
    VARIABLES.contains(&name) || name == ROOTS_VARIABLE
}

/// Read the configuration, build the client and start the thread: the whole
/// of what a composition root does. `None` when [`PROJECT_VARIABLE`] is
/// unset. Every refusal here happens before the process serves, so a
/// deployment that named the exporter badly exits rather than running with
/// a push that goes nowhere.
///
/// `bearer` is called only when the exporter is configured, so a process
/// that pushes nothing never constructs a metadata-server client.
pub fn start(
    lookup: &dyn Fn(&str) -> Option<String>,
    telemetry: &Telemetry,
    clock: Arc<dyn Clock>,
    bearer: &dyn Fn() -> Arc<dyn BearerSource>,
) -> Result<Option<(CloudMonitoringConfig, PushHandle)>> {
    let Some(config) = CloudMonitoringConfig::read(lookup)? else {
        return Ok(None);
    };
    let tls = if config.needs_tls() {
        let path = lookup(ROOTS_VARIABLE)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: {ENDPOINT_VARIABLE} is https and {ROOTS_VARIABLE} is not                      set; the API's certificate is checked against those pinned roots and                      against nothing else"
                ))
            })?;
        let roots = std::fs::read(&path).map_err(|error| {
            Error::invalid(format!(
                "configuration: cannot read the Google TLS roots at {path} ({ROOTS_VARIABLE}):                  {error}"
            ))
        })?;
        Some(qip_transport::tls::server_authenticated_client(&roots)?)
    } else {
        None
    };
    let client = MonitoringClient::new(&config, push_limits(), tls)?;
    let handle = spawn_push(telemetry.clone(), config.clone(), client, bearer(), clock)?;
    Ok(Some((config, handle)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_or_blank_project_is_no_exporter_rather_than_a_refusal() {
        assert_eq!(CloudMonitoringConfig::read(&|_| None).unwrap(), None);
        assert_eq!(
            CloudMonitoringConfig::read(&|name| (name == PROJECT_VARIABLE).then(|| "  ".into()))
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_malformed_project_id_stops_the_process_rather_than_pushing_somewhere() {
        for bad in ["Qip-dev", "qip", "qip_dev01", "qip-dev-", "1qip-dev"] {
            let error = CloudMonitoringConfig::parse(
                bad,
                Some("http://127.0.0.1:9106"),
                Some("europe-west2"),
                None,
            )
            .expect_err(bad);
            assert!(error.message().contains(PROJECT_VARIABLE), "{bad}");
        }
        // And the gate admits a good one: a gate that refuses everything is
        // not a gate.
        let config = CloudMonitoringConfig::parse(
            "qip-dev-01",
            Some("http://127.0.0.1:9106"),
            Some("europe-west2"),
            None,
        )
        .unwrap();
        assert_eq!(config.interval(), DEFAULT_INTERVAL);
        assert!(!config.needs_tls());
        assert_eq!(
            config.time_series_url().unwrap().to_string(),
            "http://127.0.0.1:9106/v3/projects/qip-dev-01/timeSeries"
        );
    }

    #[test]
    fn a_plaintext_endpoint_off_loopback_is_refused_because_the_request_carries_a_token() {
        let error = CloudMonitoringConfig::parse(
            "qip-dev-01",
            Some("http://monitoring.googleapis.com:80"),
            Some("europe-west2"),
            None,
        )
        .expect_err("a plaintext endpoint off loopback was accepted");
        assert!(error.message().contains(ENDPOINT_VARIABLE));
        let direct = CloudMonitoringConfig::parse(
            "qip-dev-01",
            Some("https://monitoring.googleapis.com"),
            Some("europe-west2"),
            None,
        )
        .unwrap();
        assert!(direct.needs_tls());
        assert!(
            MonitoringClient::new(&direct, push_limits(), None).is_err(),
            "an https endpoint with no roots must be refused"
        );
    }

    #[test]
    fn a_missing_endpoint_or_location_and_a_short_interval_are_refused() {
        let missing_endpoint =
            CloudMonitoringConfig::parse("qip-dev-01", None, Some("europe-west2"), None)
                .expect_err("no endpoint");
        assert!(missing_endpoint.message().contains(ENDPOINT_VARIABLE));
        let missing_location =
            CloudMonitoringConfig::parse("qip-dev-01", Some("http://127.0.0.1:9106"), None, None)
                .expect_err("no location");
        assert!(missing_location.message().contains(LOCATION_VARIABLE));
        let short = CloudMonitoringConfig::parse(
            "qip-dev-01",
            Some("http://127.0.0.1:9106"),
            Some("europe-west2"),
            Some("5"),
        )
        .expect_err("a five-second interval");
        assert!(short.message().contains(INTERVAL_VARIABLE));
        let with_path = CloudMonitoringConfig::parse(
            "qip-dev-01",
            Some("http://127.0.0.1:9106/v3"),
            Some("europe-west2"),
            None,
        )
        .expect_err("a base path");
        assert!(with_path.message().contains("path"));
    }
}
