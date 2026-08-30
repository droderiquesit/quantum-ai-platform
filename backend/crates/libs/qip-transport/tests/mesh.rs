//! Both ends of the mesh, against a real socket.
//!
//! The publisher and the endpoint are exercised together here because the two
//! properties that matter most are properties of the pair: a message that is
//! delivered twice must be recognised as one fact, and a poll whose response is
//! lost must not lose the frames it was carrying.
//!
//! The last test in this file is the one that matters for the architecture: a
//! forty-line adapter makes this transport a `qip_streaming::Publisher` and
//! `qip_streaming::Subscriber`, so it is a drop-in for `pubsub.rs` — which
//! refuses — without either side knowing about the other.

#![allow(clippy::panic_in_result_fn)]

mod common;

use qip_contracts::time::Watermark;
use qip_core::error::Result;
use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_streaming::{
    EventFacts, PublishReceipt, Publisher, Region, RoutingClass, SourceId, SourceIdentity,
    SourceType, StreamEnvelope, Subject, Subscriber, TransportDescriptor, TransportPath,
};
use qip_transport::{
    Admission, ClientLimits, DeadLetterSink, MemoryDeadLetters, MeshBatch, MeshConfig,
    MeshEndpoint, MeshInbox, MeshMessage, MeshPublisher, Method, PollRequest, RecordingSleeper,
    RemoteSubscriber, RetryPolicy, Sleeper,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const BASE_NANOS: i64 = 1_704_205_445_000_000_000;

fn at(millis: i64) -> Timestamp {
    Timestamp::from_nanos(BASE_NANOS + millis * 1_000_000)
}

/// What a regional cell publishes up to the global opportunity brain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RegionalDelta {
    region: String,
    sequence: u64,
    opportunities: u32,
    confidence_bp: u32,
}

impl EventBody for RegionalDelta {
    const TOPIC: Topic = Topic::OpportunityDetected;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.region, self.sequence))
    }
}

fn body(region: &str, sequence: u64) -> RegionalDelta {
    RegionalDelta {
        region: region.to_string(),
        sequence,
        opportunities: 3 + sequence as u32,
        confidence_bp: 7_250,
    }
}

fn frame(region: &str, sequence: u64, known_at: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        qip_core::Id::from_string(format!("EVT{region}{sequence:0>20}")),
        known_at,
        known_at,
        qip_core::Lineage::root(
            qip_core::CorrelationId::from_string(format!("COR{region}{sequence:0>20}")),
            "qip-transport-tests",
        ),
        body(region, sequence),
    )
    .erase()
}

/// The same fact wrapped in the platform's stream envelope, which is what
/// `qip-streaming` actually publishes.
fn envelope(region: &str, sequence: u64, known_at: Timestamp) -> Result<StreamEnvelope> {
    let facts = EventFacts::derived(
        SourceIdentity::new(
            SourceId::new("regional-cell"),
            SourceType::Internal,
            Region::new(region),
        ),
        Subject::unattributed(),
        Topic::OpportunityDetected,
    )
    .with_routing_class(RoutingClass::Warm);

    StreamEnvelope::seal(
        qip_core::Id::from_string(format!("EVT{region}{sequence:0>20}")),
        qip_core::Lineage::root(
            qip_core::CorrelationId::from_string(format!("COR{region}{sequence:0>20}")),
            "qip-transport-tests",
        ),
        body(region, sequence),
        known_at,
        known_at,
        facts,
    )
}

// --- a server that serves a real mesh endpoint --------------------------

/// A loopback server backed by a [`MeshEndpoint`], with a scriptable fault.
///
/// `sabotage(n)` says what to do to the nth response instead of sending it:
/// `Some(())` means the endpoint processes the request and then the connection
/// dies before the answer arrives, which is exactly the lost-acknowledgement
/// case at-least-once delivery exists for.
struct MeshServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MeshServer {
    fn spawn<F>(endpoint: MeshEndpoint, drop_response: F) -> Self
    where
        F: Fn(usize) -> bool + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let address = listener.local_addr().expect("a local address").to_string();
        listener.set_nonblocking(true).expect("pollable listener");

        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicUsize::new(0));
        let thread_stop = stop.clone();
        let thread_served = served.clone();

        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let index = thread_served.fetch_add(1, Ordering::SeqCst);
                        serve_one(stream, &endpoint, drop_response(index));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            stop,
            served,
            handle: Some(handle),
        }
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

/// Read one request, hand it to the endpoint, write the answer.
fn serve_one(mut stream: TcpStream, endpoint: &MeshEndpoint, drop_response: bool) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
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

    // The endpoint runs either way: the fault being simulated is a lost
    // *response*, not a request that never arrived.
    let response = endpoint.handle(method, &target, &body);
    if drop_response {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return;
    }

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

fn ladder() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(5),
        max_backoff: Duration::from_millis(40),
        multiplier: 2,
        jitter_basis_points: 0,
    }
}

fn publisher_against(peer: &str) -> Result<MeshPublisher> {
    MeshPublisher::new(
        MeshConfig::new("mesh:us-east", peer)
            .with_retry(ladder())
            .with_limits(ClientLimits {
                read_timeout: std::time::Duration::from_millis(500),
                connect_timeout: std::time::Duration::from_millis(500),
                ..ClientLimits::default()
            }),
        Arc::new(ManualClock::new(at(900))) as Arc<dyn Clock>,
        Arc::new(RecordingSleeper::new()) as Arc<dyn Sleeper>,
        Box::new(MemoryDeadLetters::new(16)),
    )
}

// --- the round trip -----------------------------------------------------

#[test]
fn a_stream_envelope_crosses_the_mesh_and_arrives_verifiable() -> Result<()> {
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |_| false);
    let mut publisher = publisher_against(&server.url())?;

    let original = envelope("us-east", 1, at(10))?;
    let delivery = publisher.publish_frame(original.to_frame()?, at(20))?;
    assert_eq!(delivery.attempts, 1);
    assert!(!delivery.duplicate);
    assert_eq!(delivery.peer_position, Some(1));

    let mut subscriber = RemoteSubscriber::new(
        MeshConfig::new("central-inbox", server.url()).with_retry(ladder()),
        Arc::new(RecordingSleeper::new()) as Arc<dyn Sleeper>,
    )?;
    let frames = subscriber.poll(at(50)).expect("the poll failed");
    assert_eq!(frames.len(), 1);

    let recovered = StreamEnvelope::from_frame(&frames[0])?;
    assert_eq!(
        recovered, original,
        "the envelope did not survive the wire unchanged"
    );
    // `from_frame` recomputes the payload hash, so this is not a formality:
    // an envelope edited in transit is refused rather than delivered.
    recovered.verify_payload_hash()?;
    assert_eq!(recovered.region().as_str(), "us-east");
    Ok(())
}

#[test]
fn a_batch_arrives_in_the_order_one_publisher_enqueued_it() -> Result<()> {
    // The whole ordering guarantee: per-publisher FIFO. Nothing stronger is
    // claimed anywhere, and this is the claim.
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |_| false);
    let mut publisher = publisher_against(&server.url())?;

    let frames: Vec<AnyEvent> = (1..=6)
        .map(|sequence| frame("us-east", sequence, at(sequence as i64)))
        .collect::<Result<_>>()?;
    let deliveries = publisher.publish_frames(frames, at(50))?;
    assert_eq!(deliveries.len(), 6);
    assert_eq!(
        deliveries.iter().map(|d| d.position).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6],
        "the publisher's own sequence must follow the enqueue order"
    );

    let response = inbox.read(0, Timestamp::MAX, 100);
    let sequences: Vec<u64> = response
        .frames
        .iter()
        .map(|entry| {
            entry
                .frame
                .decode::<RegionalDelta>()
                .map(|decoded| decoded.body.sequence)
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        sequences,
        vec![1, 2, 3, 4, 5, 6],
        "the peer received one publisher's messages out of order"
    );
    Ok(())
}

// --- at-least-once ------------------------------------------------------

#[test]
fn a_lost_acknowledgement_produces_a_redelivery_that_the_receiver_detects() -> Result<()> {
    // The case that makes exactly-once impossible: the peer accepts the
    // message and the acknowledgement is lost on the way back. The publisher
    // cannot tell that apart from the message never arriving, so it retries.
    let inbox = MeshInbox::new("central", 64, 256)?;
    // The first response is dropped after the endpoint has already queued it.
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |index| index == 0);
    let mut publisher = publisher_against(&server.url())?;

    let delivery = publisher.publish_frame(frame("us-east", 1, at(10))?, at(20))?;

    assert_eq!(
        delivery.attempts, 2,
        "the publisher must have retried a message it could not confirm"
    );
    assert!(
        delivery.duplicate,
        "the peer must report the second delivery as a repeat rather than accepting it twice"
    );
    assert_eq!(server.served(), 2);

    let stats = inbox.stats();
    assert_eq!(stats.accepted, 1, "the same fact was queued twice");
    assert_eq!(stats.duplicates, 1, "the redelivery was not recognised");
    assert_eq!(inbox.depth(), 1);
    assert_eq!(publisher.stats().duplicates, 1);
    Ok(())
}

#[test]
fn two_deliveries_of_one_fact_under_different_event_ids_are_still_one_fact() -> Result<()> {
    // A reconnecting publisher replaying its buffer produces new event ids for
    // facts the peer has already seen. Deduplication is on the idempotency key
    // — the body's natural key — precisely so this case is caught.
    let inbox = MeshInbox::new("central", 64, 256)?;

    let first = Envelope::new(
        qip_core::Id::from_string("EVTAAA00000000000000000000"),
        at(10),
        at(10),
        qip_core::Lineage::root(
            qip_core::CorrelationId::from_string("CORAAA00000000000000000000"),
            "test",
        ),
        body("us-east", 1),
    )
    .erase()?;
    let second = Envelope::new(
        qip_core::Id::from_string("EVTBBB00000000000000000000"),
        at(10),
        at(11),
        qip_core::Lineage::root(
            qip_core::CorrelationId::from_string("CORBBB00000000000000000000"),
            "test",
        ),
        body("us-east", 1),
    )
    .erase()?;

    assert_ne!(first.event_id, second.event_id);
    assert_eq!(
        first.dedup_key(),
        second.dedup_key(),
        "the fixture must actually be one fact delivered twice"
    );

    let accept = |event: &AnyEvent| {
        inbox.accept(&MeshMessage {
            key: event.dedup_key(),
            attempt: 1,
            frame: event.clone(),
        })
    };
    assert_eq!(accept(&first), Admission::Accepted(1));
    assert_eq!(accept(&second), Admission::Duplicate);
    assert_eq!(inbox.depth(), 1);
    Ok(())
}

#[test]
fn the_dedup_window_is_bounded_and_says_when_it_has_forgotten() -> Result<()> {
    // Duplicate detection is best-effort by construction: the window is
    // bounded because an unbounded key set is an unbounded allocation. What
    // matters is that it is countable, so nobody reads the guarantee as
    // absolute.
    let inbox = MeshInbox::new("central", 64, 2)?;
    for sequence in 1..=4u64 {
        let event = frame("us-east", sequence, at(sequence as i64))?;
        inbox.accept(&MeshMessage {
            key: event.dedup_key(),
            attempt: 1,
            frame: event,
        });
    }
    assert_eq!(inbox.stats().forgotten_keys, 2);

    // The oldest key has been forgotten, so its redelivery is no longer
    // detectable here — which is why the consumer must be idempotent too.
    let old = frame("us-east", 1, at(1))?;
    assert!(matches!(
        inbox.accept(&MeshMessage {
            key: old.dedup_key(),
            attempt: 2,
            frame: old,
        }),
        Admission::Accepted(_)
    ));
    Ok(())
}

#[test]
fn a_poll_whose_response_is_lost_redelivers_rather_than_losing_frames() -> Result<()> {
    // An inbox that deleted on read would turn one dropped response into
    // permanently missing state deltas. It does not delete until the next poll
    // acknowledges, so a lost response costs a duplicate and nothing else.
    let inbox = MeshInbox::new("central", 64, 256)?;
    for sequence in 1..=3u64 {
        let event = frame("us-east", sequence, at(sequence as i64))?;
        inbox.accept(&MeshMessage {
            key: event.dedup_key(),
            attempt: 1,
            frame: event,
        });
    }

    // Every response to a poll is dropped, so the subscriber exhausts its
    // ladder and reports a failure.
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |_| true);
    let mut subscriber = RemoteSubscriber::new(
        MeshConfig::new("central-inbox", server.url()).with_retry(ladder()),
        Arc::new(RecordingSleeper::new()) as Arc<dyn Sleeper>,
    )?;
    assert!(
        subscriber.poll(at(50)).is_err(),
        "a poll that never got an answer reported success"
    );
    assert_eq!(
        inbox.depth(),
        3,
        "frames were consumed by a poll whose answer never arrived"
    );
    assert!(
        subscriber.watermark().is_none(),
        "a subscriber that has received nothing must not publish a watermark: a consumer resuming \
         against a zero watermark would believe it had missed nothing"
    );
    drop(server);

    // A new server, no faults: the frames are still there.
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |_| false);
    let mut subscriber = RemoteSubscriber::new(
        MeshConfig::new("central-inbox", server.url()).with_retry(ladder()),
        Arc::new(RecordingSleeper::new()) as Arc<dyn Sleeper>,
    )?;
    assert_eq!(subscriber.poll(at(50))?.len(), 3);
    assert_eq!(
        subscriber.watermark().map(|w| w.position),
        Some(3),
        "the watermark must be the position actually taken"
    );

    // The next poll acknowledges them, and only then are they discarded.
    assert!(subscriber.poll(at(50))?.is_empty());
    assert_eq!(inbox.depth(), 0);
    assert_eq!(inbox.stats().acknowledged, 3);
    Ok(())
}

// --- the pull contract --------------------------------------------------

#[test]
fn a_poll_returns_only_what_was_known_by_its_until() -> Result<()> {
    // `until` is compared against known-time. A consumer at `until` can only
    // have been told things the platform already knew.
    let inbox = MeshInbox::new("central", 64, 256)?;
    for sequence in 1..=4u64 {
        let event = frame("us-east", sequence, at(sequence as i64 * 10))?;
        inbox.accept(&MeshMessage {
            key: event.dedup_key(),
            attempt: 1,
            frame: event,
        });
    }
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |_| false);
    let mut subscriber = RemoteSubscriber::new(
        MeshConfig::new("central-inbox", server.url()).with_retry(ladder()),
        Arc::new(RecordingSleeper::new()) as Arc<dyn Sleeper>,
    )?;

    assert_eq!(
        subscriber.poll(at(25))?.len(),
        2,
        "a poll ran past its own until"
    );
    assert_eq!(
        subscriber.pending_hint(),
        2,
        "the peer must report what it is still holding"
    );
    assert_eq!(subscriber.poll(at(45))?.len(), 2);
    assert!(subscriber.poll(at(45))?.is_empty());
    Ok(())
}

#[test]
fn a_poll_that_states_no_clock_of_its_own_is_refused() -> Result<()> {
    // An endpoint that supplied a default `until` would be handing back events
    // against a time nobody chose, which is the ambient-clock dependency the
    // pull contract exists to prevent.
    let endpoint = MeshEndpoint::new(MeshInbox::new("central", 8, 64)?);
    let response = endpoint.handle(Method::Post, "/v1/mesh/poll", b"{}");
    assert_eq!(response.status, 400);
    assert!(response.body_as_str().contains("until"));
    Ok(())
}

#[test]
fn the_endpoint_answers_the_routes_it_has_and_refuses_the_rest() -> Result<()> {
    let inbox = MeshInbox::new("central", 8, 64)?;
    let endpoint = MeshEndpoint::new(inbox);

    assert_eq!(
        endpoint.handle(Method::Get, "/v1/mesh/health", b"").status,
        200
    );
    assert_eq!(
        endpoint.handle(Method::Get, "/v1/mesh/publish", b"").status,
        405,
        "a publish by GET must be refused rather than treated as an empty batch"
    );
    assert_eq!(endpoint.handle(Method::Get, "/", b"").status, 404);
    assert_eq!(
        endpoint
            .handle(Method::Post, "/v1/mesh/publish", b"not json")
            .status,
        400
    );
    // A query string must not change which route is chosen.
    assert_eq!(
        endpoint
            .handle(Method::Get, "/v1/mesh/health?verbose=1", b"")
            .status,
        200
    );
    Ok(())
}

#[test]
fn a_frame_whose_payload_no_longer_matches_its_hash_is_refused() -> Result<()> {
    // Not cryptography and not claimed to be: it catches corruption in transit
    // and a naive edit, and stops nobody who can also recompute a SHA-256. The
    // authentication story is in the crate documentation, and it is mTLS.
    let inbox = MeshInbox::new("central", 8, 64)?;
    let endpoint = MeshEndpoint::new(inbox.clone());

    let mut event = frame("us-east", 1, at(10))?;
    event.payload = serde_json::json!({"region": "us-east", "sequence": 1, "opportunities": 9999});
    let batch = MeshBatch {
        sender: "impostor".to_string(),
        messages: vec![MeshMessage {
            key: "opportunity.detected:us-east:1".to_string(),
            attempt: 1,
            frame: event,
        }],
    };

    let response = endpoint.handle(
        Method::Post,
        "/v1/mesh/publish",
        &serde_json::to_vec(&batch)?,
    );
    assert_eq!(response.status, 422);
    assert_eq!(inbox.depth(), 0, "an edited frame was queued");
    Ok(())
}

// --- backpressure, end to end -------------------------------------------

#[test]
fn a_full_inbox_refuses_and_the_publisher_backs_off_rather_than_losing_the_message() -> Result<()> {
    // The chain the module documentation claims: a slow consumer fills its
    // inbox, the inbox answers 503, the publisher treats that as transient and
    // retries, and when the ladder runs out the message is recorded rather
    // than dropped.
    let inbox = MeshInbox::new("central", 2, 64)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |_| false);
    let letters = Arc::new(std::sync::Mutex::new(MemoryDeadLetters::new(8)));
    let sleeper = Arc::new(RecordingSleeper::new());
    let mut publisher = MeshPublisher::new(
        MeshConfig::new("mesh:us-east", server.url()).with_retry(ladder()),
        Arc::new(ManualClock::new(at(900))) as Arc<dyn Clock>,
        sleeper.clone() as Arc<dyn Sleeper>,
        Box::new(letters.clone()),
    )?;

    publisher.publish_frame(frame("us-east", 1, at(1))?, at(20))?;
    publisher.publish_frame(frame("us-east", 2, at(2))?, at(20))?;
    assert_eq!(inbox.depth(), 2, "the inbox should now be at its capacity");

    let error = publisher
        .publish_frame(frame("us-east", 3, at(3))?, at(20))
        .expect_err("a full inbox accepted a third message");
    assert_eq!(error.code(), "dead_lettered");
    assert_eq!(
        inbox.stats().refused,
        3,
        "the inbox must refuse every attempt rather than growing past its capacity"
    );
    assert_eq!(
        sleeper.recorded().len(),
        2,
        "a 503 must be treated as transient and backed off, not dead-lettered on the first try"
    );

    let held = letters.lock().unwrap_or_else(|p| p.into_inner());
    assert_eq!(held.len(), 1, "the refused message was not recorded");
    assert!(
        held.find("opportunity.detected:us-east:3")
            .is_some_and(|letter| letter.last_error.contains("503")),
        "the record must say the peer was full, which is the answer to 'why did this never arrive'"
    );
    Ok(())
}

// --- the drop-in claim --------------------------------------------------

/// The adapter that makes this transport a `qip_streaming` transport.
///
/// It lives in the test rather than in the crate for a structural reason:
/// `qip-streaming` is a service and `qip-transport` is a library, and
/// `a_library_never_depends_on_a_service_or_an_application` in the acceptance
/// suite enforces that direction. So the port adapter belongs beside the other
/// transports in `qip-streaming` — next to `pubsub.rs`, `local.rs` and
/// `durable.rs` — and this is the proof that it is the forty lines below and
/// nothing more.
#[derive(Debug)]
struct MeshTransport {
    publisher: MeshPublisher,
}

impl Publisher for MeshTransport {
    fn descriptor(&self) -> TransportDescriptor {
        let inner = self.publisher.descriptor();
        TransportDescriptor {
            name: inner.name,
            // Warm and cold events are what crosses this wire; a hot one is
            // refused, so the path it belongs to is the durable one.
            path: TransportPath::Durable,
            durable: inner.durable,
            available: inner.available,
            production_requirement: inner.production_requirement,
        }
    }

    fn publish(&mut self, envelope: StreamEnvelope, at: Timestamp) -> Result<PublishReceipt> {
        let delivery = self.publisher.publish_frame(envelope.to_frame()?, at)?;
        Ok(PublishReceipt {
            transport: delivery.transport,
            path: TransportPath::Durable,
            position: delivery.position,
            record_hash: None,
            accepted_at: delivery.accepted_at,
        })
    }
}

/// The pull half of the same adapter.
#[derive(Debug)]
struct MeshSubscription {
    subscriber: RemoteSubscriber,
}

impl Subscriber for MeshSubscription {
    fn descriptor(&self) -> TransportDescriptor {
        TransportDescriptor {
            name: self.subscriber.name().to_string(),
            path: TransportPath::Durable,
            durable: false,
            available: true,
            production_requirement: Some(MeshPublisher::REQUIREMENTS.join("; ")),
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<StreamEnvelope>> {
        self.subscriber
            .poll(until)?
            .iter()
            .map(StreamEnvelope::from_frame)
            .collect()
    }

    fn watermark(&self) -> Option<Watermark> {
        self.subscriber.watermark()
    }

    fn has_pending(&self, _until: Timestamp) -> bool {
        self.subscriber.pending_hint() > 0
    }
}

#[test]
fn the_mesh_is_a_drop_in_for_the_streaming_transport_ports() -> Result<()> {
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()), |_| false);

    let mut publisher = MeshTransport {
        publisher: publisher_against(&server.url())?,
    };
    let mut subscription = MeshSubscription {
        subscriber: RemoteSubscriber::new(
            MeshConfig::new("central-inbox", server.url()).with_retry(ladder()),
            Arc::new(RecordingSleeper::new()) as Arc<dyn Sleeper>,
        )?,
    };

    // Used only through the port traits from here on, which is the claim.
    fn publish_through_port(
        port: &mut dyn Publisher,
        envelopes: Vec<StreamEnvelope>,
    ) -> Result<()> {
        for (index, envelope) in envelopes.into_iter().enumerate() {
            let receipt = port.publish(envelope, at(20))?;
            assert_eq!(receipt.position, index as u64 + 1);
            assert_eq!(receipt.path, TransportPath::Durable);
        }
        Ok(())
    }

    let sent: Vec<StreamEnvelope> = (1..=3)
        .map(|sequence| envelope("us-east", sequence, at(sequence as i64 * 10)))
        .collect::<Result<_>>()?;
    publish_through_port(&mut publisher, sent.clone())?;

    let descriptor = Publisher::descriptor(&publisher);
    assert!(descriptor.available);
    assert!(
        !descriptor.durable,
        "the mesh must not claim a durability it does not have; that is the mistake pubsub.rs \
         refuses to make in the other direction"
    );
    assert!(descriptor.production_requirement.is_some());

    let received = Subscriber::poll(&mut subscription, at(25))?;
    assert_eq!(
        received,
        sent[..2].to_vec(),
        "what came back through the port is not what went in"
    );
    assert_eq!(
        subscription.watermark().map(|w| w.position),
        Some(2),
        "the port's watermark must be the position actually consumed"
    );
    assert!(subscription.has_pending(at(50)));

    let rest = Subscriber::poll(&mut subscription, at(50))?;
    assert_eq!(rest, sent[2..].to_vec());
    Ok(())
}

#[test]
fn the_inbox_speaks_the_same_wire_format_a_publisher_writes() -> Result<()> {
    // A round trip through JSON of every wire type, so a change to one side's
    // shape fails here rather than in a deployment where the two ends were
    // upgraded at different times.
    let event = frame("us-east", 1, at(10))?;
    let batch = MeshBatch {
        sender: "mesh:us-east".to_string(),
        messages: vec![MeshMessage {
            key: event.dedup_key(),
            attempt: 2,
            frame: event,
        }],
    };
    let encoded = serde_json::to_vec(&batch)?;
    let decoded: MeshBatch = serde_json::from_slice(&encoded)?;
    assert_eq!(decoded, batch);

    let request = PollRequest {
        until_nanos: at(50).as_nanos(),
        acknowledged: 7,
    };
    let decoded: PollRequest = serde_json::from_slice(&serde_json::to_vec(&request)?)?;
    assert_eq!(decoded, request);
    assert_eq!(
        Timestamp::from_nanos(decoded.until_nanos),
        at(50),
        "the poll boundary must survive the wire to the nanosecond: rounding it to the \
         millisecond would drop or double-deliver everything inside that millisecond"
    );
    Ok(())
}
