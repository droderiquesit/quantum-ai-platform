//! The ledger's read API over mutual TLS (ADR 0112, R12-API-8113,
//! R12-GCP-8119): `qip_ledgerd::start` — the composition `main` runs — binds
//! the read API under the process's workload certificate whenever it has
//! one, and serves only a client presenting a certificate the same roots
//! chain.
//!
//! Until this, the read API was bound with `Server::bind` whatever the
//! configuration said: a deployed ledger whose fabric link was mutual TLS
//! served every balance and posting in plaintext to anything that could
//! route a packet to the port, while its start-up banner described an
//! encrypted link. The fabric half was proven; the half that hands the book
//! out was not.
//!
//! Every test here starts a real ledger on loopback with a fresh pool
//! minted for the run. Its fabric link points at a port nothing serves:
//! these tests are about who may read, not what is booked.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "../../../libs/qip-transport/tests/support/certs.rs"]
mod certs;
#[path = "support/pki.rs"]
mod pki;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::SystemClock;
use qip_core::error::Error;
use qip_ledgerd::config::LedgerConfig;
use qip_ledgerd::consumer::Connect;
use qip_ledgerd::{Running, start};
use qip_transport::event_fabric::protocol::{Request, Response};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::http::{ClientLimits, HttpClient, HttpRequest, Method};
use qip_transport::retry::Sleeper;

use pki::{Pki, READER_NAME, get};

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn scratch(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-ledgerd-tls-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A broker that is never there. The workers retry it; nothing here waits
/// on them.
#[derive(Debug)]
struct NoBroker;

impl FabricTransport for NoBroker {
    fn call(&mut self, _request: Request, _timeouts: Timeouts) -> qip_core::Result<Response> {
        Err(Error::unavailable("no broker in a read-API test"))
    }
}

/// Waits a millisecond whatever it is asked, so the workers' production
/// backoff does not hold up `Running::stop`.
#[derive(Debug)]
struct BriefSleeper;

impl Sleeper for BriefSleeper {
    fn sleep(&self, _duration: qip_core::Duration) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// A workstation ledger whose links are mutual TLS under `pki`, admitting
/// `peers` (comma-separated) when given.
fn start_ledger(dir: &std::path::Path, pki: &Pki, peers: Option<&str>) -> Running {
    let mut variables: BTreeMap<String, String> = [
        (qip_ledgerd::config::ENVIRONMENT, "workstation".to_string()),
        (
            qip_ledgerd::config::DATA_DIR,
            dir.join("store").display().to_string(),
        ),
        (qip_ledgerd::config::LISTEN, "127.0.0.1:0".to_string()),
        (qip_ledgerd::config::CONSUMER_GROUP, "ledger".to_string()),
        (
            qip_ledgerd::config::FABRIC_URL,
            "https://127.0.0.1:9".to_string(),
        ),
        (
            qip_ledgerd::config::FABRIC_STREAM,
            "outcomes.test".to_string(),
        ),
        (qip_ledgerd::config::FABRIC_PARTITIONS, "1".to_string()),
        (qip_ledgerd::config::POLL_MS, "20".to_string()),
        // A workstation, so the local store is admitted; the link is mutual
        // TLS because the variables below say so, not because the
        // deployment forces it — the same path a deployment takes.
        (qip_ledgerd::config::DEPLOYMENT, "local".to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    variables.extend(pki.variables());
    if let Some(peers) = peers {
        variables.insert(
            qip_ledgerd::config::ADMITTED_PEERS.to_string(),
            peers.to_string(),
        );
    }
    let config = LedgerConfig::parse(&variables).expect("a mutual-TLS configuration loads");
    let connect: Connect = Arc::new(|| {
        let transport: Box<dyn FabricTransport + Send> = Box::new(NoBroker);
        Ok(transport)
    });
    start(
        &config,
        Arc::new(SystemClock),
        Arc::new(BriefSleeper),
        connect,
    )
    .expect("the ledger starts with a workload certificate")
}

/// A client holding a certificate the ledger's roots chain is served the
/// book; a client presenting no certificate, and one speaking plaintext to
/// the same port, are served nothing.
///
/// Mutation: in `qip_ledgerd::start_with_access`, bind the read API with
/// `Server::bind` in the `Some(paths)` arm too — the premise fails (the
/// certified client's handshake finds no TLS server) and the plaintext
/// client is answered 200.
#[test]
fn a_ledger_with_a_workload_certificate_serves_its_read_api_only_to_a_client_presenting_one() {
    let dir = scratch("served");
    let pki = Pki::mint(&dir.join("tls"));
    let running = start_ledger(&dir, &pki, None);

    // Premise: the read API is up and answers a certified reader, so the
    // refusals below are the handshake's doing and not a dead listener.
    let served = get(
        &pki.client(&pki.reader),
        running.address(),
        "/api/v1/ledger/trial-balance",
    )
    .expect("a reader presenting a pool certificate is served");
    assert_eq!(served.status, 200, "{}", served.body_excerpt());
    let health = get(&pki.client(&pki.reader), running.address(), "/health")
        .expect("health answers a certified reader");
    assert_eq!(health.status, 200, "{}", health.body_excerpt());

    let anonymous = get(
        &pki.anonymous(),
        running.address(),
        "/api/v1/ledger/trial-balance",
    );
    assert!(
        anonymous.is_err(),
        "a client presenting no certificate was answered: {anonymous:?}"
    );

    let plaintext = HttpClient::new(ClientLimits {
        read_timeout: std::time::Duration::from_secs(5),
        ..ClientLimits::default()
    })
    .send(
        &HttpRequest::new(
            Method::Get,
            &format!("http://{}/api/v1/ledger/trial-balance", running.address()),
        )
        .expect("a url"),
    );
    assert!(
        plaintext
            .as_ref()
            .map_or(true, |response| response.status != 200),
        "the read API answered plaintext: {plaintext:?}"
    );

    running.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// With `QIP_LEDGER_ADMITTED_PEERS` set, a certificate the roots chain is
/// not enough: it must name a listed workload. A reader the pool certified
/// but the list does not name is refused; the same reader, listed, is
/// served.
///
/// Mutation: in `qip_ledgerd::start_with_access`, build the `TlsServer`
/// with `PeerNames::default()` instead of the configured names — the
/// unlisted reader is served and the first assertion fails.
#[test]
fn a_ledger_naming_its_readers_serves_only_the_workloads_it_names() {
    let dir = scratch("named");
    let pki = Pki::mint(&dir.join("tls"));

    let running = start_ledger(&dir, &pki, Some("someone-else.test.qip.internal"));
    let unlisted = get(
        &pki.client(&pki.reader),
        running.address(),
        "/api/v1/ledger/trial-balance",
    );
    assert!(
        unlisted
            .as_ref()
            .map_or(true, |response| response.status == 403),
        "a reader the list does not name was served: {unlisted:?}"
    );
    running.stop();

    let listed = start_ledger(
        &scratch("named-listed"),
        &pki,
        Some(&format!("someone-else.test.qip.internal,{READER_NAME}")),
    );
    let served = get(
        &pki.client(&pki.reader),
        listed.address(),
        "/api/v1/ledger/trial-balance",
    )
    .expect("a listed reader is served");
    assert_eq!(served.status, 200, "{}", served.body_excerpt());
    listed.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
