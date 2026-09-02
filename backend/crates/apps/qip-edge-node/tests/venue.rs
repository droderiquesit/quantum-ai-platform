//! Which venue the node's orders reach, and what it refuses to do quietly.
//!
//! Two halves. The first is a pure one: [`VenueChoice::read`] takes a lookup
//! rather than the process environment, so every refusal is reachable from a
//! map — which matters because `std::env::set_var` is `unsafe` in this edition
//! and this workspace forbids `unsafe_code`, so a test that had to mutate the
//! environment to reach a branch could not be written at all.
//!
//! The second half opens sockets. The gateway that sends real orders is only
//! worth testing where it meets one, so those tests script a loopback venue and
//! assert on the bytes it received: that an order the cell placed arrived, that
//! the credential travelled in a header and never in the URL, and — the
//! property the whole adapter is arranged around — that a submit the venue
//! never answered leaves the order *unknown* rather than rejected.

#![allow(clippy::panic_in_result_fn)]

mod server;

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::dec;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_edge::cell::Placer;
use qip_edge_node::gateway::{NodeGateway, RestGateway};
use qip_edge_node::venue::{
    ACKNOWLEDGEMENT_VARIABLE, ADAPTER_VARIABLE, DESTINATION_PREFIX, IDEMPOTENCY_VARIABLE,
    LiveVenueChoice, REST_ADAPTER, SEED_VARIABLE, VenueChoice,
};
use server::{Action, RawRequest, Route, TestVenue};
use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

const VENUE: &str = "sandbox-venue";
const ACCOUNT: &str = "cell-account-1";
const SECRET: &str = "a-session-secret-nobody-logs";

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// A lookup over a fixed map, which is what `VenueChoice::read` takes instead
/// of the process environment.
fn lookup(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

fn read(env: &BTreeMap<String, String>, ceiling_permits_live: bool) -> Result<VenueChoice> {
    VenueChoice::read(
        &|name| env.get(name).cloned(),
        &venue(),
        ceiling_permits_live,
    )
}

/// Everything a live selection needs except the acknowledgement, which each
/// test adds, omits or corrupts as its own premise requires.
fn live_env(endpoint: &str) -> Vec<(&'static str, String)> {
    vec![
        (ADAPTER_VARIABLE, REST_ADAPTER.to_string()),
        ("QIP_SANDBOX_VENUE_ENDPOINT", endpoint.to_string()),
        ("QIP_SANDBOX_VENUE_CREDENTIAL", SECRET.to_string()),
        ("QIP_SANDBOX_VENUE_ACCOUNT", ACCOUNT.to_string()),
    ]
}

fn with(pairs: Vec<(&'static str, String)>) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

// --- the choice, without a socket -------------------------------------------

#[test]
fn a_deployment_that_configures_nothing_places_against_the_in_process_exchange() -> Result<()> {
    // The premise: an empty environment. Not "the variables happen to be
    // unset in this process" — an explicitly empty map, so the assertion is
    // about the code's default rather than about the test runner's ambient
    // state.
    let env = lookup(&[]);
    let choice = read(&env, false)?;

    assert!(
        matches!(choice, VenueChoice::Simulated { seed: 1 }),
        "silence must select the simulator: {choice:?}"
    );
    assert!(
        !choice.reaches_a_socket(),
        "the default selection must not be able to send an order anywhere"
    );
    assert_eq!(choice.selector(), "simulated");
    Ok(())
}

#[test]
fn the_simulated_venue_still_replays_because_its_seed_is_configuration() -> Result<()> {
    let env = lookup(&[(SEED_VARIABLE, "424242")]);
    assert!(matches!(read(&env, false)?, VenueChoice::Simulated { seed } if seed == 424_242));

    // And a seed that is not a number is refused rather than silently becoming
    // the default: a session seeded by accident is a session that does not
    // replay, which is the only reason the seed is configuration at all.
    let mistyped = lookup(&[(SEED_VARIABLE, "4o4")]);
    let error = read(&mistyped, false).expect_err("a mistyped seed was accepted");
    assert!(
        error.message().starts_with("configuration:"),
        "{}",
        error.message()
    );
    assert!(error.message().contains("replay"), "{}", error.message());
    Ok(())
}

#[test]
fn the_live_adapter_is_selected_only_when_an_operator_names_the_endpoint() -> Result<()> {
    let endpoint = "http://venue-egress.internal:9443";

    // The premise, stated as an assertion rather than assumed: without the
    // acknowledgement this configuration is complete in every other respect —
    // endpoint, credential and account are all present.
    let unacknowledged = with(live_env(endpoint));
    for name in [
        "QIP_SANDBOX_VENUE_ENDPOINT",
        "QIP_SANDBOX_VENUE_CREDENTIAL",
        "QIP_SANDBOX_VENUE_ACCOUNT",
    ] {
        assert!(unacknowledged.contains_key(name), "{name} is the premise");
    }
    let error = read(&unacknowledged, false).expect_err("an unacknowledged venue was selected");
    assert_eq!(error.code(), "denied", "{}", error.message());
    assert!(
        error.message().contains(ACKNOWLEDGEMENT_VARIABLE),
        "the refusal must name the variable that unblocks it: {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("sandbox host from a production one")
            || error.message().contains("production"),
        "the refusal must say why an acknowledgement is the only control there is: {}",
        error.message()
    );

    // With it, and only with it, the live adapter is chosen.
    let mut acknowledged = live_env(endpoint);
    acknowledged.push((ACKNOWLEDGEMENT_VARIABLE, endpoint.to_string()));
    let choice = read(&with(acknowledged), false)?;
    let VenueChoice::Live(live) = &choice else {
        panic!("an acknowledged endpoint did not select the live adapter: {choice:?}");
    };
    assert_eq!(live.endpoint, endpoint);
    assert_eq!(live.account, ACCOUNT);
    assert_eq!(live.venue.as_str(), VENUE);
    assert!(choice.reaches_a_socket());
    assert_eq!(choice.selector(), REST_ADAPTER);
    Ok(())
}

#[test]
fn an_acknowledgement_of_a_different_address_acknowledges_nothing() -> Result<()> {
    // This is the sandbox-to-production edit, which is the whole reason the
    // acknowledgement is the endpoint rather than a boolean: a flag set once
    // would still be set after somebody changed the host underneath it.
    let mut env = live_env("http://venue-production.example:443");
    env.push((
        ACKNOWLEDGEMENT_VARIABLE,
        "http://venue-sandbox.example:443".to_string(),
    ));
    let error = read(&with(env), false).expect_err("a stale acknowledgement was accepted");

    assert_eq!(error.code(), "denied", "{}", error.message());
    assert!(
        error.message().contains("venue-production.example")
            && error.message().contains("venue-sandbox.example"),
        "the refusal must show both addresses so an operator can see which moved: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_live_venue_under_a_ceiling_that_permits_live_trading_needs_an_explicit_enablement()
-> Result<()> {
    let endpoint = "http://venue-egress.internal:9443";
    let mut acknowledged = live_env(endpoint);
    acknowledged.push((ACKNOWLEDGEMENT_VARIABLE, endpoint.to_string()));
    let env = with(acknowledged);

    // The premise, asserted: this exact configuration is accepted while the
    // ceiling is paper. Nothing about the venue changes below — only whether
    // the cell would be permitted to execute live.
    assert!(
        read(&env, false).is_ok(),
        "the configuration under test must be otherwise complete"
    );

    let error = read(&env, true).expect_err("the dangerous combination was accepted");
    assert_eq!(error.code(), "denied", "{}", error.message());
    assert!(
        error.message().contains("QIP_SANDBOX_VENUE_ENABLED"),
        "the refusal must name the enablement that unblocks it: {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("autonomy ceiling permits live execution"),
        "the refusal must name the other half of the combination: {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("holding a credential is not the same as having decided to trade"),
        "the refusal must say why a credential is not consent: {}",
        error.message()
    );

    // And with the enablement, the same configuration is accepted.
    let mut enabled: Vec<(&'static str, String)> = live_env(endpoint);
    enabled.push((ACKNOWLEDGEMENT_VARIABLE, endpoint.to_string()));
    enabled.push(("QIP_SANDBOX_VENUE_ENABLED", VENUE.to_string()));
    let choice = read(&with(enabled), true)?;
    assert!(matches!(choice, VenueChoice::Live(_)), "{choice:?}");
    Ok(())
}

#[test]
fn an_enablement_copied_from_another_venue_does_not_enable_this_one() -> Result<()> {
    let endpoint = "http://venue-egress.internal:9443";
    let mut env = live_env(endpoint);
    env.push((ACKNOWLEDGEMENT_VARIABLE, endpoint.to_string()));
    env.push(("QIP_SANDBOX_VENUE_ENABLED", "some-other-venue".to_string()));

    let error = read(&with(env), true).expect_err("another venue's enablement was honoured");
    assert_eq!(error.code(), "denied", "{}", error.message());
    assert!(
        error.message().contains(VENUE),
        "the refusal must name the venue the enablement has to carry: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_adapter_name_this_node_cannot_build_is_refused_rather_than_defaulted() -> Result<()> {
    let env = lookup(&[(ADAPTER_VARIABLE, "fix")]);
    let error = read(&env, false).expect_err("an unknown adapter name was accepted");

    assert!(
        error.message().starts_with("configuration:"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("simulated") && error.message().contains("rest"),
        "the refusal must name what it does accept: {}",
        error.message()
    );
    assert!(
        error.message().contains("quietly did not"),
        "falling back to the simulator here would be the silent failure: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_live_selection_missing_its_credential_names_every_variable_at_once() -> Result<()> {
    let env = lookup(&[
        (ADAPTER_VARIABLE, REST_ADAPTER),
        (ACKNOWLEDGEMENT_VARIABLE, "http://venue.internal:9443"),
    ]);
    let error = read(&env, false).expect_err("an unconfigured live venue was selected");

    for name in [
        "QIP_SANDBOX_VENUE_ENDPOINT",
        "QIP_SANDBOX_VENUE_CREDENTIAL",
        "QIP_SANDBOX_VENUE_ACCOUNT",
    ] {
        assert!(
            error.message().contains(name),
            "deploying a venue should be one restart, not three; {name} is missing from: {}",
            error.message()
        );
    }
    Ok(())
}

#[test]
fn a_credential_resolved_to_the_empty_string_counts_as_absent() -> Result<()> {
    // A secret manager that resolved nothing writes an empty string, not an
    // unset variable, and that is the failure that looks exactly like success.
    let env = lookup(&[
        (ADAPTER_VARIABLE, REST_ADAPTER),
        ("QIP_SANDBOX_VENUE_ENDPOINT", "http://venue.internal:9443"),
        ("QIP_SANDBOX_VENUE_CREDENTIAL", "   "),
        ("QIP_SANDBOX_VENUE_ACCOUNT", ACCOUNT),
        (ACKNOWLEDGEMENT_VARIABLE, "http://venue.internal:9443"),
    ]);
    let error = read(&env, false).expect_err("a blank credential was accepted");
    assert!(
        error.message().contains("QIP_SANDBOX_VENUE_CREDENTIAL"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_idempotency_setting_the_adapter_cannot_honour_is_refused() -> Result<()> {
    let endpoint = "http://venue.internal:9443";
    let mut env = live_env(endpoint);
    env.push((ACKNOWLEDGEMENT_VARIABLE, endpoint.to_string()));
    env.push((IDEMPOTENCY_VARIABLE, "probably".to_string()));

    let error = read(&with(env), false).expect_err("an unknown idempotency setting was accepted");
    assert!(
        error.message().contains("honoured") && error.message().contains("absent"),
        "{}",
        error.message()
    );
    Ok(())
}

// --- the banner --------------------------------------------------------------

#[test]
fn the_banner_names_the_venue_the_orders_will_actually_reach() -> Result<()> {
    let endpoint = "http://venue-egress.internal:9443";
    let mut env = live_env(endpoint);
    env.push((ACKNOWLEDGEMENT_VARIABLE, endpoint.to_string()));
    let choice = read(&with(env), false)?;

    let banner = choice.banner_lines("paper_trading").join("\n");
    assert!(
        banner.contains(endpoint),
        "the banner must print the address orders go to: {banner}"
    );
    assert!(
        banner.contains(VENUE) && banner.contains(ACCOUNT),
        "the banner must name the venue and the account: {banner}"
    );
    assert!(
        banner.contains("CANNOT tell that endpoint's sandbox host from its production host"),
        "the banner must surface the requirement the code cannot enforce: {banner}"
    );
    assert!(
        banner.contains("paper_trading"),
        "the banner must state the ceiling beside the destination: {banner}"
    );
    for line in choice.banner_lines("paper_trading") {
        assert!(
            line.starts_with(DESTINATION_PREFIX),
            "every destination line carries one greppable prefix: {line}"
        );
    }

    // And the secret is not in it. The banner is the most-copied text this
    // process emits.
    assert!(!banner.contains(SECRET), "the banner leaked the credential");
    assert!(
        !format!("{choice:?}").contains(SECRET),
        "the choice's Debug leaked the credential"
    );
    Ok(())
}

#[test]
fn the_simulated_banner_says_no_order_leaves_the_process() -> Result<()> {
    let choice = read(&lookup(&[]), false)?;
    let banner = choice.banner_lines("paper_trading").join("\n");

    assert!(banner.contains("No order leaves this process"), "{banner}");
    assert!(
        banner.contains(ADAPTER_VARIABLE),
        "an operator reading this should learn how to change it: {banner}"
    );
    Ok(())
}

// --- the socket ---------------------------------------------------------------

/// The three health answers a session bring-up needs: connect, authenticate,
/// heartbeat.
fn health_route() -> Route {
    Route::new("GET", "/v1/health", Action::json(200, r#"{"status":"ok"}"#))
}

fn live_choice(endpoint: &str) -> Result<LiveVenueChoice> {
    let mut env = live_env(endpoint);
    env.push((ACKNOWLEDGEMENT_VARIABLE, endpoint.to_string()));
    match read(&with(env), false)? {
        VenueChoice::Live(live) => Ok(live),
        other => panic!("the live adapter was not selected: {other:?}"),
    }
}

fn filled_answer(order_id: &str) -> String {
    format!(
        r#"{{"client_order_id":"{order_id}","venue_order_id":"v-1","state":"filled",
            "instrument":"obj-ABC","side":"buy","quantity":"10","filled":"10",
            "limit_price":"100.5",
            "fills":[{{"fill_id":"f-1","quantity":"10","price":"100.5","costs":"0.05",
                       "at":1760000000000000000}}]}}"#
    )
}

#[test]
fn an_order_the_cell_places_leaves_the_process_and_comes_back_on_the_drop_copy() -> Result<()> {
    let venue_server = TestVenue::routed(vec![
        health_route(),
        Route::new(
            "POST",
            "/v1/orders",
            Action::json(200, filled_answer("cell-1")),
        ),
    ]);
    let choice = live_choice(&venue_server.url())?;
    let mut gateway = RestGateway::connect(&choice, at())?;

    // The premise: bringing the session up cost three authenticated reads of
    // the health path and sent no order.
    assert_eq!(
        venue_server.requests_to("GET", "/v1/health").len(),
        3,
        "connect, authenticate and heartbeat each ask the venue whether it is there"
    );
    assert!(venue_server.requests_to("POST", "/v1/orders").is_empty());

    gateway.place(
        "cell-1",
        &ObjectId::from_string("obj-ABC"),
        &venue(),
        BookSide::Ask,
        dec!("10"),
        dec!("100.5"),
        at(),
    )?;

    let submits = venue_server.requests_to("POST", "/v1/orders");
    assert_eq!(submits.len(), 1, "exactly one order left the process");
    let submit: &RawRequest = &submits[0];
    assert!(
        submit.body.contains("\"client_order_id\":\"cell-1\""),
        "the venue received the cell's own order id: {}",
        submit.body
    );
    assert!(
        submit.body.contains("\"side\":\"buy\""),
        "taking the ask is a buy: {}",
        submit.body
    );

    // The credential travelled in a header and never in the URL, because a URL
    // is written to every access log on the path.
    assert_eq!(submit.header("x-api-key"), Some(SECRET));
    assert!(
        !submit.target.contains(SECRET),
        "the credential reached the request target: {}",
        submit.target
    );
    assert!(
        submit
            .header("idempotency-key")
            .is_some_and(|key| !key.is_empty()),
        "every submit carries a key derived from the order's terms"
    );

    // And the fill came back on the order-entry channel — the one the order
    // went out on — and not on the drop copy, because this gateway holds no
    // drop-copy session and must not pretend the acknowledgement is one.
    let reports = gateway.execution_reports();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].order_id, "cell-1");
    assert_eq!(reports[0].quantity, dec!("10"));
    assert_eq!(reports[0].venue, venue());
    assert!(
        gateway.drain_drop_copies().is_empty(),
        "an acknowledgement was handed to the drop-copy channel, so reconciliation would \
         compare the venue's answer with itself"
    );
    assert!(
        gateway
            .required_configuration()
            .iter()
            .any(|requirement| requirement.contains("drop-copy session")),
        "the missing drop copy is not reported as a production requirement"
    );
    assert_eq!(gateway.submitted_count(), 1);
    assert_eq!(gateway.rejected_count(), 0);
    assert_eq!(
        gateway.unknown_orders(),
        0,
        "an answered order is not an unknown one"
    );
    Ok(())
}

#[test]
fn a_submit_the_venue_never_answers_leaves_the_order_unknown_rather_than_rejected() -> Result<()> {
    // The venue accepts the connection and then says nothing for longer than
    // the adapter will wait. Whether the order arrived is exactly what nobody
    // knows, and "unknown" is a third state rather than a synonym for either
    // of the other two.
    let venue_server = TestVenue::routed(vec![
        health_route(),
        Route::new(
            "POST",
            "/v1/orders",
            Action::Silent(StdDuration::from_secs(30)),
        ),
    ]);
    let choice = live_choice(&venue_server.url())?;
    let mut gateway = RestGateway::connect(&choice, at())?;

    let outcome = gateway.place(
        "cell-2",
        &ObjectId::from_string("obj-ABC"),
        &venue(),
        BookSide::Ask,
        dec!("10"),
        dec!("100.5"),
        at(),
    );
    assert!(
        outcome.is_err(),
        "an ambiguous submit must not be reported as a placement"
    );

    assert_eq!(
        gateway.unknown_orders(),
        1,
        "the order the venue never answered about is the one to alert on"
    );
    assert!(
        gateway.execution_reports().is_empty() && gateway.drain_drop_copies().is_empty(),
        "an unknown order contributes no fills; inferring one would create a position \
         the venue does not have"
    );
    assert_eq!(gateway.stats().entered_unknown, 1);
    assert_eq!(
        gateway.stats().acknowledged,
        0,
        "nothing was acknowledged, so nothing may be recorded as acknowledged"
    );
    Ok(())
}

#[test]
fn a_gateway_that_reaches_a_socket_still_reports_the_requirement_the_code_cannot_enforce()
-> Result<()> {
    let venue_server = TestVenue::routed(vec![health_route()]);
    let choice = live_choice(&venue_server.url())?;
    let gateway = NodeGateway::Live(RestGateway::connect(&choice, at())?);

    assert!(gateway.reaches_a_socket());
    assert_eq!(gateway.class(), "sandbox");
    assert!(
        gateway.is_simulated(),
        "the adapter's class says paper for every endpoint, which is precisely why the \
         requirement below is reported rather than considered discharged"
    );

    let requirements = gateway.required_configuration().join(" | ");
    assert!(
        requirements.contains("nothing here can tell a sandbox host from a production one"),
        "the first standing requirement must survive into what the node publishes: {requirements}"
    );
    assert!(
        requirements.contains("TLS-terminating egress proxy"),
        "the transport has no TLS and the requirement list is where that is stated: {requirements}"
    );
    assert!(
        requirements.contains("alert on the count of unknown orders"),
        "{requirements}"
    );
    Ok(())
}

#[test]
fn the_simulated_gateway_still_places_and_reports_that_nothing_left_the_process() -> Result<()> {
    let choice = read(&lookup(&[]), false)?;
    let mut gateway = NodeGateway::open(&choice, venue(), at())?;

    assert!(!gateway.reaches_a_socket());
    assert_eq!(gateway.class(), "simulated");
    assert_eq!(gateway.unknown_orders(), 0);

    gateway.place(
        "cell-3",
        &ObjectId::from_string("obj-ABC"),
        &venue(),
        BookSide::Ask,
        dec!("5"),
        dec!("100"),
        at().saturating_add(Duration::from_secs(1)),
    )?;
    assert_eq!(gateway.submitted_count(), 1);
    Ok(())
}

#[test]
fn an_order_for_a_venue_this_gateway_does_not_hold_a_session_with_is_refused() -> Result<()> {
    let venue_server = TestVenue::routed(vec![health_route()]);
    let choice = live_choice(&venue_server.url())?;
    let mut gateway = RestGateway::connect(&choice, at())?;
    let before = venue_server.served();

    let error = gateway
        .place(
            "cell-4",
            &ObjectId::from_string("obj-ABC"),
            &VenueId::new("a-different-venue"),
            BookSide::Ask,
            dec!("1"),
            dec!("100"),
            at(),
        )
        .expect_err("an order was re-routed to whatever venue was connected");

    assert_eq!(error.code(), "denied", "{}", error.message());
    assert_eq!(
        venue_server.served(),
        before,
        "the refusal happened before anything reached a socket"
    );
    Ok(())
}
