//! The order-book depth adapter, against a real socket.
//!
//! Every test here binds a listener on loopback and lets the adapter connect to
//! it, for the reason `rest_feed.rs` gives. This suite adds a second reason: the
//! adapter under test is a *state machine*, and the interesting properties are
//! about what it does across two polls rather than what it decodes in one. A
//! mocked client cannot express "the vendor answered the first snapshot request
//! this way and the second one that way", and that sequence is the whole of the
//! recovery behaviour.
//!
//! The scripted routes are read as: the snapshot endpoint answers with these
//! bodies in order, the update endpoint with those, and the last answer of each
//! script repeats. A test that lists two snapshot bodies is a test that expects
//! a rebuild.

mod server;

use qip_core::error::Error;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_events::{EventBus, EventLog, Topic};
use qip_financial::quality::LicensingClass;
use qip_market::book::OrderBook;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_market_ingestion::depth::{DepthFeedAdapter, DepthFeedConfig, DepthInstrument};
use qip_market_ingestion::{IngestionService, MarketDataAdapter};
use qip_observability::Telemetry;
use qip_orderbook::snapshot::BookKind;
use qip_orderbook::view::{BookCondition, BookView};
use qip_sequencing::ReorderPolicy;
use qip_transport::ClientLimits;
use server::{Action, TestServer, address_with_no_listener};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration as StdDuration;

/// The credential the fixtures use. A literal so a test can assert it never
/// reaches a URL or an error message.
const API_KEY: &str = "depth-key-91c2";

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}

/// Mid-session, so nothing here depends on a session boundary.
fn poll_instant() -> Timestamp {
    at("2026-08-24T15:00:00Z")
}

fn object_id() -> ObjectId {
    ObjectId::from_string("obj-nwsc")
}

fn instrument() -> DepthInstrument {
    DepthInstrument::new(object_id(), "NWSC", "XNAS", "depth-a")
}

/// Short enough that a test asserting a timeout does not sit on it.
fn tight() -> ClientLimits {
    ClientLimits {
        max_body: 64 * 1024,
        max_headers: 32,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(500),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

/// A configuration whose clock behaviour is out of the way: no publication
/// delay, so a book the vendor stamped before the poll instant is immediately
/// knowable, and a zero gap deadline, so a hole that this poll's own response
/// did not fill is abandoned within it. The tests that care about either of
/// those set them explicitly.
fn config(base: &str) -> DepthFeedConfig {
    DepthFeedConfig {
        name: "test-depth".into(),
        provider: "a loopback depth vendor".into(),
        base_url: Some(base.to_string()),
        api_key: Some(API_KEY.into()),
        licensing: LicensingClass::Licensed,
        publication_delay: Duration::ZERO,
        reorder: ReorderPolicy::new(64, Duration::ZERO),
        http: tight(),
        ..DepthFeedConfig::default()
    }
}

fn adapter_for(server: &TestServer) -> DepthFeedAdapter {
    DepthFeedAdapter::new(config(&server.url()), vec![instrument()])
        .expect("the fixture configuration is valid")
}

/// A vendor answering the snapshot endpoint with `snapshots` in order and the
/// update endpoint with `updates` in order, each repeating its last answer.
fn vendor(snapshots: Vec<&str>, updates: Vec<&str>) -> TestServer {
    TestServer::routed(vec![
        (
            "snapshot",
            snapshots
                .into_iter()
                .map(|body| Action::json(200, body))
                .collect(),
        ),
        (
            "updates",
            updates
                .into_iter()
                .map(|body| Action::json(200, body))
                .collect(),
        ),
    ])
}

fn book_of(records: &[SensedRecord]) -> &OrderBook {
    records
        .iter()
        .find_map(|record| match record {
            SensedRecord::Book(book) => Some(book.as_ref()),
            _ => None,
        })
        .expect("the records contain a book")
}

/// A two-sided book, open, complete as of sequence 1000.
const SNAPSHOT: &str = r#"{
  "symbol": "NWSC",
  "sequence": 1000,
  "at": "2026-08-24T14:59:50Z",
  "status": "open",
  "bids": [
    {"price": "101.00", "size": "500", "orders": 3},
    {"price": "100.99", "size": "300", "orders": 2}
  ],
  "asks": [
    {"price": "101.02", "size": "400", "orders": 4},
    {"price": "101.03", "size": "600", "orders": 1}
  ]
}"#;

/// A new best bid inside the spread, and the old best offer deleted.
const UPDATES: &str = r#"{"updates": [
  {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "level_set",
   "side": "bid", "price": "101.01", "size": "250", "orders": 1},
  {"sequence": 1002, "at": "2026-08-24T14:59:56Z", "type": "level_set",
   "side": "ask", "price": "101.02", "size": "0"}
]}"#;

const NO_UPDATES: &str = r#"{"updates": []}"#;

// --- the book itself --------------------------------------------------------

#[test]
fn a_snapshot_and_the_increments_after_it_build_the_book_the_venue_published() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = adapter_for(&server);

    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let book = book_of(&records);

    // The snapshot's two bids plus the one the increment added, dearest first.
    assert_eq!(
        book.bids
            .iter()
            .map(|level| (level.price, level.size))
            .collect::<Vec<_>>(),
        vec![
            (dec!("101.01"), dec!("250")),
            (dec!("101.00"), dec!("500")),
            (dec!("100.99"), dec!("300")),
        ],
        "the increment's new level should sit at the touch, in front of the snapshot's"
    );
    // The increment set 101.02 to zero, which removes it rather than resting a
    // level of no size.
    assert_eq!(
        book.asks
            .iter()
            .map(|level| (level.price, level.size))
            .collect::<Vec<_>>(),
        vec![(dec!("101.03"), dec!("600"))],
        "a level set to zero size should be gone, not present and empty"
    );
    assert_eq!(
        book.sequence, 1002,
        "the book should carry the venue position of the last message applied"
    );
    assert_eq!(
        book.at,
        at("2026-08-24T14:59:56Z"),
        "the book's event time should be the venue time of the last message applied, not the \
         poll instant"
    );
    assert_eq!(book.venue, "XNAS");
    assert_eq!(book.object_id, object_id());

    let stats = adapter.stats();
    assert_eq!(stats.snapshots, 1, "one snapshot to start the book");
    assert_eq!(stats.resynchronisations, 0, "nothing had to be rebuilt");
    assert_eq!(stats.emitted, 1);
}

#[test]
fn the_first_poll_asks_for_a_snapshot_before_it_asks_for_the_increments_to_apply_to_it() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = adapter_for(&server);
    adapter.poll(poll_instant()).expect("the poll succeeds");

    let targets: Vec<String> = server
        .requests()
        .iter()
        .map(|request| request.target.clone())
        .collect();
    assert_eq!(
        targets.len(),
        2,
        "one poll of one instrument is one snapshot request and one update request, got: \
         {targets:?}"
    );
    assert!(
        targets[0].contains("/v1/depth/snapshot"),
        "the snapshot has to come first — there is nothing to apply increments to otherwise: \
         {targets:?}"
    );
    assert!(targets[1].contains("/v1/depth/updates"), "{targets:?}");
}

#[test]
fn an_increment_resumes_from_the_sequence_already_applied_rather_than_from_an_instant() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES, NO_UPDATES]);
    let mut adapter = adapter_for(&server);
    adapter
        .poll(poll_instant())
        .expect("the first poll succeeds");
    adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the second poll succeeds");

    let targets: Vec<String> = server
        .requests()
        .iter()
        .map(|request| request.target.clone())
        .filter(|target| target.contains("/updates"))
        .collect();
    assert!(
        targets[0].contains("after_sequence=1000"),
        "the first update request should resume from the snapshot's own sequence: {targets:?}"
    );
    assert!(
        targets[1].contains("after_sequence=1002"),
        "the second should resume from the last sequence applied, so nothing is re-applied and \
         nothing is skipped: {targets:?}"
    );
    assert!(
        targets[1].contains("until=2026-08-24T15:00:05"),
        "the caller's clock bounds the request, so a vendor cannot hand a backtest the future: \
         {targets:?}"
    );
}

#[test]
fn the_published_book_is_the_book_qip_orderbook_holds_and_not_a_second_copy_of_the_levels() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = adapter_for(&server);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let book = book_of(&records);

    let state = adapter
        .venue_state("NWSC")
        .expect("the instrument has venue state");
    assert_eq!(
        state.best_bid().map(|level| level.price),
        book.best_bid().map(|level| level.price),
        "the record's touch has to be the book's touch; two copies of the levels is how they \
         drift apart"
    );
    assert_eq!(state.condition(), BookCondition::Normal);
    assert!(
        state.continuous_trading(),
        "the venue said open and nothing has been lost, so the book is tradeable"
    );
    assert_eq!(state.kind(), BookKind::Aggregated);
}

#[test]
fn a_poll_that_finds_no_new_increment_republishes_the_book_rather_than_dropping_it() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES, NO_UPDATES]);
    let mut adapter = adapter_for(&server);
    let first = adapter
        .poll(poll_instant())
        .expect("the first poll succeeds");
    let second = adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the second poll succeeds");

    assert_eq!(book_of(&first).sequence, 1002);
    assert_eq!(
        book_of(&second).sequence,
        1002,
        "an unchanged book is published again carrying the same venue sequence: the bus \
         deduplicates on it, and a book lost to a dropped poll must have another chance"
    );
    assert_eq!(adapter.stats().snapshots, 1, "nothing needed rebuilding");
}

// --- sequence gaps ----------------------------------------------------------

/// Sequences 1003 and 1004 never arrive; 1005 does. Its level sits inside the
/// spread rather than through it, so these fixtures exercise a hole and nothing
/// else.
const UPDATES_WITH_A_HOLE: &str = r#"{"updates": [
  {"sequence": 1005, "at": "2026-08-24T15:00:02Z", "type": "level_set",
   "side": "bid", "price": "101.02", "size": "900"}
]}"#;

/// Obviously not the first snapshot: every price is different, so a test can
/// tell a rebuilt book from a book that was quietly carried forward.
const REBUILT_SNAPSHOT: &str = r#"{
  "symbol": "NWSC",
  "sequence": 1010,
  "at": "2026-08-24T15:00:03Z",
  "status": "open",
  "bids": [{"price": "200.00", "size": "700", "orders": 5}],
  "asks": [{"price": "200.05", "size": "800", "orders": 6}]
}"#;

#[test]
fn a_sequence_gap_that_will_not_close_forces_a_re_snapshot_rather_than_a_silent_apply() {
    let server = vendor(
        vec![SNAPSHOT, REBUILT_SNAPSHOT],
        vec![UPDATES, UPDATES_WITH_A_HOLE],
    );
    let mut adapter = adapter_for(&server);
    adapter
        .poll(poll_instant())
        .expect("the first poll succeeds");

    let records = adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the second poll succeeds");
    let book = book_of(&records);

    let stats = adapter.stats();
    assert_eq!(
        stats.gaps_abandoned, 1,
        "1003 and 1004 never arrived, so the hole had to be given up on"
    );
    assert_eq!(
        stats.resynchronisations, 1,
        "giving up on a hole has to force a rebuild"
    );
    assert_eq!(stats.snapshots, 2, "the rebuild is a second snapshot");

    assert_eq!(
        book.sequence, 1010,
        "the published book should be the rebuilt one"
    );
    assert_eq!(
        book.bids
            .iter()
            .map(|level| level.price)
            .collect::<Vec<_>>(),
        vec![dec!("200.00")],
        "the rebuilt book should hold the fresh snapshot's levels"
    );
    assert!(
        !book
            .bids
            .iter()
            .any(|level| level.price == dec!("101.02") || level.price == dec!("101.01")),
        "neither the update after the hole nor the levels from before it may survive into the \
         rebuilt book: applying 1005 on top of a book missing 1003 and 1004 is exactly the \
         silent apply this adapter exists to refuse"
    );

    let targets: Vec<String> = server
        .requests()
        .iter()
        .map(|request| request.target.clone())
        .collect();
    assert_eq!(
        targets
            .iter()
            .filter(|target| target.contains("/snapshot"))
            .count(),
        2,
        "the recovery has to be a real request to the vendor, not a book invented locally: \
         {targets:?}"
    );
}

#[test]
fn a_book_with_a_hole_still_open_behind_it_is_withheld_rather_than_published() {
    // A deadline long enough that the hole is still open when the poll ends:
    // the messages behind it may yet arrive, so this is the "wait" case rather
    // than the "give up" case.
    let server = vendor(vec![SNAPSHOT], vec![UPDATES, UPDATES_WITH_A_HOLE]);
    let mut adapter = DepthFeedAdapter::new(
        DepthFeedConfig {
            reorder: ReorderPolicy::new(64, Duration::from_secs(30)),
            ..config(&server.url())
        },
        vec![instrument()],
    )
    .expect("the fixture configuration is valid");

    let first = adapter
        .poll(poll_instant())
        .expect("the first poll succeeds");
    assert_eq!(book_of(&first).sequence, 1002);

    let second = adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the second poll succeeds");
    assert!(
        second.is_empty(),
        "a book with a known hole behind it must not be published: it is a correct prefix of \
         the stream wearing a timestamp that reads as the market now, got {second:?}"
    );

    let stats = adapter.stats();
    assert_eq!(stats.gaps_opened, 1);
    assert_eq!(
        stats.gaps_abandoned, 0,
        "the deadline has not passed, so the hole may still fill"
    );
    assert_eq!(stats.withheld_gapped, 1);
    assert_eq!(
        stats.snapshots, 1,
        "an open hole is not a diverged book, so nothing is rebuilt yet"
    );
    assert_eq!(
        adapter
            .venue_state("NWSC")
            .and_then(|state| state.last_sequence()),
        Some(1002),
        "1005 must be held rather than applied: applying it would leave the book missing 1003 \
         and 1004 with nothing recording that"
    );
}

#[test]
fn a_hole_that_fills_before_its_deadline_releases_what_was_held_and_publishes_again() {
    const FILLED: &str = r#"{"updates": [
      {"sequence": 1003, "at": "2026-08-24T15:00:03Z", "type": "level_set",
       "side": "bid", "price": "100.98", "size": "111"},
      {"sequence": 1004, "at": "2026-08-24T15:00:04Z", "type": "level_set",
       "side": "ask", "price": "101.04", "size": "222"}
    ]}"#;
    let server = vendor(
        vec![SNAPSHOT],
        vec![UPDATES, UPDATES_WITH_A_HOLE, FILLED, NO_UPDATES],
    );
    let mut adapter = DepthFeedAdapter::new(
        DepthFeedConfig {
            reorder: ReorderPolicy::new(64, Duration::from_secs(30)),
            ..config(&server.url())
        },
        vec![instrument()],
    )
    .expect("the fixture configuration is valid");

    adapter.poll(poll_instant()).expect("the first poll");
    adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the second");
    let third = adapter
        .poll(at("2026-08-24T15:00:06Z"))
        .expect("the third poll succeeds");

    let stats = adapter.stats();
    assert_eq!(stats.gaps_filled, 1, "1003 and 1004 arrived after 1005 did");
    assert_eq!(
        stats.gaps_abandoned, 0,
        "nothing was lost, so nothing should have been rebuilt"
    );
    assert_eq!(stats.snapshots, 1);

    let book = book_of(&third);
    assert_eq!(
        book.sequence, 1005,
        "the held message is applied once its predecessors are, and the book resumes at it"
    );
    assert_eq!(
        book.bids
            .iter()
            .map(|level| (level.price, level.size))
            .collect::<Vec<_>>(),
        vec![
            (dec!("101.02"), dec!("900")),
            (dec!("101.01"), dec!("250")),
            (dec!("101.00"), dec!("500")),
            (dec!("100.99"), dec!("300")),
            (dec!("100.98"), dec!("111")),
        ],
        "every level from both sides of the hole should be present exactly once"
    );
    assert_eq!(
        book.asks
            .iter()
            .map(|level| (level.price, level.size))
            .collect::<Vec<_>>(),
        vec![(dec!("101.03"), dec!("600")), (dec!("101.04"), dec!("222"))],
    );
}

#[test]
fn an_increment_already_applied_is_dropped_rather_than_applied_a_second_time() {
    // The window overlaps on purpose, so a vendor re-sending what this cell
    // already has is the normal case rather than a fault.
    const OVERLAPPING: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "level_set",
       "side": "bid", "price": "101.01", "size": "250", "orders": 1},
      {"sequence": 1002, "at": "2026-08-24T14:59:56Z", "type": "level_set",
       "side": "ask", "price": "101.02", "size": "0"},
      {"sequence": 1003, "at": "2026-08-24T15:00:01Z", "type": "level_set",
       "side": "bid", "price": "101.00", "size": "50"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT], vec![UPDATES, OVERLAPPING]);
    let mut adapter = adapter_for(&server);
    adapter.poll(poll_instant()).expect("the first poll");
    let records = adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the second poll succeeds");

    assert_eq!(
        adapter.stats().duplicates,
        2,
        "1001 and 1002 had already been applied and must be recognised, not re-applied"
    );
    let book = book_of(&records);
    assert_eq!(book.sequence, 1003);
    assert_eq!(
        book.bids
            .iter()
            .find(|level| level.price == dec!("101.00"))
            .map(|level| level.size),
        Some(dec!("50")),
        "the only new message should have been applied exactly once"
    );
}

#[test]
fn a_vendor_asking_for_a_resubscribe_rebuilds_the_book_rather_than_carrying_on() {
    const RESET: &str = r#"{"updates": [
      {"sequence": 1003, "at": "2026-08-24T15:00:01Z", "type": "reset",
       "reason": "the venue restarted its feed"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT, REBUILT_SNAPSHOT], vec![UPDATES, RESET]);
    let mut adapter = adapter_for(&server);
    adapter.poll(poll_instant()).expect("the first poll");
    let records = adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the second poll succeeds");

    assert_eq!(
        adapter.stats().resynchronisations,
        1,
        "a reset the vendor sent means the same thing as a reset a gap produced"
    );
    assert_eq!(book_of(&records).sequence, 1010);
}

// --- crossed and locked books ----------------------------------------------

/// The bid jumps above the best offer while the venue says it is open.
const UPDATES_THAT_CROSS: &str = r#"{"updates": [
  {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "level_set",
   "side": "bid", "price": "101.10", "size": "100"}
]}"#;

#[test]
fn a_book_that_crosses_while_the_venue_says_it_is_trading_is_rebuilt_rather_than_normalised() {
    let server = vendor(
        vec![SNAPSHOT, REBUILT_SNAPSHOT],
        vec![UPDATES_THAT_CROSS, NO_UPDATES],
    );
    let mut adapter = adapter_for(&server);
    // Past the rebuilt snapshot's own instant, so the recovered book is
    // knowable and this test is about the recovery rather than about the clock.
    let records = adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the poll succeeds");

    let stats = adapter.stats();
    assert_eq!(
        stats.crossed_in_continuous, 1,
        "a bid through the offer at one venue in continuous session is corruption, and this \
         adapter holds one venue's book"
    );
    assert_eq!(stats.resynchronisations, 1);
    assert_eq!(stats.snapshots, 2);

    let book = book_of(&records);
    assert_eq!(book.sequence, 1010, "the rebuilt book is what is published");
    assert!(
        book.validate().is_ok(),
        "the rebuilt book should be structurally sound: {:?}",
        book.validate()
    );
    assert!(
        !book.bids.iter().any(|level| level.price == dec!("101.10")),
        "the crossing level must not survive the rebuild"
    );
    assert!(
        !book.asks.iter().any(|level| level.price == dec!("101.02")),
        "nor may the level it crossed be quietly deleted to make the old book look sound — the \
         whole book is thrown away and re-fetched"
    );
}

#[test]
fn a_vendor_that_keeps_sending_a_crossed_book_is_refused_and_its_levels_are_left_as_it_sent_them() {
    const CROSSED_SNAPSHOT: &str = r#"{
      "symbol": "NWSC",
      "sequence": 1010,
      "at": "2026-08-24T15:00:03Z",
      "status": "open",
      "bids": [{"price": "101.50", "size": "700"}],
      "asks": [{"price": "101.20", "size": "800"}]
    }"#;
    let server = vendor(
        vec![SNAPSHOT, CROSSED_SNAPSHOT],
        vec![UPDATES_THAT_CROSS, NO_UPDATES],
    );
    let mut adapter = adapter_for(&server);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        records.is_empty(),
        "a crossed book must not be published: the mid computed from an inverted touch is a \
         plausible number no strategy should size against, got {records:?}"
    );
    let stats = adapter.stats();
    assert_eq!(stats.withheld_crossed, 1);
    assert_eq!(
        stats.snapshots, 2,
        "one recovery attempt, not a loop: a vendor answering a rebuild with another broken \
         book is something to alert on"
    );

    let state = adapter
        .venue_state("NWSC")
        .expect("the instrument has venue state");
    assert_eq!(state.condition(), BookCondition::Crossed);
    assert_eq!(
        state.book().best_bid().map(|level| level.price),
        Some(dec!("101.50")),
        "the vendor's own levels should still be there, untouched, for an operator to look at"
    );
    assert_eq!(
        state.book().best_ask().map(|level| level.price),
        Some(dec!("101.20")),
        "neither side is moved to meet the other; nothing here invents a level"
    );
    assert_eq!(
        state.mid(),
        None,
        "and no derived price is served off it either"
    );
}

#[test]
fn a_crossed_book_during_an_auction_is_withheld_as_the_auction_state_it_is_and_not_rebuilt() {
    // `qip_orderbook::auction` models an auction as running beside the
    // continuous book: orders accumulate and cross, and the venue publishes the
    // indicative price separately. A cross here is expected, not corruption.
    const AUCTION_SNAPSHOT: &str = r#"{
      "symbol": "NWSC",
      "sequence": 1000,
      "at": "2026-08-24T14:59:50Z",
      "status": "auction",
      "bids": [{"price": "101.50", "size": "5000"}],
      "asks": [{"price": "101.20", "size": "4200"}]
    }"#;
    const AUCTION_UPDATES: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "auction",
       "indicative_price": "101.35", "paired": "4200", "imbalance": "800",
       "imbalance_side": "bid"}
    ]}"#;
    let server = vendor(vec![AUCTION_SNAPSHOT], vec![AUCTION_UPDATES]);
    let mut adapter = adapter_for(&server);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        records.is_empty(),
        "the continuous touch during an auction is a book nobody can hit, so it is withheld \
         whatever its condition, got {records:?}"
    );
    let stats = adapter.stats();
    assert_eq!(stats.withheld_auction, 1);
    assert_eq!(
        stats.crossed_in_continuous, 0,
        "a cross during an auction is not corruption and must not be counted as it"
    );
    assert_eq!(
        stats.resynchronisations, 0,
        "and nothing is wrong with the book, so nothing is rebuilt"
    );
    assert_eq!(stats.snapshots, 1);

    let state = adapter
        .venue_state("NWSC")
        .expect("the instrument has venue state");
    assert!(
        !state.continuous_trading(),
        "the venue is in an auction, so the continuous book is not tradeable"
    );
    assert!(
        !state.is_stale(),
        "withholding an auction book is not the same as distrusting it"
    );
    let auction = state.auction().expect("the auction state was recorded");
    assert_eq!(
        auction.indicative_price,
        Some(dec!("101.35")),
        "the auction's own price is kept beside the book rather than folded into the levels"
    );
    assert_eq!(auction.paired, dec!("4200"));
    assert_eq!(auction.signed_imbalance(), dec!("800"));
    assert_eq!(
        state.condition(),
        BookCondition::Crossed,
        "the levels are still reported as they are; nothing was normalised to hide the cross"
    );
}

#[test]
fn a_locked_book_is_published_because_qip_orderbook_calls_a_locked_touch_consistent() {
    const LOCKING: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "level_set",
       "side": "bid", "price": "101.02", "size": "150"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT], vec![LOCKING]);
    let mut adapter = adapter_for(&server);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");

    let book = book_of(&records);
    assert_eq!(
        book.best_bid().map(|level| level.price),
        Some(dec!("101.02"))
    );
    assert_eq!(
        book.best_ask().map(|level| level.price),
        Some(dec!("101.02"))
    );
    assert_eq!(
        book.spread(),
        Some(Decimal::ZERO),
        "a locked book is legal on several venues and common at the open; it earns no spread \
         but nothing about it is inconsistent"
    );
    assert_eq!(
        adapter.condition("NWSC"),
        Some(BookCondition::Locked),
        "and it is reported as locked rather than as crossed"
    );
    assert_eq!(adapter.stats().crossed_in_continuous, 0);
    assert_eq!(adapter.stats().snapshots, 1, "nothing needed rebuilding");
}

// --- venue status -----------------------------------------------------------

#[test]
fn a_snapshot_that_states_no_venue_status_is_refused_rather_than_assumed_open() {
    const NO_STATUS: &str = r#"{
      "symbol": "NWSC", "sequence": 1000, "at": "2026-08-24T14:59:50Z",
      "bids": [{"price": "101.00", "size": "500"}],
      "asks": [{"price": "101.02", "size": "400"}]
    }"#;
    let server = vendor(vec![NO_STATUS], vec![NO_UPDATES]);
    let mut adapter = adapter_for(&server);
    let error = adapter
        .poll(poll_instant())
        .expect_err("a snapshot with no venue status is refused");

    let text = error.to_string();
    assert!(
        text.contains("states no venue status"),
        "the refusal should name what is missing: {text}"
    );
    assert!(
        text.contains("crossed"),
        "and why it matters — the status is what tells an auction from corruption: {text}"
    );
    assert!(
        adapter.awaiting_snapshot("NWSC").unwrap_or(false),
        "and the book should still be waiting to be built, not left half-made"
    );
}

#[test]
fn a_book_at_a_venue_that_is_not_trading_continuously_is_withheld() {
    const HALTED: &str = r#"{
      "symbol": "NWSC", "sequence": 1000, "at": "2026-08-24T14:59:50Z",
      "status": "halted",
      "bids": [{"price": "101.00", "size": "500"}],
      "asks": [{"price": "101.02", "size": "400"}]
    }"#;
    let server = vendor(vec![HALTED], vec![NO_UPDATES]);
    let mut adapter = adapter_for(&server);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        records.is_empty(),
        "a halted venue's resting levels are not a market to price off, got {records:?}"
    );
    assert_eq!(adapter.stats().withheld_not_trading, 1);
    assert_eq!(
        adapter.stats().snapshots,
        1,
        "a halt is not a reason to rebuild"
    );
}

#[test]
fn a_venue_status_this_decoder_cannot_name_is_refused_with_the_ones_it_can() {
    const ODD: &str = r#"{
      "symbol": "NWSC", "sequence": 1000, "at": "2026-08-24T14:59:50Z",
      "status": "pre_cross_indicative",
      "bids": [{"price": "101.00", "size": "500"}], "asks": []
    }"#;
    let server = vendor(vec![ODD], vec![NO_UPDATES]);
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("an unknown status is refused")
        .to_string();
    assert!(text.contains("pre_cross_indicative"), "{text}");
    assert!(
        text.contains("open") && text.contains("auction") && text.contains("halted"),
        "the refusal should list what this decoder does accept: {text}"
    );
}

// --- resolution -------------------------------------------------------------

#[test]
fn an_order_by_order_feed_builds_a_book_that_tracks_every_resting_order() {
    const L3_SNAPSHOT: &str = r#"{
      "symbol": "NWSC", "sequence": 2000, "at": "2026-08-24T14:59:50Z",
      "status": "open",
      "orders": [
        {"order_ref": 11, "side": "bid", "price": "101.00", "quantity": "300"},
        {"order_ref": 12, "side": "bid", "price": "101.00", "quantity": "200"},
        {"order_ref": 13, "side": "ask", "price": "101.02", "quantity": "400"}
      ]
    }"#;
    const L3_UPDATES: &str = r#"{"updates": [
      {"sequence": 2001, "at": "2026-08-24T14:59:55Z", "type": "order_added",
       "order_ref": 14, "side": "bid", "price": "101.01", "quantity": "150"},
      {"sequence": 2002, "at": "2026-08-24T14:59:56Z", "type": "order_reduced",
       "order_ref": 11, "remaining": "100"},
      {"sequence": 2003, "at": "2026-08-24T14:59:57Z", "type": "order_removed",
       "order_ref": 13}
    ]}"#;
    let server = vendor(vec![L3_SNAPSHOT], vec![L3_UPDATES]);
    let mut adapter = DepthFeedAdapter::new(
        DepthFeedConfig {
            book_kind: BookKind::OrderByOrder,
            ..config(&server.url())
        },
        vec![instrument()],
    )
    .expect("the fixture configuration is valid");

    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let book = book_of(&records);
    assert_eq!(
        book.bids
            .iter()
            .map(|level| (level.price, level.size))
            .collect::<Vec<_>>(),
        vec![(dec!("101.01"), dec!("150")), (dec!("101.00"), dec!("300"))],
        "the level should aggregate order 11's reduced size with order 12's"
    );
    assert!(
        book.asks.is_empty(),
        "order 13 was the whole offer and it left"
    );

    let state = adapter
        .venue_state("NWSC")
        .expect("the instrument has venue state");
    assert_eq!(state.kind(), BookKind::OrderByOrder);
    assert_eq!(
        state.book().resting_orders(),
        3,
        "an order-by-order book tracks each order, which is what lets it answer a queue \
         position an aggregated feed cannot"
    );
    assert!(
        state.book().queue_position(12).is_ok(),
        "and the question is actually answerable"
    );
}

#[test]
fn a_level_one_feed_that_publishes_only_a_quote_still_builds_a_touch() {
    const QUOTES: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "quote",
       "bid": {"price": "101.01", "size": "120"},
       "ask": {"price": "101.02", "size": "340"}}
    ]}"#;
    let server = vendor(vec![SNAPSHOT], vec![QUOTES]);
    let mut adapter = adapter_for(&server);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let book = book_of(&records);

    assert_eq!(book.best_bid().map(|level| level.size), Some(dec!("120")));
    assert_eq!(book.best_ask().map(|level| level.size), Some(dec!("340")));
    assert!(
        !book.bids.iter().any(|level| level.price > dec!("101.01")),
        "a quote is a statement that this is the touch, so anything more aggressive is stale \
         and should have been dropped"
    );
}

#[test]
fn an_order_message_against_an_aggregated_book_is_refused_and_leaves_the_book_to_be_rebuilt() {
    const MIXED: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "level_set",
       "side": "bid", "price": "101.01", "size": "250"},
      {"sequence": 1002, "at": "2026-08-24T14:59:56Z", "type": "order_added",
       "order_ref": 9, "side": "ask", "price": "101.02", "quantity": "50"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT, REBUILT_SNAPSHOT], vec![MIXED, NO_UPDATES]);
    let mut adapter = adapter_for(&server);

    let text = adapter
        .poll(poll_instant())
        .expect_err("an order-by-order message cannot be applied to an aggregated book")
        .to_string();
    assert!(
        text.contains("order-by-order") || text.contains("aggregated"),
        "the refusal should name the resolution mismatch: {text}"
    );
    assert!(
        adapter.awaiting_snapshot("NWSC").unwrap_or(false),
        "1001 was applied before 1002 was refused, so the book is half an update ahead of \
         itself; it must be marked for rebuild rather than published as a whole book"
    );

    let records = adapter
        .poll(at("2026-08-24T15:00:05Z"))
        .expect("the next poll rebuilds and succeeds");
    assert_eq!(
        book_of(&records).sequence,
        1010,
        "a refusal must not wedge the adapter: the next poll rebuilds"
    );
}

// --- point in time ----------------------------------------------------------

#[test]
fn a_book_the_caller_could_not_yet_have_seen_is_withheld_until_the_clock_reaches_it() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = DepthFeedAdapter::new(
        DepthFeedConfig {
            publication_delay: Duration::from_mins(15),
            ..config(&server.url())
        },
        vec![instrument()],
    )
    .expect("the fixture configuration is valid");

    // The last message applied is stamped 14:59:56, so a fifteen-minute
    // entitlement delay puts the book's knowable instant at 15:14:56.
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    assert!(
        records.is_empty(),
        "a delayed entitlement means this deployment was not allowed to see this book yet, got \
         {records:?}"
    );
    assert_eq!(adapter.stats().withheld_late, 1);
    assert_eq!(adapter.stats().emitted, 0);

    let later = adapter
        .poll(at("2026-08-24T15:20:00Z"))
        .expect("the later poll succeeds");
    assert_eq!(
        book_of(&later).sequence,
        1002,
        "the same book should arrive once the clock has passed its knowable instant, which is \
         only possible because a withheld book is not marked as already published"
    );
}

#[test]
fn the_fetch_helper_returns_what_the_vendor_sent_without_the_knowable_gate() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = DepthFeedAdapter::new(
        DepthFeedConfig {
            publication_delay: Duration::from_mins(15),
            ..config(&server.url())
        },
        vec![instrument()],
    )
    .expect("the fixture configuration is valid");

    let records = adapter.fetch(poll_instant()).expect("the fetch succeeds");
    assert_eq!(
        records.len(),
        1,
        "fetch exists so an operator can test the connection and the credential without the \
         answer depending on where the clock is"
    );
    assert_eq!(adapter.stats().withheld_late, 0);
}

// --- the peer is untrusted --------------------------------------------------

#[test]
fn a_body_larger_than_the_limit_is_refused_before_it_is_buffered() {
    let server = TestServer::routed(vec![(
        "snapshot",
        vec![Action::Oversized { bytes: 256 * 1024 }],
    )]);
    let mut adapter = adapter_for(&server);
    let error = adapter
        .poll(poll_instant())
        .expect_err("a body over the cap is refused");
    let text = error.to_string();
    assert!(
        text.contains("body") || text.contains("large") || text.contains("limit"),
        "the refusal should say the body was too large rather than that the JSON was bad: {text}"
    );
}

#[test]
fn a_peer_that_dies_part_way_through_its_own_body_is_a_close_and_not_a_short_book() {
    let server = TestServer::routed(vec![(
        "snapshot",
        vec![Action::Truncated {
            declared: 4096,
            written: 40,
        }],
    )]);
    let mut adapter = adapter_for(&server);
    let error = adapter
        .poll(poll_instant())
        .expect_err("a half-sent body is refused");
    assert!(
        adapter.awaiting_snapshot("NWSC").unwrap_or(false),
        "and the book is not built from the half that arrived"
    );
    assert!(!error.to_string().is_empty());
}

#[test]
fn a_peer_that_accepts_the_connection_and_says_nothing_is_refused_within_the_timeout() {
    let server = TestServer::routed(vec![(
        "snapshot",
        vec![Action::Silent(StdDuration::from_secs(30))],
    )]);
    let mut adapter = adapter_for(&server);
    let started = std::time::Instant::now();
    adapter
        .poll(poll_instant())
        .expect_err("a peer that never answers is refused");
    assert!(
        started.elapsed() < StdDuration::from_secs(5),
        "the read timeout has to bound the wait; a poll loop with no bound stops polling"
    );
}

#[test]
fn an_unreachable_vendor_is_refused_rather_than_waited_on() {
    let mut adapter =
        DepthFeedAdapter::new(config(&address_with_no_listener()), vec![instrument()])
            .expect("the fixture configuration is valid");
    let started = std::time::Instant::now();
    adapter
        .poll(poll_instant())
        .expect_err("nothing is listening");
    assert!(started.elapsed() < StdDuration::from_secs(5));
}

#[test]
fn a_vendor_that_rejects_the_credential_produces_a_denial_that_does_not_quote_it() {
    let server = TestServer::always(Action::json(401, r#"{"error":"bad key"}"#));
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a 401 is a refusal")
        .to_string();
    assert!(text.contains("401"), "{text}");
    assert!(
        !text.contains(API_KEY),
        "a credential in an error message is a credential in a log: {text}"
    );
}

#[test]
fn a_body_that_is_not_json_is_refused_with_an_error_that_names_the_feed() {
    let server = TestServer::routed(vec![("snapshot", vec![Action::json(200, "<html>nope")])]);
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a non-JSON body is refused")
        .to_string();
    assert!(text.contains("test-depth"), "{text}");
    assert!(text.contains("snapshot"), "{text}");
}

#[test]
fn more_levels_than_the_cap_are_refused_even_when_the_body_fits() {
    let levels: Vec<String> = (0..40)
        .map(|i| format!(r#"{{"price":"{}.00","size":"10"}}"#, 100 - i))
        .collect();
    let body = format!(
        r#"{{"symbol":"NWSC","sequence":1000,"at":"2026-08-24T14:59:50Z","status":"open",
            "bids":[{}],"asks":[]}}"#,
        levels.join(",")
    );
    let server = vendor(vec![&body], vec![NO_UPDATES]);
    let mut adapter = DepthFeedAdapter::new(
        DepthFeedConfig {
            max_messages: 10,
            ..config(&server.url())
        },
        vec![instrument()],
    )
    .expect("the fixture configuration is valid");
    let text = adapter
        .poll(poll_instant())
        .expect_err("40 levels against a cap of 10 is refused")
        .to_string();
    assert!(text.contains("40 levels"), "{text}");
}

#[test]
fn a_snapshot_for_another_instrument_is_refused_rather_than_applied_to_this_book() {
    const WRONG: &str = r#"{
      "symbol": "OTHR", "sequence": 1000, "at": "2026-08-24T14:59:50Z", "status": "open",
      "bids": [{"price": "5.00", "size": "1"}], "asks": [{"price": "6.00", "size": "1"}]
    }"#;
    let server = vendor(vec![WRONG], vec![NO_UPDATES]);
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a book for another instrument is refused")
        .to_string();
    assert!(text.contains("OTHR") && text.contains("NWSC"), "{text}");
}

#[test]
fn a_snapshot_at_sequence_zero_is_refused_because_nothing_can_resume_from_it() {
    const ZERO: &str = r#"{
      "symbol": "NWSC", "sequence": 0, "at": "2026-08-24T14:59:50Z", "status": "open",
      "bids": [{"price": "101.00", "size": "500"}], "asks": []
    }"#;
    let server = vendor(vec![ZERO], vec![NO_UPDATES]);
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a snapshot at sequence zero is refused")
        .to_string();
    assert!(text.contains("sequence 0"), "{text}");
    assert!(
        text.contains("idempotency"),
        "and it should say what else breaks: a book at sequence zero has no idempotency key: \
         {text}"
    );
}

#[test]
fn a_side_this_decoder_cannot_name_is_refused_rather_than_guessed_at() {
    const ODD_SIDE: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "level_set",
       "side": "middle", "price": "101.01", "size": "250"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT], vec![ODD_SIDE]);
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("an unknown side is refused")
        .to_string();
    assert!(text.contains("middle"), "{text}");
    assert!(
        text.contains("wrong half"),
        "the refusal should say what a guess would cost: {text}"
    );
}

#[test]
fn a_trade_condition_this_decoder_cannot_name_is_refused_rather_than_read_as_regular() {
    const ODD_CONDITION: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "trade",
       "price": "101.01", "size": "100", "condition": "crossing_session"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT], vec![ODD_CONDITION]);
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("an unknown trade condition is refused")
        .to_string();
    assert!(text.contains("crossing_session"), "{text}");
}

#[test]
fn a_trade_with_no_stated_condition_is_refused_rather_than_read_as_regular() {
    // The failure this guards: an off-exchange or late-reported print whose
    // condition the vendor omitted must not silently become `Regular`, which
    // is the one condition that moves the last-sale price and counts toward
    // session volume (`TradeCondition::updates_last`,
    // `::counts_toward_volume`). `rest.rs` refuses the same omission on a
    // top-of-book trade for the identical reason.
    const NO_CONDITION: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "trade",
       "price": "101.01", "size": "100"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT], vec![NO_CONDITION]);
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a trade stating no condition is refused")
        .to_string();
    assert!(text.contains("no condition"), "{text}");

    // The premise: had this been admitted, it would have moved the session
    // state. Assert the refusal actually left it untouched, so this test does
    // not just check an error string against a change that happened anyway.
    let state = adapter
        .venue_state("NWSC")
        .expect("the instrument has venue state");
    assert_eq!(state.session_volume(), dec!("0"));
    assert_eq!(state.last_trade().map(|trade| trade.price), None);
}

#[test]
fn a_print_the_venue_reported_updates_the_session_without_touching_the_levels() {
    const WITH_TRADE: &str = r#"{"updates": [
      {"sequence": 1001, "at": "2026-08-24T14:59:55Z", "type": "trade",
       "price": "101.02", "size": "100", "condition": "regular", "aggressor": "bid"}
    ]}"#;
    let server = vendor(vec![SNAPSHOT], vec![WITH_TRADE]);
    let mut adapter = adapter_for(&server);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");

    let state = adapter
        .venue_state("NWSC")
        .expect("the instrument has venue state");
    assert_eq!(state.session_volume(), dec!("100"));
    assert_eq!(
        state.last_trade().map(|trade| trade.price),
        Some(dec!("101.02"))
    );
    assert_eq!(
        book_of(&records).asks.len(),
        2,
        "a print is not a book update; the levels are the venue's to change"
    );
}

// --- configuration ----------------------------------------------------------

#[test]
fn an_unconfigured_adapter_names_every_missing_piece_and_opens_no_connection() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = DepthFeedAdapter::new(DepthFeedConfig::default(), Vec::new())
        .expect("an adapter with nothing configured still has to exist in order to say so");

    assert!(!adapter.is_available());
    let missing = adapter.missing_configuration();
    assert_eq!(
        missing.len(),
        3,
        "endpoint, credential and instruments should each be named on their own: {missing:?}"
    );
    assert!(
        missing.iter().any(|m| m.contains("no endpoint")),
        "{missing:?}"
    );
    assert!(
        missing.iter().any(|m| m.contains("no credential")),
        "{missing:?}"
    );
    assert!(
        missing.iter().any(|m| m.contains("no instruments")),
        "{missing:?}"
    );

    let start = adapter.start(poll_instant());
    assert!(matches!(start, Err(Error::Unavailable { .. })), "{start:?}");
    let polled = adapter.poll(poll_instant());
    assert!(
        matches!(polled, Err(Error::Unavailable { .. })),
        "{polled:?}"
    );
    assert!(
        adapter
            .poll(poll_instant())
            .expect_err("still unavailable")
            .to_string()
            .contains("will not substitute a generated book"),
        "an adapter that cannot fetch says so rather than inventing depth"
    );

    assert_eq!(
        server.served(),
        0,
        "an unconfigured adapter must not open a socket at all: there is nothing to ask and \
         nowhere to ask it"
    );
}

#[test]
fn an_endpoint_without_a_credential_is_still_unavailable_and_still_opens_nothing() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = DepthFeedAdapter::new(
        DepthFeedConfig {
            api_key: None,
            ..config(&server.url())
        },
        vec![instrument()],
    )
    .expect("the fixture configuration is valid");

    assert!(!adapter.is_available());
    adapter
        .poll(poll_instant())
        .expect_err("no credential means no fetch");
    assert_eq!(server.served(), 0, "and no connection either");
}

#[test]
fn a_configured_adapter_still_states_what_production_has_to_add() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let adapter = adapter_for(&server);
    assert!(adapter.is_available());

    let descriptor = adapter.descriptor();
    let requirement = descriptor
        .production_requirement
        .clone()
        .expect("a fully configured depth feed is still not a production feed on its own");
    assert!(requirement.contains("TLS"), "{requirement}");
    assert!(requirement.contains("depth licence"), "{requirement}");
    assert!(requirement.contains("dissemination delay"), "{requirement}");
    assert!(
        requirement.contains("resumes from a sequence"),
        "a snapshot-plus-increment feed needs a sequence-addressable resume point, and saying so \
         is the difference between a recovery and a guess: {requirement}"
    );
    assert_eq!(descriptor.topics, vec![Topic::MarketOrderBook]);
    assert_eq!(descriptor.licensing, LicensingClass::Licensed);
    assert!(descriptor.is_production_grade());
    assert_eq!(adapter.instruments(), vec![object_id()]);
    assert_eq!(adapter.symbols(), vec!["NWSC".to_string()]);
}

#[test]
fn the_credential_travels_in_a_header_and_never_in_the_url() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let mut adapter = adapter_for(&server);
    adapter.poll(poll_instant()).expect("the poll succeeds");

    for request in server.requests() {
        assert_eq!(
            request.method, "GET",
            "reading a book is a read; a depth poll that mutated anything at the vendor would \
             be a different kind of request"
        );
        assert!(
            !request.target.contains(API_KEY),
            "a URL is written to every access log on the path: {}",
            request.target
        );
        assert_eq!(
            request.headers.get("x-api-key").map(String::as_str),
            Some(API_KEY),
            "the credential should travel in the configured header"
        );
    }
}

#[test]
fn an_https_endpoint_is_refused_at_configuration_time_rather_than_downgraded() {
    let built = DepthFeedAdapter::new(
        DepthFeedConfig {
            base_url: Some("https://vendor.example".into()),
            api_key: Some(API_KEY.into()),
            ..DepthFeedConfig::default()
        },
        vec![instrument()],
    );
    assert!(
        built.is_err(),
        "the transport has no TLS stack, so an https endpoint would send the credential in \
         clear text if it were quietly downgraded"
    );
}

#[test]
fn two_instruments_numbered_in_one_stream_are_refused_because_their_sequences_would_collide() {
    let built = DepthFeedAdapter::new(
        DepthFeedConfig::default(),
        vec![
            DepthInstrument::new(object_id(), "NWSC", "XNAS", "depth-a"),
            DepthInstrument::new(ObjectId::from_string("obj-othr"), "OTHR", "XNAS", "depth-a"),
        ],
    );
    let text = built
        .expect_err("two instruments cannot share a stream key")
        .to_string();
    assert!(
        text.contains("interleaved"),
        "the refusal should say what would go wrong: two instruments' sequences in one space \
         manufacture gaps neither venue had: {text}"
    );
}

#[test]
fn two_instruments_in_separate_partitions_of_one_feed_are_accepted_and_polled_apart() {
    let built = DepthFeedAdapter::new(
        DepthFeedConfig::default(),
        vec![
            DepthInstrument::new(object_id(), "NWSC", "XNAS", "depth-a"),
            DepthInstrument::new(ObjectId::from_string("obj-othr"), "OTHR", "XNAS", "depth-a")
                .with_partition(1),
        ],
    )
    .expect("distinct partitions are distinct sequence spaces");
    assert_eq!(
        built.symbols(),
        vec!["NWSC".to_string(), "OTHR".to_string()]
    );
    assert_ne!(
        DepthInstrument::new(object_id(), "NWSC", "XNAS", "depth-a").stream_key(),
        DepthInstrument::new(object_id(), "NWSC", "XNAS", "depth-a")
            .with_partition(1)
            .stream_key()
    );
}

#[test]
fn a_credential_header_the_transport_writes_itself_is_refused_at_configuration_time() {
    let built = DepthFeedAdapter::new(
        DepthFeedConfig {
            api_key_header: "Host".into(),
            api_key: Some(API_KEY.into()),
            base_url: Some("http://vendor.example".into()),
            ..DepthFeedConfig::default()
        },
        vec![instrument()],
    );
    let text = built
        .expect_err("the transport writes `host` itself and drops a caller's copy")
        .to_string();
    assert!(text.contains("host"), "{text}");
}

#[test]
fn a_credential_carrying_a_newline_is_refused_before_it_can_forge_a_header() {
    let built = DepthFeedAdapter::new(
        DepthFeedConfig {
            api_key: Some("key\r\nx-admin: true".into()),
            base_url: Some("http://vendor.example".into()),
            ..DepthFeedConfig::default()
        },
        vec![instrument()],
    );
    assert!(built.is_err());
}

#[test]
fn a_symbol_that_would_split_the_request_line_is_refused_when_it_is_configured() {
    let built = DepthFeedAdapter::new(
        DepthFeedConfig::default(),
        vec![DepthInstrument::new(
            object_id(),
            "NWSC&symbol=OTHR",
            "XNAS",
            "depth-a",
        )],
    );
    assert!(built.is_err());
}

#[test]
fn a_path_carrying_its_own_query_is_refused_because_this_adapter_builds_the_query() {
    let built = DepthFeedAdapter::new(
        DepthFeedConfig {
            updates_path: "/v1/depth/updates?mode=full".into(),
            ..DepthFeedConfig::default()
        },
        vec![instrument()],
    );
    let text = built
        .expect_err("a path with a query is refused")
        .to_string();
    assert!(text.contains("update path"), "{text}");
}

#[test]
fn a_configuration_that_would_publish_nothing_or_refuse_everything_is_caught_where_it_is_written() {
    for (label, config) in [
        (
            "a depth of zero publishes a book with no levels",
            DepthFeedConfig {
                depth: 0,
                ..DepthFeedConfig::default()
            },
        ),
        (
            "a message cap of zero refuses every response",
            DepthFeedConfig {
                max_messages: 0,
                ..DepthFeedConfig::default()
            },
        ),
        (
            "a reorder buffer of zero makes every out-of-order update an unrecoverable gap",
            DepthFeedConfig {
                reorder: ReorderPolicy::new(0, Duration::from_millis(50)),
                ..DepthFeedConfig::default()
            },
        ),
        (
            "a feed needs a name because it is the provenance source",
            DepthFeedConfig {
                name: "  ".into(),
                ..DepthFeedConfig::default()
            },
        ),
    ] {
        assert!(
            DepthFeedAdapter::new(config, vec![instrument()]).is_err(),
            "{label}"
        );
    }
}

#[test]
fn the_debug_rendering_of_a_configuration_shows_that_a_credential_is_set_but_never_its_value() {
    let rendered = format!("{:?}", config("http://vendor.example"));
    assert!(
        !rendered.contains(API_KEY),
        "a config in a crash dump or a support ticket must not carry the key: {rendered}"
    );
    assert!(
        rendered.contains("<redacted>"),
        "but whether one is set at all is worth knowing: {rendered}"
    );
}

// --- through the ingestion service ------------------------------------------

#[test]
fn books_from_a_live_fetch_publish_through_the_ingestion_service() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
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
        published, 1,
        "the book passed the validation gate and published"
    );
    let log = log.borrow();
    assert_eq!(
        log.by_topic(Topic::MarketOrderBook).len(),
        1,
        "a book built from a vendor's snapshot and increments reaches the bus like any other \
         record"
    );
    assert!(
        service.non_production_sources().is_empty(),
        "a licensed depth feed is not a stand-in"
    );
}

#[test]
fn the_venue_state_of_a_symbol_this_feed_does_not_carry_is_absent_rather_than_invented() {
    let server = vendor(vec![SNAPSHOT], vec![UPDATES]);
    let adapter = adapter_for(&server);
    assert!(adapter.venue_state("OTHR").is_none());
    assert!(adapter.condition("OTHR").is_none());
    assert!(adapter.awaiting_snapshot("OTHR").is_none());
    assert_eq!(
        adapter.awaiting_snapshot("NWSC"),
        Some(true),
        "before the first poll there is no book, which is not the same as an empty one"
    );
}
