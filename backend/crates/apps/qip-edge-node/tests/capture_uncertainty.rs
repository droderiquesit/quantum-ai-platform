//! What a captured market event says about the clock it was stamped by
//! (R12-RES-7103).
//!
//! `SimulatedFeed` used to store every event with a clock uncertainty of
//! zero, so a venue the cell had already stopped trading for stamping its
//! messages ahead of the cell's own clock still reached the lake as an exact
//! observation. A message cannot be received before it was sent: the lead of
//! the venue's stamp over the cell's receive instant is a proven lower bound
//! on the clock error, and it is what the event now carries.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Duration, SystemClock, dec};
use qip_edge::cell::CellConfig;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
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
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let mut feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    gateway.advance_to(t(venue_at))?;
    gateway.seed_touch(
        &ObjectId::from_string("obj-BOOK"),
        Side::Buy,
        dec!("99"),
        dec!("500"),
        t(venue_at),
    )?;
    feed.publish(&gateway, &mut node.cell, t(cell_at))?;
    Ok(feed
        .take_captured()
        .iter()
        .map(qip_contracts::market_event::MarketEvent::uncertainty)
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
