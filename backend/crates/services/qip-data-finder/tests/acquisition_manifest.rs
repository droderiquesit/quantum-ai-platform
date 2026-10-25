//! The pass-through manifest filled on acquisition, the re-fetch driver and
//! the ambient watcher (R12-DATA-1033..1035, 1037, R12-AMBIENT-1040).
//!
//! Until 2026-10-11 a reference's etag, cursor, entitlement, geography,
//! expiry and re-fetch instructions were `None` from every door, and the
//! kernel's test for them asserted they were absent. These tests assert the
//! values a connector fetch actually produces.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::kv::KeyValueStore;
use qip_core::{Decimal, Timestamp};
use qip_data_finder::admission::{self, AdmittedSource};
use qip_data_finder::reference::{DataReference, RevisionCheck};
use qip_data_finder::refetch::{
    ReFetchDriver, ReFetchOutcome, Refetcher, SourceWatcher, TransportRefetcher,
    UNAVAILABLE_BEFORE_UNRETRIEVABLE, WatchDelta,
};
use qip_market_ingestion::connector::emulator::{RecordedAnswer, RecordedExchange, SourceEmulator};
use qip_market_ingestion::connector::evidence::ParserLineage;
use qip_market_ingestion::connector::{Cursor, FetchDigest, RawEvent, SourceConnector};
use qip_market_ingestion::connectors::FrankfurterRatesConnector;
use qip_storage::MemoryKeyValueStore;

const LOCATOR: &str = "/v1/latest?base=EUR&symbols=USD,GBP,JPY";
const TABLE: &str = r#"{"amount":1.0,"base":"EUR","date":"2026-09-04","rates":{"GBP":0.85898,"JPY":181.59,"USD":1.1622}}"#;
const REVISED: &str = r#"{"amount":1.0,"base":"EUR","date":"2026-09-04","rates":{"GBP":0.85898,"JPY":181.59,"USD":1.1655}}"#;

fn retrieved() -> Timestamp {
    Timestamp::from_secs(1_760_100_000)
}

fn admitted() -> Result<AdmittedSource> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let decision = admission::admit(&manifest.source_id, manifest.licensing, retrieved())?;
    AdmittedSource::from_decision(&decision, &manifest)
}

fn digest(body: &str) -> Result<FetchDigest> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let connector = FrankfurterRatesConnector::new(manifest.clone())?;
    let payload: serde_json::Value = serde_json::from_str(body)?;
    let events: Vec<RawEvent> = connector.decode(&payload, &Cursor::beginning())?;
    let subjects: Vec<String> = events
        .iter()
        .filter_map(|event| connector.map(event, retrieved()).ok())
        .filter_map(|record| record.subject_id().map(str::to_string))
        .collect();
    FetchDigest::of(
        &manifest,
        LOCATOR,
        body.as_bytes(),
        &events,
        subjects,
        retrieved(),
    )
}

fn reference(body: &str) -> Result<DataReference> {
    let digest = digest(body)?
        .with_etag(Some("\"v42\"".to_string()))
        .with_cursor("event_time_ns:1757000000000000000");
    DataReference::from_digest(&admitted()?, &digest, Decimal::ZERO, 0.95)
}

#[test]
fn a_reference_from_a_connector_fetch_carries_every_acquisition_fact() -> Result<()> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let reference = reference(TABLE)?;
    assert_eq!(
        reference.source_id(),
        manifest.source_id,
        "premise: the reference is the source's"
    );

    assert_eq!(reference.etag(), Some("\"v42\""));
    assert_eq!(
        reference.cursor(),
        Some("event_time_ns:1757000000000000000")
    );
    assert_eq!(reference.entitlement(), Some(manifest.licensing));
    assert_eq!(reference.geography(), Some(manifest.region));
    assert_eq!(
        reference.expiry(),
        Some(retrieved().saturating_add(manifest.freshness_sla()))
    );
    assert_eq!(reference.parser(), Some(&ParserLineage::of(&manifest)));
    let instruction = reference
        .re_fetch_instructions()
        .expect("an instruction is written");
    assert!(instruction.contains(LOCATOR), "{instruction}");
    assert!(
        instruction.contains(reference.content_hash()),
        "{instruction}"
    );
    assert!(instruction.contains("\"v42\""), "{instruction}");
    assert_eq!(reference.is_expired(retrieved()), Some(false));
    Ok(())
}

#[test]
fn every_acquisition_fact_survives_the_event_log_round_trip() -> Result<()> {
    let reference = reference(TABLE)?;
    let back: DataReference = serde_json::from_str(&serde_json::to_string(&reference)?)?;
    assert_eq!(back, reference);
    assert!(
        back.parser().is_some(),
        "premise: the parser was there to lose"
    );
    Ok(())
}

/// A scripted refetcher: answers in order, the last repeating.
struct Scripted(Vec<ReFetchOutcome>);

impl Refetcher for Scripted {
    fn refetch(&mut self, _reference: &DataReference, _at: Timestamp) -> Result<ReFetchOutcome> {
        Ok(if self.0.len() > 1 {
            self.0.remove(0)
        } else {
            self.0[0].clone()
        })
    }
}

#[test]
fn a_re_fetch_reports_unchanged_and_revised_extents_with_both_hashes() -> Result<()> {
    let reference = reference(TABLE)?;
    let mut driver = ReFetchDriver::new();
    let same = driver.run(
        [&reference],
        &mut Scripted(vec![ReFetchOutcome::Fetched {
            content_hash: qip_core::sha256_hex(TABLE.as_bytes()),
            etag: None,
        }]),
        10,
        retrieved(),
    )?;
    assert_eq!(same.len(), 1, "premise: the reference was re-fetched");
    assert_eq!(same[0].check, Some(RevisionCheck::Unchanged));

    let revised = driver.run(
        [&reference],
        &mut Scripted(vec![ReFetchOutcome::Fetched {
            content_hash: qip_core::sha256_hex(REVISED.as_bytes()),
            etag: None,
        }]),
        10,
        retrieved(),
    )?;
    assert!(
        matches!(&revised[0].check, Some(RevisionCheck::Revised { was, .. }) if was == reference.content_hash())
    );
    assert!(driver.unretrievable_sources().is_empty());
    Ok(())
}

#[test]
fn a_withdrawn_extent_is_unretrievable_at_once_and_an_unanswered_one_only_after_the_threshold()
-> Result<()> {
    let reference = reference(TABLE)?;
    let source = reference.source_id().to_string();

    let mut withdrawn = ReFetchDriver::new();
    let found = withdrawn.run(
        [&reference],
        &mut Scripted(vec![ReFetchOutcome::Withdrawn]),
        10,
        retrieved(),
    )?;
    assert_eq!(found[0].check, Some(RevisionCheck::Unretrievable));
    assert!(withdrawn.unretrievable_sources().contains(&source));

    let mut silent = ReFetchDriver::new();
    let mut down = Scripted(vec![ReFetchOutcome::Unavailable {
        detail: "timeout".into(),
    }]);
    for attempt in 1..UNAVAILABLE_BEFORE_UNRETRIEVABLE {
        let found = silent.run([&reference], &mut down, 10, retrieved())?;
        assert_eq!(
            found[0].check, None,
            "attempt {attempt} marked one timeout as withdrawal"
        );
        assert!(silent.unretrievable_sources().is_empty());
    }
    let found = silent.run([&reference], &mut down, 10, retrieved())?;
    assert_eq!(found[0].check, Some(RevisionCheck::Unretrievable));
    assert!(silent.unretrievable_sources().contains(&source));

    // The source serves it again: the mark is lifted.
    silent.run(
        [&reference],
        &mut Scripted(vec![ReFetchOutcome::Fetched {
            content_hash: qip_core::sha256_hex(TABLE.as_bytes()),
            etag: None,
        }]),
        10,
        retrieved(),
    )?;
    assert!(silent.unretrievable_sources().is_empty());
    Ok(())
}

#[test]
fn the_transport_refetcher_asks_the_same_locator_and_reads_a_404_as_withdrawn() -> Result<()> {
    let reference = reference(TABLE)?;
    let mut emulator = SourceEmulator::new(vec![RecordedExchange::always(
        "/v1/latest",
        RecordedAnswer::json(404, "{}"),
    )]);
    let mut fetcher = TransportRefetcher::new(&mut emulator);
    assert_eq!(
        fetcher.refetch(&reference, retrieved())?,
        ReFetchOutcome::Withdrawn
    );
    assert_eq!(
        emulator.calls(),
        [LOCATOR.to_string()],
        "the same request, rebuilt from the locator"
    );
    Ok(())
}

#[test]
fn a_watcher_classifies_new_unchanged_and_changed_extents_and_survives_a_restart() -> Result<()> {
    let first = reference(TABLE)?;
    let revised = reference(REVISED)?;
    let mut watcher = SourceWatcher::new();
    assert_eq!(watcher.observe(&first), WatchDelta::New);
    assert_eq!(watcher.observe(&first), WatchDelta::Unchanged);

    let store = MemoryKeyValueStore::new();
    watcher.save(&store)?;
    assert!(!store.is_empty()?, "premise: something was persisted");
    let mut restarted = SourceWatcher::load(&store)?;
    assert_eq!(
        restarted
            .state(first.source_id())
            .and_then(|s| s.cursor.as_deref()),
        Some("event_time_ns:1757000000000000000")
    );
    assert_eq!(
        restarted.observe(&revised),
        WatchDelta::Changed {
            was: first.content_hash().to_string(),
            now: revised.content_hash().to_string()
        }
    );
    Ok(())
}
