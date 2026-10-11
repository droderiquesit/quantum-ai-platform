//! The server's bounds on peers that have not yet shown who they are, over
//! real loopback sockets: a wall-clock deadline on the handshake and request
//! read that a dribbling peer cannot reset, a cap on connections still
//! handshaking that is refused at the accept loop, and a tight bound on the
//! read the server does for a peer it has already refused.
//!
//! Before these, a host that sent a ClientHello one byte at a time held a
//! worker for as long as it liked — the per-read timeout restarted on every
//! byte — and enough such hosts left no worker for a legitimate peer.
//!
//! Every limit a test relies on is set small in that test and every other is
//! set large, so that what closes a connection can only be the bound under
//! test: a test of the overall deadline whose per-read timeout was shorter
//! than the dribble interval would pass with the deadline deleted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // integration tests may unwrap: a panic is the failure report

#[path = "support/certs.rs"]
mod certs;

use qip_transport::server::{
    Handler, Request as ServerRequest, Response as ServerResponse, Server, ServerLimits,
};
use qip_transport::tls::{PeerNames, TlsServer, WorkloadIdentity};
use qip_transport::{ClientLimits, HttpRequest, Method, TlsHttpClient};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Counting {
    calls: Arc<AtomicUsize>,
}

impl Handler for Counting {
    fn handle(&self, _request: &ServerRequest) -> ServerResponse {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ServerResponse::text(200, "served".to_string())
    }
}

struct Pki {
    ca: certs::Minted,
    server: certs::Minted,
    client: certs::Minted,
}

fn pki() -> Pki {
    let ca = certs::authority("qip deadline test pool");
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

/// Every bound generous, so a test tightens only the one it is about.
fn generous() -> ServerLimits {
    ServerLimits {
        read_timeout: Duration::from_secs(30),
        connection_deadline: Duration::from_secs(30),
        max_handshaking: 64,
        refused_request_deadline: Duration::from_secs(30),
        refused_request_bytes: 64 * 1024,
        ..ServerLimits::default()
    }
}

fn tls_server(pki: &Pki, peers: PeerNames, limits: ServerLimits) -> (Server, Arc<AtomicUsize>) {
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
    let server = Server::bind_tls("127.0.0.1:0", handler, limits, tls).expect("binds");
    (server, calls)
}

fn port_of(server: &Server) -> String {
    server
        .local_address()
        .expect("address")
        .rsplit_once(':')
        .expect("host:port")
        .1
        .to_string()
}

/// Serve exactly one connection on a thread and report when it is done.
fn serve_one(server: Server) -> mpsc::Receiver<Instant> {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = server.serve_once();
        let _ = done.send(Instant::now());
    });
    finished
}

/// Write one byte every `interval` until the server stops taking them or
/// `stop` is set, so the peer is never silent long enough for a per-read
/// timeout to fire.
fn dribble(mut socket: TcpStream, interval: Duration, stop: mpsc::Receiver<()>) {
    std::thread::spawn(move || {
        loop {
            if socket.write_all(&[0]).is_err() {
                return;
            }
            if stop.recv_timeout(interval).is_ok() {
                return;
            }
        }
    });
}

/// The failure this prevents: a TLS record header announcing a ClientHello
/// and then one byte every 200 ms kept a worker in `complete_io` for as
/// long as the peer kept sending, because each byte restarted the 30 s
/// per-read timeout. Mutation: make `Budgeted::read` ignore `deadline` (read
/// with the per-read timeout only) and the connection outlives the
/// ten-second wait; this fails.
#[test]
fn a_peer_dribbling_its_client_hello_is_closed_by_the_connection_deadline() {
    let pki = pki();
    let limits = ServerLimits {
        connection_deadline: Duration::from_secs(1),
        ..generous()
    };
    let (server, calls) = tls_server(&pki, PeerNames::default(), limits);
    let port = port_of(&server);
    let finished = serve_one(server);

    let started = Instant::now();
    let mut socket = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connects");
    // A handshake record of 512 bytes: rustls buffers until it is whole,
    // so every zero after it is a byte it waits for, never one it rejects.
    socket
        .write_all(&[0x16, 0x03, 0x01, 0x02, 0x00])
        .expect("the record header is written");
    let (stop, stopped) = mpsc::channel();
    dribble(
        socket.try_clone().expect("a second handle"),
        Duration::from_millis(200),
        stopped,
    );

    let closed_at = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the dribbling connection was still held ten seconds later");
    let _ = stop.send(());
    let held = closed_at.duration_since(started);
    // Premise: it was the deadline and not a rejected byte that closed it —
    // the server held the connection for about the deadline, not at once.
    assert!(
        held >= Duration::from_millis(900),
        "the connection closed after {held:?}, before the deadline could have"
    );
    assert!(
        held < Duration::from_secs(5),
        "held {held:?} on a 1 s deadline"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// The same deadline on a plaintext server, where the slow part is the
/// request line itself. Mutation: as above; the server waits on the
/// dribble and this fails.
#[test]
fn a_plaintext_peer_dribbling_its_request_line_is_closed_by_the_connection_deadline() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler: Arc<dyn Handler> = Arc::new(Counting {
        calls: Arc::clone(&calls),
    });
    let limits = ServerLimits {
        connection_deadline: Duration::from_secs(1),
        ..generous()
    };
    let server = Server::bind("127.0.0.1:0", handler, limits).expect("binds");
    let port = port_of(&server);
    let finished = serve_one(server);

    let started = Instant::now();
    let mut socket = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connects");
    socket.write_all(b"GET /").expect("the start of a request");
    let (stop, stopped) = mpsc::channel();
    // `a` bytes extend the path forever without ending the line.
    let mut writer = socket.try_clone().expect("a second handle");
    std::thread::spawn(move || {
        loop {
            if writer.write_all(b"a").is_err() {
                return;
            }
            if stopped.recv_timeout(Duration::from_millis(200)).is_ok() {
                return;
            }
        }
    });

    let closed_at = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the dribbling request was still being read ten seconds later");
    let _ = stop.send(());
    let held = closed_at.duration_since(started);
    assert!(held >= Duration::from_millis(900), "closed after {held:?}");
    assert!(
        held < Duration::from_secs(5),
        "held {held:?} on a 1 s deadline"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the server closed `socket` within `within`: a read that returns
/// end-of-stream or a reset, rather than timing out.
fn closed_within(socket: &mut TcpStream, within: Duration) -> bool {
    socket
        .set_read_timeout(Some(within))
        .expect("a read timeout");
    let mut byte = [0u8; 1];
    match socket.read(&mut byte) {
        Ok(0) => true,
        Ok(_) => false,
        Err(error) => !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
    }
}

/// The failure this prevents: connections that never finish a handshake
/// each took a worker, up to the whole of `max_concurrent`. With the cap at
/// two, two silent connections hold both slots; a third is closed at once by
/// the accept loop rather than given a worker; and the slot a stalled peer
/// leaves is returned, so a legitimate client is served afterwards.
///
/// Mutation: delete the `max_handshaking` check in `serve` and the third
/// connection is held open (its read times out instead of seeing the
/// close); delete `HandshakeSlot`'s `Drop` and the count never falls back,
/// so the wait for it fails. Either fails this.
#[test]
fn connections_beyond_the_handshake_cap_are_closed_at_once_and_a_freed_slot_serves_a_peer() {
    let pki = pki();
    let limits = ServerLimits {
        max_handshaking: 2,
        ..generous()
    };
    let (server, calls) = tls_server(&pki, PeerNames::default(), limits);
    let port = port_of(&server);
    let handshaking = server.handshaking_handle();
    let shutdown = server.shutdown_handle();
    let serving = std::thread::spawn(move || server.serve());

    let address = format!("127.0.0.1:{port}");
    let first = TcpStream::connect(&address).expect("connects");
    let mut second = TcpStream::connect(&address).expect("connects");
    wait_for("two connections in their handshake", || {
        handshaking.load(Ordering::SeqCst) == 2
    });
    // Premise: a held slot is held — the second stalled peer is not closed.
    assert!(
        !closed_within(&mut second, Duration::from_millis(300)),
        "a connection inside the cap was closed"
    );

    let mut third = TcpStream::connect(&address).expect("the kernel accepts it");
    assert!(
        closed_within(&mut third, Duration::from_secs(2)),
        "a connection beyond the handshake cap was held open"
    );
    assert_eq!(handshaking.load(Ordering::SeqCst), 2);

    drop(first);
    wait_for("the stalled peer's slot to be returned", || {
        handshaking.load(Ordering::SeqCst) == 1
    });
    let client = TlsHttpClient::new(
        ClientLimits {
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(2),
            ..ClientLimits::default()
        },
        identity(&pki.client, &pki.ca)
            .client_config()
            .expect("client config"),
    );
    let response = client
        .send(
            &HttpRequest::new_tls(Method::Get, &format!("https://localhost:{port}/x"))
                .expect("url"),
        )
        .expect("a legitimate client is served once a slot is free");
    assert_eq!(response.status, 200);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    drop(second);
    shutdown.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(&address);
    serving
        .join()
        .expect("the accept loop does not panic")
        .expect("the accept loop ends cleanly");
}

/// A TLS client holding the listed certificate's sibling — chained by the
/// same CA, named nowhere in the server's list — with the handshake done.
fn refused_session(
    pki: &Pki,
    port: &str,
) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    let unlisted = certs::leaf(&pki.ca, 4, "ledgerd.test.qip.internal");
    let config = identity(&unlisted, &pki.ca)
        .client_config()
        .expect("client config");
    let name = rustls::pki_types::ServerName::try_from("localhost").expect("a server name");
    let session = rustls::ClientConnection::new(config, name).expect("a client session");
    let socket = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connects");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read timeout");
    let mut stream = rustls::StreamOwned::new(session, socket);
    while stream.conn.is_handshaking() {
        stream
            .conn
            .complete_io(&mut stream.sock)
            .expect("the unlisted certificate chains, so the handshake completes");
    }
    stream
}

/// The failure this prevents: the server read a refused workload's request
/// under the full limits — fifteen seconds a read, a megabyte of body — so a
/// workload it had already refused held a worker as long as any admitted
/// request could. Here the peer finishes its handshake and then sends
/// nothing. Mutation: drop the `tighten` call on the refusal path and the
/// server waits out the 30 s connection deadline; this fails.
#[test]
fn a_refused_workload_that_sends_nothing_is_answered_within_the_refusal_deadline() {
    let pki = pki();
    let limits = ServerLimits {
        refused_request_deadline: Duration::from_millis(500),
        ..generous()
    };
    let admitted = PeerNames::new(["edge-node.test.qip.internal".to_string()]).expect("names");
    let (server, calls) = tls_server(&pki, admitted, limits);
    let port = port_of(&server);
    let finished = serve_one(server);

    let started = Instant::now();
    let mut stream = refused_session(&pki, &port);
    let closed_at = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the refused, silent workload was still held ten seconds later");
    let held = closed_at.duration_since(started);
    assert!(held < Duration::from_secs(3), "held {held:?}");
    // And the refusal still reaches it, which is what the read was for.
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// The byte half of the same bound: a refused workload declares a body the
/// full limits admit and sends it a kilobyte at a time. With the refusal
/// deadline set long, only the byte cap can end the read early. Mutation:
/// read the refused request without `take(refused_request_bytes)` and with
/// the full limits, and the server keeps reading the body until the
/// refusal deadline; this fails.
#[test]
fn a_refused_workload_streaming_a_large_body_is_answered_after_a_few_kilobytes() {
    let pki = pki();
    let limits = ServerLimits {
        refused_request_bytes: 2 * 1024,
        refused_request_deadline: Duration::from_secs(8),
        ..generous()
    };
    let admitted = PeerNames::new(["edge-node.test.qip.internal".to_string()]).expect("names");
    let (server, calls) = tls_server(&pki, admitted, limits);
    let port = port_of(&server);
    let finished = serve_one(server);

    let mut stream = refused_session(&pki, &port);
    let started = Instant::now();
    stream
        .write_all(b"POST /x HTTP/1.1\r\nhost: localhost\r\ncontent-length: 900000\r\n\r\n")
        .expect("the head is written");
    stream.flush().expect("flushes");
    let (stop, stopped) = mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        let chunk = [b'x'; 1024];
        for _ in 0..900 {
            if stream.write_all(&chunk).is_err() || stream.flush().is_err() {
                break;
            }
            if stopped.recv_timeout(Duration::from_millis(50)).is_ok() {
                break;
            }
        }
        stream
    });

    let closed_at = finished
        .recv_timeout(Duration::from_secs(15))
        .expect("the refused upload was still being read fifteen seconds later");
    let _ = stop.send(());
    let held = closed_at.duration_since(started);
    assert!(
        held < Duration::from_secs(3),
        "the server read a refused body for {held:?} rather than stopping at the cap"
    );
    let _ = writer.join();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
