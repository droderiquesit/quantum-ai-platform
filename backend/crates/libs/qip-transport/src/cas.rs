//! Workload certificates from Certificate Authority Service, renewed without
//! a restart (ADR 0112 §2).
//!
//! Until this module a Compute Engine process read its certificate and key
//! from files somebody had placed on the machine, and nothing renewed them:
//! the key sat on disk for as long as the certificate lived, and the
//! certificate lived for as long as whoever placed it chose. ADR 0112 asks
//! for the opposite on both counts — a key generated in memory that never
//! touches disk, and a certificate of hours rather than months, so a stolen
//! key dies with its certificate.
//!
//! What this module holds:
//!
//! * [`WorkloadKey`] — an ECDSA P-256 key generated from the operating
//!   system's entropy through `ring`, held only as `rustls`'s signing key.
//!   The PKCS#8 document `ring` produces exists for the length of
//!   [`WorkloadKey::generate`] and is never written, logged or returned.
//! * [`CasClient`] — one REST call, `POST {ca_pool}/certificates`, over
//!   [`TlsHttpClient`] against a pinned root set with a metadata-server
//!   bearer token: the shape [`crate::kms`] already uses for Cloud KMS. The
//!   public key goes in the request's `config` rather than in a CSR, which
//!   is what lets this module avoid an ASN.1 request encoder: the only DER it
//!   writes is the fixed `SubjectPublicKeyInfo` prefix around the curve
//!   point ([`p256_spki_der`]).
//! * [`Renewer`] — the loop: issue at start, renew at two-thirds of the
//!   certificate's window, back off and retry on failure, and install each
//!   new certificate into the shared [`WorkloadIdentity`] that every server
//!   and client config built from it reads per handshake. A process whose
//!   certificate lapses because the CA is unreachable stops presenting it —
//!   the identity's slot answers nothing — and [`RenewalStatus`] tells its
//!   health endpoint it is not ready. There is no plaintext fallback, here or
//!   anywhere: the link simply has no certificate until the CA answers.
//!
//! **The request shape, and what is not certain about it.** The body follows
//! the v1 `Certificate` resource as Google documents it — `lifetime` as a
//! protobuf `Duration` string, optional `certificateTemplate`, and `config`
//! with `publicKey { format: "PEM", key }`, `subjectConfig` and
//! `x509Config` — and the response's `pemCertificate` and
//! `pemCertificateChain` (issuer first, ending at the pool's root). Two
//! points were written from the API reference rather than observed against
//! the live service: that `publicKey.key` is the base64 of the PEM text
//! (the field is `bytes`, so JSON carries it base64-encoded, and its content
//! is the PEM `PUBLIC KEY` block when `format` is `PEM`), and that a
//! `subjectConfig` carrying only a subject alternative name and no `subject`
//! is accepted. Both are named in the first failure they would produce:
//! the CA answers HTTP 400 and [`CasClient::request`] carries its message.
//!
//! **The name a certificate carries.** The service account's email is not a
//! DNS name, and the platform's peer checks ([`crate::tls::PeerNames`]) are
//! DNS names, so [`WorkloadSubject`] maps `name@project.iam.gserviceaccount.com`
//! to the DNS name `name.project.iam.gserviceaccount.com` and requests that
//! one name as its only subject alternative name. That a workload can
//! obtain a certificate naming only *itself* (ADR 0112 §2) is not something
//! this module can enforce — it asks for its own name, and a compromised
//! process could ask for another. The pool enforces it: its workload
//! template admits exactly one DNS SAN shaped as a service account's, and
//! each requester's `certificateRequester` grant is IAM-conditioned on the
//! SAN naming its own account (`infrastructure/terraform/security.tf`). That
//! pairing is written from Google's documentation and has not been run
//! against a live CA.
//! Until the Terraform names one of those, the pool's requester binding is
//! the only control on who may be named.

//!
//! **Where the pieces live.** The settings, the names and [`WorkloadKey`] are
//! `qip_tls::cas`, re-exported here: the TLS policy reads them, and since it
//! became the `qip-tls` crate it cannot reach back into this one. What stays
//! here is what needs the HTTP client, a KMS bearer or `serde_json` — the
//! request body, [`CasClient`], the renewal loop and its status.

pub use qip_tls::cas::*;

use crate::http::{ClientLimits, HttpRequest, Method, TlsHttpClient};
use crate::kms::{BearerSource, encode_base64};
use crate::tls::{self, Validity, WorkloadIdentity};
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_tls::ring::rand::SecureRandom;
use qip_tls::rustls::pki_types::CertificateDer;
use qip_tls::rustls::pki_types::pem::PemObject;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The CA Service REST endpoint. A constant, not configuration, for the
/// reason [`crate::kms::KMS_ENDPOINT`] is: a configurable base would be the
/// one setting that could point the platform's identity at a server that
/// issues whatever it likes. A test reaches a stand-in through
/// [`CasClient::with_endpoint`], and whatever answers, the certificate is
/// checked against the pool's roots before it is installed.
pub const CAS_ENDPOINT: &str = "https://privateca.googleapis.com/v1";

/// The JSON body of one `certificates.create` call.
pub fn request_body(
    key: &WorkloadKey,
    subject: &WorkloadSubject,
    lifetime_secs: u64,
    template: Option<&CertificateTemplate>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "lifetime": format!("{lifetime_secs}s"),
        "config": {
            "publicKey": {
                "format": "PEM",
                // `bytes` in the API, so base64 on the wire, of the PEM text.
                "key": encode_base64(key.public_key_pem().as_bytes()),
            },
            "subjectConfig": {
                "subjectAltName": {
                    // One DNS SAN and nothing else: the pool's workload
                    // template refuses any other shape, and each requester's
                    // IAM condition admits only the SAN naming its own
                    // service account (infrastructure/terraform/security.tf).
                    // An email SAN beside it would make every request fail.
                    "dnsNames": [subject.dns_name()],
                },
            },
            "x509Config": {
                "caOptions": { "isCa": false },
                "keyUsage": {
                    "baseKeyUsage": { "digitalSignature": true },
                    "extendedKeyUsage": { "serverAuth": true, "clientAuth": true },
                },
            },
        },
    });
    if let (Some(template), Some(object)) = (template, body.as_object_mut()) {
        object.insert(
            "certificateTemplate".to_string(),
            serde_json::Value::String(template.as_str().to_string()),
        );
    }
    body
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuedResponse {
    name: String,
    pem_certificate: String,
    #[serde(default)]
    pem_certificate_chain: Vec<String>,
}

/// Something that turns a public key into a certificate chain, leaf first.
/// [`CasClient`] is the production one; a test's issuer mints locally.
///
/// Named `issue_certificate`, not `issue`: `issue` is the capital
/// envelope's verb, and qip-acceptance's `security.rs` review list of
/// functions that move a capital weight finds callers by that name. A
/// certificate is not capital, and the plain name made it read as one.
pub trait CertificateIssuer: Send + Sync + std::fmt::Debug {
    fn issue_certificate(
        &self,
        key: &WorkloadKey,
        certificate_id: &str,
    ) -> Result<Vec<CertificateDer<'static>>>;
}

/// The CA Service REST client for one workload.
#[derive(Debug)]
pub struct CasClient {
    client: TlsHttpClient,
    bearer: Arc<dyn BearerSource>,
    endpoint: String,
    pool: CaPool,
    template: Option<CertificateTemplate>,
    lifetime_secs: u64,
    subject: WorkloadSubject,
}

impl CasClient {
    pub fn new(
        settings: &CasSettings,
        subject: WorkloadSubject,
        client: TlsHttpClient,
        bearer: Arc<dyn BearerSource>,
    ) -> Self {
        Self {
            client,
            bearer,
            endpoint: CAS_ENDPOINT.to_string(),
            pool: settings.pool.clone(),
            template: settings.template.clone(),
            lifetime_secs: settings.lifetime_secs,
            subject,
        }
    }

    /// The same client against another `https` base — a test's stand-in.
    /// No composition root calls this; see [`CAS_ENDPOINT`] for why it
    /// cannot make a forged certificate stick.
    pub fn with_endpoint(mut self, endpoint: &str) -> Result<Self> {
        if !endpoint.starts_with("https://") {
            return Err(Error::invalid(format!(
                "a CA Service endpoint must be https, and {endpoint:?} is not"
            )));
        }
        self.endpoint = endpoint.trim_end_matches('/').to_string();
        Ok(self)
    }

    /// The bounds every call runs under. Issuance is one small JSON document
    /// each way; the read timeout allows for the CA signing in an HSM.
    pub fn limits() -> ClientLimits {
        ClientLimits {
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(10),
            max_body: 256 * 1024,
            ..ClientLimits::default()
        }
    }

    /// One `certificates.create` call: the chain, leaf first, unverified —
    /// [`WorkloadIdentity::install`] is what verifies it, so no path can
    /// install a certificate without the checks.
    pub fn request(
        &self,
        key: &WorkloadKey,
        certificate_id: &str,
    ) -> Result<Vec<CertificateDer<'static>>> {
        let body = serde_json::to_vec(&request_body(
            key,
            &self.subject,
            self.lifetime_secs,
            self.template.as_ref(),
        ))
        .map_err(|error| {
            Error::invalid(format!("cannot encode the CA Service request: {error}"))
        })?;
        let url = format!(
            "{}/{}/certificates?certificateId={certificate_id}",
            self.endpoint,
            self.pool.as_str()
        );
        let request = HttpRequest::json_tls(Method::Post, &url, body)
            .map_err(Error::from)?
            .with_header(
                "authorization",
                &format!("Bearer {}", self.bearer.bearer()?),
            );
        let response = self.client.send(&request).map_err(|error| {
            Error::from(error).relabelled(format!(
                "the certificate request to CA pool {} failed",
                self.pool.as_str()
            ))
        })?;
        if !response.is_success() {
            return Err(Error::unavailable(format!(
                "CA Service answered the certificate request to {} with status {}: {}",
                self.pool.as_str(),
                response.status,
                response.body_excerpt()
            )));
        }
        let answer: IssuedResponse = serde_json::from_slice(&response.body).map_err(|error| {
            Error::invalid(format!(
                "the CA Service response does not parse as a certificate: {error}"
            ))
        })?;
        let expected = format!("{}/certificates/{certificate_id}", self.pool.as_str());
        if answer.name != expected {
            return Err(Error::denied(format!(
                "CA Service answered for {} when {expected} was requested; the certificate is \
                 discarded rather than attributed to the wrong request",
                answer.name
            )));
        }
        let mut chain = parse_pem_certificates(&answer.pem_certificate, "pemCertificate")?;
        if chain.len() != 1 {
            return Err(Error::denied(format!(
                "pemCertificate holds {} certificates where one leaf was expected",
                chain.len()
            )));
        }
        for (index, issuer) in answer.pem_certificate_chain.iter().enumerate() {
            chain.extend(parse_pem_certificates(
                issuer,
                &format!("pemCertificateChain[{index}]"),
            )?);
        }
        Ok(chain)
    }
}

fn parse_pem_certificates(pem: &str, field: &str) -> Result<Vec<CertificateDer<'static>>> {
    CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::invalid(format!(
                "the CA Service response's {field} does not parse: {error}"
            ))
        })
}

impl CertificateIssuer for CasClient {
    fn issue_certificate(
        &self,
        key: &WorkloadKey,
        certificate_id: &str,
    ) -> Result<Vec<CertificateDer<'static>>> {
        self.request(key, certificate_id)
    }
}

/// What the renewal loop last did, for a health endpoint.
#[derive(Debug)]
pub struct RenewalStatus {
    identity: Arc<WorkloadIdentity>,
    issued: AtomicU64,
    consecutive_failures: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl RenewalStatus {
    pub fn new(identity: Arc<WorkloadIdentity>) -> Self {
        Self {
            identity,
            issued: AtomicU64::new(0),
            consecutive_failures: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    /// Whether a handshake now would present a certificate — read from the
    /// identity at the instant of asking, not from a flag the loop last set,
    /// so a certificate that lapses between two renewal attempts reads as
    /// not-ready at once rather than at the next attempt.
    pub fn is_current(&self) -> bool {
        self.identity.is_current()
    }

    pub fn issued(&self) -> u64 {
        self.issued.load(Ordering::SeqCst)
    }

    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn succeeded(&self) {
        self.issued.fetch_add(1, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn failed(&self, error: &Error) {
        self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(error.message().to_string());
    }

    /// The health endpoints' `workload_certificate` object.
    pub fn to_json(&self) -> serde_json::Value {
        let validity = self.identity.validity();
        serde_json::json!({
            "source": "certificate_authority_service",
            "current": self.is_current(),
            "not_after": validity.map(|v| v.not_after.to_string()),
            "renew_at": validity.map(|v| v.renew_at().to_string()),
            "issued": self.issued(),
            "consecutive_failures": self.consecutive_failures(),
            "last_error": self.last_error(),
        })
    }
}

/// The first wait after a failed attempt, and the longest.
pub const RETRY_INITIAL: Duration = Duration::from_secs(5);
pub const RETRY_MAXIMUM: Duration = Duration::from_secs(300);

/// The renewal loop for one identity.
#[derive(Debug)]
pub struct Renewer {
    issuer: Arc<dyn CertificateIssuer>,
    identity: Arc<WorkloadIdentity>,
    status: Arc<RenewalStatus>,
    clock: Arc<dyn Clock>,
    workload: String,
    random: RandomSource,
    backoff: Duration,
}

/// `SecureRandom` is not `Debug`; this is, and says nothing.
struct RandomSource(Arc<dyn SecureRandom + Send + Sync>);

impl std::fmt::Debug for RandomSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RandomSource")
    }
}

impl Renewer {
    /// `workload` prefixes each certificate id, so the CA's own audit log
    /// says which binary asked: lowercase letters, digits and `-`, at most
    /// forty characters, leaving room in the API's 63 for the suffix.
    pub fn new(
        issuer: Arc<dyn CertificateIssuer>,
        identity: Arc<WorkloadIdentity>,
        random: Arc<dyn SecureRandom + Send + Sync>,
        clock: Arc<dyn Clock>,
        workload: &str,
    ) -> Result<Self> {
        if workload.is_empty()
            || workload.len() > 40
            || !workload
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Error::invalid(format!(
                "{workload:?} cannot prefix a CA Service certificate id; use lowercase letters, \
                 digits and '-', at most forty"
            )));
        }
        Ok(Self {
            status: Arc::new(RenewalStatus::new(Arc::clone(&identity))),
            issuer,
            identity,
            clock,
            workload: workload.to_string(),
            random: RandomSource(random),
            backoff: RETRY_INITIAL,
        })
    }

    pub fn status(&self) -> Arc<RenewalStatus> {
        Arc::clone(&self.status)
    }

    pub fn identity(&self) -> Arc<WorkloadIdentity> {
        Arc::clone(&self.identity)
    }

    /// Whether an attempt is due: no certificate yet, or two-thirds of the
    /// installed one's window has passed.
    pub fn is_due(&self) -> bool {
        self.identity
            .validity()
            .is_none_or(|validity| self.clock.now() >= validity.renew_at())
    }

    /// One turn of the loop: attempt if due, and say how long to wait before
    /// the next turn.
    ///
    /// After a success, the wait runs to the new certificate's renewal
    /// instant. After a failure it is the backoff — five seconds, doubling
    /// to five minutes — and the previous certificate keeps serving until
    /// its own not-after, after which the identity presents nothing and the
    /// loop keeps trying. A failure is never answered by anything but
    /// another attempt.
    pub fn tick(&mut self) -> Duration {
        let now = self.clock.now();
        if !self.is_due() {
            return self
                .identity
                .validity()
                .map_or(RETRY_INITIAL, |validity| until(now, validity.renew_at()));
        }
        match self.renew_now() {
            Ok(validity) => {
                self.status.succeeded();
                self.backoff = RETRY_INITIAL;
                until(self.clock.now(), validity.renew_at())
            }
            Err(error) => {
                self.status.failed(&error);
                let wait = self.backoff;
                self.backoff = (self.backoff * 2).min(RETRY_MAXIMUM);
                wait
            }
        }
    }

    /// Generate a fresh key, have it certified, install the result. A new
    /// key every time: a renewal that reused the key would extend the life
    /// of whatever had already stolen it.
    pub fn renew_now(&self) -> Result<Validity> {
        let key = WorkloadKey::generate(self.random.0.as_ref())?;
        let mut suffix = [0u8; 4];
        self.random.0.fill(&mut suffix).map_err(|_| {
            Error::unavailable("the operating system's random source refused a certificate id")
        })?;
        let certificate_id = format!(
            "{}-{}-{}",
            self.workload,
            self.clock.now().as_secs(),
            qip_core::hash::to_hex(&suffix)
        );
        let chain = self.issuer.issue_certificate(&key, &certificate_id)?;
        self.identity.install(&key, chain)
    }

    /// Run [`Self::tick`] on a thread of its own until `stop` is set,
    /// sleeping in slices of at most a second so a stop is seen promptly.
    pub fn spawn(mut self, stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("qip-cert-renewal".to_string())
            .spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let wait = self.tick();
                    let started = std::time::Instant::now();
                    while !stop.load(Ordering::SeqCst) && started.elapsed() < wait {
                        std::thread::sleep(
                            wait.saturating_sub(started.elapsed())
                                .min(Duration::from_secs(1)),
                        );
                    }
                }
            })
            .map_err(|error| {
                Error::io(format!(
                    "cannot start the certificate renewal thread: {error}"
                ))
            })
    }
}

/// From `now` to `then`, at least a second so a loop never spins, at most
/// an hour so a clock step is noticed.
fn until(now: Timestamp, then: Timestamp) -> Duration {
    let nanos = u64::try_from(then.since(now).as_nanos()).unwrap_or(0);
    Duration::from_nanos(nanos).clamp(Duration::from_secs(1), Duration::from_secs(3600))
}

/// A running issued identity: the identity every link shares, what its
/// renewal loop last did, and the loop itself. Dropping it stops the loop —
/// so a composition root holds it for the life of the process.
#[derive(Debug)]
pub struct CasIdentity {
    identity: Arc<WorkloadIdentity>,
    status: Arc<RenewalStatus>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CasIdentity {
    /// Read both root files, build the identity awaiting issuance, make the
    /// first attempt on the calling thread, and start the loop.
    ///
    /// A first attempt that fails does **not** stop the caller: the
    /// identity presents nothing, the status says why, and the loop retries.
    /// A root that binds its listener with such an identity refuses every
    /// handshake and reports not-ready until the CA answers — the
    /// fail-closed state ADR 0112 names — rather than exiting into a restart
    /// loop that would hide the CA's error behind the supervisor's.
    pub fn start(
        settings: &CasSettings,
        workload: &str,
        subject: WorkloadSubject,
        bearer: Arc<dyn BearerSource>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let roots = tls::read(&settings.trust_roots, "CA pool roots")?;
        let google_roots = tls::read(&settings.google_roots, "Google TLS roots")?;
        let identity = Arc::new(WorkloadIdentity::awaiting_issuance(
            &roots,
            Arc::clone(&clock),
        )?);
        let client = CasClient::new(
            settings,
            subject,
            TlsHttpClient::new(
                CasClient::limits(),
                tls::server_authenticated_client(&google_roots)?,
            ),
            bearer,
        );
        Self::run(
            Renewer::new(
                Arc::new(client),
                Arc::clone(&identity),
                os_entropy(),
                clock,
                workload,
            )?,
            identity,
        )
    }

    /// [`Self::start`] for a renewer a caller built — a test's, over its own
    /// issuer.
    pub fn run(mut renewer: Renewer, identity: Arc<WorkloadIdentity>) -> Result<Self> {
        let status = renewer.status();
        // The first attempt, here, so a healthy start is ready before the
        // caller binds anything. Its wait is discarded: the loop's first
        // turn recomputes it from the identity.
        let _ = renewer.tick();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = renewer.spawn(Arc::clone(&stop))?;
        Ok(Self {
            identity,
            status,
            stop,
            thread: Some(thread),
        })
    }

    pub fn identity(&self) -> Arc<WorkloadIdentity> {
        Arc::clone(&self.identity)
    }

    pub fn status(&self) -> Arc<RenewalStatus> {
        Arc::clone(&self.status)
    }

    /// Stop the loop and wait for it.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CasIdentity {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject() -> WorkloadSubject {
        WorkloadSubject::from_service_account("qip-dev-fabricd@proj-1.iam.gserviceaccount.com")
            .expect("a service account")
    }

    /// The body's shape, field by field. Mutation: send the public key's
    /// raw DER instead of the base64 of its PEM, drop the lifetime's `s`, or
    /// drop either key usage, and an assertion fails.
    #[test]
    fn the_request_body_carries_the_key_in_config_and_no_csr() {
        let key = WorkloadKey::generate(&qip_tls::ring::rand::SystemRandom::new()).expect("a key");
        let template = CertificateTemplate::parse(
            "projects/proj-1/locations/europe-west2/certificateTemplates/workload",
        )
        .expect("a template");
        let body = request_body(&key, &subject(), 28_800, Some(&template));
        assert_eq!(body["lifetime"], "28800s");
        assert_eq!(body["certificateTemplate"], template.as_str());
        assert!(body.get("pemCsr").is_none(), "a CSR was sent");
        let public_key = &body["config"]["publicKey"];
        assert_eq!(public_key["format"], "PEM");
        let sent = crate::kms::decode_base64(public_key["key"].as_str().expect("a string"))
            .expect("canonical base64");
        assert_eq!(
            String::from_utf8(sent).expect("PEM text"),
            key.public_key_pem()
        );
        let san = &body["config"]["subjectConfig"]["subjectAltName"];
        assert_eq!(
            san["dnsNames"],
            serde_json::json!(["qip-dev-fabricd.proj-1.iam.gserviceaccount.com"])
        );
        // The pool's template admits exactly one DNS SAN; anything beside it
        // is a request every CA would refuse.
        assert_eq!(
            san.as_object().map(|fields| fields.len()),
            Some(1),
            "the request carries a SAN kind the template refuses: {san}"
        );
        let x509 = &body["config"]["x509Config"];
        assert_eq!(x509["caOptions"]["isCa"], false);
        assert_eq!(x509["keyUsage"]["baseKeyUsage"]["digitalSignature"], true);
        assert_eq!(x509["keyUsage"]["extendedKeyUsage"]["serverAuth"], true);
        assert_eq!(x509["keyUsage"]["extendedKeyUsage"]["clientAuth"], true);
        // Without a template the field is absent, not null: the API reads a
        // null template as a reference to one.
        let bare = request_body(&key, &subject(), 3_600, None);
        assert!(bare.get("certificateTemplate").is_none());
    }
}
