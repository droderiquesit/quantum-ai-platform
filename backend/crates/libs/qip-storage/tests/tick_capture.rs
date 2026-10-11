//! Market events into the Tick Lake: sealed, replayable, retained only as
//! the entitlement allows (R12-DATA-2004, R12-TICK-2042, 2050, 2055, 2056,
//! 2089, R12-DATA-1039).

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::{Duration, Timestamp};
use qip_storage::lake::Lake;
use qip_storage::tick_capture::{
    CapturedMarketEvent, ClockCorrectionLineage, RetentionRight, TickLakeWriter,
};
use qip_storage::{BlobStore, MemoryBlobStore};
use std::collections::BTreeMap;
use std::sync::Arc;

fn at(nanos: i64) -> Timestamp {
    Timestamp::from_nanos(1_790_000_000_000_000_000 + nanos)
}

fn event(sequence: u64, instrument: &str, entitlement: &str) -> CapturedMarketEvent {
    CapturedMarketEvent {
        venue: "XNAS".into(),
        feed: "itch".into(),
        instrument: instrument.into(),
        sequence: Some(sequence),
        event_time: at(sequence as i64 * 1_000),
        receive_time: at(sequence as i64 * 1_000 + 400),
        normalized_time: at(sequence as i64 * 1_000 + 150),
        uncertainty: Duration::from_nanos(30),
        corrections: vec![ClockCorrectionLineage {
            kind: "offset_estimate".into(),
            delta_nanos: 150,
            applied_at: at(sequence as i64 * 1_000 + 400),
            samples: 32,
            uncertainty_nanos: 30.0,
        }],
        arrival_order: Some("before".into()),
        entitlement: entitlement.into(),
        canonical: serde_json::json!({ "price": "101.25", "seq": sequence }),
        raw_sha256: None,
        raw: Some(format!("raw-{sequence}").into_bytes()),
    }
}

fn policy() -> BTreeMap<String, RetentionRight> {
    BTreeMap::from([
        (
            "full".to_string(),
            RetentionRight {
                canonical: true,
                raw: true,
            },
        ),
        (
            "derived".to_string(),
            RetentionRight {
                canonical: true,
                raw: false,
            },
        ),
        (
            "live-only".to_string(),
            RetentionRight {
                canonical: false,
                raw: false,
            },
        ),
    ])
}

fn writer(store: &Arc<MemoryBlobStore>, batch: usize, max: usize) -> Result<TickLakeWriter> {
    let store: Arc<dyn BlobStore> = store.clone();
    TickLakeWriter::new(store, policy(), batch, max)
}

#[test]
fn market_events_are_sealed_and_replay_with_their_three_times_sequence_and_corrections()
-> Result<()> {
    let store = Arc::new(MemoryBlobStore::new());
    let mut lake = writer(&store, 2, 16)?;
    // Captured out of arrival order: the segment orders them.
    assert!(lake.capture(event(2, "AAPL", "full"))?.is_empty());
    let sealed = lake.capture(event(1, "AAPL", "full"))?;
    assert_eq!(sealed.len(), 1, "a full partition seals one segment");
    assert_eq!(lake.buffered(), 0);

    let replayed = Lake::new(store.as_ref()).read_market_segment(&sealed[0])?;
    assert_eq!(replayed.len(), 2, "premise: both events came back");
    let mut expected_first = event(1, "AAPL", "full");
    expected_first.raw = None; // the raw bytes live in the raw object
    expected_first.raw_sha256 = Some(sha256_hex(b"raw-1"));
    assert_eq!(replayed[0], expected_first);
    assert_eq!(replayed[1].sequence, Some(2));
    let first = &replayed[0];
    assert!(first.event_time < first.normalized_time && first.normalized_time < first.receive_time);
    assert_eq!(first.corrections.len(), 1);
    Ok(())
}

#[test]
fn raw_bytes_are_kept_only_where_the_right_permits_and_their_hash_either_way() -> Result<()> {
    let store = Arc::new(MemoryBlobStore::new());
    let mut lake = writer(&store, 1, 16)?;
    let full = lake.capture(event(1, "AAPL", "full"))?;
    let derived = lake.capture(event(1, "MSFT", "derived"))?;
    assert_eq!((full.len(), derived.len()), (1, 1), "premise: both sealed");

    let raw_key = full[0]
        .raw_key
        .as_ref()
        .expect("the full right keeps raw bytes");
    assert!(store.get(raw_key)?.is_some());
    assert!(
        derived[0].raw_key.is_none(),
        "the derived right keeps no raw bytes"
    );
    assert!(derived[0].raw_sha256.is_some(), "but keeps their hash");
    let replayed = Lake::new(store.as_ref()).read_market_segment(&derived[0])?;
    assert_eq!(replayed[0].raw_sha256, Some(sha256_hex(b"raw-1")));
    Ok(())
}

#[test]
fn an_entitlement_without_a_retention_right_is_refused_and_counted() -> Result<()> {
    let store = Arc::new(MemoryBlobStore::new());
    let mut lake = writer(&store, 1, 16)?;
    assert!(lake.capture(event(1, "AAPL", "unevaluated")).is_err());
    assert!(lake.capture(event(2, "AAPL", "live-only")).is_err());
    assert_eq!(lake.refused(), 2);
    assert!(
        store.list("lake/")?.is_empty(),
        "nothing was written for either"
    );
    assert_eq!(
        lake.capture(event(3, "AAPL", "full"))?.len(),
        1,
        "premise: a permitted one is"
    );
    Ok(())
}

#[test]
fn a_normalized_time_its_corrections_do_not_account_for_is_refused() -> Result<()> {
    let store = Arc::new(MemoryBlobStore::new());
    let mut lake = writer(&store, 1, 16)?;
    let mut unrecorded = event(1, "AAPL", "full");
    unrecorded.normalized_time = at(1_000 + 999);
    assert!(unrecorded.validate().is_err());
    assert!(lake.capture(unrecorded).is_err());
    assert!(event(1, "AAPL", "full").validate().is_ok());
    Ok(())
}

#[test]
fn resealing_identical_events_is_a_no_op_and_the_buffer_never_exceeds_its_bound() -> Result<()> {
    let store = Arc::new(MemoryBlobStore::new());
    let mut first = writer(&store, 1, 16)?;
    let sealed = first.capture(event(1, "AAPL", "full"))?;
    let mut again = writer(&store, 1, 16)?;
    let resealed = again.capture(event(1, "AAPL", "full"))?;
    assert_eq!(sealed[0].key, resealed[0].key, "the id is the content's");
    assert_eq!(sealed[0].canonical_sha256, resealed[0].canonical_sha256);

    let mut bounded = writer(&Arc::new(MemoryBlobStore::new()), 10, 3)?;
    for (i, name) in ["A", "B", "C", "D", "E"].iter().enumerate() {
        bounded.capture(event(i as u64, name, "full"))?;
        assert!(
            bounded.buffered() < 3,
            "buffered {} after {name}",
            bounded.buffered()
        );
    }
    assert!(TickLakeWriter::new(Arc::new(MemoryBlobStore::new()), policy(), 4, 3).is_err());
    Ok(())
}
