//! The cell's end of the mesh spine, against a real socket.
//!
//! Every test here runs an actual `MeshEndpoint` on an actual `TcpListener` and
//! makes the cell talk to it over HTTP/1.1. A mesh tested by handing frames
//! between two objects in one process proves the frames are well typed and
//! nothing about the wire: it never discovers a connection refused, a peer that
//! answers and dies, or a retry ladder spent on a socket nobody is listening
//! on, and those are the failures the transport exists for.
//!
//! The properties under test, in the order they matter:
//!
//! * a delta the cell produced arrives at the centre unchanged,
//! * an envelope that does not verify is refused *even though it arrived*,
//! * the same grant delivered twice renews the cell's capital once,
//! * a peer that is down dead-letters rather than blocking forever,
//! * and the second message to that peer costs no ladder at all.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{
    Clock, CorrelationId, Decimal, Duration, Id, Lineage, ManualClock, ObjectId, Timestamp, dec,
};
use qip_edge::cell::{Cell, CellConfig, PlacedOrder, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{
    CapitalDownlink, CapitalGrantTopic, CellStateDelta, CellUplink, Dispatch, DownlinkConfig,
    UplinkConfig,
};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
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

const KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const OTHER_KEY: &[u8] = b"a-key-this-cell-has-never-held";
const CELL: &str = "london-1";
const REGION: &str = "eu-west";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

// --- a loopback server that serves a real mesh endpoint ---------------------

/// A mesh endpoint on a real port, in its own thread.
struct MeshServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MeshServer {
    fn spawn(endpoint: MeshEndpoint) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| qip_core::Error::io(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| qip_core::Error::io(error.to_string()))?
            .to_string();
        listener
            .set_nonblocking(true)
            .map_err(|error| qip_core::Error::io(error.to_string()))?;

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
                        serve_one(stream, &endpoint);
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

fn serve_one(mut stream: TcpStream, endpoint: &MeshEndpoint) {
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

    let response = endpoint.handle(method, &target, &body);
    let mut out = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.status,
        qip_transport::EndpointResponse::CONTENT_TYPE,
        response.body.len()
    )
    .into_bytes();
    out.extend_from_slice(&response.body);
    let _ = stream.write_all(&out);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// An address nothing is listening on.
///
/// Bound and immediately dropped, so the port is real, free, and refuses.
fn dead_peer() -> Result<String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|error| qip_core::Error::io(error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| qip_core::Error::io(error.to_string()))?
        .to_string();
    drop(listener);
    Ok(format!("http://{address}"))
}

// --- fixtures ---------------------------------------------------------------

fn ladder() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
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

fn uplink(peer: &str) -> Result<CellUplink> {
    CellUplink::connect(
        UplinkConfig::new(CELL, REGION, mesh_config("uplink:london-1", peer)),
        clock(),
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )
}

fn downlink(peer: &str, key: &[u8]) -> Result<CapitalDownlink> {
    CapitalDownlink::connect(
        DownlinkConfig::new(CELL, mesh_config("downlink:london-1", peer)),
        key,
        clock(),
        sleeper(),
    )
}

/// An envelope signed the way the central allocator signs one.
fn signed_envelope(cell: &str, gross: &str, key: &[u8], expires: i64) -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("mean-reversion-1"),
            cell,
            Decimal::parse(gross).unwrap_or(Decimal::ZERO),
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(expires),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    build(&sign_payload(key, &unsigned.signing_payload()))
}

/// A strategy that compiles and never fires.
///
/// What is under test is what crosses the wire and what is refused on arrival,
/// not what the strategy computes, so the cheapest well-typed program is the
/// honest fixture.
fn trivial_strategy() -> Result<(
    qip_strategy::compile::CompiledStrategy,
    qip_strategy::program::Program,
)> {
    use qip_contracts::signal::SignalKind;
    use qip_strategy::catalogue::FeatureCatalogue;
    use qip_strategy::compile::StrategyCompiler;
    use qip_strategy::ir::{Expr, Rule, StrategySpec};

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(
        StrategyId::new("mean-reversion-1"),
        object("ACME"),
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "never",
        SignalKind::Enter,
        Expr::Flag(false),
        Expr::Exact(dec!("1")),
        Expr::Statistic(0.5),
        100,
    ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A cell with one strategy deployed under a grant that expires at `expires`.
fn deployed_cell(expires: i64) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new("XLON"));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    let (strategy, program) = trivial_strategy()?;
    let grant = VerifiedEnvelope::verify(
        signed_envelope(CELL, "1000", KEY, expires)?,
        KEY,
        CELL,
        t(10),
    )?;
    cell.deploy(strategy, program, grant)?;
    Ok(cell)
}

/// The centre's side of a grant, as the cell's downlink expects to find it.
///
/// Written here rather than imported from `qip-mesh`: that crate is a service
/// and this one is an edge library, so the two ends cannot share a type. What
/// they share is the vocabulary — the payload *is* a `CapitalEnvelope` — and
/// the topic constant the cell publishes for exactly this purpose. That this
/// hand-written sender is understood by the downlink is the contract being
/// tested.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct GrantFrame(CapitalEnvelope);

impl EventBody for GrantFrame {
    const TOPIC: Topic = CapitalGrantTopic::TOPIC;
    const SCHEMA_VERSION: u32 = CapitalGrantTopic::SCHEMA_VERSION;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}|{}|{}",
            self.0.cell(),
            self.0.strategy().as_str(),
            self.0.signature()
        ))
    }
}

/// Frame a grant, with an event id the caller chooses.
///
/// The id is a parameter so a test can send the *same grant* under two
/// different frame identities, which is what a redelivery past the transport's
/// own dedup window looks like from the cell's side.
fn grant_frame(envelope: &CapitalEnvelope, event_id: &str, at: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(event_id.to_string()),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "qip-edge-tests",
        ),
        GrantFrame(envelope.clone()),
    )
    .erase()
}

/// An unrelated frame the centre puts on the same inbox.
///
/// It exists for one purpose: to roll the inbox's own duplicate-detection
/// window past a grant, so a redelivery of that grant actually reaches the
/// cell. The transport documents that window as bounded and says that past it
/// "the consumer's own idempotency is the only thing left" — this is how a test
/// gets to that state deliberately instead of waiting for a busy day.
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
            "qip-edge-tests",
        ),
        Heartbeat { sequence },
    )
    .erase()
}

/// A publisher standing in for the central plane.
fn centre_publisher(peer: &str) -> Result<MeshPublisher> {
    MeshPublisher::new(
        mesh_config("central-plane", peer),
        clock(),
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )
}

// --- up: state deltas -------------------------------------------------------

/// The cell's half of the same agreement. Both ends assert it, because either
/// one reacquiring a literal of its own is the failure, and a test that lives
/// only at the other end cannot see it happen here.
#[test]
fn the_cell_states_its_delta_schema_from_the_shared_constant() {
    assert_eq!(
        <CellStateDelta as qip_events::EventBody>::SCHEMA_VERSION,
        qip_contracts::wire::CELL_DELTA_SCHEMA_VERSION,
        "the cell's delta schema version is no longer the shared one, so it \
         can drift from the centre's"
    );
}

#[test]
fn a_state_delta_a_cell_produced_arrives_at_the_centre_unchanged() -> Result<()> {
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()))?;

    let mut cell = deployed_cell(3_600)?;
    cell.autonomy_mut().kill_switch_mut().trip_global(
        t(15),
        "test",
        "so the delta carries a halt somebody has to see",
    );
    let report = WorkReport {
        orders: vec![PlacedOrder {
            order_id: "london-1-1".to_string(),
            strategy: StrategyId::new("mean-reversion-1"),
            // One contributor: this fixture is a single strategy's order, and
            // the vector says so rather than being left empty, which would
            // describe an order nobody asked for.
            contributors: vec![qip_contracts::intent::Contributor {
                strategy: StrategyId::new("mean-reversion-1"),
                signed_size: dec!("-3"),
                // A real revision pair rather than an empty vector: the delta
                // now carries these, and a fixture that ships nothing would
                // let a wire that dropped them still pass.
                inputs: vec![("book_pressure{levels=5}".to_string(), 7)],
            }],
            object_id: object("ACME"),
            venue: VenueId::new("XLON"),
            side: BookSide::Ask,
            quantity: dec!("3"),
            price: dec!("101.5"),
            simulated: true,
        }],
        refusals: vec![("capital".to_string(), "the envelope refused it".to_string())],
        // A booked cross, so the uplink is asked to carry the one record
        // §27.1 calls a regulatory expectation. It went untested at first: a
        // delta built by hand proved the wire shape and proved nothing about
        // `state_delta`, which is the function that actually fills it in.
        crosses: vec![qip_edge::cell::InternalCross {
            object_id: object("ACME"),
            venue: VenueId::new("XLON"),
            quantity: dec!("2"),
            price: dec!("100.25"),
            bought: vec![StrategyId::new("mean-reversion-1")],
            sold: vec![StrategyId::new("momentum-2")],
        }],
        ..WorkReport::default()
    };
    let delta = cell.state_delta(&report, t(20));

    let mut link = uplink(&server.url())?;
    let dispatch = link.publish(delta.clone(), t(20))?;
    assert!(
        dispatch.is_delivered(),
        "the delta did not reach the centre: {dispatch:?}"
    );

    let response = inbox.read(0, Timestamp::MAX, 16);
    assert_eq!(response.frames.len(), 1, "the centre received nothing");
    let received = response.frames[0].frame.decode::<CellStateDelta>()?.body;

    assert_eq!(received.cell, CELL);
    assert_eq!(received.region, REGION);
    assert_eq!(
        received.sequence, 1,
        "the uplink owns the numbering, and it starts at one"
    );
    assert!(
        received.halted,
        "a halted cell that reports itself running is the worst possible delta"
    );
    assert_eq!(received.orders.len(), 1);
    assert!(
        received.orders[0].simulated,
        "a paper fill that crossed the wire as real is the one bit that must not flip"
    );
    assert_eq!(received.orders[0].quantity, dec!("3"));
    // The contributor vector, which the fixture above ships deliberately. It
    // had no assertion for one commit: the fixture carried a real revision
    // pair under a comment claiming that a wire dropping them would be caught,
    // and zeroing `contributors` in `state_delta` left this suite green. A
    // fixture is not a test.
    assert_eq!(
        received.orders[0].contributors.len(),
        1,
        "the order arrived naming no contributor, so a netted fill could not \
         be traced to the strategies that caused it"
    );
    assert_eq!(
        received.orders[0].contributors[0].strategy.as_str(),
        "mean-reversion-1"
    );
    assert_eq!(received.orders[0].contributors[0].signed_size, dec!("-3"));
    assert_eq!(
        received.orders[0].contributors[0].inputs,
        vec![("book_pressure{levels=5}".to_string(), 7)],
        "the feature revisions did not survive the wire, so the fill cannot be \
         attributed to the values that produced it"
    );
    assert_eq!(
        received.refusals.len(),
        1,
        "the centre must hear the refusals too"
    );
    assert_eq!(
        received.utilisation.len(),
        1,
        "the delta did not carry what the deployed strategy has committed"
    );
    assert_eq!(
        received.utilisation[0].envelope_expires_at,
        t(3_600),
        "the centre cannot see which cells are about to run out of authority"
    );
    assert_eq!(
        received.crosses.len(),
        1,
        "the cross stopped at the cell, so the centre cannot see a trade the \
         platform made with itself"
    );
    assert_eq!(received.crosses[0].quantity, dec!("2"));
    assert_eq!(
        received.crosses[0].price,
        dec!("100.25"),
        "the crossing price did not survive, and a cross without its price is \
         not a ledger entry"
    );
    assert_eq!(
        received.crosses[0].bought[0].as_str(),
        "mean-reversion-1",
        "the buying side was not carried"
    );
    assert_eq!(
        received.crosses[0].sold[0].as_str(),
        "momentum-2",
        "the selling side was not carried"
    );
    Ok(())
}

#[test]
fn a_delta_for_a_peer_that_is_not_listening_is_dead_lettered_rather_than_blocking_forever()
-> Result<()> {
    // The failure the retry ladder bounds. Without it this call does not
    // return: it discovers a refused connection and tries again, and the cell's
    // work loop is the thread doing the waiting.
    let mut link = uplink(&dead_peer()?)?;
    let cell = deployed_cell(3_600)?;
    let delta = cell.state_delta(&WorkReport::default(), t(20));

    let dispatch = link.publish(delta, t(20))?;
    match dispatch {
        Dispatch::DeadLettered { attempts, .. } => assert_eq!(
            attempts, 3,
            "the ladder spent a different number of attempts than the policy allows"
        ),
        other => panic!("an unreachable peer produced {other:?}"),
    }
    assert_eq!(
        link.publisher().dead_letters().len(),
        1,
        "the frame was not recorded anywhere, so nobody can say what never arrived"
    );
    assert_eq!(link.stats().dead_lettered, 1);
    Ok(())
}

#[test]
fn a_second_delta_to_a_peer_that_is_down_is_refused_by_the_circuit_without_spending_a_ladder()
-> Result<()> {
    // The property the breaker exists for, stated as a number: the second
    // message costs *zero* attempts. Without it, a thousand queued deltas
    // become a thousand ladders against a socket nobody is listening on, and
    // the thread spends an hour and a half discovering one fact.
    let mut link = CellUplink::connect(
        UplinkConfig::new(CELL, REGION, mesh_config("uplink:london-1", &dead_peer()?))
            .with_breaker(
                BreakerPolicy {
                    failure_threshold: 1,
                    ..BreakerPolicy::default()
                },
                7,
            ),
        clock(),
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )?;
    let cell = deployed_cell(3_600)?;

    let first = link.publish(cell.state_delta(&WorkReport::default(), t(20)), t(20))?;
    assert_eq!(first.code(), "dead_lettered");
    let after_first = link.publisher().stats().attempts;
    assert_eq!(after_first, 3, "the first message spends the whole ladder");
    assert_eq!(link.circuit(), BreakerState::Open);

    let second = link.publish(cell.state_delta(&WorkReport::default(), t(21)), t(21))?;
    assert_eq!(
        second.code(),
        "circuit_open",
        "the second message went to the network anyway: {second:?}"
    );
    assert_eq!(
        link.publisher().stats().attempts,
        after_first,
        "the circuit was open and the transport tried anyway"
    );
    assert_eq!(link.stats().circuit_refusals, 1);
    Ok(())
}

// --- down: signed capital envelopes -----------------------------------------

#[test]
fn a_capital_envelope_that_does_not_verify_is_refused_even_though_it_arrived_over_the_mesh()
-> Result<()> {
    // Delivery is not approval. The transport authenticates nobody, so anything
    // that can route a packet to the peer can put an envelope in this inbox;
    // the signature is what says the centre issued it, and it is checked here.
    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox))?;
    let mut centre = centre_publisher(&server.url())?;

    // Signed with a key this cell has never held.
    let forged = signed_envelope(CELL, "9000000", OTHER_KEY, 3_600)?;
    centre.publish_frame(grant_frame(&forged, "EVT-FORGED", t(11))?, t(11))?;
    // Genuine, and for another cell entirely — the replay a signature alone
    // does not stop.
    let elsewhere = signed_envelope("tokyo-2", "1000", KEY, 3_600)?;
    centre.publish_frame(grant_frame(&elsewhere, "EVT-ELSEWHERE", t(12))?, t(12))?;
    // Genuine, for this cell, and expired before the cell's clock reached it.
    let expired = signed_envelope(CELL, "1000", KEY, 30)?;
    centre.publish_frame(grant_frame(&expired, "EVT-EXPIRED", t(13))?, t(13))?;

    let mut link = downlink(&server.url(), KEY)?;
    let batch = link.poll(t(60))?;

    assert!(
        batch.verified.is_empty(),
        "an envelope nobody could verify was handed to the cell: {:?}",
        batch.verified
    );
    assert_eq!(
        batch.refused.len(),
        3,
        "three unverifiable envelopes produced {} refusals",
        batch.refused.len()
    );
    assert!(
        batch.refused[0].reason.contains("does not verify"),
        "the forgery was refused for the wrong reason: {}",
        batch.refused[0].reason
    );
    assert!(
        batch.refused[1].reason.contains("cell"),
        "the replay was refused for the wrong reason: {}",
        batch.refused[1].reason
    );
    assert!(
        batch.refused[2].reason.contains("validity window"),
        "the expired grant was refused for the wrong reason: {}",
        batch.refused[2].reason
    );
    assert_eq!(link.stats().refused, 3);
    assert_eq!(link.stats().verified, 0);
    Ok(())
}

#[test]
fn a_genuine_envelope_crosses_the_mesh_and_renews_the_cells_capital() -> Result<()> {
    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox))?;
    let mut centre = centre_publisher(&server.url())?;

    let renewal = signed_envelope(CELL, "2000", KEY, 7_200)?;
    centre.publish_frame(grant_frame(&renewal, "EVT-RENEWAL", t(30))?, t(30))?;

    let mut cell = deployed_cell(3_600)?;
    let mut link = downlink(&server.url(), KEY)?;
    let batch = link.poll(t(60))?;
    assert_eq!(batch.verified.len(), 1, "the renewal did not arrive");

    for envelope in batch.verified {
        cell.renew_capital(envelope, t(60))?;
    }
    let delta = cell.state_delta(&WorkReport::default(), t(61));
    assert_eq!(
        delta.utilisation[0].envelope_expires_at,
        t(7_200),
        "the cell is still running on the envelope it started with"
    );
    assert_eq!(
        cell.journal().tally().get("capital_renewed"),
        Some(&1),
        "a change in the cell's authority was not journalled"
    );
    assert!(
        server.served() >= 2,
        "the round trip did not use the socket"
    );
    Ok(())
}

#[test]
fn the_same_grant_delivered_twice_renews_the_cells_capital_once() -> Result<()> {
    // At-least-once means this happens, and the receiving inbox absorbs a
    // redelivery only while it still remembers the key. Here the inbox is built
    // with a window of one and an unrelated frame is sent in between, which
    // rolls that window past the grant — the exact state the transport
    // documents as "the consumer's own idempotency is the only thing left".
    //
    // The cell recognises the second copy because it keys on the grant's
    // *signature*, which covers every bound the grant sets, rather than on
    // anything the wire chose.
    let inbox = MeshInbox::new("london-1-inbox", 64, 1)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    let mut centre = centre_publisher(&server.url())?;

    let renewal = signed_envelope(CELL, "2000", KEY, 7_200)?;
    centre.publish_frame(grant_frame(&renewal, "EVT-FIRST", t(30))?, t(30))?;
    centre.publish_frame(heartbeat_frame(1, t(31))?, t(31))?;
    centre.publish_frame(grant_frame(&renewal, "EVT-SECOND", t(32))?, t(32))?;
    assert_eq!(
        inbox.depth(),
        3,
        "the inbox absorbed the redelivery itself, so this test would prove nothing about the cell"
    );

    let mut cell = deployed_cell(3_600)?;
    let mut link = downlink(&server.url(), KEY)?;
    let batch = link.poll(t(60))?;

    assert_eq!(
        batch.verified.len(),
        1,
        "one grant delivered twice was verified {} times",
        batch.verified.len()
    );
    assert_eq!(
        batch.duplicates.len(),
        1,
        "the redelivery was not recognised, so it would have been applied twice"
    );
    assert_eq!(
        link.stats().ignored,
        1,
        "the frame that was not a grant was treated as one"
    );
    for envelope in batch.verified {
        cell.renew_capital(envelope, t(60))?;
    }
    assert_eq!(
        cell.journal().tally().get("capital_renewed"),
        Some(&1),
        "one grant produced more than one effect"
    );

    // And it stays recognised across polls, which is the case a rebuilt
    // subscriber produces.
    let again = link.poll(t(90))?;
    assert!(again.verified.is_empty());
    assert_eq!(link.stats().duplicates, 1);
    Ok(())
}

#[test]
fn an_envelope_for_a_strategy_this_cell_does_not_run_does_not_deploy_it() -> Result<()> {
    // ADR 0008's core safety argument from the other side: a cell receives
    // authority, it never promotes itself into using it. A grant arriving for
    // an undeployed strategy is refused rather than treated as an instruction
    // to start running one.
    let mut cell = deployed_cell(3_600)?;
    let unsigned = CapitalEnvelope::new(
        StrategyId::new("momentum-9"),
        CELL,
        dec!("1000"),
        dec!("400"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        t(0),
        t(3_600),
        "alice@example.com",
        "unsigned",
    )?;
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    let stranger = CapitalEnvelope::new(
        StrategyId::new("momentum-9"),
        CELL,
        dec!("1000"),
        dec!("400"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        t(0),
        t(3_600),
        "alice@example.com",
        signature,
    )?;
    let verified = VerifiedEnvelope::verify(stranger, KEY, CELL, t(10))?;

    let refusal = cell
        .renew_capital(verified, t(10))
        .expect_err("a grant for an undeployed strategy was accepted");
    assert!(
        refusal.message().contains("nothing for the grant to fund"),
        "{}",
        refusal.message()
    );
    assert_eq!(cell.deployed_strategies(), vec!["mean-reversion-1"]);
    Ok(())
}

#[test]
fn a_downlink_that_cannot_verify_anything_refuses_to_exist() -> Result<()> {
    // The cell's start-up rule, enforced where the envelopes actually arrive: a
    // downlink built without a key would deliver every envelope it received
    // while looking exactly like one that checked them.
    let error = CapitalDownlink::connect(
        DownlinkConfig::new(CELL, mesh_config("downlink:london-1", "http://127.0.0.1:1")),
        b"",
        clock(),
        sleeper(),
    )
    .expect_err("a downlink with no key was built");
    assert!(
        error.message().contains("cannot verify"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_uplink_refuses_to_relabel_another_cells_delta_as_its_own() -> Result<()> {
    // A deployment mistake — one cell's uplink handed another cell's state —
    // that would otherwise file the positions under the wrong name. The centre
    // would then believe them, which is worse than the delta never arriving.
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    let mut link = uplink(&server.url())?;

    let mut delta = deployed_cell(3_600)?.state_delta(&WorkReport::default(), t(20));
    delta.cell = "tokyo-2".to_string();
    let error = link
        .publish(delta, t(20))
        .expect_err("a delta belonging to another cell was published");
    assert!(error.message().contains("tokyo-2"), "{}", error.message());
    assert_eq!(inbox.depth(), 0, "the mislabelled delta was sent anyway");
    Ok(())
}
