//! The connection cap: a server holding `max_concurrent` live connections
//! answers the next one 503 at once, and serves the held ones when they
//! finish (R12-FABRIC-8074).
//!
//! Driven here with a handler that holds its requests, because no route of
//! `qip-fabricd`'s data plane holds a connection: every route answers at
//! once, a connection carries one request, and a peer that is slow to send
//! its request is bounded first by `max_handshaking`. The broker's own
//! limits are pinned to this server's defaults by
//! `qip-fabricd/tests/broker.rs::the_broker_data_plane_runs_under_the_capped_server_limits`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // integration tests may unwrap: a panic is the failure report

use qip_transport::server::{Handler, Request, Response, Server, ServerLimits};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Holds every request until the gate opens, counting those inside.
struct Held {
    inside: AtomicUsize,
    open: Mutex<bool>,
    changed: Condvar,
}

impl Handler for Held {
    fn handle(&self, _request: &Request) -> Response {
        self.inside.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
        Response::json(200, r#"{"served":true}"#)
    }
}

fn status_of(address: &str) -> u16 {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    stream
        .write_all(format!("GET /held HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .unwrap();
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);
    String::from_utf8_lossy(&answer)
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

/// Mutation: in `Server::serve`, compare `live` against
/// `max_concurrent + 1` — the next connection is admitted and held like the
/// others, so it is not answered 503 and the refusal assertion fails.
#[test]
fn the_connection_past_the_cap_is_refused_503_while_the_held_ones_are_served() {
    const CAP: usize = 4;
    let held = Arc::new(Held {
        inside: AtomicUsize::new(0),
        open: Mutex::new(false),
        changed: Condvar::new(),
    });
    let server = Server::bind(
        "127.0.0.1:0",
        held.clone(),
        ServerLimits {
            max_concurrent: CAP,
            ..ServerLimits::default()
        },
    )
    .unwrap();
    let address = server.local_address().unwrap();
    let stop = server.shutdown_handle();
    std::thread::spawn(move || {
        let _ = server.serve();
    });

    let holders: Vec<_> = (0..CAP)
        .map(|_| {
            let address = address.clone();
            std::thread::spawn(move || status_of(&address))
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(30);
    while held.inside.load(Ordering::SeqCst) < CAP {
        assert!(Instant::now() < deadline, "the held requests never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(status_of(&address), 503, "the connection past the cap");
    assert_eq!(
        held.inside.load(Ordering::SeqCst),
        CAP,
        "and it never reached the handler"
    );

    *held.open.lock().unwrap() = true;
    held.changed.notify_all();
    for holder in holders {
        assert_eq!(holder.join().unwrap(), 200, "a held connection is served");
    }
    assert_eq!(status_of(&address), 200, "a slot freed is a slot reused");
    stop.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(&address);
}
