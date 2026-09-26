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

/// A whole, encoded, hex batch stamped at `base_offset` with one record — a
/// fetch response's `batches` field, exactly as a broker would send it.
fn encoded_batch_hex(base_offset: u64) -> String {
    let mut batch = one_record_batch(&format!("EVT{base_offset:023}"));
    batch.base_offset = base_offset;
    let bytes = batch.encode().expect("batch encodes");
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
