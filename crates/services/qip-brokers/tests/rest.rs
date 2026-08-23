//! The order-entry adapter, against a real socket.
//!
//! Every test here binds a listener on loopback and lets the adapter connect to
//! it. A mocked client would prove a decoder was called; it would not prove
//! that a peer which answers with nothing at all, with more bytes than this
//! process will hold, or with a record for somebody else's order, leaves the
//! order *unknown* rather than assumed filled — and on an order-entry path that
//! is the only property that matters.
//!
//! The tests are arranged around the three ways an order can end:
//!
//! * the venue answered and this adapter read the answer;
//! * the venue refused and this adapter read the refusal, which is an answer;
//! * neither, in which case nobody knows, and the adapter's job is to say so
//!   and keep saying so until the venue is asked again.
//!
//! Several tests assert a *negative* — that no request was sent, that no fill
//! was reported, that a string never reached a URL. Each of those is paired
//! with an assertion that the thing being looked for exists at all, because a
//! test searching for something absent proves nothing if it was never present.

#![allow(clippy::panic_in_result_fn)]

mod server;

use qip_brokers::adapter::{AdapterClass, VenueAdapter, VenueOrderState};
use qip_brokers::connection::ConnectionPhase;
use qip_brokers::credential::{
    RequirementKind, Secret, VenueCredential, requirements_of_kind, standard_requirements,
};
use qip_brokers::rest::{IdempotencySupport, RestOrderEntryAdapter, RestVenueConfig};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_execution_engine::broker::Broker;
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_transport::ClientLimits;
use server::{Action, Route, TestServer, address_with_no_listener};
use std::time::Duration as StdDuration;

const VENUE: &str = "XSBX";
const ACCOUNT: &str = "book-under-test";
const ORDERS: &str = "/v1/orders";
const HEALTH: &str = "/v1/health";
const OBJECT: &str = "OBJ00000000000000000000AAA";

/// The session secret the fixtures use.
///
/// A literal, and a distinctive one, so a test can assert it never reaches a
/// request target, an error message or a `Debug` line — and can first assert
/// that it really was sent, which is what stops that from being vacuous.
const SECRET: &str = "session-secret-4c91f0e2-never-in-a-url";

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string(OBJECT)
}

/// Limits tight enough that a test trips them in bytes and milliseconds rather
/// than in megabytes and seconds.
fn tight() -> ClientLimits {
    ClientLimits {
        max_body: 4096,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(250),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

fn config(base_url: Option<String>) -> RestVenueConfig {
    RestVenueConfig {
        base_url,
        http: tight(),
        ..RestVenueConfig::default()
    }
}

fn session_credential_name() -> String {
    standard_requirements(&venue())
        .into_iter()
        .find(|requirement| requirement.kind == RequirementKind::SessionCredential)
        .map(|requirement| requirement.name)
        .expect("the standard list always names a session credential")
}

/// A credential carrying the resolved value, which is the only shape a
/// transport can use.
fn credential() -> VenueCredential {
    let enforced = requirements_of_kind(
        &standard_requirements(&venue()),
        &[RequirementKind::Account, RequirementKind::SessionCredential],
    );
    VenueCredential::satisfying(VENUE, ACCOUNT, &enforced)
        .expect("a named venue and account")
        .with_secret(
            session_credential_name(),
            format!("QIP_{VENUE}_CREDENTIAL"),
            Secret::new(SECRET),
        )
}

fn order(label: &str, quantity: i64) -> Order {
    Order::new(
        OrderId::from_string(label),
        object(),
        Side::Buy,
        Decimal::from_int(quantity),
        OrderType::Limit {
            price: dec!("100.02"),
        },
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    )
}

/// The health route every session needs: `connect`, `authenticate` and
/// `heartbeat` are each a real request, and a test that forgets one gets a 404
/// from the harness rather than a hang.
fn health() -> Route {
    Route::new("GET", HEALTH, Action::json(200, "{}"))
}

/// One venue order record, in the schema this adapter promises to read.
fn record(id: &str, state: &str, quantity: &str, filled: &str, extra: &str) -> String {
    format!(
        r#"{{"client_order_id":"{id}","venue_order_id":"V-1","state":"{state}",
            "instrument":"{OBJECT}","side":"buy","quantity":"{quantity}",
            "filled":"{filled}"{extra}}}"#
    )
}

/// A fill list carrying one fill, for the responses that report a trade.
fn one_fill(fill_id: &str, quantity: &str) -> String {
    format!(
        r#","fills":[{{"fill_id":"{fill_id}","quantity":"{quantity}","price":"100.01",
            "costs":"0.10","at":"2026-08-24T00:00:01Z"}}]"#
    )
}

/// An adapter that has connected, logged on and heartbeated against `server`.
fn brought_up(server: &TestServer) -> Result<RestOrderEntryAdapter> {
    let mut adapter = RestOrderEntryAdapter::new(venue(), config(Some(server.url())), start())?;
    adapter.bring_up(&credential(), start())?;
    Ok(adapter)
}

// --- the venue answered -----------------------------------------------------

#[test]
fn a_submit_the_venue_acknowledges_reports_the_state_and_the_fills_the_venue_stated() -> Result<()>
{
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            Action::json(
                200,
                record("ORD-1", "filled", "100", "100", &one_fill("F-1", "100")),
            ),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let ack = adapter.submit_order(&ticket, &order("ORD-1", 100), start())?;

    // The premise: a request really was sent, so the answer below is the
    // venue's and not a local invention.
    assert_eq!(
        server.hits("POST", ORDERS),
        1,
        "exactly one submit should have reached the venue"
    );
    assert_eq!(ack.state, VenueOrderState::Filled);
    assert_eq!(ack.fills.len(), 1, "the venue reported one fill");
    assert_eq!(ack.filled_quantity(), Decimal::from_int(100));
    assert_eq!(ack.remaining, Decimal::ZERO);
    assert!(
        ack.fills.iter().all(|fill| fill.simulated),
        "every fill carries the adapter's own simulation flag, not the venue's"
    );
    assert!(
        adapter.unknown_orders().is_empty(),
        "an order the venue answered about is not unknown"
    );
    assert_eq!(adapter.stats().acknowledged, 1);
    Ok(())
}

#[test]
fn the_body_of_a_submit_names_the_order_and_carries_the_idempotency_key() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            Action::json(200, record("ORD-1", "working", "100", "0", "")),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let submitted = order("ORD-1", 100);
    let expected = adapter.idempotency_key_for(&submitted)?;

    adapter.submit_order(&ticket, &submitted, start())?;

    let sent = server.requests_to("POST", ORDERS);
    assert_eq!(sent.len(), 1, "one submit reached the venue");
    assert_eq!(
        sent[0].header("idempotency-key"),
        Some(expected.as_str()),
        "the submit carries the key a caller can compute without submitting"
    );
    assert!(
        sent[0].body.contains("ORD-1") && sent[0].body.contains(OBJECT),
        "the body names the order and the instrument: {}",
        sent[0].body
    );
    assert!(
        !expected.is_empty() && expected.chars().all(|c| c.is_ascii_hexdigit()),
        "the key is hex, so it is safe in a header without escaping"
    );
    Ok(())
}

#[test]
fn a_rejection_from_the_venue_is_an_answer_and_not_a_transport_failure() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            // The venue refusing, and saying why, over a status that means it
            // understood the request and would not take it.
            Action::json(
                422,
                record(
                    "ORD-1",
                    "rejected",
                    "100",
                    "0",
                    r#","reason":"instrument not enabled for this account""#,
                ),
            ),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let ack = adapter
        .submit_order(&ticket, &order("ORD-1", 100), start())
        .expect("a venue that refuses an order has answered it, so this is not an error");

    match &ack.state {
        VenueOrderState::Rejected { reason } => {
            assert!(
                reason.contains("not enabled"),
                "the venue's own reason survives to the caller: {reason}"
            );
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert!(ack.fills.is_empty(), "a rejected order traded nothing");
    assert_eq!(ack.remaining, Decimal::from_int(100));
    assert_eq!(adapter.stats().rejected, 1);
    assert!(
        adapter.unknown_orders().is_empty(),
        "a refusal the adapter read is knowledge, not ambiguity"
    );
    Ok(())
}

#[test]
fn a_cancel_needs_no_readiness_ticket_and_records_the_venues_answer() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "DELETE",
            ORDERS,
            Action::json(
                200,
                record(
                    "ORD-1",
                    "cancelled",
                    "100",
                    "0",
                    r#","reason":"cancelled on client request""#,
                ),
            ),
        ),
    ]);
    let mut adapter = brought_up(&server)?;

    // No ticket is minted anywhere in this test: the signature does not take
    // one, which is the property.
    let ack = adapter.cancel_order(&OrderId::from_string("ORD-1"), start())?;

    assert!(matches!(ack.state, VenueOrderState::Cancelled { .. }));
    assert_eq!(server.hits("DELETE", ORDERS), 1);
    let sent = server.requests_to("DELETE", ORDERS);
    assert!(
        sent[0].target.contains("client_order_id=ORD-1"),
        "the cancel names the order by the client's own id: {}",
        sent[0].target
    );
    assert_eq!(adapter.stats().cancels_sent, 1);
    Ok(())
}

#[test]
fn a_degraded_session_refuses_a_new_order_and_still_accepts_a_cancel() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "DELETE",
            ORDERS,
            Action::json(
                200,
                record("ORD-1", "cancelled", "100", "0", r#","reason":"pulled""#),
            ),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    // Long enough after the last heartbeat that the session is untrustworthy.
    let later = start().saturating_add(Duration::from_secs(600));
    assert_eq!(
        adapter.connection().effective_phase(later),
        ConnectionPhase::Degraded,
        "a session that has gone quiet is degraded whether or not anybody polled it"
    );

    let refused = adapter
        .submit_order(&ticket, &order("ORD-1", 100), later)
        .expect_err("new risk needs a healthy session");
    assert_eq!(refused.code(), "denied");
    assert_eq!(
        server.hits("POST", ORDERS),
        0,
        "the refusal happened before anything left the process"
    );

    adapter
        .cancel_order(&OrderId::from_string("ORD-1"), later)
        .expect("cancelling reduces risk and never waits for permission");
    Ok(())
}

// --- nobody knows -----------------------------------------------------------

#[test]
fn a_submit_that_times_out_leaves_the_order_unknown_and_reports_no_fill() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        // Longer than the 250ms read timeout: the request arrives and the
        // answer does not.
        Route::new("POST", ORDERS, Action::Silent(StdDuration::from_secs(2))),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let error = adapter
        .submit_order(&ticket, &order("ORD-1", 100), start())
        .expect_err("a submit with no answer cannot succeed");

    assert_eq!(error.code(), "timeout");
    // The premise: the order really did leave the process. This is ambiguity,
    // not a failure to send, and the two have opposite handling.
    assert_eq!(
        server.hits("POST", ORDERS),
        1,
        "the venue received the order it may or may not have acted on"
    );
    assert_eq!(
        adapter.unknown_orders(),
        vec![OrderId::from_string("ORD-1")],
        "the order is listed as unknown so an operator can count it"
    );
    assert_eq!(adapter.stats().entered_unknown, 1);
    assert!(
        adapter.query_fills(None)?.is_empty(),
        "an unknown order contributes no fills; it is not assumed filled"
    );

    let refusal = adapter
        .query_order(&OrderId::from_string("ORD-1"))
        .expect_err("there is no state to report");
    assert_eq!(
        refusal.code(),
        "unavailable",
        "the absence of a state is reported as absence, not as flat"
    );
    assert!(
        refusal.message().contains("not been assumed filled"),
        "the refusal says what it has not assumed: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn an_unknown_order_becomes_known_only_when_the_venue_answers_a_query() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new("POST", ORDERS, Action::Silent(StdDuration::from_secs(2))),
        Route::new(
            "GET",
            ORDERS,
            Action::json(200, record("ORD-1", "working", "100", "0", "")),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let _ = adapter.submit_order(&ticket, &order("ORD-1", 100), start());
    assert_eq!(
        adapter.unknown_orders().len(),
        1,
        "the premise: the order is unknown before the query"
    );

    let resolved = adapter.reconcile(&OrderId::from_string("ORD-1"), start())?;

    assert_eq!(resolved.state, VenueOrderState::Working);
    assert_eq!(
        resolved.filled,
        Decimal::ZERO,
        "the venue says it has not traded, so the platform now knows that"
    );
    assert!(
        adapter.unknown_orders().is_empty(),
        "the venue's answer, and only the venue's answer, resolved it"
    );
    assert_eq!(adapter.stats().reconciled, 1);
    assert_eq!(adapter.stats().queries_sent, 1);
    Ok(())
}

#[test]
fn a_venue_with_no_record_of_an_order_does_not_resolve_it_to_flat() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new("POST", ORDERS, Action::Silent(StdDuration::from_secs(2))),
        Route::new(
            "GET",
            ORDERS,
            Action::json(404, r#"{"error":"no such order"}"#),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let _ = adapter.submit_order(&ticket, &order("ORD-1", 100), start());

    let error = adapter
        .reconcile(&OrderId::from_string("ORD-1"), start())
        .expect_err("a venue that will not say is not a venue that said no");

    assert_eq!(error.code(), "not_found");
    assert_eq!(
        adapter.unknown_orders(),
        vec![OrderId::from_string("ORD-1")],
        "absence of a record is not evidence the order never arrived, so it stays unknown"
    );
    Ok(())
}

#[test]
fn a_body_that_is_not_the_venue_schema_is_refused_and_leaves_the_order_unknown() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            // A proxy's error page, which is what a venue's endpoint answers
            // with on the day the venue is not the thing answering.
            Action::json(200, "<html><body>gateway error</body></html>"),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let error = adapter
        .submit_order(&ticket, &order("ORD-1", 100), start())
        .expect_err("a body this adapter cannot read is not an acknowledgement");

    assert_eq!(error.code(), "schema");
    assert_eq!(
        adapter.unknown_orders().len(),
        1,
        "an unreadable answer is ignorance, and ignorance is unknown"
    );
    assert!(
        adapter.query_fills(None)?.is_empty(),
        "nothing was decoded, so nothing was booked"
    );
    Ok(())
}

#[test]
fn a_response_larger_than_the_limit_is_refused_before_it_is_held() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        // Four times the 4 kB ceiling this adapter is configured to hold.
        Route::new("POST", ORDERS, Action::Oversized { bytes: 16 * 1024 }),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let error = adapter
        .submit_order(&ticket, &order("ORD-1", 100), start())
        .expect_err("a peer must not be able to decide how much this process holds");

    assert_eq!(
        error.code(),
        "guard",
        "a tripped limit is this process refusing, not the peer failing"
    );
    assert!(
        error.message().contains("4096"),
        "the refusal names the limit that was tripped: {}",
        error.message()
    );
    assert_eq!(
        adapter.unknown_orders().len(),
        1,
        "the order was sent and the answer was never read, so nobody knows"
    );
    Ok(())
}

#[test]
fn an_answer_about_a_different_order_is_not_evidence_about_this_one() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            Action::json(200, record("ORD-SOMEBODY-ELSE", "filled", "100", "100", "")),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let error = adapter
        .submit_order(&ticket, &order("ORD-1", 100), start())
        .expect_err("an acknowledgement naming another order says nothing about this one");

    assert_eq!(error.code(), "invalid");
    assert_eq!(adapter.unknown_orders().len(), 1);
    assert!(
        adapter.query_fills(None)?.is_empty(),
        "the fill on somebody else's order was not booked here"
    );
    Ok(())
}

#[test]
fn an_acknowledgement_that_contradicts_itself_is_refused_rather_than_reconciled() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        // Filled in full, while reporting that less than the full quantity
        // traded. Believing the state would book a position that never traded;
        // believing the number would leave a live order off the books.
        Route::new(
            "POST",
            ORDERS,
            Action::json(200, record("ORD-1", "filled", "100", "40", "")),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let error = adapter
        .submit_order(&ticket, &order("ORD-1", 100), start())
        .expect_err("this adapter does not pick which of two contradictory numbers is true");

    assert_eq!(error.code(), "numeric");
    assert_eq!(adapter.unknown_orders().len(), 1);
    Ok(())
}

// --- idempotency ------------------------------------------------------------

#[test]
fn a_resubmission_of_an_order_the_venue_has_already_answered_about_sends_nothing() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            Action::json(200, record("ORD-1", "working", "100", "0", "")),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let submitted = order("ORD-1", 100);

    let first = adapter.submit_order(&ticket, &submitted, start())?;
    let second = adapter.submit_order(&ticket, &submitted, start())?;

    assert_eq!(first.state, VenueOrderState::Working);
    assert_eq!(second.state, VenueOrderState::Working);
    assert_eq!(
        server.hits("POST", ORDERS),
        1,
        "the venue was asked once, so there is one order at the venue"
    );
    assert_eq!(adapter.stats().duplicates_suppressed, 1);
    assert!(
        second.fills.is_empty(),
        "a suppressed resubmission reports no fills; the first acknowledgement owns them"
    );
    assert_eq!(
        adapter.tracked_orders().len(),
        1,
        "one order was sent and one order is tracked"
    );
    Ok(())
}

#[test]
fn a_second_submit_after_an_unknown_outcome_is_refused_where_the_venue_does_not_deduplicate()
-> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new("POST", ORDERS, Action::Silent(StdDuration::from_secs(2))),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let submitted = order("ORD-1", 100);
    assert_eq!(
        adapter.config().idempotency,
        IdempotencySupport::Absent,
        "the premise: this venue promises nothing about repeated keys, which is the default"
    );

    let _ = adapter.submit_order(&ticket, &submitted, start());
    let error = adapter
        .submit_order(&ticket, &submitted, start())
        .expect_err("a venue that may not deduplicate must not be sent the order twice");

    assert_eq!(error.code(), "guard");
    assert!(
        error.message().contains("reconcile"),
        "the refusal says what to do instead: {}",
        error.message()
    );
    assert_eq!(
        server.hits("POST", ORDERS),
        1,
        "one order left the process, so at most one order exists at the venue"
    );
    Ok(())
}

#[test]
fn a_retry_permitted_by_a_venues_idempotency_carries_the_key_of_the_first_attempt() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::in_turn(
            "POST",
            ORDERS,
            vec![
                Action::Silent(StdDuration::from_secs(2)),
                Action::json(200, record("ORD-1", "working", "100", "0", "")),
            ],
        ),
    ]);
    let mut adapter = RestOrderEntryAdapter::new(
        venue(),
        RestVenueConfig {
            idempotency: IdempotencySupport::Honoured,
            ..config(Some(server.url()))
        },
        start(),
    )?;
    adapter.bring_up(&credential(), start())?;
    let ticket = adapter.ready(start())?;
    let submitted = order("ORD-1", 100);

    let _ = adapter.submit_order(&ticket, &submitted, start());
    let ack = adapter.submit_order(&ticket, &submitted, start())?;

    let sent = server.requests_to("POST", ORDERS);
    assert_eq!(sent.len(), 2, "the premise: the order was sent twice");
    let first = sent[0]
        .header("idempotency-key")
        .expect("every submit carries a key");
    let second = sent[1]
        .header("idempotency-key")
        .expect("every submit carries a key");
    assert_eq!(
        first, second,
        "the key is a function of the order's terms, so the retry is recognisable as one"
    );
    assert_eq!(ack.state, VenueOrderState::Working);
    assert_eq!(
        adapter.tracked_orders().len(),
        1,
        "two requests about one order remain one order"
    );
    assert!(adapter.unknown_orders().is_empty());
    Ok(())
}

#[test]
fn a_venue_that_answers_one_key_with_two_order_ids_is_refused_as_a_duplicate() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::in_turn(
            "POST",
            ORDERS,
            vec![
                Action::json(200, record("ORD-1", "working", "100", "0", "")),
                // The same key, a different venue order id: the venue did not
                // deduplicate and there are now two live orders.
                Action::json(
                    200,
                    r#"{"client_order_id":"ORD-1","venue_order_id":"V-2","state":"working",
                        "instrument":"OBJ00000000000000000000AAA","side":"buy",
                        "quantity":"100","filled":"0"}"#,
                ),
            ],
        ),
        Route::new("DELETE", ORDERS, Action::Silent(StdDuration::from_secs(2))),
    ]);
    let mut adapter = RestOrderEntryAdapter::new(
        venue(),
        RestVenueConfig {
            idempotency: IdempotencySupport::Honoured,
            ..config(Some(server.url()))
        },
        start(),
    )?;
    adapter.bring_up(&credential(), start())?;
    let ticket = adapter.ready(start())?;
    let submitted = order("ORD-1", 100);

    adapter.submit_order(&ticket, &submitted, start())?;
    // A cancel whose answer never comes is what puts a known order back into
    // the unknown state, which is the only way a retry is reached at all.
    let _ = adapter.cancel_order(&submitted.order_id, start());
    let error = adapter
        .submit_order(&ticket, &submitted, start())
        .expect_err("a second venue order id under one key is the failure the key exists to catch");

    assert_eq!(error.code(), "guard");
    assert!(
        error.message().contains("two orders"),
        "the refusal says plainly what has happened: {}",
        error.message()
    );
    assert_eq!(
        adapter.unknown_orders().len(),
        1,
        "the order stays unknown: there are now two at the venue and this adapter tracks neither \
         confidently"
    );
    Ok(())
}

#[test]
fn a_fill_the_venue_reports_twice_is_booked_once() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            Action::json(
                200,
                record(
                    "ORD-1",
                    "partially_filled",
                    "100",
                    "40",
                    &one_fill("F-1", "40"),
                ),
            ),
        ),
        // The venue repeats the whole fill list on the cancel, as venues do.
        Route::new(
            "DELETE",
            ORDERS,
            Action::json(
                200,
                record(
                    "ORD-1",
                    "cancelled",
                    "100",
                    "40",
                    &format!(r#","reason":"pulled"{}"#, one_fill("F-1", "40")),
                ),
            ),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let submitted = order("ORD-1", 100);

    let submit = adapter.submit_order(&ticket, &submitted, start())?;
    assert_eq!(
        submit.fills.len(),
        1,
        "the premise: the fill was reported and booked on the submit"
    );

    let cancel = adapter.cancel_order(&submitted.order_id, start())?;

    assert!(
        cancel.fills.is_empty(),
        "the repeated fill is not reported a second time, which would double a real position"
    );
    assert_eq!(
        adapter.query_fills(None)?.len(),
        1,
        "one trade happened, so this process holds one fill"
    );
    assert_eq!(adapter.stats().fills_deduplicated, 1);
    assert!(matches!(cancel.state, VenueOrderState::Cancelled { .. }));
    Ok(())
}

// --- it refuses, and stands in for nothing ----------------------------------

#[test]
fn an_adapter_with_no_endpoint_refuses_every_instruction_and_opens_no_connection() -> Result<()> {
    // A listener the adapter is deliberately never told about. If the adapter
    // ever invented an endpoint, or fell back to one, this would notice.
    let server = TestServer::routed(vec![health()]);
    let mut adapter = RestOrderEntryAdapter::new(venue(), config(None), start())?;

    assert!(
        !adapter.is_available(),
        "an adapter with no endpoint cannot send"
    );

    let refusals = vec![
        adapter
            .connect(start())
            .expect_err("no endpoint to connect to"),
        adapter
            .authenticate(&credential(), start())
            .expect_err("no endpoint to log on to"),
        adapter
            .bring_up(&credential(), start())
            .expect_err("no endpoint to bring up"),
        adapter
            .submit(&order("ORD-1", 100), start())
            .expect_err("no endpoint to send an order to"),
        adapter
            .reconcile(&OrderId::from_string("ORD-1"), start())
            .expect_err("no endpoint to ask"),
    ];
    for refusal in &refusals {
        assert!(
            matches!(refusal.code(), "unavailable" | "denied"),
            "a missing endpoint is a refusal, not a failure: {refusal}"
        );
    }
    assert!(
        refusals[0].message().contains("will not stand in"),
        "the refusal says it will not substitute a venue: {}",
        refusals[0].message()
    );

    assert_eq!(
        server.served(),
        0,
        "not one connection was opened: the refusal happens before any socket"
    );
    assert!(
        adapter.query_fills(None)?.is_empty(),
        "an adapter that sent nothing reports no fills, simulated or otherwise"
    );
    assert!(
        adapter.tracked_orders().is_empty(),
        "nothing was sent, so nothing is tracked"
    );
    Ok(())
}

#[test]
fn an_adapter_whose_venue_is_unreachable_refuses_rather_than_simulating() -> Result<()> {
    let mut adapter =
        RestOrderEntryAdapter::new(venue(), config(Some(address_with_no_listener())), start())?;

    let error = adapter
        .connect(start())
        .expect_err("nothing is listening on that address");
    assert_eq!(error.code(), "io");

    let refused = adapter
        .submit(&order("ORD-1", 100), start())
        .expect_err("there is no session, so there is no order");
    assert_eq!(
        refused.code(),
        "denied",
        "an order needs a ready venue and there is not one"
    );
    assert!(
        adapter.query_fills(None)?.is_empty(),
        "an unreachable venue produces no fills; there is no simulator behind this adapter"
    );
    Ok(())
}

#[test]
fn a_credential_that_is_only_a_reference_is_not_enough_to_log_on() -> Result<()> {
    let server = TestServer::routed(vec![health()]);
    let mut adapter = brought_up_less_authentication(&server)?;
    // The ordinary shape of a credential: it names where the secret lives and
    // does not carry it.
    let reference_only = VenueCredential::satisfying(
        VENUE,
        ACCOUNT,
        &requirements_of_kind(
            &standard_requirements(&venue()),
            &[RequirementKind::Account, RequirementKind::SessionCredential],
        ),
    )?;

    let error = adapter
        .authenticate(&reference_only, start())
        .expect_err("a transport cannot write a header from a reference");

    assert_eq!(error.code(), "unavailable");
    assert!(
        error.message().contains("with_secret"),
        "the refusal names the fix: {}",
        error.message()
    );
    assert!(
        adapter.account().is_none(),
        "a failed logon leaves the adapter unauthenticated rather than half-configured"
    );
    Ok(())
}

#[test]
fn a_rejected_credential_leaves_the_adapter_unauthenticated() -> Result<()> {
    let server = TestServer::routed(vec![Route::in_turn(
        "GET",
        HEALTH,
        vec![
            // `connect` is deliberately unauthenticated and only proves the
            // endpoint answers; the logon that follows is what is refused.
            Action::json(200, "{}"),
            Action::json(401, r#"{"error":"bad key"}"#),
        ],
    )]);
    let mut adapter = RestOrderEntryAdapter::new(venue(), config(Some(server.url())), start())?;
    adapter.connect(start())?;

    let error = adapter
        .authenticate(&credential(), start())
        .expect_err("the venue refused the credential");

    assert_eq!(error.code(), "denied");
    assert!(
        !error.message().contains(SECRET),
        "the refusal does not quote the credential: {}",
        error.message()
    );
    assert!(
        adapter.account().is_none() && !adapter.is_available(),
        "a refused logon does not leave the adapter looking configured"
    );
    Ok(())
}

/// An adapter that has connected but not logged on.
fn brought_up_less_authentication(server: &TestServer) -> Result<RestOrderEntryAdapter> {
    let mut adapter = RestOrderEntryAdapter::new(venue(), config(Some(server.url())), start())?;
    adapter.connect(start())?;
    Ok(adapter)
}

#[test]
fn the_credential_travels_in_a_header_and_reaches_no_url_and_no_debug_line() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            Action::json(200, record("ORD-1", "working", "100", "0", "")),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    adapter.submit_order(&ticket, &order("ORD-1", 100), start())?;

    let requests = server.requests();
    // Non-vacuity: the secret really was sent, in a header, on requests that
    // needed it. A test that only searched for its absence would pass on an
    // adapter that never authenticated at all.
    assert!(
        requests
            .iter()
            .any(|request| request.header("x-api-key") == Some(SECRET)),
        "the credential is sent, in the header configured for it"
    );
    for request in &requests {
        assert!(
            !request.target.contains(SECRET),
            "a URL is written to every access log on the path: {}",
            request.target
        );
    }
    assert!(
        !format!("{adapter:?}").contains(SECRET),
        "the adapter's own Debug redacts the secret it holds"
    );
    assert!(
        format!("{adapter:?}").contains("redacted"),
        "and says that it has one, which is the part an operator needs"
    );
    Ok(())
}

// --- what it will not pretend to be -----------------------------------------

#[test]
fn the_adapter_refuses_to_amend_to_keep_books_or_to_serve_market_data() -> Result<()> {
    let server = TestServer::routed(vec![health()]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let refusals = vec![
        adapter
            .replace_order(
                &ticket,
                &OrderId::from_string("ORD-1"),
                Decimal::from_int(50),
                None,
                start(),
            )
            .expect_err(
                "an amendment can be partially applied and leave a size nobody can compute",
            ),
        adapter
            .market_data(&object(), start())
            .expect_err("an order API is not a licensed quote source"),
        adapter
            .query_positions()
            .expect_err("this adapter keeps no book"),
        adapter
            .query_cash()
            .expect_err("this adapter settles nothing"),
        adapter
            .query_margin(start())
            .expect_err("margin is the venue's risk desk's number"),
    ];
    for refusal in &refusals {
        assert_eq!(
            refusal.code(),
            "unavailable",
            "a refusal to invent an answer is 'unavailable': {refusal}"
        );
    }
    assert_eq!(
        server.hits("POST", ORDERS),
        0,
        "none of those touched the venue"
    );
    Ok(())
}

#[test]
fn an_operator_may_resolve_an_unknown_order_flat_and_may_not_assert_a_fill() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new("POST", ORDERS, Action::Silent(StdDuration::from_secs(2))),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let _ = adapter.submit_order(&ticket, &order("ORD-1", 100), start());
    let id = OrderId::from_string("ORD-1");

    let refused = adapter
        .resolve_manually(&id, VenueOrderState::Filled, "operator-on-call", start())
        .expect_err("a fill comes from the venue or it does not exist");
    assert_eq!(refused.code(), "denied");

    let unattributed = adapter
        .resolve_manually(
            &id,
            VenueOrderState::Cancelled {
                reason: "confirmed flat on the venue's own screen".into(),
            },
            "   ",
            start(),
        )
        .expect_err("an unattributed resolution is indistinguishable from a bug");
    assert_eq!(unattributed.code(), "invalid");

    adapter.resolve_manually(
        &id,
        VenueOrderState::Cancelled {
            reason: "confirmed flat on the venue's own screen".into(),
        },
        "operator-on-call",
        start(),
    )?;
    assert!(adapter.unknown_orders().is_empty());
    assert_eq!(adapter.stats().resolved_manually, 1);
    assert!(
        adapter
            .tracked(&id)
            .is_some_and(|tracked| tracked.detail.contains("the venue never confirmed")),
        "the record says the venue never confirmed it, because a person did"
    );
    Ok(())
}

#[test]
fn a_configured_adapter_still_reports_what_production_has_not_supplied() -> Result<()> {
    let server = TestServer::routed(vec![health()]);
    let adapter = brought_up(&server)?;

    assert!(
        adapter.is_available(),
        "the premise: everything this adapter can check for itself is supplied"
    );
    let missing = adapter.missing_requirements();
    assert!(
        !missing.is_empty(),
        "an entitlement nobody granted cannot be verified from here, and is not claimed to be"
    );
    assert!(
        missing
            .iter()
            .all(|requirement| requirement.kind != RequirementKind::Endpoint),
        "what has been supplied is no longer listed as missing"
    );
    assert_eq!(adapter.class(), AdapterClass::Sandbox);
    assert!(
        adapter.is_simulated(),
        "the class this crate permits settles nothing, and the flag follows the class"
    );
    assert!(
        RestOrderEntryAdapter::REQUIREMENTS[0].contains("sandbox"),
        "the first thing a deployment owes is the one this code cannot check for itself"
    );
    Ok(())
}

#[test]
fn a_venue_answer_that_walks_a_filled_quantity_backwards_is_refused() -> Result<()> {
    let server = TestServer::routed(vec![
        health(),
        Route::new(
            "POST",
            ORDERS,
            Action::json(
                200,
                record(
                    "ORD-1",
                    "partially_filled",
                    "100",
                    "60",
                    &one_fill("F-1", "60"),
                ),
            ),
        ),
        // A stale read: less filled than the platform has already been told.
        Route::new(
            "GET",
            ORDERS,
            Action::json(200, record("ORD-1", "partially_filled", "100", "10", "")),
        ),
    ]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;
    let submitted = order("ORD-1", 100);
    adapter.submit_order(&ticket, &submitted, start())?;
    assert_eq!(
        adapter
            .tracked(&submitted.order_id)
            .map(|tracked| tracked.filled),
        Some(Decimal::from_int(60)),
        "the premise: the platform has been told 60 traded"
    );

    let error = adapter
        .reconcile(&submitted.order_id, start())
        .expect_err("filled quantity is cumulative and cannot fall");

    assert_eq!(error.code(), "numeric");
    assert_eq!(
        adapter
            .tracked(&submitted.order_id)
            .map(|tracked| tracked.filled),
        Some(Decimal::from_int(60)),
        "the stale answer was refused rather than written down"
    );
    Ok(())
}

#[test]
fn an_error_message_from_a_refused_configuration_names_the_field_that_is_wrong() {
    let credential_in_a_framing_header = RestOrderEntryAdapter::new(
        venue(),
        RestVenueConfig {
            api_key_header: "content-length".into(),
            ..config(None)
        },
        start(),
    )
    .expect_err("the transport writes that header itself and would drop this one");
    assert_eq!(credential_in_a_framing_header.code(), "invalid");
    assert!(
        credential_in_a_framing_header
            .message()
            .contains("content-length"),
        "the refusal names the header: {credential_in_a_framing_header}"
    );

    let path_with_a_query = RestOrderEntryAdapter::new(
        venue(),
        RestVenueConfig {
            orders_path: "/v1/orders?account=guess".into(),
            ..config(None)
        },
        start(),
    )
    .expect_err("this adapter builds the query itself");
    assert_eq!(path_with_a_query.code(), "invalid");

    let one_header_for_two_things = RestOrderEntryAdapter::new(
        venue(),
        RestVenueConfig {
            idempotency_header: "x-api-key".into(),
            ..config(None)
        },
        start(),
    )
    .expect_err("one of the two would overwrite the other");
    assert_eq!(one_header_for_two_things.code(), "invalid");
}

#[test]
fn an_order_the_adapter_will_not_put_on_the_wire_is_refused_before_it_is_sent() -> Result<()> {
    let server = TestServer::routed(vec![health()]);
    let mut adapter = brought_up(&server)?;
    let ticket = adapter.ready(start())?;

    let algorithmic = Order::new(
        OrderId::from_string("ORD-ALGO"),
        object(),
        Side::Buy,
        Decimal::from_int(100),
        OrderType::TimeWeighted { minutes: 30 },
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    );
    let error = adapter
        .submit_order(&ticket, &algorithmic, start())
        .expect_err("an execution algorithm is worked into child orders before any venue sees it");
    assert_eq!(error.code(), "denied");

    let unwriteable = Order::new(
        OrderId::from_string("ORD 1 WITH SPACES"),
        object(),
        Side::Buy,
        Decimal::from_int(100),
        OrderType::Market,
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    );
    let split = adapter
        .submit_order(&ticket, &unwriteable, start())
        .expect_err("an id with a space in it would split the request line");
    assert_eq!(split.code(), "invalid");

    assert_eq!(
        server.hits("POST", ORDERS),
        0,
        "neither order reached the venue"
    );
    assert!(
        adapter.tracked_orders().is_empty(),
        "an order refused before it was sent is not tracked as unknown"
    );
    Ok(())
}
