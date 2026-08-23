//! The properties a managed message bus would have provided.
//!
//! ADR 0011 says the cost of not buying Pub/Sub is that "retries, backpressure,
//! at-least-once delivery, dead-lettering, ordering guarantees" become code
//! here that has to be right. This file is the evidence for four of those. The
//! fifth, at-least-once, is in `mesh.rs` because it needs both ends.
//!
//! Two things make these assertions possible at all, and both are the
//! platform's no-ambient-anything rule doing real work: the backoff jitter
//! comes from a seeded `Xoshiro256`, so a schedule is reproducible, and the
//! waiting goes through an injected sleeper, so a test can read the schedule
//! without spending it.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{Action, TestServer, address_with_no_listener};
use qip_core::error::Result;
use qip_core::{Clock, Duration, ManualClock, Timestamp, Xoshiro256};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_transport::{
    DeadLetterReason, DeadLetterSink, MemoryDeadLetters, MeshConfig, MeshPublisher,
    RecordingSleeper, RetryPolicy, Sleeper, TransportError,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration as StdDuration;

/// 2024-01-02T14:24:05Z, so a rendered timestamp in a failure is legible.
const BASE_NANOS: i64 = 1_704_205_445_000_000_000;

fn at(millis: i64) -> Timestamp {
    Timestamp::from_nanos(BASE_NANOS + millis * 1_000_000)
}

/// A regional state delta: what a cell publishes up to the global brain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RegionalDelta {
    region: String,
    sequence: u64,
    opportunities: u32,
}

impl EventBody for RegionalDelta {
    // A `Discover` topic: not latency-critical, not lossy-tolerable, so it may
    // cross a network hop.
    const TOPIC: Topic = Topic::OpportunityDetected;
    const SCHEMA_VERSION: u32 = 1;

    /// The natural key of the fact, so a redelivery of the same delta under a
    /// new event id is still recognisable as the same delta.
    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.region, self.sequence))
    }
}

/// A market tick, which must never cross the mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Quote {
    price: i64,
}

impl EventBody for Quote {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;
}

fn frame_of<T: EventBody>(label: &str, body: T, at_time: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        qip_core::Id::from_string(format!("EVT{label:0>23}")),
        at_time,
        at_time,
        qip_core::Lineage::root(
            qip_core::CorrelationId::from_string(format!("COR{label:0>23}")),
            "qip-transport-tests",
        ),
        body,
    )
    .erase()
}

fn delta(region: &str, sequence: u64) -> Result<AnyEvent> {
    frame_of(
        &format!("{region}{sequence}"),
        RegionalDelta {
            region: region.to_string(),
            sequence,
            opportunities: 3,
        },
        at(sequence as i64),
    )
}

/// A publisher against `peer`, with a recording sleeper so the retry ladder is
/// readable, a manual clock so dead letters are stamped deterministically, and
/// an inspectable dead-letter sink.
struct Harness {
    publisher: MeshPublisher,
    sleeper: Arc<RecordingSleeper>,
    letters: Arc<std::sync::Mutex<MemoryDeadLetters>>,
}

impl Harness {
    fn build(peer: &str, retry: RetryPolicy, queue_capacity: usize) -> Result<Self> {
        let sleeper = Arc::new(RecordingSleeper::new());
        let letters = Arc::new(std::sync::Mutex::new(MemoryDeadLetters::new(16)));
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(at(500)));
        let config = MeshConfig::new("mesh:us-east", peer)
            .with_retry(retry)
            .with_queue_capacity(queue_capacity)
            .with_seed(20_240_102);
        let publisher = MeshPublisher::new(
            config,
            clock,
            sleeper.clone() as Arc<dyn Sleeper>,
            Box::new(letters.clone()),
        )?;
        Ok(Self {
            publisher,
            sleeper,
            letters,
        })
    }

    fn letters(&self) -> Vec<qip_transport::DeadLetter> {
        self.letters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .letters()
            .into_iter()
            .cloned()
            .collect()
    }
}

/// A ladder short enough to read: three attempts, 10ms, doubling, no jitter.
fn plain_ladder() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(100),
        multiplier: 2,
        jitter_basis_points: 0,
    }
}

fn quick_limits() -> qip_transport::ClientLimits {
    qip_transport::ClientLimits {
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(300),
        write_timeout: StdDuration::from_millis(500),
        ..qip_transport::ClientLimits::default()
    }
}

// --- the backoff ladder -------------------------------------------------

#[test]
fn the_ladder_is_exponential_and_never_exceeds_its_own_cap() {
    // The cap being a real cap is the whole reason jitter subtracts rather
    // than adds. A maximum backoff that can be exceeded is a number that gets
    // quoted in a runbook and is then wrong.
    let policy = RetryPolicy {
        max_attempts: 12,
        initial_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_secs(2),
        multiplier: 3,
        jitter_basis_points: 2_500,
    };
    policy.validate().expect("a sane policy was refused");

    for seed in [0u64, 1, 7, 4_242, u64::MAX] {
        let mut rng = Xoshiro256::seeded(seed);
        let mut previous_ceiling = 0i64;
        for attempt in 1..=12u32 {
            let backoff = policy.backoff(attempt, &mut rng);
            assert!(
                backoff <= policy.max_backoff,
                "attempt {attempt} at seed {seed} waited {backoff:?}, past the {:?} cap",
                policy.max_backoff
            );
            assert!(
                backoff.as_nanos() >= 0,
                "attempt {attempt} at seed {seed} produced a negative wait"
            );
            // Jitter can only pull below the exponential value, so the
            // undiscounted ceiling grows monotonically until it is capped.
            let ceiling = (20 * 3i64.saturating_pow(attempt - 1)).min(2_000);
            assert!(
                backoff.as_millis() <= ceiling,
                "attempt {attempt} at seed {seed} waited {backoff:?}, above the exponential value \
                 of {ceiling}ms — jitter must never add"
            );
            assert!(ceiling >= previous_ceiling);
            previous_ceiling = ceiling;
        }
    }
}

#[test]
fn without_jitter_the_ladder_is_exactly_the_documented_sequence() {
    let policy = plain_ladder();
    let mut rng = Xoshiro256::seeded(1);
    let ladder: Vec<i64> = (1..=5)
        .map(|n| policy.backoff(n, &mut rng).as_millis())
        .collect();
    assert_eq!(
        ladder,
        vec![10, 20, 40, 80, 100],
        "the ladder must double from the initial backoff and then sit on the cap"
    );
}

#[test]
fn jitter_is_deterministic_from_the_seed_and_differs_between_seeds() {
    // The reason the seed is injected rather than drawn from the environment:
    // nine cells reconnecting after a rollout must not retry on the same
    // millisecond, and a replay of one cell must reproduce its own timing.
    let policy = RetryPolicy {
        jitter_basis_points: 5_000,
        ..plain_ladder()
    };
    let ladder = |seed: u64| -> Vec<i64> {
        let mut rng = Xoshiro256::seeded(seed);
        (1..=6)
            .map(|n| policy.backoff(n, &mut rng).as_nanos())
            .collect()
    };

    assert_eq!(
        ladder(99),
        ladder(99),
        "the same seed produced two schedules"
    );
    assert_ne!(
        ladder(99),
        ladder(100),
        "two cells with different seeds drew the same schedule, so the jitter is not spreading \
         anything"
    );

    // And the jitter stays inside the half it was told to use.
    let mut rng = Xoshiro256::seeded(7);
    for attempt in 1..=6 {
        let waited = policy.backoff(attempt, &mut rng).as_nanos();
        let undiscounted = (10i64 * 2i64.pow(attempt - 1)).min(100) * 1_000_000;
        assert!(
            waited >= undiscounted / 2 && waited <= undiscounted,
            "attempt {attempt} waited {waited}ns, outside the 50% jitter band below {undiscounted}ns"
        );
    }
}

#[test]
fn a_policy_that_cannot_do_what_it_says_is_refused_at_construction() {
    // Each of these presents in production as "messages are never sent" or
    // "the maximum backoff is not the maximum", discovered during the incident
    // the retry existed for.
    let base = plain_ladder();
    for (policy, why) in [
        (
            RetryPolicy {
                max_attempts: 0,
                ..base
            },
            "zero attempts",
        ),
        (
            RetryPolicy {
                multiplier: 0,
                ..base
            },
            "a zero multiplier",
        ),
        (
            RetryPolicy {
                initial_backoff: Duration::from_secs(10),
                ..base
            },
            "an initial backoff above the cap",
        ),
        (
            RetryPolicy {
                jitter_basis_points: 20_000,
                ..base
            },
            "jitter that would go below zero",
        ),
    ] {
        assert!(policy.validate().is_err(), "{why} was accepted");
    }
}

// --- retry against a real peer ------------------------------------------

#[test]
fn a_peer_that_fails_twice_then_succeeds_is_delivered_on_the_third_attempt() -> Result<()> {
    // The case the whole retry ladder exists for: a peer restarting during a
    // rollout. Two 503s, then it is back.
    let server = TestServer::spawn(|index, _| {
        if index < 2 {
            Action::json(503, r#"{"error":"the inbox is full"}"#)
        } else {
            Action::json(
                200,
                r#"{"accepted":1,"duplicates":[],"position":9,"depth":1}"#,
            )
        }
    });

    let mut harness = Harness::build(&server.url(), plain_ladder(), 16)?;
    let delivery = harness
        .publisher
        .publish_frame(delta("us-east", 1)?, at(100))
        .expect("a peer that came back was not retried into success");

    assert_eq!(
        delivery.attempts, 3,
        "delivery must report how many sends it took"
    );
    assert_eq!(delivery.peer_position, Some(9));
    assert_eq!(
        delivery.accepted_at,
        at(100),
        "the receipt carries the caller's clock"
    );
    assert_eq!(
        server.served(),
        3,
        "the peer saw the wrong number of requests"
    );

    // Two failures, so two waits, and they are the documented ladder.
    assert_eq!(
        harness
            .sleeper
            .recorded()
            .iter()
            .map(|d| d.as_millis())
            .collect::<Vec<_>>(),
        vec![10, 20],
        "the backoff between attempts is not the policy's ladder"
    );

    let stats = harness.publisher.stats();
    assert_eq!(stats.attempts, 3);
    assert_eq!(stats.retries, 2);
    assert_eq!(stats.delivered, 1);
    assert_eq!(stats.dead_lettered, 0);
    assert!(harness.letters().is_empty());
    Ok(())
}

#[test]
fn the_retry_count_is_bounded_and_the_message_lands_in_the_dead_letter_log() -> Result<()> {
    // A peer that never comes back. The point is that this terminates: an
    // unbounded retry is a queue that grows until the process dies, with no
    // record of what was in it.
    let server = TestServer::always(Action::json(503, r#"{"error":"still full"}"#));
    let mut harness = Harness::build(&server.url(), plain_ladder(), 16)?;

    let frame = delta("us-east", 4)?;
    let key = frame.dedup_key();
    let error = harness
        .publisher
        .publish_frame(frame, at(100))
        .expect_err("a peer that never answered reported a delivery");

    assert_eq!(error.code(), "dead_lettered");
    assert!(
        !error.is_retryable_by_caller(),
        "the transport has already spent its whole ladder; telling the caller to try again would \
         double it"
    );
    // The error a caller holds must name the same message the sink recorded,
    // by the same key, or the two records cannot be joined during an incident.
    match &error {
        TransportError::DeadLettered {
            key: reported,
            attempts,
            reason,
            last_error,
        } => {
            assert_eq!(reported, &key);
            assert_eq!(*attempts, 3);
            assert_eq!(*reason, DeadLetterReason::RetriesExhausted);
            assert!(
                last_error.contains("503"),
                "the error lost the peer's answer"
            );
        }
        other => panic!("expected a dead-letter refusal, got {other:?}"),
    }
    assert_eq!(
        server.served(),
        3,
        "the peer saw {} requests against a three-attempt policy",
        server.served()
    );
    assert_eq!(
        harness.sleeper.recorded().len(),
        2,
        "three attempts means two waits, not three"
    );

    let letters = harness.letters();
    assert_eq!(letters.len(), 1, "the message was not recorded anywhere");
    let letter = letters.first().expect("one letter");
    assert_eq!(
        letter.key, key,
        "the letter must be findable by idempotency key"
    );
    assert_eq!(letter.reason, DeadLetterReason::RetriesExhausted);
    assert_eq!(letter.attempts, 3);
    assert_eq!(
        letter.recorded_at,
        at(500),
        "the dead letter is stamped from the injected clock, not a wall clock"
    );
    assert!(
        letter.last_error.contains("503"),
        "'why did this delta never arrive' must be answerable from the letter: {}",
        letter.summary()
    );
    Ok(())
}

#[test]
fn the_dead_letter_keeps_the_whole_frame_so_it_can_be_sent_again() -> Result<()> {
    let server = TestServer::always(Action::json(503, "{}"));
    let mut harness = Harness::build(&server.url(), plain_ladder(), 16)?;

    let original = delta("eu-west", 11)?;
    let _ = harness.publisher.publish_frame(original.clone(), at(100));

    let letters = harness.letters();
    let recovered = &letters.first().expect("a letter was recorded").frame;
    assert_eq!(
        recovered, &original,
        "a summary is not enough: re-driving a dead letter needs the message itself"
    );
    // And it is still decodable, so a redrive does not have to reconstruct it.
    let body: Envelope<RegionalDelta> = recovered.decode()?;
    assert_eq!(body.body.sequence, 11);
    Ok(())
}

#[test]
fn a_permanent_failure_is_not_retried_at_all() -> Result<()> {
    // A 400 means the peer read the request and will read it the same way
    // next time. Spending the ladder on it delays every message behind it.
    let server = TestServer::always(Action::json(400, r#"{"error":"unreadable batch"}"#));
    let mut harness = Harness::build(&server.url(), plain_ladder(), 16)?;

    let error = harness
        .publisher
        .publish_frame(delta("ap-north", 2)?, at(100))
        .expect_err("a 400 was reported as a delivery");
    assert_eq!(error.code(), "dead_lettered");

    assert_eq!(server.served(), 1, "a permanent failure was retried");
    assert!(
        harness.sleeper.recorded().is_empty(),
        "a permanent failure spent backoff it had no reason to spend"
    );
    assert_eq!(
        harness.letters().first().map(|letter| letter.reason),
        Some(DeadLetterReason::PermanentFailure),
        "the reason must say it was permanent, not that the retries ran out"
    );
    Ok(())
}

#[test]
fn a_peer_that_rejects_on_the_merits_is_recorded_as_rejected() -> Result<()> {
    let server = TestServer::always(Action::json(422, r#"{"error":"payload hash mismatch"}"#));
    let mut harness = Harness::build(&server.url(), plain_ladder(), 16)?;

    let _ = harness
        .publisher
        .publish_frame(delta("us-west", 3)?, at(100));
    assert_eq!(server.served(), 1);
    assert_eq!(
        harness.letters().first().map(|letter| letter.reason),
        Some(DeadLetterReason::Rejected)
    );
    Ok(())
}

#[test]
fn a_peer_that_is_not_listening_is_retried_and_then_dead_lettered() -> Result<()> {
    let address = address_with_no_listener();
    let mut harness = Harness::build(&address, plain_ladder(), 16)?;

    let error = harness
        .publisher
        .publish_frame(delta("us-east", 5)?, at(100))
        .expect_err("publishing into a closed port reported a delivery");
    assert_eq!(error.code(), "dead_lettered");
    assert_eq!(harness.publisher.stats().attempts, 3);
    assert_eq!(
        harness.letters().first().map(|letter| letter.reason),
        Some(DeadLetterReason::RetriesExhausted)
    );
    Ok(())
}

#[test]
fn a_slow_peer_trips_the_read_timeout_and_is_retried_rather_than_waited_on() -> Result<()> {
    // Two responses that arrive well after the client's 300ms read timeout,
    // then one that is prompt. Without the timeout the first attempt never
    // returns and the retry ladder is never reached.
    let server = TestServer::spawn(|index, _| {
        if index < 2 {
            Action::Silent(StdDuration::from_millis(900))
        } else {
            Action::json(
                200,
                r#"{"accepted":1,"duplicates":[],"position":1,"depth":1}"#,
            )
        }
    });

    let sleeper = Arc::new(RecordingSleeper::new());
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(at(500)));
    let config = MeshConfig::new("mesh:slow", server.url())
        .with_retry(plain_ladder())
        .with_limits(quick_limits());
    let mut publisher = MeshPublisher::new(
        config,
        clock,
        sleeper.clone() as Arc<dyn Sleeper>,
        Box::new(MemoryDeadLetters::new(4)),
    )?;

    let started = std::time::Instant::now();
    let delivery = publisher
        .publish_frame(delta("us-east", 6)?, at(100))
        .expect("a peer that eventually answered was not retried into success");
    let elapsed = started.elapsed();

    assert_eq!(delivery.attempts, 3);
    assert!(
        elapsed < StdDuration::from_millis(1_800),
        "two attempts against a 900ms peer took {elapsed:?}, so the 300ms read timeout is not \
         bounding them"
    );
    Ok(())
}

// --- backpressure -------------------------------------------------------

#[test]
fn a_full_outbound_queue_refuses_by_name_rather_than_growing() -> Result<()> {
    // Nothing is ever sent here: the peer is a black hole, so the queue is the
    // only thing that can say no.
    let address = address_with_no_listener();
    let mut harness = Harness::build(&address, plain_ladder(), 2)?;

    harness
        .publisher
        .enqueue(delta("us-east", 1)?)
        .expect("first");
    harness
        .publisher
        .enqueue(delta("us-east", 2)?)
        .expect("second");
    assert_eq!(harness.publisher.queue_depth(), 2);

    let error = harness
        .publisher
        .enqueue(delta("us-east", 3)?)
        .expect_err("a queue at capacity accepted a third message");

    match &error {
        TransportError::QueueFull { capacity, .. } => assert_eq!(*capacity, 2),
        other => panic!("expected a named queue-full refusal, got {other:?}"),
    }
    assert!(
        error.is_retryable_by_caller(),
        "a full queue is the one refusal the caller can act on by trying again later"
    );
    assert!(
        error.to_string().contains("refuses rather than growing"),
        "the refusal must say why it is a refusal and not a drop: {error}"
    );
    assert_eq!(
        harness.publisher.queue_depth(),
        2,
        "the refused message must not have displaced one already queued: unlike the hot-path \
         queue, nothing here is replaceable"
    );
    assert_eq!(harness.publisher.queue_stats().refused, 1);

    // And it crosses into the platform vocabulary as a guard, not as I/O:
    // nothing failed at the socket, a limit declined.
    let platform: qip_core::Error = error.into();
    assert_eq!(platform.code(), "guard");
    Ok(())
}

#[test]
fn a_batch_that_does_not_fit_is_refused_whole() -> Result<()> {
    // The failure `qip_streaming::Publisher::publish_batch` documents: a
    // transport that took half a batch and reported an error leaves the caller
    // unable to say which half.
    let address = address_with_no_listener();
    let mut harness = Harness::build(&address, plain_ladder(), 3)?;

    let batch = vec![
        delta("us-east", 1)?,
        delta("us-east", 2)?,
        delta("us-east", 3)?,
        delta("us-east", 4)?,
    ];
    let error = harness
        .publisher
        .publish_frames(batch, at(100))
        .expect_err("a batch larger than the queue was accepted");

    assert_eq!(error.code(), "queue_full");
    assert_eq!(
        harness.publisher.queue_depth(),
        0,
        "nothing may be admitted from a batch that is going to be refused"
    );
    assert_eq!(harness.publisher.stats().enqueued, 0);
    Ok(())
}

#[test]
fn a_queue_with_no_room_at_all_is_refused_at_construction() {
    let address = address_with_no_listener();
    assert!(
        Harness::build(&address, plain_ladder(), 0).is_err(),
        "a zero-capacity queue would present as a transport that has never delivered anything"
    );
}

// --- dead letters -------------------------------------------------------

#[test]
fn the_dead_letter_store_is_bounded_and_says_when_it_has_lost_letters() -> Result<()> {
    // Uncomfortable and deliberate: an unbounded dead-letter store turns a long
    // outage into the out-of-memory kill the bounded queue exists to prevent,
    // and takes every earlier letter with it. What is not acceptable is losing
    // them silently.
    let server = TestServer::always(Action::json(400, "{}"));
    let sleeper = Arc::new(RecordingSleeper::new());
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(at(500)));
    let letters = Arc::new(std::sync::Mutex::new(MemoryDeadLetters::new(2)));
    let mut publisher = MeshPublisher::new(
        MeshConfig::new("mesh:small", server.url()).with_retry(plain_ladder()),
        clock,
        sleeper as Arc<dyn Sleeper>,
        Box::new(letters.clone()),
    )?;

    for sequence in 1..=5 {
        let _ = publisher.publish_frame(delta("us-east", sequence)?, at(100));
    }

    let held = letters.lock().unwrap_or_else(|p| p.into_inner());
    assert_eq!(held.len(), 2, "the store grew past its capacity");
    assert_eq!(
        held.recorded(),
        5,
        "the count of what was recorded must be complete"
    );
    assert_eq!(
        held.evicted(),
        3,
        "an incomplete record must say how incomplete it is"
    );
    // The newest are kept: during an outage the newest messages are the ones
    // still worth re-sending.
    assert!(held.find("opportunity.detected:us-east:5").is_some());
    assert!(held.find("opportunity.detected:us-east:1").is_none());
    Ok(())
}

// --- what may not travel ------------------------------------------------

#[test]
fn a_market_tick_may_not_cross_the_mesh() -> Result<()> {
    // The mirror of `qip_streaming::durable`'s refusal, at the other extreme:
    // a network hop with a retry ladder and a dead-letter path in front of a
    // decision measured in microseconds is the latency the edge exists to
    // avoid, and the tick is replaceable within milliseconds anyway.
    let server = TestServer::always(Action::json(200, "{}"));
    let mut harness = Harness::build(&server.url(), plain_ladder(), 16)?;

    let tick = frame_of("Q1", Quote { price: 101 }, at(1))?;
    assert!(
        MeshPublisher::refusal(&tick).is_some(),
        "a router must be able to ask before it builds a batch"
    );

    let error = harness
        .publisher
        .enqueue(tick)
        .expect_err("a venue-critical tick was queued for a network hop");
    assert_eq!(error.code(), "inadmissible");
    assert_eq!(
        server.served(),
        0,
        "the refusal must happen before any send"
    );

    // And a state delta is admissible, so the check is not refusing everything.
    assert!(MeshPublisher::refusal(&delta("us-east", 1)?).is_none());
    Ok(())
}

#[test]
fn the_descriptor_names_what_production_must_still_supply() -> Result<()> {
    let address = address_with_no_listener();
    let harness = Harness::build(&address, plain_ladder(), 4)?;
    let descriptor = harness.publisher.descriptor();

    assert!(
        descriptor.available,
        "this transport does work in this build"
    );
    assert!(
        !descriptor.durable,
        "the outbound queue and the default dead-letter sink are in memory, and claiming \
         durability would be a claim a pod restart disproves"
    );
    let requirement = descriptor
        .production_requirement
        .expect("a transport with no TLS must say so in its descriptor");
    for expected in ["TLS", "NetworkPolicy", "durable", "dead-letter"] {
        assert!(
            requirement.contains(expected),
            "the requirement list does not mention {expected}: {requirement}"
        );
    }
    Ok(())
}

#[test]
fn the_recording_sleeper_reports_what_a_wall_clock_would_have_spent() {
    // The sleeper is what makes the ladder assertable. It is worth one test of
    // its own, because a sleeper that quietly recorded nothing would make every
    // backoff assertion above vacuously pass.
    let sleeper = RecordingSleeper::new();
    sleeper.sleep(Duration::from_millis(10));
    sleeper.sleep(Duration::from_millis(25));
    assert_eq!(sleeper.recorded().len(), 2);
    assert_eq!(sleeper.total().as_millis(), 35);
}
