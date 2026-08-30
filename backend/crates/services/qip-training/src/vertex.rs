//! The Vertex AI training port, over REST.
//!
//! The interface was complete before the transport was, and everything
//! downstream is written against it. This module now carries a real client —
//! `qip_transport::HttpClient` over Vertex's JSON REST API — while keeping the
//! reasoning that made the refusing version right, because that reasoning was
//! never about the absence of code.
//!
//! # The rule this module is built around
//!
//! **A job state is reported only when this adapter read it from the service.**
//! Everything downstream of a training run ends up on a model card: which
//! machine fitted it, when, and whether the fit finished. A client that
//! answered "succeeded" because a socket closed cleanly, or "failed" because a
//! request timed out, would write a model card recording a training run that
//! never happened — and a model card is exactly the artefact nobody re-derives,
//! because its whole purpose is to be the record.
//!
//! So there is a third answer beside the five [`JobState`] variants, and it is
//! not one of them: **unresolved**. A submit whose response this adapter could
//! not read in full leaves an [`UnresolvedSubmission`] — a job that may be
//! queued at Vertex, may be running, or may never have been created — and the
//! caller gets an error rather than a [`TrainingJob`]. A poll whose response
//! could not be read leaves the job's last known state *untouched* and returns
//! an error; it never advances a state on silence. The way out of unresolved is
//! [`VertexAiProvider::reconcile`], which asks the service and believes only
//! what it answers.
//!
//! That is deliberately conservative in one direction. An HTTP 401 on a submit
//! almost certainly created nothing, and this adapter records an unresolved
//! submission anyway, because "almost certainly" is not a basis on which to
//! decide whether a training run exists.
//!
//! # Authentication is a port, not a fake
//!
//! Vertex authenticates with an OAuth2 bearer token, and minting one from a
//! service-account key means RSA-signing a JWT. `docs/adr/0009` permits
//! `serde` and `serde_json` and nothing else, so there is no in-tree crypto to
//! sign it with and there will not be one. This adapter therefore **takes a
//! token it is given** ([`VertexAccessToken`]) and refuses clearly when it has
//! none. Obtaining and refreshing that token is the deployment's job — a
//! sidecar, the metadata server on a workload-identity-bound pod, or an
//! injected secret — and a token that has expired produces an HTTP 401 from
//! Google, which this adapter reports as a refusal naming the token rather than
//! as a job state.
//!
//! The token is held in a type that redacts in `Debug` and implements neither
//! `Serialize` nor `Deserialize`, so a struct holding one cannot derive them
//! either.
//!
//! # It needs a TLS-terminating proxy, and says so
//!
//! `qip_transport::http` has no TLS stack and refuses `https` by name rather
//! than downgrading it. Vertex is `https` only. A deployment therefore has to
//! put a TLS-terminating egress proxy in front of this adapter and point
//! [`VertexTransport::base_url`] at it over `http` on the cluster network.
//! That is a production requirement rather than a detail, it is listed in
//! [`VertexAiProvider::production_requirements`], and it does not go away when
//! everything else is configured: a bearer token sent in clear text to a public
//! endpoint is a credential in every hop's logs.
//!
//! # What this module still does not promise
//!
//! * **It does not produce an artifact.** [`TrainingProvider::artifact`]
//!   refuses even when connected, and that is not an omission. A finished
//!   Vertex job leaves a model in the staging bucket; a [`TrainingArtifact`]
//!   carries a [`TrainedTeacher`] — a model fitted *in this process*. This
//!   build has no bucket reader and no importer for Vertex's model format, and
//!   fitting something locally to fill the field would attach a model this
//!   platform trained to a job Vertex ran. That is the model-card lie in its
//!   purest form.
//! * **It does not manage the token's lifetime.** See above.
//! * **It does not decide that a model is fit for use.** That is
//!   `qip-lifecycle`, deliberately not a dependency of this crate.
//! * **It reads no clock.** Every timestamp written into a job comes from the
//!   caller's `at` or from the service's own message.
//!
//! [`TrainedTeacher`]: crate::local::TrainedTeacher

use crate::dataset::TrainingDataset;
use crate::job::{JobState, TrainingArtifact, TrainingJob, TrainingProvider, TrainingSpec};
use qip_core::error::{Error, Result};
use qip_core::{JobId, Timestamp};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, HttpResponse, Method, Url};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

/// How a Vertex AI training job would be configured.
///
/// Every field is a thing a deployment must supply. They are named separately
/// so the error says which one is missing rather than "not configured".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexAiConfig {
    /// The GCP project the training job is billed to and runs in.
    pub project_id: String,
    /// The region. Not interchangeable: a job cannot read a bucket or a
    /// dataset in another region without an egress charge and a latency cost,
    /// and some accelerator types exist in some regions only.
    pub region: String,
    /// The `gs://` bucket Vertex stages code, inputs and model output in.
    pub staging_bucket: String,
    /// What actually runs the fit.
    pub workload: VertexWorkload,
    /// The Kubernetes service account bound to a Google service account, and
    /// the roles that binding carries. Vertex jobs authenticate as a service
    /// account; a deployment with a token in an environment variable has not
    /// configured this, it has worked around it.
    pub workload_identity: WorkloadIdentityBinding,
}

/// What Vertex is asked to run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VertexWorkload {
    /// A custom training container: an image URI and the machine to run it on.
    CustomContainer {
        /// e.g. `europe-west4-docker.pkg.dev/PROJECT/REPO/trainer:TAG`.
        image_uri: String,
        machine_type: String,
        /// Accelerator type and count, where the fit needs one.
        accelerator: Option<String>,
    },
    /// AutoML: no container, and a target column and budget instead.
    AutoMl {
        objective: String,
        target_column: String,
        /// Node-hours the training budget allows.
        budget_node_hours: u32,
    },
}

impl VertexWorkload {
    pub fn describe(&self) -> String {
        match self {
            Self::CustomContainer {
                image_uri,
                machine_type,
                accelerator,
            } => format!(
                "the training container {image_uri} on a {machine_type}{}",
                accelerator
                    .as_ref()
                    .map_or_else(String::new, |a| format!(" with {a}"))
            ),
            Self::AutoMl {
                objective,
                target_column,
                budget_node_hours,
            } => format!(
                "an AutoML {objective} job on the target column {target_column} with a \
                 {budget_node_hours} node-hour budget"
            ),
        }
    }

    fn is_configured(&self) -> bool {
        match self {
            Self::CustomContainer {
                image_uri,
                machine_type,
                ..
            } => !image_uri.trim().is_empty() && !machine_type.trim().is_empty(),
            Self::AutoMl {
                objective,
                target_column,
                budget_node_hours,
            } => {
                !objective.trim().is_empty()
                    && !target_column.trim().is_empty()
                    && *budget_node_hours > 0
            }
        }
    }
}

/// The identity a Vertex job runs as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadIdentityBinding {
    /// The Kubernetes service account the platform's pods run as.
    pub kubernetes_service_account: String,
    /// The Google service account it impersonates.
    pub google_service_account: String,
    /// Roles that account must hold. Named because a binding with the wrong
    /// roles fails at the first API call with a message about a permission
    /// rather than about a binding.
    pub roles: Vec<String>,
}

impl WorkloadIdentityBinding {
    pub fn describe(&self) -> String {
        format!(
            "the Kubernetes service account {} bound to the Google service account {} holding {}",
            self.kubernetes_service_account,
            self.google_service_account,
            if self.roles.is_empty() {
                "no roles".to_string()
            } else {
                self.roles.join(", ")
            }
        )
    }

    fn is_configured(&self) -> bool {
        !self.kubernetes_service_account.trim().is_empty()
            && !self.google_service_account.trim().is_empty()
            && !self.roles.is_empty()
    }
}

/// An OAuth2 bearer token this process was handed.
///
/// Not minted here, and the reason is structural rather than a shortcut:
/// minting one from a service-account key means RSA-signing a JWT, and ADR
/// 0009 permits `serde` and `serde_json` and nothing else. There is no crypto
/// in this workspace to sign one with, so a "token provider" written here
/// would either shell out to something or fabricate a value — and a fabricated
/// bearer token is not a shortcut to a working client, it is a client that
/// gets a 401 and has to be debugged twice.
///
/// So the token is an input. A deployment resolves it — the metadata server on
/// a workload-identity-bound pod, a sidecar that refreshes it, an injected
/// secret — and hands it in. This type's job is only to stop it being printed.
///
/// `Debug` is written by hand and `Serialize`/`Deserialize` are not
/// implemented at all, which is the stronger of the two statements: a struct
/// holding one of these cannot derive them either, so the compiler refuses the
/// snapshot rather than emitting one with a token in it.
#[derive(Clone)]
pub struct VertexAccessToken(String);

impl VertexAccessToken {
    /// Wrap a resolved token.
    ///
    /// Refuses blank, because a token resolver that produced nothing writes an
    /// empty string rather than failing, and an empty `Authorization: Bearer `
    /// header is the failure that looks exactly like an expired credential.
    /// Refuses control characters, because a header value carrying one ends
    /// the header and lets the rest be read as another.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::invalid(
                "the Vertex access token is blank. An unresolved token is absent rather than \
                 empty, so that this adapter reports itself unavailable instead of sending an \
                 empty Authorization header and reading the 401 as a job state",
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(Error::invalid(
                "the Vertex access token contains a control character; sent as a header value it \
                 would end the header and let the rest be read as another one",
            ));
        }
        Ok(Self(value))
    }

    /// Hand the value to a transport writing an authentication header.
    ///
    /// Named to be conspicuous: a reviewer scanning a diff should see the word
    /// at every point the token leaves this type, and every such point should
    /// be exactly that.
    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for VertexAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VertexAccessToken(<redacted>)")
    }
}

/// How this process reaches Vertex.
///
/// Separate from [`VertexAiConfig`] because the two answer different questions
/// and change on different schedules: the config says what job to run and is
/// checked into a deployment, the transport says how to get there and carries
/// a secret that must not be.
#[derive(Debug)]
pub struct VertexTransport {
    /// `http://host[:port]` of the **TLS-terminating egress proxy** in front of
    /// `{region}-aiplatform.googleapis.com`.
    ///
    /// `http`, not `https`, and that is not an oversight: `qip_transport::http`
    /// has no TLS stack and refuses `https` by name rather than downgrading it.
    /// Pointing this straight at Google would fail to parse, which is the right
    /// failure — the alternative would be a bearer token crossing the internet
    /// in clear text.
    pub base_url: String,
    /// The bearer token, redacted in every printed form.
    pub token: VertexAccessToken,
    /// What this process will wait for and hold. The peer chooses how much to
    /// send; these decide how much of it is read.
    pub limits: ClientLimits,
}

impl VertexTransport {
    /// A transport with limits sized for a control-plane API.
    ///
    /// Larger bodies and longer reads than order entry gets: a Vertex job
    /// record is a few kilobytes, a list of them is more, and none of this is
    /// on a latency-sensitive path. The timeouts are still explicit and still
    /// short enough that a hung proxy is a visible failure within seconds
    /// rather than a thread parked for ever.
    pub fn new(base_url: impl Into<String>, token: VertexAccessToken) -> Self {
        Self {
            base_url: base_url.into(),
            token,
            limits: ClientLimits {
                max_body: 1024 * 1024,
                connect_timeout: StdDuration::from_secs(5),
                read_timeout: StdDuration::from_secs(20),
                write_timeout: StdDuration::from_secs(20),
                ..ClientLimits::default()
            },
        }
    }
}

/// A submit whose outcome this adapter could not read.
///
/// The job may be queued at Vertex, may be running, or may never have been
/// created. It is **not** a failure and it is **not** a success; it is the
/// third thing, and it stays that way until [`VertexAiProvider::reconcile`]
/// asks the service by display name and the service answers.
///
/// The spec is carried so a reconciled job can be rebuilt with the terms it was
/// submitted under rather than with terms invented at reconciliation time.
#[derive(Clone, Debug)]
pub struct UnresolvedSubmission {
    /// The display name the submit used, which is what a reconciliation filters
    /// on. Derived from the spec, so a repeated submit of the same spec carries
    /// the same name and a list finds the earlier job rather than creating a
    /// second one nobody is watching.
    pub display_name: String,
    pub spec: TrainingSpec,
    pub rows: usize,
    /// What this adapter saw instead of a job record.
    pub reason: String,
    pub at: Timestamp,
}

/// What this adapter has done, for metrics and for tests that assert a request
/// happened rather than assuming it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VertexStats {
    /// Submits this adapter committed to sending.
    pub submits_sent: u64,
    /// Submits that came back as a job record it could read in full.
    pub jobs_created: u64,
    /// Poll requests that left the process.
    pub polls_sent: u64,
    /// Times a request's outcome could not be read. Cumulative, so it counts a
    /// job that went unresolved twice twice; the current set is
    /// [`VertexAiProvider::unresolved_submissions`].
    pub entered_unresolved: u64,
    /// Unresolved submissions a reconciliation matched to a real job.
    pub reconciled: u64,
    /// States the service reported that this decoder will not map onto a
    /// [`JobState`]. Counted rather than folded into the error count because it
    /// is the number that says "Vertex's API moved and this decoder has not".
    pub unmappable_states: u64,
}

/// One job as this adapter holds it.
#[derive(Clone, Debug)]
struct TrackedJob {
    job: TrainingJob,
    /// The service's own resource name,
    /// `projects/{p}/locations/{r}/customJobs/{id}`.
    resource: String,
    /// Why this job's state is currently not trustworthy, when it is not.
    /// Set by a poll that could not read an answer and cleared by one that
    /// could. The job's `state` is never changed by either.
    unresolved: Option<String>,
}

/// An adapter to Vertex AI custom or AutoML training.
///
/// Constructed one of two ways, and the difference is the whole point:
/// [`Self::new`] and [`Self::with_credentials`] build a port that has no
/// transport and refuses every call, and [`Self::connected`] builds one that
/// opens sockets. There is no configuration that turns the first into the
/// second, and no fallback that turns the second back into the first — a
/// managed-training path that degraded to a local fit would put this process's
/// own model on a model card naming Vertex.
#[derive(Debug)]
pub struct VertexAiProvider {
    config: VertexAiConfig,
    /// Whether Application Default Credentials resolve in this environment.
    /// Injected rather than read from the environment here, so the same code
    /// path is exercised in a test. A connected provider has a token, which is
    /// a credential by construction.
    credentials_present: bool,
    /// The transport, when there is one. `None` is the port: it reports
    /// unavailable, names what is missing, and opens nothing.
    transport: Option<VertexTransport>,
    client: HttpClient,
    /// Keyed by the job id, which is the service's own identifier for the
    /// resource. Deliberately not a locally generated one: an id this process
    /// invented would have to be mapped to the service's, and the mapping is
    /// one more thing that can be lost when the process restarts.
    jobs: BTreeMap<String, TrackedJob>,
    unresolved: Vec<UnresolvedSubmission>,
    stats: VertexStats,
}

impl VertexAiProvider {
    /// The port: no credentials, no transport, and every call refused.
    pub fn new(config: VertexAiConfig) -> Self {
        Self {
            config,
            credentials_present: false,
            transport: None,
            client: HttpClient::new(ClientLimits::default()),
            jobs: BTreeMap::new(),
            unresolved: Vec::new(),
            stats: VertexStats::default(),
        }
    }

    /// Construct with credentials asserted present, to exercise the
    /// availability logic. The transport still is not.
    pub fn with_credentials(config: VertexAiConfig, credentials_present: bool) -> Self {
        Self {
            credentials_present,
            ..Self::new(config)
        }
    }

    /// The adapter that actually reaches Vertex.
    ///
    /// Fails on a base URL that cannot be parsed — which includes an `https`
    /// one, because this transport has no TLS and will not pretend otherwise.
    /// Opens nothing: whether the proxy is up and the token accepted is settled
    /// by the first request, and a constructor that dialled would make
    /// assembling a provider a network operation.
    pub fn connected(config: VertexAiConfig, transport: VertexTransport) -> Result<Self> {
        // Parsed here so a malformed endpoint fails where it was configured
        // rather than on the first submit.
        Url::parse(&transport.base_url).map_err(|error| {
            Error::invalid(format!(
                "the Vertex endpoint {:?} cannot be used: {error}. It must be the \
                 `http://host[:port]` of a TLS-terminating egress proxy in front of \
                 {}-aiplatform.googleapis.com — this transport has no TLS stack and refuses \
                 `https` by name rather than sending a bearer token in clear text",
                transport.base_url, config.region
            ))
        })?;
        let client = HttpClient::new(transport.limits);
        Ok(Self {
            config,
            credentials_present: true,
            transport: Some(transport),
            client,
            jobs: BTreeMap::new(),
            unresolved: Vec::new(),
            stats: VertexStats::default(),
        })
    }

    pub fn config(&self) -> &VertexAiConfig {
        &self.config
    }

    pub fn stats(&self) -> VertexStats {
        self.stats
    }

    /// Whether this adapter can put bytes on a wire at all.
    pub const fn has_transport(&self) -> bool {
        self.transport.is_some()
    }

    /// Every submit whose outcome nobody knows.
    ///
    /// The number to alert on. Each entry is a training run that may exist at
    /// Vertex, may be billing, and is not on any model card — and the platform
    /// cannot tell which until [`Self::reconcile`] asks.
    pub fn unresolved_submissions(&self) -> &[UnresolvedSubmission] {
        &self.unresolved
    }

    /// Jobs whose last poll could not be read, and why.
    ///
    /// Distinct from an unresolved *submission*: these jobs certainly exist and
    /// this adapter knows their identifiers; what it does not know is whether
    /// the state it last recorded is still true.
    pub fn stale_jobs(&self) -> Vec<(JobId, String)> {
        self.jobs
            .values()
            .filter_map(|tracked| {
                tracked
                    .unresolved
                    .as_ref()
                    .map(|reason| (tracked.job.id.clone(), reason.clone()))
            })
            .collect()
    }

    /// What this adapter last read from the service about a job, without
    /// asking again.
    pub fn tracked(&self, job: &JobId) -> Option<&TrainingJob> {
        self.jobs.get(job.as_str()).map(|tracked| &tracked.job)
    }

    /// Everything missing, named one item at a time.
    pub fn missing(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.config.project_id.trim().is_empty() {
            missing.push("a GCP project id to run and bill the job in".to_string());
        }
        if self.config.region.trim().is_empty() {
            missing.push(
                "a region: Vertex is regional, and a job cannot read a dataset or a bucket in \
                 another one without an egress charge"
                    .to_string(),
            );
        }
        if !self.config.staging_bucket.starts_with("gs://") {
            missing.push(
                "a gs:// staging bucket for the job's inputs, code and model output".to_string(),
            );
        }
        if !self.config.workload.is_configured() {
            missing.push(format!(
                "a fully specified workload: {}",
                self.config.workload.describe()
            ));
        }
        if !self.config.workload_identity.is_configured() {
            missing.push(format!(
                "a workload-identity binding: {}",
                self.config.workload_identity.describe()
            ));
        }
        if !self.credentials_present {
            missing.push(
                "resolvable Application Default Credentials for the bound service account"
                    .to_string(),
            );
        }
        if self.transport.is_none() {
            missing.push(
                "an HTTPS transport and a Vertex AI client, neither of which is present in this \
                 build; ADR 0009 permits both at the I/O edge and neither has been built"
                    .to_string(),
            );
        }
        missing
    }

    /// What a deployment still owes even when every field is set.
    ///
    /// These stand for a fully connected adapter, which is why they are
    /// reported separately from [`Self::missing`]: an adapter that can reach
    /// Vertex is not by itself a deployment anyone should be training
    /// production models through.
    pub fn production_requirements(&self) -> Vec<String> {
        vec![
            "a TLS-terminating egress proxy in front of this adapter: `qip_transport::http` has \
             no TLS stack and refuses `https` by name, so a bearer token sent straight to \
             googleapis.com would cross the internet in clear text"
                .to_string(),
            "something that mints and refreshes the OAuth2 access token. This adapter takes a \
             token it is given and cannot renew one: ADR 0009 forbids the in-tree crypto that \
             signing a service-account JWT needs, so an expired token becomes an HTTP 401 that \
             is reported as a refusal and never as a job state"
                .to_string(),
            "an alert on the count of unresolved submissions, which is what a control plane that \
             has started timing out looks like from the outside. An unresolved submission may be \
             a training run that is billing and that nothing is watching"
                .to_string(),
            "a reader for the staging bucket, or an importer for Vertex's model format. Until \
             one exists `artifact` refuses even for a job Vertex reports as succeeded, because \
             filling a `TrainedTeacher` with a locally fitted model would attach this process's \
             model to Vertex's job"
                .to_string(),
        ]
    }

    /// The text an operator needs.
    ///
    /// Never empty, even when the adapter is available — unlike the default on
    /// [`TrainingProvider::requirement`], and for the same reason
    /// `qip_brokers::rest` deviates: a configured adapter still owes the
    /// standing requirements above, and reporting nothing would read as
    /// "nothing left to do".
    pub fn requirement(&self) -> String {
        let mut parts = self.missing();
        parts.extend(self.production_requirements());
        format!(
            "Vertex AI training in project {} ({}) {}: it needs {}. {}",
            if self.config.project_id.trim().is_empty() {
                "<unset>"
            } else {
                &self.config.project_id
            },
            if self.config.region.trim().is_empty() {
                "<unset>"
            } else {
                &self.config.region
            },
            if self.is_usable() {
                "is reachable and still incomplete"
            } else {
                "is not usable"
            },
            parts.join("; and "),
            if self.is_usable() {
                "Nothing falls back: a managed run that degraded to a local fit would put this \
                 process's own model on a model card naming Vertex."
            } else {
                "The platform trains locally instead, which fits a smaller model on data that \
                 fits in memory and is not a substitute for a managed run"
            }
        )
    }

    fn is_usable(&self) -> bool {
        self.credentials_present && self.transport.is_some() && self.missing().is_empty()
    }

    /// The refusal every entry point returns when this adapter cannot send.
    fn unavailable(&self) -> Error {
        Error::unavailable(self.requirement())
    }

    // --- the wire ----------------------------------------------------------

    /// `projects/{project}/locations/{region}`.
    fn parent(&self) -> String {
        format!(
            "projects/{}/locations/{}",
            self.config.project_id, self.config.region
        )
    }

    /// The resource collection this workload lives in.
    ///
    /// Two different Vertex APIs, because they genuinely are two: a custom
    /// container is a `CustomJob`, and AutoML is a `TrainingPipeline` with a
    /// task definition rather than an image. Sending one to the other's
    /// endpoint is a 400 with a message about a field nobody set.
    const fn collection(&self) -> &'static str {
        match self.config.workload {
            VertexWorkload::CustomContainer { .. } => "customJobs",
            VertexWorkload::AutoMl { .. } => "trainingPipelines",
        }
    }

    /// Build a request carrying the bearer token, and nothing that is not
    /// needed.
    fn authenticated(
        &self,
        method: Method,
        target: &str,
        body: Option<Vec<u8>>,
    ) -> Result<HttpRequest> {
        let transport = self.transport.as_ref().ok_or_else(|| self.unavailable())?;
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

    /// A full URL under the configured base for a path this adapter composed.
    fn url(&self, path: &str) -> Result<String> {
        let transport = self.transport.as_ref().ok_or_else(|| self.unavailable())?;
        let base = Url::parse(&transport.base_url).map_err(Error::from)?;
        Ok(base.with_path(path).map_err(Error::from)?.to_string())
    }

    fn send(&self, request: &HttpRequest) -> Result<HttpResponse> {
        self.client.send(request).map_err(Error::from)
    }

    /// Read a response as a job record, or say why it is not one.
    ///
    /// A non-2xx is never a job state. Google's error bodies carry a message
    /// worth surfacing, so it is excerpted — bounded, because an error that
    /// embeds a megabyte of a peer's response makes a log unreadable at exactly
    /// the moment it is needed.
    fn decode_job(&self, response: &HttpResponse) -> Result<WireJob> {
        if !response.is_success() {
            return Err(self.status_refusal(response.status, &response.body_excerpt()));
        }
        let body = response.body_as_str().map_err(Error::from)?;
        serde_json::from_str::<WireJob>(body).map_err(|error| {
            Error::schema(format!(
                "Vertex answered HTTP {} with a body this adapter cannot read as a job: {error}. \
                 The first bytes of it were: {}",
                response.status,
                response.body_excerpt()
            ))
        })
    }

    /// What a status this adapter will not read a job state from means.
    ///
    /// Separated by class because the operator action differs, and because the
    /// 401 is the one worth naming precisely: this adapter cannot refresh a
    /// token, so an expired one is a deployment problem and not a transient
    /// one, and reporting it as "the service is unavailable" would put it on
    /// the wrong runbook page.
    fn status_refusal(&self, status: u16, excerpt: &str) -> Error {
        match status {
            401 => Error::denied(format!(
                "Vertex refused the access token (HTTP 401). This adapter takes a token it is \
                 given and cannot mint or refresh one — see the module documentation — so this is \
                 a token the deployment must renew. The token itself is not quoted here and is \
                 not written to any log by this adapter: {excerpt}"
            )),
            403 => Error::denied(format!(
                "Vertex refused the request (HTTP 403). The token authenticated and the bound \
                 service account is not permitted to do this in {}: {excerpt}",
                self.parent()
            )),
            404 => Error::not_found(format!(
                "Vertex has no such resource under {} (HTTP 404): {excerpt}. This is not evidence \
                 that a job does not exist — a resource can 404 while an index catches up — so it \
                 never resolves an unresolved submission",
                self.parent()
            )),
            408 | 429 => Error::unavailable(format!(
                "Vertex is rate-limiting or timing out this deployment (HTTP {status}): {excerpt}"
            )),
            500..=599 => Error::unavailable(format!(
                "Vertex failed to serve the request (HTTP {status}): {excerpt}"
            )),
            other => Error::invalid(format!(
                "Vertex answered HTTP {other}, which this adapter will not read a job state \
                 from: {excerpt}"
            )),
        }
    }

    /// Record a submit whose outcome could not be read.
    fn mark_unresolved(
        &mut self,
        display_name: &str,
        spec: &TrainingSpec,
        rows: usize,
        reason: String,
        at: Timestamp,
    ) {
        self.stats.entered_unresolved = self.stats.entered_unresolved.saturating_add(1);
        self.unresolved.push(UnresolvedSubmission {
            display_name: display_name.to_string(),
            spec: spec.clone(),
            rows,
            reason,
            at,
        });
    }

    /// Turn a job record the service sent into the platform's own shape.
    ///
    /// The one place a [`JobState`] is produced, and it is produced only from a
    /// state string the service sent. Everything else in this module routes
    /// through here.
    fn adopt(
        &mut self,
        wire: &WireJob,
        spec: &TrainingSpec,
        rows: usize,
        at: Timestamp,
    ) -> Result<TrainingJob> {
        // An answer that names a different job is not evidence about this one.
        // The display name is a pure function of the spec, so this catches a
        // list answer that matched something else and a poll that followed a
        // resource name into the wrong record.
        let expected = display_name_for(spec);
        if let Some(reported) = wire.display_name.as_deref()
            && reported != expected
        {
            return Err(Error::invalid(format!(
                "Vertex answered about a job displayed as {reported:?} where this adapter asked \
                 about {expected:?}. A record for another job is not a state for this one, and \
                 recording it would put someone else's run on this model's card"
            )));
        }
        let id = wire.job_id().ok_or_else(|| {
            Error::schema(format!(
                "Vertex answered with the resource name {:?}, which this adapter cannot read an \
                 identifier out of. Without one the job cannot be polled or cancelled, so it is \
                 treated as unresolved rather than recorded under a name nobody can use",
                wire.name
            ))
        })?;
        let state = match wire.job_state() {
            Ok(state) => state,
            Err(error) => {
                self.stats.unmappable_states = self.stats.unmappable_states.saturating_add(1);
                return Err(error);
            }
        };
        let job = TrainingJob {
            id: JobId::from_string(id.clone()),
            provider: "vertex-ai".to_string(),
            spec: spec.clone(),
            state,
            submitted_at: at,
            updated_at: at,
            rows,
        };
        self.jobs.insert(
            id,
            TrackedJob {
                job: job.clone(),
                resource: wire.name.clone(),
                unresolved: None,
            },
        );
        Ok(job)
    }

    /// Ask the service about every job carrying a display name, and believe
    /// only what it answers.
    ///
    /// The way out of [`UnresolvedSubmission`]. Returns the jobs the service
    /// reported, registering each so it can be polled and cancelled like any
    /// other.
    ///
    /// **An empty answer resolves nothing.** A list that returns no match may
    /// be an index that has not caught up, and treating absence as "the job was
    /// never created" is exactly how a running, billing training job becomes
    /// invisible. The submission stays on the unresolved list until the service
    /// names a job, and the only other way off it is
    /// [`Self::abandon_unresolved`], which is attributed and refuses to assert
    /// anything about what Vertex did.
    pub fn reconcile(&mut self, display_name: &str, at: Timestamp) -> Result<Vec<TrainingJob>> {
        if !self.is_usable() {
            return Err(self.unavailable());
        }
        let Some(pending) = self
            .unresolved
            .iter()
            .find(|entry| entry.display_name == display_name)
            .cloned()
        else {
            return Err(Error::not_found(format!(
                "no unresolved submission is recorded under the display name {display_name:?}. \
                 This adapter's list is the record of what *this process* could not read, and it \
                 does not survive a restart; the service's own job list is the reconciliation of \
                 record"
            )));
        };

        // `filter` is a real Vertex query parameter. Encoded rather than
        // interpolated: the value carries quotes, and a raw one would put a
        // space or a quote into a request line.
        let target = self.url(&format!(
            "/v1/{}/{}?filter={}",
            self.parent(),
            self.collection(),
            percent_encode(&format!("display_name=\"{display_name}\""))
        ))?;
        let request = self.authenticated(Method::Get, &target, None)?;
        let response = self.send(&request)?;
        if !response.is_success() {
            return Err(self.status_refusal(response.status, &response.body_excerpt()));
        }
        let body = response.body_as_str().map_err(Error::from)?;
        let listed: WireList = serde_json::from_str(body).map_err(|error| {
            Error::schema(format!(
                "Vertex answered a job list with a body this adapter cannot read: {error}. The \
                 first bytes of it were: {}",
                response.body_excerpt()
            ))
        })?;

        let mut found = Vec::new();
        for wire in listed.jobs() {
            found.push(self.adopt(wire, &pending.spec, pending.rows, at)?);
        }
        if found.is_empty() {
            // Deliberately not resolved. See the doc comment.
            return Ok(found);
        }
        self.unresolved
            .retain(|entry| entry.display_name != display_name);
        self.stats.reconciled = self.stats.reconciled.saturating_add(1);
        Ok(found)
    }

    /// Drop an unresolved submission because a person decided it never
    /// happened.
    ///
    /// Named to be conspicuous in review, and it asserts nothing: it does not
    /// record a job, a state or an outcome, because nobody here knows one. It
    /// only stops this process from carrying a submission it will never be able
    /// to resolve — a job list that has been checked by hand, a project that
    /// has been torn down. The attribution is required so the audit trail says
    /// who decided.
    pub fn abandon_unresolved(&mut self, display_name: &str, operator: &str) -> Result<()> {
        if operator.trim().is_empty() {
            return Err(Error::invalid(
                "abandoning an unresolved submission needs a named operator: it is a person \
                 deciding something the service never confirmed, and the audit trail is the point",
            ));
        }
        let before = self.unresolved.len();
        self.unresolved
            .retain(|entry| entry.display_name != display_name);
        if self.unresolved.len() == before {
            return Err(Error::not_found(format!(
                "no unresolved submission is recorded under {display_name:?}"
            )));
        }
        Ok(())
    }
}

impl TrainingProvider for VertexAiProvider {
    fn name(&self) -> &str {
        "vertex-ai"
    }

    fn is_available(&self) -> bool {
        self.is_usable()
    }

    fn requirement(&self) -> String {
        Self::requirement(self)
    }

    /// Create a training job, once.
    ///
    /// Every exit leaves the submission in exactly one of three places: a job
    /// record the service sent, an unresolved submission, or — where this
    /// adapter refused before anything left the process — nothing at all.
    /// There is no fourth, and in particular there is no path that reports a
    /// job the service did not describe.
    fn submit(
        &mut self,
        spec: TrainingSpec,
        data: &TrainingDataset,
        at: Timestamp,
    ) -> Result<TrainingJob> {
        if !self.is_usable() {
            return Err(self.unavailable());
        }
        spec.validate()?;
        if data.arity() == 0 {
            return Err(Error::invalid(
                "a training job needs at least one feature; Vertex would accept the request and \
                 fail the fit, which costs a queue slot to learn something checkable here",
            ));
        }
        let rows = data.len();
        let display_name = display_name_for(&spec);
        let body =
            serde_json::to_vec(&self.request_body(&spec, &display_name)).map_err(|error| {
                Error::schema(format!("this job cannot be written as JSON: {error}"))
            })?;
        let target = self.url(&format!("/v1/{}/{}", self.parent(), self.collection()))?;
        let request = self.authenticated(Method::Post, &target, Some(body))?;

        self.stats.submits_sent = self.stats.submits_sent.saturating_add(1);
        let response = match self.send(&request) {
            Ok(response) => response,
            Err(error) => {
                let reason = format!(
                    "the submit failed with no answer this adapter could read ({}). The job may \
                     be queued at Vertex, may be running, or may never have been created",
                    error.message()
                );
                self.mark_unresolved(&display_name, &spec, rows, reason, at);
                return Err(error);
            }
        };
        let wire = match self.decode_job(&response) {
            Ok(wire) => wire,
            Err(error) => {
                let reason = format!(
                    "Vertex answered and the answer could not be read as a job ({}). The job may \
                     be queued at Vertex, may be running, or may never have been created",
                    error.message()
                );
                self.mark_unresolved(&display_name, &spec, rows, reason, at);
                return Err(error);
            }
        };
        match self.adopt(&wire, &spec, rows, at) {
            Ok(job) => {
                self.stats.jobs_created = self.stats.jobs_created.saturating_add(1);
                Ok(job)
            }
            Err(error) => {
                let reason = format!(
                    "Vertex answered with a job record this adapter will not record ({}). The \
                     job exists or it does not, and this process cannot tell which",
                    error.message()
                );
                self.mark_unresolved(&display_name, &spec, rows, reason, at);
                Err(error)
            }
        }
    }

    /// Read a job's state from the service.
    ///
    /// The job's recorded state is replaced only by one the service stated. A
    /// poll this adapter could not read marks the job stale — visible through
    /// [`VertexAiProvider::stale_jobs`] — and leaves the last state it did read
    /// exactly as it was. Nothing here advances a job toward `Succeeded` on
    /// silence, which is the whole reason this module exists in the shape it
    /// does.
    fn poll(&mut self, job: &JobId, at: Timestamp) -> Result<TrainingJob> {
        if !self.is_usable() {
            return Err(self.unavailable());
        }
        let tracked = self.jobs.get(job.as_str()).ok_or_else(|| {
            Error::not_found(format!(
                "this adapter is not tracking job {}. It records the jobs it created in this \
                 process and nothing else — a job from an earlier process is reconciled against \
                 Vertex's own records, not against this list",
                job.as_str()
            ))
        })?;
        let (resource, spec, rows) = (
            tracked.resource.clone(),
            tracked.job.spec.clone(),
            tracked.job.rows,
        );

        let target = self.url(&format!("/v1/{resource}"))?;
        let request = self.authenticated(Method::Get, &target, None)?;
        self.stats.polls_sent = self.stats.polls_sent.saturating_add(1);

        let outcome = self
            .send(&request)
            .and_then(|response| self.decode_job(&response));
        let wire = match outcome {
            Ok(wire) => wire,
            Err(error) => {
                let reason = format!(
                    "the poll produced no state this adapter could read ({}); the state below is \
                     the last one Vertex stated and may no longer be true",
                    error.message()
                );
                if let Some(entry) = self.jobs.get_mut(job.as_str()) {
                    entry.unresolved = Some(reason);
                }
                return Err(error);
            }
        };
        // Submitted-at is the moment this process first recorded the job, not
        // a fresh read: a poll learns a state, not a creation time.
        let submitted_at = self
            .jobs
            .get(job.as_str())
            .map_or(at, |entry| entry.job.submitted_at);
        let mut job = self.adopt(&wire, &spec, rows, at)?;
        job.submitted_at = submitted_at;
        if let Some(entry) = self.jobs.get_mut(job.id.as_str()) {
            entry.job.submitted_at = submitted_at;
        }
        Ok(job)
    }

    /// Refused, including for a job Vertex reports as succeeded.
    ///
    /// See the module documentation. A [`TrainingArtifact`] carries a model
    /// fitted in this process; a finished Vertex job leaves a model in the
    /// staging bucket that this build can neither read nor import. Fitting
    /// something locally to fill the field would attach this platform's model
    /// to Vertex's job, which is exactly the model-card lie the whole module is
    /// arranged against.
    fn artifact(&self, job: &JobId) -> Result<TrainingArtifact> {
        // An adapter that cannot reach Vertex at all answers with the port's
        // own refusal, which names the transport. The refusal below is the
        // narrower one that survives a *working* connection, and conflating
        // the two would tell an operator to build a bucket reader when what
        // they are missing is the proxy.
        if !self.is_usable() {
            return Err(self.unavailable());
        }
        Err(Error::unavailable(format!(
            "the model for job {} is in {} and this build has no reader for it and no importer \
             for Vertex's model format. A `TrainingArtifact` carries a model fitted in this \
             process, so there is nothing honest to put in one: fitting a local model to fill it \
             would record this platform's model against a run Vertex performed. What is needed is \
             {}",
            job.as_str(),
            if self.config.staging_bucket.trim().is_empty() {
                "the staging bucket"
            } else {
                &self.config.staging_bucket
            },
            self.production_requirements()
                .last()
                .map_or("a bucket reader", String::as_str)
        )))
    }

    /// Ask Vertex to stop a job, then read what it says the job's state is.
    ///
    /// Two requests, because Vertex's cancel returns an empty body: it records
    /// the intent and the job moves to `CANCELLING` and then to `CANCELLED` in
    /// its own time. Returning `Cancelled` off the back of the first request
    /// would report a terminal state the service had not reached, so this reads
    /// the job afterwards and reports whatever the service actually says —
    /// usually `Running`, because a job that is cancelling is still running.
    ///
    /// An ambiguous cancel does not mean cancelled. It marks the job stale for
    /// the same reason an ambiguous poll does.
    fn cancel(&mut self, job: &JobId, at: Timestamp) -> Result<TrainingJob> {
        if !self.is_usable() {
            return Err(self.unavailable());
        }
        let resource = self
            .jobs
            .get(job.as_str())
            .map(|tracked| tracked.resource.clone())
            .ok_or_else(|| {
                Error::not_found(format!(
                    "this adapter is not tracking job {}, so it does not know the resource to \
                     cancel",
                    job.as_str()
                ))
            })?;

        let target = self.url(&format!("/v1/{resource}:cancel"))?;
        let request = self.authenticated(Method::Post, &target, Some(b"{}".to_vec()))?;
        let response = match self.send(&request) {
            Ok(response) => response,
            Err(error) => {
                if let Some(entry) = self.jobs.get_mut(job.as_str()) {
                    entry.unresolved = Some(format!(
                        "the cancel produced no answer this adapter could read ({}); whether \
                         Vertex received it is not known",
                        error.message()
                    ));
                }
                return Err(error);
            }
        };
        if !response.is_success() {
            return Err(self.status_refusal(response.status, &response.body_excerpt()));
        }
        // The service accepted the intent. What the job's state now *is* comes
        // from the job, not from this acknowledgement.
        self.poll(job, at)
    }
}

impl VertexAiProvider {
    /// The request body for one submit.
    ///
    /// Built as a `serde_json::Value` rather than as a typed struct, and that
    /// asymmetry with the response types below is deliberate: the request shape
    /// is Google's and deeply nested, and nothing downstream depends on it
    /// being re-readable here. The *response* is what correctness depends on,
    /// so that is typed and every unknown value in it is refused.
    fn request_body(&self, spec: &TrainingSpec, display_name: &str) -> serde_json::Value {
        use serde_json::json;
        match &self.config.workload {
            VertexWorkload::CustomContainer {
                image_uri,
                machine_type,
                accelerator,
            } => {
                let mut machine = json!({ "machineType": machine_type });
                if let Some(accelerator) = accelerator {
                    machine["acceleratorType"] = json!(accelerator);
                    machine["acceleratorCount"] = json!(1);
                }
                json!({
                    "displayName": display_name,
                    "jobSpec": {
                        "workerPoolSpecs": [{
                            "machineSpec": machine,
                            "replicaCount": 1,
                            "containerSpec": {
                                "imageUri": image_uri,
                                // The fit's own terms, passed to the trainer.
                                // The seed is here because a run nobody can
                                // reproduce is a run nobody can audit.
                                "args": [
                                    format!("--dataset={}", spec.dataset),
                                    format!("--holdout-fraction={}", spec.holdout_fraction),
                                    format!("--seed={}", spec.seed),
                                ],
                            },
                        }],
                        "baseOutputDirectory": {
                            "outputUriPrefix": self.config.staging_bucket,
                        },
                    },
                })
            }
            VertexWorkload::AutoMl {
                objective,
                target_column,
                budget_node_hours,
            } => json!({
                "displayName": display_name,
                "trainingTaskDefinition":
                    "gs://google-cloud-aiplatform/schema/trainingjob/definition/\
                     automl_tabular_1.0.0.yaml",
                "trainingTaskInputs": {
                    "targetColumn": target_column,
                    "predictionType": objective,
                    "trainBudgetMilliNodeHours": u64::from(*budget_node_hours) * 1_000,
                },
            }),
        }
    }
}

// --- the wire schema --------------------------------------------------------
//
// What this adapter promises to read, and the whole of it. Unknown *fields* are
// ignored, because Google adding one is not a fault and must not stop a poll.
// Unknown *values* in a field this adapter reads are refused, because those
// change what the record means — and the field that decides whether a model
// card says a run finished is exactly the one where a guess is unacceptable.

/// One job or training pipeline, as Vertex returns it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireJob {
    /// `projects/{p}/locations/{r}/customJobs/{id}`.
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    /// `JOB_STATE_QUEUED`, `JOB_STATE_RUNNING`, `JOB_STATE_SUCCEEDED`, …
    state: String,
    #[serde(default)]
    error: Option<WireStatus>,
}

/// Google's `Status`, carried on a failed job.
#[derive(Debug, Deserialize)]
struct WireStatus {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

impl WireJob {
    /// The trailing identifier of the resource name.
    ///
    /// `None` for a name this adapter cannot read one out of, which is treated
    /// as an unreadable answer rather than given an invented id: a job recorded
    /// under a name nobody can use cannot be polled or cancelled.
    fn job_id(&self) -> Option<String> {
        let id = self.name.rsplit('/').next()?.trim();
        if id.is_empty() || id == self.name.trim() {
            return None;
        }
        Some(id.to_string())
    }

    /// The platform's state, from the service's own.
    ///
    /// The mapping loses precision in exactly one direction, and never the
    /// dangerous one: every state Vertex considers non-terminal maps onto a
    /// non-terminal [`JobState`], so nothing downstream reads an artifact from
    /// a job that is still running. `JOB_STATE_CANCELLING` is `Running` because
    /// a job that is cancelling has not stopped; the next poll reports
    /// `Cancelled` when the service does.
    ///
    /// `JOB_STATE_PARTIALLY_SUCCEEDED` is refused rather than mapped. It is
    /// terminal and it is neither of the two things this platform can record:
    /// calling it a success would put a partial fit on a model card as a whole
    /// one, and calling it a failure would discard work the service says
    /// happened.
    fn job_state(&self) -> Result<JobState> {
        let described = || -> String {
            self.error.as_ref().map_or_else(
                || format!("Vertex reported {} with no error attached", self.state),
                |status| format!("{} (code {})", status.message, status.code),
            )
        };
        match self.state.as_str() {
            "JOB_STATE_QUEUED" | "JOB_STATE_PENDING" | "JOB_STATE_PAUSED" => Ok(JobState::Queued),
            "JOB_STATE_RUNNING" | "JOB_STATE_UPDATING" | "JOB_STATE_CANCELLING" => {
                Ok(JobState::Running)
            }
            "JOB_STATE_SUCCEEDED" => Ok(JobState::Succeeded),
            "JOB_STATE_FAILED" => Ok(JobState::Failed(described())),
            "JOB_STATE_EXPIRED" => Ok(JobState::Failed(format!(
                "the job expired before it ran: {}",
                described()
            ))),
            "JOB_STATE_CANCELLED" => Ok(JobState::Cancelled(described())),
            "JOB_STATE_PARTIALLY_SUCCEEDED" => Err(Error::schema(format!(
                "Vertex reported job {} as JOB_STATE_PARTIALLY_SUCCEEDED, which this platform \
                 cannot record: `Succeeded` would put a partial fit on a model card as a whole \
                 one, and `Failed` would discard work the service says happened. Read the job in \
                 the console and decide",
                self.name
            ))),
            other => Err(Error::schema(format!(
                "Vertex reported job {} in state {other:?}, which this decoder cannot name. An \
                 unnamed state is not defaulted to running or to succeeded — a default here is a \
                 model card entry nobody wrote",
                self.name
            ))),
        }
    }
}

/// A list answer. Two field names because two collections, and whichever one
/// the request asked for is the one that comes back.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireList {
    #[serde(default)]
    custom_jobs: Vec<WireJob>,
    #[serde(default)]
    training_pipelines: Vec<WireJob>,
}

impl WireList {
    fn jobs(&self) -> &[WireJob] {
        if self.custom_jobs.is_empty() {
            &self.training_pipelines
        } else {
            &self.custom_jobs
        }
    }
}

// --- pure helpers -----------------------------------------------------------

/// The display name one spec always produces.
///
/// A pure function of the spec's identity, so a repeated submit of the same
/// spec carries the same name — which is what makes [`VertexAiProvider::reconcile`]
/// able to find a job an earlier, ambiguous submit may have created. Vertex has
/// no client-supplied request id on these resources, so the display name is the
/// only handle a reconciliation has.
///
/// Restricted to characters Vertex accepts and truncated to its limit, because
/// a name the service refuses turns every submit into a 400 nobody can act on.
fn display_name_for(spec: &TrainingSpec) -> String {
    let raw = format!("{}-{}-{}", spec.name, spec.version, spec.seed);
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.chars().take(128).collect()
}

/// Percent-encode everything that is not unreserved.
///
/// Written here rather than pulled in, because the alternative is a dependency
/// and the rule is short. Deliberately aggressive: anything outside the
/// unreserved set is escaped, so a value carrying a quote, a space or an
/// equals sign cannot break the request line this adapter composes by hand.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
