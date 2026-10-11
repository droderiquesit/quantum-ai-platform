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
//! to the DNS name `name.project.iam.gserviceaccount.com` and requests both
//! that and the email as subject alternative names. That a workload can
//! obtain a certificate naming only *itself* (ADR 0112 §2) is not something
//! this module can enforce — it asks for its own name, and a compromised
//! process could ask for another. CA Service's `DEFAULT` subject mode lets
//! a template constrain the requested names by pattern but not to the
//! caller, so the guarantee needs either one CA pool, or one IAM-conditioned
//! certificate template, per workload identity, or the API's
//! `REFLECTED_SPIFFE` subject mode (which writes the caller's identity as a
//! URI SAN, and would need `PeerNames` to match URIs as well as DNS names).
//! Until the Terraform names one of those, the pool's requester binding is
//! the only control on who may be named.

use crate::http::{ClientLimits, HttpRequest, Method, TlsHttpClient};
use crate::kms::{BearerSource, encode_base64};
use crate::tls::{
    self, TLS_MODE_VARIABLE, TLS_TRUST_ROOTS_VARIABLE, Validity, WorkloadIdentity, provider,
};
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use ring::rand::SecureRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::sign::SigningKey;
use serde::Deserialize;
use std::path::PathBuf;
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

/// The CA pool, as `projects/P/locations/L/caPools/POOL`.
pub const CA_POOL_VARIABLE: &str = "QIP_CAS_CA_POOL";
/// Optional: the certificate template, as
/// `projects/P/locations/L/certificateTemplates/T`.
pub const CERTIFICATE_TEMPLATE_VARIABLE: &str = "QIP_CAS_CERTIFICATE_TEMPLATE";
/// Optional: the lifetime to request, in seconds.
pub const LIFETIME_VARIABLE: &str = "QIP_CAS_LIFETIME_SECS";
/// The pinned roots the REST call to Google is authenticated against — the
/// same variable the API's KMS signer reads.
pub const GOOGLE_TLS_ROOTS_VARIABLE: &str = "QIP_GOOGLE_TLS_ROOTS_PATH";

/// The settings that only mean something under `QIP_TLS_MODE=cas`. The two
/// roots paths are not here: each is also read by another mode or another
/// client, so its presence says nothing about which mode was meant.
pub const CAS_VARIABLES: [&str; 3] = [
    CA_POOL_VARIABLE,
    CERTIFICATE_TEMPLATE_VARIABLE,
    LIFETIME_VARIABLE,
];

/// Eight hours: long enough that a CA outage of a working morning does not
/// stop a link (renewal starts at five hours twenty and retries for the
/// remaining two hours forty), short enough that a stolen key is worth a
/// working day at most.
pub const DEFAULT_LIFETIME_SECS: u64 = 8 * 3600;
/// Below an hour, renewal at two-thirds leaves twenty minutes of retry
/// before a lapse, which is shorter than an ordinary CA Service incident.
pub const MINIMUM_LIFETIME_SECS: u64 = 3600;
/// ADR 0112 says hours, not months; a day is the ceiling of "hours".
pub const MAXIMUM_LIFETIME_SECS: u64 = 24 * 3600;

/// The DER `SubjectPublicKeyInfo` header for an uncompressed P-256 point
/// (RFC 5480 §2): `SEQUENCE (89) { SEQUENCE (19) { OID id-ecPublicKey
/// 1.2.840.10045.2.1, OID prime256v1 1.2.840.10045.3.1.7 }, BIT STRING (66,
/// no unused bits) }`, followed by the 65-byte point. Every P-256 key's SPKI
/// is these 26 bytes and its point, which is why this is the only DER the
/// issuance path writes.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// The DER `SubjectPublicKeyInfo` of an uncompressed P-256 point, refusing
/// anything that is not one (65 bytes, leading `0x04`).
pub fn p256_spki_der(point: &[u8]) -> Result<Vec<u8>> {
    if point.len() != 65 || point.first() != Some(&0x04) {
        return Err(Error::invalid(format!(
            "a P-256 public key is a 65-byte uncompressed point starting 0x04; this one is {} \
             bytes",
            point.len()
        )));
    }
    let mut der = P256_SPKI_PREFIX.to_vec();
    der.extend_from_slice(point);
    Ok(der)
}

/// PEM armour, 64 characters to the line as RFC 7468 §2 writes it.
pub fn pem(label: &str, der: &[u8]) -> String {
    let body = encode_base64(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        // The base64 alphabet is ASCII, so every chunk is valid UTF-8.
        out.push_str(&String::from_utf8_lossy(line));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// The operating system's entropy, as the value a composition root injects
/// (ADR 0043 decision 5): `ring`'s `SystemRandom`, which reads `getrandom`.
pub fn os_entropy() -> Arc<dyn SecureRandom + Send + Sync> {
    Arc::new(ring::rand::SystemRandom::new())
}

/// An ECDSA P-256 key that exists only in this process's memory.
pub struct WorkloadKey {
    signing: Arc<dyn SigningKey>,
    spki: Vec<u8>,
}

// The signing key is never formatted. The public half is safe to print, and
// is the one thing an operator correlating a certificate with a process
// needs.
impl std::fmt::Debug for WorkloadKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadKey")
            .field("spki_sha256", &qip_core::hash::sha256_hex(&self.spki))
            .field("private", &"<in memory only>")
            .finish()
    }
}

impl WorkloadKey {
    /// A fresh key from `random`. The PKCS#8 document `ring` encodes it in
    /// is handed straight to `rustls`'s key loader and dropped; it is never
    /// written, and nothing here can return it.
    ///
    /// Refused: a key whose SPKI as this module encodes it differs from the
    /// SPKI `rustls` derives for the signing key. The CA certifies the bytes
    /// this module sends, and `rustls` presents the key it loaded; if the two
    /// encodings disagreed, the certificate would name a key nothing here
    /// can sign for, and every handshake would fail for a reason nobody
    /// would think to look for.
    pub fn generate(random: &dyn SecureRandom) -> Result<Self> {
        let refused = |what: &str| {
            Error::unavailable(format!(
                "the operating system's random source refused {what}; no workload key exists"
            ))
        };
        let document = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, random)
            .map_err(|_| refused("a key"))?;
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, document.as_ref(), random)
                .map_err(|_| refused("to load the key it had just made"))?;
        let spki = p256_spki_der(pair.public_key().as_ref())?;
        let signing = provider()
            .key_provider
            .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                document.as_ref().to_vec(),
            )))
            .map_err(|error| {
                Error::invalid(format!(
                    "rustls cannot load the workload key ring generated: {error}"
                ))
            })?;
        match signing.public_key() {
            Some(theirs) if theirs.as_ref() == spki.as_slice() => {}
            _ => {
                return Err(Error::invalid(
                    "the public key this process would send the CA differs from the one rustls \
                     would sign with; nothing is requested",
                ));
            }
        }
        Ok(Self { signing, spki })
    }

    /// The DER `SubjectPublicKeyInfo` the CA is asked to certify.
    pub fn spki_der(&self) -> &[u8] {
        &self.spki
    }

    /// The same, as a PEM `PUBLIC KEY` block.
    pub fn public_key_pem(&self) -> String {
        pem("PUBLIC KEY", &self.spki)
    }

    pub(crate) fn signing_key(&self) -> Arc<dyn SigningKey> {
        Arc::clone(&self.signing)
    }
}

/// A `projects/P/locations/L/{collection}/ID` resource name, refusing
/// anything else — including characters that would change the meaning of
/// the URL the name is pasted into (`?`, `#`, `%`, whitespace).
fn resource_name(text: &str, collection: &str, variable: &str) -> Result<String> {
    let text = text.trim();
    let parts: Vec<&str> = text.split('/').collect();
    let legal = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    };
    if parts.len() == 6
        && parts[0] == "projects"
        && parts[2] == "locations"
        && parts[4] == collection
        && [parts[1], parts[3], parts[5]].into_iter().all(legal)
    {
        Ok(text.to_string())
    } else {
        Err(Error::invalid(format!(
            "{variable}={text:?} is not a full resource name; it must be \
             projects/PROJECT/locations/LOCATION/{collection}/ID"
        )))
    }
}

/// A CA pool's full resource name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaPool(String);

impl CaPool {
    pub fn parse(text: &str) -> Result<Self> {
        resource_name(text, "caPools", CA_POOL_VARIABLE).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A certificate template's full resource name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateTemplate(String);

impl CertificateTemplate {
    pub fn parse(text: &str) -> Result<Self> {
        resource_name(text, "certificateTemplates", CERTIFICATE_TEMPLATE_VARIABLE).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What `QIP_TLS_MODE=cas` needs, validated before anything is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CasSettings {
    pub pool: CaPool,
    pub template: Option<CertificateTemplate>,
    pub lifetime_secs: u64,
    /// PEM of the pool's CA certificates: what peers' chains, and this
    /// process's own issued chain, must end in.
    pub trust_roots: PathBuf,
    /// PEM roots the REST call to `privateca.googleapis.com` is
    /// authenticated against.
    pub google_roots: PathBuf,
}

impl CasSettings {
    /// Read the settings through `lookup`, refusing the first thing wrong by
    /// name. The lifetime is refused outside its bounds rather than clamped:
    /// a lifetime silently raised to an hour is a configuration somebody
    /// believes says ten minutes.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let set = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let required = |name: &str, what: &str| {
            set(name).ok_or_else(|| {
                Error::invalid(format!(
                    "{TLS_MODE_VARIABLE}=cas and {name} is not set; set it to {what}"
                ))
            })
        };
        let pool = CaPool::parse(&required(
            CA_POOL_VARIABLE,
            "the CA pool's full resource name, projects/P/locations/L/caPools/POOL",
        )?)?;
        let template = set(CERTIFICATE_TEMPLATE_VARIABLE)
            .map(|text| CertificateTemplate::parse(&text))
            .transpose()?;
        let lifetime_secs = match set(LIFETIME_VARIABLE) {
            None => DEFAULT_LIFETIME_SECS,
            Some(text) => text.parse::<u64>().map_err(|_| {
                Error::invalid(format!(
                    "{LIFETIME_VARIABLE}={text:?} is not a whole number of seconds"
                ))
            })?,
        };
        if !(MINIMUM_LIFETIME_SECS..=MAXIMUM_LIFETIME_SECS).contains(&lifetime_secs) {
            return Err(Error::invalid(format!(
                "{LIFETIME_VARIABLE}={lifetime_secs} is outside {MINIMUM_LIFETIME_SECS} to \
                 {MAXIMUM_LIFETIME_SECS} seconds: under an hour leaves too little time to \
                 retry a renewal before the certificate lapses, and over a day is not the \
                 short-lived certificate ADR 0112 asks for"
            )));
        }
        let trust_roots = PathBuf::from(required(
            TLS_TRUST_ROOTS_VARIABLE,
            "the PEM file of the CA pool's certificates, which every peer's chain and this \
             process's own must end in",
        )?);
        let google_roots = PathBuf::from(required(
            GOOGLE_TLS_ROOTS_VARIABLE,
            "the pinned PEM roots the call to privateca.googleapis.com is verified against",
        )?);
        Ok(Self {
            pool,
            template,
            lifetime_secs,
            trust_roots,
            google_roots,
        })
    }
}

/// The names a workload's certificate carries, derived from its service
/// account so that the certificate's SAN is "mapped to the workload's
/// service account" as ADR 0112 §1 says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkloadSubject {
    email: String,
    dns_name: String,
}

impl WorkloadSubject {
    /// `name@domain` becomes the DNS name `name.domain`. Refused: anything
    /// that is not one `@` between two non-empty lowercase DNS-shaped
    /// halves, because a name that cannot be a DNS SAN can never be matched
    /// by a peer's admitted-name list.
    pub fn from_service_account(email: &str) -> Result<Self> {
        let email = email.trim();
        let refuse = || {
            Error::invalid(format!(
                "{email:?} is not a service-account email this module can name a certificate \
                 after (name@project.iam.gserviceaccount.com)"
            ))
        };
        let (local, domain) = email.split_once('@').ok_or_else(refuse)?;
        let dns_shaped = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        };
        if !dns_shaped(local) || !dns_shaped(domain) || local.contains('.') {
            return Err(refuse());
        }
        let dns_name = format!("{local}.{domain}");
        match ServerName::try_from(dns_name.as_str()) {
            Ok(ServerName::DnsName(_)) => {}
            _ => return Err(refuse()),
        }
        Ok(Self {
            email: email.to_string(),
            dns_name,
        })
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    /// The DNS SAN, and the name a peer's admitted list must carry.
    pub fn dns_name(&self) -> &str {
        &self.dns_name
    }
}

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
                    "dnsNames": [subject.dns_name()],
                    "emailAddresses": [subject.email()],
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
pub trait CertificateIssuer: Send + Sync + std::fmt::Debug {
    fn issue(
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
    fn issue(
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
        let chain = self.issuer.issue(&key, &certificate_id)?;
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

    /// The public half of a P-256 key `openssl ecparam -name prime256v1
    /// -genkey` generated for this test, and `openssl ec -pubout`'s PEM of
    /// it. Public material only; the private half was deleted unread. An
    /// encoder checked against a second implementation, not against itself.
    const OPENSSL_POINT: &str = "04ca196142103abf06ca62b01f6992aa5c668d0220f4b8af87df7008b0f35d2def6b7589296645eead43a3b62f398d122ab20c653183569fc8852094aedbf485f3";
    const OPENSSL_PEM: &str = "-----BEGIN PUBLIC KEY-----\n\
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEyhlhQhA6vwbKYrAfaZKqXGaNAiD0\n\
uK+H33AIsPNdLe9rdYkpZkXurUOjti85jRIqsgxlMYNWn8iFIJSu2/SF8w==\n\
-----END PUBLIC KEY-----\n";

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// Mutation: change any byte of `P256_SPKI_PREFIX` (or the PEM line
    /// width) and the output stops matching openssl's; this fails.
    #[test]
    fn the_spki_encoding_matches_openssl_for_a_known_p256_key() {
        let point = unhex(OPENSSL_POINT);
        assert_eq!(
            point.len(),
            65,
            "premise: the vector is an uncompressed point"
        );
        let der = p256_spki_der(&point).expect("a valid point");
        assert_eq!(pem("PUBLIC KEY", &der), OPENSSL_PEM);
    }

    /// The same encoding, against `rustls`'s own SPKI for a key it loaded:
    /// a fresh key's SPKI as sent to the CA is byte-for-byte the one
    /// `rustls` will present. Mutation: drop the point from `p256_spki_der`
    /// or alter the prefix and `generate` refuses; this fails.
    #[test]
    fn a_generated_key_sends_the_ca_the_spki_rustls_signs_under() {
        let key = WorkloadKey::generate(&ring::rand::SystemRandom::new()).expect("a key");
        let theirs = key
            .signing
            .public_key()
            .expect("ring keys expose their SPKI");
        assert_eq!(key.spki_der(), theirs.as_ref());
        assert_eq!(key.spki_der().len(), 91);
    }

    /// Mutation: accept a 64-byte or compressed point and this fails.
    #[test]
    fn a_point_that_is_not_uncompressed_p256_is_refused() {
        let point = unhex(OPENSSL_POINT);
        assert!(p256_spki_der(&point[1..]).is_err());
        let mut compressed = point[..33].to_vec();
        compressed[0] = 0x02;
        assert!(p256_spki_der(&compressed).is_err());
        let mut wrong_prefix = point;
        wrong_prefix[0] = 0x05;
        assert!(p256_spki_der(&wrong_prefix).is_err());
    }

    fn subject() -> WorkloadSubject {
        WorkloadSubject::from_service_account("qip-dev-fabricd@proj-1.iam.gserviceaccount.com")
            .expect("a service account")
    }

    /// The body's shape, field by field. Mutation: send the public key's
    /// raw DER instead of the base64 of its PEM, drop the lifetime's `s`, or
    /// drop either key usage, and an assertion fails.
    #[test]
    fn the_request_body_carries_the_key_in_config_and_no_csr() {
        let key = WorkloadKey::generate(&ring::rand::SystemRandom::new()).expect("a key");
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
        assert_eq!(
            san["emailAddresses"],
            serde_json::json!(["qip-dev-fabricd@proj-1.iam.gserviceaccount.com"])
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

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    const POOL: &str = "projects/proj-1/locations/europe-west2/caPools/qip-dev-internal";

    fn complete() -> Vec<(&'static str, &'static str)> {
        vec![
            (CA_POOL_VARIABLE, POOL),
            (TLS_TRUST_ROOTS_VARIABLE, "/etc/qip/pool-roots.pem"),
            (GOOGLE_TLS_ROOTS_VARIABLE, "/etc/qip/google-roots.pem"),
        ]
    }

    /// Premise first: the complete set parses, with the default lifetime.
    /// Mutation: drop the shape check in `resource_name` and the malformed
    /// pools parse; this fails.
    #[test]
    fn a_malformed_ca_pool_is_refused_by_name() {
        let settings = CasSettings::from_lookup(&lookup(&complete())).expect("complete");
        assert_eq!(settings.pool.as_str(), POOL);
        assert_eq!(settings.lifetime_secs, DEFAULT_LIFETIME_SECS);
        for bad in [
            "qip-dev-internal",
            "projects/proj-1/locations/europe-west2/caPools/",
            "projects/proj-1/locations/europe-west2/certificateAuthorities/x",
            "projects/proj-1/locations/europe-west2/caPools/x/certificates/y",
            "projects/proj-1/locations/europe-west2/caPools/x?certificateId=forged",
            "projects/proj 1/locations/europe-west2/caPools/x",
        ] {
            let mut pairs = complete();
            pairs[0] = (CA_POOL_VARIABLE, bad);
            let error = CasSettings::from_lookup(&lookup(&pairs))
                .expect_err(&format!("{bad:?} was accepted as a pool"));
            assert!(error.message().contains(CA_POOL_VARIABLE), "{error}");
        }
    }

    /// Mutation: clamp the lifetime instead of refusing, or widen either
    /// bound, and one of these parses; this fails.
    #[test]
    fn a_lifetime_outside_one_hour_to_one_day_is_refused_not_clamped() {
        for (text, admitted) in [
            ("3599", false),
            ("3600", true),
            ("28800", true),
            ("86400", true),
            ("86401", false),
            ("0", false),
            ("8h", false),
        ] {
            let mut pairs = complete();
            pairs.push((LIFETIME_VARIABLE, text));
            let outcome = CasSettings::from_lookup(&lookup(&pairs));
            assert_eq!(outcome.is_ok(), admitted, "{LIFETIME_VARIABLE}={text}");
            if let Ok(settings) = outcome {
                assert_eq!(
                    settings.lifetime_secs.to_string(),
                    text,
                    "the value was changed"
                );
            }
        }
    }

    /// Mutation: default either roots path, or make the pool optional, and
    /// this fails.
    #[test]
    fn cas_without_its_roots_or_its_pool_is_refused() {
        for missing in [
            CA_POOL_VARIABLE,
            TLS_TRUST_ROOTS_VARIABLE,
            GOOGLE_TLS_ROOTS_VARIABLE,
        ] {
            let pairs: Vec<_> = complete()
                .into_iter()
                .filter(|(name, _)| *name != missing)
                .collect();
            let error = CasSettings::from_lookup(&lookup(&pairs))
                .expect_err(&format!("cas without {missing} was accepted"));
            assert!(error.message().contains(missing), "{error}");
        }
        let mut pairs = complete();
        pairs.push((CERTIFICATE_TEMPLATE_VARIABLE, "workload"));
        assert!(CasSettings::from_lookup(&lookup(&pairs)).is_err());
    }

    /// Mutation: accept a dotted local part or an uppercase name and this
    /// fails — either would produce a DNS SAN no admitted list can match.
    #[test]
    fn only_a_dns_shaped_service_account_names_a_certificate() {
        assert_eq!(
            subject().dns_name(),
            "qip-dev-fabricd.proj-1.iam.gserviceaccount.com"
        );
        for bad in [
            "no-at-sign",
            "@proj.iam.gserviceaccount.com",
            "name@",
            "Name@proj.iam.gserviceaccount.com",
            "a.b@proj.iam.gserviceaccount.com",
            "name@proj iam",
        ] {
            assert!(
                WorkloadSubject::from_service_account(bad).is_err(),
                "{bad:?} was accepted"
            );
        }
    }
}
