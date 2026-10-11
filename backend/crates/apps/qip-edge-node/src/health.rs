//! The health and scrape surface, on a thread of its own.
//!
//! Until this module the node accepted each probe on the thread that ran
//! `Cell::work`, and did its halt poll, journal flush, mesh exchange and the
//! pass itself *because* a probe had arrived: a node nobody probed never
//! decided, and a node probed by a slow client decided no faster than that
//! client sent its request. The read was bounded to two seconds, which
//! bounded the stall without removing it; and rendering a scrape — a walk of
//! every series in the registry — ran on the same thread too.
//!
//! The decision loop now runs on a timer (`scheduler`), and this thread only
//! answers. It reads the body the decision thread last published into a
//! [`HealthSnapshot`] and renders a scrape from the shared registry. The
//! decision thread publishes with `try_lock`, so a probe holding the slot
//! costs it one skipped publication and never a wait; this thread takes the
//! lock only long enough to clone a string.

use crate::telemetry::respond;
use qip_core::error::{Error, Result};
use qip_observability::metrics::Metrics;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

/// The health thread's name.
pub const HEALTH_THREAD: &str = "qip-health";

/// The body a probe reads: the decision thread's last publication.
#[derive(Clone, Debug)]
pub struct HealthSnapshot {
    body: Arc<Mutex<String>>,
}

impl HealthSnapshot {
    /// A snapshot holding `initial` until the first publication.
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            body: Arc::new(Mutex::new(initial.into())),
        }
    }

    /// Replace the body without waiting. `false` when a probe held the slot
    /// at this instant; the next turn publishes a newer body anyway.
    pub fn publish(&self, body: String) -> bool {
        match self.body.try_lock() {
            Ok(mut slot) => {
                *slot = body;
                true
            }
            Err(TryLockError::Poisoned(poisoned)) => {
                *poisoned.into_inner() = body;
                true
            }
            Err(TryLockError::WouldBlock) => false,
        }
    }

    /// The body as last published. For the health thread, which may wait.
    pub fn read(&self) -> String {
        self.body
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Serve `listener` on its own thread until the listener fails.
pub fn spawn(
    listener: TcpListener,
    metrics: Arc<Metrics>,
    snapshot: HealthSnapshot,
    timeout: Duration,
) -> Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(HEALTH_THREAD.to_string())
        .spawn(move || {
            for incoming in listener.incoming() {
                match incoming {
                    Ok(stream) => {
                        if let Err(error) = answer(stream, &metrics, &snapshot, timeout) {
                            eprintln!("qip-edge-node: health request failed: {}", error.message());
                        }
                    }
                    Err(error) => eprintln!("qip-edge-node: accept failed: {error}"),
                }
            }
        })
        .map_err(|error| Error::io(format!("cannot start the {HEALTH_THREAD} thread: {error}")))
}

/// Answer one request from the snapshot and the registry.
pub fn answer(
    mut stream: TcpStream,
    metrics: &Metrics,
    snapshot: &HealthSnapshot,
    timeout: Duration,
) -> Result<()> {
    bound(&stream, timeout)?;
    let mut buffer = [0u8; 1024];
    // The read has to happen or the client sees a reset instead of a
    // response, and it is what tells a scrape from a probe.
    let read = stream.read(&mut buffer).unwrap_or(0);
    let body = snapshot.read();
    let (content_type, payload) = respond(&buffer[..read], metrics, &body);
    write_response(stream, content_type, &payload)
}

/// Put a deadline on both halves of one health exchange.
///
/// A held socket no longer stops the cell deciding, but it still stops
/// every later probe on this single-threaded server, which reads as a dead
/// node to the orchestrator; so it stays bounded.
pub fn bound(stream: &TcpStream, timeout: Duration) -> Result<()> {
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|error| Error::io(format!("cannot bound the health request: {error}")))
}

fn write_response(mut stream: TcpStream, content_type: &str, body: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| Error::io(format!("cannot write the health response: {error}")))?;
    stream
        .flush()
        .map_err(|error| Error::io(format!("cannot flush the health response: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// The failure this prevents: a client that connects to the health port
    /// and never sends a byte holding the health thread for ever, so every
    /// later probe times out and the orchestrator restarts a node that was
    /// deciding perfectly well. Moved here with `bound` from `main.rs`,
    /// where it guarded the decision thread itself until the decision loop
    /// left this thread.
    ///
    /// The read runs on its own thread and reports through a channel so that
    /// a regression fails on a deadline instead of hanging the test binary.
    #[test]
    fn a_client_that_sends_nothing_does_not_hold_the_health_thread() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let address = listener.local_addr().expect("the listener has an address");
        let _silent_client = TcpStream::connect(address).expect("connect to the listener");

        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the client connected");
            bound(&stream, Duration::from_millis(50)).expect("the socket can be bound");
            let mut buffer = [0u8; 16];
            let outcome = (&stream).read(&mut buffer).map_err(|error| error.kind());
            let _ = sender.send(outcome);
        });

        let outcome = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the read from a silent client never returned: the health socket is unbounded");
        // The premise as well as the property: an `Ok` here would mean the
        // client sent something after all.
        let kind = outcome.expect_err("a client that sent nothing produced readable bytes");
        assert!(
            matches!(
                kind,
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "the read failed for a reason that is not the deadline: {kind:?}"
        );
        reader.join().expect("the reading thread finished");
    }

    /// A probe holding the snapshot must cost the decision thread a skipped
    /// publication, never a wait: `publish` is the decision thread's call.
    #[test]
    fn publishing_while_a_probe_holds_the_snapshot_returns_at_once() {
        let snapshot = HealthSnapshot::new("{}");
        let held = Arc::clone(&snapshot.body);
        let guard = held.lock().expect("the slot is free");
        let started = std::time::Instant::now();
        let published = snapshot.publish("{\"turns\":1}".to_string());
        let waited = started.elapsed();
        drop(guard);
        assert!(
            !published,
            "the slot was held, so nothing can have been published"
        );
        assert!(
            waited < Duration::from_millis(100),
            "publish waited {waited:?} on a held slot"
        );
        assert!(snapshot.publish("{\"turns\":2}".to_string()));
        assert_eq!(snapshot.read(), "{\"turns\":2}");
    }
}
