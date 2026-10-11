//! Mutual TLS where the platform owns the socket (ADR 0112), through
//! `rustls` on the `ring` provider.
//!
//! A crate of its own since the Reflex Mesh was held apart from the durable
//! fabric (R12-COVERAGE-7194). It was `qip_transport::tls`, and that path
//! still resolves through a re-export, but the peer link now depends on this
//! crate alone and so reaches none of the fabric's wire.
//!
//! Until this module every link this crate opened was plaintext with a bearer
//! token for identity, and the crate's own documentation said "production
//! must add mutual TLS". ADR 0112 decides where: between processes on Compute
//! Engine (edge node ↔ fabric broker, broker ↔ ledger, cell ↔ cell), and out
//! to Google APIs. Into a Cloud Run service Google terminates TLS, so nothing
//! here applies to that hop.
//!
//! What this module holds, and what it deliberately does not:
//!
//! * [`WorkloadIdentity`] — this process's certificate chain and private key
//!   and the CA roots its peers chain to. Two sources fill it: **files** a
//!   composition root names (a developer workstation, `QIP_TLS_MODE=mtls`),
//!   or **Certificate Authority Service** (`QIP_TLS_MODE=cas`, ADR 0112 §2,
//!   [`crate::cas`]), which issues a certificate of hours over a key
//!   generated in memory and renews it at two-thirds of its life. Either
//!   way the material sits in one swappable slot that every config built
//!   from the identity reads at each handshake, so a renewal reaches the
//!   next connection without rebuilding a server or restarting a process —
//!   and a certificate outside its window is never presented: the slot
//!   answers "nothing" and the handshake fails, rather than carrying an
//!   expired certificate that each peer would refuse for its own reason.
//!   This crate never writes a key anywhere.
//! * [`LinkSecurity`] — the one decision a root makes per link: mutual TLS,
//!   or plaintext. Plaintext exists only when configuration says so *and* the
//!   deployment is [`Deployment::Local`]; there is no fallback from one to
//!   the other at run time. A link configured for TLS that cannot load its
//!   identity stops the process, because a link that quietly came up in
//!   plaintext would read, on every dashboard, as the encrypted link the
//!   configuration describes.
//! * TLS 1.3 only. `rustls` is built without its `tls12` feature and every
//!   config here names [`rustls::version::TLS13`], so a peer that can only
//!   offer 1.2 fails the handshake rather than negotiating down.
//! * The server always requires a client certificate chained to the
//!   configured roots ([`WorkloadIdentity::server_config`]). There is no
//!   "optional client auth" builder call anywhere in this file, and a server
//!   built here cannot be asked for one.
//!
//! **What a certificate proves here.** That the peer holds a key a CA in the
//! configured pool certified. ADR 0112's CA Service template constrains the
//! SAN to the caller's own identity, so a node can only obtain a certificate
//! naming itself; the server can additionally require the peer's certificate
//! to be valid for one of a list of names ([`PeerNames`]), checked by
//! `webpki` against the certificate's DNS SANs. The bearer token the fabric
//! carried is kept on top for now, as authorisation — ADR 0112 says
//! certificate identity replaces it, and that replacement is the broker's
//! change, not this module's.

use crate::cas::{CasSettings, WorkloadKey};
use qip_core::deployment::Deployment;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, SystemClock, Timestamp};
use rustls::RootCertStore;
use rustls::SignatureScheme;
use rustls::client::ResolvesClientCert;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

// The session types a caller drives on a socket it owns, re-exported so
// that caller takes them from here rather than naming `rustls` itself. The
// peer link (ADR 0116) drives these connections on its own wire thread; with
// its own `rustls` line it was a second manifest pinning the same version,
// features and provider, and `cargo deny`'s workspace-duplicate check failed
// on exactly that: two pins of one crate that nothing forced to agree. One
// pin here means a version bump of the TLS stack is one edit, and the link
// cannot drift to a provider or a protocol version this module refuses.
pub use rustls::pki_types::ServerName;
pub use rustls::{ClientConfig, ClientConnection, ServerConfig, ServerConnection};

// The three TLS crates, re-exported whole for the crates that drive a
// session or verify a signature themselves (`qip-transport`'s client, server,
// trust bundle and KMS signer, and the mTLS test support). This crate is the
// one manifest that pins them; see its `Cargo.toml`.
pub use ring;
pub use rustls;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
pub use webpki;

/// The provider every config here is built on: `ring`, so the platform's TLS
/// and its artefact verification share one implementation of the primitives
/// (ADR 0110 §3).
pub(crate) fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn tls_error(context: &str, error: impl std::fmt::Display) -> Error {
    Error::invalid(format!("{context}: {error}"))
}

/// Where a workload's TLS material is on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsPaths {
    /// PEM certificate chain, leaf first.
    pub certificate_chain: PathBuf,
    /// PEM private key (PKCS#8, SEC1 or PKCS#1). Read once; never logged,
    /// never copied anywhere but the `rustls` config.
    pub private_key: PathBuf,
    /// PEM roots the peer's chain must end in — the CA pool's certificates.
    pub trust_roots: PathBuf,
}

/// The validity window of an installed certificate, read from the leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Validity {
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

impl Validity {
    /// The instant ADR 0112 renews at: two-thirds of the way through the
    /// certificate's own window. Read from the certificate rather than from
    /// the lifetime that was asked for, because a CA may shorten a requested
    /// lifetime to its pool's maximum or to its own expiry, and a renewal
    /// scheduled on the request would then fall after the real expiry.
    pub fn renew_at(&self) -> Timestamp {
        let span = self.not_after.since(self.not_before).as_nanos().max(0);
        self.not_before
            .saturating_add(Duration::from_nanos(span / 3 * 2))
    }

    /// Whether `now` is inside the window, as a peer's verifier judges it.
    pub fn admits(&self, now: Timestamp) -> bool {
        self.not_before <= now && now < self.not_after
    }
}

/// One certificate and the key it certifies, as `rustls` presents them.
#[derive(Clone)]
struct Installed {
    certified: Arc<CertifiedKey>,
    validity: Validity,
}

/// The swappable cell every config built from one identity reads.
///
/// A `RwLock` rather than anything cleverer: a handshake takes the read lock
/// for one `Arc` clone, and a renewal takes the write lock once every few
/// hours.
struct CertificateSlot {
    installed: RwLock<Option<Installed>>,
    clock: Arc<dyn Clock>,
}

impl CertificateSlot {
    /// The certificate to present now, or nothing. Nothing when none has
    /// been installed, and nothing when the installed one is outside its
    /// window: ADR 0112's fail-closed rule is that a process without a valid
    /// certificate stops making and accepting mutual-TLS connections, and
    /// presenting an expired one would be making them.
    fn current(&self) -> Option<Arc<CertifiedKey>> {
        let now = self.clock.now();
        let guard = self
            .installed
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        guard
            .as_ref()
            .filter(|installed| installed.validity.admits(now))
            .map(|installed| Arc::clone(&installed.certified))
    }

    fn validity(&self) -> Option<Validity> {
        self.installed
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|installed| installed.validity)
    }

    fn replace(&self, installed: Installed) {
        *self
            .installed
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(installed);
    }
}

// Neither the key nor the certificate is printed: the slot is reachable from
// every config, and a `Debug` that walked into it would put a signing key's
// description into whatever log formatted the config.
impl std::fmt::Debug for CertificateSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateSlot")
            .field("validity", &self.validity())
            .finish_non_exhaustive()
    }
}

/// The resolver both config halves are built on: whatever the slot holds at
/// the instant of the handshake. This is what makes a renewal reach a server
/// that was built once — `rustls` asks the resolver per connection, where
/// `with_single_cert` fixed the certificate for the config's whole life and
/// a renewal meant rebuilding the server.
#[derive(Debug)]
struct Presented(Arc<CertificateSlot>);

impl ResolvesServerCert for Presented {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.0.current()
    }
}

impl ResolvesClientCert for Presented {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        self.0.current()
    }

    fn has_certs(&self) -> bool {
        self.0.current().is_some()
    }
}

/// This process's certificate, its key, and the roots it trusts.
pub struct WorkloadIdentity {
    slot: Arc<CertificateSlot>,
    roots: Arc<RootCertStore>,
}

// The key is deliberately absent from `Debug`: a struct that prints its own
// secret gets it into a log the first time anything derives `Debug` on a
// type that contains it.
impl std::fmt::Debug for WorkloadIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadIdentity")
            .field("validity", &self.slot.validity())
            .field("roots", &self.roots.len())
            .field("key", &"<redacted>")
            .finish()
    }
}

impl WorkloadIdentity {
    /// Parse PEM material, refusing an empty chain, a missing key or an
    /// empty root set — each would build a config that fails every
    /// handshake, which is better found at start-up than on the first call.
    /// Refused for the same reason: a key that does not match the leaf, and
    /// a leaf already past its not-after.
    pub fn from_pem(chain_pem: &[u8], key_pem: &[u8], roots_pem: &[u8]) -> Result<Self> {
        Self::from_pem_at(chain_pem, key_pem, roots_pem, Arc::new(SystemClock))
    }

    /// [`Self::from_pem`] judged by `clock`, which decides from then on
    /// whether the certificate is still presented.
    pub fn from_pem_at(
        chain_pem: &[u8],
        key_pem: &[u8],
        roots_pem: &[u8],
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let chain = CertificateDer::pem_slice_iter(chain_pem)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| tls_error("the workload certificate chain does not parse", error))?;
        let Some(leaf) = chain.first() else {
            return Err(Error::invalid(
                "the workload certificate chain holds no certificate; a TLS link cannot present \
                 an identity it does not have",
            ));
        };
        let validity = validity_of(leaf)?;
        let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|error| {
            // The error from the PEM reader names a section kind, never the
            // key's bytes, so it is safe to carry.
            tls_error("the workload private key does not parse", error)
        })?;
        let roots = roots_from_pem(roots_pem)?;
        let certified = CertifiedKey::from_der(chain, key, &provider())
            .map_err(|error| tls_error("the workload certificate and key do not match", error))?;
        if clock.now() >= validity.not_after {
            return Err(Error::invalid(format!(
                "the workload certificate expired at {}; every peer would refuse it. Replace \
                 the certificate files, or let Certificate Authority Service issue one \
                 ({TLS_MODE_VARIABLE}=cas)",
                validity.not_after
            )));
        }
        let identity = Self::awaiting_issuance_with_roots(roots, clock);
        identity.slot.replace(Installed {
            certified: Arc::new(certified),
            validity,
        });
        Ok(identity)
    }

    /// Read the three files and parse them.
    pub fn from_files(paths: &TlsPaths) -> Result<Self> {
        Self::from_pem(
            &read(&paths.certificate_chain, "workload certificate chain")?,
            &read(&paths.private_key, "workload private key")?,
            &read(&paths.trust_roots, "TLS trust roots")?,
        )
    }

    /// An identity holding the pool's roots and no certificate yet: every
    /// handshake through a config built from it fails until
    /// [`Self::install`] succeeds. A composition root on Certificate
    /// Authority Service builds this, binds its listener and hands the
    /// identity to the renewal loop, so the listener refuses every peer
    /// until the first certificate arrives — ADR 0112's fail-closed start,
    /// rather than a window in which it serves without one.
    pub fn awaiting_issuance(roots_pem: &[u8], clock: Arc<dyn Clock>) -> Result<Self> {
        Ok(Self::awaiting_issuance_with_roots(
            roots_from_pem(roots_pem)?,
            clock,
        ))
    }

    fn awaiting_issuance_with_roots(roots: RootCertStore, clock: Arc<dyn Clock>) -> Self {
        Self {
            slot: Arc::new(CertificateSlot {
                installed: RwLock::new(None),
                clock,
            }),
            roots: Arc::new(roots),
        }
    }

    /// Install a freshly issued certificate over `key`, replacing whatever
    /// the slot held, after checking the three things whose absence would
    /// let a wrong certificate be presented without anyone noticing:
    ///
    /// * **The leaf certifies `key`.** A CA, or anything answering for it,
    ///   that returned a certificate over another public key would hand this
    ///   process an identity it cannot sign for — or one somebody else can.
    /// * **The chain verifies to this identity's roots, for both server and
    ///   client authentication, now.** The roots are the CA pool's
    ///   certificates; a chain ending anywhere else would be refused by every
    ///   peer, and finding that here keeps the old certificate in place
    ///   instead of swapping in one nobody accepts.
    /// * **The window is the one `webpki` enforces.** The not-after is read
    ///   from the leaf by a small DER walk and then checked against `webpki`
    ///   itself: a certificate `webpki` still accepts one second after the
    ///   not-after this module read would mean the walk misread it, and a
    ///   renewal scheduled on a misread expiry is one that comes too late.
    ///
    /// On refusal the slot is untouched: a renewal that fails leaves the
    /// previous certificate serving until its own expiry.
    pub fn install(
        &self,
        key: &WorkloadKey,
        chain: Vec<CertificateDer<'static>>,
    ) -> Result<Validity> {
        let Some(leaf_der) = chain.first() else {
            return Err(Error::denied(
                "the issued certificate chain is empty; there is nothing to install",
            ));
        };
        let leaf = webpki::EndEntityCert::try_from(leaf_der).map_err(|error| {
            Error::denied(format!(
                "the issued certificate does not parse as an end-entity certificate: {error}"
            ))
        })?;
        if leaf.subject_public_key_info().as_ref() != key.spki_der() {
            return Err(Error::denied(
                "the issued certificate certifies a different public key from the one this \
                 process generated and sent; it is discarded rather than presented, because \
                 this process cannot sign for it and whoever can is not this process",
            ));
        }
        let now = self.slot.clock.now();
        let verify_at = |instant: Timestamp| -> std::result::Result<(), webpki::Error> {
            let time = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
                u64::try_from(instant.as_secs()).unwrap_or(0),
            ));
            let algorithms = provider().signature_verification_algorithms.all;
            for usage in [
                webpki::KeyUsage::server_auth(),
                webpki::KeyUsage::client_auth(),
            ] {
                leaf.verify_for_usage(
                    algorithms,
                    &self.roots.roots,
                    &chain[1..],
                    time,
                    usage,
                    None,
                    None,
                )?;
            }
            Ok(())
        };
        verify_at(now).map_err(|error| {
            Error::denied(format!(
                "the issued certificate does not verify, now, to the configured CA pool roots \
                 for both server and client authentication: {error:?}. Check that \
                 {TLS_TRUST_ROOTS_VARIABLE} holds the pool's CA certificates and that the \
                 certificate is neither expired nor not yet valid"
            ))
        })?;
        let validity = validity_of(leaf_der)?;
        if !validity.admits(now) {
            return Err(Error::denied(format!(
                "the issued certificate is valid from {} to {}, which does not include now; it \
                 is not installed",
                validity.not_before, validity.not_after
            )));
        }
        if verify_at(validity.not_after.saturating_add(Duration::from_secs(1))).is_ok() {
            return Err(Error::invalid(format!(
                "the issued certificate's not-after was read as {}, and webpki still accepts the \
                 certificate a second later; the window was misread, and a renewal scheduled on \
                 it could fall after the real expiry",
                validity.not_after
            )));
        }
        let certified = CertifiedKey::new(chain, key.signing_key());
        certified.keys_match().map_err(|error| {
            Error::denied(format!(
                "rustls cannot confirm the issued certificate matches this process's key: \
                 {error}"
            ))
        })?;
        self.slot.replace(Installed {
            certified: Arc::new(certified),
            validity,
        });
        Ok(validity)
    }

    /// Whether a handshake now would present a certificate.
    pub fn is_current(&self) -> bool {
        self.slot.current().is_some()
    }

    /// The installed certificate's window, expired or not; `None` before
    /// the first installation.
    pub fn validity(&self) -> Option<Validity> {
        self.slot.validity()
    }

    /// A server config that **requires** a client certificate chained to
    /// this identity's roots, over TLS 1.3 only, presenting whatever
    /// certificate the identity holds at each handshake.
    pub fn server_config(&self) -> Result<Arc<ServerConfig>> {
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::clone(&self.roots),
            provider(),
        )
        .build()
        .map_err(|error| tls_error("cannot build the client-certificate verifier", error))?;
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| tls_error("cannot select TLS 1.3", error))?
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(Arc::new(Presented(Arc::clone(&self.slot))));
        // No session resumption, in either half. A TLS 1.3 resumed session
        // presents no certificate: it carries forward the identity the
        // original handshake proved. With resumption on, a client whose
        // certificate had lapsed kept being served on its ticket, and a
        // server kept "presenting" the certificate it had before a renewal
        // — `a_server_built_once_presents_the_renewed_certificate_to_the_next_connection`
        // and the expiry test both failed that way before this. Every
        // connection proving a certificate that is current *now* is the
        // property ADR 0112's fail-closed rule needs; the cost is a full
        // handshake per connection, which keep-alive amortises.
        config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
        config.send_tls13_tickets = 0;
        Ok(Arc::new(config))
    }

    /// A client config that presents this identity and verifies the server
    /// against the same roots, over TLS 1.3 only. Like the server half, it
    /// reads the identity's slot at each handshake.
    pub fn client_config(&self) -> Result<Arc<ClientConfig>> {
        let mut config = ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| tls_error("cannot select TLS 1.3", error))?
            .with_root_certificates(Arc::clone(&self.roots))
            .with_client_cert_resolver(Arc::new(Presented(Arc::clone(&self.slot))));
        // See `server_config`: a resumed session would skip the certificate
        // this identity is supposed to prove current on every connection.
        config.resumption = rustls::client::Resumption::disabled();
        Ok(Arc::new(config))
    }
}

/// The validity window of a DER certificate.
///
/// A deliberately small walk: `Certificate ::= SEQUENCE { tbsCertificate
/// SEQUENCE { [0] version OPTIONAL, serialNumber INTEGER, signature
/// SEQUENCE, issuer SEQUENCE, validity SEQUENCE { notBefore Time, notAfter
/// Time }, … } … }` (RFC 5280 §4.1), where `Time` is `UTCTime` or
/// `GeneralizedTime`. `webpki` parses the same field but does not expose it,
/// and the renewal schedule needs it; [`WorkloadIdentity::install`] checks
/// what this reads against `webpki`'s own verdict.
fn validity_of(certificate: &[u8]) -> Result<Validity> {
    let refuse = || {
        Error::invalid(
            "the certificate's validity cannot be read; it is not a DER X.509 certificate",
        )
    };
    let (_, certificate, _) = der_element(certificate, Some(0x30)).ok_or_else(refuse)?;
    let (_, tbs, _) = der_element(certificate, Some(0x30)).ok_or_else(refuse)?;
    let mut rest = tbs;
    if rest.first() == Some(&0xa0) {
        rest = der_element(rest, Some(0xa0)).ok_or_else(refuse)?.2;
    }
    // serialNumber, signature, issuer.
    for tag in [0x02, 0x30, 0x30] {
        rest = der_element(rest, Some(tag)).ok_or_else(refuse)?.2;
    }
    let (_, validity, _) = der_element(rest, Some(0x30)).ok_or_else(refuse)?;
    let (before_tag, before, rest) = der_element(validity, None).ok_or_else(refuse)?;
    let (after_tag, after, _) = der_element(rest, None).ok_or_else(refuse)?;
    Ok(Validity {
        not_before: der_time(before_tag, before).ok_or_else(refuse)?,
        not_after: der_time(after_tag, after).ok_or_else(refuse)?,
    })
}

/// One DER element: its tag, its contents, and what follows it. `None` on a
/// truncated or indefinite-length element, or a tag other than `expected`.
fn der_element(input: &[u8], expected: Option<u8>) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    if expected.is_some_and(|expected| expected != tag) {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let length = rest[..count]
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        (length, &rest[count..])
    };
    if rest.len() < length {
        return None;
    }
    Some((tag, &rest[..length], &rest[length..]))
}

/// `UTCTime` (`YYMMDDHHMMSSZ`; RFC 5280 §4.1.2.5.1 reads `YY` below 50 as
/// 20YY) or `GeneralizedTime` (`YYYYMMDDHHMMSSZ`): the only two forms RFC
/// 5280 permits in a certificate, and only with seconds and `Z`.
fn der_time(tag: u8, text: &[u8]) -> Option<Timestamp> {
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let slice = text.get(range)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    let (year, offset) = match (tag, text.len()) {
        (0x17, 13) => {
            let short = digits(0..2)?;
            (
                if short < 50 {
                    2000 + short
                } else {
                    1900 + short
                },
                2,
            )
        }
        (0x18, 15) => (digits(0..4)?, 4),
        _ => return None,
    };
    if text.last() != Some(&b'Z') {
        return None;
    }
    let month = digits(offset..offset + 2)?;
    let day = digits(offset + 2..offset + 4)?;
    let hour = digits(offset + 4..offset + 6)?;
    let minute = digits(offset + 6..offset + 8)?;
    let second = digits(offset + 8..offset + 10)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let date = Timestamp::from_civil_checked(
        i32::try_from(year).ok()?,
        u32::try_from(month).ok()?,
        u32::try_from(day).ok()?,
    )?;
    Some(date.saturating_add(Duration::from_secs(hour * 3600 + minute * 60 + second)))
}

/// A client config that authenticates the server only, against a pinned
/// root set, for calls out to a Google API (ADR 0112's third row): Cloud
/// KMS, Spanner, Storage. No client certificate — the caller is identified
/// by its metadata-server bearer token.
///
/// The roots are a file, not a compiled-in list: no root-store crate is
/// admitted, and a pinned file baked into the attested image is the same
/// trust decision the trust bundle makes (ADR 0110 decision 2).
pub fn server_authenticated_client(roots_pem: &[u8]) -> Result<Arc<ClientConfig>> {
    let roots = roots_from_pem(roots_pem)?;
    let config = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| tls_error("cannot select TLS 1.3", error))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn roots_from_pem(pem: &[u8]) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(pem) {
        let certificate =
            certificate.map_err(|error| tls_error("a TLS trust root does not parse", error))?;
        roots
            .add(certificate)
            .map_err(|error| tls_error("a TLS trust root is not a usable CA", error))?;
    }
    if roots.is_empty() {
        return Err(Error::invalid(
            "the TLS trust roots file holds no certificate, so no peer could ever be verified",
        ));
    }
    Ok(roots)
}

pub(crate) fn read(path: &Path, what: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| {
        Error::invalid(format!(
            "cannot read the {what} at {}: {error}",
            path.display()
        ))
    })
}

/// The names a server accepts a client certificate for, checked against the
/// certificate's DNS subject alternative names.
///
/// Empty means "any certificate the roots chain" — the pool's own template is
/// then the only identity control, which is what ADR 0112 describes for a CA
/// pool per environment. Non-empty narrows it to the listed workloads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerNames(Vec<String>);

impl PeerNames {
    pub fn new(names: impl IntoIterator<Item = String>) -> Result<Self> {
        let names: Vec<String> = names.into_iter().collect();
        for name in &names {
            ServerName::try_from(name.as_str()).map_err(|error| {
                Error::invalid(format!(
                    "{name:?} is not a DNS name a certificate can carry: {error}"
                ))
            })?;
        }
        Ok(Self(names))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The first listed name the leaf certificate is valid for, or a refusal.
    pub fn admit(&self, leaf: &CertificateDer<'_>) -> Result<Option<String>> {
        if self.0.is_empty() {
            return Ok(None);
        }
        let certificate = webpki::EndEntityCert::try_from(leaf).map_err(|error| {
            Error::denied(format!(
                "the peer's certificate does not parse as an end-entity: {error}"
            ))
        })?;
        for name in &self.0 {
            let Ok(server_name) = ServerName::try_from(name.as_str()) else {
                continue;
            };
            if certificate
                .verify_is_valid_for_subject_name(&server_name)
                .is_ok()
            {
                return Ok(Some(name.clone()));
            }
        }
        Err(Error::denied(format!(
            "the peer's certificate names none of the {} workload(s) this server admits",
            self.0.len()
        )))
    }
}

/// The server half of a mutual-TLS link: the config and the names it admits.
#[derive(Clone, Debug)]
pub struct TlsServer {
    pub config: Arc<ServerConfig>,
    pub peers: PeerNames,
}

/// How one link is secured. Chosen once, at the composition root.
#[derive(Clone, Debug)]
pub enum LinkSecurity {
    /// Plaintext. Constructible only through [`LinkSecurity::configure`] with
    /// an explicit `plaintext` mode in a [`Deployment::Local`] process, or by
    /// [`LinkSecurity::plaintext_for_tests`].
    Plaintext,
    /// Mutual TLS 1.3 with this process's workload identity.
    MutualTls(Arc<WorkloadIdentity>),
}

/// The decision [`LinkSecurity::configure`] makes, without reading a file:
/// `None` for plaintext, the paths for mutual TLS, or the refusal. Pure, so a
/// composition root can validate its configuration before it opens anything
/// and load the identity later, in the order it proves storage and binds
/// ports.
///
/// * `mtls` — the paths are required. Never falls back.
/// * `plaintext` — admitted only in [`Deployment::Local`]; anywhere else it
///   is refused, because ADR 0112 admits no plaintext link between deployed
///   workloads.
/// * absent — mutual TLS. Plaintext is never the default: a developer must
///   say `plaintext` out loud, so an omission cannot produce one.
pub fn link_mode(
    deployment: Deployment,
    mode: Option<&str>,
    paths: Option<&TlsPaths>,
) -> Result<Option<TlsPaths>> {
    let mode = mode.map(str::trim).filter(|mode| !mode.is_empty());
    match mode {
        Some("plaintext") => {
            if deployment.permits_plaintext_links() {
                Ok(None)
            } else {
                Err(Error::denied(format!(
                    "{TLS_MODE_VARIABLE}=plaintext in the {deployment} deployment; a link \
                     between deployed workloads is mutual TLS (ADR 0112), and plaintext is \
                     admitted only where the deployment is `local`. Set the three \
                     {TLS_CERTIFICATE_CHAIN_VARIABLE}, {TLS_PRIVATE_KEY_VARIABLE} and \
                     {TLS_TRUST_ROOTS_VARIABLE} files and {TLS_MODE_VARIABLE}=mtls"
                )))
            }
        }
        Some("mtls") | None => match paths {
            Some(paths) => Ok(Some(paths.clone())),
            None => Err(Error::invalid(format!(
                "this link is mutual TLS ({TLS_MODE_VARIABLE} is {}) and its workload \
                 certificate is not configured; set {TLS_CERTIFICATE_CHAIN_VARIABLE}, \
                 {TLS_PRIVATE_KEY_VARIABLE} and {TLS_TRUST_ROOTS_VARIABLE}, or — on a \
                 developer machine only — {TLS_MODE_VARIABLE}=plaintext with \
                 QIP_DEPLOYMENT=local. There is no fallback to plaintext",
                mode.unwrap_or("unset, which means mtls")
            ))),
        },
        // `cas` is a mode, but not one this function can satisfy: its
        // identity is issued by a renewal loop the composition root runs,
        // not read from paths. Answering `mtls` here would send a caller
        // looking for files that, under `cas`, nobody places.
        Some(TLS_MODE_CAS) => Err(Error::invalid(format!(
            "{TLS_MODE_VARIABLE}=cas: this link's certificate is issued by Certificate \
             Authority Service, which this caller does not run. Decide the link with \
             `tls::link_source` and take the identity the composition root's renewal loop \
             holds (`cas::CasIdentity::identity`)"
        ))),
        Some(other) => Err(Error::invalid(format!(
            "{TLS_MODE_VARIABLE}={other:?} is not a link mode; use `cas`, `mtls` with \
             hand-placed files, or `plaintext` on a developer machine"
        ))),
    }
}

/// `QIP_TLS_MODE`'s value selecting Certificate Authority Service issuance.
pub const TLS_MODE_CAS: &str = "cas";

/// Where a mutual-TLS link's identity comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TlsSource {
    /// Hand-placed files (`QIP_TLS_MODE=mtls`): a workstation, or a test.
    Files(TlsPaths),
    /// Certificate Authority Service (`QIP_TLS_MODE=cas`, ADR 0112 §2): a
    /// short-lived certificate over a key generated in memory, renewed
    /// without a restart by the loop in [`crate::cas`].
    Issued(CasSettings),
}

impl TlsSource {
    /// One phrase for a start-up banner.
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::Files(_) => "mutual TLS 1.3 with a hand-placed workload certificate",
            Self::Issued(_) => {
                "mutual TLS 1.3 with a workload certificate Certificate Authority Service \
                 issues and renews (ADR 0112)"
            }
        }
    }
}

/// [`link_mode`] with the `cas` arm: `None` for plaintext, or where the
/// identity comes from. Pure and file-free, like [`link_mode`], so a root
/// validates its whole configuration before it opens anything.
///
/// Two settings that name one fact refuse rather than pick: `cas` with a
/// certificate or key path set, and a `QIP_CAS_*` setting under any mode
/// but `cas`. Either is a configuration that believes two different things
/// about where its certificate comes from, and whichever one won would be a
/// guess.
pub fn link_source(
    deployment: Deployment,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<TlsSource>> {
    let set = |name: &str| {
        lookup(name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let mode = set(TLS_MODE_VARIABLE);
    if mode.as_deref() == Some(TLS_MODE_CAS) {
        let placed: Vec<&str> = [TLS_CERTIFICATE_CHAIN_VARIABLE, TLS_PRIVATE_KEY_VARIABLE]
            .into_iter()
            .filter(|name| set(name).is_some())
            .collect();
        if !placed.is_empty() {
            return Err(Error::invalid(format!(
                "{TLS_MODE_VARIABLE}=cas and {} set; under `cas` Certificate Authority Service \
                 issues the certificate over a key that never leaves memory, so a hand-placed \
                 certificate or key is a second answer to the same question. Unset {}",
                placed.join(" and "),
                placed.join(" and ")
            )));
        }
        return Ok(Some(TlsSource::Issued(CasSettings::from_lookup(lookup)?)));
    }
    let stray: Vec<&str> = crate::cas::CAS_VARIABLES
        .iter()
        .copied()
        .filter(|name| set(name).is_some())
        .collect();
    if !stray.is_empty() {
        return Err(Error::invalid(format!(
            "{} set but {TLS_MODE_VARIABLE} is {}; a CA pool nothing requests from reads as \
             issuance. Set {TLS_MODE_VARIABLE}=cas, or unset {}",
            stray.join(", "),
            mode.as_deref().unwrap_or("unset"),
            stray.join(", ")
        )));
    }
    Ok(link_mode(
        deployment,
        mode.as_deref(),
        TlsPaths::from_lookup(lookup)?.as_ref(),
    )?
    .map(TlsSource::Files))
}

/// The variable naming a link's mode, and the three naming its files. The
/// composition roots read them; this crate documents them so its refusals
/// can name what to set.
pub const TLS_MODE_VARIABLE: &str = "QIP_TLS_MODE";
pub const TLS_CERTIFICATE_CHAIN_VARIABLE: &str = "QIP_TLS_CERTIFICATE_CHAIN_PATH";
pub const TLS_PRIVATE_KEY_VARIABLE: &str = "QIP_TLS_PRIVATE_KEY_PATH";
pub const TLS_TRUST_ROOTS_VARIABLE: &str = "QIP_TLS_TRUST_ROOTS_PATH";

impl TlsPaths {
    /// Read the three path variables through `lookup`: `None` when none is
    /// set, the paths when all three are, and a refusal naming the missing
    /// ones when only some are — a half-configured identity is a typo, not a
    /// choice, and treating it as "none" would hand the decision to
    /// [`LinkSecurity::configure`]'s refusal with the wrong message.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let read = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let chain = read(TLS_CERTIFICATE_CHAIN_VARIABLE);
        let key = read(TLS_PRIVATE_KEY_VARIABLE);
        let roots = read(TLS_TRUST_ROOTS_VARIABLE);
        match (chain, key, roots) {
            (None, None, None) => Ok(None),
            (Some(chain), Some(key), Some(roots)) => Ok(Some(Self {
                certificate_chain: PathBuf::from(chain),
                private_key: PathBuf::from(key),
                trust_roots: PathBuf::from(roots),
            })),
            (chain, key, roots) => {
                let missing: Vec<&str> = [
                    (chain.is_none(), TLS_CERTIFICATE_CHAIN_VARIABLE),
                    (key.is_none(), TLS_PRIVATE_KEY_VARIABLE),
                    (roots.is_none(), TLS_TRUST_ROOTS_VARIABLE),
                ]
                .into_iter()
                .filter_map(|(absent, name)| absent.then_some(name))
                .collect();
                Err(Error::invalid(format!(
                    "the workload certificate is half configured: {} not set",
                    missing.join(", ")
                )))
            }
        }
    }
}

impl LinkSecurity {
    /// [`Self::configure`] from the four variables, read through `lookup`.
    pub fn from_lookup(
        deployment: Deployment,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let mode = lookup(TLS_MODE_VARIABLE);
        // Under `cas` only the roots path is set, so reading the paths first
        // would refuse with "half configured" — true, and pointing at the
        // wrong fix. `link_mode` names the right one.
        if mode.as_deref().map(str::trim) == Some(TLS_MODE_CAS) {
            return Self::configure(deployment, mode.as_deref().map(str::trim), None);
        }
        let paths = TlsPaths::from_lookup(lookup)?;
        Self::configure(deployment, mode.as_deref(), paths.as_ref())
    }

    /// Mutual TLS with an identity a composition root already holds — the
    /// one Certificate Authority Service issued and the renewal loop keeps
    /// current. The `Arc` is shared, not copied, so every link built from it
    /// presents the newest certificate.
    pub const fn issued(identity: Arc<WorkloadIdentity>) -> Self {
        Self::MutualTls(identity)
    }

    /// Decide a link's security from the raw configuration a root read.
    ///
    /// * `mtls` — load the identity from `paths`; refuse to start if any
    ///   file is missing or malformed. Never falls back.
    /// * `plaintext` — admitted only in [`Deployment::Local`]; anywhere else
    ///   it stops the process, because ADR 0112 admits no plaintext link
    ///   between deployed workloads.
    /// * absent — `mtls` everywhere but [`Deployment::Local`], and refused in
    ///   `Local` too: a developer must say `plaintext` out loud, so that the
    ///   default of every configuration is the secure one and an omission
    ///   never produces a plaintext link.
    pub fn configure(
        deployment: Deployment,
        mode: Option<&str>,
        paths: Option<&TlsPaths>,
    ) -> Result<Self> {
        match link_mode(deployment, mode, paths)? {
            None => Ok(Self::Plaintext),
            Some(paths) => Ok(Self::MutualTls(Arc::new(WorkloadIdentity::from_files(
                &paths,
            )?))),
        }
    }

    /// Plaintext, for an in-process test that owns both ends of the socket.
    /// Named so a reviewer sees it; a composition root has no reason to call
    /// it and the acceptance suite can grep for one that does.
    pub const fn plaintext_for_tests() -> Self {
        Self::Plaintext
    }

    pub const fn is_mutual_tls(&self) -> bool {
        matches!(self, Self::MutualTls(_))
    }

    /// The scheme a URL to a peer over this link carries.
    pub const fn scheme(&self) -> &'static str {
        match self {
            Self::Plaintext => "http",
            Self::MutualTls(_) => "https",
        }
    }

    /// One line for a start-up banner.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Plaintext => {
                "PLAINTEXT — local deployment only; any host that can route a packet here can \
                 read and alter what is in flight"
            }
            Self::MutualTls(_) => "mutual TLS 1.3 with a workload certificate (ADR 0112)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutation: let `plaintext` through in any deployment (drop the
    /// `permits_plaintext_links` check) and this fails.
    #[test]
    fn plaintext_is_refused_in_every_deployed_environment_and_admitted_only_locally() {
        // Premise: the local case really is admitted, or the refusals below
        // would be a configure that refuses everything.
        assert!(matches!(
            LinkSecurity::configure(Deployment::Local, Some("plaintext"), None),
            Ok(LinkSecurity::Plaintext)
        ));
        for deployment in [
            Deployment::Dev,
            Deployment::Test,
            Deployment::Stage,
            Deployment::Prod,
        ] {
            assert!(
                LinkSecurity::configure(deployment, Some("plaintext"), None).is_err(),
                "plaintext was admitted in {deployment}"
            );
        }
    }

    /// Mutation: make an absent mode mean plaintext, or let `mtls` with no
    /// files return `Plaintext`, and this fails.
    #[test]
    fn an_unconfigured_link_never_comes_up_plaintext() {
        for deployment in Deployment::ALL {
            for mode in [None, Some("mtls"), Some("  ")] {
                assert!(
                    LinkSecurity::configure(deployment, mode, None).is_err(),
                    "{deployment} with mode {mode:?} and no certificate came up"
                );
            }
        }
        let missing = TlsPaths {
            certificate_chain: PathBuf::from("/nonexistent/chain.pem"),
            private_key: PathBuf::from("/nonexistent/key.pem"),
            trust_roots: PathBuf::from("/nonexistent/roots.pem"),
        };
        assert!(LinkSecurity::configure(Deployment::Local, Some("mtls"), Some(&missing)).is_err());
    }

    /// Mutation: treat a half-configured identity as unconfigured and this
    /// fails.
    #[test]
    fn a_half_configured_workload_certificate_is_refused_by_name() {
        let only_chain = |name: &str| {
            (name == TLS_CERTIFICATE_CHAIN_VARIABLE).then(|| "/etc/qip/chain.pem".to_string())
        };
        let error = TlsPaths::from_lookup(&only_chain).expect_err("half an identity was accepted");
        assert!(error.message().contains(TLS_PRIVATE_KEY_VARIABLE));
        assert!(error.message().contains(TLS_TRUST_ROOTS_VARIABLE));
        assert!(
            TlsPaths::from_lookup(&|_| None)
                .expect("none set")
                .is_none()
        );
    }

    fn variables<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    const CAS: [(&str, &str); 4] = [
        (TLS_MODE_VARIABLE, "cas"),
        (
            crate::cas::CA_POOL_VARIABLE,
            "projects/p/locations/europe-west2/caPools/qip-dev-internal",
        ),
        (TLS_TRUST_ROOTS_VARIABLE, "/etc/qip/pool-roots.pem"),
        (crate::cas::GOOGLE_TLS_ROOTS_VARIABLE, "/etc/qip/google.pem"),
    ];

    /// Premise first: complete `cas` settings are an issued source, in every
    /// deployment including `local`. Mutation: let a hand-placed key path
    /// through under `cas`, or ignore a stray pool under `mtls`, and a
    /// refusal below does not fire.
    #[test]
    fn cas_is_its_own_source_and_never_shares_the_question_with_files() {
        for deployment in Deployment::ALL {
            assert!(
                matches!(
                    link_source(deployment, &variables(&CAS)),
                    Ok(Some(TlsSource::Issued(_)))
                ),
                "cas was not an issued source in {deployment}"
            );
        }
        let mut with_key = CAS.to_vec();
        with_key.push((TLS_PRIVATE_KEY_VARIABLE, "/etc/qip/key.pem"));
        let error = link_source(Deployment::Dev, &variables(&with_key))
            .expect_err("cas with a hand-placed key was accepted");
        assert!(
            error.message().contains(TLS_PRIVATE_KEY_VARIABLE),
            "{error}"
        );

        let files = [
            (TLS_MODE_VARIABLE, "mtls"),
            (TLS_CERTIFICATE_CHAIN_VARIABLE, "/c"),
            (TLS_PRIVATE_KEY_VARIABLE, "/k"),
            (TLS_TRUST_ROOTS_VARIABLE, "/r"),
        ];
        assert!(matches!(
            link_source(Deployment::Dev, &variables(&files)),
            Ok(Some(TlsSource::Files(_)))
        ));
        let mut stray = files.to_vec();
        stray.push((crate::cas::LIFETIME_VARIABLE, "28800"));
        let error = link_source(Deployment::Dev, &variables(&stray))
            .expect_err("a CAS lifetime under mtls was accepted");
        assert!(
            error.message().contains(crate::cas::LIFETIME_VARIABLE),
            "{error}"
        );
        // Plaintext is still local-only through this path.
        let plain = [(TLS_MODE_VARIABLE, "plaintext")];
        assert!(matches!(
            link_source(Deployment::Local, &variables(&plain)),
            Ok(None)
        ));
        assert!(link_source(Deployment::Prod, &variables(&plain)).is_err());
    }

    /// A caller that cannot run issuance must say so, not ask for files.
    /// Mutation: drop the `cas` arm from `link_mode` (it falls to "not a
    /// link mode") or the early return in `from_lookup` (it says "half
    /// configured"), and the message check fails.
    #[test]
    fn a_caller_without_issuance_refuses_cas_by_naming_link_source() {
        let error = LinkSecurity::from_lookup(Deployment::Dev, &variables(&CAS))
            .expect_err("from_lookup brought up a cas link it cannot issue for");
        assert!(error.message().contains("link_source"), "{error}");
    }

    /// RFC 5280's two time forms and UTCTime's century pivot. Mutation: read
    /// `YY` 49 as 1949, or accept a time without seconds, and this fails.
    #[test]
    fn certificate_times_are_read_in_both_forms_with_the_rfc_5280_pivot() {
        assert_eq!(
            der_time(0x17, b"491231235959Z"),
            Some(Timestamp::from_civil(2050, 1, 1).saturating_sub(Duration::from_secs(1)))
        );
        assert_eq!(
            der_time(0x17, b"500101000000Z"),
            Some(Timestamp::from_civil(1950, 1, 1))
        );
        assert_eq!(
            der_time(0x18, b"20301231120000Z"),
            Some(Timestamp::from_civil(2030, 12, 31).saturating_add(Duration::from_hours(12)))
        );
        for bad in [
            &b"4912312359Z"[..],
            b"491231235959",
            b"491331235959Z",
            b"49123123595AZ",
        ] {
            assert_eq!(
                der_time(0x17, bad),
                None,
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
        assert_eq!(der_time(0x18, b"491231235959Z"), None);
    }

    /// Mutation: accept an empty PEM as an empty root store and this fails.
    #[test]
    fn empty_tls_material_is_refused_at_load_rather_than_at_the_first_handshake() {
        assert!(WorkloadIdentity::from_pem(b"", b"", b"").is_err());
        assert!(server_authenticated_client(b"").is_err());
    }
}
