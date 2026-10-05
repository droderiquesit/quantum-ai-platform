//! The platform's own outcomes reach the Tick/Internal Lake (blueprint
//! TICK-065).
//!
//! The lake existed, every composition root archived its event log, and no
//! order, fill or verdict ever reached the lake, because nothing connected the
//! two. The connection is [`ChainArchive::with_outcome_lake`]: the hand-over
//! that archives the log also seals what only this platform has into the
//! lake's internal class. These tests hold the three properties that make
//! that a record rather than a copy — only outcomes, never lost to a failed
//! write, never refused for having half-succeeded.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{CorrelationId, EventId, Lineage, Timestamp};
use qip_events::envelope::{AnyEvent, canonical_json};
use qip_events::log::{EventLog, LogRecord};
use qip_events::topic::Topic;
use qip_storage::lake::{INTERNAL_ENTITLEMENT, Lake};
use qip_storage::{BlobStore, ChainArchive, MemoryBlobStore, MemoryKeyValueStore};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

fn day_one() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn event(n: u64, topic: Topic, at: Timestamp) -> AnyEvent {
    let payload = serde_json::json!({ "n": n });
    AnyEvent {
        event_id: EventId::from_string(format!("EVT{n:023}")),
        topic,
        schema_version: 1,
        occurred_at: at,
        recorded_at: at,
        sequence: 0,
        lineage: Lineage::root(
            CorrelationId::from_string("COR00000000000000000000001"),
            "outcome-lake-test",
        ),
        idempotency_key: None,
        payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    }
}

/// A run holding what the platform did and what the world sent, interleaved:
/// a tick, a fill, a risk verdict, a news item, and on the next day an order
/// and a lesson.
fn a_mixed_run() -> Result<Vec<LogRecord>> {
    let day_two = Timestamp::from_secs(1_760_000_000 + 86_400);
    let mut log = EventLog::in_memory();
    log.append(&event(1, Topic::MarketTick, day_one()))?;
    log.append(&event(2, Topic::OrderFilled, day_one()))?;
    log.append(&event(3, Topic::RiskRejected, day_one()))?;
    log.append(&event(4, Topic::NewsReceived, day_one()))?;
    log.append(&event(5, Topic::OrderSubmitted, day_two))?;
    // Filed as an episode, not as "irreplaceable", and as permanent as a fill:
    // the cycle journal is recorded this way, and a filter on the one row of
    // the retention table kept the fills and dropped the decisions.
    log.append(&event(6, Topic::LessonRecorded, day_two))?;
    Ok(log.records().to_vec())
}

/// Every record the lake's internal class holds, by source sequence, after
/// checking each line is the record exactly as the log wrote it.
fn internal_sequences(blobs: &dyn BlobStore) -> Result<Vec<u64>> {
    let mut sequences = Vec::new();
    for key in blobs.list("lake/class=internal/")? {
        let bytes = blobs.get(&key)?.expect("a listed segment is readable");
        for line in String::from_utf8(bytes).expect("utf-8").lines() {
            let record: LogRecord = serde_json::from_str(line)?;
            assert_eq!(
                line,
                canonical_json(&serde_json::to_value(&record)?),
                "the lake line is not the record the log wrote"
            );
            sequences.push(record.sequence);
        }
    }
    sequences.sort_unstable();
    Ok(sequences)
}

fn archive_with_lake(blobs: Arc<dyn BlobStore>) -> Result<ChainArchive> {
    Ok(ChainArchive::open(Arc::new(MemoryKeyValueStore::new()))?.with_outcome_lake(blobs))
}

/// The opposite failure matters as much as the missing write: a writer that
/// copied the whole slice would file a tick and a news item as internal
/// history, which is the world-data path leaking into the one class that is
/// never discarded.
#[test]
fn a_hand_over_seals_the_platforms_own_outcomes_into_the_internal_lake_and_nothing_the_world_sent()
-> Result<()> {
    let records = a_mixed_run()?;
    // Premise: the slice holds both kinds, so "only outcomes" is a filter
    // that had something to remove and something to keep.
    let own: Vec<u64> = records
        .iter()
        .filter(|record| record.event.topic.requires_permanent_retention())
        .map(|record| record.sequence)
        .collect();
    assert_eq!(
        own,
        vec![2, 3, 5, 6],
        "premise: four of the six are outcomes"
    );

    let blobs = Arc::new(MemoryBlobStore::new());
    let archive = archive_with_lake(blobs.clone())?;
    assert!(blobs.list("lake/")?.is_empty(), "premise: an empty lake");

    assert_eq!(
        archive.absorb(&records)?,
        6,
        "the ledger half takes all six"
    );
    assert_eq!(internal_sequences(blobs.as_ref())?, own);
    assert!(
        blobs.list("lake/class=market/")?.is_empty(),
        "an outcome was filed as market history"
    );
    // Two days, two partitions, each under the internal entitlement.
    let keys = blobs.list("lake/class=internal/")?;
    assert_eq!(keys.len(), 2, "{keys:?}");
    for (key, date) in keys.iter().zip(["2025-10-09", "2025-10-10"]) {
        assert!(key.contains(&format!("/date={date}/")), "{key}");
        assert!(
            key.contains(&format!("/entitlement={INTERNAL_ENTITLEMENT}/")),
            "{key}"
        );
    }

    // Handing the same slice over again seals nothing twice.
    assert_eq!(archive.absorb(&records)?, 0);
    assert_eq!(blobs.list("lake/")?.len(), 2);
    assert_eq!(internal_sequences(blobs.as_ref())?, own);
    Ok(())
}

/// An archive nobody gave a lake writes none, so the test above is not
/// passing on something `absorb` does unconditionally.
#[test]
fn an_archive_given_no_lake_archives_the_log_and_seals_nothing() -> Result<()> {
    let records = a_mixed_run()?;
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::new()))?;
    assert_eq!(archive.absorb(&records)?, 6);
    assert_eq!(archive.len()?, 6);
    Ok(())
}

/// A blob store whose first `refusals` writes fail.
#[derive(Debug)]
struct Faulty {
    inner: MemoryBlobStore,
    refusals: AtomicU64,
}

impl BlobStore for Faulty {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        if self.refusals.load(Ordering::Relaxed) > 0 {
            self.refusals.fetch_sub(1, Ordering::Relaxed);
            return Err(Error::io("the lake is unreachable"));
        }
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
}

/// The order of the two writes is the property. Chain first, and a lake that
/// failed would return an error with the watermark already past the fill: the
/// next hand-over skips it, and the fill is in the ledger and never in the
/// lake, with nothing anywhere saying so.
#[test]
fn a_lake_that_refuses_the_write_stops_the_hand_over_before_the_ledger_moves_and_the_retry_loses_nothing()
-> Result<()> {
    let records = a_mixed_run()?;
    let blobs = Arc::new(Faulty {
        inner: MemoryBlobStore::new(),
        refusals: AtomicU64::new(1),
    });
    let archive = archive_with_lake(blobs.clone())?;

    let refused = archive
        .absorb(&records)
        .expect_err("a lake that cannot be written must fail the hand-over");
    assert!(refused.message().contains("unreachable"), "{refused}");
    assert_eq!(
        archive.len()?,
        0,
        "the ledger moved past a failed lake write"
    );
    assert_eq!(archive.absorbed_through(), 0);

    assert_eq!(archive.absorb(&records)?, 6);
    assert_eq!(internal_sequences(blobs.as_ref())?, vec![2, 3, 5, 6]);
    Ok(())
}

/// A hand-over whose lake write landed and whose first ledger write did not
/// is retried under the same segment id. Refusing that as an overwrite would
/// stop every later hand-over for good; accepting a different body under the
/// id would be the overwrite the lake exists to refuse.
#[test]
fn sealing_the_same_outcomes_under_the_same_id_is_not_an_overwrite_and_a_different_body_is()
-> Result<()> {
    let records = a_mixed_run()?;
    let all: Vec<&LogRecord> = records.iter().collect();
    let blobs = MemoryBlobStore::new();
    let lake = Lake::new(&blobs);

    let first = lake.seal_internal_outcomes("chain-0-4", &all)?;
    assert_eq!(first.len(), 2, "premise: something was sealed");
    assert_eq!(lake.seal_internal_outcomes("chain-0-4", &all)?, first);
    assert_eq!(blobs.list("lake/")?.len(), 2);

    // Without the risk verdict the first day's body differs.
    let fewer = [all[1], all[4]];
    let refused = lake
        .seal_internal_outcomes("chain-0-4", &fewer)
        .expect_err("a different body under a sealed id is an overwrite");
    assert!(refused.message().contains("sealed"), "{refused}");
    Ok(())
}
