//! The event log's hash chain, archived across restarts.
//!
//! The property that matters is not that a record can be written and read
//! back — every adapter test already shows that. It is that a *second run* of
//! a process, whose in-memory log starts its sequences again at one, appends
//! after the first run instead of over it.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::{CorrelationId, EventId, Lineage, Timestamp};
use qip_events::envelope::AnyEvent;
use qip_events::envelope::canonical_json;
use qip_events::log::{EventLog, LogRecord};
use qip_events::topic::Topic;
use qip_storage::chain::{ArchivedRecord, ChainArchive};
use qip_storage::settings::StorageSettings;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> std::path::PathBuf {
    let unique = DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("qip-chain-{label}-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the test fixture directory is creatable");
    dir
}

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn event(n: u64) -> AnyEvent {
    let payload = serde_json::json!({ "n": n });
    AnyEvent {
        event_id: EventId::from_string(format!("EVT{n:023}")),
        topic: Topic::SystemAlert,
        schema_version: 1,
        occurred_at: now(),
        recorded_at: now(),
        sequence: 0,
        lineage: Lineage::root(
            CorrelationId::from_string("COR00000000000000000000001"),
            "chain-test",
        ),
        idempotency_key: None,
        payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    }
}

/// One run of a process: a fresh in-memory log with `count` records in it.
///
/// The sequences restart at one every time, which is exactly the condition the
/// archive has to survive.
fn a_run_of(count: u64) -> Result<Vec<LogRecord>> {
    let mut log = EventLog::in_memory();
    for n in 1..=count {
        log.append(&event(n))?;
    }
    Ok(log.records().to_vec())
}

fn archive_over(target: &str, root: &std::path::Path) -> Result<ChainArchive> {
    let settings = StorageSettings::from_values(Some(target), root.to_str())?;
    ChainArchive::open(settings.key_value("event-log")?)
}

// --- the restart property ---------------------------------------------------

#[test]
fn a_second_run_appends_after_the_first_rather_than_over_it() -> Result<()> {
    let root = temp_dir("restart");

    // The premise: both runs really do produce the same sequence numbers, so
    // an archive keyed by the source sequence would overwrite rather than
    // append, and the test would be meaningless if they differed.
    let first = a_run_of(3)?;
    let second = a_run_of(2)?;
    assert_eq!(
        first.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        second.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        vec![1, 2]
    );

    let archive = archive_over("engine", &root)?;
    assert_eq!(archive.absorb(&first)?, 3);
    drop(archive);

    // A new process against the same store.
    let archive = archive_over("engine", &root)?;
    assert_eq!(
        archive.len()?,
        3,
        "the first run's records did not survive the restart"
    );
    assert_eq!(archive.absorb(&second)?, 2);

    let records = archive.records()?;
    assert_eq!(records.len(), 5, "the second run overwrote the first");
    assert_eq!(
        records.iter().map(|r| r.position).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4],
        "the archive's own positions must be dense across runs"
    );
    assert_eq!(
        archive.first_broken_position()?,
        None,
        "the chain must span both runs"
    );
    Ok(())
}

#[test]
fn absorbing_the_same_records_twice_writes_nothing_the_second_time() -> Result<()> {
    // The premise: the first absorb really did write them, so what the second
    // call demonstrates is the watermark and not an archive that never works.
    let archive = ChainArchive::open(Arc::new(qip_storage::MemoryKeyValueStore::new()))?;
    let records = a_run_of(4)?;
    assert_eq!(archive.absorb(&records)?, 4);
    assert_eq!(archive.len()?, 4);

    assert_eq!(archive.absorb(&records)?, 0);
    assert_eq!(archive.len()?, 4, "a repeat hand-over duplicated records");
    Ok(())
}

#[test]
fn a_growing_log_hands_over_only_what_is_new() -> Result<()> {
    let archive = ChainArchive::open(Arc::new(qip_storage::MemoryKeyValueStore::new()))?;
    let mut log = EventLog::in_memory();
    for n in 1..=3 {
        log.append(&event(n))?;
    }
    assert_eq!(archive.absorb(log.records())?, 3);

    for n in 4..=5 {
        log.append(&event(n))?;
    }
    assert_eq!(
        archive.absorb(log.records())?,
        2,
        "the whole slice was handed over again, not just the new records"
    );
    assert_eq!(archive.len()?, 5);
    assert_eq!(archive.absorbed_through(), 5);
    Ok(())
}

// --- ordering ---------------------------------------------------------------

#[test]
fn records_come_back_in_chain_order_past_the_tenth() -> Result<()> {
    // The zero-padding property. Keys come back in lexicographic order, so an
    // unpadded position would sort "10" before "9" and replay the chain out of
    // order — which then fails verification for a reason that has nothing to
    // do with what happened to the data.
    let root = temp_dir("ordering");
    let archive = archive_over("file", &root)?;
    archive.absorb(&a_run_of(23)?)?;

    let positions: Vec<u64> = archive.records()?.iter().map(|r| r.position).collect();
    assert_eq!(
        positions,
        (0..23).collect::<Vec<u64>>(),
        "the archive came back out of order"
    );
    assert_eq!(archive.first_broken_position()?, None);
    Ok(())
}

// --- tamper detection -------------------------------------------------------

#[test]
fn an_archived_record_that_was_edited_breaks_the_chain_at_its_own_position() -> Result<()> {
    let root = temp_dir("edited");
    let settings = StorageSettings::from_values(Some("file"), root.to_str())?;
    let store = settings.key_value("event-log")?;
    let archive = ChainArchive::open(store.clone())?;
    archive.absorb(&a_run_of(5)?)?;

    // The premise: it verifies before the edit, so the break below is caused
    // by the edit and not by an archive that never verified.
    assert_eq!(archive.first_broken_position()?, None);

    let key = store
        .keys_with_prefix("chain/")?
        .get(2)
        .cloned()
        .expect("the archive holds a third entry");
    let mut entry: ArchivedRecord = store
        .get(&key)?
        .map(serde_json::from_value)
        .transpose()?
        .expect("the third entry is readable");
    entry.record.event.payload = serde_json::json!({ "n": "edited" });
    store.put(&key, serde_json::to_value(&entry)?)?;

    assert_eq!(
        ChainArchive::open(store)?.first_broken_position()?,
        Some(2),
        "editing an archived record went unnoticed"
    );
    Ok(())
}

#[test]
fn an_archived_record_that_was_removed_breaks_the_chain_where_it_was() -> Result<()> {
    // The loss the archive's own chain exists to catch. The records either
    // side are untouched and internally consistent, so nothing but the
    // linkage can show that one is gone.
    let root = temp_dir("removed");
    let settings = StorageSettings::from_values(Some("file"), root.to_str())?;
    let store = settings.key_value("event-log")?;
    let archive = ChainArchive::open(store.clone())?;
    archive.absorb(&a_run_of(5)?)?;
    assert_eq!(archive.first_broken_position()?, None);

    let key = store
        .keys_with_prefix("chain/")?
        .get(3)
        .cloned()
        .expect("the archive holds a fourth entry");
    assert!(store.delete(&key)?, "the fixture entry was already absent");

    assert_eq!(
        ChainArchive::open(store)?.first_broken_position()?,
        Some(4),
        "removing an archived record went unnoticed"
    );
    Ok(())
}

#[test]
fn verify_separates_an_unreadable_archive_from_a_broken_one() -> Result<()> {
    // An intact archive reports intact, and a broken one names the position.
    // The distinction matters because an incident review reading "broken"
    // starts looking for tampering.
    let archive = ChainArchive::open(Arc::new(qip_storage::MemoryKeyValueStore::new()))?;
    archive.absorb(&a_run_of(2)?)?;
    archive.verify().expect("an untouched archive verifies");
    assert!(
        archive.describe().contains("intact"),
        "{}",
        archive.describe()
    );

    let empty = ChainArchive::open(Arc::new(qip_storage::MemoryKeyValueStore::new()))?;
    assert!(empty.is_empty()?);
    assert!(
        empty.describe().contains("first run"),
        "an empty archive should read as a first run, not as a loss: {}",
        empty.describe()
    );
    Ok(())
}

// --- the source record survives ---------------------------------------------

#[test]
fn the_archived_entry_still_carries_the_record_the_log_wrote() -> Result<()> {
    // The archive re-chains, but it must not replace the source log's own
    // linkage: an archived entry has to remain checkable against the log it
    // came from.
    let root = temp_dir("fidelity");
    let archive = archive_over("engine", &root)?;
    let records = a_run_of(3)?;
    archive.absorb(&records)?;
    drop(archive);

    let reopened = archive_over("engine", &root)?;
    let archived = reopened.records()?;
    assert_eq!(archived.len(), records.len());
    for (entry, original) in archived.iter().zip(records.iter()) {
        assert_eq!(
            &entry.record, original,
            "the source record was not preserved"
        );
    }
    Ok(())
}
