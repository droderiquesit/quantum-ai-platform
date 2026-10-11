//! Mutual TLS over real loopback sockets (ADR 0112): a server bound with
//! `Server::bind_tls` serves a client that presents a certificate its roots
//! chain, and refuses — before any handler runs — a client with no
//! certificate, a certificate from another authority, or no TLS at all.
//!
//! The properties are about what reaches the handler, so every test counts
//! handler invocations rather than trusting an error string: a refusal that
//! happened after the handler ran would be no refusal.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "support/certs.rs"]
mod certs;

use qip_core::hash::sha256_hex;
use qip_transport::event_fabric::auth::{self, BearerToken, IdentityTable};
use qip_transport::event_fabric::protocol::{
    Metadata, MetadataRequest, Refusal, Request, Response, encode_response,
};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::server::{
    Handler, PEER_IDENTITY_HEADER, Request as ServerRequest, Response as ServerResponse, Server,
    ServerLimits,
};
use qip_transport::tls::{LinkSecurity, PeerNames, TlsServer, WorkloadIdentity};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method, TlsHttpClient};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Counts calls and echoes the identity header it was handed.
struct Counting {
    calls: Arc<AtomicUsize>,
}

impl Handler for Counting {
    fn handle(&self, request: &ServerRequest) -> ServerResponse {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let identity = request.header(PEER_IDENTITY_HEADER).unwrap_or("none");
        ServerResponse::text(200, identity.to_string())
    }
}

struct Pki {
    ca: certs::Minted,
    server: certs::Minted,
    client: certs::Minted,
}

fn pki() -> Pki {
    let ca = certs::authority("qip test pool");
    let server = certs::leaf(&ca, 2, "localhost");
    let client = certs::leaf(&ca, 3, "edge-node.test.qip.internal");
    Pki { ca, server, client }
}

fn identity(leaf: &certs::Minted, ca: &certs::Minted) -> WorkloadIdentity {
    WorkloadIdentity::from_pem(
        leaf.certificate_pem().as_bytes(),
        leaf.key_pem().as_bytes(),
        ca.certificate_pem().as_bytes(),
    )
    .expect("minted material loads")
}

fn limits() -> ClientLimits {
    ClientLimits {
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(2),
        write_timeout: Duration::from_secs(2),
        ..ClientLimits::default()
    }
}

/// Bind a TLS server answering `connections` connections on a thread.
fn serve_tls(
    pki: &Pki,
    peers: PeerNames,
    connections: usize,
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler: Arc<dyn Handler> = Arc::new(Counting {
        calls: Arc::clone(&calls),
    });
    let tls = TlsServer {
        config: identity(&pki.server, &pki.ca)
            .server_config()
            .expect("a server config"),
        peers,
    };
    let server = Server::bind_tls("127.0.0.1:0", handler, ServerLimits::default(), tls)
        .expect("binds an ephemeral port");
    assert!(server.is_mutual_tls());
    let port = server
        .local_address()
        .expect("address")
        .rsplit_once(':')
        .expect("host:port")
        .1
        .to_string();
    let thread = std::thread::spawn(move || {
        for _ in 0..connections {
            let _ = server.serve_once();
        }
    });
    (port, calls, thread)
}

/// Mutation: make `Server::bind_tls` ignore its config (serve plaintext) or
/// drop the identity header insertion, and the request below fails or reads
/// `none`.
#[test]
fn a_client_presenting_a_certificate_the_roots_chain_is_served_and_named() {
    let pki = pki();
    let (port, calls, thread) = serve_tls(&pki, PeerNames::default(), 1);
    let client = TlsHttpClient::new(
        limits(),
        identity(&pki.client, &pki.ca)
            .client_config()
            .expect("client config"),
    );
    let response = client
        .send(
            &HttpRequest::new_tls(Method::Get, &format!("https://localhost:{port}/who"))
                .expect("url")
                // A client cannot assert its own identity; this copy is stripped.
                .with_header(PEER_IDENTITY_HEADER, "operator"),
        )
        .expect("an authenticated client is served");
    thread.join().expect("server thread");
    assert_eq!(response.status, 200);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // No admitted-name list, so no identity is asserted — and the client's own
    // claim did not survive.
    assert_eq!(response.body_as_str().expect("text"), "none");
}

/// A workload identity whose chain holds no certificate is refused when it is
/// loaded, even beside a good key and good roots.
///
/// `empty_tls_material_is_refused_at_load_rather_than_at_the_first_handshake`
/// in `tls.rs` passes `b""` for all three parts, so the key parse refuses
/// before the chain check is reached; the round-2 mutation pass deleted the
/// chain check and that test still passed. This one isolates it.
///
/// Mutation: delete the `chain.is_empty()` refusal in
/// `WorkloadIdentity::from_pem` and this fails.
#[test]
fn an_identity_with_no_certificate_is_refused_beside_a_good_key_and_roots() {
    let pki = pki();
    // Premise: the same key and roots load with the chain present.
    identity(&pki.client, &pki.ca);
    let error = WorkloadIdentity::from_pem(
        b"",
        pki.client.key_pem().as_bytes(),
        pki.ca.certificate_pem().as_bytes(),
    )
    .expect_err("an identity with no certificate loaded");
    assert!(
        error.message().contains("certificate chain holds no certificate"),
        "refused for another reason: {}",
        error.message()
    );
}

/// Mutation: build the verifier with `allow_unauthenticated()` (optional
/// client auth) and the anonymous client completes the handshake; this fails
/// on `outcome.is_err()`.
///
/// It does not fail on the handler count, and this comment said it would
/// until the round-2 mutation pass ran it: under that mutation the session
/// has no peer certificate, `serve_tls_connection`'s `None` arm answers 403,
/// and the handler still never runs. That arm is the second layer; the
/// handshake refusal is the first, and this test pins the first.
#[test]
fn a_client_with_no_certificate_never_reaches_a_handler() {
    let pki = pki();
    let (port, calls, thread) = serve_tls(&pki, PeerNames::default(), 1);
    // Server-authenticated only: verifies the server, presents nothing.
    let anonymous = TlsHttpClient::new(
        limits(),
        qip_transport::tls::server_authenticated_client(pki.ca.certificate_pem().as_bytes())
            .expect("config"),
    );
    let outcome = anonymous.send(
        &HttpRequest::new_tls(Method::Get, &format!("https://localhost:{port}/who")).expect("url"),
    );
    thread.join().expect("server thread");
    assert!(
        outcome.is_err(),
        "an anonymous client was answered: {outcome:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the handler ran for a client that presented no certificate"
    );
}

/// Mutation: swap the client verifier's roots for "any" and a certificate
/// from a stranger's CA is admitted; this fails.
#[test]
fn a_certificate_from_another_authority_is_refused_at_the_handshake() {
    let pki = pki();
    let stranger_ca = certs::authority("someone else's pool");
    let stranger = certs::leaf(&stranger_ca, 9, "edge-node.test.qip.internal");
    let (port, calls, thread) = serve_tls(&pki, PeerNames::default(), 1);
    // The stranger trusts our server's CA (so the failure is ours refusing
    // them, not them refusing us) but presents their own pool's certificate.
    let config = WorkloadIdentity::from_pem(
        stranger.certificate_pem().as_bytes(),
        stranger.key_pem().as_bytes(),
        pki.ca.certificate_pem().as_bytes(),
    )
    .expect("loads")
    .client_config()
    .expect("config");
    let outcome = TlsHttpClient::new(limits(), config).send(
        &HttpRequest::new_tls(Method::Get, &format!("https://localhost:{port}/who")).expect("url"),
    );
    thread.join().expect("server thread");
    assert!(outcome.is_err(), "a foreign certificate was admitted");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Mutation: route a TLS server's accepted socket to the plaintext path when
/// the first bytes look like HTTP, and the plaintext client is answered;
/// this fails.
#[test]
fn a_plaintext_client_gets_nothing_from_a_mutual_tls_server() {
    let pki = pki();
    let (port, calls, thread) = serve_tls(&pki, PeerNames::default(), 1);
    let outcome = HttpClient::new(limits()).send(
        &HttpRequest::new(Method::Get, &format!("http://127.0.0.1:{port}/who")).expect("url"),
    );
    thread.join().expect("server thread");
    assert!(
        outcome
            .as_ref()
            .map_or(true, |response| response.status != 200),
        "a plaintext request was served by a TLS server: {outcome:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Mutation: make `PeerNames::admit` return `Ok(None)` for a non-empty list
/// and the unlisted workload is served; this fails.
#[test]
fn a_server_naming_its_peers_admits_only_those_names() {
    let pki = pki();
    let admitted = PeerNames::new(["edge-node.test.qip.internal".to_string()]).expect("names");
    let (port, calls, thread) = serve_tls(&pki, admitted, 2);
    let listed = TlsHttpClient::new(
        limits(),
        identity(&pki.client, &pki.ca)
            .client_config()
            .expect("config"),
    );
    let response = listed
        .send(
            &HttpRequest::new_tls(Method::Get, &format!("https://localhost:{port}/who"))
                .expect("url"),
        )
        .expect("a listed workload is served");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_as_str().expect("text"),
        "edge-node.test.qip.internal",
        "the handler was not told which workload called"
    );

    let unlisted_leaf = certs::leaf(&pki.ca, 4, "ledgerd.test.qip.internal");
    let unlisted = TlsHttpClient::new(
        limits(),
        identity(&unlisted_leaf, &pki.ca)
            .client_config()
            .expect("config"),
    );
    let refused = unlisted
        .send(
            &HttpRequest::new_tls(Method::Get, &format!("https://localhost:{port}/who"))
                .expect("url"),
        )
        .expect("the refusal is an HTTP answer");
    thread.join().expect("server thread");
    assert_eq!(refused.status, 403);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the unlisted workload reached the handler"
    );
}

/// Mutation: let either client send the other's URLs (drop the scheme
/// checks in `send`) and a configured TLS link goes out in plaintext; this
/// fails without either opening a socket.
#[test]
fn neither_client_will_send_the_other_schemes_url() {
    let pki = pki();
    let tls = TlsHttpClient::new(
        limits(),
        identity(&pki.client, &pki.ca)
            .client_config()
            .expect("config"),
    );
    let plain_request = HttpRequest::new(Method::Get, "http://127.0.0.1:1/x").expect("url");
    assert_eq!(
        tls.send(&plain_request)
            .expect_err("TLS client sent http")
            .code(),
        "unsupported_scheme"
    );
    let tls_request = HttpRequest::new_tls(Method::Get, "https://127.0.0.1:1/x").expect("url");
    assert_eq!(
        HttpClient::new(limits())
            .send(&tls_request)
            .expect_err("plaintext client sent https")
            .code(),
        "unsupported_scheme"
    );
}

const PRODUCER_TOKEN: &str = "mtlsTestProducerTokenForTransportAuth0123456789x";

struct Fabric {
    identities: IdentityTable,
}

impl Handler for Fabric {
    fn handle(&self, request: &ServerRequest) -> ServerResponse {
        let response = match auth::verify(&self.identities, request.header(auth::HEADER)) {
            Ok(_) => Response::Metadata(Metadata::new("orders", 3, 7, 120, 80).expect("metadata")),
            Err(_) => Response::Refused(Refusal::AclDenied),
        };
        let body = encode_response(&response).expect("encodes");
        ServerResponse::json(200, String::from_utf8(body).expect("utf-8"))
    }
}

/// The fabric transport over a mutual-TLS link speaks `https` with the
/// workload certificate and still presents its bearer token.
///
/// Mutation: make `HttpTransport::over` build a plaintext transport for a
/// `MutualTls` link and the call reaches a TLS broker as plaintext and
/// fails; this fails.
#[test]
fn the_fabric_transport_over_a_mutual_tls_link_is_answered_by_a_tls_broker() {
    let pki = pki();
    let identities = IdentityTable::parse(&format!(
        "producer-a {}\n",
        sha256_hex(PRODUCER_TOKEN.as_bytes())
    ))
    .expect("identities");
    let handler: Arc<dyn Handler> = Arc::new(Fabric { identities });
    let server = Server::bind_tls(
        "127.0.0.1:0",
        handler,
        ServerLimits::default(),
        TlsServer {
            config: identity(&pki.server, &pki.ca)
                .server_config()
                .expect("config"),
            peers: PeerNames::default(),
        },
    )
    .expect("binds");
    let port = server
        .local_address()
        .expect("address")
        .rsplit_once(':')
        .expect("port")
        .1
        .to_string();
    let thread = std::thread::spawn(move || {
        let _ = server.serve_once();
    });
    let link = LinkSecurity::MutualTls(Arc::new(identity(&pki.client, &pki.ca)));
    let mut transport = HttpTransport::over(
        &link,
        &format!("localhost:{port}"),
        BearerToken::new(PRODUCER_TOKEN.to_string()).expect("token"),
    )
    .expect("a transport");
    assert!(transport.is_mutual_tls());
    let answer = transport
        .call(
            Request::Metadata(MetadataRequest {
                stream: "orders".to_string(),
                partition: 3,
            }),
            Timeouts::default(),
        )
        .expect("the TLS broker answers");
    thread.join().expect("server thread");
    assert!(matches!(answer, Response::Metadata(_)), "{answer:?}");
}
