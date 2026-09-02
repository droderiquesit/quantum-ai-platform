//! A degraded universe is visible at assembly, not after its first bad trade.
//!
//! `Universe::not_decision_grade` documented that the kernel logged it at
//! start-up, and nothing did: a universe assembled entirely from
//! research-only instruments looked exactly like one fit to trade until a
//! proposal was sized on it.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str, provenance: Provenance) -> FinancialObject {
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::CommonStock,
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(provenance)
    .build(start())
    .expect("valid object")
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

#[test]
fn a_research_only_instrument_is_counted_and_named_at_assembly() -> Result<()> {
    // One instrument licensed for production decisions, one synthetic — the
    // licensing class that never may drive one. Premise: the universe itself
    // says exactly one is unfit, so the gauge below measures the kernel's
    // reading of it and not an empty list.
    let mut universe = Universe::new();
    universe.insert(object(
        "AAA",
        Provenance::new("vendor", start(), start()).with_licensing(LicensingClass::Licensed),
    ))?;
    universe.insert(object("ZZZ", Provenance::synthetic("research", start())))?;
    assert_eq!(
        universe.not_decision_grade().len(),
        1,
        "the fixture must hold exactly one degraded instrument"
    );

    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let platform = Platform::new(config, context, Telemetry::silent(), universe, limits())?;

    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .gauge(names::UNIVERSE_NOT_DECISION_GRADE, &labels([])),
        Some(1.0),
        "the degraded count did not reach the registry at assembly"
    );
    let degraded = platform.universe_not_decision_grade();
    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0].0, "obj-ZZZ");
    assert!(
        degraded[0].1.contains("licensing class") && degraded[0].1.contains("Synthetic"),
        "the reason must name the licensing class that disqualified it: {}",
        degraded[0].1
    );
    Ok(())
}

/// A one-record catalogue in the committed schema, built here rather than
/// read from `data/` so the test proves the seam and not the fixture.
fn catalogue_text() -> String {
    r#"{
  "schema_version": 1,
  "version": "kernel-test-2026.09.01",
  "as_of": "2026-09-01T00:00:00Z",
  "source": "qip-kernel-test-catalogue",
  "instruments": [
    {
      "object_id": "OBJ0000000000000000KTST",
      "symbol": "KTST",
      "instrument_type": "common_stock",
      "venue": "XNYS",
      "sector": "information_technology",
      "geography": "US",
      "currency": "USD",
      "price": "100.00",
      "lot_size": "1",
      "tick_size": "0.01",
      "licensing": "internal"
    }
  ]
}"#
    .to_string()
}

/// The first fact a replay needs is which universe the run saw, by hash, and
/// until now it was the one fact the hash-chained log did not hold: the roots
/// journaled the catalogue manifest in a key-value namespace beside the log
/// because the kernel offered no seam to append through. The platform now
/// writes it itself, at assembly, before any cycle can exist — so the record
/// is the first non-genesis link on the chain, carries the hash the loader
/// computed, and the first cycle's own entry comes after it.
#[test]
fn the_catalogue_a_platform_assembled_from_is_the_first_record_on_its_event_log() -> Result<()> {
    use qip_events::Topic;
    use qip_events::log::GENESIS_HASH;
    use qip_financial::catalogue;
    use qip_kernel::UniverseAssembled;
    use qip_streaming::envelope::StreamEnvelope;

    let now = Timestamp::from_civil(2026, 9, 2);
    let text = catalogue_text();
    let loaded = catalogue::load(&text, now)?;
    // Premise: the loader's hash is over the text, and there is one record.
    assert_eq!(
        loaded.manifest.sha256,
        qip_core::sha256_hex(text.as_bytes())
    );
    assert_eq!(loaded.manifest.instruments, 1);

    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(now, config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        loaded.universe,
        limits(),
    )?;

    let records = platform.event_log().records();
    let first = records
        .first()
        .expect("an assembled platform has written at least one record");
    assert_eq!(
        first.sequence, 1,
        "the universe record is not the first link"
    );
    assert_eq!(first.previous_hash, GENESIS_HASH);
    assert_eq!(first.event.topic, Topic::ReferenceDataUpdated);
    // The log holds the stream envelope's frame; the body is inside it.
    let body = StreamEnvelope::from_frame(&first.event)?
        .decode::<UniverseAssembled>()?
        .body;
    let origin = body
        .catalogue
        .as_ref()
        .expect("a platform assembled from a catalogue records which one");
    assert_eq!(origin.sha256, loaded.manifest.sha256);
    assert_eq!(origin.version, loaded.manifest.version);
    assert_eq!(origin.source, loaded.manifest.source);
    assert_eq!(body.instruments, loaded.manifest.instruments);
    assert_eq!(body.not_decision_grade, 0);
    assert_eq!(&body, platform.universe_assembled());

    // A cycle comes after it, never before: the record keeps its place, the
    // cycle's own entry lands later, and the chain still verifies.
    let report = platform.run_cycle(now);
    assert!(report.stages.iter().any(|stage| stage.ran), "no stage ran");
    let records = platform.event_log().records();
    assert!(
        records.len() > 1,
        "the cycle wrote nothing after the universe"
    );
    assert_eq!(
        StreamEnvelope::from_frame(&records[0].event)?
            .decode::<UniverseAssembled>()?
            .body,
        body
    );
    assert_eq!(
        platform
            .event_log()
            .by_topic(Topic::ReferenceDataUpdated)
            .len(),
        1,
        "the universe is recorded once, at assembly"
    );
    assert!(platform.event_log().verify_chain().is_ok());
    Ok(())
}

/// A universe built in-process from no catalogue is the state every test
/// fixture is in, and a deployment must never be: the record says so rather
/// than the log staying silent, so a replay reads "there was no catalogue"
/// as its first fact instead of discovering it.
#[test]
fn a_universe_assembled_from_no_catalogue_is_recorded_as_having_none() -> Result<()> {
    use qip_kernel::UniverseAssembled;
    use qip_streaming::envelope::StreamEnvelope;

    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        limits(),
    )?;
    let record = platform
        .event_log()
        .records()
        .first()
        .expect("an assembled platform has written at least one record");
    let first = StreamEnvelope::from_frame(&record.event)?
        .decode::<UniverseAssembled>()?
        .body;
    assert!(first.catalogue.is_none(), "{}", first.describe());
    assert_eq!(first.instruments, 0);
    assert!(
        first.describe().contains("no catalogue"),
        "{}",
        first.describe()
    );
    Ok(())
}
