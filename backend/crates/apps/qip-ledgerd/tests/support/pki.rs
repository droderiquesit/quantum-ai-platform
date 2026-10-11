//! A throwaway PKI on disk for the ledger's mutual-TLS tests (ADR 0112).
//!
//! The certificates come from `qip-transport`'s test minter — fresh P-256
//! keys per run, because committing PEM private keys as fixtures would put
//! key material in the repository. This file only writes the ledger's own
//! certificate where `QIP_TLS_*` can name it, and builds the clients that
//! read the ledger: one presenting a certificate, one presenting none.
//!
//! Included with `#[path]` by each test binary that needs it, beside the
//! minter itself (`mod certs;` in the including file).

#![allow(dead_code)]

use crate::certs::{self, Minted};
use qip_transport::TlsHttpClient;
use qip_transport::http::{ClientLimits, HttpError, HttpRequest, HttpResponse, Method};
use qip_transport::tls::{
    TLS_CERTIFICATE_CHAIN_VARIABLE, TLS_MODE_VARIABLE, TLS_PRIVATE_KEY_VARIABLE,
    TLS_TRUST_ROOTS_VARIABLE, WorkloadIdentity, server_authenticated_client,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The name the ledger's own certificate carries, and the host a client
/// dials it by, so the client verifies the server it meant.
pub(crate) const LEDGER_NAME: &str = "localhost";

/// The workload a reader's certificate names.
pub(crate) const READER_NAME: &str = "reader.test.qip.internal";

/// One pool: its CA, the ledger's certificate and a reader's.
pub(crate) struct Pki {
    pub(crate) ca: Minted,
    pub(crate) ledger: Minted,
    pub(crate) reader: Minted,
    chain: PathBuf,
    key: PathBuf,
    roots: PathBuf,
}

impl Pki {
    /// Mint a pool and write the ledger's chain, key and roots under `dir`.
    pub(crate) fn mint(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).expect("a directory for the TLS files");
        let ca = certs::authority("qip ledger test pool");
        let ledger = certs::leaf(&ca, 2, LEDGER_NAME);
        let reader = certs::leaf(&ca, 3, READER_NAME);
        let chain = dir.join("ledger-chain.pem");
        let key = dir.join("ledger-key.pem");
        let roots = dir.join("roots.pem");
        std::fs::write(&chain, ledger.certificate_pem()).expect("the chain file");
        std::fs::write(&key, ledger.key_pem()).expect("the key file");
        std::fs::write(&roots, ca.certificate_pem()).expect("the roots file");
        Self {
            ca,
            ledger,
            reader,
            chain,
            key,
            roots,
        }
    }

    /// The four variables that make the ledger's links mutual TLS under
    /// this pool's certificate.
    pub(crate) fn variables(&self) -> Vec<(String, String)> {
        vec![
            (TLS_MODE_VARIABLE.to_string(), "mtls".to_string()),
            (
                TLS_CERTIFICATE_CHAIN_VARIABLE.to_string(),
                self.chain.display().to_string(),
            ),
            (
                TLS_PRIVATE_KEY_VARIABLE.to_string(),
                self.key.display().to_string(),
            ),
            (
                TLS_TRUST_ROOTS_VARIABLE.to_string(),
                self.roots.display().to_string(),
            ),
        ]
    }

    /// A client presenting `leaf`, which trusts this pool's CA.
    pub(crate) fn client(&self, leaf: &Minted) -> TlsHttpClient {
        let identity = WorkloadIdentity::from_pem(
            leaf.certificate_pem().as_bytes(),
            leaf.key_pem().as_bytes(),
            self.ca.certificate_pem().as_bytes(),
        )
        .expect("minted material loads");
        TlsHttpClient::new(limits(), identity.client_config().expect("a client config"))
    }

    /// A client that verifies the ledger against this pool and presents
    /// nothing of its own.
    pub(crate) fn anonymous(&self) -> TlsHttpClient {
        TlsHttpClient::new(
            limits(),
            server_authenticated_client(self.ca.certificate_pem().as_bytes())
                .expect("a server-authenticated config"),
        )
    }
}

/// Short bounds, so a refused handshake fails the test quickly rather than
/// at the default timeouts.
pub(crate) fn limits() -> ClientLimits {
    ClientLimits {
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
        ..ClientLimits::default()
    }
}

/// `GET path` on the ledger bound at `address` (`127.0.0.1:port`), dialled
/// by [`LEDGER_NAME`] so the server's certificate is checked against it.
pub(crate) fn get(
    client: &TlsHttpClient,
    address: &str,
    path: &str,
) -> Result<HttpResponse, HttpError> {
    let port = address.rsplit_once(':').expect("host:port").1;
    client.send(&HttpRequest::new_tls(
        Method::Get,
        &format!("https://{LEDGER_NAME}:{port}{path}"),
    )?)
}
