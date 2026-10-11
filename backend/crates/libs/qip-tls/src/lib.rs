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
//!   and the CA roots its peers chain to, **loaded from files** the
//!   composition root names. In a deployment Certificate Authority Service
//!   issues the certificate and something on the machine keeps the files
//!   fresh; this crate reads, it never mints, and it never writes a key.
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

use qip_core::deployment::Deployment;
use qip_core::error::{Error, Result};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

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
use std::sync::Arc;
pub use webpki;

/// The provider every config here is built on: `ring`, so the platform's TLS
/// and its artefact verification share one implementation of the primitives
/// (ADR 0110 §3).
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
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

/// This process's certificate, its key, and the roots it trusts.
pub struct WorkloadIdentity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    roots: Arc<RootCertStore>,
}

// The key is deliberately absent from `Debug`: a struct that prints its own
// secret gets it into a log the first time anything derives `Debug` on a
// type that contains it.
impl std::fmt::Debug for WorkloadIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadIdentity")
            .field("chain_length", &self.chain.len())
            .field("roots", &self.roots.len())
            .field("key", &"<redacted>")
            .finish()
    }
}

impl WorkloadIdentity {
    /// Parse PEM material, refusing an empty chain, a missing key or an
    /// empty root set — each would build a config that fails every
    /// handshake, which is better found at start-up than on the first call.
    pub fn from_pem(chain_pem: &[u8], key_pem: &[u8], roots_pem: &[u8]) -> Result<Self> {
        let chain = CertificateDer::pem_slice_iter(chain_pem)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| tls_error("the workload certificate chain does not parse", error))?;
        if chain.is_empty() {
            return Err(Error::invalid(
                "the workload certificate chain holds no certificate; a TLS link cannot present \
                 an identity it does not have",
            ));
        }
        let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|error| {
            // The error from the PEM reader names a section kind, never the
            // key's bytes, so it is safe to carry.
            tls_error("the workload private key does not parse", error)
        })?;
        let roots = roots_from_pem(roots_pem)?;
        Ok(Self {
            chain,
            key,
            roots: Arc::new(roots),
        })
    }

    /// Read the three files and parse them.
    pub fn from_files(paths: &TlsPaths) -> Result<Self> {
        Self::from_pem(
            &read(&paths.certificate_chain, "workload certificate chain")?,
            &read(&paths.private_key, "workload private key")?,
            &read(&paths.trust_roots, "TLS trust roots")?,
        )
    }

    /// A server config that **requires** a client certificate chained to
    /// this identity's roots, over TLS 1.3 only.
    pub fn server_config(&self) -> Result<Arc<ServerConfig>> {
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::clone(&self.roots),
            provider(),
        )
        .build()
        .map_err(|error| tls_error("cannot build the client-certificate verifier", error))?;
        let config = ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| tls_error("cannot select TLS 1.3", error))?
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.chain.clone(), self.key.clone_key())
            .map_err(|error| tls_error("the workload certificate and key do not match", error))?;
        Ok(Arc::new(config))
    }

    /// A client config that presents this identity and verifies the server
    /// against the same roots, over TLS 1.3 only.
    pub fn client_config(&self) -> Result<Arc<ClientConfig>> {
        let config = ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| tls_error("cannot select TLS 1.3", error))?
            .with_root_certificates(Arc::clone(&self.roots))
            .with_client_auth_cert(self.chain.clone(), self.key.clone_key())
            .map_err(|error| tls_error("the workload certificate and key do not match", error))?;
        Ok(Arc::new(config))
    }
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

fn read(path: &Path, what: &str) -> Result<Vec<u8>> {
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
        Some(other) => Err(Error::invalid(format!(
            "{TLS_MODE_VARIABLE}={other:?} is not a link mode; use `mtls`, or `plaintext` on a \
             developer machine"
        ))),
    }
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
        let paths = TlsPaths::from_lookup(lookup)?;
        Self::configure(
            deployment,
            lookup(TLS_MODE_VARIABLE).as_deref(),
            paths.as_ref(),
        )
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

    /// Mutation: accept an empty PEM as an empty root store and this fails.
    #[test]
    fn empty_tls_material_is_refused_at_load_rather_than_at_the_first_handshake() {
        assert!(WorkloadIdentity::from_pem(b"", b"", b"").is_err());
        assert!(server_authenticated_client(b"").is_err());
    }
}
