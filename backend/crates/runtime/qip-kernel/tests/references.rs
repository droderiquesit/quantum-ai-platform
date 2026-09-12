//! The reference ledger's consequence, driven through the platform the way a
//! composition root drives it: a connector's digest in, a reference recorded,
//! and — when the source revises what it served — a flag a research run can
//! read, a record on the hash-chained log, and a series that moved.
//!
//! Every test is premise-first, because the property under test is that a
//! control *fires*: a platform with nothing admitted and nothing recorded
//! would pass an assertion that nothing was flagged.

// See the note in `absorption.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Context, Duration, Timestamp};
use qip_data_finder::admission::{self, AdmittedSource};
use qip_data_finder::ledger::LedgerOutcome;
use qip_data_finder::reference::{DataPeriod, SourceOrigin};
use qip_events::Topic;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_kernel::references::SourceRevisionDetected;
use qip_market_ingestion::connector::{FetchDigest, RawEvent};
use qip_market_ingestion::connectors::{CoinbaseTickerConnector, FrankfurterRatesConnector};
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::LimitSet;
use qip_streaming::envelope::StreamEnvelope;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

/// The shipped Frankfurter source through the real catalogue and the real
/// gate — the same path `qip-api` and `qip-fastbrain` take.
fn admitted_frankfurter() -> Result<AdmittedSource> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let decision = admission::admit(&manifest.source_id, manifest.licensing, start())?;
    AdmittedSource::from_decision(&decision, &manifest)
}

/// The reference date the ECB table carries: the period a fetch of it
/// describes, whatever instant it was fetched at.
fn table_date() -> Timestamp {
    Timestamp::parse_rfc3339("2026-09-04T00:00:00Z").expect("a literal instant parses")
}

const SOURCE: &str = "frankfurter-ecb-reference-rates";
const LOCATOR: &str = "/v1/latest?base=EUR&symbols=USD,GBP,JPY";
const TABLE: &str = r#"{"amount":1.0,"base":"EUR","date":"2026-09-04","rates":{"GBP":0.85898,"JPY":181.59,"USD":1.1622}}"#;
/// The same table with the dollar rate rewritten — what a vendor correcting
/// a published fixing looks like on the wire.
const REVISED_TABLE: &str = r#"{"amount":1.0,"base":"EUR","date":"2026-09-04","rates":{"GBP":0.85898,"JPY":181.59,"USD":1.1655}}"#;

/// The digest `ConnectorRuntime::ingest` would take over `body`, built the
/// way the runtime builds it — from the manifest, the locator, the bytes and
/// the decoded events — so the test exercises the same constructor the
/// production seam does.
fn frankfurter_digest(body: &str, retrieved_at: Timestamp) -> Result<FetchDigest> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let events: Vec<RawEvent> = ["GBP", "JPY", "USD"]
        .into_iter()
        .map(|key| RawEvent::new(key, table_date(), serde_json::Value::Null))
        .collect();
    Ok(
        FetchDigest::of(&manifest, LOCATOR, body.as_bytes(), &events, retrieved_at)?
            .with_topic(Topic::MacroUpdated),
    )
}

/// The platform refuses to reference bytes from a source it holds no
/// admission for, however they arrived — and records nothing about them.
///
/// Mutated by replacing the `.get(digest.source_id())` lookup in
/// `Platform::reference_fetch` with `.values().next()` (any admitted source)
/// — confirmed this test then fails because the refusal is the
/// source-mismatch `invalid` from `from_digest` rather than the `denied` the
/// missing admission produces, then restored.
#[test]
fn a_digest_from_a_source_this_platform_never_admitted_is_refused_and_records_nothing() -> Result<()>
{
    let mut platform = platform()?;
    platform.admit_source(admitted_frankfurter()?);
    // Premise: one source *is* admitted, so the refusal below is about the
    // source in the digest and not about an empty admission table.
    assert!(platform.admitted_source(SOURCE).is_some());
    assert!(platform.admitted_source("coinbase-spot-ticker").is_none());

    let coinbase = CoinbaseTickerConnector::shipped_manifest()?;
    let events = vec![RawEvent::new("12345", start(), serde_json::Value::Null)];
    let digest = FetchDigest::of(
        &coinbase,
        "/products/BTC-USD/ticker",
        br#"{"trade_id":12345,"price":"61000.10"}"#,
        &events,
        start(),
    )?;

    let error = platform
        .reference_fetch(&digest, start())
        .expect_err("bytes from an unadmitted source were referenced");
    assert_eq!(error.code(), "denied", "got {error:?}");
    assert!(
        error.message().contains("holds no admission"),
        "the refusal does not name the missing admission: {error}"
    );
    assert!(
        platform.reference_ledger().is_empty(),
        "a refused digest must leave no reference behind"
    );
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(names::DATA_REFERENCES_RECORDED),
        0,
        "a refused digest must not be counted as recorded"
    );
    Ok(())
}

/// The consequence, end to end. A re-fetch of the same extent with the same
/// bytes is unchanged and is not a data-quality event; a re-fetch that
/// hashes differently is a revision, and a revision flags the symbol and
/// period it covered, is written to the hash-chained log with both hashes,
/// and moves the revisions series.
///
/// Mutated by deleting the `if let LedgerOutcome::Revised(revision)` block
/// in `Platform::record_reference` — confirmed the log and metric assertions
/// then fail while the ledger's own outcome still reads `Revised`, which is
/// exactly the "detected and acted on by nothing" state the block exists to
/// prevent, then restored.
#[test]
fn a_source_that_revises_an_extent_after_use_is_flagged_in_the_ledger_the_log_and_the_metrics()
-> Result<()> {
    let mut platform = platform()?;
    platform.admit_source(admitted_frankfurter()?);
    let period = DataPeriod::instant(table_date());

    let first = platform.reference_fetch(&frankfurter_digest(TABLE, start())?, start())?;
    assert_eq!(first.outcome, LedgerOutcome::First);
    assert_eq!(first.reference.origin(), SourceOrigin::CatalogueAdmitted);
    assert_eq!(first.reference.source_id(), SOURCE);
    assert_eq!(first.reference.locator(), LOCATOR);
    assert!(
        platform.revision_covering(SOURCE, "USD", &period).is_none(),
        "premise: nothing is flagged before any revision"
    );
    let logged_before = platform
        .event_log()
        .by_topic(Topic::DataQualityFailed)
        .len();

    // The table re-served an hour later, byte for byte.
    let later = start().saturating_add(Duration::from_hours(1));
    let same = platform.reference_fetch(&frankfurter_digest(TABLE, later)?, later)?;
    assert_eq!(same.outcome, LedgerOutcome::Unchanged);
    assert_eq!(
        platform
            .event_log()
            .by_topic(Topic::DataQualityFailed)
            .len(),
        logged_before,
        "an unchanged re-fetch is not a data-quality event"
    );
    assert!(platform.revision_covering(SOURCE, "USD", &period).is_none());

    // The vendor rewrites the dollar fixing for the same date.
    let latest = later.saturating_add(Duration::from_hours(1));
    let revised = platform.reference_fetch(&frankfurter_digest(REVISED_TABLE, latest)?, latest)?;
    let LedgerOutcome::Revised(record) = &revised.outcome else {
        panic!("a rewritten table was recorded as {:?}", revised.outcome);
    };
    assert_eq!(record.was(), first.reference.content_hash());
    assert_eq!(record.now(), revised.reference.content_hash());
    assert_ne!(record.was(), record.now());
    assert_eq!(record.detected_at(), latest);
    assert_eq!(
        record.used_at(),
        later,
        "the extent contradicted is the one last used, an hour after the first fetch"
    );

    // 1. The flag a research run reads: the symbol and period the revised
    //    extent covered, and not a period it did not.
    let flag = platform
        .revision_covering(SOURCE, "USD", &period)
        .expect("a revised extent flags the symbol and period it covered");
    assert_eq!(flag.now(), record.now());
    assert!(
        platform
            .revision_covering(
                SOURCE,
                "USD",
                &DataPeriod::instant(table_date().saturating_add(Duration::from_days(1)))
            )
            .is_none(),
        "the next day's table was never fetched, so it cannot be flagged"
    );
    assert!(
        platform.revision_covering(SOURCE, "CHF", &period).is_none(),
        "a symbol the table never carried cannot be flagged"
    );

    // 2. The hash-chained log holds the revision with both hashes, decodable
    //    as the typed record, and the chain still verifies.
    let records = platform.event_log().by_topic(Topic::DataQualityFailed);
    assert_eq!(
        records.len(),
        logged_before + 1,
        "exactly one data-quality record was written for one revision"
    );
    let logged = StreamEnvelope::from_frame(records[records.len() - 1])?
        .decode::<SourceRevisionDetected>()?
        .body;
    assert_eq!(logged.revision.was(), record.was());
    assert_eq!(logged.revision.now(), record.now());
    assert_eq!(logged.revision.source_id(), SOURCE);
    assert!(logged.revision.symbols().contains("USD"));
    assert!(platform.event_log().verify_chain().is_ok());

    // 3. The series moved, labelled by the door and never by the source id.
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::DATA_REVISIONS_DETECTED,
            &labels([("origin", "catalogue_admitted")])
        ),
        1
    );
    for (outcome, expected) in [("first", 1), ("unchanged", 1), ("revised", 1)] {
        assert_eq!(
            snapshot.counter(
                names::DATA_REFERENCES_RECORDED,
                &labels([("outcome", outcome)])
            ),
            expected,
            "qip_data_references_recorded_total{{outcome={outcome}}}"
        );
    }

    // And the ledger keeps what the source now serves, so a third fetch of
    // the rewritten table is unchanged rather than a second revision.
    let again = platform.reference_fetch(
        &frankfurter_digest(
            REVISED_TABLE,
            latest.saturating_add(Duration::from_hours(1)),
        )?,
        latest.saturating_add(Duration::from_hours(1)),
    )?;
    assert_eq!(again.outcome, LedgerOutcome::Unchanged);
    Ok(())
}
