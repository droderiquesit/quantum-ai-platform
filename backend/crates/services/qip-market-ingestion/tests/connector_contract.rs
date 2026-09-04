//! Both example connectors, through the contract harness, with no network.
//!
//! The fixtures are bodies recorded from the live endpoints. Running the
//! harness against them proves the connectors are self-consistent — the
//! manifest matches the decoder, the fingerprint is stable across two decodes
//! of the same bytes, the cursor resumes, a broken payload is quarantined. It
//! does not prove the recordings still match the sources; that is what the
//! opt-in tests in `live_connectors.rs` are for.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp};
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::connector::emulator::SourceEmulator;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{
    ConnectorRuntime, ContractHarness, ContractReport, RuntimeConfig, SourceConnector,
};
use qip_market_ingestion::connectors::{
    CoinbaseTickerConnector, FrankfurterRatesConnector, coinbase_ticker, frankfurter_rates,
};
use qip_transport::RecordingSleeper;
use std::sync::Arc;

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}

/// After the recorded Coinbase trade at 14:59:41Z, on a feed with no
/// dissemination delay.
fn coinbase_horizon() -> Timestamp {
    at("2026-08-24T15:00:00Z")
}

/// After the recorded rate table's reference date plus the ECB's sixteen-hour
/// publication delay, so the rates are knowable rather than correctly withheld.
fn frankfurter_horizon() -> Timestamp {
    at("2026-09-05T09:00:00Z")
}

fn coinbase() -> Result<(CoinbaseTickerConnector, SourceEmulator)> {
    let connector = CoinbaseTickerConnector::new(
        CoinbaseTickerConnector::shipped_manifest()?,
        "BTC-USD",
        ObjectId::from_string("OBJ0000000000000000BTCUSD"),
        CoinbaseTickerConnector::VENUE,
    )?;
    Ok((
        connector,
        SourceEmulator::from_json(coinbase_ticker::FIXTURE)?,
    ))
}

fn frankfurter() -> Result<(FrankfurterRatesConnector, SourceEmulator)> {
    let connector = FrankfurterRatesConnector::new(FrankfurterRatesConnector::shipped_manifest()?)?;
    Ok((
        connector,
        SourceEmulator::from_json(frankfurter_rates::FIXTURE)?,
    ))
}

fn runtime_for(connector: &dyn SourceConnector) -> Result<ConnectorRuntime> {
    let config = RuntimeConfig::seeded(11).with_sleeper(Arc::new(RecordingSleeper::new()));
    ConnectorRuntime::new(connector.manifest().clone(), config)
}

fn assert_report(report: &ContractReport) {
    assert!(
        report.passed(),
        "a connector failed the contract every connector must pass:\n{}",
        report.describe()
    );
}

#[test]
fn the_coinbase_ticker_connector_passes_the_connector_contract() -> Result<()> {
    let (mut connector, mut emulator) = coinbase()?;
    let report = ContractHarness::new(coinbase_horizon()).run(&mut connector, &mut emulator)?;

    assert_report(&report);
    assert!(
        report.checks.len() >= 10,
        "the harness ran only {} checks, so a connector could pass it without being exercised",
        report.checks.len()
    );
    Ok(())
}

#[test]
fn the_frankfurter_rates_connector_passes_the_connector_contract() -> Result<()> {
    let (mut connector, mut emulator) = frankfurter()?;
    let report = ContractHarness::new(frankfurter_horizon()).run(&mut connector, &mut emulator)?;

    assert_report(&report);
    Ok(())
}

#[test]
fn the_coinbase_connector_decodes_the_recorded_ticker_into_one_tick() -> Result<()> {
    let (mut connector, mut emulator) = coinbase()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, coinbase_horizon())?;
    let envelope = report
        .admitted
        .first()
        .expect("the recorded Coinbase ticker produced no record");

    match envelope.record() {
        SensedRecord::Tick(tick) => {
            assert_eq!(tick.price.to_string(), "64230.99");
            assert_eq!(
                tick.volume.to_string(),
                "0.001843",
                "the tick carries the product's rolling 24-hour volume instead of the size of \
                 the print it reports, which would make every tick look like a nine-thousand \
                 bitcoin trade"
            );
            assert_eq!(tick.venue, CoinbaseTickerConnector::VENUE);
            assert_eq!(tick.at, at("2026-08-24T14:59:41.812734Z"));
        }
        other => panic!("the Coinbase ticker decoded into {other:?} rather than a tick"),
    }
    assert_eq!(
        envelope.upstream_key(),
        "712553481",
        "the record does not carry the source's own trade id, so a reconciliation against \
         Coinbase has nothing to join on"
    );
    Ok(())
}

#[test]
fn the_frankfurter_connector_fans_one_rate_table_out_into_one_observation_per_pair() -> Result<()> {
    // One record for the table would make a source that dropped a currency
    // indistinguishable from one that changed every rate.
    let (mut connector, mut emulator) = frankfurter()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, frankfurter_horizon())?;
    assert_eq!(
        report.admitted.len(),
        3,
        "the recorded table holds three pairs"
    );

    let mut series: Vec<String> = report
        .admitted
        .iter()
        .map(|envelope| match envelope.record() {
            SensedRecord::Macro(observation) => observation.series_id.clone(),
            other => panic!("a reference rate decoded into {other:?}"),
        })
        .collect();
    series.sort();
    assert_eq!(series, ["FX.EUR.GBP", "FX.EUR.JPY", "FX.EUR.USD"]);

    let usd = report
        .admitted
        .iter()
        .find(|envelope| envelope.upstream_key().starts_with("EUR/USD"))
        .expect("the table holds a USD rate");
    match usd.record() {
        SensedRecord::Macro(observation) => {
            assert!((observation.value - 1.1622).abs() < 1e-12);
            assert_eq!(observation.unit, "USD per EUR");
            assert_eq!(observation.region, FrankfurterRatesConnector::REGION);
            assert_eq!(observation.reference_date, at("2026-09-04T00:00:00Z"));
        }
        other => panic!("a reference rate decoded into {other:?}"),
    }
    Ok(())
}

#[test]
fn a_reference_rate_is_withheld_until_the_ecb_would_have_published_it() -> Result<()> {
    // The clearest case for keeping event time and knowable time apart: a
    // backtest filtering on event time would use the day's closing reference
    // rate to trade that day's open.
    let (mut connector, mut emulator) = frankfurter()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let too_early = runtime.poll(&mut connector, transport, at("2026-09-04T09:00:00Z"))?;
    assert!(
        too_early.admitted.is_empty(),
        "a reference rate stamped 2026-09-04 was published at 09:00 on 2026-09-04, seven hours \
         before the ECB would have released it"
    );
    assert_eq!(too_early.withheld, 3);

    let knowable = runtime.poll(&mut connector, transport, frankfurter_horizon())?;
    assert_eq!(
        knowable.admitted.len(),
        3,
        "the withheld rates never arrived on a later poll"
    );
    for envelope in &knowable.admitted {
        assert_eq!(
            envelope.knowable_at(),
            envelope
                .event_time()
                .saturating_add(Duration::from_hours(16))
        );
    }
    Ok(())
}

#[test]
fn a_connector_whose_manifest_fetches_another_product_is_refused_at_construction() -> Result<()> {
    // Left alone, every tick from one product would be published under the
    // other's instrument id: a wrong price on a real instrument rather than an
    // error anybody sees.
    let error = CoinbaseTickerConnector::new(
        CoinbaseTickerConnector::shipped_manifest()?,
        "ETH-USD",
        ObjectId::from_string("OBJ0000000000000000ETHUSD"),
        CoinbaseTickerConnector::VENUE,
    )
    .expect_err("a connector mapped a product its manifest does not fetch");
    assert!(
        error.message().contains("/products/ETH-USD/ticker"),
        "the refusal does not name the path the connector expected: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_rate_source_that_declares_no_dissemination_delay_is_refused() -> Result<()> {
    let mut manifest = FrankfurterRatesConnector::shipped_manifest()?;
    manifest.publication_delay_ms = 0;

    let error = FrankfurterRatesConnector::new(manifest)
        .expect_err("a delayed publication was configured as an instantaneous one");
    assert!(
        error.message().contains("trades the open on the close"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_harness_fails_a_connector_whose_manifest_breaches_its_own_rate_limit() -> Result<()> {
    // A harness that passed everything would prove nothing, so this asserts it
    // can fail — and names the check that catches it.
    const CHECK: &str = "the poll interval stays inside the source's own rate limit";
    let mut manifest = CoinbaseTickerConnector::shipped_manifest()?;
    manifest.rate_limit.requests = 1;
    manifest.rate_limit.burst = 4;
    manifest.rate_limit.per_ms = 3_600_000;
    let mut connector = CoinbaseTickerConnector::new(
        CoinbaseTickerConnector::shipped_manifest()?,
        "BTC-USD",
        ObjectId::from_string("OBJ0000000000000000BTCUSD"),
        CoinbaseTickerConnector::VENUE,
    )?;
    // Swap the manifest under the connector, which is what a bad edit does.
    let mut emulator = SourceEmulator::from_json(coinbase_ticker::FIXTURE)?;
    let broken = BrokenManifest {
        inner: &mut connector,
        manifest,
    };
    let mut broken = broken;
    let report = ContractHarness::new(coinbase_horizon()).run(&mut broken, &mut emulator)?;

    let check = report
        .check(CHECK)
        .unwrap_or_else(|| panic!("the harness no longer runs the check named {CHECK:?}"));
    assert!(!check.passed, "{}", report.describe());
    assert!(!report.passed());
    Ok(())
}

/// A connector wearing a manifest that does not match it.
///
/// Exists so the harness can be shown failing. Every method delegates except
/// the manifest, which is the field a bad edit changes.
#[derive(Debug)]
struct BrokenManifest<'a> {
    inner: &'a mut CoinbaseTickerConnector,
    manifest: qip_market_ingestion::connector::SourceManifest,
}

impl SourceConnector for BrokenManifest<'_> {
    fn manifest(&self) -> &qip_market_ingestion::connector::SourceManifest {
        &self.manifest
    }

    fn decode(
        &self,
        payload: &serde_json::Value,
        cursor: &qip_market_ingestion::connector::Cursor,
    ) -> Result<Vec<qip_market_ingestion::connector::RawEvent>> {
        self.inner.decode(payload, cursor)
    }

    fn map(
        &self,
        event: &qip_market_ingestion::connector::RawEvent,
        ingest_time: Timestamp,
    ) -> Result<SensedRecord> {
        self.inner.map(event, ingest_time)
    }
}

// --- the connector as a feed: the bridge into the decision loop --------------

use qip_financial::quality::LicensingClass;
use qip_market_ingestion::adapter::DataAdapter;
use qip_market_ingestion::connector_feed::{ConnectorFeed, KNOWN_SOURCES, shipped_class};

#[test]
fn the_connector_feed_polls_a_recorded_source_into_sensed_records() -> Result<()> {
    // The bridge, end to end with no socket: the same runtime path a
    // deployment takes, over the recorded emulator instead of the egress
    // proxy. What this pins is that a connector's admitted envelopes really
    // do come out of the adapter contract as records the loop can absorb —
    // the seam gap-matrix item 6 named.
    let manifest = CoinbaseTickerConnector::shipped_manifest()?;
    let mut manifest = manifest;
    manifest.endpoint.base_url = Some("http://egress.test:8080".to_string());
    let connector = CoinbaseTickerConnector::new(
        manifest.clone(),
        "BTC-USD",
        ObjectId::from_string("BTC-USD"),
        "COINBASE",
    )?;
    let emulator = SourceEmulator::from_json(coinbase_ticker::FIXTURE)?;

    let mut feed = ConnectorFeed::over_transport(
        Box::new(connector),
        manifest,
        Box::new(emulator),
        11,
        coinbase_horizon(),
    )?;

    // The descriptor carries the manifest's own identity and licensing, so
    // every record's provenance says what its source's terms were.
    let descriptor = feed.descriptor();
    assert_eq!(descriptor.name, "coinbase-spot-ticker");
    assert_eq!(descriptor.licensing, LicensingClass::Internal);
    assert!(
        descriptor.is_production_grade(),
        "an internal-licensed live source must be admissible for decisions"
    );

    let records = feed.poll(coinbase_horizon())?;
    assert!(
        !records.is_empty(),
        "the recorded fixture produced no records through the bridge"
    );
    for record in &records {
        assert!(
            record.validate().is_empty(),
            "the bridge produced a record the loop would reject: {record:?}"
        );
    }
    Ok(())
}

#[test]
fn the_shipped_class_and_the_known_sources_agree_with_the_manifests() -> Result<()> {
    // `shipped_class` is what the licensing gate reads before anything opens,
    // so it must be the manifest's own claim and not a copy that can drift.
    assert_eq!(
        shipped_class("coinbase-spot-ticker")?,
        CoinbaseTickerConnector::shipped_manifest()?.licensing
    );
    assert_eq!(
        shipped_class("coinbase-spot-ticker")?,
        LicensingClass::Internal,
        "the Coinbase evaluation concluded internal — no redistribution — and \
         the manifest must say so"
    );
    assert!(shipped_class("unknown-source").is_err());
    // Premise for every list-driven check: the list is not empty.
    assert!(!KNOWN_SOURCES.is_empty());
    Ok(())
}
