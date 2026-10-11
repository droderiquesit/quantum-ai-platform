//! The pure half of Certificate Authority Service issuance (ADR 0112 §2):
//! the settings, the names, and the in-memory workload key — everything the
//! TLS policy in this crate reads, and nothing that speaks to the CA.
//!
//! Split from `qip_transport::cas` when this crate was cut out of
//! `qip-transport`. [`crate::WorkloadIdentity::install`] takes a
//! [`WorkloadKey`], [`crate::TlsSource`] carries [`CasSettings`], and
//! [`crate::link_source`] refuses stray [`CAS_VARIABLES`], so these types had to live on this side of the
//! cut; `qip-tls` depending back on `qip-transport` would be a cycle and would
//! put the durable fabric's wire back under the peer link, which is what the
//! cut exists to stop. The REST client, the renewal loop and the health
//! status — the parts that need an HTTP client, a KMS bearer or `serde_json`
//! — stay in `qip_transport::cas`, which re-exports everything here so every
//! `qip_transport::cas::X` path still resolves.
//!
//! The only DER this issuance path writes is the fixed
//! `SubjectPublicKeyInfo` prefix around the curve point
//! ([`p256_spki_der`]); the public key goes to the CA in the request's
//! `config` rather than in a CSR, so no ASN.1 request encoder exists.
//!
//! **The name a certificate carries.** The service account's email is not a
//! DNS name, and the platform's peer checks ([`crate::PeerNames`]) are DNS
//! names, so [`WorkloadSubject`] maps `name@project.iam.gserviceaccount.com`
//! to the DNS name `name.project.iam.gserviceaccount.com`. That a workload
//! can obtain a certificate naming only *itself* is enforced by the pool's
//! template and IAM condition, not here; see `qip_transport::cas`.

use crate::{TLS_MODE_VARIABLE, TLS_TRUST_ROOTS_VARIABLE, provider};
use qip_core::error::{Error, Result};
use ring::rand::SecureRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::sign::SigningKey;
use std::path::PathBuf;
use std::sync::Arc;

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

/// RFC 4648 §4 base64, padded — a copy of `qip_transport::kms::encode_base64`,
/// and deliberately a copy. [`pem`] is its only caller on this side of the
/// cut, and this crate cannot reach `qip-transport` without a cycle; moving
/// `kms`'s strict decoder here as well would put a KMS concern in the TLS
/// crate for one twenty-line function. The two are held to one output by
/// test rather than by sharing: `the_spki_encoding_matches_openssl_for_a_known_p256_key`
/// pins this one against openssl's PEM, and `kms`'s own vectors pin that one.
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for (index, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
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
