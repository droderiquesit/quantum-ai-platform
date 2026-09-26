//! The tape driver's own contract: what it refuses at parse, what it seeds
//! at the venue, and what it never lets a wall clock decide.
//!
//! Nothing here deploys a strategy or runs a `Cell` — that is `pass.rs`'s
//! seam, and `strategies.rs`/`features.rs` already prove a deployed strategy
//! fires and fills against a hand-seeded book. What only this suite can
//! prove is the driver's own contract: a tape refused rather than repaired,
//! an aggressor never told it filled more than the depth actually resting,
//! and a tape that seeds the venue the same way however many times it is
//! read. The committed fixture targets `obj-SLICE13` — the instrument
//! `fixtures/slice-strategy-plan.json`'s strategy reads — so a node that
//! later deploys that plan and applies this tape has real liquidity and real
//! fills for it to confirm; this suite proves the tape itself drives at
//! least five of them at the venue, independent of whether any strategy is
//! deployed.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::MessageBody;
use qip_contracts::venue::VenueId;
use qip_core::Clock;
use qip_core::dec;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_edge_node::feed::FeedChoice;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::tape::TapeDriver;
use std::fs;
use std::path::{Path, PathBuf};

const VENUE: &str = "XLON";

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

/// Where the committed fixture lives, read from disk rather than rebuilt in
/// memory: what is under test is that *this reviewed file* seeds the venue,
/// not a copy the test happens to construct the same way.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/reflex-slice-tape.jsonl")
}

/// A synthetic entitlement, granted well past every instant this suite uses.
fn granted() -> &'static str {
    r#"{"dataset":"tape-test","expires_at":"2030-01-01T00:00:00Z"}"#
}

/// One line of tape text. `entitlement` is the raw JSON for the field —
/// `None` renders `null`, so a test can build a line with no licence without
/// hand-writing the rest of the object twice.
#[allow(clippy::too_many_arguments)]
fn line(
    sequence: u64,
    kind: &str,
    side: &str,
    price: &str,
    quantity: &str,
    receive_time: &str,
    entitlement: Option<&str>,
) -> String {
    let entitlement = entitlement.unwrap_or("null");
    format!(
        "{{\"object_id\":\"obj-TAPE-TEST\",\"venue\":\"{VENUE}\",\"feed\":\"test-tape\",\
         \"sequence\":{sequence},\"kind\":\"{kind}\",\"side\":\"{side}\",\"price\":\"{price}\",\
         \"quantity\":\"{quantity}\",\"event_time\":\"{receive_time}\",\
         \"receive_time\":\"{receive_time}\",\"normalized_time\":\"{receive_time}\",\
         \"uncertainty_ms\":0,\"entitlement\":{entitlement}}}"
    )
}

#[test]
fn a_tape_whose_receive_times_go_backwards_is_refused_not_sorted() -> Result<()> {
    // The premise: the same two lines, receivable in the order they are
    // written, load without complaint.
    let ordered = format!(
        "{}\n{}\n",
        line(
            1,
            "touch",
            "Ask",
            "100",
            "500",
            "2025-06-01T00:00:00Z",
            Some(granted())
        ),
        line(
            2,
            "aggressor",
            "Ask",
            "100",
            "100",
            "2025-06-01T00:01:00Z",
            Some(granted())
        ),
    );
    TapeDriver::parse(&ordered, Some(FeedChoice::Simulated)).expect("an ordered tape did not load");

    // The second line's receive_time precedes the first's: out of order.
    let backwards = format!(
        "{}\n{}\n",
        line(
            1,
            "touch",
            "Ask",
            "100",
            "500",
            "2025-06-01T00:01:00Z",
            Some(granted())
        ),
        line(
            2,
            "aggressor",
            "Ask",
            "100",
            "100",
            "2025-06-01T00:00:00Z",
            Some(granted())
        ),
    );
    let refusal = TapeDriver::parse(&backwards, Some(FeedChoice::Simulated))
        .expect_err("a tape whose receive times go backwards was admitted");
    assert!(
        refusal.message().contains("out of order"),
        "the refusal does not say the tape is out of order: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_tape_event_without_an_entitlement_is_refused() -> Result<()> {
    // The premise: the same line, carrying an entitlement, loads.
    let with_grant = format!(
        "{}\n",
        line(
            1,
            "touch",
            "Ask",
            "100",
            "500",
            "2025-06-01T00:00:00Z",
            Some(granted())
        )
    );
    TapeDriver::parse(&with_grant, Some(FeedChoice::Simulated))
        .expect("the premise: a line with an entitlement loads");

    let without_grant = format!(
        "{}\n",
        line(
            1,
            "touch",
            "Ask",
            "100",
            "500",
            "2025-06-01T00:00:00Z",
            None
        )
    );
    let refusal = TapeDriver::parse(&without_grant, Some(FeedChoice::Simulated))
        .expect_err("a tape line with no entitlement was admitted");
    assert!(
        refusal.message().contains("no entitlement"),
        "the refusal does not name the missing entitlement: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_tape_is_refused_unless_the_feed_mode_is_simulated() -> Result<()> {
    let text = format!(
        "{}\n",
        line(
            1,
            "touch",
            "Ask",
            "100",
            "500",
            "2025-06-01T00:00:00Z",
            Some(granted())
        )
    );
    // The premise: the same tape, with the feed mode simulated, loads.
    TapeDriver::parse(&text, Some(FeedChoice::Simulated))
        .expect("the premise: a tape loads when the feed mode is simulated");

    let refusal =
        TapeDriver::parse(&text, None).expect_err("a tape was admitted with no feed configured");
    assert!(
        refusal
            .message()
            .contains(qip_edge_node::feed::SIMULATED_FEED),
        "the refusal does not name the feed mode this driver requires: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn no_fill_is_ever_larger_than_the_depth_the_tape_seeded() -> Result<()> {
    // TICK-006: a touch of 100 seeds exactly 100 of depth; an aggressor
    // asking for 250 must never be told more than that 100 traded.
    let text = format!(
        "{}\n{}\n",
        line(
            1,
            "touch",
            "Ask",
            "100",
            "100",
            "2025-06-01T00:00:00Z",
            Some(granted())
        ),
        line(
            2,
            "aggressor",
            "Ask",
            "100",
            "250",
            "2025-06-01T00:01:00Z",
            Some(granted())
        ),
    );
    let mut driver = TapeDriver::parse(&text, Some(FeedChoice::Simulated))?;
    let mut gateway = SimulatedGateway::new(venue(), 11, t(0))?;
    let mut applied = Vec::new();
    while let Some(now) = driver.advance() {
        applied.extend(driver.seed(&mut gateway, now)?);
    }
    // The premise: both lines were actually applied, not silently dropped.
    assert_eq!(applied.len(), 2, "the premise: both lines applied");
    let MessageBody::Trade { quantity, .. } = &applied[1].payload().body else {
        panic!("the second applied event is not the aggressor's trade: {applied:?}");
    };
    assert_eq!(
        *quantity,
        dec!("100"),
        "the aggressor's reported fill exceeded the depth the tape seeded"
    );
    Ok(())
}

#[test]
fn the_same_tape_applied_twice_on_a_manual_clock_seeds_the_venue_identically() -> Result<()> {
    let text = fs::read_to_string(fixture_path())
        .unwrap_or_else(|e| panic!("the committed fixture is unreadable: {e}"));

    let run = |seed: u64| -> Result<(Vec<MarketEvent>, Timestamp)> {
        let mut driver = TapeDriver::parse(&text, Some(FeedChoice::Simulated))?;
        let mut gateway = SimulatedGateway::new(venue(), seed, t(0))?;
        let mut applied = Vec::new();
        while let Some(now) = driver.advance() {
            applied.extend(driver.seed(&mut gateway, now)?);
        }
        Ok((applied, driver.clock().now()))
    };

    let (applied_a, clock_a) = run(7)?;
    let (applied_b, clock_b) = run(7)?;

    // The premise: the committed fixture actually drives at least five
    // fills for the slice strategy's instrument. Asserted before the
    // equality checks below, because those would hold just as well of two
    // empty runs and prove nothing about the property under test.
    let fills = applied_a
        .iter()
        .filter(|event| matches!(event.payload().body, MessageBody::Trade { .. }))
        .count();
    assert!(
        fills >= 5,
        "the committed tape must drive at least five fills for the slice strategy's instrument, \
         got {fills}"
    );

    assert_eq!(
        applied_a, applied_b,
        "two independent applications of the same tape did not seed the venue identically"
    );
    assert_eq!(
        clock_a, clock_b,
        "the two runs' clocks did not end at the same instant"
    );
    assert_eq!(
        clock_a,
        Timestamp::parse_rfc3339("2025-06-02T00:05:00Z").expect("a literal instant parses"),
        "the driver's clock did not end where the committed fixture's last line says it should; \
         a clock reading the wall rather than the tape would not land here"
    );
    Ok(())
}
