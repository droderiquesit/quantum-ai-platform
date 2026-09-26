//! `qip_transport::event_fabric::{producer, consumer}` — the client SDK ADR
//! 0100 §1 places beside the protocol and the transport seam.
//!
//! Every test here builds a [`Producer`] or [`Consumer`] over a scripted or
//! generative [`FabricTransport`] that never opens a socket, with one
//! exception (the read-timeout test), which is the one property that needs a
//! real one to prove at all.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use qip_core::error::{Error, Result};
use qip_core::time::SystemClock;
use qip_core::{CorrelationId, Duration as CoreDuration, EventId, Lineage, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::AckProfile;
use qip_events::{Envelope, EventBody, Topic};
use serde::{Deserialize, Serialize};

use qip_transport::breaker::{BreakerPolicy, BreakerState};
use qip_transport::event_fabric::consumer::{Consumer, ConsumerConfig, SubscriptionEvent};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::{
    FetchResponse, GroupCommitResponse, GroupJoinResponse, GroupLagResponse, ProduceAck,
    ProducerInitResponse, Refusal, Request, Response,
};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::retry::RecordingSleeper;
use qip_transport::server::{
    Handler, Request as ServerRequest, Response as ServerResponse, Server, ServerLimits,
};

// --- fixtures ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tick {
    symbol: String,
}

impl EventBody for Tick {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;
}

fn sample_event(id: &str) -> qip_events::AnyEvent {
    let lineage = Lineage {
        correlation_id: CorrelationId::from_string("COR00000000000000000000001"),
        causation_id: None,
        trace_id: None,
        producer: "event-fabric-client-test".to_string(),
    };
    let occurred_at = Timestamp::from_civil(2026, 3, 1);
    let envelope = Envelope::new(
        EventId::from_string(id),
        occurred_at,
        occurred_at,
        lineage,
        Tick {
            symbol: "SOLO".to_string(),
        },
    );
    envelope.erase().expect("Tick erases to AnyEvent")
}

/// A one-record batch, unstamped (writer-owned fields only) — exactly what
/// [`Producer::send`] expects to be given.
fn one_record_batch(id: &str) -> Batch {
    let event = sample_event(id);
    let record =
        Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("record encodes");
    Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .expect("one record makes a valid batch")
}

/// A whole, encoded batch stamped at `base_offset` with one record, as raw
/// wire bytes — what a fetch response's `batches` field carries hex-encoded,
/// before that encoding, so a test can concatenate several of these to build
/// a multi-batch response.
fn encoded_batch_bytes(base_offset: u64) -> Vec<u8> {
    let mut batch = one_record_batch(&format!("EVT{base_offset:023}"));
    batch.base_offset = base_offset;
    batch.encode().expect("batch encodes")
}

/// A whole, encoded, hex batch stamped at `base_offset` with one record — a
/// fetch response's `batches` field, exactly as a broker would send it.
fn encoded_batch_hex(base_offset: u64) -> String {
    qip_core::hash::to_hex(&encoded_batch_bytes(base_offset))
}

/// Several whole, encoded batches at the given base offsets, concatenated
/// and hex-encoded as one fetch response's `batches` field — a broker
/// answering with more than one batch in a single fetch, exactly as
/// `FetchResponse`'s own module documentation says it may.
fn concatenated_batches_hex(base_offsets: &[u64]) -> String {
    let mut bytes = Vec::new();
    for &offset in base_offsets {
        bytes.extend_from_slice(&encoded_batch_bytes(offset));
    }
    qip_core::hash::to_hex(&bytes)
}

fn producer_config(transport: Box<dyn FabricTransport + Send>) -> ProducerConfig {
    ProducerConfig {
        transport,
        stream: "orders".to_string(),
        partition: 0,
        producer_id: "cell-eu-1".to_string(),
        ack_profile: AckProfile::LeaderOnly,
        retry_policy: qip_transport::retry::RetryPolicy {
            max_attempts: 2,
            ..qip_transport::retry::RetryPolicy::default()
        },
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(SystemClock),
        sleeper: Arc::new(RecordingSleeper::new()),
        retry_seed: 11,
        breaker_seed: 11,
        timeouts: Timeouts::default(),
    }
}

fn new_producer(transport: Box<dyn FabricTransport + Send>) -> Producer {
    Producer::new(producer_config(transport)).expect("a well-formed producer config builds")
}

fn new_consumer(transport: Box<dyn FabricTransport + Send>) -> Consumer {
    let config = ConsumerConfig {
        transport,
        stream: "orders".to_string(),
        partition: 0,
        group_id: "risk-desk".to_string(),
        retry_policy: qip_transport::retry::RetryPolicy {
            max_attempts: 2,
            ..qip_transport::retry::RetryPolicy::default()
        },
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(SystemClock),
        sleeper: Arc::new(RecordingSleeper::new()),
        retry_seed: 13,
        breaker_seed: 13,
        timeouts: Timeouts::default(),
        fetch_credit_bytes: 4096,
    };
    Consumer::new(config).expect("a well-formed consumer config builds")
}

/// A [`FabricTransport`] answering one canned [`Response`] per call, in
/// order, with no socket anywhere. `calls` and `last_request` are handed out
/// as shared handles so a test can inspect them after the transport itself
/// has been moved into a [`Producer`] or [`Consumer`].
#[derive(Debug)]
struct ScriptedTransport {
    script: Arc<Mutex<VecDeque<Response>>>,
    calls: Arc<AtomicUsize>,
    last_request: Arc<Mutex<Option<Request>>>,
}

impl ScriptedTransport {
    fn new(script: Vec<Response>) -> (Self, Arc<AtomicUsize>, Arc<Mutex<Option<Request>>>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let last_request = Arc::new(Mutex::new(None));
        let transport = Self {
            script: Arc::new(Mutex::new(VecDeque::from(script))),
            calls: calls.clone(),
            last_request: last_request.clone(),
        };
        (transport, calls, last_request)
    }
}

impl FabricTransport for ScriptedTransport {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> Result<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_request.lock().unwrap_or_else(|p| p.into_inner()) = Some(request.clone());
        self.script
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
            .ok_or_else(|| {
                Error::invalid("test bug: the scripted transport's script ran out of answers")
            })
    }
}

/// A [`FabricTransport`] that answers every [`Request::Fetch`] with a fresh,
/// one-record batch at the requested offset, as fast as the calling thread
/// can ask — no delay, so a bound on how many calls happen in a fixed real
/// time window is a bound on the *consumer's own backpressure*, not on this
/// mock's speed.
#[derive(Debug)]
struct FetchLoopTransport {
    calls: Arc<AtomicUsize>,
}

impl FabricTransport for FetchLoopTransport {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> Result<Response> {
        let Request::Fetch(fetch) = request else {
            return Err(Error::invalid(
                "test bug: FetchLoopTransport only answers Fetch",
            ));
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        let hex = encoded_batch_hex(fetch.offset);
        let response = FetchResponse::new(
            fetch.stream.clone(),
            fetch.partition,
            fetch.offset + 1,
            fetch.offset,
            hex,
        )
        .expect("a coherent fetch response");
        Ok(Response::Fetch(response))
    }
}

// --- tests -------------------------------------------------------------

/// m5, RES-058: a `Refused{Fenced}` names the epoch it fenced, not the
/// producer. Asserts its own premise at every step — that the first init
/// really holds epoch 1, that the send really is refused, that the latch
/// really is set — before asserting the property the test is named for:
/// that re-initialising clears it.
///
/// Mutation: replace [`Producer::is_fenced`]'s epoch-scoped comparison with a
/// bare `self.fenced_epoch.is_some()` (latch on any `Fenced`, forever) —
/// fails, because the send after re-init would then be refused locally by
/// the stale latch instead of reaching the transport's third scripted
/// answer.
#[test]
fn a_producer_refused_for_an_epoch_it_no_longer_holds_does_not_latch_fenced() {
    let (transport, _calls, _last_request) = ScriptedTransport::new(vec![
        Response::ProducerInit(ProducerInitResponse { producer_epoch: 1 }),
        Response::Refused(Refusal::Fenced),
        Response::ProducerInit(ProducerInitResponse { producer_epoch: 2 }),
        Response::Produce(ProduceAck::new("orders", 0, 10, 20, 20).expect("a coherent ack")),
    ]);
    let mut producer = new_producer(Box::new(transport));

    producer.init().expect("the first init succeeds");
    assert_eq!(
        producer.epoch(),
        Some(1),
        "premise: the producer holds epoch 1 after the first init"
    );

    let refused = producer.send(one_record_batch("EVT0000000000000000000001"));
    assert!(
        refused.is_err(),
        "premise: the send under epoch 1 is genuinely refused"
    );
    assert!(
        producer.is_fenced(),
        "premise: the producer now believes the epoch it holds (1) is fenced"
    );

    producer
        .init()
        .expect("re-init after a fencing succeeds and assigns a fresh epoch");
    assert_eq!(
        producer.epoch(),
        Some(2),
        "premise: the producer now holds epoch 2, not the fenced epoch 1"
    );
    assert!(
        !producer.is_fenced(),
        "epoch 1 is fenced, but the producer no longer holds epoch 1: a stale Fenced answer \
         must not latch the reconnected producer"
    );

    let ack = producer
        .send(one_record_batch("EVT0000000000000000000002"))
        .expect(
            "a send under the fresh epoch must reach the transport, not be refused locally \
                 by the epoch-1 fencing",
        );
    assert_eq!(ack.base_offset(), 10);
}

/// FABRIC-080's retry/breaker split, proved against a real socket: a peer
/// that never answers must be discovered within the *configured* read
/// timeout, not the peer's own delay, and recorded as a breaker failure.
///
/// Mutation: in `Producer::call`, pass `Timeouts::default()` instead of
/// `self.timeouts` — fails, because the call then waits out the (much
/// longer) default read timeout instead of this test's short one, and the
/// bounded wall-clock deadline this test asserts on elapses before the call
/// returns.
#[test]
fn a_stalled_broker_times_out_on_the_explicit_read_timeout_and_opens_the_breaker() {
    let handler: Arc<dyn Handler> = Arc::new(StallingHandler {
        delay: StdDuration::from_secs(2),
    });
    let server =
        Server::bind("127.0.0.1:0", handler, ServerLimits::default()).expect("binds a port");
    let address = server
        .local_address()
        .expect("a bound listener reports its own address");
    std::thread::spawn(move || {
        let _ = server.serve_once();
    });

    let mut config = producer_config(Box::new(HttpTransport::new(
        format!("http://{address}"),
        qip_transport::event_fabric::auth::BearerToken::new(
            "sliceTestHarnessTokenForTheHttpTransport0123".to_string(),
        )
        .expect("a well-formed test token"),
    )));
    config.retry_policy.max_attempts = 1;
    config.breaker_policy.failure_threshold = 1;
    config.timeouts = Timeouts::new(
        StdDuration::from_millis(200),
        StdDuration::from_millis(150),
        StdDuration::from_millis(200),
    );
    let mut producer = Producer::new(config).expect("a well-formed producer config builds");
    assert_eq!(
        producer.breaker_state(),
        BreakerState::Closed,
        "premise: the breaker starts closed"
    );

    let started = Instant::now();
    let result = producer.init();
    let elapsed = started.elapsed();

    assert!(
        result.is_err(),
        "a broker that never answers must fail the call, not hang until it eventually does"
    );
    assert!(
        elapsed < StdDuration::from_millis(1_500),
        "the call must fail within the configured 150ms read timeout, not the stalling \
         handler's own 2s delay: took {elapsed:?}"
    );
    assert_eq!(
        producer.breaker_state(),
        BreakerState::Open,
        "one failure against a breaker with a failure threshold of one must open the circuit"
    );
}

#[derive(Debug)]
struct StallingHandler {
    delay: StdDuration,
}

impl Handler for StallingHandler {
    fn handle(&self, _request: &ServerRequest) -> ServerResponse {
        std::thread::sleep(self.delay);
        ServerResponse::json(200, "{}".to_string())
    }
}

/// FABRIC-034/FABRIC-080: subscribing must not let a slow caller turn into an
/// unbounded backlog. Asserts its own premise (the fetch thread actually ran)
/// before asserting the bound, then proves the bound is not permanent by
/// draining one event and watching the thread make progress again.
///
/// Mutation: in `Consumer::subscribe`, replace `mpsc::sync_channel(channel_bound)`
/// with `mpsc::channel()` (unbounded) — fails, because the fetch thread then
/// runs far ahead of the bound while nothing drains it, and the settle-time
/// call count is far more than `bound + 1`.
#[test]
fn a_subscription_delivers_into_a_bounded_channel_and_a_slow_caller_back_pressures_the_fetch_thread()
 {
    let calls = Arc::new(AtomicUsize::new(0));
    let consumer = new_consumer(Box::new(FetchLoopTransport {
        calls: calls.clone(),
    }));

    let bound = 2usize;
    let subscription = consumer
        .subscribe(bound, CoreDuration::from_millis(5))
        .expect("subscribe builds with a positive channel bound");

    std::thread::sleep(StdDuration::from_millis(200));
    let settled = calls.load(Ordering::SeqCst);
    assert!(
        settled > 0,
        "premise: the fetch thread actually fetched something before we assert a bound on it"
    );
    assert!(
        settled <= bound + 1,
        "a bounded channel of capacity {bound} must back-pressure the fetch thread to at most \
         one fetch beyond that capacity while nothing drains it, but it made {settled} calls"
    );

    let event = subscription
        .recv()
        .expect("a delivered batch is waiting in the channel");
    assert!(
        matches!(event, SubscriptionEvent::Delivered(_)),
        "the first event off a healthy subscription must be a delivered batch: {event:?}"
    );

    std::thread::sleep(StdDuration::from_millis(100));
    assert!(
        calls.load(Ordering::SeqCst) > settled,
        "draining exactly one event must free exactly enough room for the fetch thread to make \
         progress again"
    );
}

/// FABRIC-016/RES-058: resuming after a restart reads one past what was
/// committed, never the committed offset itself — reading the committed
/// offset again would redeliver the record the commit already said was
/// processed. Simulates the restart as a second, independent [`Consumer`]
/// rather than reusing the first, because nothing about resuming may depend
/// on in-process state a real restart would not have.
///
/// Mutation: in `Consumer::resume`, use `lag.committed_offset()` directly
/// instead of `.checked_add(1)` — fails, because the recorded fetch request
/// then asks for offset 41 (a duplicate of what was already committed)
/// instead of 42.
#[test]
fn a_consumer_killed_after_committing_n_resumes_at_n_plus_one() {
    let (transport, _calls, _last_request) = ScriptedTransport::new(vec![
        Response::GroupJoin(GroupJoinResponse {
            member_id: "member-1".to_string(),
            generation: 3,
            assigned_partitions: vec![0],
        }),
        Response::GroupCommit(GroupCommitResponse {
            committed_offset: 41,
        }),
    ]);
    let mut consumer = new_consumer(Box::new(transport));
    consumer.join().expect("join succeeds");
    let committed = consumer.commit(41).expect("commit succeeds");
    assert_eq!(
        committed, 41,
        "premise: the broker actually committed offset 41"
    );
    drop(consumer); // the process is "killed": nothing about it survives.

    let (transport, _calls, last_request) = ScriptedTransport::new(vec![
        Response::GroupLag(GroupLagResponse::new(41, 200).expect("a coherent lag answer")),
        Response::Fetch(
            FetchResponse::new("orders", 0, 200, 41, String::new()).expect("an empty fetch answer"),
        ),
    ]);
    let mut consumer = new_consumer(Box::new(transport));

    let resumed = consumer.resume().expect("resume succeeds");
    assert_eq!(
        resumed, 42,
        "a consumer that killed after committing 41 must resume at 42, not re-read 41"
    );
    assert_eq!(consumer.next_offset(), 42);

    consumer.fetch().expect("fetch succeeds");
    let request = last_request
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .expect("the transport recorded the fetch request");
    let Request::Fetch(fetch) = request else {
        panic!("expected the resumed consumer's call to be a Fetch request: {request:?}");
    };
    assert_eq!(
        fetch.offset, 42,
        "the resumed consumer must fetch at offset 42, not the committed offset 41"
    );
}

/// ADR 0100 §1's swap seam, from the client's own side: a [`Producer`] built
/// over an in-memory [`FabricTransport`] must drive every one of its calls
/// through that transport and nothing else.
///
/// Mutation: in `call_with_resilience`, on `Decision::Admitted`, answer from
/// a fabricated, route-shaped value instead of ever calling
/// `transport.call(...)` — exactly what a production bug that opened its own
/// socket to the configured peer, bypassing this seam, would look like from
/// the caller's side. Fails, because the in-memory transport's own call
/// counter then stays at zero while the producer still reports success.
#[test]
fn a_producer_runs_over_an_in_memory_transport_without_opening_a_socket() {
    let (transport, calls, _last_request) = ScriptedTransport::new(vec![
        Response::ProducerInit(ProducerInitResponse { producer_epoch: 9 }),
        Response::Produce(ProduceAck::new("orders", 0, 0, 5, 5).expect("a coherent ack")),
    ]);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "premise: nothing has called the transport before the producer does anything"
    );
    let mut producer = new_producer(Box::new(transport));

    producer
        .init()
        .expect("init answers over the in-memory transport");
    producer
        .send(one_record_batch("EVT0000000000000000000003"))
        .expect("send answers over the in-memory transport");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "every call this producer made (one init, one send) must have gone through the \
         in-memory transport, and only it"
    );
}

// --- FU-CODEC / SLICE-28: every batch in a fetch --------------------------

/// SLICE-28 found `Consumer::fetch` decoding only the first batch of a
/// `FetchResponse` and moving `next_offset` only past that one batch's own
/// records, with no error at all — a broker answering with more than one
/// concatenated batch left every record past the first to be re-requested by
/// a later fetch rather than delivered from this one, an extra round trip
/// and re-spent credit each time but never a permanently lost record (the
/// broker still held them at the offset the next fetch would ask for).
/// `Batch::decode_prefix` (FU-CODEC) reports how many bytes one frame
/// consumed, so this client can walk every frame in the response and deliver
/// each in turn without a second round trip.
///
/// Mutation: in `Consumer::fetch`, after decoding, keep only
/// `batches.next()` and never populate `self.pending` with the rest (the
/// behaviour this test replaces) — fails, because the second call to
/// `fetch()` below then issues a fresh network request against a transport
/// whose one scripted answer is already spent, returning `Err("test bug: the
/// scripted transport's script ran out of answers")` instead of the second
/// batch.
#[test]
fn a_fetch_carrying_three_concatenated_batches_delivers_all_three_in_order() {
    let hex = concatenated_batches_hex(&[10, 11, 12]);
    let (transport, calls, _last_request) = ScriptedTransport::new(vec![Response::Fetch(
        FetchResponse::new("orders", 0, 13, 0, hex).expect("a coherent fetch response"),
    )]);
    let mut consumer = new_consumer(Box::new(transport));

    // Premise: nothing has touched the transport yet.
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        consumer.next_offset(),
        0,
        "premise: the consumer has not advanced before the first fetch"
    );

    let first = consumer
        .fetch()
        .expect("fetch succeeds")
        .expect("the first of three concatenated batches is delivered");
    assert_eq!(
        first.batch.base_offset, 10,
        "the first batch delivered must be the first in order"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the first batch must come from the transport's one scripted network call"
    );
    assert_eq!(
        consumer.next_offset(),
        11,
        "next_offset must already sit one past the first batch's own record before the second \
         batch is ever asked for, exactly as `advance_past`'s own documentation promises \
         between calls — not only once every batch has been drained"
    );

    let second = consumer
        .fetch()
        .expect("fetch succeeds")
        .expect("the second of three concatenated batches is delivered");
    assert_eq!(
        second.batch.base_offset, 11,
        "the second batch delivered must be the second in order"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the second batch must come from what this consumer already decoded, not a second \
         network call — the transport's script holds only one scripted answer, so a second \
         call here would already have failed"
    );
    assert_eq!(
        consumer.next_offset(),
        12,
        "next_offset must sit one past the second batch's own record immediately, before the \
         third batch is delivered"
    );

    let third = consumer
        .fetch()
        .expect("fetch succeeds")
        .expect("the third of three concatenated batches is delivered");
    assert_eq!(
        third.batch.base_offset, 12,
        "the third batch delivered must be the third in order"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    assert_eq!(
        consumer.next_offset(),
        13,
        "after every batch has been delivered, the next offset must sit one past the last \
         record of the last batch, exactly as if each had been fetched over the wire on its own"
    );
}

/// SLICE-28's known limitation, closed the other way: a fetch response must
/// never be partly trusted. A batch after a good one that fails to decode is
/// refused, naming the byte offset within the response's decoded body at
/// which it begins, rather than the good batch being handed to the caller
/// while the corrupt one is quietly dropped.
///
/// Mutation: in `decode_every_batch`, replace the `Err(error) => return
/// Err(...)` arm with `Err(_) => break` — the batches found so far are
/// returned as `Ok` instead of the whole response being refused. Fails,
/// because `consumer.fetch()` then returns `Ok(Some(_))` carrying the first,
/// good batch instead of the `Err` this test requires, and the offset
/// assertion never runs.
#[test]
fn a_corrupt_second_batch_in_a_fetch_is_refused_naming_its_offset_not_skipped() {
    let first_bytes = encoded_batch_bytes(30);
    let first_len = first_bytes.len();
    let mut second_bytes = encoded_batch_bytes(31);
    // Flip the last bit of the second batch's own final byte — inside its
    // last record's trailing CRC — so only that record's own CRC, not its
    // prefix or header CRC, catches it. The same corruption
    // `qip-events`' own `flipping_any_bit_of_a_complete_batch_is_refused_as_corruption_naming_its_offset`
    // uses.
    let flip_at = second_bytes.len() - 1;
    second_bytes[flip_at] ^= 0x01;

    let mut bytes = first_bytes.clone();
    bytes.extend_from_slice(&second_bytes);

    // Premise: corrupting the second batch's own bytes left the first
    // batch's bytes, still sitting at the front of the buffer, untouched.
    assert_eq!(&bytes[..first_len], first_bytes.as_slice());

    let hex = qip_core::hash::to_hex(&bytes);
    let (transport, calls, _last_request) = ScriptedTransport::new(vec![Response::Fetch(
        FetchResponse::new("orders", 0, 32, 0, hex).expect("a coherent fetch response"),
    )]);
    let mut consumer = new_consumer(Box::new(transport));

    // Premise: the offset has not moved before the refused fetch.
    assert_eq!(consumer.next_offset(), 0);

    let err = consumer.fetch().expect_err(
        "a corrupt batch after a good one must refuse the whole fetch, not deliver the good \
         batch while silently skipping the corrupt one",
    );
    // A bare `contains(&first_len.to_string())` is a substring trap here:
    // the corrupt batch's own inner CRC-mismatch message names a *second*
    // offset (relative to the corrupt batch's own bytes, not the response),
    // and nothing stops that inner number from containing this one's digits
    // as a substring by coincidence on a future fixture change. Matching the
    // delimited phrase the outer wrapper actually writes pins this down to
    // the offset this test means to check.
    let expected_phrase = format!("byte offset {first_len} of");
    assert!(
        err.message().contains(&expected_phrase),
        "the refusal must name the byte offset within the response's decoded body at which \
         the corrupt batch begins (\"{expected_phrase}\"): {}",
        err.message()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "premise: the refusal came from the transport's one scripted network call, not a retry"
    );
    assert_eq!(
        consumer.next_offset(),
        0,
        "a refused fetch must not advance the offset past the good batch that preceded the \
         corrupt one — the same offset must be retried"
    );
}

/// FU-CODEC rework, defect 3: a *torn* batch (a truncated frame, not a
/// corrupt one) after a good batch must be refused the same way a corrupt
/// one is — naming its offset, never silently dropped in favour of the good
/// batch that preceded it. Before this test, nothing in this crate drove a
/// good-batch-then-torn-batch fetch, so a change to `decode_every_batch`'s
/// `Torn` arm that special-cased "torn after at least one good batch" by
/// breaking out of the loop instead of refusing would leave every test in
/// this file green.
///
/// Mutation: in `decode_every_batch`, replace the `Ok(PrefixDecodeOutcome::Torn) => { return
/// Err(...) }` arm with `Ok(PrefixDecodeOutcome::Torn) if offset > 0 => break` (falling through
/// to `Ok(batches)` with only the good batch collected so far) — fails, because
/// `consumer.fetch()` then returns `Ok(Some(_))` carrying the first, good batch instead of the
/// `Err` this test requires, and the offset assertion never runs.
#[test]
fn a_torn_second_batch_in_a_fetch_is_refused_naming_its_offset_not_skipped() {
    let first_bytes = encoded_batch_bytes(40);
    let first_len = first_bytes.len();
    let full_second_bytes = encoded_batch_bytes(41);
    // Cut the second batch's own bytes short mid-frame: past the prefix (so
    // its declared length is read and trusted) but short of that declared
    // length, exactly what a batch cut off mid-write looks like on the wire.
    let torn_second_bytes = &full_second_bytes[..full_second_bytes.len() - 5];

    let mut bytes = first_bytes.clone();
    bytes.extend_from_slice(torn_second_bytes);

    // Premise: the first batch's own bytes, still at the front of the
    // buffer, are untouched, and the second batch really is shorter than a
    // complete frame.
    assert_eq!(&bytes[..first_len], first_bytes.as_slice());
    assert!(
        torn_second_bytes.len() < full_second_bytes.len(),
        "premise: the second batch is genuinely truncated, not a full frame"
    );

    let hex = qip_core::hash::to_hex(&bytes);
    let (transport, calls, _last_request) = ScriptedTransport::new(vec![Response::Fetch(
        FetchResponse::new("orders", 0, 42, 0, hex).expect("a coherent fetch response"),
    )]);
    let mut consumer = new_consumer(Box::new(transport));

    // Premise: the offset has not moved before the refused fetch.
    assert_eq!(consumer.next_offset(), 0);

    let err = consumer.fetch().expect_err(
        "a torn batch after a good one must refuse the whole fetch, not deliver the good batch \
         while silently dropping the torn one",
    );
    let expected_phrase = format!("byte offset {first_len} of");
    assert!(
        err.message().contains(&expected_phrase),
        "the refusal must name the byte offset within the response's decoded body at which the \
         torn batch begins (\"{expected_phrase}\"): {}",
        err.message()
    );
    assert!(
        err.message().contains("torn"),
        "the refusal must say the frame is torn, not merely corrupt, so an operator does not \
         chase a CRC mismatch that was never there: {}",
        err.message()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "premise: the refusal came from the transport's one scripted network call, not a retry"
    );
    assert_eq!(
        consumer.next_offset(),
        0,
        "a refused fetch must not advance the offset past the good batch that preceded the torn \
         one — the same offset must be retried"
    );
}

/// FU-CODEC rework, defect 1 (most severe): a batch that cannot be advanced
/// past — here, one whose `base_offset + records.len()` overflows `u64` —
/// must not leave any *later* batch from the same response sitting in
/// `pending`. Before this fix, `Consumer::fetch` populated `pending` with
/// every batch after the first *before* calling `advance_past` on the first,
/// so a first batch that failed to advance still left the second batch
/// cached; the very next `fetch()` call served that second batch straight
/// out of `pending` with **no network call and no error at all**, silently
/// skipping the batch this call had just refused. This is reachable in
/// production: `qip-edge-node`'s control loop records a fetch `Err` and
/// keeps stepping, so its next fetch would skip a refused control-stream
/// batch rather than being refused again on it.
///
/// Mutation: in `Consumer::fetch`'s network-response arm, swap the order back
/// to `self.pending.extend(...)` before `self.advance_past(&first)?` — fails,
/// because the second `fetch()` call below then returns `Ok(Some(_))` for the
/// batch at offset 5 straight out of `pending`, with the call count staying
/// at 1 (no second network request), instead of the fresh `Err` this test
/// requires from a genuine retry at the same offset.
#[test]
fn a_fetch_whose_first_batch_cannot_advance_the_offset_leaves_nothing_pending_for_the_next_call() {
    let hex = concatenated_batches_hex(&[u64::MAX, 5]);
    let retry_hex = encoded_batch_hex(0);
    let (transport, calls, last_request) = ScriptedTransport::new(vec![
        Response::Fetch(
            FetchResponse::new("orders", 0, 0, 0, hex).expect("a coherent fetch response"),
        ),
        Response::Fetch(
            FetchResponse::new("orders", 0, 1, 0, retry_hex).expect("a coherent fetch response"),
        ),
    ]);
    let mut consumer = new_consumer(Box::new(transport));

    // Premise: nothing has moved yet.
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(consumer.next_offset(), 0);

    let err = consumer.fetch().expect_err(
        "a batch whose base offset plus its own record count overflows u64 must refuse the \
         fetch rather than silently wrap or truncate the arithmetic",
    );
    assert!(
        err.message().contains("overflow"),
        "the refusal must name the overflow: {}",
        err.message()
    );
    assert!(
        err.message().contains(&u64::MAX.to_string()),
        "the refusal must name the base offset that overflowed: {}",
        err.message()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "premise: the failing fetch consumed exactly the first scripted network call"
    );
    assert_eq!(
        consumer.next_offset(),
        0,
        "a fetch that fails to advance past its first batch must leave next_offset untouched, \
         so the very same offset is retried rather than skipped"
    );

    let delivered = consumer
        .fetch()
        .expect("the retry succeeds")
        .expect("a batch is delivered on retry");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the batch after a failed advance must come from a fresh network call, never from a \
         batch this consumer already cached in `pending` — caching it there would let this call \
         silently skip whatever made the previous batch unrefusable, with no request to the \
         broker at all"
    );
    assert_eq!(
        delivered.batch.base_offset, 0,
        "the retry must genuinely re-ask the broker at offset 0, not resume from a batch at \
         offset 5 left over in `pending` by the failed call"
    );
    let request = last_request
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .expect("the transport recorded the retry's request");
    let Request::Fetch(fetch) = request else {
        panic!("expected the retry to be a Fetch request: {request:?}");
    };
    assert_eq!(
        fetch.offset, 0,
        "the retry must actually ask the broker for offset 0, the offset the failed fetch never \
         advanced past"
    );
}

/// FU-CODEC rework, defect 1's other flaw: the *pending* path had the same
/// bug in mirror image. Before this fix, `Consumer::fetch` popped a batch out
/// of `pending` and only then called `advance_past` on it; a failure there
/// returned `Err` with the batch already removed from the queue, so it was
/// gone for good rather than refused-and-retryable. A caller retrying after
/// the `Err` (exactly what a control loop that keeps stepping on a fetch
/// error does) would then either fall through to a brand-new network call at
/// whatever offset was never advanced past, or — with more than one bad batch
/// queued — silently move on to the *next* queued batch, skipping the one
/// that had just failed.
///
/// Mutation: in `Consumer::fetch`'s pending branch, go back to `if let
/// Some(fetched) = self.pending.pop_front() { self.advance_past(&fetched.batch)?; return
/// Ok(Some(fetched)); }` — fails, because the third `fetch()` call below then issues a fresh
/// network request (the popped, overflowing batch is already gone from `pending`) against a
/// transport whose one scripted answer is already spent, so both the call count and the
/// "overflow" wording in the error diverge from what this test requires.
#[test]
fn a_batch_queued_in_pending_that_cannot_advance_the_offset_is_refused_again_not_dropped() {
    let hex = concatenated_batches_hex(&[10, u64::MAX]);
    let (transport, calls, _last_request) = ScriptedTransport::new(vec![Response::Fetch(
        FetchResponse::new("orders", 0, 0, 0, hex).expect("a coherent fetch response"),
    )]);
    let mut consumer = new_consumer(Box::new(transport));

    let first = consumer
        .fetch()
        .expect("fetch succeeds")
        .expect("the first of two concatenated batches is delivered");
    assert_eq!(
        first.batch.base_offset, 10,
        "premise: the first batch delivered is base offset 10, leaving the overflowing batch \
         cached in `pending`"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(consumer.next_offset(), 11);

    let first_refusal = consumer.fetch().expect_err(
        "a batch queued in `pending` whose base offset plus its own record count overflows u64 \
         must refuse the fetch rather than being silently dropped from the queue",
    );
    assert!(
        first_refusal.message().contains("overflow"),
        "{}",
        first_refusal.message()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "premise: the refusal came from the already-decoded queue, not a new network call"
    );

    // The failing batch must still be sitting in `pending` to be retried —
    // never popped and lost the moment `advance_past` first refused it.
    let second_refusal = consumer.fetch().expect_err(
        "retrying the same failed batch must refuse identically, not silently move past it to a \
         network call the transport's script holds no answer for",
    );
    assert!(
        second_refusal.message().contains("overflow"),
        "the same overflowing batch must still be the one refused, not a different failure from \
         a network call the mock cannot answer: {}",
        second_refusal.message()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the overflowing batch must still be the one being retried out of `pending` on every \
         call, never popped, dropped, and replaced by a fresh network request"
    );
}

/// FU-CODEC rework, defect 2: `seek` must control what the *next* `fetch()`
/// returns, not merely what `next_offset()` reports. Before this fix, `seek`
/// overwrote `next_offset` but left `pending` untouched, so a caller seeking
/// after a multi-batch fetch would still be served whatever this consumer had
/// already cached from *before* the seek — silently undoing the seek one
/// `fetch()` call later.
///
/// Mutation: remove `self.pending.clear()` from `Consumer::seek` — fails,
/// because the fetch after `seek(0)` below then delivers the batch at offset
/// 11 straight out of the stale `pending` queue, over the transport's one
/// scripted network call, instead of asking the broker for offset 0.
#[test]
fn seeking_after_a_multi_batch_fetch_clears_whatever_was_left_pending() {
    let hex = concatenated_batches_hex(&[10, 11, 12]);
    let (transport, calls, last_request) = ScriptedTransport::new(vec![
        Response::Fetch(
            FetchResponse::new("orders", 0, 13, 0, hex).expect("a coherent fetch response"),
        ),
        Response::Fetch(
            FetchResponse::new("orders", 0, 1, 0, encoded_batch_hex(0))
                .expect("a coherent fetch response"),
        ),
    ]);
    let mut consumer = new_consumer(Box::new(transport));

    let first = consumer
        .fetch()
        .expect("fetch succeeds")
        .expect("the first of three concatenated batches is delivered");
    assert_eq!(
        first.batch.base_offset, 10,
        "premise: the first batch delivered is base offset 10, leaving batches 11 and 12 cached \
         in `pending`"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    consumer.seek(0);
    assert_eq!(
        consumer.next_offset(),
        0,
        "premise: seek(0) moved next_offset back to 0"
    );

    let after_seek = consumer
        .fetch()
        .expect("fetch after seek succeeds")
        .expect("a batch is delivered after seeking");
    assert_eq!(
        after_seek.batch.base_offset, 0,
        "a fetch after seek(0) must deliver the batch actually at offset 0, not a batch still \
         cached in `pending` from before the seek"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "seek must have cleared whatever `pending` held, forcing the delivery above to come \
         from a fresh network request rather than the stale cache"
    );
    let request = last_request
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .expect("the transport recorded the post-seek request");
    let Request::Fetch(fetch) = request else {
        panic!("expected the post-seek call to be a Fetch request: {request:?}");
    };
    assert_eq!(
        fetch.offset, 0,
        "the fetch after seek(0) must actually ask the broker for offset 0"
    );
}

/// FU-CODEC rework, defect 2's other call site: `resume` has the identical
/// flaw `seek` has, and for the identical reason — it overwrites
/// `next_offset` from the broker's own lag answer but, before this fix, left
/// `pending` untouched.
///
/// Mutation: remove `self.pending.clear()` from `Consumer::resume` — fails,
/// because the fetch after `resume()` below then delivers the batch at
/// offset 11 straight out of the stale `pending` queue instead of the batch
/// actually sitting at the resumed offset 100, and the network call count
/// stays at two instead of reaching three.
#[test]
fn resuming_after_a_multi_batch_fetch_clears_whatever_was_left_pending() {
    let hex = concatenated_batches_hex(&[10, 11, 12]);
    let (transport, calls, _last_request) = ScriptedTransport::new(vec![
        Response::Fetch(
            FetchResponse::new("orders", 0, 13, 0, hex).expect("a coherent fetch response"),
        ),
        Response::GroupLag(GroupLagResponse::new(99, 200).expect("a coherent lag answer")),
        Response::Fetch(
            FetchResponse::new("orders", 0, 101, 100, encoded_batch_hex(100))
                .expect("a coherent fetch response"),
        ),
    ]);
    let mut consumer = new_consumer(Box::new(transport));

    let first = consumer
        .fetch()
        .expect("fetch succeeds")
        .expect("the first of three concatenated batches is delivered");
    assert_eq!(
        first.batch.base_offset, 10,
        "premise: the first batch delivered is base offset 10, leaving batches 11 and 12 cached \
         in `pending`"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let resumed = consumer.resume().expect("resume succeeds");
    assert_eq!(
        resumed, 100,
        "premise: the broker's committed offset of 99 resumes this consumer at 100"
    );
    assert_eq!(consumer.next_offset(), 100);

    let after_resume = consumer
        .fetch()
        .expect("fetch after resume succeeds")
        .expect("a batch is delivered after resuming");
    assert_eq!(
        after_resume.batch.base_offset, 100,
        "a fetch after resume() must deliver the batch actually at the resumed offset, not a \
         batch still cached in `pending` from before the resume"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "resume must have cleared whatever `pending` held, forcing the delivery above to come \
         from a fresh network request (the GroupLag call plus this fetch) rather than the stale \
         cache"
    );
}

/// FU-CODEC rework, defect 8: `decode_every_batch` must preserve the class
/// the codec itself assigned a refusal, not flatten every one of them to
/// `Error::schema`. An oversized declared length is the codec's own
/// `Error::invalid` (a value refused outright, checked before any record is
/// parsed) — distinct from `Error::schema` (a value that parsed but failed a
/// CRC), and a caller matching on `Error::code()` must see that distinction
/// survive a second batch in the same response exactly as it would from the
/// first.
///
/// Mutation: in `decode_every_batch`'s `Err(error)` arm, go back to
/// `Error::schema(format!(...))` instead of `error.relabelled(format!(...))`
/// — fails, because the refusal's `code()` then reads `"schema"` instead of
/// the `"invalid"` this test requires.
#[test]
fn a_second_batch_with_an_oversized_declared_length_keeps_the_codecs_own_invalid_class() {
    let first_bytes = encoded_batch_bytes(50);
    let first_len = first_bytes.len();

    // A second frame whose own declared length claims more than the codec's
    // ceiling allows — refused by the codec itself as `Error::invalid`,
    // checked before any byte past the prefix is trusted.
    const PREFIX_FIELDS_LEN: usize = 4 + 2 + 4;
    const PREFIX_LEN: usize = PREFIX_FIELDS_LEN + 4;
    let mut oversized_prefix = vec![0u8; PREFIX_LEN];
    oversized_prefix[0..4].copy_from_slice(b"QEVB");
    oversized_prefix[4..6].copy_from_slice(&1u16.to_le_bytes());
    let huge_len = u32::MAX;
    oversized_prefix[6..10].copy_from_slice(&huge_len.to_le_bytes());
    let prefix_crc =
        qip_events::event_fabric::crc32c::crc32c(&oversized_prefix[..PREFIX_FIELDS_LEN]);
    oversized_prefix[PREFIX_FIELDS_LEN..PREFIX_LEN].copy_from_slice(&prefix_crc.to_le_bytes());

    let mut bytes = first_bytes.clone();
    bytes.extend_from_slice(&oversized_prefix);

    // Premise: the first batch's own bytes are untouched, and the fixture
    // appended is exactly one prefix's worth of bytes (a torn frame would
    // hide the oversized-length refusal behind a `Torn` outcome instead).
    assert_eq!(&bytes[..first_len], first_bytes.as_slice());
    assert_eq!(bytes.len(), first_len + PREFIX_LEN);

    let hex = qip_core::hash::to_hex(&bytes);
    let (transport, calls, _last_request) = ScriptedTransport::new(vec![Response::Fetch(
        FetchResponse::new("orders", 0, 51, 0, hex).expect("a coherent fetch response"),
    )]);
    let mut consumer = new_consumer(Box::new(transport));

    let err = consumer.fetch().expect_err(
        "a second batch declaring a body over the codec's ceiling must refuse the whole fetch",
    );
    assert_eq!(
        err.code(),
        "invalid",
        "the codec's own `Error::invalid` class for an oversized declared length must survive \
         decode_every_batch's own wrapping, not be flattened to \"schema\": {}",
        err.message()
    );
    let expected_phrase = format!("byte offset {first_len} of");
    assert!(
        err.message().contains(&expected_phrase),
        "the refusal must still name the byte offset within the response at which the second \
         batch begins (\"{expected_phrase}\"): {}",
        err.message()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
