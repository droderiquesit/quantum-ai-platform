//! A minimal, bounded `GET`, and the one reading of Prometheus exposition the
//! slice suites need.
//!
//! The suites read a sibling process's `/metrics` without an HTTP client
//! crate — the workspace permits exactly two dependencies and neither is one
//! (ADR 0002, ADR 0009). `qip-transport` is the in-tree client for production
//! code; this is smaller than that on purpose. It exists to prove a body came
//! back within a bound, never to be a general client, and it never ships.
//!
//! Bounded by one deadline for the whole exchange, because the reason a suite
//! reaches for this against a proxied connection is to prove a fault took
//! effect, and a scrape that could hang — or be held open indefinitely by a
//! peer trickling one byte at a time — would make that suite hang too rather
//! than fail with a reason.

use std::io::{Error, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// The largest response this reads. Exposition from these binaries is a few
/// kilobytes; a peer sending more than this is broken, and reading it all
/// would be an unbounded buffer.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// `GET {path} HTTP/1.1` against `address`, returning the body of a `200`.
///
/// Refuses, as an [`std::io::Error`]:
///
/// * any exchange not finished within `timeout` in total — connection, write
///   and every read together, so a peer that trickles cannot re-arm it
///   ([`ErrorKind::TimedOut`]);
/// * any status but `200`, naming the status and the start of the body,
///   because a suite that parsed an error page as metrics would find no
///   series and blame the process that served it ([`ErrorKind::Other`]);
/// * a body shorter than its `content-length`, malformed chunking, or bytes
///   that are not UTF-8 ([`ErrorKind::InvalidData`]).
///
/// A chunked body is decoded rather than returned with its framing in it.
pub(crate) fn get(address: SocketAddr, path: &str, timeout: Duration) -> std::io::Result<String> {
    let deadline = Instant::now() + timeout;
    let expired = || {
        Error::new(
            ErrorKind::TimedOut,
            format!("GET {path} from {address} did not finish within {timeout:?}"),
        )
    };
    let remaining = || {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(expired())
        } else {
            Ok(left)
        }
    };

    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_write_timeout(Some(remaining()?))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        stream.set_read_timeout(Some(remaining()?))?;
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buffer[..n]);
                if raw.len() > MAX_RESPONSE_BYTES {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "GET {path} from {address} sent more than {MAX_RESPONSE_BYTES} bytes"
                        ),
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(expired());
            }
            Err(error) => return Err(error),
        }
    }
    parse_response(&raw).map_err(|reason| {
        Error::new(
            reason.kind,
            format!("GET {path} from {address}: {}", reason.message),
        )
    })
}

/// The value of the sample whose series is exactly `series`, if the
/// exposition has one.
///
/// `series` is the whole identifier as exposed — the metric name, and its
/// label set exactly as the process prints it when there is one:
/// `qip_edge_journal_pressure{state="normal"}`. Matched whole, never as a
/// prefix: `qip_edge_orders_total` must not read the value of
/// `qip_edge_orders_total_expired`, and a substring match is exactly the bug
/// this repository has already shipped once in a test.
pub(crate) fn sample(exposition: &str, series: &str) -> Option<f64> {
    exposition
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let value = line.strip_prefix(series)?.strip_prefix(' ')?;
            value.split_whitespace().next()?.parse().ok()
        })
}

#[derive(Debug)]
struct Refusal {
    kind: ErrorKind,
    message: String,
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal {
        kind: ErrorKind::InvalidData,
        message: message.into(),
    }
}

fn parse_response(raw: &[u8]) -> Result<String, Refusal> {
    let header_end = find(raw, b"\r\n\r\n")
        .ok_or_else(|| invalid("the response ended before its headers did"))?;
    let head = std::str::from_utf8(&raw[..header_end])
        .map_err(|_| invalid("the response headers are not UTF-8"))?;
    let body = &raw[header_end + 4..];

    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| invalid(format!("no status code in {status_line:?}")))?;
    if status != "200" {
        let preview = String::from_utf8_lossy(&body[..body.len().min(512)]);
        return Err(Refusal {
            kind: ErrorKind::Other,
            message: format!("answered {status_line:?}, not 200: {preview}"),
        });
    }

    let mut chunked = false;
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value
                .split(',')
                .any(|coding| coding.trim().eq_ignore_ascii_case("chunked"));
        } else if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| invalid(format!("content-length {value:?} is not a number")))?,
            );
        }
    }

    let decoded = if chunked {
        decode_chunked(body)?
    } else if let Some(length) = content_length {
        if body.len() < length {
            return Err(invalid(format!(
                "the body is {} bytes of a declared {length}",
                body.len()
            )));
        }
        body[..length].to_vec()
    } else {
        body.to_vec()
    };
    String::from_utf8(decoded).map_err(|_| invalid("the body is not UTF-8"))
}

/// Undo `transfer-encoding: chunked`. Chunk extensions and trailers are read
/// past and dropped; nothing these binaries serve uses either.
fn decode_chunked(mut rest: &[u8]) -> Result<Vec<u8>, Refusal> {
    let mut decoded = Vec::new();
    loop {
        let line_end =
            find(rest, b"\r\n").ok_or_else(|| invalid("a chunk-size line never ended"))?;
        let size_line = std::str::from_utf8(&rest[..line_end])
            .map_err(|_| invalid("a chunk-size line is not UTF-8"))?;
        let size_hex = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| invalid(format!("chunk size {size_hex:?} is not hexadecimal")))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        let chunk_end = size
            .checked_add(2)
            .filter(|end| *end <= rest.len())
            .ok_or_else(|| invalid(format!("a {size}-byte chunk was cut short")))?;
        if &rest[size..chunk_end] != b"\r\n" {
            return Err(invalid(format!(
                "a {size}-byte chunk is not followed by CRLF"
            )));
        }
        decoded.extend_from_slice(&rest[..size]);
        rest = &rest[chunk_end..];
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
