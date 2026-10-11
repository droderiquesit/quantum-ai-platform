//! Workload certificates from Certificate Authority Service (ADR 0112 §2),
//! over real loopback sockets.
//!
//! Three properties, each the one whose absence would be invisible until it
//! mattered:
//!
//! * **What the CA answers is checked before it is presented** — a
//!   certificate over another key, an expired one, one chained to another
//!   authority, or one for another request is refused and the identity keeps
//!   presenting nothing (or what it had).
//! * **A renewal reaches a server built once.** The certificate a listener
//!   presents to a new connection is the newest installed, without rebuilding
//!   the server's config or restarting anything.
//! * **An expired identity makes no handshake.** In either direction, a
//!   process whose certificate has lapsed refuses rather than presenting it.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "support/certs.rs"]
mod certs;

use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration as Span, ManualClock, SystemClock, Timestamp};
use qip_transport::ClientLimits;
use qip_transport::TlsHttpClient;
use qip_transport::cas::{
    CasClient, CasIdentity, CasSettings, CertificateIssuer, Renewer, WorkloadKey, WorkloadSubject,
    os_entropy,
};
use qip_transport::kms::{BearerSource, decode_base64};
use qip_transport::server::{
    Handler, Request as ServerRequest, Response as ServerResponse, Server, ServerLimits,
};
use qip_transport::tls::{self, PeerNames, TlsServer, WorkloadIdentity};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const POOL: &str = "projects/proj-1/locations/europe-west2/caPools/qip-test-internal";
const SERVICE_ACCOUNT: &str = "qip-test-fabricd@proj-1.iam.gserviceaccount.com";

#[derive(Debug)]
struct Token;

impl BearerSource for Token {
    fn bearer(&self) -> Result<String> {
        Ok("test-token".to_string())
    }
}

/// The 65-byte point inside a P-256 SPKI.
fn point_of(spki: &[u8]) -> Vec<u8> {
    assert_eq!(spki.len(), 91, "premise: a P-256 SPKI");
    spki[26..].to_vec()
}

/// A window around `now`.
fn window(now: Timestamp, before: Span, after: Span) -> (String, String) {
    (
        certs::utc_time(now.saturating_sub(before)),
        certs::utc_time(now.saturating_add(after)),
    )
}

// ---------------------------------------------------------------------------
// A stand-in for the CA Service REST endpoint.
// ---------------------------------------------------------------------------

/// What the stand-in received.
struct Captured {
    request_line: String,
    headers: String,
    body: serde_json::Value,
}

/// How the stand-in answers.
#[derive(Clone, Copy)]
enum Answer {
    /// A leaf over the key that was sent, from the pool's CA, valid now.
    Faithful,
    /// A leaf over a key the stand-in made up.
    OtherKey,
    /// A leaf over the sent key that expired last year.
    Expired,
    /// A leaf over the sent key from an authority outside the pool.
    ForeignAuthority,
    /// A faithful leaf, under another request's name.
    OtherName,
}

struct Pool {
    ca: certs::Minted,
    stranger: certs::Minted,
    google_ca: certs::Minted,
    google_leaf: certs::Minted,
}

fn pool() -> Pool {
    let google_ca = certs::authority("stand-in google roots");
    let google_leaf = certs::leaf(&google_ca, 2, "localhost");
    Pool {
        ca: certs::authority("qip test pool"),
        stranger: certs::authority("someone else's pool"),
        google_ca,
        google_leaf,
    }
}

fn read_request(stream: &mut impl Read) -> Option<(String, String, Vec<u8>)> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    let split = loop {
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(n) => raw.extend_from_slice(&buffer[..n]),
        }
    };
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = raw[split + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(n) => body.extend_from_slice(&buffer[..n]),
        }
    }
    let (request_line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    Some((request_line.to_string(), headers.to_lowercase(), body))
}

/// Serve one `certificates.create` over server-authenticated TLS, answering
/// as `answer` says, and hand back what was asked.
fn fake_cas(pool: &Pool, answer: Answer) -> (u16, mpsc::Receiver<Captured>) {
    let chain = vec![CertificateDer::from(
        pool.google_leaf.certificate_der.clone(),
    )];
    let key = PrivateKeyDer::from_pem_slice(pool.google_leaf.key_pem().as_bytes()).expect("key");
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("tls 1.3")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("server config"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let ca_pem = pool.ca.certificate_pem();
    let stranger_pem = pool.stranger.certificate_pem();
    // The minting halves cannot cross the thread boundary by reference.
    let issuer = Arc::new(certs::authority_from(&pool.ca));
    let stranger = Arc::new(certs::authority_from(&pool.stranger));
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut session = rustls::ServerConnection::new(config).expect("session");
        while session.is_handshaking() {
            if session.complete_io(&mut socket).is_err() {
                return;
            }
        }
        let mut stream = rustls::StreamOwned::new(session, socket);
        let Some((request_line, headers, body)) = read_request(&mut stream) else {
            return;
        };
        let body: serde_json::Value = serde_json::from_slice(&body).expect("a JSON body");
        let sent = decode_base64(body["config"]["publicKey"]["key"].as_str().expect("a key"))
            .expect("base64");
        let spki = public_key_der(&sent);
        let now = SystemClock.now();
        let (from, to) = window(now, Span::from_hours(1), Span::from_hours(8));
        let (point, authority, chain_pem, validity) = match answer {
            Answer::Faithful | Answer::OtherName => {
                (point_of(&spki), &issuer, ca_pem.clone(), (from, to))
            }
            Answer::OtherKey => {
                let other = WorkloadKey::generate(&ring::rand::SystemRandom::new()).expect("key");
                (
                    point_of(other.spki_der()),
                    &issuer,
                    ca_pem.clone(),
                    (from, to),
                )
            }
            Answer::Expired => (
                point_of(&spki),
                &issuer,
                ca_pem.clone(),
                window(now, Span::from_days(400), Span::from_days(-365)),
            ),
            Answer::ForeignAuthority => {
                (point_of(&spki), &stranger, stranger_pem.clone(), (from, to))
            }
        };
        let leaf = certs::leaf_over(
            authority,
            7,
            "qip-test-fabricd.proj-1.iam.gserviceaccount.com",
            &point,
            (&validity.0, &validity.1),
        );
        let id = request_line
            .split("certificateId=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("none")
            .to_string();
        let name = match answer {
            Answer::OtherName => format!("{POOL}/certificates/somebody-else"),
            _ => format!("{POOL}/certificates/{id}"),
        };
        let reply = serde_json::json!({
            "name": name,
            "pemCertificate": certs::pem_of("CERTIFICATE", &leaf),
            "pemCertificateChain": [chain_pem],
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        let _ = sender.send(Captured {
            request_line,
            headers,
            body,
        });
    });
    (port, receiver)
}

/// The DER inside a PEM `PUBLIC KEY` block.
fn public_key_der(pem: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(pem).expect("PEM is text");
    assert!(text.starts_with("-----BEGIN PUBLIC KEY-----\n"), "{text}");
    let body: String = text
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    decode_base64(&body).expect("base64 body")
}

fn settings(pool: &Pool, directory: &std::path::Path) -> CasSettings {
    let roots = directory.join("pool-roots.pem");
    let google = directory.join("google-roots.pem");
    std::fs::write(&roots, pool.ca.certificate_pem()).expect("write roots");
    std::fs::write(&google, pool.google_ca.certificate_pem()).expect("write google roots");
    let pairs = [
        ("QIP_CAS_CA_POOL", POOL.to_string()),
        ("QIP_TLS_TRUST_ROOTS_PATH", roots.display().to_string()),
        ("QIP_GOOGLE_TLS_ROOTS_PATH", google.display().to_string()),
    ];
    CasSettings::from_lookup(&|name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    })
    .expect("complete settings")
}

fn scratch(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "qip-cas-{name}-{}-{}",
        std::process::id(),
        SystemClock.now().as_nanos()
    ));
    std::fs::create_dir_all(&directory).expect("scratch directory");
    directory
}

/// A renewer whose issuer is the stand-in at `port`.
fn renewer_against(pool: &Pool, port: u16, clock: Arc<dyn Clock>) -> Renewer {
    let directory = scratch("client");
    let settings = settings(pool, &directory);
    let identity = Arc::new(
        WorkloadIdentity::awaiting_issuance(
            pool.ca.certificate_pem().as_bytes(),
            Arc::clone(&clock),
        )
        .expect("roots load"),
    );
    let client = CasClient::new(
        &settings,
        WorkloadSubject::from_service_account(SERVICE_ACCOUNT).expect("subject"),
        TlsHttpClient::new(
            CasClient::limits(),
            tls::server_authenticated_client(pool.google_ca.certificate_pem().as_bytes())
                .expect("google roots"),
        ),
        Arc::new(Token),
    )
    .with_endpoint(&format!("https://localhost:{port}/v1"))
    .expect("https");
    Renewer::new(
        Arc::new(client),
        identity,
        os_entropy(),
        clock,
        "qip-test-fabricd",
    )
    .expect("renewer")
}

/// The admitting half, and the request's shape on the wire. Mutation: send
/// the bearer under another header, drop the `certificateId`, or post to
/// the pool without `/certificates`, and an assertion here fails; make
/// `install` skip the slot update and `is_current` stays false.
#[test]
fn a_faithful_answer_is_installed_and_the_request_carried_the_key_and_the_token() {
    let pool = pool();
    let (port, captured) = fake_cas(&pool, Answer::Faithful);
    let renewer = renewer_against(&pool, port, Arc::new(SystemClock));
    let identity = renewer.identity();
    assert!(!identity.is_current(), "premise: nothing before issuance");
    let validity = renewer.renew_now().expect("a faithful answer installs");
    assert!(identity.is_current());
    assert_eq!(identity.validity(), Some(validity));
    let request = captured
        .recv_timeout(Duration::from_secs(5))
        .expect("the stand-in was asked");
    let path = request
        .request_line
        .strip_prefix("POST ")
        .expect("a POST")
        .split_whitespace()
        .next()
        .expect("a target");
    let id = path
        .strip_prefix(&format!("/v1/{POOL}/certificates?certificateId="))
        .unwrap_or_else(|| panic!("{path} is not certificates.create on the pool"));
    assert!(id.starts_with("qip-test-fabricd-"), "{id}");
    assert!(
        id.len() <= 63 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "{id} is not a legal certificate id"
    );
    assert!(
        request
            .headers
            .lines()
            .any(|line| line == "authorization: bearer test-token"),
        "{}",
        request.headers
    );
    assert_eq!(request.body["lifetime"], "28800s");
    assert_eq!(request.body["config"]["publicKey"]["format"], "PEM");
}

/// Each refusal leaves the identity presenting nothing. Mutation: delete
/// the SPKI comparison in `install` and `OtherKey` installs; delete the
/// webpki chain verification and `ForeignAuthority` and `Expired` install;
/// delete the name check in `CasClient::request` and `OtherName` installs.
#[test]
fn an_answer_for_another_key_an_expired_one_a_foreign_chain_or_another_request_is_refused() {
    let pool = pool();
    for (answer, phrase) in [
        (Answer::OtherKey, "different public key"),
        (Answer::ForeignAuthority, "does not verify, now"),
        (Answer::Expired, "does not verify, now"),
        (Answer::OtherName, "answered for"),
    ] {
        let (port, captured) = fake_cas(&pool, answer);
        let renewer = renewer_against(&pool, port, Arc::new(SystemClock));
        let error = renewer
            .renew_now()
            .expect_err(&format!("the answer containing {phrase:?} was installed"));
        assert!(
            error.message().contains(phrase),
            "refused for the wrong reason: {error}"
        );
        assert!(!renewer.identity().is_current());
        // Premise: the stand-in really was reached, so the refusal is about
        // its answer and not a connection that never happened.
        captured
            .recv_timeout(Duration::from_secs(5))
            .expect("the stand-in was asked");
    }
}

// ---------------------------------------------------------------------------
// A local issuer, for the properties that are about the identity, not HTTP.
// ---------------------------------------------------------------------------

/// Mints leaves over whatever key it is given, as the pool's CA would,
/// for `lifetime` from the clock's now; or refuses while `failing`.
struct LocalIssuer {
    ca: certs::Minted,
    dns_name: String,
    clock: Arc<dyn Clock>,
    lifetime: Span,
    failing: AtomicBool,
    minted: Mutex<Vec<Vec<u8>>>,
}

impl LocalIssuer {
    fn new(ca: &certs::Minted, dns_name: &str, clock: Arc<dyn Clock>, lifetime: Span) -> Self {
        Self {
            ca: certs::authority_from(ca),
            dns_name: dns_name.to_string(),
            clock,
            lifetime,
            failing: AtomicBool::new(false),
            minted: Mutex::new(Vec::new()),
        }
    }

    fn last(&self) -> Vec<u8> {
        self.minted
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("minted one")
    }

    fn count(&self) -> usize {
        self.minted.lock().unwrap().len()
    }
}

impl std::fmt::Debug for LocalIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalIssuer")
    }
}

impl CertificateIssuer for LocalIssuer {
    fn issue(&self, key: &WorkloadKey, _id: &str) -> Result<Vec<CertificateDer<'static>>> {
        if self.failing.load(Ordering::SeqCst) {
            return Err(Error::unavailable("the stand-in CA is down"));
        }
        let now = self.clock.now();
        let leaf = certs::leaf_over(
            &self.ca,
            u8::try_from(self.count() % 200 + 10).unwrap(),
            &self.dns_name,
            &point_of(key.spki_der()),
            (
                &certs::utc_time(now),
                &certs::utc_time(now.saturating_add(self.lifetime)),
            ),
        );
        self.minted.lock().unwrap().push(leaf.clone());
        Ok(vec![
            CertificateDer::from(leaf),
            CertificateDer::from(self.ca.certificate_der.clone()),
        ])
    }
}

struct Counting(Arc<AtomicUsize>);

impl Handler for Counting {
    fn handle(&self, _request: &ServerRequest) -> ServerResponse {
        self.0.fetch_add(1, Ordering::SeqCst);
        ServerResponse::text(200, "ok".to_string())
    }
}

/// A server bound once, answering `connections` connections.
fn serve(
    identity: &WorkloadIdentity,
    connections: usize,
) -> (u16, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = Server::bind_tls(
        "127.0.0.1:0",
        Arc::new(Counting(Arc::clone(&calls))),
        ServerLimits::default(),
        TlsServer {
            config: identity.server_config().expect("server config"),
            peers: PeerNames::default(),
        },
    )
    .expect("binds");
    let port: u16 = server
        .local_address()
        .expect("address")
        .rsplit_once(':')
        .expect("host:port")
        .1
        .parse()
        .expect("a port");
    let thread = std::thread::spawn(move || {
        for _ in 0..connections {
            let _ = server.serve_once();
        }
    });
    (port, calls, thread)
}

/// One request over a fresh connection, returning the certificate the
/// server presented, or the handshake's refusal.
fn connect(port: u16, client: &Arc<rustls::ClientConfig>) -> std::result::Result<Vec<u8>, String> {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let mut session = rustls::ClientConnection::new(
        Arc::clone(client),
        ServerName::try_from("localhost").expect("a name"),
    )
    .map_err(|e| e.to_string())?;
    while session.is_handshaking() {
        session
            .complete_io(&mut socket)
            .map_err(|e| e.to_string())?;
    }
    let presented = session
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|leaf| leaf.as_ref().to_vec())
        .ok_or("no certificate presented")?;
    let mut stream = rustls::StreamOwned::new(session, socket);
    stream
        .write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    if !response.starts_with(b"HTTP/1.1 200") {
        return Err(String::from_utf8_lossy(&response).to_string());
    }
    Ok(presented)
}

fn client_of(pool: &Pool) -> Arc<rustls::ClientConfig> {
    let client = certs::leaf(&pool.ca, 3, "edge-node.test.qip.internal");
    WorkloadIdentity::from_pem(
        client.certificate_pem().as_bytes(),
        client.key_pem().as_bytes(),
        pool.ca.certificate_pem().as_bytes(),
    )
    .expect("client identity")
    .client_config()
    .expect("client config")
}

/// The rotation property. Mutation: build `server_config` with
/// `with_single_cert` over the certificate held at build time (the shape
/// this module had before) and the third connection is presented the
/// first certificate; this fails.
#[test]
fn a_server_built_once_presents_the_renewed_certificate_to_the_next_connection() {
    let pool = pool();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let identity = Arc::new(
        WorkloadIdentity::awaiting_issuance(
            pool.ca.certificate_pem().as_bytes(),
            Arc::clone(&clock),
        )
        .expect("roots"),
    );
    let issuer = Arc::new(LocalIssuer::new(
        &pool.ca,
        "localhost",
        Arc::clone(&clock),
        Span::from_hours(8),
    ));
    let renewer = Renewer::new(
        issuer.clone(),
        Arc::clone(&identity),
        os_entropy(),
        clock,
        "qip-test-fabricd",
    )
    .expect("renewer");
    let (port, calls, thread) = serve(&identity, 3);
    let client = client_of(&pool);

    // Before the first issuance the listener is bound and refuses.
    assert!(
        connect(port, &client).is_err(),
        "served with no certificate"
    );

    renewer.renew_now().expect("first issuance");
    let first = issuer.last();
    assert_eq!(connect(port, &client).expect("served"), first);

    renewer.renew_now().expect("renewal");
    let second = issuer.last();
    assert_ne!(
        first, second,
        "premise: the renewal is a different certificate"
    );
    assert_eq!(
        connect(port, &client).expect("served after renewal"),
        second,
        "the server built before the renewal presented the old certificate"
    );
    thread.join().expect("server thread");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// The expiry property, as a server and as a client. Mutation: drop the
/// window check from the slot's `current` (present whatever was installed)
/// and the expired server completes its handshake — the stand-in client
/// verifies against real time, inside the certificate's window — so this
/// fails.
#[test]
fn an_expired_identity_refuses_new_handshakes_as_a_server_and_as_a_client() {
    let pool = pool();
    let start = SystemClock.now();
    let manual = Arc::new(ManualClock::new(start));
    let clock: Arc<dyn Clock> = manual.clone();
    // Valid in real time for days, so any refusal below is this process's
    // own clock saying the certificate has lapsed, not a peer's.
    let issuer = Arc::new(LocalIssuer::new(
        &pool.ca,
        "localhost",
        Arc::clone(&clock),
        Span::from_days(3),
    ));

    // As a server.
    let server_identity = Arc::new(
        WorkloadIdentity::awaiting_issuance(
            pool.ca.certificate_pem().as_bytes(),
            Arc::clone(&clock),
        )
        .expect("roots"),
    );
    Renewer::new(
        issuer.clone(),
        Arc::clone(&server_identity),
        os_entropy(),
        Arc::clone(&clock),
        "qip-test-fabricd",
    )
    .expect("renewer")
    .renew_now()
    .expect("issued");
    let (port, calls, thread) = serve(&server_identity, 2);
    let client = client_of(&pool);
    connect(port, &client).expect("premise: served while current");
    manual.advance(Span::from_days(3));
    assert!(!server_identity.is_current());
    assert!(
        connect(port, &client).is_err(),
        "an expired server identity completed a handshake"
    );
    thread.join().expect("server thread");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // As a client, against a server whose certificate does not lapse. A
    // clock of its own, starting at real time: the server judges the
    // client's certificate by real time, so the certificate must be inside
    // its window by that clock for the refusal to be this process's own.
    let client_manual = Arc::new(ManualClock::new(SystemClock.now()));
    let client_clock: Arc<dyn Clock> = client_manual.clone();
    let client_identity = Arc::new(
        WorkloadIdentity::awaiting_issuance(
            pool.ca.certificate_pem().as_bytes(),
            Arc::clone(&client_clock),
        )
        .expect("roots"),
    );
    let issued_at = client_manual.now();
    Renewer::new(
        Arc::new(LocalIssuer::new(
            &pool.ca,
            "edge-node.test.qip.internal",
            Arc::clone(&client_clock),
            Span::from_hours(1),
        )),
        Arc::clone(&client_identity),
        os_entropy(),
        Arc::clone(&client_clock),
        "qip-test-edge",
    )
    .expect("renewer")
    .renew_now()
    .expect("issued");
    let server_leaf = certs::leaf(&pool.ca, 4, "localhost");
    let server = WorkloadIdentity::from_pem(
        server_leaf.certificate_pem().as_bytes(),
        server_leaf.key_pem().as_bytes(),
        pool.ca.certificate_pem().as_bytes(),
    )
    .expect("server identity");
    let (port, calls, thread) = serve(&server, 2);
    // One config, built while current and kept: the refusal below must come
    // from the identity lapsing under it, not from building a new config.
    let config = client_identity.client_config().expect("config");
    connect(port, &config).expect("premise: a current client is served");
    client_manual.set(issued_at.saturating_add(Span::from_hours(2)));
    assert!(!client_identity.is_current());
    let outcome = connect(port, &config);
    thread.join().expect("server thread");
    assert!(outcome.is_err(), "an expired client identity was served");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the handler ran for a client whose certificate had lapsed"
    );
}

/// The schedule, on a manual clock. Mutation: renew at the not-after
/// instead of two-thirds and the second tick does not issue; reset the
/// backoff on failure and the third wait is not doubled; clear the slot on
/// a failed renewal and the old certificate stops serving early.
#[test]
fn renewal_comes_at_two_thirds_and_a_failure_keeps_the_old_certificate_until_it_lapses() {
    let pool = pool();
    let manual = Arc::new(ManualClock::new(Timestamp::from_civil(2030, 1, 1)));
    let clock: Arc<dyn Clock> = manual.clone();
    let identity = Arc::new(
        WorkloadIdentity::awaiting_issuance(
            pool.ca.certificate_pem().as_bytes(),
            Arc::clone(&clock),
        )
        .expect("roots"),
    );
    let issuer = Arc::new(LocalIssuer::new(
        &pool.ca,
        "localhost",
        Arc::clone(&clock),
        Span::from_hours(3),
    ));
    let mut renewer = Renewer::new(
        issuer.clone(),
        Arc::clone(&identity),
        os_entropy(),
        Arc::clone(&clock),
        "qip-test-fabricd",
    )
    .expect("renewer");
    let status = renewer.status();

    let wait = renewer.tick();
    assert_eq!(issuer.count(), 1, "the first tick issues");
    assert!(identity.is_current());
    let start = Timestamp::from_civil(2030, 1, 1);
    assert_eq!(
        identity.validity().map(|v| v.renew_at()),
        Some(start.saturating_add(Span::from_hours(2))),
        "renewal at two-thirds of 3h"
    );
    // The wait is capped at an hour, so a clock that steps is noticed
    // within one; the loop wakes and finds nothing due.
    assert_eq!(wait, Duration::from_secs(3600));

    manual.advance(Span::from_hours(1));
    renewer.tick();
    assert_eq!(issuer.count(), 1, "renewed before two-thirds");

    manual.advance(Span::from_hours(1));
    renewer.tick();
    assert_eq!(issuer.count(), 2, "not renewed at two-thirds");
    assert_eq!(status.issued(), 2);

    // The CA goes away at the next renewal.
    issuer.failing.store(true, Ordering::SeqCst);
    manual.advance(Span::from_hours(2));
    assert_eq!(renewer.tick(), Duration::from_secs(5));
    assert_eq!(renewer.tick(), Duration::from_secs(10));
    assert_eq!(status.consecutive_failures(), 2);
    assert!(
        status
            .last_error()
            .is_some_and(|e| e.contains("stand-in CA is down")),
        "{:?}",
        status.last_error()
    );
    assert!(
        identity.is_current(),
        "a failed renewal withdrew a certificate that had an hour left"
    );

    // Past the not-after, nothing is presented and the status says so.
    manual.advance(Span::from_hours(1));
    assert!(!identity.is_current());
    assert!(!status.is_current());
    assert_eq!(status.to_json()["current"], false);

    // And the loop's next success restores it.
    issuer.failing.store(false, Ordering::SeqCst);
    renewer.tick();
    assert!(status.is_current());
    assert_eq!(status.consecutive_failures(), 0);
}

/// `CasIdentity::run`'s first attempt happens before it returns, so a
/// healthy start is ready before its caller binds a port; and dropping the
/// handle stops the loop. Mutation: skip the first tick in `run` and the
/// identity is not current on return; this fails.
#[test]
fn an_issued_identity_is_current_when_start_returns() {
    let pool = pool();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let identity = Arc::new(
        WorkloadIdentity::awaiting_issuance(
            pool.ca.certificate_pem().as_bytes(),
            Arc::clone(&clock),
        )
        .expect("roots"),
    );
    let issuer = Arc::new(LocalIssuer::new(
        &pool.ca,
        "localhost",
        Arc::clone(&clock),
        Span::from_hours(8),
    ));
    let running = CasIdentity::run(
        Renewer::new(
            issuer.clone(),
            Arc::clone(&identity),
            os_entropy(),
            clock,
            "qip-test-fabricd",
        )
        .expect("renewer"),
        identity,
    )
    .expect("runs");
    assert!(running.identity().is_current());
    assert_eq!(running.status().issued(), 1);
    running.stop();
    assert_eq!(issuer.count(), 1, "the loop issued again before it was due");
}

/// The hand-placed path keeps the same rule at load: a certificate already
/// past its not-after is refused, not loaded into a slot that will present
/// nothing. Mutation: drop the expiry check from `from_pem_at` and the
/// identity loads; this fails.
#[test]
fn a_hand_placed_certificate_already_expired_is_refused_at_load() {
    let pool = pool();
    let leaf = certs::leaf(&pool.ca, 5, "localhost");
    let load = |clock: Arc<dyn Clock>| {
        WorkloadIdentity::from_pem_at(
            leaf.certificate_pem().as_bytes(),
            leaf.key_pem().as_bytes(),
            pool.ca.certificate_pem().as_bytes(),
            clock,
        )
    };
    // Premise: inside its window (2020 to the end of 2049) it loads.
    assert!(load(Arc::new(SystemClock)).expect("loads").is_current());
    let error = load(Arc::new(ManualClock::new(Timestamp::from_civil(
        2050, 1, 2,
    ))))
    .expect_err("an expired certificate loaded");
    assert!(error.message().contains("expired"), "{error}");
}

/// Kept from the KMS suite's pattern: an `http` endpoint is refused, so a
/// misconfigured stand-in can never be reached in plaintext.
#[test]
fn the_ca_service_client_will_not_be_pointed_at_plaintext() {
    let pool = pool();
    let directory = scratch("plaintext");
    let settings = settings(&pool, &directory);
    let client = CasClient::new(
        &settings,
        WorkloadSubject::from_service_account(SERVICE_ACCOUNT).expect("subject"),
        TlsHttpClient::new(
            ClientLimits::default(),
            tls::server_authenticated_client(pool.google_ca.certificate_pem().as_bytes())
                .expect("roots"),
        ),
        Arc::new(Token),
    );
    assert!(client.with_endpoint("http://127.0.0.1:1/v1").is_err());
}
