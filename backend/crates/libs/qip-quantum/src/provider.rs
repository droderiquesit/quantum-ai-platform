//! The quantum provider port, and a hosted adapter that reaches IBM Quantum.
//!
//! Everything downstream is written against [`QuantumProvider`]. Two
//! implementations exist: [`SimulatedProvider`], which runs the in-tree
//! statevector simulator, and [`HostedProvider`], which talks to a hosted
//! service over its JSON REST API — or refuses, loudly and specifically, when
//! this deployment has not been given what it needs to.
//!
//! # Simulated and hardware results stay distinguishable, by refusal
//!
//! [`ProviderCapabilities::simulated`] is the flag that decides whether a
//! result may ever be presented as hardware evidence, and it is `false` on
//! every [`HostedProvider`]. Keeping that honest is the constraint the whole
//! adapter is arranged around, because the failure it guards against is not
//! subtle: a deployment points the backend at `ibmq_qasm_simulator`, the runs
//! come back, and a report says a quantum device produced them.
//!
//! Configuration cannot settle it — a `backend` field is a string somebody
//! typed, and a name is not evidence. So the adapter **asks the service**. Before
//! it will submit anything it reads the backend's own configuration, which
//! carries the vendor's `simulator` flag, and:
//!
//! * if the service says the backend is a simulator, [`HostedProvider::solve_qubo`]
//!   **refuses** rather than returning a result. It does not flip its own
//!   `simulated` flag and carry on: a caller that read the capabilities before
//!   the run would then be holding `simulated: false` beside a simulated
//!   answer, which is precisely the confusion this is here to prevent. A
//!   deployment that wants a simulation has [`SimulatedProvider`], which says
//!   so in its capabilities and never pretends otherwise.
//! * if the service names a different backend from the configured one, it
//!   refuses too. A device nobody chose is not a device anyone can reason
//!   about — coupling map, basis gates and error rates all differ.
//!
//! `simulated: false` is therefore not a claim about the endpoint. It is a
//! statement about every result this adapter has ever returned, enforced by
//! the fact that it returns none from anything the service called a simulator.
//! [`HostedProvider::confirmed_device`] publishes what the service actually
//! said, so the provenance is readable rather than implied.
//!
//! # The classical baseline rule is untouched
//!
//! `qip-quantum`'s rule is that no quantum result is used without a classical
//! baseline solved on the same problem. Nothing here weakens it. A
//! [`QaoaResult`] from this adapter is a *candidate*, exactly as one from the
//! simulator is; the router in `qip-optimization-engine` and
//! [`crate::benchmark`] still require the baseline, and
//! [`crate::benchmark::ValidatedSolution`] still re-evaluates the assignment
//! classically before anything may use it.
//!
//! This adapter adds a check of its own on the way in, for a reason specific to
//! a remote peer: the energy in the response is a number a service sent, and
//! this process can recompute it from the assignment for the cost of one pass
//! over the QUBO. So it does, and a claim that does not match the assignment is
//! refused here rather than carried forward to be caught later. That is
//! belt-and-braces with the validator downstream, deliberately.
//!
//! # Authentication is a port, not a fake
//!
//! The adapter **takes a token it is given** ([`HostedToken`]) and refuses
//! clearly when it has none. It does not read the environment, mint a
//! credential, or sign anything: ADR 0009 permits `serde` and `serde_json` and
//! nothing else, so there is no crypto in this workspace to sign with.
//! [`HostedConfig::credential_env`] therefore names *where an operator's token
//! lives*, and remains a note for the operator rather than something this code
//! reads.
//!
//! The token is held in a type that redacts in `Debug` and implements neither
//! `Serialize` nor `Deserialize`, so a struct holding one cannot derive them.
//!
//! # It needs a TLS-terminating proxy, and says so
//!
//! `qip_transport::http` has no TLS stack and refuses `https` by name rather
//! than downgrading it, and IBM Quantum is `https` only. A deployment puts a
//! TLS-terminating egress proxy in front of this adapter and points
//! [`HostedTransport::base_url`] at it over `http` on the cluster network. That
//! is a production requirement, it is listed in
//! [`HostedProvider::production_requirements`], and it does not go away when
//! everything else is configured.
//!
//! # What this module does not promise
//!
//! * **It does not compile circuits.** The job it submits names a runtime
//!   program the *deployment* publishes, and the wire schema below is the whole
//!   of what this adapter will read back. A deployment whose service does not
//!   speak it points this adapter at a translating endpoint; guessing at a
//!   payload shape would produce a result nobody could check.
//! * **It does not wait indefinitely.** Polling is bounded by
//!   [`HostedTransport::max_polls`], and exhausting that budget is an error
//!   naming the job id — never a result. A job that is still queued when the
//!   budget runs out is still queued, still billing, and the operator is told
//!   which one it is.
//! * **It does not fall back.** There is no path from here into the in-tree
//!   simulator. A hardware path that degraded to a simulation would produce
//!   exactly the mislabelled result the first section is about.
//! * **It reads no clock.** Waiting between polls goes through a
//!   [`Sleeper`], so a test can drive the whole loop without spending the
//!   wall-clock time a real queue would.

use crate::qaoa::{QaoaResult, QaoaSettings, solve};
use qip_core::error::{Error, Result};
use qip_core::rng::Xoshiro256;
use qip_core::time::Duration;
use qip_numerics::anneal::Qubo;
use qip_transport::retry::{Sleeper, ThreadSleeper};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, HttpResponse, Method, Url};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

/// What a provider can do and what it costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Largest problem the provider will accept.
    pub max_qubits: usize,
    /// Whether the provider is a simulator. Reported so a result can never be
    /// presented as hardware evidence when it is not.
    pub simulated: bool,
    /// Whether results carry device noise.
    pub noisy: bool,
    /// Typical queue delay before a job starts.
    pub typical_queue: Duration,
    /// Cost per job in USD micro-units.
    pub cost_per_job_micros: u64,
}

impl ProviderCapabilities {
    pub fn simulator(max_qubits: usize) -> Self {
        Self {
            max_qubits,
            simulated: true,
            noisy: false,
            typical_queue: Duration::ZERO,
            cost_per_job_micros: 0,
        }
    }
}

/// A quantum backend.
pub trait QuantumProvider: Send + Sync + fmt::Debug {
    fn name(&self) -> &str;

    /// Whether this deployment can actually reach the provider.
    fn is_available(&self) -> bool;

    fn capabilities(&self) -> ProviderCapabilities;

    /// Solve a QUBO, or explain why not.
    fn solve_qubo(&self, qubo: &Qubo, settings: &QaoaSettings) -> Result<QaoaResult>;

    /// What an operator would have to provide to make this usable.
    ///
    /// Empty when the provider is already available.
    fn requirement(&self) -> String {
        String::new()
    }
}

/// The in-tree statevector simulator.
#[derive(Debug)]
pub struct SimulatedProvider {
    seed: u64,
    max_qubits: usize,
}

impl SimulatedProvider {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            max_qubits: crate::statevector::MAX_QUBITS,
        }
    }

    /// Restrict the simulator further, e.g. to keep a test fast.
    pub fn with_max_qubits(mut self, max_qubits: usize) -> Self {
        self.max_qubits = max_qubits.min(crate::statevector::MAX_QUBITS);
        self
    }
}

impl QuantumProvider for SimulatedProvider {
    fn name(&self) -> &str {
        "statevector-simulator"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::simulator(self.max_qubits)
    }

    fn solve_qubo(&self, qubo: &Qubo, settings: &QaoaSettings) -> Result<QaoaResult> {
        if qubo.n > self.max_qubits {
            return Err(Error::invalid(format!(
                "{} variables exceeds the simulator's {} qubit limit; a statevector needs 2^n amplitudes",
                qubo.n, self.max_qubits
            )));
        }
        // Seeded per problem size so a repeated run reproduces exactly.
        let mut rng = Xoshiro256::seeded(self.seed).fork(&format!("qaoa-{}", qubo.n));
        solve(qubo, settings, &mut rng)
    }
}
/// How a hosted provider is configured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostedConfig {
    /// Vendor name, e.g. `ibm-quantum`.
    pub vendor: String,
    /// Backend or device name.
    pub backend: String,
    /// Environment variable holding the API token. The token itself is never
    /// stored in configuration or in a repository — and it is never read here
    /// either: this names where an operator's token lives, and the token
    /// reaches the adapter as a [`HostedToken`] somebody hands it.
    pub credential_env: String,
    /// API endpoint. A note for an operator: the address this adapter actually
    /// dials is [`HostedTransport::base_url`], which is the TLS-terminating
    /// proxy in front of this one.
    pub endpoint: String,
    pub max_qubits: usize,
    pub cost_per_job_micros: u64,
}

/// An API token this process was handed.
///
/// Not read from the environment and not minted here. A "token provider"
/// written in this crate would have to either shell out or fabricate a value,
/// and a fabricated token is not a shortcut to a working client — it is a
/// client that gets a 401 and has to be debugged twice.
///
/// `Debug` is written by hand and redacts. `Serialize` and `Deserialize` are
/// not implemented at all, which is the stronger statement: a struct holding
/// one cannot derive them either, so the compiler refuses the snapshot rather
/// than emitting one with a token in it.
#[derive(Clone)]
pub struct HostedToken(String);

impl HostedToken {
    /// Wrap a resolved token.
    ///
    /// Refuses blank, because a secret manager that resolved nothing writes an
    /// empty string rather than failing, and an empty `Authorization` header is
    /// the failure that looks exactly like an expired credential. Refuses
    /// control characters, because a header value carrying one ends the header
    /// and lets the rest be read as another.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::invalid(
                "the hosted provider's API token is blank. An unresolved token is absent rather \
                 than empty, so that the adapter reports itself unavailable instead of sending an \
                 empty Authorization header",
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(Error::invalid(
                "the hosted provider's API token contains a control character; sent as a header \
                 value it would end the header and let the rest be read as another one",
            ));
        }
        Ok(Self(value))
    }

    /// Hand the value to a transport writing an authentication header.
    ///
    /// Named to be conspicuous: a reviewer scanning a diff should see the word
    /// at every point the token leaves this type.
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HostedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostedToken(<redacted>)")
    }
}

/// How this process reaches the hosted service.
///
/// Separate from [`HostedConfig`] because the two answer different questions:
/// the config says which device to run on and is checked into a deployment, the
/// transport says how to get there and carries a secret that must not be.
#[derive(Debug)]
pub struct HostedTransport {
    /// `http://host[:port]` of the **TLS-terminating egress proxy** in front of
    /// the vendor's API.
    ///
    /// `http`, not `https`, and that is not an oversight: `qip_transport::http`
    /// has no TLS stack and refuses `https` by name rather than downgrading it.
    /// Pointing this straight at the vendor fails to parse, which is the right
    /// failure — the alternative is an API token crossing the internet in clear
    /// text.
    pub base_url: String,
    /// The token, redacted in every printed form.
    pub token: HostedToken,
    /// The runtime program the deployment has published, which this adapter
    /// submits by name. Named rather than assumed because the payload it reads
    /// back is this adapter's schema, and only a program the deployment
    /// controls emits it.
    pub program_id: String,
    /// What this process will wait for and hold.
    pub limits: ClientLimits,
    /// How long to wait between polls of a queued job.
    pub poll_interval: Duration,
    /// How many times to poll before giving up.
    ///
    /// Giving up is an error naming the job, never a result. A shared device's
    /// queue can be hours long, and a client that waited for it would hold a
    /// thread for hours; a deployment that wants to wait that long raises this
    /// deliberately and knows what it costs.
    pub max_polls: u32,
    /// What does the waiting. A [`Sleeper`] rather than `thread::sleep` so a
    /// test drives the whole queue loop without spending the time.
    pub sleeper: Arc<dyn Sleeper>,
}

impl HostedTransport {
    /// A transport with limits sized for a job-control API.
    pub fn new(base_url: impl Into<String>, token: HostedToken) -> Self {
        Self {
            base_url: base_url.into(),
            token,
            program_id: DEFAULT_PROGRAM_ID.to_string(),
            limits: ClientLimits {
                max_body: 1024 * 1024,
                connect_timeout: StdDuration::from_secs(5),
                read_timeout: StdDuration::from_secs(20),
                write_timeout: StdDuration::from_secs(20),
                ..ClientLimits::default()
            },
            poll_interval: Duration::from_secs(5),
            max_polls: 120,
            sleeper: Arc::new(ThreadSleeper),
        }
    }
}

/// The runtime program this adapter submits unless a deployment names another.
const DEFAULT_PROGRAM_ID: &str = "qip-qaoa";

/// What the service said about the device a job would run on.
///
/// Read from the service before anything is submitted, and the reason
/// [`ProviderCapabilities::simulated`] can be `false` without that being a
/// claim nobody checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedDevice {
    /// The backend name the service reported, which must match the configured
    /// one.
    pub backend: String,
    /// The vendor's own answer to "is this a simulator". A `true` here makes
    /// this adapter refuse to run, rather than run and relabel itself.
    pub simulator: bool,
    /// Qubits the service says the device has, which bounds a problem
    /// independently of what `max_qubits` was configured as.
    pub qubits: usize,
}

/// What this adapter has done, for metrics and for tests that assert a request
/// happened rather than assuming it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostedStats {
    /// Backend confirmations that reached the service.
    pub device_confirmations: u64,
    /// Jobs submitted.
    pub jobs_submitted: u64,
    /// Poll requests that left the process.
    pub polls_sent: u64,
    /// Results this adapter read, parsed and re-evaluated classically.
    pub results_accepted: u64,
    /// Runs refused because the service attributed the backend to a simulator.
    /// The number that says a deployment has pointed hardware configuration at
    /// a simulator, which is the mislabelling this module exists to prevent.
    pub simulator_refusals: u64,
    /// Results refused because the claimed energy did not match the assignment.
    pub arithmetic_refusals: u64,
    /// Jobs abandoned because the poll budget ran out. Each one may still be
    /// queued and still be billing.
    pub poll_budgets_exhausted: u64,
}

/// An adapter to a hosted quantum service.
///
/// Constructed one of two ways, and the difference is the whole point:
/// [`Self::new`] and [`Self::with_credential`] build a port that has no
/// transport and refuses every call, and [`Self::connected`] builds one that
/// opens sockets. There is no configuration that turns the first into the
/// second, and no fallback that turns the second into the in-tree simulator.
#[derive(Debug)]
pub struct HostedProvider {
    config: HostedConfig,
    /// Whether a credential was found. Injected rather than read from the
    /// environment here, so the same code path is exercised in a test. A
    /// connected provider holds a token, which is a credential by construction.
    credential_present: bool,
    /// The transport, when there is one. `None` is the port: it reports
    /// unavailable, names what is missing, and opens nothing.
    transport: Option<HostedTransport>,
    client: HttpClient,
    /// What the service said about the device, once it has been asked.
    ///
    /// Behind a `Mutex` because [`QuantumProvider::solve_qubo`] takes `&self`
    /// and the trait is `Send + Sync`. Poisoning is recovered from rather than
    /// propagated: a panic in another thread is not a reason to stop being able
    /// to report which device answered.
    device: Mutex<Option<ConfirmedDevice>>,
    stats: Mutex<HostedStats>,
}

impl HostedProvider {
    /// The port: no transport, and every call refused.
    pub fn new(config: HostedConfig) -> Self {
        Self {
            config,
            credential_present: false,
            transport: None,
            client: HttpClient::new(ClientLimits::default()),
            device: Mutex::new(None),
            stats: Mutex::new(HostedStats::default()),
        }
    }

    /// Construct with a credential, for testing the availability logic. The
    /// transport still is not present.
    pub fn with_credential(config: HostedConfig, present: bool) -> Self {
        Self {
            credential_present: present,
            ..Self::new(config)
        }
    }

    /// The adapter that actually reaches the service.
    ///
    /// Fails on a base URL that cannot be parsed — which includes an `https`
    /// one, because this transport has no TLS and will not pretend otherwise.
    /// Opens nothing: whether the proxy is up and the token accepted is settled
    /// by the first request.
    pub fn connected(config: HostedConfig, transport: HostedTransport) -> Result<Self> {
        Url::parse(&transport.base_url).map_err(|error| {
            Error::invalid(format!(
                "the {} endpoint {:?} cannot be used: {error}. It must be the \
                 `http://host[:port]` of a TLS-terminating egress proxy in front of the vendor's \
                 API — this transport has no TLS stack and refuses `https` by name rather than \
                 sending an API token in clear text",
                config.vendor, transport.base_url
            ))
        })?;
        if transport.max_polls == 0 {
            return Err(Error::invalid(
                "max_polls is zero, which would abandon every job before its first poll and bill \
                 for results nobody ever read",
            ));
        }
        let client = HttpClient::new(transport.limits);
        Ok(Self {
            config,
            credential_present: true,
            transport: Some(transport),
            client,
            device: Mutex::new(None),
            stats: Mutex::new(HostedStats::default()),
        })
    }

    pub fn config(&self) -> &HostedConfig {
        &self.config
    }

    pub fn stats(&self) -> HostedStats {
        *self.lock_stats()
    }

    /// Whether this adapter can put bytes on a wire at all.
    pub const fn has_transport(&self) -> bool {
        self.transport.is_some()
    }

    /// What the service said about the device, once it has been asked.
    ///
    /// `None` before the first run. This is the provenance a report should
    /// carry beside a result: the backend that answered, and the vendor's own
    /// statement that it is not a simulator.
    pub fn confirmed_device(&self) -> Option<ConfirmedDevice> {
        self.device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// What a deployment still owes even when the adapter is reachable.
    pub fn production_requirements(&self) -> Vec<String> {
        vec![
            "a TLS-terminating egress proxy in front of this adapter: `qip_transport::http` has \
             no TLS stack and refuses `https` by name, so an API token sent straight to the \
             vendor would cross the internet in clear text"
                .to_string(),
            format!(
                "a published runtime program on {} that accepts this adapter's job payload and \
                 emits its result schema. The circuit is not compiled here, and a payload guessed \
                 at would produce a result nobody could check",
                self.config.vendor
            ),
            "a budget and an alert on queue time: polling is bounded, and a job still queued when \
             the budget runs out is still queued and still billing"
                .to_string(),
            "a classical baseline solved on the same problem. This crate's rule is that no \
             quantum result is used without one, and this adapter produces candidates rather than \
             answers — `qip_quantum::benchmark::ValidatedSolution` is where a candidate becomes \
             usable"
                .to_string(),
        ]
    }

    fn lock_stats(&self) -> std::sync::MutexGuard<'_, HostedStats> {
        self.stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // --- the wire ----------------------------------------------------------

    fn transport(&self) -> Result<&HostedTransport> {
        self.transport
            .as_ref()
            .ok_or_else(|| Error::unavailable(self.requirement()))
    }

    fn url(&self, path: &str) -> Result<String> {
        let base = Url::parse(&self.transport()?.base_url).map_err(Error::from)?;
        Ok(base.with_path(path).map_err(Error::from)?.to_string())
    }

    fn authenticated(
        &self,
        method: Method,
        target: &str,
        body: Option<Vec<u8>>,
    ) -> Result<HttpRequest> {
        let transport = self.transport()?;
        let request = match body {
            Some(body) => HttpRequest::json(method, target, body).map_err(Error::from)?,
            None => HttpRequest::new(method, target).map_err(Error::from)?,
        };
        Ok(request
            .with_header(
                "authorization",
                &format!("Bearer {}", transport.token.expose()),
            )
            .with_header("accept", "application/json"))
    }

    /// Send, and refuse anything that is not a success.
    ///
    /// A non-2xx is never data here. The vendor's error bodies carry a message
    /// worth surfacing, bounded so a log stays readable at the moment it is
    /// needed.
    fn fetch(&self, request: &HttpRequest, what: &str) -> Result<HttpResponse> {
        let response = self.client.send(request).map_err(Error::from)?;
        if !response.is_success() {
            return Err(self.status_refusal(response.status, what, &response.body_excerpt()));
        }
        Ok(response)
    }

    fn status_refusal(&self, status: u16, what: &str, excerpt: &str) -> Error {
        let vendor = &self.config.vendor;
        match status {
            401 => Error::denied(format!(
                "{vendor} refused the API token while {what} (HTTP 401). This adapter takes a \
                 token it is given and cannot mint or refresh one, so this is a token the \
                 deployment must renew. The token is not quoted here and is not written to any \
                 log by this adapter: {excerpt}"
            )),
            403 => Error::denied(format!(
                "{vendor} refused the request while {what} (HTTP 403). The token authenticated \
                 and this account is not entitled to {}: {excerpt}",
                self.config.backend
            )),
            404 => Error::not_found(format!(
                "{vendor} has no such resource while {what} (HTTP 404): {excerpt}. This is not \
                 evidence that a job does not exist"
            )),
            408 | 429 => Error::unavailable(format!(
                "{vendor} is rate-limiting or timing out this deployment while {what} (HTTP \
                 {status}): {excerpt}"
            )),
            500..=599 => Error::unavailable(format!(
                "{vendor} failed to serve the request while {what} (HTTP {status}): {excerpt}"
            )),
            other => Error::invalid(format!(
                "{vendor} answered HTTP {other} while {what}, which this adapter will not read \
                 data from: {excerpt}"
            )),
        }
    }

    /// Ask the service what the configured backend actually is.
    ///
    /// The check that keeps [`ProviderCapabilities::simulated`] honest. Cached
    /// after the first answer, because a device does not become a simulator
    /// between two jobs and asking again on every run would spend a request to
    /// re-learn a constant.
    fn confirm_device(&self) -> Result<ConfirmedDevice> {
        if let Some(known) = self.confirmed_device() {
            return Ok(known);
        }
        let target = self.url(&format!(
            "/api/v1/backends/{}/configuration",
            self.config.backend
        ))?;
        let request = self.authenticated(Method::Get, &target, None)?;
        let response = self.fetch(&request, "confirming the backend")?;
        self.lock_stats().device_confirmations += 1;

        let body = response.body_as_str().map_err(Error::from)?;
        let wire: WireBackend = serde_json::from_str(body).map_err(|error| {
            Error::schema(format!(
                "{} answered with a backend configuration this adapter cannot read: {error}. \
                 Without it there is no statement from the service about whether {} is a \
                 simulator, and this adapter will not assume one. The first bytes were: {}",
                self.config.vendor,
                self.config.backend,
                response.body_excerpt()
            ))
        })?;

        if wire.backend_name != self.config.backend {
            return Err(Error::guard(format!(
                "{} answered a request about {} with a configuration for {}. A device nobody \
                 chose is a device nobody can reason about — coupling map, basis gates and error \
                 rates all differ — so nothing is submitted",
                self.config.vendor, self.config.backend, wire.backend_name
            )));
        }
        let device = ConfirmedDevice {
            backend: wire.backend_name,
            simulator: wire.simulator,
            qubits: wire.n_qubits,
        };
        *self
            .device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(device.clone());
        Ok(device)
    }

    /// Submit one job and return the identifier the service issued.
    fn submit_job(&self, qubo: &Qubo, settings: &QaoaSettings) -> Result<String> {
        let transport = self.transport()?;
        let body = serde_json::to_vec(&WireSubmit {
            program_id: &transport.program_id,
            backend: &self.config.backend,
            params: WireParams {
                qubo: WireQubo {
                    n: qubo.n,
                    entries: qubo.entries.clone(),
                    offset: qubo.offset,
                },
                layers: settings.layers,
                optimiser_iterations: settings.optimiser_iterations,
                shots: settings.shots,
            },
        })
        .map_err(|error| Error::schema(format!("this job cannot be written as JSON: {error}")))?;

        let target = self.url("/api/v1/jobs")?;
        let request = self.authenticated(Method::Post, &target, Some(body))?;
        let response = self.fetch(&request, "submitting a job")?;
        self.lock_stats().jobs_submitted += 1;

        let text = response.body_as_str().map_err(Error::from)?;
        let wire: WireJob = serde_json::from_str(text).map_err(|error| {
            Error::schema(format!(
                "{} answered a submit with a body this adapter cannot read as a job: {error}. The \
                 first bytes were: {}",
                self.config.vendor,
                response.body_excerpt()
            ))
        })?;
        if wire.id.trim().is_empty() {
            return Err(Error::schema(format!(
                "{} accepted a job and named no identifier for it. A job nobody can name cannot \
                 be polled, cancelled or reconciled against a bill",
                self.config.vendor
            )));
        }
        // A job that came back attributed to another backend is a job on a
        // device nobody chose, and it is refused before a single poll.
        if let Some(reported) = wire.backend.as_deref()
            && reported != self.config.backend
        {
            return Err(Error::guard(format!(
                "{} accepted job {} for {reported} where it was submitted for {}. The result \
                 would carry the wrong device's noise and the wrong device's name",
                self.config.vendor, wire.id, self.config.backend
            )));
        }
        Ok(wire.id)
    }

    /// Poll until the service says the job finished, or the budget runs out.
    ///
    /// Exhausting the budget is an error naming the job, never a result: the
    /// job is still queued, still billing, and an operator needs its
    /// identifier to do anything about it.
    fn await_completion(&self, job_id: &str) -> Result<()> {
        let transport = self.transport()?;
        let target = self.url(&format!("/api/v1/jobs/{job_id}"))?;
        for attempt in 0..transport.max_polls {
            if attempt > 0 {
                transport.sleeper.sleep(transport.poll_interval);
            }
            let request = self.authenticated(Method::Get, &target, None)?;
            let response = self.fetch(&request, "polling a job")?;
            self.lock_stats().polls_sent += 1;

            let text = response.body_as_str().map_err(Error::from)?;
            let wire: WireJob = serde_json::from_str(text).map_err(|error| {
                Error::schema(format!(
                    "{} answered a poll with a body this adapter cannot read as a job: {error}. \
                     The first bytes were: {}",
                    self.config.vendor,
                    response.body_excerpt()
                ))
            })?;
            match wire.progress(&self.config.vendor, job_id)? {
                JobProgress::Waiting => {}
                JobProgress::Completed => return Ok(()),
            }
        }
        self.lock_stats().poll_budgets_exhausted += 1;
        Err(Error::timeout(format!(
            "{} job {job_id} on {} had not finished after {} poll(s). It is still queued or still \
             running, and it is still billing: this adapter reports the identifier rather than an \
             answer, because a result nobody received is not a result",
            self.config.vendor, self.config.backend, transport.max_polls
        )))
    }

    /// Read a finished job's result, and check it against the problem.
    ///
    /// The claimed energy is thrown away and recomputed from the assignment.
    /// It costs one pass over the QUBO and it catches a service that returned a
    /// result for a different problem, a truncated bit string, and a
    /// transcription error — none of which are visible from the number alone.
    fn read_result(
        &self,
        job_id: &str,
        qubo: &Qubo,
        settings: &QaoaSettings,
    ) -> Result<QaoaResult> {
        let target = self.url(&format!("/api/v1/jobs/{job_id}/results"))?;
        let request = self.authenticated(Method::Get, &target, None)?;
        let response = self.fetch(&request, "reading a job result")?;

        let text = response.body_as_str().map_err(Error::from)?;
        let wire: WireResult = serde_json::from_str(text).map_err(|error| {
            Error::schema(format!(
                "{} answered with a result this adapter cannot read: {error}. This adapter reads \
                 one schema and refuses the rest, because a half-parsed result is a number \
                 nobody checked. The first bytes were: {}",
                self.config.vendor,
                response.body_excerpt()
            ))
        })?;

        if wire.assignment.len() != qubo.n {
            return Err(Error::invalid(format!(
                "{} returned {} bit(s) for a {}-variable problem in job {job_id}. A truncated or \
                 padded assignment scores as a different problem's answer",
                self.config.vendor,
                wire.assignment.len(),
                qubo.n
            )));
        }
        if let Some(bad) = wire.assignment.iter().find(|bit| **bit > 1) {
            return Err(Error::invalid(format!(
                "{} returned {bad} in a binary assignment for job {job_id}",
                self.config.vendor
            )));
        }
        if !(0.0..=1.0).contains(&wire.success_probability) {
            return Err(Error::invalid(format!(
                "{} reported a success probability of {} for job {job_id}, which is not a \
                 probability",
                self.config.vendor, wire.success_probability
            )));
        }

        // The claim is discarded and the assignment re-scored. Finite
        // arithmetic reorders sums, so an exact match is not the bar; a claim
        // that misses by more than this was not arithmetic.
        let recomputed = qubo.evaluate(&wire.assignment);
        let tolerance = 1e-6 * recomputed.abs().max(1.0);
        if (recomputed - wire.energy).abs() > tolerance {
            self.lock_stats().arithmetic_refusals += 1;
            return Err(Error::numeric(format!(
                "{} claimed job {job_id} found an objective of {} and the same assignment scores \
                 {recomputed} against this problem. The claim is refused rather than recorded: a \
                 number that does not match the bits it came with is not evidence about anything",
                self.config.vendor, wire.energy
            )));
        }

        self.lock_stats().results_accepted += 1;
        Ok(QaoaResult {
            assignment: wire.assignment,
            // The recomputed value, not the claimed one. They agree to within
            // the tolerance above; recording the one this process computed is
            // what makes the objective reproducible from the assignment alone.
            energy: recomputed,
            expectation: wire.expectation,
            success_probability: wire.success_probability,
            layers: wire.layers.unwrap_or(settings.layers),
            angles: wire.angles,
            evaluations: wire.evaluations,
        })
    }
}

impl QuantumProvider for HostedProvider {
    fn name(&self) -> &str {
        &self.config.backend
    }

    fn is_available(&self) -> bool {
        self.credential_present && self.transport.is_some()
    }

    /// What this provider is, as far as anything downstream is concerned.
    ///
    /// `simulated` is `false` and stays `false`, and that is an invariant this
    /// adapter enforces rather than a claim it makes: [`Self::solve_qubo`]
    /// refuses to return a result from a backend the service itself calls a
    /// simulator. See the module documentation for why refusing is the right
    /// answer and relabelling is not.
    ///
    /// `max_qubits` is the smaller of what the deployment configured and what
    /// the service said the device has, once the device has been confirmed. A
    /// deployment that configured more than the hardware has should be refused
    /// by the number rather than by the vendor.
    fn capabilities(&self) -> ProviderCapabilities {
        let confirmed = self.confirmed_device();
        ProviderCapabilities {
            max_qubits: confirmed.as_ref().map_or(self.config.max_qubits, |device| {
                device.qubits.min(self.config.max_qubits)
            }),
            simulated: false,
            noisy: true,
            typical_queue: Duration::from_mins(30),
            cost_per_job_micros: self.config.cost_per_job_micros,
        }
    }

    /// Run one QUBO on the device, or say why not.
    ///
    /// The order matters and is the point: the device is confirmed *before*
    /// anything is submitted, so a deployment that has pointed hardware
    /// configuration at a simulator finds out without spending a job — and,
    /// much more importantly, without a simulated result existing anywhere for
    /// something downstream to mislabel.
    fn solve_qubo(&self, qubo: &Qubo, settings: &QaoaSettings) -> Result<QaoaResult> {
        if !self.is_available() {
            return Err(Error::unavailable(self.requirement()));
        }
        let device = self.confirm_device()?;
        if device.simulator {
            self.lock_stats().simulator_refusals += 1;
            return Err(Error::guard(format!(
                "{} reports that {} is a simulator, and this adapter's capabilities say \
                 simulated=false. Returning the result would make a simulated answer \
                 indistinguishable from a hardware one everywhere downstream, which is the one \
                 thing `ProviderCapabilities::simulated` exists to prevent. Nothing was \
                 submitted. Use `SimulatedProvider`, which reports itself as a simulator, or \
                 configure a device the vendor does not call one",
                self.config.vendor, self.config.backend
            )));
        }
        let ceiling = device.qubits.min(self.config.max_qubits);
        if qubo.n > ceiling {
            return Err(Error::invalid(format!(
                "{} variables exceeds the {} qubit(s) available on {}: the vendor reports {} and \
                 this deployment is configured for at most {}",
                qubo.n, ceiling, self.config.backend, device.qubits, self.config.max_qubits
            )));
        }

        let job_id = self.submit_job(qubo, settings)?;
        self.await_completion(&job_id)?;
        self.read_result(&job_id, qubo, settings)
    }

    /// What an operator would have to provide, and what a deployment still owes
    /// when it has.
    ///
    /// Never empty, even for a reachable adapter — unlike the trait's default,
    /// and for the same reason `qip_brokers::rest` deviates: a configured
    /// adapter still owes the standing requirements, and reporting nothing
    /// would read as "nothing left to do". Only consulted when the provider is
    /// unavailable, by the router and by the benchmark, so a non-empty answer
    /// here changes nothing downstream.
    fn requirement(&self) -> String {
        let mut missing = Vec::new();
        if !self.credential_present {
            missing.push(format!(
                "an API token in the environment variable {}, handed to this adapter as a \
                 `HostedToken` — it is not read from the environment here",
                self.config.credential_env
            ));
        }
        if self.transport.is_none() {
            missing.push(format!(
                "an HTTPS transport and the {} client, neither of which is present in this build",
                self.config.vendor
            ));
        }
        if missing.is_empty() {
            return format!(
                "{} on {} is reachable and still incomplete: it needs {}. Nothing falls back to \
                 the classical solver from here — a hardware path that degraded to a simulation \
                 would produce exactly the mislabelled result this adapter refuses.",
                self.config.backend,
                self.config.endpoint,
                self.production_requirements().join("; and ")
            );
        }
        format!(
            "{} on {} is not usable: missing {}. The platform falls back to the classical solver, which is the configured default.",
            self.config.backend,
            self.config.endpoint,
            missing.join("; and ")
        )
    }
}

// --- the wire schema --------------------------------------------------------
//
// What this adapter promises to read, and the whole of it. Unknown *fields* are
// ignored, because the vendor adding one is not a fault. Unknown *values* in a
// field this adapter reads are refused, because those change what the record
// means — and the field that decides whether an answer counts as hardware
// evidence is exactly the one where a guess is unacceptable.

/// A backend's configuration, which is where the vendor states whether it is a
/// simulator.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct WireBackend {
    backend_name: String,
    /// Required, and deliberately not defaulted: a missing flag is not `false`.
    /// A response that does not say makes the whole request unreadable, which
    /// is the safe direction — the alternative is an unstated simulator
    /// reported as hardware.
    simulator: bool,
    n_qubits: usize,
}

/// What goes out on a submit.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct WireSubmit<'a> {
    program_id: &'a str,
    backend: &'a str,
    params: WireParams,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct WireParams {
    qubo: WireQubo,
    layers: usize,
    optimiser_iterations: usize,
    shots: usize,
}

/// The problem, in full. Sent rather than referenced so the job is
/// self-describing: a result can be checked against the problem it names
/// without depending on anything this process still holds.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct WireQubo {
    n: usize,
    entries: Vec<(usize, usize, f64)>,
    offset: f64,
}

/// A job, as the service returns it from a submit or a poll.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct WireJob {
    id: String,
    #[serde(default)]
    backend: Option<String>,
    /// The newer shape, `{"state": {"status": "Completed"}}`.
    #[serde(default)]
    state: Option<WireJobState>,
    /// The older shape, a bare `status`. Both are accepted because both are in
    /// the wild; a job with neither is unreadable rather than assumed running.
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct WireJobState {
    status: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Where a poll leaves a job.
enum JobProgress {
    /// Not finished. Poll again.
    Waiting,
    /// Finished, and a result can be read.
    Completed,
}

impl WireJob {
    /// The job's progress, from a status the service stated.
    ///
    /// A status this decoder cannot name is refused rather than treated as
    /// "still running": polling for ever on a state nobody recognises turns a
    /// vendor's API change into a hung thread and a bill.
    fn progress(&self, vendor: &str, job_id: &str) -> Result<JobProgress> {
        let (status, reason) = match (&self.state, &self.status) {
            (Some(state), _) => (state.status.as_str(), state.reason.clone()),
            (None, Some(status)) => (status.as_str(), None),
            (None, None) => {
                return Err(Error::schema(format!(
                    "{vendor} answered about job {job_id} with no status at all. A job with no \
                     stated status is not a job that is running; it is an answer this adapter \
                     cannot read"
                )));
            }
        };
        let detail = reason.unwrap_or_else(|| "no reason given".to_string());
        match status {
            "Queued" | "Running" | "Validating" | "Initializing" | "InProgress" => {
                Ok(JobProgress::Waiting)
            }
            "Completed" => Ok(JobProgress::Completed),
            "Failed" => Err(Error::io(format!(
                "{vendor} reports job {job_id} failed: {detail}"
            ))),
            "Cancelled" => Err(Error::io(format!(
                "{vendor} reports job {job_id} was cancelled: {detail}"
            ))),
            other => Err(Error::schema(format!(
                "{vendor} reports job {job_id} in state {other:?}, which this decoder cannot \
                 name. It is not defaulted to running: polling for ever on an unrecognised state \
                 is how a vendor's API change becomes a hung thread and a bill"
            ))),
        }
    }
}

/// The result payload, which is this adapter's schema and the deployment's
/// runtime program's responsibility to emit.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct WireResult {
    /// The best bit string found. Checked against the problem's size and
    /// re-scored before anything is returned.
    assignment: Vec<u8>,
    /// What the service says that assignment is worth. Discarded and
    /// recomputed; kept only long enough to be compared.
    energy: f64,
    expectation: f64,
    success_probability: f64,
    #[serde(default)]
    layers: Option<usize>,
    #[serde(default)]
    angles: Vec<f64>,
    #[serde(default)]
    evaluations: usize,
}
