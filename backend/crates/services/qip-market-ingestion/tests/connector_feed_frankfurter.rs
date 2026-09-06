//! `ConnectorFeed::open` against the Frankfurter source, over a real loopback
//! socket.
//!
//! `connector_contract.rs` proves the connector itself is correct against the
//! recorded emulator; `live_connectors.rs` proves the recording still matches
//! the vendor, opt-in and skipped with no network. Neither proves the bridge
//! this crate ships for the decision loop — [`ConnectorFeed::open`] — can
//! actually open Frankfurter by name: until this test existed, the source was
//! reachable only through [`ConnectorFeed::over_transport`] with a
//! hand-built connector, which every other production caller bypasses.
//! `open` is what a composition root calls, so this is the seam that has to
//! be proven: a name in, a real socket, a validated [`SensedRecord`] out.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the gate it is exercising still has
// to assert, and the abort is the reporting mechanism rather than a defect —
// the same allowance `connector_contract.rs` and `licensing.rs`'s own tests
// carry.
#![allow(clippy::panic_in_result_fn)]

mod server;

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_market_ingestion::connector_feed::{ConnectorFeed, shipped_class};
use qip_market_ingestion::connectors::FrankfurterRatesConnector;
use server::{Action, TestServer, address_with_no_listener};
use std::time::Duration as StdDuration;

/// The rate table's reference date is `2026-09-04`; the ECB's own sixteen-hour
/// publication delay is the manifest's, not this test's, so the horizon only
/// has to clear it — sixty hours after the reference date leaves no ambiguity.
fn horizon() -> Timestamp {
    Timestamp::parse_rfc3339("2026-09-07T00:00:00Z").expect("a literal instant parses")
}

/// The body `fixtures/frankfurter-ecb-reference-rates.json` records from the
/// live endpoint, unwrapped from the harness's own recording envelope.
const RATE_TABLE: &str = r#"{"amount":1.0,"base":"EUR","date":"2026-09-04","rates":{"GBP":0.85898,"JPY":181.59,"USD":1.1622}}"#;

/// The one path the shipped manifest requests, read from the manifest so a
/// repointed connector fails the premise below instead of being agreed with.
fn manifest_path() -> Result<String> {
    Ok(FrankfurterRatesConnector::shipped_manifest()?.endpoint.path)
}

#[test]
fn the_connector_feed_opens_frankfurter_by_name_and_polls_real_rates() -> Result<()> {
    // Both the health probe `ConnectorFeed::open` performs during connect and
    // the poll that follows hit the same `/v1/latest` path, so one scripted
    // answer serves both — the manifest's own `health_path`.
    //
    // Premise: the path is the versioned one the vendor serves since it
    // moved hosts. The unversioned `/latest` answers 404 on
    // `api.frankfurter.dev`, and a server here that answered any path would
    // hide a manifest that had drifted back.
    let path = manifest_path()?;
    assert_eq!(
        path, "/v1/latest",
        "the shipped manifest requests {path}; the vendor serves /v1/latest and nothing else"
    );
    let server = TestServer::always(Action::json(200, RATE_TABLE));

    let mut feed = ConnectorFeed::open(
        FrankfurterRatesConnector::SOURCE_ID,
        &server.url(),
        7,
        horizon(),
    )?;

    let descriptor = feed.descriptor();
    assert_eq!(descriptor.name, "frankfurter-ecb-reference-rates");
    assert_eq!(descriptor.licensing, LicensingClass::Public);
    assert_eq!(
        descriptor.topics,
        vec![qip_events::Topic::MacroUpdated],
        "a fan-out of exchange rates is a macro observation, not a tick"
    );

    let records = feed.poll(horizon())?;
    assert!(
        !records.is_empty(),
        "the scripted rate table produced no record through the bridge"
    );
    assert_eq!(
        records.len(),
        3,
        "the manifest asks for three currencies (GBP, JPY, USD)"
    );
    for record in &records {
        assert!(
            record.validate().is_empty(),
            "the bridge produced a record the loop would reject: {record:?}"
        );
        assert!(
            matches!(record, SensedRecord::Macro(_)),
            "frankfurter's records must arrive as macro observations: {record:?}"
        );
    }
    assert!(
        server.served() >= 2,
        "expected at least the health probe and one poll, saw {} request(s)",
        server.served()
    );
    // Every request the bridge sent actually reached `/v1/latest` — the
    // manifest's own endpoint path — over a plain GET. A bridge that
    // silently reformed the request (a different path, a body on a GET)
    // would still pass on the response alone; the recorded request is the
    // only place that mistake is visible.
    let requests = server.requests();
    assert!(
        !requests.is_empty(),
        "the server recorded no request at all, so the assertions above proved nothing"
    );
    for request in &requests {
        assert_eq!(request.method, "GET");
        assert!(
            request.target.starts_with(&path),
            "expected the manifest's own path {path}, got {}",
            request.target
        );
        // The manifest declares `auth: {"scheme": "none"}` — Frankfurter
        // needs no credential. A request carrying one anyway would mean a
        // stray header travelled from a different connector's configuration,
        // and the recorded request is the only place that would show up.
        assert!(
            !request.headers.contains_key("x-api-key")
                && !request.headers.contains_key("authorization"),
            "an unauthenticated source's request carried a credential header: {:?}",
            request.headers
        );
    }

    Ok(())
}

#[test]
fn the_shipped_class_for_frankfurter_matches_its_manifest_before_any_socket_opens() -> Result<()> {
    // The licensing gate a composition root runs reads this before
    // constructing anything; a class this function got wrong would let an
    // evaluation compare against the wrong answer and admit a source under
    // false pretences.
    let class = shipped_class(FrankfurterRatesConnector::SOURCE_ID)?;
    assert_eq!(class, LicensingClass::Public);
    Ok(())
}

#[test]
fn an_unreachable_frankfurter_source_is_refused_rather_than_hanging_the_composition_root() {
    // The same property `live_connectors.rs` proves for Coinbase, at the
    // level a composition root actually calls: `ConnectorFeed::open` must
    // fail loudly during its own health check rather than construct a feed
    // that looks live and then times out on the first real poll.
    let refused = ConnectorFeed::open(
        FrankfurterRatesConnector::SOURCE_ID,
        &address_with_no_listener(),
        7,
        horizon(),
    );
    assert!(
        refused.is_err(),
        "a connector feed opened against a socket nothing listens on"
    );
}

#[test]
fn a_rate_table_that_dies_mid_body_refuses_the_feed_rather_than_absorbing_a_partial_table() {
    // A partial JSON body decoded as if complete could publish some
    // currencies and silently drop the rest — indistinguishable downstream
    // from a quiet source. The health probe this test drives through is
    // where that has to be caught, since `ConnectorFeed::open` never
    // returns a feed whose first exchange already failed.
    let server = TestServer::always(Action::Truncated {
        declared: RATE_TABLE.len(),
        written: RATE_TABLE.len() / 3,
    });
    let refused = ConnectorFeed::open(
        FrankfurterRatesConnector::SOURCE_ID,
        &server.url(),
        7,
        horizon(),
    );
    assert!(
        refused.is_err(),
        "a truncated rate table was accepted as a healthy source"
    );
}

#[test]
fn a_rate_table_larger_than_the_transport_limit_is_refused_before_it_is_buffered() {
    // `HttpSourceTransport::LIMITS.max_body` is a megabyte, chosen for a
    // ticker or a rate table — generous, and refusing past it costs nothing.
    // A transport that buffered past its own declared bound would turn a
    // vendor's runaway response into this process's memory problem.
    let server = TestServer::always(Action::Oversized {
        bytes: 2 * 1024 * 1024,
    });
    let refused = ConnectorFeed::open(
        FrankfurterRatesConnector::SOURCE_ID,
        &server.url(),
        7,
        horizon(),
    );
    assert!(
        refused.is_err(),
        "a two-megabyte response was accepted from a source whose limit is one"
    );
}

#[test]
fn a_silent_frankfurter_source_is_refused_within_its_own_timeout() {
    // The bounded-wait counterpart to the two refusals above: a peer that
    // accepts the connection and then says nothing must not hang the
    // composition root either. Longer than any client timeout this crate
    // configures, so a client that forgot to bound its wait would make this
    // test hang rather than merely fail.
    let server = TestServer::always(Action::Silent(StdDuration::from_secs(30)));
    let refused = ConnectorFeed::open(
        FrankfurterRatesConnector::SOURCE_ID,
        &server.url(),
        7,
        horizon(),
    );
    assert!(
        refused.is_err(),
        "a source that never answered was treated as reachable"
    );
}
