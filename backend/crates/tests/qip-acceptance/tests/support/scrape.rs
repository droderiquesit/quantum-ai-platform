//! A minimal, bounded `GET` for one path: `/metrics`.
//!
//! The slice suites need to read a sibling process's Prometheus exposition
//! without pulling in an HTTP client crate — the workspace permits exactly
//! two dependencies and neither is this (ADR 0002, ADR 0009). `qip-transport`
//! is the in-tree client for production code; this is smaller than that on
//! purpose. It exists to prove a body came back within a bound, never to be a
//! general HTTP client, and it never ships.
//!
//! Bounded rather than blocking indefinitely, because the whole reason a
//! suite reaches for this against a proxied connection is to prove a fault
//! took effect — and a fault that hangs the client instead of the connection
//! it targets would make every such suite hang too, rather than fail with a
//! reason.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// `GET {path} HTTP/1.1` against `address`, returning the response body.
///
/// Refuses to wait past `timeout` for the connection, the write, or the
/// read — a stalled peer accepts the connection and never answers, and that
/// must surface here as a bounded, named [`std::io::Error`] rather than a
/// hang, or a suite asserting the fault fired would itself hang instead of
/// failing.
pub(crate) fn get(address: SocketAddr, path: &str, timeout: Duration) -> std::io::Result<String> {
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;

    let remaining = |started: Instant| -> Duration {
        timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(1))
    };
    stream.set_read_timeout(Some(remaining(started)))?;
    stream.set_write_timeout(Some(remaining(started)))?;

    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let body = text
        .split_once("\r\n\r\n")
        .map_or(text.as_ref(), |(_, body)| body);
    Ok(body.to_string())
}
