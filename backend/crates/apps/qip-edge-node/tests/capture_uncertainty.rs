//! What a captured market event says about the clock it was stamped by
//! (R12-RES-7103).
//!
//! `SimulatedFeed` used to store every event with a clock uncertainty of
//! zero, so a venue the cell had already stopped trading for stamping its
//! messages ahead of the cell's own clock still reached the lake as an exact
//! observation. A message cannot be received before it was sent: the lead of
//! the venue's stamp over the cell's receive instant is a proven lower bound
//! on the clock error, and it is what the event now carries.
//!
//! The lead alone is a floor, not the bound. Since the cell was threaded into
//! the capture, every event carries `Cell::clock_uncertainty` for its venue:
//! the lead that venue's last batch showed, or, for a venue the cell has not
//! measured while another venue's clock is skewed past the bound, the widest
//! lead any venue shows. The last two tests here drive that through
//! `run_pass`, the step the decision loop runs.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::degradation::{Capability, Freshness};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Duration, SystemClock, dec};
use qip_edge::cell::{CellConfig, DEFAULT_MAX_CLOCK_SKEW};
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use std::sync::Arc;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

/// The uncertainty of every event the feed captured when the venue's clock
/// reads `venue_at` and the cell's reads `cell_at`.
fn uncertainties(venue_at: i64, cell_at: i64) -> Result<Vec<Duration>> {
    let config = CellConfig::new("capture-1", "europe-west2").with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let object = ObjectId::from_string("obj-BOOK");
    let mut gateway = permitting_to_seed(SimulatedGateway::new(venue(), 7, t(0))?, &[&object])?;
    let mut feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    gateway.advance_to(t(venue_at))?;
    gateway.seed_touch(&object, Side::Buy, dec!("99"), dec!("500"), t(venue_at))?;
    feed.publish(&gateway, &mut node.cell, t(cell_at))?;
    Ok(feed
        .take_captured()
        .iter()
        .map(MarketEvent::uncertainty)
        .collect())
}

#[test]
fn a_venue_stamping_ahead_of_the_cell_is_captured_with_that_lead_as_its_clock_uncertainty()
-> Result<()> {
    // Premise: a level clock captures events, and none of them claims doubt.
    let level = uncertainties(10, 10)?;
    assert!(!level.is_empty(), "premise: the feed captured nothing");
    assert!(
        level.iter().all(|u| *u == Duration::ZERO),
        "premise: a level clock was captured with doubt: {level:?}"
    );

    let ahead = uncertainties(10, 7)?;
    assert_eq!(
        ahead.len(),
        level.len(),
        "the same publication, the same events"
    );
    assert!(
        ahead.iter().all(|u| *u == Duration::from_secs(3)),
        "a venue three seconds ahead of the cell was captured with {ahead:?}"
    );
    Ok(())
}

#[test]
fn a_venue_stamping_behind_the_cell_is_captured_without_inventing_doubt() -> Result<()> {
    // Latency makes the venue's stamp trail the receive instant on every
    // healthy feed. That says nothing about the clock, so it must not widen
    // the event.
    let behind = uncertainties(7, 10)?;
    assert!(!behind.is_empty(), "premise: the feed captured nothing");
    assert!(
        behind.iter().all(|u| *u == Duration::ZERO),
        "ordinary latency was recorded as clock doubt: {behind:?}"
    );
    Ok(())
}

fn other_venue() -> VenueId {
    VenueId::new("XPAR")
}

/// A node configured for both venues, with `other_venue()` already measured
/// by the cell: its gateway's clock read `other_ahead` seconds past the
/// cell's at `t(10)`, published through its own `SimulatedFeed`.
fn node_with_the_other_venue_ahead_by(other_ahead: i64) -> Result<NodeAssembly> {
    let config = CellConfig::new("capture-2", "europe-west2")
        .with_venue(venue())
        .with_venue(other_venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let object = ObjectId::from_string("obj-OTHER");
    let mut gateway =
        permitting_to_seed(SimulatedGateway::new(other_venue(), 11, t(0))?, &[&object])?;
    let mut feed = SimulatedFeed::new(other_venue());
    feed.attach(&mut node.cell)?;
    gateway.advance_to(t(10 + other_ahead))?;
    gateway.seed_touch(
        &object,
        Side::Buy,
        dec!("49"),
        dec!("500"),
        t(10 + other_ahead),
    )?;
    feed.publish(&gateway, &mut node.cell, t(10))?;
    assert!(
        !feed.take_captured().is_empty(),
        "premise: the other venue's feed published nothing, so the cell measured nothing"
    );
    Ok(node)
}

/// The clock uncertainty of every event `feed` captured, after asserting
/// there were some and that each one's venue stamp was level with the
/// cell's receive instant — so that whatever doubt it carries came from the
/// cell, not from its own frame's lead.
fn level_uncertainties(feed: &mut SimulatedFeed) -> Vec<Duration> {
    let captured = feed.take_captured();
    assert!(!captured.is_empty(), "premise: the pass captured nothing");
    assert!(
        captured
            .iter()
            .all(|event| event.event_time() == event.receive_time()),
        "premise: the venue's own stamp was not level with the cell's"
    );
    captured.iter().map(MarketEvent::uncertainty).collect()
}

#[test]
fn a_captured_event_from_a_venue_the_cell_has_not_measured_carries_the_widened_clock_uncertainty()
-> Result<()> {
    // R12-RES-7103. The failure this prevents: the feed stored each event
    // with its own frame's lead only, so while the cell's clock row read
    // unavailable — another venue stamping three seconds ahead of the cell,
    // the arbitrage desk already paused on it — the first frame from a venue
    // the cell had never measured reached the lake as exact, and the deep
    // brain's precision gate had nothing to refuse it on.
    let mut node = node_with_the_other_venue_ahead_by(3)?;
    let skew = Duration::from_secs(3);
    assert!(
        skew > DEFAULT_MAX_CLOCK_SKEW,
        "premise: three seconds is within the cell's skew bound"
    );
    assert_eq!(
        node.cell.narrowing(t(10)).observed(Capability::Clock),
        Some(Freshness::Unavailable),
        "premise: one venue measured, and that one skewed, should read unavailable"
    );

    // XLON's first frame, through the pass the decision loop runs. The pass
    // advances the gateway to the pass instant, so XLON's stamp is level
    // with the cell's and its own lead is zero.
    let object = ObjectId::from_string("obj-BOOK");
    let mut gateway = permitting_to_seed(SimulatedGateway::new(venue(), 7, t(0))?, &[&object])?;
    gateway.advance_to(t(10))?;
    gateway.seed_touch(&object, Side::Buy, dec!("99"), dec!("500"), t(10))?;
    let mut feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let mut stats = PassStats::default();
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
    assert_eq!(stats.passes, 1, "premise: the pass did not run");

    let uncertainties = level_uncertainties(&mut feed);
    assert!(
        uncertainties.iter().all(|u| *u == skew),
        "an event from a venue the cell had not measured, captured while another venue was \
         three seconds ahead, was stored with {uncertainties:?}"
    );
    Ok(())
}

#[test]
fn a_captured_event_from_a_venue_the_cell_has_measured_carries_its_measured_clock_uncertainty()
-> Result<()> {
    // The other half: a measured venue carries the lead its own last batch
    // showed — one second here — and neither the other venue's three nor
    // its own frame's zero. A level frame from a venue whose previous frame
    // led by a second does not prove the pair has come back level, because
    // latency hides a lead; the second the cell measured is the bound it
    // holds when the frame arrives.
    let mut node = node_with_the_other_venue_ahead_by(3)?;
    let object = ObjectId::from_string("obj-BOOK");
    let mut gateway = permitting_to_seed(SimulatedGateway::new(venue(), 7, t(0))?, &[&object])?;
    let mut feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    gateway.advance_to(t(11))?;
    gateway.seed_touch(&object, Side::Buy, dec!("99"), dec!("500"), t(11))?;
    feed.publish(&gateway, &mut node.cell, t(10))?;
    assert!(
        !feed.take_captured().is_empty(),
        "premise: XLON's first frame captured nothing, so the cell measured nothing"
    );
    assert_eq!(
        node.cell.narrowing(t(10)).observed(Capability::Clock),
        Some(Freshness::Unavailable),
        "premise: both venues lead past the bound, so the unmeasured arm would widen to three"
    );

    // A second frame through the pass, its own stamp level with the cell's.
    gateway.seed_touch(&object, Side::Buy, dec!("100"), dec!("300"), t(11))?;
    let mut stats = PassStats::default();
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(12),
    )?;
    assert_eq!(stats.passes, 1, "premise: the pass did not run");

    let uncertainties = level_uncertainties(&mut feed);
    assert!(
        uncertainties.iter().all(|u| *u == Duration::from_secs(1)),
        "an event from a venue the cell measured a second ahead was stored with \
         {uncertainties:?}; three seconds is the other venue's figure, zero the frame's own"
    );
    Ok(())
}

/// `gateway`, permitted to list each of `objects` on demand and to seed
/// liquidity into it: a recorded venue, product and jurisdiction permission
/// for each action on each instrument (R12-COVERAGE-5107). `Seed` as well as
/// `List`, because the tests here seed the touch they trade against and
/// permission to list is not permission to seed. A fixture; the refusals
/// without either are proven in qip-edge-node's tests/gateway.rs.
fn permitting_to_seed(
    gateway: SimulatedGateway,
    objects: &[&ObjectId],
) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for object in objects {
        for action in [
            CreationAction::Create,
            CreationAction::List,
            CreationAction::Seed,
        ] {
            let request = CreationRequest::new(action, venue.as_str(), object.as_str(), "paper");
            for authority in CreationAuthority::ALL {
                permissions.record(authority, &request, "test fixture")?;
            }
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}
