//! The central plane's end of the mesh spine, against a real socket.
//!
//! Two claims are under test and both are about failure rather than about the
//! happy path:
//!
//! * **A capital instruction is not lost.** It is persisted before any attempt
//!   to send it and removed only once the cell has acknowledged it, so a
//!   process that dies in between re-sends rather than forgets. The restart is
//!   simulated the way `qip-transport`'s own spool tests simulate one — the
//!   dispatcher is *dropped* and a new one is opened over the same store —
//!   because a spool tested without a restart is a spool whose single claim is
//!   the one thing untested.
//! * **A delta delivered twice is absorbed once.** The inbox detects a
//!   redelivery only while it still remembers the key; past that window the
//!   consumer's own idempotency is all there is, and that is the state these
//!   tests put it in deliberately.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Clock, CorrelationId, Decimal, Duration, Id, Lineage, ManualClock, Timestamp};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_mesh::spine::{
    CELL_DELTA_TOPIC, CapitalDispatch, CapitalDispatcher, CellDeltaReceiver, CellDeltaSink,
    DispatcherConfig, HeldReason,
};
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_transport::breaker::{BreakerPolicy, BreakerState};
use qip_transport::{
    ClientLimits, MemoryDeadLetters, MeshConfig, MeshEndpoint, MeshInbox, MeshPublisher, Method,
    RecordingSleeper, RetryPolicy, Sleeper,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const CELL: &str = "london-1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

// --- a loopback server ------------------------------------------------------

/// What the server does with a request.
enum Behaviour {
    /// Hand it to a real mesh endpoint.
    Endpoint(Box<MeshEndpoint>),
    /// Answer this status and nothing else, whatever was asked.
    Always(u16),
}

struct MeshServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MeshServer {
    fn spawn(behaviour: Behaviour) -> Result<Self> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|error| Error::io(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| Error::io(error.to_string()))?
            .to_string();
        listener
            .set_nonblocking(true)
            .map_err(|error| Error::io(error.to_string()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicUsize::new(0));
        let thread_stop = Arc::clone(&stop);
        let thread_served = Arc::clone(&served);
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        thread_served.fetch_add(1, Ordering::SeqCst);
                        serve_one(stream, &behaviour);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            stop,
            served,
            handle: Some(handle),
        })
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

impl Drop for MeshServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_one(mut stream: TcpStream, behaviour: &Behaviour) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let Some(method) = parts.next().and_then(Method::parse) else {
        return;
    };
    let Some(target) = parts.next().map(str::to_string) else {
        return;
    };

    let mut length = 0usize;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let (status, payload) = match behaviour {
        Behaviour::Endpoint(endpoint) => {
            let response = endpoint.handle(method, &target, &body);
            (response.status, response.body)
        }
        Behaviour::Always(status) => (*status, b"{\"error\":\"refused\"}".to_vec()),
    };
    let mut out = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        status,
        qip_transport::EndpointResponse::CONTENT_TYPE,
        payload.len()
    )
    .into_bytes();
    out.extend_from_slice(&payload);
    let _ = stream.write_all(&out);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// An address nothing is listening on: bound, then dropped.
fn dead_peer() -> Result<String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|error| Error::io(error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| Error::io(error.to_string()))?
        .to_string();
    drop(listener);
    Ok(format!("http://{address}"))
}

// --- fixtures ---------------------------------------------------------------

fn ladder() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 2,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(4),
        multiplier: 2,
        jitter_basis_points: 0,
    }
}

fn mesh_config(name: &str, peer: &str) -> MeshConfig {
    MeshConfig::new(name, peer)
        .with_retry(ladder())
        .with_limits(ClientLimits {
            read_timeout: std::time::Duration::from_millis(500),
            connect_timeout: std::time::Duration::from_millis(500),
            ..ClientLimits::default()
        })
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(t(0)))
}

fn sleeper() -> Arc<dyn Sleeper> {
    Arc::new(RecordingSleeper::new())
}

/// The disk that outlives the process.
fn disk() -> Arc<dyn KeyValueStore> {
    Arc::new(MemoryKeyValueStore::new())
}

fn dispatcher(peer: &str, store: Arc<dyn KeyValueStore>) -> Result<CapitalDispatcher> {
    CapitalDispatcher::open(
        DispatcherConfig::new(CELL, mesh_config("capital:london-1", peer)),
        store,
        clock(),
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )
}

/// An envelope. The signature is opaque here: what this crate does with a grant
/// is carry it, and whether it verifies is the receiving cell's question.
fn envelope(cell: &str, gross: i64, signature: &str) -> Result<CapitalEnvelope> {
    CapitalEnvelope::new(
        StrategyId::new("mean-reversion-1"),
        cell,
        Decimal::from_int(gross),
        Decimal::from_int(400),
        Decimal::from_int(50_000),
        vec![VenueId::new("XLON")],
        t(0),
        t(7_200),
        "alice@example.com",
        signature,
    )
}

/// What a cell publishes up, from the centre's side of the wire.
///
/// Deliberately a *different type* from `qip_edge::CellStateDelta` carrying the
/// same topic: this crate may not name the edge crate, and the receiver is
/// built so that it does not need to. That the centre can absorb, deduplicate
/// and account for a delta it never decodes is the property, and using a stand-in
/// body here is how the test asserts it rather than assuming it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct OpaqueDelta {
    cell: String,
    sequence: u64,
}

impl EventBody for OpaqueDelta {
    const TOPIC: Topic = CELL_DELTA_TOPIC;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.cell, self.sequence))
    }
}

fn delta_frame(sequence: u64, event_id: &str, at: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(event_id.to_string()),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "qip-mesh-tests",
        ),
        OpaqueDelta {
            cell: CELL.to_string(),
            sequence,
        },
    )
    .erase()
}

/// A frame that is not a cell delta at all.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Heartbeat {
    sequence: u64,
}

impl EventBody for Heartbeat {
    const TOPIC: Topic = Topic::ServiceStarted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("heartbeat:{}", self.sequence))
    }
}

fn heartbeat_frame(sequence: u64, at: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(format!("EVT-HEARTBEAT-{sequence}")),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR-HEARTBEAT-{sequence}")),
            "qip-mesh-tests",
        ),
        Heartbeat { sequence },
    )
    .erase()
}

fn cell_publisher(peer: &str) -> Result<MeshPublisher> {
    MeshPublisher::new(
        mesh_config("uplink:london-1", peer),
        clock(),
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )
}

/// A sink that counts what it absorbed and can be told to refuse one frame.
#[derive(Debug, Default)]
struct CountingSink {
    absorbed: Vec<String>,
    refuse: Option<String>,
}

impl CellDeltaSink for CountingSink {
    fn absorb(&mut self, frame: &AnyEvent) -> Result<()> {
        let key = frame.dedup_key();
        if self.refuse.as_ref() == Some(&key) {
            return Err(Error::unavailable(
                "the central plane cannot absorb this delta right now",
            ));
        }
        self.absorbed.push(key);
        Ok(())
    }
}

// --- down: the capital path -------------------------------------------------

#[test]
fn an_envelope_is_committed_only_once_the_cell_has_acknowledged_it() -> Result<()> {
    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let server = MeshServer::spawn(Behaviour::Endpoint(Box::new(MeshEndpoint::new(
        inbox.clone(),
    ))))?;
    let store = disk();
    let mut dispatch = dispatcher(&server.url(), Arc::clone(&store))?;

    let outcome = dispatch.dispatch(envelope(CELL, 1_000, "sig-one")?, t(10))?;
    assert!(
        outcome.is_delivered(),
        "the envelope did not reach the cell: {outcome:?}"
    );
    assert_eq!(
        dispatch.pending()?,
        0,
        "an acknowledged envelope was left in the spool, so a restart would send it again"
    );
    assert_eq!(dispatch.spool_stats().committed, 1);

    // And the cell received the grant itself, not a wrapper around it.
    let response = inbox.read(0, Timestamp::MAX, 16);
    assert_eq!(response.frames.len(), 1);
    let carried: CapitalEnvelope = serde_json::from_value(response.frames[0].frame.payload.clone())
        .map_err(|error| Error::schema(error.to_string()))?;
    assert_eq!(carried.cell(), CELL);
    assert_eq!(carried.signature(), "sig-one");
    Ok(())
}

#[test]
fn an_envelope_to_a_cell_that_is_down_stays_in_the_spool_rather_than_being_lost() -> Result<()> {
    // The whole reason the capital path is spooled and the delta path is not. A
    // delta that never arrives is superseded by the next one; a grant that
    // never arrives is authority the cell simply does not have, and nobody
    // finds out until it goes quiet.
    let store = disk();
    let mut dispatch = dispatcher(&dead_peer()?, Arc::clone(&store))?;

    let outcome = dispatch.dispatch(envelope(CELL, 1_000, "sig-one")?, t(10))?;
    match &outcome {
        CapitalDispatch::Held {
            reason: HeldReason::Undelivered { attempts, .. },
            ..
        } => assert_eq!(
            *attempts, 2,
            "the ladder spent the wrong number of attempts"
        ),
        other => panic!("an unreachable cell produced {other:?}"),
    }
    assert_eq!(
        dispatch.pending()?,
        1,
        "an undelivered capital instruction was dropped"
    );
    let backlog = dispatch.backlog()?;
    assert_eq!(
        backlog[0].attempts, 1,
        "the attempt was not persisted, so a restart would reset the retry budget"
    );
    assert!(backlog[0].last_error.is_some());
    Ok(())
}

#[test]
fn a_spooled_envelope_survives_a_restart_and_is_delivered_when_the_cell_returns() -> Result<()> {
    // The restart is a *drop*. A spool tested without one proves nothing about
    // the only claim it makes.
    let store = disk();
    let held = {
        let mut dispatch = dispatcher(&dead_peer()?, Arc::clone(&store))?;
        dispatch.dispatch(envelope(CELL, 1_000, "sig-one")?, t(10))?;
        dispatch.dispatch(envelope(CELL, 2_000, "sig-two")?, t(11))?;
        dispatch.pending()?
    }; // <- the pod dies here.
    assert_eq!(held, 2, "the envelopes were not persisted before sending");

    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let server = MeshServer::spawn(Behaviour::Endpoint(Box::new(MeshEndpoint::new(
        inbox.clone(),
    ))))?;
    let mut replacement = dispatcher(&server.url(), Arc::clone(&store))?;
    assert_eq!(
        replacement.spool_stats().recovered,
        2,
        "the replacement did not inherit the previous process's unfinished work"
    );

    let report = replacement.recover(t(60))?;
    assert_eq!(
        report.delivered(),
        2,
        "the backlog was not sent: {report:?}"
    );
    assert_eq!(report.remaining, 0);
    assert_eq!(
        inbox.depth(),
        2,
        "the cell did not receive the grants that outlived the process"
    );

    // In order, because capital instructions to one cell are ordered and a
    // later grant may be a narrower replacement for an earlier one.
    let response = inbox.read(0, Timestamp::MAX, 16);
    let signatures: Vec<String> = response
        .frames
        .iter()
        .filter_map(|entry| {
            serde_json::from_value::<CapitalEnvelope>(entry.frame.payload.clone())
                .ok()
                .map(|grant| grant.signature().to_string())
        })
        .collect();
    assert_eq!(signatures, vec!["sig-one", "sig-two"]);
    Ok(())
}

#[test]
fn a_second_envelope_to_a_down_cell_is_held_by_the_circuit_without_spending_a_ladder() -> Result<()>
{
    let store = disk();
    let mut dispatch = CapitalDispatcher::open(
        DispatcherConfig::new(CELL, mesh_config("capital:london-1", &dead_peer()?)).with_breaker(
            BreakerPolicy {
                failure_threshold: 1,
                ..BreakerPolicy::default()
            },
            11,
        ),
        Arc::clone(&store),
        clock(),
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )?;

    dispatch.dispatch(envelope(CELL, 1_000, "sig-one")?, t(10))?;
    let after_first = dispatch.publisher().stats().attempts;
    assert_eq!(after_first, 2);
    assert_eq!(dispatch.circuit(), BreakerState::Open);

    let second = dispatch.dispatch(envelope(CELL, 2_000, "sig-two")?, t(11))?;
    assert!(
        matches!(
            second,
            CapitalDispatch::Held {
                reason: HeldReason::CircuitOpen(_),
                ..
            }
        ),
        "the second envelope went to the network anyway: {second:?}"
    );
    assert_eq!(
        dispatch.publisher().stats().attempts,
        after_first,
        "the circuit was open and the transport tried anyway"
    );
    // Refused by the circuit and still persisted: nothing about the breaker
    // makes a capital instruction disappear.
    assert_eq!(dispatch.pending()?, 2);
    let backlog = dispatch.backlog()?;
    assert_eq!(
        backlog[1].attempts, 0,
        "a call the circuit never made was counted against the entry's retry budget"
    );
    Ok(())
}

#[test]
fn an_envelope_the_cell_refuses_is_released_so_it_cannot_block_the_ones_behind_it() -> Result<()> {
    // The one escape from head-of-line blocking, and the reason it is safe: the
    // peer *answered*. The frame and the reason are in the dead-letter sink,
    // where an operator reads them, so releasing the spool entry loses no
    // record — and keeping it would mean one envelope the cell will never
    // accept stops every later grant to that cell, permanently.
    let server = MeshServer::spawn(Behaviour::Always(422))?;
    let store = disk();
    let mut dispatch = dispatcher(&server.url(), Arc::clone(&store))?;

    let outcome = dispatch.dispatch(envelope(CELL, 1_000, "sig-one")?, t(10))?;
    assert_eq!(outcome.code(), "rejected", "{outcome:?}");
    assert_eq!(
        dispatch.pending()?,
        0,
        "a refused envelope stayed at the head of the spool"
    );
    assert_eq!(
        dispatch.publisher().dead_letters().len(),
        1,
        "the refusal was released without being recorded anywhere"
    );

    // The next one goes out normally, which is what "does not block" means.
    let second = dispatch.dispatch(envelope(CELL, 2_000, "sig-two")?, t(11))?;
    assert_eq!(second.code(), "rejected");
    assert!(server.served() >= 2);
    Ok(())
}

#[test]
fn an_envelope_for_another_cell_is_refused_before_anything_is_written() -> Result<()> {
    let store = disk();
    let mut dispatch = dispatcher(&dead_peer()?, Arc::clone(&store))?;
    let error = dispatch
        .dispatch(envelope("tokyo-2", 1_000, "sig-one")?, t(10))
        .expect_err("a dispatcher for one cell sent another cell's capital");
    assert!(error.message().contains("tokyo-2"), "{}", error.message());
    assert_eq!(
        dispatch.pending()?,
        0,
        "a refused envelope was persisted, so a restart would try to deliver it"
    );
    Ok(())
}

// --- up: cell deltas --------------------------------------------------------

#[test]
fn a_delta_published_by_a_cell_reaches_the_sink_the_centre_supplied() -> Result<()> {
    let mut receiver = CellDeltaReceiver::with_defaults("central", 64)?;
    let server = MeshServer::spawn(Behaviour::Endpoint(Box::new(receiver.endpoint().clone())))?;
    let mut cell = cell_publisher(&server.url())?;

    cell.publish_frame(delta_frame(1, "EVT-D1", t(10))?, t(10))?;
    cell.publish_frame(delta_frame(2, "EVT-D2", t(11))?, t(11))?;

    let mut sink = CountingSink::default();
    let report = receiver.drain(t(60), 16, &mut sink)?;
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.absorbed, 2);
    assert_eq!(
        sink.absorbed,
        vec![
            "position.updated:london-1:1".to_string(),
            "position.updated:london-1:2".to_string()
        ],
        "the deltas did not arrive, or arrived out of the order one publisher sent them"
    );
    assert_eq!(receiver.cursor(), 2);
    Ok(())
}

#[test]
fn the_same_delta_delivered_twice_is_absorbed_once() -> Result<()> {
    // The inbox's own duplicate detection is bounded, so this builds one with a
    // window of a single key and sends an unrelated frame in between: the
    // second copy of the delta is then accepted by the inbox exactly as it
    // would be after a busy hour, and the receiver's own memory is the only
    // thing standing between it and a double count.
    let mut receiver = CellDeltaReceiver::new("central", 64, 1, 256)?;
    let server = MeshServer::spawn(Behaviour::Endpoint(Box::new(receiver.endpoint().clone())))?;
    let mut cell = cell_publisher(&server.url())?;

    cell.publish_frame(delta_frame(1, "EVT-D1", t(10))?, t(10))?;
    cell.publish_frame(heartbeat_frame(1, t(11))?, t(11))?;
    cell.publish_frame(delta_frame(1, "EVT-D1-REDELIVERED", t(12))?, t(12))?;
    assert_eq!(
        receiver.inbox().depth(),
        3,
        "the inbox absorbed the redelivery itself, so this test would prove nothing"
    );

    let mut sink = CountingSink::default();
    let report = receiver.drain(t(60), 16, &mut sink)?;
    assert_eq!(
        report.absorbed, 1,
        "one delta delivered twice produced {} effects",
        report.absorbed
    );
    assert_eq!(
        report.duplicates.len(),
        1,
        "the redelivery was not recognised"
    );
    assert_eq!(
        report.ignored, 1,
        "the frame that was not a delta was absorbed"
    );
    assert_eq!(sink.absorbed.len(), 1);
    assert_eq!(receiver.stats().duplicates, 1);
    Ok(())
}

#[test]
fn a_delta_the_sink_could_not_absorb_stops_the_drain_instead_of_being_skipped() -> Result<()> {
    // Head-of-line blocking, on purpose. A delta the centre could not absorb is
    // a hole in its view of a cell; moving the cursor past it would leave that
    // hole invisible, and aggregate exposure is the one number that has to be
    // right during an incident.
    let mut receiver = CellDeltaReceiver::with_defaults("central", 64)?;
    let server = MeshServer::spawn(Behaviour::Endpoint(Box::new(receiver.endpoint().clone())))?;
    let mut cell = cell_publisher(&server.url())?;

    cell.publish_frame(delta_frame(1, "EVT-D1", t(10))?, t(10))?;
    cell.publish_frame(delta_frame(2, "EVT-D2", t(11))?, t(11))?;
    cell.publish_frame(delta_frame(3, "EVT-D3", t(12))?, t(12))?;

    let mut sink = CountingSink {
        refuse: Some("position.updated:london-1:2".to_string()),
        ..CountingSink::default()
    };
    let report = receiver.drain(t(60), 16, &mut sink)?;
    assert_eq!(report.absorbed, 1);
    let halt = report
        .halted
        .expect("a refused delta did not stop the drain");
    assert_eq!(halt.key, "position.updated:london-1:2");
    assert_eq!(
        receiver.cursor(),
        1,
        "the cursor moved past a delta nobody absorbed"
    );

    // Once the sink can take it, the drain resumes at the frame it stopped on
    // rather than after it.
    sink.refuse = None;
    let resumed = receiver.drain(t(60), 16, &mut sink)?;
    assert!(resumed.is_clean(), "{resumed:?}");
    assert_eq!(resumed.absorbed, 2);
    assert_eq!(
        sink.absorbed,
        vec![
            "position.updated:london-1:1".to_string(),
            "position.updated:london-1:2".to_string(),
            "position.updated:london-1:3".to_string()
        ]
    );
    Ok(())
}
