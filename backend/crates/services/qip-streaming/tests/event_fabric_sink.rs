//! FABRIC-082: the ledger sink retries indefinitely and never drops a
//! financial outcome (v2.1 §25's ledger-outage game day, at the library).

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use qip_core::error::{Error, Result};
use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::sink::{LedgerSink, OutcomeWriter, SinkStep, backoff_ms};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("qip-sink-{label}-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A fully declared policy: the one thing `Broker::declare_stream` takes in
/// place of the defaults a broker would otherwise supply (FABRIC-057).
fn policy() -> qip_events::event_fabric::policy::StreamPolicy {
    use qip_events::event_fabric::policy::{
        AckProfile, Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, StreamPolicy,
        StreamPolicySpec,
    };
    StreamPolicy::new(StreamPolicySpec {
        qos_class: qip_events::event_fabric::policy::QosClass::P1Outcomes,
        partition_key: "account".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::EventAnchored,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: AckProfile::Quorum,
        byte_quota_per_producer: 1_048_576,
        message_quota_per_producer: 10_000,
        lag_limit: 1_000,
        entitlement: Entitlement::new("internal-reflex", "trade").unwrap(),
        seal_age_ms: 500,
        peak_bytes_per_second: 5_000_000,
    })
    .unwrap()
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)))
}

fn outcome_batch(sequence: u64) -> Batch {
    let record = Record {
        event_id: format!("fill-{sequence}"),
        trace_id: None,
        source_timestamp_ns: sequence as i64,
        payload: format!("outcome-{sequence}").into_bytes(),
    };
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .unwrap();
    stamp_drain(&mut batch, "execution-node", 1, sequence);
    batch
}

/// A ledger whose store can be denied from outside, and which is idempotent
/// on `event_id` the way the real one must be: `deliveries` counts every
/// accepted call, `effects` counts what actually changed the book.
#[derive(Debug)]
struct Ledger {
    denied: Arc<AtomicBool>,
    deliveries: u64,
    effects: BTreeMap<String, u32>,
}

impl OutcomeWriter for Ledger {
    fn apply(&mut self, record: &Record) -> Result<()> {
        if self.denied.load(Ordering::SeqCst) {
            return Err(Error::io("the ledger store refuses writes"));
        }
        self.deliveries += 1;
        *self.effects.entry(record.event_id.clone()).or_insert(0) += 1;
        Ok(())
    }
}

fn open_broker(label: &str) -> Broker {
    let broker = Broker::open(temp_dir(label), clock()).unwrap();
    broker
        .declare_stream("outcomes", 1, policy(), SegmentLogConfig::new(clock()))
        .unwrap();
    broker
        .register_schema("outcomes", 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    broker
}

/// The game day: commit two outcomes, deny the store for a sustained
/// interval while more outcomes arrive, and judge the sink against the
/// requirement's five claims.
///
/// Mutation: in `LedgerSink::step`, call `broker.commit_offset` before the
/// `for record in &batch.records` loop (commit-then-write) — fails at the
/// "committed offset does not advance" assertion, because the checkpoint then
/// moves past a batch the ledger never accepted.
#[test]
fn a_sustained_ledger_outage_holds_the_checkpoint_skips_nothing_and_every_outcome_commits_once_in_effect()
 {
    let broker = open_broker("outage");
    let denied = Arc::new(AtomicBool::new(false));
    let ledger = Ledger {
        denied: Arc::clone(&denied),
        deliveries: 0,
        effects: BTreeMap::new(),
    };
    let mut sink = LedgerSink::new("ledger-sink", "outcomes", 0, ledger, 10, 1_000).unwrap();

    for sequence in 0..2 {
        broker
            .produce("outcomes", "book", outcome_batch(sequence))
            .unwrap();
    }
    assert_eq!(
        sink.step(&broker).unwrap(),
        SinkStep::Committed { offset: 0 }
    );
    assert_eq!(
        sink.step(&broker).unwrap(),
        SinkStep::Committed { offset: 1 }
    );
    assert_eq!(sink.step(&broker).unwrap(), SinkStep::Idle);
    // Premise: two outcomes are committed before the outage, so "does not
    // advance" is a claim about a checkpoint that had somewhere to go.
    assert_eq!(
        broker
            .committed_offset("ledger-sink", "outcomes", 0)
            .unwrap(),
        Some(1)
    );

    // The outage: sustained, with new outcomes arriving throughout.
    denied.store(true, Ordering::SeqCst);
    for sequence in 2..6 {
        broker
            .produce("outcomes", "book", outcome_batch(sequence))
            .unwrap();
    }
    let mut last_backoff = 0;
    let mut lags = Vec::new();
    for attempt in 1..=40u32 {
        match sink.step(&broker).unwrap() {
            SinkStep::Retrying {
                offset,
                attempts,
                backoff_ms,
            } => {
                assert_eq!(
                    offset, 2,
                    "the sink must keep retrying the same held outcome"
                );
                assert_eq!(attempts, attempt);
                assert!(backoff_ms >= last_backoff && backoff_ms <= 1_000);
                last_backoff = backoff_ms;
            }
            other => panic!("attempt {attempt}: a denied ledger must retry, got {other:?}"),
        }
        lags.push(sink.lag(&broker).unwrap());
    }
    assert_eq!(
        broker
            .committed_offset("ledger-sink", "outcomes", 0)
            .unwrap(),
        Some(1),
        "the committed offset must not advance while the ledger refuses writes"
    );
    assert_eq!(
        sink.retries(),
        40,
        "every refused write must move the retry metric"
    );
    assert_eq!(
        lags,
        vec![4; 40],
        "lag must show the four outcomes waiting behind the outage"
    );
    assert_eq!(
        last_backoff, 1_000,
        "the backoff must reach its cap and stay there, not give up"
    );
    assert_eq!(
        sink.writer().effects.len(),
        2,
        "nothing past the outage was applied"
    );

    // Restore: every held outcome commits, in order, once in effect.
    denied.store(false, Ordering::SeqCst);
    for expected in 2..6 {
        assert_eq!(
            sink.step(&broker).unwrap(),
            SinkStep::Committed { offset: expected }
        );
    }
    assert_eq!(sink.step(&broker).unwrap(), SinkStep::Idle);
    assert_eq!(sink.lag(&broker).unwrap(), 0);
    let applied: BTreeSet<String> = sink.writer().effects.keys().cloned().collect();
    let all: BTreeSet<String> = (0..6).map(|s| format!("fill-{s}")).collect();
    assert_eq!(applied, all, "no outcome may be skipped");
    assert!(
        sink.writer().effects.values().all(|n| *n == 1),
        "each outcome must have taken effect exactly once"
    );
}

/// The retry has no ceiling: after any number of failures the next step still
/// retries rather than giving up, and the wait is capped, not growing without
/// bound.
///
/// Mutation: in `LedgerSink::step`, return `Ok(SinkStep::Committed { offset })`
/// without committing once `self.attempts` passes a limit (skip-after-N) —
/// fails at the 1,001st step.
#[test]
fn a_thousand_consecutive_failures_still_retry_the_same_outcome_and_the_wait_stays_capped() {
    let broker = open_broker("no-ceiling");
    let denied = Arc::new(AtomicBool::new(true));
    let ledger = Ledger {
        denied,
        deliveries: 0,
        effects: BTreeMap::new(),
    };
    let mut sink = LedgerSink::new("ledger-sink", "outcomes", 0, ledger, 5, 500).unwrap();
    broker
        .produce("outcomes", "book", outcome_batch(0))
        .unwrap();
    for _ in 0..1_001 {
        match sink.step(&broker).unwrap() {
            SinkStep::Retrying {
                offset: 0,
                backoff_ms,
                ..
            } => assert!(backoff_ms <= 500),
            other => panic!("expected a retry of offset 0, got {other:?}"),
        }
    }
    assert_eq!(
        broker
            .committed_offset("ledger-sink", "outcomes", 0)
            .unwrap(),
        None
    );
}

#[test]
fn the_backoff_doubles_to_its_cap_and_a_huge_attempt_count_does_not_overflow() {
    assert_eq!(backoff_ms(1, 10, 1_000), 10);
    assert_eq!(backoff_ms(2, 10, 1_000), 20);
    assert_eq!(backoff_ms(7, 10, 1_000), 640);
    assert_eq!(backoff_ms(8, 10, 1_000), 1_000);
    assert_eq!(backoff_ms(u32::MAX, 10, 1_000), 1_000);
    assert!(
        LedgerSink::new(
            "g",
            "outcomes",
            0,
            Ledger {
                denied: Arc::new(AtomicBool::new(false)),
                deliveries: 0,
                effects: BTreeMap::new(),
            },
            0,
            10
        )
        .is_err()
    );
}
