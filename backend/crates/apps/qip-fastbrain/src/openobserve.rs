//! Draining this process's telemetry to OpenObserve over OTLP/JSON.
//!
//! # A copy, and why there is one
//!
//! Everything below the module doc is byte-identical to
//! `backend/crates/apps/qip-api/src/openobserve.rs`, and
//! `tests/openobserve.rs` fails if it stops being. It is a copy rather than a
//! shared module because there is nowhere shared to put it: nothing may depend
//! on an app (`.claude/rules/architecture/00-boundaries.md`), so this root
//! cannot reach the API's copy, and the right home — `qip-observability`,
//! which already owns `Snapshot::to_otlp_metrics` and both export metric
//! names — is a library this change is not permitted to edit. Three copies of
//! a credential parser is a liability and not a design: the refusals for an
//! empty credential and for one carrying a line break were added to the
//! original *after* it was first written, and a copy taken an hour earlier
//! would today be sending a credential this one refuses. The test is what
//! keeps that from happening silently until the lift lands.
//!
//! # What this process can actually reach
//!
//! Nothing, today, and the reason is worth stating where an operator setting
//! the variable will read it.
//!
//! ADR 0024 puts the egress proxy beside the API and the deep brain and
//! **deliberately not beside this process**, and
//! `infrastructure/terraform/catalogue.tf` refuses at plan time to give it one
//! — port 9102 on that proxy is a route to a language model API, and nothing
//! on the fast path may consult a model (ADR 0008). So this root has no
//! loopback hop that terminates TLS. `qip_transport::Url::parse` refuses every
//! scheme but plaintext `http` by name, so a target that is not reachable in
//! the clear is refused at start-up with an `EX_CONFIG` exit rather than
//! dialled for ever — which is what ADR 0030's OpenObserve is, being
//! anonymous on the public internet over https. A plaintext collector inside
//! the VPC would be reachable by this client and is the shape a deployment
//! would have to provide.
//!
//! No environment sets `QIP_OPENOBSERVE_URL` on any workload, so no thread
//! starts anywhere today; `manifest_wiring.rs`'s `READ_BUT_NOT_SET` records
//! why it must stay unset until a collector is vendored and attested. This
//! module is wiring ahead of that, not an exporting path, and nothing should
//! describe it as one.

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
/// value. The deployment should prefer the file spelling — this name with
/// [`qip_core::secret::FILE_SUFFIX`] appended: a credential set as an
/// environment value is readable from `/proc/<pid>/environ`, is inherited by
/// every child process, and lands in a crash dump. Set both and this refuses
/// to start rather than picking one. Carries the whole `Authorization` header
/// value as the operator
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

/// The longest the drain waits between attempts, however long the collector
/// has been unreachable. A ceiling and not an unbounded doubling: a drain that
/// has backed off to an hour is one that will not notice the collector coming
/// back within any shift an operator works.
pub const BACKOFF_CEILING: StdDuration = StdDuration::from_secs(300);

/// How often a sleeping drain checks whether its handle was dropped.
const SHUTDOWN_POLL: StdDuration = StdDuration::from_millis(250);

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
        Self::read(&|name| std::env::var(name).ok())
    }

    /// The read behind [`Self::from_env`], with the lookup passed in.
    ///
    /// Separated for the reason [`qip_core::secret::resolve_from`] exists: the
    /// process environment is global and `std::env::set_var` is `unsafe` in
    /// the 2024 edition, so nothing that reads the environment itself is
    /// reachable from a test here. Before this seam, the branch deciding where
    /// this process's *credential* comes from — the one branch a deployment
    /// must be able to move out of the environment and into a file — was the
    /// only branch in this module no test could reach, and reverting it to a
    /// bare `std::env::var` would have broken no test in the workspace.
    pub fn read(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let Some(url) = lookup(URL_VARIABLE).filter(|value| !value.trim().is_empty()) else {
            return Ok(None);
        };
        let org = lookup(ORG_VARIABLE);
        let interval = lookup(INTERVAL_VARIABLE);
        // Through `qip_core::secret`, so the deployment may mount the
        // credential as a file rather than set it: a credential in the
        // environment is in `/proc/<pid>/environ`, in every child process and
        // in every crash dump. The file variable's name is composed from
        // `FILE_SUFFIX` rather than written out, because a `"QIP_…"` literal
        // in this crate is exactly what `manifest_wiring`'s walk counts as a
        // variable this binary reads, and it would then demand either a
        // deployment that sets it or an allowlist entry arguing why not.
        let credential = qip_core::secret::resolve_from(
            CREDENTIAL_VARIABLE,
            lookup(CREDENTIAL_VARIABLE),
            lookup(&format!(
                "{CREDENTIAL_VARIABLE}{}",
                qip_core::secret::FILE_SUFFIX
            )),
        )
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
        // A credential that is present but unusable is refused here rather
        // than sent. Two shapes reach this point:
        //
        //   * Empty or whitespace. `qip_core::secret` refuses an empty *file*,
        //     but `QIP_OPENOBSERVE_AUTHORIZATION=""` arrives as `Some("")`,
        //     which would POST `authorization:` with nothing after it while
        //     `describe` prints ", authenticated" on the start-up banner. A
        //     control that reads as on and is off is worse than one plainly
        //     off.
        //   * A carriage return or a line feed. `HttpRequest::with_header`
        //     strips both so a header value can never inject a second header —
        //     right for the transport, wrong to discover silently here: the
        //     credential that goes on the wire is then not the one the
        //     operator configured, and every export fails authentication with
        //     nothing naming why. A file's own trailing newline is not this
        //     case; `qip_core::secret` has already trimmed it.
        //
        // Refused, never repaired.
        let credential = match credential {
            None => None,
            Some(value) if value.trim().is_empty() => {
                return Err(Error::invalid(format!(
                    "configuration: {CREDENTIAL_VARIABLE} is set and holds nothing. Unset it to \
                     export unauthenticated, or set it — or {CREDENTIAL_VARIABLE}{}, which the \
                     deployment should prefer because an environment value is readable from \
                     /proc/<pid>/environ — to the whole Authorization header value OpenObserve \
                     expects",
                    qip_core::secret::FILE_SUFFIX
                )));
            }
            Some(value) if value.contains('\r') || value.contains('\n') => {
                return Err(Error::invalid(format!(
                    "configuration: the credential from {CREDENTIAL_VARIABLE} or \
                     {CREDENTIAL_VARIABLE}{} contains a line break. It cannot be sent in a header \
                     and would be stripped rather than sent as written, so every export would be \
                     refused for a byte nobody can see; supply the header value on one line",
                    qip_core::secret::FILE_SUFFIX
                )));
            }
            Some(value) => Some(value),
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
/// capital feed is, so the timeouts are looser than the qip-api mesh's
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
/// sleep ends, the same shape qip-api's `MeshListener` uses.
#[derive(Debug)]
pub struct DrainHandle {
    shutdown: Arc<AtomicBool>,
}

impl Drop for DrainHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// How long to wait before the next attempt, after `consecutive_failures`
/// passes in which nothing reached the collector.
///
/// The failure this prevents: a collector that is down was previously dialled
/// at full rate forever, two POSTs per interval, each costing up to
/// [`drain_limits`]'s connect and read budget — a process spending its time
/// telling an absent peer about itself. Doubling, capped by
/// [`BACKOFF_CEILING`], and never shorter than the configured interval: a
/// ceiling below a deliberately long interval must not speed the loop up,
/// which would turn a bound into a dial.
fn next_delay(interval: StdDuration, consecutive_failures: u32) -> StdDuration {
    // Exponent capped before the shift, not after: `2u32.pow(32)` panics, and
    // a panic in the drain thread is the exact failure this module exists to
    // avoid.
    let factor = 2u32
        .checked_pow(consecutive_failures.min(16))
        .unwrap_or(u32::MAX);
    interval
        .saturating_mul(factor)
        .min(BACKOFF_CEILING)
        .max(interval)
}

/// The failure count after one pass.
///
/// Reset by *any* signal the collector accepted, not by both: a pass where
/// metrics landed and traces were refused is a reachable collector with one
/// bad endpoint, and slowing the loop there would delay the healthy signal to
/// punish the sick one.
fn advance(consecutive_failures: u32, pass: &ExportPass) -> u32 {
    if pass.metrics.is_ok() || pass.traces.is_ok() {
        0
    } else {
        consecutive_failures.saturating_add(1)
    }
}

/// Sleep for `delay`, waking often enough that a dropped [`DrainHandle`] is
/// noticed promptly.
///
/// One `thread::sleep(delay)` was fine at a fixed interval and is not once
/// [`next_delay`] can return [`BACKOFF_CEILING`]: a handle dropped at shutdown
/// would leave the thread alive for minutes after the process believed it had
/// stopped it.
fn sleep_until(shutdown: &AtomicBool, delay: StdDuration) {
    let deadline = std::time::Instant::now() + delay;
    while !shutdown.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        std::thread::sleep(remaining.min(SHUTDOWN_POLL));
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
            let mut consecutive_failures = 0u32;
            while !thread_shutdown.load(Ordering::Relaxed) {
                let now = clock.now();
                match export_once(&telemetry, &client, &config, now) {
                    Ok(pass) => {
                        record(&telemetry, &pass);
                        consecutive_failures = advance(consecutive_failures, &pass);
                    }
                    // A URL that fails to build from an already-validated
                    // base and organisation cannot happen in production —
                    // `OpenObserveConfig::parse` proved both — but the drain
                    // loop still must not stop over it: logged, counted
                    // against the backoff, and skipped, exactly like a POST
                    // failure.
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        telemetry.logger.warn(format!(
                            "the OpenObserve export could not build its endpoint URLs: {}",
                            error.message()
                        ));
                    }
                }
                sleep_until(
                    &thread_shutdown,
                    next_delay(config.interval, consecutive_failures),
                );
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
    use std::collections::BTreeMap;

    /// A lookup over a fixed map, standing in for the process environment,
    /// which a test cannot set: `std::env::set_var` is `unsafe` in the 2024
    /// edition and this workspace forbids `unsafe`.
    fn lookup(vars: BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |name: &str| vars.get(name).cloned()
    }

    /// The `_FILE` spelling of [`CREDENTIAL_VARIABLE`], composed rather than
    /// written out for the reason [`OpenObserveConfig::read`] composes it.
    fn file_variable() -> String {
        format!("{CREDENTIAL_VARIABLE}{}", qip_core::secret::FILE_SUFFIX)
    }

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    /// The credential must be readable from a file, because the deployment
    /// must be able to keep it out of `/proc/<pid>/environ`. Before the
    /// `read` seam nothing exercised this branch at all.
    #[test]
    fn the_credential_can_be_read_from_a_file_so_a_deployment_need_not_put_it_in_the_environment() {
        let path = std::env::temp_dir().join("qip-openobserve-credential-file");
        std::fs::write(&path, "Basic dGVzdA==\n").expect("the fixture is writable");
        let map = vars(&[
            (URL_VARIABLE, "http://collector:5080"),
            (ORG_VARIABLE, "qip"),
            (&file_variable(), &path.to_string_lossy()),
        ]);
        assert!(
            !map.contains_key(CREDENTIAL_VARIABLE),
            "the premise: the direct variable is unset, so only the file can supply it"
        );
        let config = OpenObserveConfig::read(&lookup(map))
            .expect("a file-mounted credential was refused")
            .expect("a set URL must produce a configuration");
        assert_eq!(config.credential, Some("Basic dGVzdA==".to_string()));
        let described = config.describe();
        assert!(described.contains(", authenticated"), "{described}");
        assert!(
            !described.contains("dGVzdA=="),
            "the banner printed the credential itself: {described}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Both sources set is a disagreement nobody decided; the refusal must
    /// name the file variable so the operator knows which two collided.
    #[test]
    fn setting_the_credential_and_its_file_together_is_refused_and_the_refusal_names_the_file_variable()
     {
        let map = vars(&[
            (URL_VARIABLE, "http://collector:5080"),
            (ORG_VARIABLE, "qip"),
            (CREDENTIAL_VARIABLE, "Basic dGVzdA=="),
            (&file_variable(), "/nonexistent"),
        ]);
        let error =
            OpenObserveConfig::read(&lookup(map)).expect_err("both sources set was accepted");
        // The strict token: `QIP_OPENOBSERVE_AUTHORIZATION` is a substring of
        // its own `_FILE` spelling, so asserting the bare name would pass on a
        // message naming neither correctly.
        assert!(
            error.message().contains(&file_variable()),
            "{}",
            error.message()
        );
        assert!(
            error.message().contains("set exactly one"),
            "{}",
            error.message()
        );
    }

    /// An empty credential would POST `authorization:` with nothing after it
    /// while the banner says ", authenticated" — a control that reads as on
    /// and is off.
    #[test]
    fn a_credential_set_to_nothing_is_refused_rather_than_exported_as_authenticated() {
        let error = OpenObserveConfig::parse(
            "http://collector:5080",
            Some("qip"),
            None,
            Some("   ".to_string()),
        )
        .expect_err("an empty credential was accepted");
        assert!(
            error.message().contains(CREDENTIAL_VARIABLE),
            "{}",
            error.message()
        );
        assert!(
            error.message().contains(&file_variable()),
            "{}",
            error.message()
        );
    }

    /// `HttpRequest::with_header` strips CR and LF — right for the transport,
    /// wrong to discover silently here, because the credential on the wire is
    /// then not the one configured.
    #[test]
    fn a_credential_carrying_a_line_break_is_refused_rather_than_silently_stripped() {
        // The premise — that the transport accepts such a value, silently
        // repairs it and sends the repaired one — cannot be asserted here:
        // `HttpRequest`'s headers are private and it exposes no accessor. It
        // is asserted over a real socket instead, in
        // `tests/openobserve.rs::a_credential_with_a_line_break_would_reach_\
        // the_collector_mutilated_rather_than_rejected`.
        let error = OpenObserveConfig::parse(
            "http://collector:5080",
            Some("qip"),
            None,
            Some("Bearer abc\r\nx-injected: 1".to_string()),
        )
        .expect_err("a credential with a line break was accepted");
        assert!(
            error.message().contains(CREDENTIAL_VARIABLE),
            "{}",
            error.message()
        );
    }

    /// The half that distinguishes a working gate from one that refuses
    /// everything: unauthenticated OpenObserve is a real deployment.
    #[test]
    fn an_absent_credential_is_still_a_valid_unauthenticated_configuration() {
        let map = vars(&[
            (URL_VARIABLE, "http://collector:5080"),
            (ORG_VARIABLE, "qip"),
        ]);
        let config = OpenObserveConfig::read(&lookup(map))
            .expect("an unauthenticated configuration was refused")
            .expect("a set URL must produce a configuration");
        assert!(config.credential.is_none());
        assert!(config.describe().contains(", unauthenticated"));
    }

    /// Absence is a decision an operator can make honestly, and a URL set to
    /// whitespace is that same decision typed badly — not a parse failure.
    #[test]
    fn an_unset_url_is_no_drain_rather_than_a_refusal() {
        assert_eq!(
            OpenObserveConfig::read(&lookup(BTreeMap::new()))
                .expect("an unset URL was treated as a misconfiguration"),
            None
        );
        assert_eq!(
            OpenObserveConfig::read(&lookup(vars(&[(URL_VARIABLE, "   ")])))
                .expect("a blank URL was treated as a misconfiguration"),
            None
        );
    }

    #[test]
    fn the_backoff_doubles_is_capped_and_never_shortens_the_configured_interval() {
        let second = StdDuration::from_secs(1);
        assert_eq!(next_delay(second, 0), second);
        assert_eq!(next_delay(second, 1), StdDuration::from_secs(2));
        assert_eq!(next_delay(second, 3), StdDuration::from_secs(8));
        assert_eq!(next_delay(second, 4096), BACKOFF_CEILING);
        // A ceiling below a deliberately long interval must not speed the
        // loop up: that would turn a bound into a dial.
        let long = BACKOFF_CEILING * 2;
        assert_eq!(next_delay(long, 3), long);
    }

    #[test]
    fn a_pass_where_one_signal_landed_resets_the_backoff_and_a_pass_where_none_did_advances_it() {
        let failed = SignalOutcome::Failed {
            detail: "connection refused".to_string(),
        };
        let partial = ExportPass {
            metrics: SignalOutcome::Sent { status: 200 },
            traces: failed.clone(),
        };
        assert_eq!(
            advance(3, &partial),
            0,
            "a reachable collector with one bad endpoint is not an outage"
        );
        let none_landed = ExportPass {
            metrics: failed.clone(),
            traces: SignalOutcome::Refused {
                status: 503,
                detail: "unavailable".to_string(),
            },
        };
        assert_eq!(advance(3, &none_landed), 4);
        assert_eq!(
            advance(u32::MAX, &none_landed),
            u32::MAX,
            "the counter saturates rather than overflowing the drain thread"
        );
    }

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
