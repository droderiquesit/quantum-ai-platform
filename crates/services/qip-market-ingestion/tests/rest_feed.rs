//! The REST adapter, against a real socket.
//!
//! Every test here binds a listener on loopback and lets the adapter connect to
//! it. A mocked client would prove that a decoder was called; it would not
//! prove that a peer which answers with half a body, with more bytes than this
//! process will hold, or with nothing at all, produces a named error instead of
//! a truncated record or a wait with no end — and those are the failures an
//! adapter that talks to a vendor exists to survive.

mod server;

use qip_core::error::Error;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_events::{EventBus, EventLog, Topic};
use qip_financial::intelligence::{DataQualityFailure, ReferenceDataUpdate};
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_market_ingestion::rest::{RestFeedConfig, RestInstrument, RestMarketDataAdapter};
use qip_market_ingestion::{IngestionService, MarketDataAdapter};
use qip_observability::Telemetry;
use qip_transport::ClientLimits;
use server::{Action, TestServer, address_with_no_listener};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration as StdDuration;

/// The credential the fixtures use. A literal so a test can assert it never
/// reaches a URL or an error message.
const API_KEY: &str = "vendor-key-7f3a";

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}

/// Mid-session, so nothing here depends on a session boundary.
fn poll_instant() -> Timestamp {
    at("2026-08-24T15:00:00Z")
}

fn object_id() -> ObjectId {
    ObjectId::from_string("OBJ00000000000000000NWSC")
}

fn instruments() -> Vec<RestInstrument> {
    vec![RestInstrument::new(object_id(), "NWSC", "XNYS")]
}

/// Limits tight enough that a test trips them in bytes and milliseconds.
fn tight() -> ClientLimits {
    ClientLimits {
        max_body: 4096,
        max_headers: 16,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(200),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

/// A fully configured feed pointed at `base`, with no dissemination delay so
/// that knowability is decided by the record's own instants and not by a
/// constant every assertion would have to carry.
fn config(base: &str) -> RestFeedConfig {
    RestFeedConfig {
        name: "vendor-rest".into(),
        provider: "a REST market-data vendor".into(),
        base_url: Some(base.to_string()),
        path: "/v1/market-data".into(),
        api_key: Some(API_KEY.into()),
        api_key_header: "x-api-key".into(),
        licensing: LicensingClass::Licensed,
        publication_delay: Duration::ZERO,
        window: Duration::from_mins(5),
        max_records: 100,
        http: tight(),
    }
}

fn adapter_for(server: &TestServer) -> RestMarketDataAdapter {
    RestMarketDataAdapter::new(config(&server.url()), instruments())
        .expect("a fully specified configuration builds")
}

/// One of each record kind, all knowable by 15:00.
const FULL_PAYLOAD: &str = r#"{
  "bars": [
    {
      "symbol": "NWSC",
      "interval": "1m",
      "open_time": "2026-08-24T14:58:00Z",
      "open": "101.20",
      "high": "101.90",
      "low": "101.05",
      "close": "101.75",
      "volume": "18400",
      "vwap": "101.53",
      "trade_count": 212
    }
  ],
  "quotes": [
    {
      "symbol": "NWSC",
      "at": "2026-08-24T14:59:30Z",
      "bid": "101.74",
      "ask": "101.76",
      "bid_size": "400",
      "ask_size": "300"
    }
  ],
  "trades": [
    {
      "symbol": "NWSC",
      "at": "2026-08-24T14:59:45Z",
      "price": "101.75",
      "size": "100",
      "aggressor": "buy",
      "condition": "regular",
      "trade_id": "print-91"
    }
  ],
  "reference": [
    {
      "symbol": "NWSC",
      "field": "lot_size",
      "previous_value": "100",
      "new_value": "1",
      "effective_from": "2026-09-01T13:30:00Z",
      "announced_at": "2026-08-24T14:50:00Z",
      "update_id": "ref-4412"
    }
  ]
}"#;

// --- a fetch that works -----------------------------------------------------

#[test]
fn a_configured_adapter_fetches_over_a_real_socket_and_decodes_every_record_kind() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut adapter = adapter_for(&server);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");

    assert_eq!(
        server.served(),
        1,
        "the premise of this test is that a request crossed a socket; it did not"
    );
    assert_eq!(records.len(), 4, "one record of each kind: {records:?}");
    assert_eq!(adapter.stats().fetches, 1);
    assert_eq!(adapter.stats().emitted, 4);
    assert_eq!(adapter.stats().withheld, 0);

    let topics: Vec<Topic> = records.iter().map(SensedRecord::topic).collect();
    assert!(topics.contains(&Topic::MarketBar));
    assert!(topics.contains(&Topic::MarketQuote));
    assert!(topics.contains(&Topic::MarketTrade));
    assert!(topics.contains(&Topic::ReferenceDataUpdated));

    let trade = records
        .iter()
        .find_map(|r| match r {
            SensedRecord::Trade(t) => Some(t),
            _ => None,
        })
        .expect("the trade decoded");
    assert_eq!(
        trade.object_id,
        object_id(),
        "the symbol resolved to the configured instrument"
    );
    assert_eq!(
        trade.venue, "XNYS",
        "the venue comes from the instrument map, not the payload"
    );
    assert_eq!(trade.price, dec!("101.75"));
    assert_eq!(trade.size, Decimal::from_int(100));
    assert_eq!(
        trade.trade_id.as_deref(),
        Some("print-91"),
        "the vendor's own id is kept so a reconciliation has something to join on"
    );

    let quote = records
        .iter()
        .find_map(|r| match r {
            SensedRecord::Quote(q) => Some(q),
            _ => None,
        })
        .expect("the quote decoded");
    assert_eq!(quote.bid, dec!("101.74"));
    assert_eq!(quote.ask, dec!("101.76"));
    assert!(
        quote.validate().is_empty(),
        "the decoded quote is publishable"
    );
}

#[test]
fn the_records_come_back_in_event_order_whatever_order_the_vendor_listed_them_in() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut adapter = adapter_for(&server);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");

    let times: Vec<i64> = records.iter().map(|r| r.occurred_at().as_nanos()).collect();
    let mut sorted = times.clone();
    sorted.sort_unstable();
    assert_eq!(
        times, sorted,
        "a consumer that assumes a monotone stream must get one"
    );
}

#[test]
fn the_request_names_the_symbols_and_the_window_the_callers_clock_defines() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut adapter = adapter_for(&server);

    adapter.poll(poll_instant()).expect("the fetch succeeds");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let target = &requests[0].target;
    assert!(
        target.starts_with("/v1/market-data?"),
        "unexpected target {target}"
    );
    assert!(
        target.contains("symbols=NWSC"),
        "the request must ask for what it maps: {target}"
    );
    assert!(
        target.contains("until=2026-08-24T15:00:00"),
        "the window's end is the caller's clock: {target}"
    );
    assert!(
        target.contains("since=2026-08-24T14:55:00"),
        "the window's start is `until` less the configured window: {target}"
    );
    assert_eq!(
        requests[0].method, "GET",
        "a fetch reads and changes nothing"
    );
}

#[test]
fn the_credential_travels_in_a_header_and_never_in_the_url() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut adapter = adapter_for(&server);

    adapter.poll(poll_instant()).expect("the fetch succeeds");

    let requests = server.requests();
    assert_eq!(
        requests[0].headers.get("x-api-key").map(String::as_str),
        Some(API_KEY),
        "the vendor cannot authenticate a request that does not carry the key"
    );
    assert!(
        !requests[0].target.contains(API_KEY),
        "a credential in the URL is a credential in every access log on the path: {}",
        requests[0].target
    );
    assert_eq!(
        requests[0].headers.get("accept").map(String::as_str),
        Some("application/json")
    );
}

// --- point in time ----------------------------------------------------------

#[test]
fn a_bar_is_stamped_with_the_vendors_bucket_rather_than_the_instant_it_was_fetched() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut adapter = adapter_for(&server);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let bar = records
        .iter()
        .find_map(|r| match r {
            SensedRecord::Bar(b) => Some(b),
            _ => None,
        })
        .expect("the bar decoded");

    assert_ne!(
        at("2026-08-24T14:58:00Z"),
        poll_instant(),
        "the premise: the fixture's bucket is not the poll instant, so the two are \
         distinguishable"
    );
    assert_eq!(bar.open_time, at("2026-08-24T14:58:00Z"));
    assert_eq!(
        bar.close_time(),
        at("2026-08-24T14:59:00Z"),
        "the close is derived from the bucket and the interval"
    );
    assert_eq!(
        records
            .iter()
            .find(|r| matches!(r, SensedRecord::Bar(_)))
            .map(SensedRecord::occurred_at),
        Some(at("2026-08-24T14:59:00Z")),
        "a bar is knowable when its bucket closes, and that is what it reports"
    );
}

#[test]
fn a_bar_that_has_not_finished_forming_is_withheld_until_the_callers_clock_reaches_it() {
    // The same response twice; only the caller's clock moves.
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut config = config(&server.url());
    // A delayed entitlement: the vendor publishes fifteen minutes late, so a
    // bar closing at 14:59 is not knowable until 15:14.
    config.publication_delay = Duration::from_mins(15);
    let mut adapter =
        RestMarketDataAdapter::new(config, instruments()).expect("the configuration builds");

    let early = adapter.poll(poll_instant()).expect("the fetch succeeds");
    assert!(
        early.is_empty(),
        "nothing in the response was knowable at 15:00 on a fifteen-minute delay: {early:?}"
    );
    assert_eq!(
        adapter.stats().withheld,
        4,
        "withholding is counted, not silent"
    );

    let later = adapter
        .poll(poll_instant().saturating_add(Duration::from_mins(20)))
        .expect("the second fetch succeeds");
    assert_eq!(
        later.len(),
        4,
        "the next poll's window covers them again, so withholding loses nothing"
    );
    assert_eq!(server.served(), 2, "each poll is its own request");
}

#[test]
fn a_reference_update_records_its_source_its_announcement_and_the_callers_clock() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut adapter = adapter_for(&server);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let update: &ReferenceDataUpdate = records
        .iter()
        .find_map(|r| match r {
            SensedRecord::ReferenceData(u) => Some(u.as_ref()),
            _ => None,
        })
        .expect("the reference update decoded");

    assert_eq!(
        update.provenance.source, "vendor-rest",
        "the adapter names itself as the source"
    );
    assert_eq!(
        update.provenance.event_time,
        at("2026-08-24T14:50:00Z"),
        "the event time is when the vendor announced the change"
    );
    assert_eq!(
        update.provenance.ingestion_time,
        poll_instant(),
        "the ingestion time is the caller's clock; a wall-clock stamp would make a replay of \
         this fetch produce a different record every run"
    );
    assert_eq!(update.provenance.licensing, LicensingClass::Licensed);
    assert_eq!(update.provenance.upstream_id.as_deref(), Some("ref-4412"));
    assert!(
        update.effective_from > poll_instant(),
        "the premise: this change takes effect in the future, so `effective_from` and the \
         instant it became knowable cannot be the same field"
    );
    assert_eq!(update.object_id, object_id().to_string());
    assert_eq!(update.field, "lot_size");
    assert_eq!(update.new_value, "1");
}

// --- what an untrusted peer cannot do ---------------------------------------

#[test]
fn a_body_that_is_not_json_is_refused_with_an_error_that_names_the_source() {
    let server = TestServer::always(Action::json(200, "{\"bars\": [ {\"symbol\""));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a truncated JSON document was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("vendor-rest"),
        "the refusal must name which feed sent it: {error}"
    );
    assert_eq!(
        adapter.stats().emitted,
        0,
        "nothing may be published from a body like this"
    );
}

#[test]
fn a_field_of_the_wrong_type_is_refused_rather_than_coerced() {
    let server = TestServer::always(Action::json(
        200,
        r#"{"quotes":[{"symbol":"NWSC","at":"2026-08-24T14:59:30Z","bid":{"value":"101.74"},
            "ask":"101.76","bid_size":"400","ask_size":"300"}]}"#,
    ));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a bid that is an object was accepted as a price");

    assert_eq!(error.code(), "schema", "got {error:?}");
}

#[test]
fn a_body_larger_than_the_limit_is_refused_before_it_is_buffered() {
    // Well-formed, correctly framed, and larger than this process will hold.
    let server = TestServer::always(Action::Oversized { bytes: 64 * 1024 });
    let mut adapter = adapter_for(&server);
    assert_eq!(
        adapter.config().http.max_body,
        4096,
        "the premise: the fixture exceeds the cap"
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("a 64 kB body was accepted against a 4 kB limit");

    assert_eq!(error.code(), "guard", "got {error:?}");
    assert!(
        error.message().contains("4096"),
        "the refusal must say what the limit was: {error}"
    );
}

#[test]
fn more_records_than_the_cap_are_refused_even_when_the_body_fits() {
    let quotes: Vec<String> = (0..5)
        .map(|i| {
            format!(
                r#"{{"symbol":"NWSC","at":"2026-08-24T14:5{i}:00Z","bid":"101.74","ask":"101.76",
                    "bid_size":"400","ask_size":"300"}}"#
            )
        })
        .collect();
    let body = format!("{{\"quotes\":[{}]}}", quotes.join(","));
    assert!(
        body.len() < 4096,
        "the premise: this body is under the size limit"
    );

    let server = TestServer::always(Action::json(200, body));
    let mut config = config(&server.url());
    config.max_records = 3;
    let mut adapter =
        RestMarketDataAdapter::new(config, instruments()).expect("the configuration builds");

    let error = adapter
        .poll(poll_instant())
        .expect_err("five records were accepted against a cap of three");

    assert_eq!(error.code(), "guard", "got {error:?}");
    assert!(
        error.message().contains('5') && error.message().contains('3'),
        "the refusal must name what arrived and what the cap was: {error}"
    );
}

#[test]
fn a_peer_that_accepts_the_connection_and_says_nothing_is_refused_within_the_timeout() {
    let server = TestServer::always(Action::Silent(StdDuration::from_millis(1_500)));
    let mut adapter = adapter_for(&server);

    let started = std::time::Instant::now();
    let error = adapter
        .poll(poll_instant())
        .expect_err("a silent peer was waited on indefinitely");

    assert_eq!(error.code(), "timeout", "got {error:?}");
    assert!(
        started.elapsed() < StdDuration::from_millis(1_200),
        "the read timeout must end the wait well before the peer speaks; waited {:?}",
        started.elapsed()
    );
}

#[test]
fn a_peer_that_dies_part_way_through_its_own_body_is_a_close_and_not_a_short_record() {
    let server = TestServer::always(Action::Truncated {
        declared: 512,
        written: 20,
    });
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a half-sent body was decoded");

    assert_eq!(error.code(), "io", "got {error:?}");
    assert_eq!(
        adapter.stats().fetches,
        0,
        "a fetch that failed is not counted as one"
    );
}

#[test]
fn an_unreachable_vendor_is_refused_rather_than_waited_on() {
    let mut config = config(&address_with_no_listener());
    config.name = "vendor-rest".into();
    let mut adapter =
        RestMarketDataAdapter::new(config, instruments()).expect("the configuration builds");

    let error = adapter
        .poll(poll_instant())
        .expect_err("a connection to nothing succeeded");

    assert_eq!(error.code(), "io", "got {error:?}");
}

#[test]
fn a_vendor_that_rejects_the_credential_produces_a_denial_that_does_not_quote_it() {
    let server = TestServer::always(Action::json(401, r#"{"error":"invalid api key"}"#));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unauthenticated response was treated as data");

    assert_eq!(error.code(), "denied", "got {error:?}");
    assert!(
        !error.message().contains(API_KEY),
        "an error message is a log line: {error}"
    );
    assert!(
        !format!("{:?}", adapter.config()).contains(API_KEY),
        "the credential must not survive a Debug of the configuration"
    );
}

#[test]
fn a_vendor_outage_and_a_missing_endpoint_are_different_errors() {
    let outage = TestServer::always(Action::json(503, "upstream unavailable"));
    let mut adapter = adapter_for(&outage);
    let error = adapter.poll(poll_instant()).expect_err("a 503 was decoded");
    assert_eq!(error.code(), "unavailable", "got {error:?}");

    let missing = TestServer::always(Action::json(404, "no such endpoint"));
    let mut adapter = adapter_for(&missing);
    let error = adapter.poll(poll_instant()).expect_err("a 404 was decoded");
    assert_eq!(
        error.code(),
        "not_found",
        "a path to fix and a vendor to wait for are different runbook pages: {error:?}"
    );
}

#[test]
fn a_symbol_with_no_instrument_behind_it_is_refused_rather_than_given_an_invented_id() {
    let server = TestServer::always(Action::json(
        200,
        r#"{"trades":[{"symbol":"ZZZZ","at":"2026-08-24T14:59:45Z","price":"10.00",
            "size":"1"}]}"#,
    ));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a record for an unmapped symbol was accepted");

    assert_eq!(error.code(), "not_found", "got {error:?}");
    assert!(
        error.message().contains("ZZZZ") && error.message().contains("NWSC"),
        "the refusal must name what arrived and what is mapped: {error}"
    );
}

#[test]
fn an_interval_this_decoder_cannot_name_is_refused_with_the_ones_it_can() {
    let server = TestServer::always(Action::json(
        200,
        r#"{"bars":[{"symbol":"NWSC","interval":"1y","open_time":"2026-08-24T14:58:00Z",
            "open":"1","high":"2","low":"0.5","close":"1.5","volume":"10"}]}"#,
    ));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unknown interval was guessed at");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("1m") && error.message().contains("1d"),
        "the refusal must say what would have been accepted: {error}"
    );
}

#[test]
fn an_unreadable_trade_condition_is_refused_rather_than_defaulted_to_regular() {
    let server = TestServer::always(Action::json(
        200,
        r#"{"trades":[{"symbol":"NWSC","at":"2026-08-24T14:59:45Z","price":"101.75","size":"100",
            "condition":"xx"}]}"#,
    ));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unknown condition became a price-forming print");

    assert_eq!(error.code(), "schema", "got {error:?}");
}

// --- an adapter with nothing behind it --------------------------------------

#[test]
fn an_unconfigured_adapter_names_every_missing_piece_and_returns_no_data() {
    let mut adapter = RestMarketDataAdapter::new(RestFeedConfig::default(), Vec::new())
        .expect("an adapter that cannot fetch still has to exist in order to say so");

    assert!(!adapter.is_available());
    let missing = adapter.missing_configuration();
    assert_eq!(
        missing.len(),
        3,
        "three separate things are missing: {missing:?}"
    );
    let joined = missing.join(" | ");
    assert!(joined.contains("no endpoint"), "{joined}");
    assert!(joined.contains("no credential"), "{joined}");
    assert!(joined.contains("no instruments"), "{joined}");

    let requirement = adapter
        .descriptor()
        .production_requirement
        .expect("the descriptor must carry the requirement");
    assert!(requirement.contains("base_url"), "{requirement}");
    assert!(requirement.contains("api_key"), "{requirement}");
    assert!(requirement.contains("ObjectId"), "{requirement}");

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unconfigured adapter produced records");
    assert_eq!(error.code(), "unavailable", "got {error:?}");
    assert!(
        error
            .message()
            .contains("will not substitute generated data"),
        "the refusal must say that no data is deliberate: {error}"
    );

    let error = adapter
        .start(poll_instant())
        .expect_err("an unconfigured adapter started");
    assert_eq!(
        error.code(),
        "unavailable",
        "a missing credential should fail the rollout, not the first poll an hour later"
    );
}

#[test]
fn an_endpoint_without_a_credential_is_still_unavailable() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut config = config(&server.url());
    config.api_key = None;
    let mut adapter =
        RestMarketDataAdapter::new(config, instruments()).expect("the configuration builds");

    assert!(!adapter.is_available());
    let missing = adapter.missing_configuration();
    assert_eq!(
        missing.len(),
        1,
        "only the credential is missing: {missing:?}"
    );
    assert!(missing[0].contains("no credential"));

    let error = adapter
        .poll(poll_instant())
        .expect_err("a keyless fetch was attempted");
    assert_eq!(error.code(), "unavailable", "got {error:?}");
    assert_eq!(
        server.served(),
        0,
        "an adapter with no credential must not open a connection at all"
    );
}

#[test]
fn a_configured_adapter_still_states_what_production_has_to_add() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let adapter = adapter_for(&server);
    assert!(adapter.is_available());

    let descriptor = adapter.descriptor();
    let requirement = descriptor
        .production_requirement
        .clone()
        .expect("a working configuration is not by itself a production feed");
    assert!(
        requirement.contains("TLS"),
        "this client speaks plaintext; a deployment has to put TLS in front of it: {requirement}"
    );
    assert!(requirement.contains("licence"), "{requirement}");
    assert!(requirement.contains("publication_delay"), "{requirement}");

    assert_eq!(descriptor.name, "vendor-rest");
    assert_eq!(descriptor.licensing, LicensingClass::Licensed);
    assert!(
        descriptor.is_production_grade(),
        "a licensed feed may drive a decision"
    );
    assert_eq!(
        descriptor.expected_latency,
        Duration::ZERO,
        "the declared latency is the configured dissemination delay"
    );
    assert_eq!(
        descriptor.topics,
        vec![
            Topic::MarketBar,
            Topic::MarketQuote,
            Topic::MarketTrade,
            Topic::ReferenceDataUpdated
        ]
    );
    assert_eq!(adapter.instruments(), vec![object_id()]);
}

// --- configuration that is present and wrong --------------------------------

#[test]
fn an_https_endpoint_is_refused_at_configuration_time_rather_than_downgraded() {
    let mut config = config("http://placeholder.internal");
    config.base_url = Some("https://vendor.example.com".into());

    let error = RestMarketDataAdapter::new(config, instruments())
        .expect_err("an https endpoint this build cannot speak was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(
        error.message().contains("TLS"),
        "the refusal must say why, so nobody concludes the URL was malformed: {error}"
    );
}

#[test]
fn a_symbol_that_would_split_the_request_line_is_refused_when_it_is_configured() {
    let error = RestMarketDataAdapter::new(
        config("http://vendor.internal"),
        vec![RestInstrument::new(object_id(), "NW SC", "XNYS")],
    )
    .expect_err("a symbol with a space was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(error.message().contains("request line"), "{error}");
}

#[test]
fn two_instruments_claiming_one_vendor_symbol_are_refused() {
    let error = RestMarketDataAdapter::new(
        config("http://vendor.internal"),
        vec![
            RestInstrument::new(object_id(), "NWSC", "XNYS"),
            RestInstrument::new(
                ObjectId::from_string("OBJ00000000000000000VNTG"),
                "NWSC",
                "XNAS",
            ),
        ],
    )
    .expect_err("an ambiguous symbol map was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(error.message().contains("NWSC"), "{error}");
}

#[test]
fn a_credential_carrying_a_newline_is_refused_before_it_can_forge_a_header() {
    let mut config = config("http://vendor.internal");
    config.api_key = Some("key\r\nx-admin: true".into());

    let error = RestMarketDataAdapter::new(config, instruments())
        .expect_err("a credential containing CRLF was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
}

// --- beside the synthetic sources -------------------------------------------

#[test]
fn records_from_a_live_fetch_publish_through_the_ingestion_service() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let now = poll_instant();
    let (context, _clock) = Context::deterministic(now, 1);
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(log.clone());
    let mut service = IngestionService::new(Telemetry::silent());

    service.register(Box::new(adapter_for(&server)));
    service.start(now).expect("a configured adapter starts");
    let published = service
        .poll_and_publish(&context, &mut bus, now)
        .expect("the poll succeeds");
    bus.drain(&context).expect("the bus drains");

    assert_eq!(
        published, 4,
        "every decoded record passed validation and published"
    );
    let log = log.borrow();
    assert_eq!(log.by_topic(Topic::MarketBar).len(), 1);
    assert_eq!(log.by_topic(Topic::MarketQuote).len(), 1);
    assert_eq!(log.by_topic(Topic::MarketTrade).len(), 1);
    assert_eq!(log.by_topic(Topic::ReferenceDataUpdated).len(), 1);
    for event in log.events() {
        assert_eq!(event.lineage.producer, "market-ingestion");
        assert!(
            event.lineage.causation_id.is_none(),
            "an observation is a root, not caused by anything"
        );
    }

    let sources = service.sources();
    assert_eq!(sources.len(), 1);
    assert!(
        service.non_production_sources().is_empty(),
        "a licensed feed is not a stand-in"
    );
}

#[test]
fn a_bar_the_vendor_got_wrong_reaches_the_validation_gate_instead_of_being_dropped_here() {
    // High below low. The adapter deliberately does not re-validate: a record
    // dropped inside an adapter is a data-quality failure nobody can see.
    let server = TestServer::always(Action::json(
        200,
        r#"{"bars":[{"symbol":"NWSC","interval":"1m","open_time":"2026-08-24T14:58:00Z",
            "open":"101.20","high":"100.00","low":"101.05","close":"101.75","volume":"10"}]}"#,
    ));
    let mut adapter = adapter_for(&server);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    assert_eq!(records.len(), 1, "the adapter hands the bad bar on");
    assert!(
        !records[0].validate().is_empty(),
        "the premise: this bar does not survive validation"
    );

    let now = poll_instant();
    let (context, _clock) = Context::deterministic(now, 1);
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(log.clone());
    let mut service = IngestionService::new(Telemetry::silent());
    let published = service
        .publish_records(&context, &mut bus, "vendor-rest", &records)
        .expect("publication succeeds");
    bus.drain(&context).expect("the bus drains");

    assert_eq!(published, 0, "the incoherent bar must not publish as a bar");
    let log = log.borrow();
    assert_eq!(log.by_topic(Topic::MarketBar).len(), 0);
    let failures = log.by_topic(Topic::DataQualityFailed);
    assert_eq!(
        failures.len(),
        1,
        "the vendor's error must be visible as an event"
    );
    let failure: DataQualityFailure = failures[0]
        .decode::<DataQualityFailure>()
        .expect("the failure decodes")
        .body;
    assert_eq!(failure.source, "vendor-rest");
    assert!(failure.rejected);
}

#[test]
fn the_fetch_helper_returns_what_the_vendor_sent_without_the_knowable_gate() {
    // The connection and the credential are what a deployment gets wrong, and
    // checking them should not depend on where the clock is.
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut config = config(&server.url());
    config.publication_delay = Duration::from_hours(24);
    let mut adapter =
        RestMarketDataAdapter::new(config, instruments()).expect("the configuration builds");

    assert!(
        adapter
            .poll(poll_instant())
            .expect("the poll succeeds")
            .is_empty(),
        "the premise: on a one-day delay nothing here is knowable yet"
    );
    let fetched = adapter.fetch(poll_instant()).expect("the fetch succeeds");
    assert_eq!(
        fetched.len(),
        4,
        "the helper reports what the vendor actually sent"
    );
}

#[test]
fn an_error_from_a_refused_response_is_a_platform_error_and_not_a_transport_one() {
    // The transport's vocabulary stops at the adapter boundary: everything
    // downstream matches on `qip_core::Error`.
    let server = TestServer::always(Action::Oversized { bytes: 64 * 1024 });
    let mut adapter = adapter_for(&server);
    let error: Error = adapter
        .poll(poll_instant())
        .expect_err("the body was accepted");
    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
}

#[test]
fn a_credential_header_the_transport_writes_itself_is_refused_at_configuration_time() {
    // `HttpRequest` drops a caller's copy of its framing headers, which is
    // correct for framing and would silently strip the credential here.
    let mut config = config("http://vendor.internal");
    config.api_key_header = "Connection".into();

    let error = RestMarketDataAdapter::new(config, instruments())
        .expect_err("a credential in a header the transport owns was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(
        error.message().contains("without a credential"),
        "the refusal must say what would have happened: {error}"
    );
}
