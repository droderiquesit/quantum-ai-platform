//! Store-backed dataset assembly (R12-MODEL-10083): a dataset read from the
//! Tick Lake names the stored segments it came from and rebuilds to the same
//! rows, or the rebuild is refused.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::governance::{Entitlement as Licence, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_storage::MemoryBlobStore;
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_history::HistoryQuery;
use qip_training::store::{StoreAssembly, assemble, dataset_digest, rebuild};
use std::collections::BTreeSet;

fn at(ms: i64) -> Timestamp {
    // 2026-10-11T00:00:00Z plus `ms`.
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

/// One trade as the edge node's lake sink writes it.
fn trade(
    dataset: &str,
    instrument: &str,
    sequence: u64,
    venue_ms: i64,
    capture_ms: i64,
    price: i64,
    condition: TradeCondition,
) -> Result<String> {
    let message = MarketMessage::new(
        ObjectId::from_string(instrument),
        Origin::new(VenueId::new("XLON"), "itch-a", 0, sequence),
        MessageBody::Trade {
            price: Decimal::from_int(price),
            quantity: Decimal::from_int(5),
            condition,
            aggressor: None,
        },
        at(venue_ms),
        at(capture_ms),
    );
    let hash = MarketEvent::hash_payload(&message)?;
    let event = MarketEvent::new(
        message,
        at(venue_ms),
        at(capture_ms),
        at(capture_ms),
        Duration::ZERO,
        hash,
        Licence::Granted {
            dataset: dataset.to_string(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    Ok(qip_core::canonical::canonical_json(&serde_json::json!({
        "exchange_time_ns": at(venue_ms).as_nanos(),
        "capture_time_ns": at(capture_ms).as_nanos(),
        "feed": "itch-a",
        "partition": 0,
        "sequence": sequence,
        "event": serde_json::to_value(&event)?,
    })))
}

fn regular(sequence: u64, venue_ms: i64, capture_ms: i64, price: i64) -> Result<String> {
    trade(
        "licensed",
        "obj-ACME",
        sequence,
        venue_ms,
        capture_ms,
        price,
        TradeCondition::Regular,
    )
}

fn seal(store: &MemoryBlobStore, dataset: &str, id: &str, lines: &[String]) -> Result<()> {
    let partition = Partition::new(
        RecordClass::Market,
        "XLON",
        "2026-10-11",
        "obj-ACME",
        Entitlement {
            id: dataset.into(),
            raw_retention_permitted: false,
        },
    )?;
    let mut body = Vec::new();
    for l in lines {
        body.extend_from_slice(l.as_bytes());
        body.push(b'\n');
    }
    Lake::new(store).write_segment(&partition, id, body, None)?;
    Ok(())
}

fn pin(store: &MemoryBlobStore, name: &str, granted: &[&str]) -> Result<()> {
    Lake::new(store).pin_market_history(
        name,
        &HistoryQuery {
            from_date: "2026-10-11".into(),
            to_date: "2026-10-11".into(),
            venues: BTreeSet::new(),
            instruments: BTreeSet::new(),
            granted: granted.iter().map(|s| (*s).to_string()).collect(),
            max_segments: 16,
            max_ticks: 1_000,
        },
    )?;
    Ok(())
}

fn assembly(manifest: &str, granted: &[&str]) -> StoreAssembly {
    StoreAssembly {
        dataset: "acme-trade-returns".into(),
        source_manifest: manifest.into(),
        instrument: "obj-ACME".into(),
        granted: granted.iter().map(|s| (*s).to_string()).collect(),
        lags: 2,
        max_ticks: 1_000,
    }
}

/// Six prints across two segments: 100, 102, 101, 104, 103, 106.
fn two_segments(store: &MemoryBlobStore) -> Result<()> {
    seal(
        store,
        "licensed",
        "seg-a",
        &[
            regular(1, 10, 11, 100)?,
            regular(2, 20, 21, 102)?,
            regular(3, 30, 31, 101)?,
        ],
    )?;
    seal(
        store,
        "licensed",
        "seg-b",
        &[
            regular(4, 40, 41, 104)?,
            regular(5, 50, 51, 103)?,
            regular(6, 60, 61, 106)?,
        ],
    )
}

/// The acceptance the requirement states: the manifest names its source
/// snapshots and the dataset rebuilds identically.
///
/// Mutation: leave `snapshots` empty in `assemble` — the two-snapshot
/// assertion fails; a manifest naming its source manifest and none of the
/// bytes under it is the record this module exists to replace.
#[test]
fn a_dataset_assembled_from_the_lake_names_its_source_snapshots_and_rebuilds_identically()
-> Result<()> {
    let store = MemoryBlobStore::new();
    two_segments(&store)?;
    pin(&store, "acme-2026-10-11", &["licensed"])?;
    let lake = Lake::new(&store);

    let built = assemble(&lake, &assembly("acme-2026-10-11", &["licensed"]))?;

    let pinned = lake.manifest("acme-2026-10-11")?;
    assert_eq!(pinned.segments.len(), 2, "premise: two segments are pinned");
    let named: Vec<(&str, &str)> = built
        .manifest
        .snapshots
        .iter()
        .map(|s| (s.key.as_str(), s.canonical_sha256.as_str()))
        .collect();
    let stored: Vec<(&str, &str)> = pinned
        .segments
        .iter()
        .map(|s| (s.key.as_str(), s.canonical_sha256.as_str()))
        .collect();
    assert_eq!(named, stored);
    // Six prints, five returns, two lags: three rows.
    assert_eq!(built.dataset.len(), 3);
    assert_eq!(built.manifest.rows, 3);
    assert_eq!(
        built.dataset.feature_names(),
        ["return_lag_1".to_string(), "return_lag_2".to_string()]
    );
    // Row 0: lag 1 is 102 -> 101, lag 2 is 100 -> 102, target 101 -> 104.
    let row = built.dataset.row(0)?;
    assert!((row[0] - (101.0 / 102.0 - 1.0)).abs() < 1e-12);
    assert!((row[1] - (102.0 / 100.0 - 1.0)).abs() < 1e-12);
    assert!((built.dataset.target(0)? - (104.0 / 101.0 - 1.0)).abs() < 1e-12);

    let again = rebuild(&lake, &built.manifest)?;
    assert_eq!(again, built.dataset);
    assert_eq!(dataset_digest(&again)?, built.manifest.dataset_sha256);
    Ok(())
}

/// Mutation: remove the digest comparison in `rebuild` — the rebuild returns
/// a dataset under a manifest that records different rows.
#[test]
fn a_rebuild_whose_rows_do_not_match_the_recorded_digest_is_refused() -> Result<()> {
    let store = MemoryBlobStore::new();
    two_segments(&store)?;
    pin(&store, "acme-2026-10-11", &["licensed"])?;
    let lake = Lake::new(&store);
    let built = assemble(&lake, &assembly("acme-2026-10-11", &["licensed"]))?;
    // Premise: the untouched manifest rebuilds.
    rebuild(&lake, &built.manifest)?;

    // The manifest as a different assembly rule would have written it: same
    // sources, same request, other rows.
    let mut recorded = built.manifest.clone();
    recorded.dataset_sha256 = "0".repeat(64);
    let refused = rebuild(&lake, &recorded).unwrap_err().to_string();
    assert!(
        refused.contains("rebuilt with digest"),
        "refused for another reason: {refused}"
    );
    Ok(())
}

/// Mutation: remove the snapshot comparison in `rebuild` — a manifest naming
/// a segment the rebuild never read is accepted.
#[test]
fn a_rebuild_that_read_other_source_snapshots_than_the_manifest_names_is_refused() -> Result<()> {
    let store = MemoryBlobStore::new();
    two_segments(&store)?;
    pin(&store, "acme-2026-10-11", &["licensed"])?;
    let lake = Lake::new(&store);
    let built = assemble(&lake, &assembly("acme-2026-10-11", &["licensed"]))?;
    assert_eq!(built.manifest.snapshots.len(), 2, "premise");

    let mut recorded = built.manifest.clone();
    recorded.snapshots[1].canonical_sha256 = "f".repeat(64);
    let refused = rebuild(&lake, &recorded).unwrap_err().to_string();
    assert!(
        refused.contains("source snapshot(s)"),
        "refused for another reason: {refused}"
    );
    Ok(())
}

/// The venue stamps the second print *before* the first, and the platform
/// received them the other way round. A row dated or ordered by the venue's
/// clock is readable before the platform could have read it.
///
/// Mutation: push `tick.event_time` instead of `tick.knowable_at` in
/// `assemble` — both the order of the series and the row instants change.
#[test]
fn rows_are_ordered_and_dated_by_when_each_print_became_knowable_not_by_the_venues_clock()
-> Result<()> {
    let store = MemoryBlobStore::new();
    seal(
        &store,
        "licensed",
        "seg-a",
        &[
            // Stored first, stamped later by the venue, received first.
            regular(1, 8, 11, 100)?,
            regular(2, 5, 21, 110)?,
            regular(3, 30, 31, 121)?,
        ],
    )?;
    pin(&store, "late-stamp", &["licensed"])?;
    let lake = Lake::new(&store);
    let mut request = assembly("late-stamp", &["licensed"]);
    request.lags = 1;

    let built = assemble(&lake, &request)?;

    assert_eq!(built.dataset.len(), 1);
    // Knowable order is 100, 110, 121: feature +10%, target +10%.
    assert!((built.dataset.row(0)?[0] - 0.10).abs() < 1e-12);
    assert!((built.dataset.target(0)? - 0.10).abs() < 1e-12);
    // The row is knowable when its last feature print was received.
    assert_eq!(built.dataset.times(), [at(21)]);
    Ok(())
}

/// Mutation: drop the `granted` filter on `snapshots` in `assemble` — the
/// manifest names a segment as read that the lake withheld.
#[test]
fn a_segment_the_grant_does_not_cover_contributes_no_row_and_is_named_withheld() -> Result<()> {
    let store = MemoryBlobStore::new();
    two_segments(&store)?;
    seal(
        &store,
        "research-only",
        "seg-r",
        &[trade(
            "research-only",
            "obj-ACME",
            7,
            70,
            71,
            999,
            TradeCondition::Regular,
        )?],
    )?;
    // Pinned by a job granted both; assembled by one granted only the first.
    pin(&store, "both", &["licensed", "research-only"])?;
    let lake = Lake::new(&store);
    assert_eq!(lake.manifest("both")?.segments.len(), 3, "premise");

    let built = assemble(&lake, &assembly("both", &["licensed"]))?;

    assert_eq!(built.manifest.withheld.len(), 1);
    assert!(built.manifest.withheld[0].contains("entitlement=research-only"));
    assert_eq!(built.manifest.snapshots.len(), 2);
    assert!(
        built
            .manifest
            .snapshots
            .iter()
            .all(|s| s.entitlement == "licensed")
    );
    // The 999 print is not in the series: still three rows from six prints.
    assert_eq!(built.dataset.len(), 3);
    Ok(())
}

/// A correction restates an earlier print; read as the next price it invents
/// a return that never traded.
///
/// Mutation: remove the `Correction | OddLot` skip in `assemble` — the row
/// count rises to four.
#[test]
fn a_correction_or_an_odd_lot_is_not_read_as_the_next_price() -> Result<()> {
    let store = MemoryBlobStore::new();
    two_segments(&store)?;
    seal(
        &store,
        "licensed",
        "seg-c",
        &[trade(
            "licensed",
            "obj-ACME",
            7,
            70,
            71,
            50,
            TradeCondition::Correction,
        )?],
    )?;
    pin(&store, "with-correction", &["licensed"])?;
    let lake = Lake::new(&store);

    let built = assemble(&lake, &assembly("with-correction", &["licensed"]))?;

    assert_eq!(built.manifest.snapshots.len(), 3, "premise: it was read");
    assert_eq!(built.dataset.len(), 3);
    Ok(())
}

/// Mutation: remove the `lags + 2` check — the refusal becomes an index
/// panic or an empty-dataset error that names nothing the caller can act on.
#[test]
fn a_history_with_too_few_trades_for_its_lags_is_refused_naming_the_count() -> Result<()> {
    let store = MemoryBlobStore::new();
    seal(
        &store,
        "licensed",
        "seg-a",
        &[
            regular(1, 10, 11, 100)?,
            regular(2, 20, 21, 102)?,
            regular(3, 30, 31, 101)?,
        ],
    )?;
    pin(&store, "thin", &["licensed"])?;
    let lake = Lake::new(&store);

    let refused = assemble(&lake, &assembly("thin", &["licensed"]))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("3 usable trade(s)") && refused.contains("at least 4"),
        "{refused}"
    );
    Ok(())
}
