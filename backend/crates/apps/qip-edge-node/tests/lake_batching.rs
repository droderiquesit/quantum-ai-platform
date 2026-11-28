//! Lake segments are sized at the seal, never merged (ADR 0181 decision 3,
//! R12-DATA-9187).
//!
//! The drain used to seal one lake segment per spool offset per partition,
//! so a cell's lake grew as one object per drained batch — a few hundred
//! bytes each, the tiny objects that make listing and replay slow and
//! costly. A compaction pass cannot fix that after the fact: a lake segment
//! is read by the key a pinned manifest names, and a merged object has no
//! place in it. So `LakeSink`, as the edge node deploys it
//! (`LakeBatching::deployed()` in `main.rs`), holds a partition's lines
//! across drained offsets and seals one segment per partition at a size or
//! age bound, and the drain commits its cursor at the first held offset so
//! a crash before the seal re-drains rather than loses what was held.
//!
//! Every test drives the production `Drain` over a real spool.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::governance::{Entitlement as Licence, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::lineage::Lineage;
use qip_core::{Clock, Decimal, Duration, EventId, ManualClock, ObjectId, Timestamp};
use qip_edge_node::event_fabric::drain::{
    Drain, DrainConfig, DrainedTo, LakeBatching, LakeSink, Sink, default_retention,
};
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::feed::SIMULATED_DATASET;
use qip_events::AnyEvent;
use qip_events::event_fabric::bindings::MARKET_EVENT_APPLIED;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::QosClass;
use qip_observability::metrics::Metrics;
use qip_storage::lake::Lake;
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::lake_history::HistoryQuery;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_storage::{BlobStore, MemoryBlobStore};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const CELL: &str = "edge-1";
/// Spool batches in every test: one tick each, alternating two instruments,
/// so the unbatched sink seals one object per batch.
const BATCHES: u64 = 40;
const MARKET_PREFIX: &str = "lake/class=market/";

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

fn instrument(sequence: u64) -> &'static str {
    if sequence.is_multiple_of(2) {
        "obj-ACME"
    } else {
        "obj-BETA"
    }
}

fn record(sequence: u64) -> Result<Record> {
    let message = MarketMessage::new(
        ObjectId::from_string(instrument(sequence)),
        Origin::new(VenueId::new("XLON"), "itch-a", 0, sequence),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: Decimal::from_int(100),
            quantity: Decimal::from_int(5),
            order_count: None,
        },
        at(sequence as i64),
        at(sequence as i64 + 1),
    );
    let hash = MarketEvent::hash_payload(&message)?;
    let event = MarketEvent::new(
        message,
        at(sequence as i64),
        at(sequence as i64 + 1),
        at(sequence as i64 + 1),
        Duration::ZERO,
        hash,
        Licence::Granted {
            dataset: SIMULATED_DATASET.into(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    let payload = serde_json::to_value(&event)?;
    let payload_hash = sha256_hex(qip_core::canonical::canonical_json(&payload).as_bytes());
    let id = format!("{CELL}/{sequence}");
    Record::from_any_event(
        &AnyEvent {
            event_id: EventId::from_string(id.clone()),
            topic: MARKET_EVENT_APPLIED.topic,
            schema_version: MARKET_EVENT_APPLIED.schema_version,
            occurred_at: at(sequence as i64),
            recorded_at: at(sequence as i64 + 1),
            sequence: 0,
            lineage: Lineage::root(qip_core::lineage::CorrelationId::from_string("c-1"), "test"),
            idempotency_key: Some(id),
            payload,
            payload_hash,
        },
        PayloadCodec::CanonicalJson,
    )
}

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A spool holding [`BATCHES`] one-tick market batches, sequences 1..=40.
fn spool(label: &str) -> Arc<SegmentLog> {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "qip-lake-batching-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(at(0)));
    let log = Arc::new(SegmentLog::open(dir, SegmentLogConfig::new(clock)).unwrap());
    for sequence in 1..=BATCHES {
        log.append(
            &Batch::new(
                MessageType::Data,
                QosClass::P2MarketJournal.batch_schema_id(),
                1,
                PayloadCodec::CanonicalJson,
                vec![record(sequence).unwrap()],
            )
            .unwrap(),
        )
        .unwrap();
    }
    log
}

fn sink(store: &Arc<dyn BlobStore>, batching: Option<LakeBatching>) -> LakeSink {
    let sink = LakeSink::new(
        Arc::clone(store),
        CELL,
        "europe-west2",
        default_retention(),
        SegmentCodec::Identity,
    )
    .unwrap();
    match batching {
        Some(batching) => sink.with_batching(batching),
        None => sink,
    }
}

/// A drain over `log` delivering to `sink`, taking up to 64 batches a step
/// as the node's does, with a seal cadence that never fires in a test.
fn drain(log: &Arc<SegmentLog>, sink: LakeSink) -> Drain {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(at(0)));
    let thresholds = Thresholds::new(
        1 << 40,
        Decimal::parse("0.5").unwrap(),
        Decimal::parse("0.9").unwrap(),
    )
    .unwrap();
    let (_gauge, _spool, publisher) =
        PressureGauge::new(thresholds, clock, Duration::from_secs(5)).unwrap();
    let telemetry = Arc::new(OutboxTelemetry::new(Arc::new(Metrics::new(
        "qip-edge-node",
    ))));
    Drain::open(
        Arc::clone(log),
        vec![Box::new(sink) as Box<dyn Sink>],
        publisher,
        telemetry,
        DrainConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_secs(3600),
            64,
        )
        .unwrap(),
    )
    .unwrap()
}

/// The lake's sealed market segments (raw slots excluded).
fn segments(store: &dyn BlobStore) -> Vec<String> {
    store
        .list(MARKET_PREFIX)
        .unwrap()
        .into_iter()
        .filter(|key| !key.ends_with(".raw"))
        .collect()
}

/// The lake cursor the drain committed to the spool's manifest.
fn committed(log: &SegmentLog) -> Option<u64> {
    log.manifest_get("drain.cursor.lake")
        .unwrap()
        .map(|bytes| String::from_utf8(bytes).unwrap().parse().unwrap())
}

static MANIFEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Every tick the lake holds, read back through a pinned manifest, as the
/// feed sequence of each line in storage order.
fn sequences_read_back(store: &dyn BlobStore) -> Vec<u64> {
    let granted = BTreeSet::from([SIMULATED_DATASET.to_string()]);
    let lake = Lake::new(store);
    let name = format!(
        "batching-{}",
        MANIFEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    lake.pin_market_history(
        &name,
        &HistoryQuery {
            from_date: "2000-01-01".to_string(),
            to_date: "2100-01-01".to_string(),
            venues: BTreeSet::new(),
            instruments: BTreeSet::new(),
            granted: granted.clone(),
            max_segments: 10_000,
            max_ticks: 100_000,
        },
    )
    .unwrap();
    let history = lake.read_market_history(&name, &granted, 100_000).unwrap();
    assert_eq!(history.unparsed, 0, "every sealed line parses");
    history
        .ticks
        .iter()
        .map(|tick| tick.message.origin.sequence)
        .collect()
}

/// Each sequence 1..=40 exactly once, whatever order storage holds them in.
fn every_tick_once() -> BTreeMap<u64, usize> {
    (1..=BATCHES).map(|sequence| (sequence, 1)).collect()
}

fn tally(sequences: &[u64]) -> BTreeMap<u64, usize> {
    let mut tally = BTreeMap::new();
    for sequence in sequences {
        *tally.entry(*sequence).or_insert(0) += 1;
    }
    tally
}

/// Forty one-tick spool batches seal, under the bounds the node deploys
/// with, as one segment per partition — two — that read back all forty
/// ticks, where the unbatched sink sealed forty. Until the seal, the drain's
/// committed cursor stays at the first held offset and the spool keeps it.
///
/// Mutation: make `LakeSink::deliver` take its unbatched branch whatever is
/// configured (`if self.batching.is_none()` to `if true`). The first step
/// seals a segment per batch and the held-not-sealed assertion fails.
#[test]
fn many_small_spool_batches_seal_as_one_segment_per_partition_and_read_back_every_tick() {
    let unbatched: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let log = spool("unbatched");
    let outcome = drain(&log, sink(&unbatched, None))
        .drain_to_tail(8)
        .unwrap();
    assert!(matches!(outcome, DrainedTo::Tail { .. }), "{outcome:?}");
    assert_eq!(
        segments(unbatched.as_ref()).len(),
        BATCHES as usize,
        "premise: sealed per spool offset, the lake holds one object per batch"
    );

    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let log = spool("deployed");
    let mut drain = drain(&log, sink(&store, Some(LakeBatching::deployed())));
    let step = drain.step().unwrap();
    assert_eq!(
        step.delivered, BATCHES as usize,
        "premise: every batch taken"
    );
    assert!(
        segments(store.as_ref()).is_empty(),
        "forty small batches are under the deployed bounds and are held, not sealed"
    );
    assert_eq!(
        committed(&log),
        Some(0),
        "the committed cursor waits at the first held offset"
    );

    let outcome = drain.drain_to_tail(8).unwrap();
    assert!(matches!(outcome, DrainedTo::Tail { .. }), "{outcome:?}");
    let sealed = segments(store.as_ref());
    assert_eq!(
        sealed.len(),
        2,
        "one segment per partition, not one per batch: {sealed:?}"
    );
    for key in &sealed {
        assert!(
            key.contains(&format!("/{CELL}-o{:020}-o{:020}", 0, BATCHES - 1)),
            "the segment id names the range of spool offsets it covers: {key}"
        );
    }
    // Partitioned by source, date and instrument: one segment under each
    // instrument's own partition of the venue and day.
    let date = at(0).to_date_string();
    let partitions: BTreeSet<String> = sealed
        .iter()
        .map(|key| key.rsplit_once('/').unwrap().0.to_string())
        .collect();
    assert_eq!(
        partitions,
        ["obj-ACME", "obj-BETA"]
            .iter()
            .map(|instrument| format!(
                "{MARKET_PREFIX}venue=XLON/date={date}/instrument={instrument}/\
                 entitlement={SIMULATED_DATASET}"
            ))
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(committed(&log), Some(BATCHES));
    assert_eq!(
        tally(&sequences_read_back(store.as_ref())),
        every_tick_once()
    );
}

/// The size bound cuts a long run of small batches into several segments
/// per partition before the drain-out, each read back once.
///
/// Mutation: drop `held.bytes >= batching.max_bytes ||` from
/// `LakeSink::seal_due`. Nothing seals until the drain-out, which seals two
/// segments, and the more-than-two assertion fails.
#[test]
fn the_size_bound_seals_before_the_drain_out_and_every_tick_reads_back_once() {
    // A line's size, measured from the unbatched lake rather than assumed.
    let probe: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    drain(&spool("probe"), sink(&probe, None))
        .drain_to_tail(8)
        .unwrap();
    let total: usize = segments(probe.as_ref())
        .iter()
        .map(|key| probe.get(key).unwrap().unwrap().len())
        .sum();
    let line = total / BATCHES as usize;
    assert!(line > 0, "premise: lines have a size");

    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let log = spool("size");
    let batching = LakeBatching::new(line * 8, std::time::Duration::from_secs(3600)).unwrap();
    let mut drain = drain(&log, sink(&store, Some(batching)));
    drain.step().unwrap();
    let before_tail = segments(store.as_ref()).len();
    assert!(
        before_tail > 2 && before_tail < BATCHES as usize,
        "the size bound sealed several segments, fewer than one per batch: {before_tail}"
    );
    let outcome = drain.drain_to_tail(8).unwrap();
    assert!(matches!(outcome, DrainedTo::Tail { .. }), "{outcome:?}");
    assert_eq!(
        tally(&sequences_read_back(store.as_ref())),
        every_tick_once()
    );
}

/// A quiet spool still seals: what the sink holds is sealed once the age
/// bound passes, on a step that delivers nothing.
///
/// The age is read on a `ManualClock` the test steps. It used to be read on
/// the process's own monotonic clock with a 1.5 s bound and a 1.6 s sleep,
/// and on a loaded CI runner the first step took longer than the bound and
/// sealed by age before the premise was checked (PR #298: "premise: held
/// under both bounds"). The test then said nothing about the property. Now
/// it fails only if the property does.
///
/// Mutations: remove the `seal_due` check at the top of each cursor's turn
/// in `Drain::step` (the quiet step seals nothing and the lake stays empty);
/// drop the age arm from `LakeSink::seal_due` (likewise); make
/// `LakeSink::with_age_clock` ignore its clock, so the bound is read on the
/// process's own clock again (the test's clock no longer moves it, and
/// nothing seals).
#[test]
fn the_age_bound_seals_what_a_quiet_spool_left_held() {
    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let log = spool("age");
    let clock = Arc::new(ManualClock::new(at(0)));
    let age_clock: Arc<dyn Clock> = clock.clone();
    let batching = LakeBatching::new(1 << 20, std::time::Duration::from_millis(1500)).unwrap();
    let mut drain = drain(&log, sink(&store, Some(batching)).with_age_clock(age_clock));
    drain.step().unwrap();
    assert!(
        segments(store.as_ref()).is_empty(),
        "premise: held under both bounds"
    );
    clock.advance(Duration::from_millis(1499));
    assert_eq!(drain.step().unwrap().delivered, 0, "premise: quiet");
    assert!(
        segments(store.as_ref()).is_empty(),
        "sealed before the age bound passed"
    );
    clock.advance(Duration::from_millis(1));
    let quiet = drain.step().unwrap();
    assert_eq!(quiet.delivered, 0, "premise: the spool is quiet");
    assert_eq!(segments(store.as_ref()).len(), 2, "sealed by age");
    assert_eq!(committed(&log), Some(BATCHES));
    assert_eq!(
        tally(&sequences_read_back(store.as_ref())),
        every_tick_once()
    );
}

/// A process that stops while the sink holds lines re-drains them on
/// restart: nothing held is lost with the process.
///
/// Mutation: make the drain's `committed` return `cursor.next`. The first
/// drain commits forty, the restarted one has nothing to deliver, and the
/// lake reads back no ticks.
#[test]
fn lines_held_when_the_process_stops_are_re_drained_and_sealed_once() {
    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let log = spool("crash");
    {
        let mut first = drain(&log, sink(&store, Some(LakeBatching::deployed())));
        assert_eq!(first.step().unwrap().delivered, BATCHES as usize);
        assert!(segments(store.as_ref()).is_empty(), "premise: all held");
        // The process stops here: the drain and its sink are dropped.
    }
    let outcome = drain(&log, sink(&store, Some(LakeBatching::deployed())))
        .drain_to_tail(8)
        .unwrap();
    assert!(matches!(outcome, DrainedTo::Tail { .. }), "{outcome:?}");
    assert_eq!(
        tally(&sequences_read_back(store.as_ref())),
        every_tick_once()
    );
}

/// A store that refuses every conditional write after the first `allow`.
#[derive(Debug)]
struct Failing {
    inner: MemoryBlobStore,
    allow: AtomicUsize,
}

impl BlobStore for Failing {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.inner.put(key, bytes)
    }
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.get(key)
    }
    fn delete(&self, key: &str) -> Result<bool> {
        self.inner.delete(key)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix)
    }
    fn put_if_absent(&self, key: &str, bytes: Vec<u8>) -> Result<bool> {
        if self
            .allow
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_err()
        {
            return Err(Error::io("the store went away mid-seal"));
        }
        self.inner.put_if_absent(key, bytes)
    }
}

/// A seal interrupted between two partitions' segments is finished, after a
/// restart, at the same boundary — even by a sink whose bounds would cut
/// elsewhere — so the partition already written is recognised as written
/// and no tick is sealed twice.
///
/// Mutation: remove the `resume_seal_through` call from `Drain::open`. The
/// restarted sink seals at its own one-byte bound, offset by offset, the
/// partition the interrupted seal wrote is written again under new ids, and
/// its ticks read back twice.
#[test]
fn a_seal_interrupted_between_partitions_is_finished_at_the_same_boundary() {
    let failing = Arc::new(Failing {
        inner: MemoryBlobStore::new(),
        allow: AtomicUsize::new(1),
    });
    let store: Arc<dyn BlobStore> = Arc::clone(&failing) as Arc<dyn BlobStore>;
    let log = spool("interrupted");
    {
        let mut first = drain(&log, sink(&store, Some(LakeBatching::deployed())));
        let outcome = first.drain_to_tail(2).unwrap();
        assert!(
            matches!(outcome, DrainedTo::Stalled { .. }),
            "premise: the seal failed after its first partition: {outcome:?}"
        );
        assert_eq!(
            segments(store.as_ref()).len(),
            1,
            "premise: one partition written"
        );
        assert_eq!(committed(&log), Some(0), "premise: nothing committed");
    }
    failing.allow.store(usize::MAX, Ordering::SeqCst);
    let eager = LakeBatching::new(1, std::time::Duration::from_secs(3600)).unwrap();
    let outcome = drain(&log, sink(&store, Some(eager)))
        .drain_to_tail(8)
        .unwrap();
    assert!(matches!(outcome, DrainedTo::Tail { .. }), "{outcome:?}");
    assert_eq!(
        segments(store.as_ref()).len(),
        2,
        "the resumed seal wrote only the partition the interrupted one did not"
    );
    assert_eq!(
        tally(&sequences_read_back(store.as_ref())),
        every_tick_once()
    );
}

/// A zero bound of either kind is refused: either seals every offset alone.
///
/// Mutation: drop either check in `LakeBatching::new`; that call returns
/// `Ok` and its assertion fails.
#[test]
fn a_zero_seal_bound_is_refused() {
    let error = LakeBatching::new(0, std::time::Duration::from_secs(60)).unwrap_err();
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("zero bytes"), "{error}");
    let error = LakeBatching::new(1 << 20, std::time::Duration::ZERO).unwrap_err();
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("age of zero"), "{error}");
}
