//! `qip demo --live`, run.
//!
//! Every test here stands the demonstration up for real: three listeners are
//! bound, the adapters connect to them, and the walk runs. Nothing is stubbed,
//! because the only thing this command claims is that the live path composes,
//! and a stub is exactly what would make that claim vacuous.
//!
//! Several tests assert a *negative* — that the run never became live-capable,
//! that no address comes from the environment, that a fill was short of what
//! was asked for. Each is paired with an assertion that the thing being looked
//! for exists at all, because a test hunting for something absent proves
//! nothing if it was never present.

// In a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::http::{Handler, Method, Request};
use qip_cli::demo::doubles::{ALTERNATIVE_PATH, MARKET_DATA_PATH, ORDERS_PATH, VendorDouble};
use qip_cli::demo::{
    CycleOutcome, DemoSettings, GAPS, LiveDemo, MAX_CYCLES, STRATEGY, SensedCounts,
};
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
use std::collections::BTreeMap;

/// A demonstration with the shipped settings, stood up and ready to run.
fn stood_up() -> Result<LiveDemo> {
    LiveDemo::stand_up(DemoSettings::default())
}

/// Both cycles of the default run, in order.
fn both_cycles() -> Result<(CycleOutcome, CycleOutcome)> {
    let mut demonstration = stood_up()?;
    let first = demonstration.cycle()?;
    let second = demonstration.cycle()?;
    Ok((first, second))
}

// --- the sockets ------------------------------------------------------------

#[test]
fn a_cycle_reads_its_observations_off_sockets_rather_than_out_of_this_process() -> Result<()> {
    let mut demonstration = stood_up()?;
    let outcome = demonstration.cycle()?;

    // The premise first: there is something to have arrived. A test that only
    // counted requests would pass against an adapter that asked and then threw
    // the answer away.
    assert!(
        outcome.sensed.total() > 100,
        "the walk absorbed {} record(s); the script serves a hundred and twenty bars alone",
        outcome.sensed.total()
    );
    assert!(
        outcome.vendor_requests >= 5,
        "five feeds were polled and only {} request(s) reached the vendor's listener, so at least \
         one adapter answered without opening a socket",
        outcome.vendor_requests
    );
    let vendor = demonstration.peers()[0];
    assert!(
        vendor.served() >= outcome.vendor_requests,
        "the vendor's server finished {} connection(s) and its router saw {} request(s)",
        vendor.served(),
        outcome.vendor_requests
    );
    Ok(())
}

#[test]
fn every_kind_of_record_the_four_feeds_produce_reaches_the_platform_in_one_cycle() -> Result<()> {
    let mut demonstration = stood_up()?;
    let counts = demonstration.cycle()?.sensed;

    // One assertion per feed, so a feed that silently stopped producing is
    // named rather than hidden inside a total that other feeds keep up.
    assert!(counts.bars > 0, "the price feed produced no bars");
    assert!(
        counts.reference > 0,
        "the price feed produced no reference-data change"
    );
    assert!(counts.news > 0, "the document feed produced no news item");
    assert!(
        counts.fundamentals > 0,
        "the document feed produced no fundamentals"
    );
    assert!(
        counts.macro_observations > 0,
        "the document feed produced no macro release"
    );
    assert!(counts.books > 0, "the depth feed produced no book");
    assert!(
        counts.alternative > 0,
        "the alternative-data feed produced no reading"
    );
    Ok(())
}

#[test]
fn the_second_cycle_sees_a_bar_the_first_withheld_although_the_vendor_said_the_same_thing_twice()
-> Result<()> {
    let (first, second) = both_cycles()?;

    // The premise: something was withheld at all. Without this the equality
    // below would pass against an adapter that published everything.
    assert_eq!(
        first.sensed.withheld, 1,
        "the first poll should withhold exactly the bar whose day has not closed"
    );
    assert_eq!(
        second.sensed.withheld, 0,
        "nothing should be withheld once the clock has passed that bar's close"
    );
    assert_eq!(
        second.sensed.bars,
        first.sensed.bars + 1,
        "the same response, polled a day later, should yield exactly one more bar: {} then {}",
        first.sensed.bars,
        second.sensed.bars
    );
    Ok(())
}

// --- the decision -----------------------------------------------------------

#[test]
fn the_jump_that_arrives_over_the_socket_is_what_the_discovery_stage_finds() -> Result<()> {
    let (first, second) = both_cycles()?;
    assert_eq!(
        first.opportunities, 0,
        "the quiet part of the series should produce nothing to act on"
    );
    assert!(
        second.opportunities > 0,
        "the bar carrying the jump reached the platform and discover found nothing: {}",
        second.loop_summary
    );
    assert!(
        second.loop_summary.contains("discover"),
        "the cycle report no longer names its stages: {}",
        second.loop_summary
    );
    Ok(())
}

// --- the order --------------------------------------------------------------

#[test]
fn the_order_leaves_over_a_socket_and_the_fill_that_comes_back_is_short_of_what_was_asked_for()
-> Result<()> {
    let mut demonstration = stood_up()?;
    let outcome = demonstration.cycle()?;

    assert!(
        outcome.venue.accepted,
        "the platform's own control path refused the order: {:?}",
        outcome.venue.refusal
    );
    assert_eq!(
        outcome.venue.submits, 1,
        "exactly one submit should have reached the venue's listener, {} did",
        outcome.venue.submits
    );
    // The premise: something filled. A shortfall asserted against a fill of
    // zero would be true of an order that never arrived.
    assert!(
        outcome.venue.filled > Decimal::ZERO,
        "the venue reported no fill at all, so the shortfall below means nothing"
    );
    assert!(
        outcome.venue.shortfall() > Decimal::ZERO,
        "a partial fill reconciled clean: {} of {}",
        outcome.venue.filled,
        outcome.venue.requested
    );
    assert_eq!(
        outcome.venue.unknown, 0,
        "a venue that answered left an order unknown"
    );
    Ok(())
}

#[test]
fn a_fill_this_run_reports_is_marked_as_the_double_s_own_and_not_as_a_market_s() -> Result<()> {
    let mut demonstration = stood_up()?;
    let outcome = demonstration.cycle()?;
    assert!(
        outcome.venue.simulated,
        "the broker reported a fill that was not simulated; every fill in this run is fabricated \
         by a double in this process"
    );
    Ok(())
}

// --- the mesh ---------------------------------------------------------------

#[test]
fn a_capital_grant_that_crossed_the_mesh_is_verified_before_the_cell_deploys_anything() -> Result<()>
{
    let mut demonstration = stood_up()?;
    let outcome = demonstration.cycle()?;

    assert_eq!(
        outcome.mesh.dispatch, "delivered",
        "the centre could not put the grant on the wire"
    );
    assert_eq!(
        outcome.mesh.verified,
        vec![STRATEGY.to_string()],
        "the cell did not verify the grant it was sent; refusals were {:?}",
        outcome.mesh.refused
    );
    assert!(
        outcome.mesh.refused.is_empty(),
        "the cell refused a grant it should have taken: {:?}",
        outcome.mesh.refused
    );
    Ok(())
}

#[test]
fn the_cell_s_own_state_delta_reaches_the_centre_over_the_same_peer() -> Result<()> {
    let mut demonstration = stood_up()?;
    let outcome = demonstration.cycle()?;

    assert_eq!(
        outcome.mesh.delta, "delivered",
        "the cell could not publish its delta"
    );
    assert_eq!(
        outcome.mesh.absorbed, 1,
        "the centre drained {} delta(s) from its inbox",
        outcome.mesh.absorbed
    );
    let delta = outcome
        .delta
        .as_ref()
        .expect("the centre kept the delta it absorbed");
    assert_eq!(delta.cell, "london-1", "the delta names another cell");
    assert!(
        !delta.utilisation.is_empty(),
        "a cell running under a verified grant reported no strategy authority, so the grant did \
         not reach the delta the centre reads"
    );
    // The grant frame shares the inbox with the deltas, and the receiver is
    // right to skip it rather than refuse it. Asserted because "ignored" going
    // to zero would mean the two consumers had stopped sharing a peer.
    assert_eq!(
        outcome.mesh.ignored, 1,
        "the centre's receiver should skip exactly the capital frame on its own inbox"
    );
    Ok(())
}

// --- what the run says it is ------------------------------------------------

#[test]
fn the_run_says_at_both_ends_that_every_fill_in_it_was_fabricated() -> Result<()> {
    let mut demonstration = stood_up()?;
    let banner = demonstration.banner_lines().join("\n");
    demonstration.cycle()?;
    let closing = demonstration.closing_lines().join("\n");

    assert!(
        banner.contains("NOT A MARKET"),
        "the banner does not say what this is not:\n{banner}"
    );
    assert!(
        banner.contains("FABRICATED"),
        "the banner does not say where the fills come from:\n{banner}"
    );
    assert!(
        banner.contains("NOT production-grade; no capital decision may rest on it"),
        "the banner does not carry the platform's own words for a feed nobody may trade on:\n\
         {banner}"
    );
    assert!(
        closing.contains("made up by"),
        "the closing does not repeat where the fills came from:\n{closing}"
    );
    assert!(
        closing.contains("NOT production-grade; no capital decision may rest on it"),
        "the closing does not carry it either:\n{closing}"
    );
    Ok(())
}

#[test]
fn the_composition_gaps_the_walk_ran_into_are_printed_where_an_operator_sees_them() -> Result<()> {
    let demonstration = stood_up()?;
    let closing = demonstration.closing_lines().join("\n");
    // The premise: there is a list to print.
    assert!(!GAPS.is_empty(), "the walk claims to have found no gaps");
    for gap in GAPS {
        assert!(
            closing.contains(gap),
            "a gap the module records is not in what the run prints:\n{gap}"
        );
    }
    Ok(())
}

#[test]
fn the_demonstration_never_raises_the_autonomy_level_and_never_becomes_live_capable() -> Result<()>
{
    let (first, second) = both_cycles()?;
    for outcome in [&first, &second] {
        assert!(
            !outcome.record.live_capable,
            "cycle {} reported a platform that could reach a live venue",
            outcome.cycle
        );
        assert_eq!(
            outcome.record.autonomy, first.record.autonomy,
            "the autonomy level moved between cycles"
        );
        assert_eq!(
            outcome.record.chain, "intact",
            "the event log's hash chain broke during the run"
        );
    }
    Ok(())
}

#[test]
fn no_peer_in_the_demonstration_can_be_moved_by_configuration() -> Result<()> {
    // The behavioural half: every address is loopback, and the three are
    // distinct, so none of them is a hard-coded port somebody else could own.
    let demonstration = stood_up()?;
    let mut seen = Vec::new();
    for peer in demonstration.peers() {
        assert!(
            peer.url().starts_with("http://127.0.0.1:"),
            "the {} peer is at {}, which is not a loopback address this process bound",
            peer.role(),
            peer.url()
        );
        seen.push(peer.url().to_string());
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 3, "two peers are sharing one address: {seen:?}");

    // The structural half. The rule is that no address reaches this command
    // from outside it, and the only way to keep that rule is for the code to
    // read nothing from outside it. Asserted against the source, because a
    // behavioural test cannot see a variable nobody happened to set.
    for (name, source) in [
        ("demo/mod.rs", include_str!("../src/demo/mod.rs")),
        ("demo/doubles.rs", include_str!("../src/demo/doubles.rs")),
        ("demo/script.rs", include_str!("../src/demo/script.rs")),
    ] {
        assert!(
            !source.contains("env::var") && !source.contains("std::env"),
            "{name} reads the process environment. Every endpoint in this demonstration must come \
             from a listener it bound itself, or it becomes a way to point the live adapters at \
             something else"
        );
    }
    Ok(())
}

// --- the bounds -------------------------------------------------------------

#[test]
fn a_run_length_nobody_could_watch_is_refused() {
    assert!(
        DemoSettings::default().with_cycles(0).is_err(),
        "a run of no cycles was accepted"
    );
    assert!(
        DemoSettings::default().with_cycles(MAX_CYCLES + 1).is_err(),
        "a run longer than the stated maximum was accepted"
    );
    let accepted = DemoSettings::default()
        .with_cycles(MAX_CYCLES)
        .expect("the stated maximum is itself a legal run length");
    assert_eq!(accepted.cycles, MAX_CYCLES);
}

#[test]
fn a_run_that_has_finished_every_cycle_refuses_to_run_another() -> Result<()> {
    let mut demonstration = LiveDemo::stand_up(DemoSettings::default().with_cycles(1)?)?;
    demonstration.cycle()?;
    assert_eq!(demonstration.completed(), 1);
    assert!(
        demonstration.cycle().is_err(),
        "a one-cycle run ran a second cycle, so the bound is not a bound"
    );
    Ok(())
}

#[test]
fn the_clock_advances_by_exactly_the_configured_interval_and_never_by_a_wall_clock() -> Result<()> {
    let settings = DemoSettings::default();
    let (first, second) = both_cycles()?;
    assert_eq!(
        first.at, settings.start,
        "the first cycle did not run at the instant the run was configured with"
    );
    assert_eq!(
        second.at,
        settings.start.saturating_add(settings.interval),
        "the second cycle did not land exactly one interval after the first"
    );
    Ok(())
}

#[test]
fn two_runs_of_the_same_settings_produce_the_same_numbers() -> Result<()> {
    let (first, _) = both_cycles()?;
    let (again, _) = both_cycles()?;
    assert_eq!(
        first.sensed, again.sensed,
        "two runs of one script sensed different things, so the run is not reproducible"
    );
    assert_eq!(
        first.granted, again.granted,
        "two runs granted different capital"
    );
    assert_eq!(
        first.venue.filled, again.venue.filled,
        "two runs were filled differently by a double that fills a fixed size"
    );
    Ok(())
}

// --- the doubles themselves -------------------------------------------------

fn request(method: Method, path: &str) -> Request {
    Request {
        method,
        path: path.to_string(),
        query: BTreeMap::new(),
        headers: BTreeMap::new(),
        body: Vec::new(),
        peer: "127.0.0.1:0".to_string(),
    }
}

#[test]
fn the_vendor_answers_a_feed_it_does_not_serve_with_a_refusal_naming_the_path() {
    let vendor = VendorDouble::new(Timestamp::from_secs(1_787_583_600));
    // The premise: this vendor does serve something, so a 404 below is about
    // the path and not about a double that answers nothing.
    let served = vendor.handle(&request(Method::Get, MARKET_DATA_PATH));
    assert_eq!(
        served.status, 200,
        "the vendor serves no market data at all"
    );

    let refused = vendor.handle(&request(Method::Get, "/v1/nothing-here"));
    assert_eq!(refused.status, 404);
    let body = String::from_utf8_lossy(&refused.body).to_string();
    assert!(
        body.contains("/v1/nothing-here"),
        "the refusal does not name the path that was asked for: {body}"
    );
}

#[test]
fn the_vendor_serves_its_book_increments_once_and_an_empty_list_after_that() {
    let vendor = VendorDouble::new(Timestamp::from_secs(1_787_583_600));
    let first = vendor.handle(&request(Method::Get, "/v1/depth/updates"));
    let second = vendor.handle(&request(Method::Get, "/v1/depth/updates"));
    let first = String::from_utf8_lossy(&first.body).to_string();
    let second = String::from_utf8_lossy(&second.body).to_string();
    assert!(
        first.contains("level_set"),
        "the first poll carried no increments: {first}"
    );
    assert!(
        !second.contains("level_set"),
        "the second poll replayed sequence numbers the book has already applied: {second}"
    );
}

#[test]
fn the_venue_refuses_a_submit_that_names_no_order_rather_than_inventing_one() {
    let venue = qip_cli::demo::doubles::VenueDouble::new();
    let mut anonymous = request(Method::Post, ORDERS_PATH);
    anonymous.body = br#"{"quantity":"100"}"#.to_vec();
    let refused = venue.handle(&anonymous);
    assert_eq!(
        refused.status, 400,
        "a submit naming no client order id was acknowledged, so the acknowledgement is not \
         evidence that any particular order arrived"
    );
}

#[test]
fn counting_records_by_kind_totals_what_was_counted() {
    let counts = SensedCounts {
        bars: 3,
        news: 1,
        books: 1,
        ..SensedCounts::default()
    };
    assert_eq!(counts.total(), 5);
    // The premise of the line the operator reads: kinds with nothing in them
    // are not listed, and kinds with something in them are.
    let described = format!("{counts:?}");
    assert!(described.contains("bars: 3"));
}

#[test]
fn the_alternative_feed_is_wired_to_the_path_the_vendor_actually_serves() {
    // A misconfigured path is a 404 the adapter reports as "no endpoint", which
    // looks exactly like a vendor problem. The constant is shared so the two
    // cannot drift; this asserts that it is shared.
    let vendor = VendorDouble::new(Timestamp::from_secs(1_787_583_600));
    let served = vendor.handle(&request(Method::Get, ALTERNATIVE_PATH));
    assert_eq!(served.status, 200);
}
