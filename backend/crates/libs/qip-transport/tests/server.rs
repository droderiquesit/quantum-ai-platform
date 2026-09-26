//! The HTTP/1.1 server, proven from its new home.
//!
//! ADR 0100 §1 moved this server out of `qip-api` and into `qip-transport` so
//! that `qip-fabricd` and `qip-ledgerd` can serve without depending on an
//! application crate. The move was meant to carry every limit unchanged; this
//! is the test a mechanical move needs and a rename does not — proof that the
//! body-size refusal, the one an unauthenticated caller could otherwise use to
//! make this process buffer an unbounded request, still fires from
//! `qip_transport::server` rather than having been left behind in the crate
//! the code moved out of.

use qip_transport::server::{Handler, Request, Response, Server, ServerLimits};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

/// A handler that would prove the limit had failed to hold, by reporting the
/// body it was actually handed. If the refusal did not fire, this is what
/// runs, and its answer is `200`, not `413`.
#[derive(Debug)]
struct Echo;

impl Handler for Echo {
    fn handle(&self, request: &Request) -> Response {
        Response::text(200, format!("body_len={}", request.body.len()))
    }
}

fn send(address: &str, raw: &str) -> String {
    let mut stream = TcpStream::connect(address).expect("connects to the bound listener");
    stream
        .write_all(raw.as_bytes())
        .expect("writes the raw request");
    stream.flush().expect("flushes the request");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

#[test]
fn the_moved_server_refuses_a_body_over_its_limit_from_its_new_home() {
    let limits = ServerLimits {
        max_body: 64,
        ..ServerLimits::default()
    };
    let server = Server::bind("127.0.0.1:0", Arc::new(Echo), limits)
        .expect("binds an ephemeral loopback port");
    let address = server
        .local_address()
        .expect("a bound listener reports its own address");
    let handle = std::thread::spawn(move || {
        let _ = server.serve_once();
    });

    // A declared length many times the limit: the interesting property is
    // that this is refused on the declared length alone, before the body
    // behind it is ever read.
    let response = send(
        &address,
        "POST /anything HTTP/1.1\r\nhost: localhost\r\ncontent-length: 100000000\r\n\r\n",
    );
    handle.join().expect("the serving thread does not panic");

    // Assert the premise first: the request actually reached the server and
    // produced a response, so a `413` below is evidence the limit fired and
    // not evidence the connection never landed.
    assert!(
        response.starts_with("HTTP/1.1"),
        "no HTTP response came back at all, so nothing below tests the limit: {response}"
    );
    assert!(
        response.starts_with("HTTP/1.1 413"),
        "a body declared far past `max_body` must be refused before it is read, \
         not handed to the handler and echoed back: {response}"
    );
}
