//! Both example connectors, through the contract harness, with no network.
//!
//! The fixtures are bodies recorded from the live endpoints. Running the
//! harness against them proves the connectors are self-consistent — the
//! manifest matches the decoder, the fingerprint is stable across two decodes
//! of the same bytes, the cursor resumes, a broken payload is quarantined. It
//! does not prove the recordings still match the sources; that is what the
//! opt-in tests in `live_connectors.rs` are for.

#![allow(clippy::panic_in_result_fn)]

// The loopback server is shared by every integration-test binary and compiled
// into each on its own; this binary scripts one of its four answers, and the
// other three are genuinely unused here rather than a helper nobody calls.
#[allow(dead_code)]
mod server;

use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp};
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::connector::emulator::SourceEmulator;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{
    ConnectorRuntime, ContractHarness, ContractReport, RuntimeConfig, SourceConnector,
};
use qip_market_ingestion::connectors::{
    AlpacaBarsConnector, CoinbaseTickerConnector, FrankfurterRatesConnector,
    KalshiMarketsConnector, alpaca_bars, coinbase_ticker, frankfurter_rates, kalshi_markets,
};
use qip_transport::RecordingSleeper;
use std::collections::BTreeMap;
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

/// After the newest `updated_time` in the Kalshi recording (02:07:00Z on
/// 2026-09-05), on a feed with no dissemination delay.
fn kalshi_horizon() -> Timestamp {
    at("2026-09-05T03:00:00Z")
}

/// After the placeholder's newest session midnight (2026-09-04T04:00Z) plus
/// the seventeen-hour delay, so both sessions' bars are knowable.
fn alpaca_horizon() -> Timestamp {
    at("2026-09-05T12:00:00Z")
}

fn kalshi() -> Result<(KalshiMarketsConnector, SourceEmulator)> {
    let connector = KalshiMarketsConnector::new(KalshiMarketsConnector::shipped_manifest()?)?;
    Ok((
        connector,
        SourceEmulator::from_json(kalshi_markets::FIXTURE)?,
    ))
}

fn alpaca() -> Result<(AlpacaBarsConnector, SourceEmulator)> {
    let instruments: BTreeMap<String, ObjectId> = [("AAPL", "OBJ-AAPL"), ("MSFT", "OBJ-MSFT")]
        .into_iter()
        .map(|(symbol, id)| (symbol.to_string(), ObjectId::from_string(id)))
        .collect();
    let connector =
        AlpacaBarsConnector::new(AlpacaBarsConnector::shipped_manifest()?, instruments)?;
    Ok((connector, SourceEmulator::from_json(alpaca_bars::FIXTURE)?))
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
fn the_kalshi_markets_connector_passes_the_connector_contract() -> Result<()> {
    let (mut connector, mut emulator) = kalshi()?;
    let report = ContractHarness::new(kalshi_horizon()).run(&mut connector, &mut emulator)?;

    assert_report(&report);
    // The health probe hit the status endpoint and not the 40 KB page.
    assert!(
        emulator
            .calls()
            .iter()
            .any(|target| target.contains("/trade-api/v2/exchange/status")),
        "the health check never asked the status endpoint: {:?}",
        emulator.calls()
    );
    Ok(())
}

#[test]
fn the_alpaca_bars_connector_passes_the_connector_contract() -> Result<()> {
    let (mut connector, mut emulator) = alpaca()?;
    let report = ContractHarness::new(alpaca_horizon()).run(&mut connector, &mut emulator)?;

    assert_report(&report);
    Ok(())
}

#[test]
fn the_kalshi_connector_publishes_nineteen_quotes_and_quarantines_the_empty_book() -> Result<()> {
    // The recording holds twenty markets, one of them with no resting order
    // on either side. One quarantine with a reason, nineteen quotes — not a
    // lost page, and not a 0/0 quote.
    let (mut connector, mut emulator) = kalshi()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, kalshi_horizon())?;
    assert_eq!(report.admitted.len(), 19, "{report:?}");
    assert_eq!(report.quarantined, 1);
    assert_eq!(report.withheld, 0);

    let quoted = report
        .admitted
        .iter()
        .find(|envelope| envelope.upstream_key() == "KXUSL1HTOTAL-26SEP05MONPHO-2")
        .expect("the two-sided market is in the recording");
    match quoted.record() {
        SensedRecord::Quote(quote) => {
            assert_eq!(quote.bid.to_string(), "0.34");
            assert_eq!(quote.ask.to_string(), "0.4");
            assert_eq!(quote.bid_size.to_string(), "564");
            assert_eq!(quote.ask_size.to_string(), "50");
            assert_eq!(quote.venue, KalshiMarketsConnector::VENUE);
            assert_eq!(quote.at, at("2026-09-05T02:07:00.686274Z"));
            assert_eq!(
                quote.object_id.as_str(),
                "KALSHI:KXUSL1HTOTAL-26SEP05MONPHO-2"
            );
        }
        other => panic!("a Kalshi market decoded into {other:?} rather than a quote"),
    }
    for envelope in &report.admitted {
        assert!(
            envelope.record().validate().is_empty(),
            "a published quote fails the platform's own validation: {:?}",
            envelope.record().validate()
        );
    }
    assert!(
        !report
            .admitted
            .iter()
            .any(|envelope| envelope.upstream_key() == "KXUSL1HTOTAL-26SEP05MONPHO-3"),
        "the market with an empty book was published"
    );
    Ok(())
}

#[test]
fn the_alpaca_connector_decodes_the_placeholder_into_four_daily_bars() -> Result<()> {
    let (mut connector, mut emulator) = alpaca()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, alpaca_horizon())?;
    assert_eq!(report.admitted.len(), 4, "{report:?}");

    let aapl = report
        .admitted
        .iter()
        .find(|envelope| envelope.upstream_key() == "AAPL@2026-09-03T04:00:00Z")
        .expect("the placeholder holds an AAPL bar for 2026-09-03");
    match aapl.record() {
        SensedRecord::Bar(bar) => {
            assert_eq!(bar.open.to_string(), "189.2");
            assert_eq!(bar.close.to_string(), "190.1");
            assert_eq!(bar.volume.to_string(), "52341");
            assert_eq!(bar.venue, AlpacaBarsConnector::VENUE);
            assert_eq!(bar.open_time, at("2026-09-03T04:00:00Z"));
            assert!(bar.is_coherent());
        }
        other => panic!("an Alpaca bar decoded into {other:?}"),
    }
    Ok(())
}

#[test]
fn a_daily_bar_is_withheld_until_the_session_it_reports_has_closed() -> Result<()> {
    // A bar stamped at the session's midnight, polled at noon that day, is
    // the day's close handed to a backtest before the day happened.
    let (mut connector, mut emulator) = alpaca()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let midday = runtime.poll(&mut connector, transport, at("2026-09-04T12:00:00Z"))?;
    // Premise: the 09-03 bars are already knowable at this instant, so the
    // withholding below is the 09-04 session's alone.
    assert_eq!(midday.admitted.len(), 2, "{midday:?}");
    assert_eq!(midday.withheld, 2);
    assert!(
        !midday
            .admitted
            .iter()
            .any(|envelope| envelope.upstream_key().ends_with("2026-09-04T04:00:00Z")),
        "a bar for the session in progress was published at midday"
    );

    let after_close = runtime.poll(&mut connector, transport, alpaca_horizon())?;
    assert_eq!(after_close.admitted.len(), 2, "{after_close:?}");
    for envelope in &after_close.admitted {
        assert_eq!(
            envelope.knowable_at(),
            envelope
                .event_time()
                .saturating_add(Duration::from_hours(17))
        );
    }
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

    // The two ADR 0034 candidates whose terms are unread declare the
    // fail-closed floor, and the bridge reports exactly that class — the
    // gate compares it with the catalogue's, and a manifest quietly relaxed
    // to `public` would disagree and be refused.
    for (source_id, connector_class) in [
        (
            KalshiMarketsConnector::SOURCE_ID,
            KalshiMarketsConnector::shipped_manifest()?.licensing,
        ),
        (
            AlpacaBarsConnector::SOURCE_ID,
            AlpacaBarsConnector::shipped_manifest()?.licensing,
        ),
    ] {
        assert!(
            KNOWN_SOURCES.contains(&source_id),
            "{source_id} is not openable by name"
        );
        assert_eq!(shipped_class(source_id)?, connector_class);
        assert_eq!(
            shipped_class(source_id)?,
            LicensingClass::Restricted,
            "{source_id}'s terms have not been read (ADR 0034), so its manifest must declare \
             the most restrictive class short of synthetic until they are"
        );
    }
    Ok(())
}

#[test]
fn every_known_source_opens_by_name_through_the_bridge_over_its_own_fixture() -> Result<()> {
    // `KNOWN_SOURCES` is the list a deployment selects from, and a name on
    // it that `ConnectorFeed` cannot actually construct is a source that
    // fails at start-up with a message about a missing arm. Each is opened
    // over its recorded (or placeholder) fixture through the same
    // `over_transport` path `open` takes after it builds the transport.
    assert_eq!(KNOWN_SOURCES.len(), 4, "{KNOWN_SOURCES:?}");
    let (kalshi, kalshi_emulator) = kalshi()?;
    let (alpaca, alpaca_emulator) = alpaca()?;
    let cases: Vec<(
        Box<dyn SourceConnector + Send>,
        SourceEmulator,
        Timestamp,
        usize,
    )> = vec![
        (Box::new(kalshi), kalshi_emulator, kalshi_horizon(), 19),
        (Box::new(alpaca), alpaca_emulator, alpaca_horizon(), 4),
    ];
    for (connector, emulator, horizon, expected) in cases {
        let manifest = connector.manifest().clone();
        let source_id = manifest.source_id.clone();
        let mut feed =
            ConnectorFeed::over_transport(connector, manifest, Box::new(emulator), 11, horizon)?;
        let records = feed.poll(horizon)?;
        assert_eq!(records.len(), expected, "{source_id} produced {records:?}");
        let topics: std::collections::BTreeSet<_> =
            records.iter().map(|record| record.topic()).collect();
        assert_eq!(
            topics.len(),
            1,
            "{source_id} publishes on more than one topic: {topics:?}"
        );
        assert_eq!(
            feed.descriptor().topics,
            topics.into_iter().collect::<Vec<_>>(),
            "{source_id}'s descriptor promises a topic its records do not carry"
        );
    }
    Ok(())
}

// --- two credential headers over a real socket -------------------------------

use qip_market_ingestion::connector::manifest::SecretRef;
use qip_market_ingestion::connector::transport::{HttpSourceTransport, REDACTED, SourceRequest};
use server::{Action, TestServer, address_with_no_listener};

const SECRET_KEY_VALUE: &str = "test-secret-value-9f2a";
const KEY_ID_VALUE: &str = "test-key-id-7b3c";

/// The shipped Alpaca manifest pointed at `base_url`, and a resolver that
/// reads its two credentials from two files through the `_FILE` variables —
/// the shape a Secret Manager volume gives a deployment — and nothing from
/// the process environment.
fn alpaca_at(
    base_url: &str,
    tag: &str,
) -> Result<(
    qip_market_ingestion::connector::SourceManifest,
    std::path::PathBuf,
    BTreeMap<String, String>,
)> {
    let mut manifest = AlpacaBarsConnector::shipped_manifest()?;
    manifest.endpoint.base_url = Some(base_url.to_string());
    let dir = std::env::temp_dir().join(format!(
        "qip-alpaca-two-header-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("secret-key"), format!("{SECRET_KEY_VALUE}\n"))?;
    std::fs::write(dir.join("key-id"), format!("{KEY_ID_VALUE}\n"))?;
    let lookup: BTreeMap<String, String> = [
        ("QIP_ALPACA_API_SECRET_KEY_FILE", dir.join("secret-key")),
        ("QIP_ALPACA_API_KEY_ID_FILE", dir.join("key-id")),
    ]
    .into_iter()
    .map(|(name, path)| (name.to_string(), path.display().to_string()))
    .collect();
    Ok((manifest, dir, lookup))
}

#[test]
fn the_alpaca_transport_sends_both_headers_from_two_files_and_writes_neither_to_any_line()
-> Result<()> {
    // The failure this guards has two halves. First, the connector shipped at
    // 712c5d3 sent only `apca-api-secret-key`, which Alpaca answers 401; the
    // server here sees both headers with the values the two files hold, in
    // the order the manifest names them. Second, a vendor's error body can
    // echo the request headers, and `body_excerpt` is quoted into the health
    // detail by design — so the body below echoes both values and the test
    // proves no line the runtime produces carries either.
    let echo = format!(
        r#"{{"error":"upstream fault","echoed_headers":{{"apca-api-key-id":"{KEY_ID_VALUE}","apca-api-secret-key":"{SECRET_KEY_VALUE}"}}}}"#
    );
    assert!(
        echo.contains(SECRET_KEY_VALUE) && echo.contains(KEY_ID_VALUE),
        "premise: the vendor's body echoes both credentials"
    );
    let server = TestServer::always(Action::json(503, echo));
    let (manifest, dir, lookup) = alpaca_at(&server.url(), "echo")?;
    let environment = |name: &str| lookup.get(name).cloned();
    let resolve = |secret: &SecretRef| secret.resolve_with(&environment);

    let mut transport = HttpSourceTransport::connect_with(&manifest, &resolve)?;
    assert_eq!(
        transport.credential_headers(),
        [
            AlpacaBarsConnector::SECRET_KEY_HEADER,
            AlpacaBarsConnector::KEY_ID_HEADER
        ],
        "the primary header is written first and the companion second"
    );
    let debug = format!("{transport:?}");
    assert!(
        debug.contains(AlpacaBarsConnector::KEY_ID_HEADER)
            && !debug.contains(SECRET_KEY_VALUE)
            && !debug.contains(KEY_ID_VALUE),
        "Debug names the headers and never a value: {debug}"
    );

    // One request straight through the transport: both headers arrive with
    // the files' contents, newline stripped, and the body comes back scrubbed.
    let request = SourceRequest::get(&manifest.endpoint.path).for_health();
    let response = transport.request(&request, alpaca_horizon())?;
    assert_eq!(response.status, 503);
    assert_eq!(
        server.served(),
        1,
        "premise: exactly one request reached the socket"
    );
    let received = server.requests();
    assert_eq!(
        received[0].method, "GET",
        "premise: the transport issues GET and only GET"
    );
    let headers = &received[0].headers;
    assert_eq!(
        headers
            .get(AlpacaBarsConnector::SECRET_KEY_HEADER)
            .map(String::as_str),
        Some(SECRET_KEY_VALUE),
        "{headers:?}"
    );
    assert_eq!(
        headers
            .get(AlpacaBarsConnector::KEY_ID_HEADER)
            .map(String::as_str),
        Some(KEY_ID_VALUE),
        "{headers:?}"
    );
    assert!(
        !response.body.contains(SECRET_KEY_VALUE) && !response.body.contains(KEY_ID_VALUE),
        "a credential the vendor echoed left the transport in the body"
    );
    assert_eq!(
        response.body.matches(REDACTED).count(),
        2,
        "both echoed values are marked, not silently cut: {}",
        response.body
    );

    // And through the runtime's health probe, which quotes the excerpt into
    // its detail: the vendor's words survive, the credentials do not.
    let (mut connector, _) = alpaca()?;
    let mut runtime = runtime_for(&connector)?;
    let health = runtime.health(&mut connector, &mut transport, alpaca_horizon())?;
    assert!(!health.reachable);
    assert!(
        health.detail.contains("upstream fault") && health.detail.contains(REDACTED),
        "the health detail no longer quotes the vendor's body: {}",
        health.detail
    );
    assert!(
        !health.detail.contains(SECRET_KEY_VALUE) && !health.detail.contains(KEY_ID_VALUE),
        "a credential reached the health detail: {}",
        health.detail
    );

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn an_alpaca_transport_missing_one_of_its_two_files_names_that_variable_and_no_value() -> Result<()>
{
    // A deployment that mounted the secret key and forgot the key id must be
    // told which one, at connect and not at the first 401 — and the message
    // that tells it must not quote the credential it did find.
    let (manifest, dir, lookup) = alpaca_at(&address_with_no_listener(), "half")?;
    let only_secret_key = |name: &str| {
        (name == "QIP_ALPACA_API_SECRET_KEY_FILE")
            .then(|| lookup.get(name).cloned())
            .flatten()
    };
    let resolve = |secret: &SecretRef| secret.resolve_with(&only_secret_key);
    assert!(
        resolve(manifest.auth.secret.as_ref().expect("the primary is named"))?.is_some(),
        "premise: the secret key resolves"
    );

    let error = HttpSourceTransport::connect_with(&manifest, &resolve)
        .expect_err("a transport with half its credential opened");
    let message = error.message();
    assert!(
        message.contains("`QIP_ALPACA_API_KEY_ID`")
            && !message.contains("`QIP_ALPACA_API_SECRET_KEY`"),
        "the refusal names the wrong variable: {message}"
    );
    assert!(
        !message.contains(SECRET_KEY_VALUE),
        "the refusal quotes the credential that was found: {message}"
    );

    std::fs::remove_dir_all(dir)?;
    Ok(())
}
