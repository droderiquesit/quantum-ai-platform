//! The example connectors against the live endpoints. Opt-in, and skipped.
//!
//! These are the only tests in the crate that would open a socket to the
//! internet, and by default they do not: each needs an environment variable
//! naming a plaintext address, and with none set every test here prints why it
//! was skipped and passes. So `cargo test` on a laptop with no network, and CI
//! with no egress, both go green — which is what stops a network test from
//! being deleted the first week it flakes.
//!
//! # Why an address rather than a flag
//!
//! `qip_transport::http` has no TLS stack and refuses `https` by name. Both
//! sources are HTTPS only. So there is nothing a flag could turn on: reaching
//! them needs a TLS-terminating egress proxy, and the variable is that proxy's
//! address. A deployment that has one runs:
//!
//! ```text
//! QIP_LIVE_COINBASE_BASE_URL=http://egress.local:8080 \
//!   cargo test -p qip-market-ingestion --test live_connectors
//! ```
//!
//! # What a live run proves that the fixtures cannot
//!
//! That the recording still matches the source. A connector that passes the
//! contract harness and fails here has a stale fixture — which is a finding,
//! and the reason to re-record.
//!
//! # Why a live failure is reported and not swallowed
//!
//! Once the address *is* set, an unreachable source fails the test. A test
//! that skipped on any error would be a test that passes when the connector is
//! broken, which is worse than no test.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{ObjectId, Timestamp};
use qip_market_ingestion::connector::transport::{HttpSourceTransport, SourceTransport};
use qip_market_ingestion::connector::{
    ConnectorRuntime, PollOutcome, RuntimeConfig, SourceConnector, SourceManifest,
};
use qip_market_ingestion::connectors::{CoinbaseTickerConnector, FrankfurterRatesConnector};
use qip_transport::RecordingSleeper;
use std::sync::Arc;

/// Plaintext address of a TLS-terminating proxy in front of
/// `api.exchange.coinbase.com`.
const COINBASE_BASE_URL: &str = "QIP_LIVE_COINBASE_BASE_URL";
/// Plaintext address of a TLS-terminating proxy in front of
/// `api.frankfurter.app`.
const FRANKFURTER_BASE_URL: &str = "QIP_LIVE_FRANKFURTER_BASE_URL";

/// The address the deployment supplied, or `None` with a printed reason.
///
/// Printed rather than silent: a suite that skips without saying so is a suite
/// where nobody notices that the live coverage has been off for a year.
fn egress(variable: &str) -> Option<String> {
    match std::env::var(variable) {
        Ok(address) if !address.trim().is_empty() => Some(address),
        _ => {
            eprintln!(
                "skipped: {variable} is not set. This test reaches a public HTTPS endpoint \
                 through a TLS-terminating egress proxy, because qip_transport::http has no TLS \
                 stack and refuses https by name. Set {variable} to that proxy's plaintext \
                 address to run it."
            );
            None
        }
    }
}

fn configured(mut manifest: SourceManifest, base_url: String) -> Result<SourceManifest> {
    manifest.endpoint.base_url = Some(base_url);
    manifest.validate()?;
    Ok(manifest)
}

/// A live run's horizon. The wall clock is read *here*, in a test, and never
/// inside the connector — which is the whole point of the runtime taking the
/// instant as an argument.
fn horizon() -> Timestamp {
    use qip_core::Clock;
    qip_core::SystemClock.now()
}

fn runtime(manifest: SourceManifest) -> Result<ConnectorRuntime> {
    ConnectorRuntime::new(
        manifest,
        RuntimeConfig::seeded(0x1eaf_c0de).with_sleeper(Arc::new(RecordingSleeper::new())),
    )
}

#[test]
fn the_coinbase_ticker_connector_fetches_a_live_tick_through_a_configured_egress() -> Result<()> {
    let Some(base_url) = egress(COINBASE_BASE_URL) else {
        return Ok(());
    };
    let manifest = configured(CoinbaseTickerConnector::shipped_manifest()?, base_url)?;
    let mut connector = CoinbaseTickerConnector::new(
        manifest.clone(),
        "BTC-USD",
        ObjectId::from_string("OBJ0000000000000000BTCUSD"),
        CoinbaseTickerConnector::VENUE,
    )?;
    let mut transport = HttpSourceTransport::connect(&manifest)?;
    let mut runtime = runtime(manifest)?;
    let socket: &mut dyn SourceTransport = &mut transport;

    let health = runtime.connect(&mut connector, socket, horizon())?;
    assert!(health.reachable, "{}", health.detail);

    let report = runtime.poll(&mut connector, socket, horizon())?;
    assert_eq!(report.outcome, PollOutcome::Delivered);
    assert_eq!(
        report.admitted.len(),
        1,
        "the live ticker produced {} record(s) and {} quarantined; the recorded fixture may have \
         gone stale: {:?}",
        report.admitted.len(),
        report.quarantined,
        runtime.quarantine().recent(1)
    );
    runtime.shutdown(&mut connector, horizon())
}

#[test]
fn the_frankfurter_rates_connector_fetches_a_live_rate_table_through_a_configured_egress()
-> Result<()> {
    let Some(base_url) = egress(FRANKFURTER_BASE_URL) else {
        return Ok(());
    };
    let manifest = configured(FrankfurterRatesConnector::shipped_manifest()?, base_url)?;
    let mut connector = FrankfurterRatesConnector::new(manifest.clone())?;
    let mut transport = HttpSourceTransport::connect(&manifest)?;
    let mut runtime = runtime(manifest)?;
    let socket: &mut dyn SourceTransport = &mut transport;

    let health = runtime.connect(&mut connector, socket, horizon())?;
    assert!(health.reachable, "{}", health.detail);

    let report = runtime.poll(&mut connector, socket, horizon())?;
    assert_eq!(report.outcome, PollOutcome::Delivered);
    assert_eq!(
        report.admitted.len(),
        3,
        "the manifest asks for three currencies and {} arrived, with {} quarantined: {:?}",
        report.admitted.len(),
        report.quarantined,
        runtime.quarantine().recent(1)
    );
    runtime.shutdown(&mut connector, horizon())
}

#[test]
fn an_offline_connector_says_what_is_missing_rather_than_failing_obscurely() -> Result<()> {
    // This one runs everywhere, including with no network at all. It is what
    // makes "fails gracefully offline" a tested property rather than a claim
    // in a comment.
    let manifest = CoinbaseTickerConnector::shipped_manifest()?;
    let mut connector = CoinbaseTickerConnector::new(
        manifest.clone(),
        "BTC-USD",
        ObjectId::from_string("OBJ0000000000000000BTCUSD"),
        CoinbaseTickerConnector::VENUE,
    )?;

    let error = HttpSourceTransport::connect(&manifest)
        .expect_err("an unconfigured connector opened a socket");
    assert!(
        error.message().contains("base_url"),
        "the refusal does not name the field an operator has to set: {}",
        error.message()
    );
    assert_eq!(
        connector.manifest().source_id,
        "coinbase-spot-ticker",
        "an unconfigured connector still has to exist in order to say what it is missing"
    );
    connector.shutdown(horizon())
}

#[test]
fn a_source_that_cannot_be_reached_is_refused_and_dead_lettered_rather_than_hanging() -> Result<()>
{
    // Pointed at a port nothing listens on. Proves the offline path is a
    // bounded refusal — every wait in `HttpSourceTransport::LIMITS` is
    // explicit — and not a poll loop that never returns.
    let mut manifest = CoinbaseTickerConnector::shipped_manifest()?;
    // Port 1 on loopback: reachable network, nothing accepting.
    manifest.endpoint.base_url = Some("http://127.0.0.1:1".to_string());
    manifest.validate()?;
    let mut connector = CoinbaseTickerConnector::new(
        manifest.clone(),
        "BTC-USD",
        ObjectId::from_string("OBJ0000000000000000BTCUSD"),
        CoinbaseTickerConnector::VENUE,
    )?;
    let mut transport = HttpSourceTransport::connect(&manifest)?;
    let mut runtime = runtime(manifest)?;
    let socket: &mut dyn SourceTransport = &mut transport;

    let report = runtime.poll(&mut connector, socket, horizon())?;
    assert_eq!(report.outcome, PollOutcome::Refused);
    assert_eq!(
        runtime.quarantine().len(),
        1,
        "an unreachable source produced no dead letter, so the outage is a gap in the data with \
         nothing anywhere saying why"
    );
    Ok(())
}

// --- the connector feed, live: the whole bridge the loop will use ------------

use qip_market_ingestion::adapter::DataAdapter;
use qip_market_ingestion::connector_feed::ConnectorFeed;

#[test]
fn the_connector_feed_fetches_a_live_tick_through_the_same_egress() -> Result<()> {
    // The path a deployment actually takes, minus only the licensing gate,
    // which lives in the composition root above this crate: source selection
    // by name, the manifest's own transport, the runtime's probe and gates,
    // and the adapter contract the decision loop polls. A tick that arrives
    // here is a tick `Platform::observe` would absorb.
    let Some(base_url) = egress(COINBASE_BASE_URL) else {
        return Ok(());
    };
    let mut feed = ConnectorFeed::open("coinbase-spot-ticker", &base_url, 7, horizon())?;
    let records = feed.poll(horizon())?;
    assert!(
        !records.is_empty(),
        "the live source answered and the bridge delivered no record"
    );
    for record in &records {
        assert!(
            record.validate().is_empty(),
            "the live path produced a record the loop would reject: {record:?}"
        );
    }
    eprintln!(
        "live evidence: {} record(s) from {} ({:?})",
        records.len(),
        feed.descriptor().name,
        feed.descriptor().licensing
    );
    Ok(())
}
