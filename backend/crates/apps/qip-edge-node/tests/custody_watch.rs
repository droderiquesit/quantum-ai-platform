//! Custody's freshness on the node's own pass (ADR 0242, R12-RES-7099).
//!
//! `qip-edge`'s `journal_pressure` suite proves a cell sizes by a custody
//! reading it is handed. What it cannot see is whether anything on the node
//! produces one: until the pass fed the venue's statement to the cell, no row
//! was ever recorded on a running node and custody read as nothing at all.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::degradation::{Capability, Freshness};
use qip_contracts::venue::VenueId;
use qip_core::SystemClock;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_edge::cell::CellConfig;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassStats, arm_custody_watch, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use std::sync::Arc;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

fn node(config: CellConfig) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    Ok((node, gateway, feed))
}

fn custody(node: &NodeAssembly, secs: i64) -> Option<Freshness> {
    node.cell.narrowing(t(secs)).observed(Capability::Custody)
}

#[test]
fn a_node_armed_for_custody_reads_it_from_the_venues_statement_and_ages_it_when_the_venue_goes_quiet()
-> Result<()> {
    let config =
        arm_custody_watch(CellConfig::new("london-1", "europe-west2").with_venue(venue()))?;
    let (mut node, mut gateway, mut feed) = node(config)?;

    // Premise: before the venue has stated anything the reading is
    // unavailable, not absent: a watch with no statement is a finding.
    assert_eq!(
        custody(&node, 10),
        Some(Freshness::Unavailable),
        "a cell armed for custody read nothing before any statement"
    );

    let mut stats = PassStats::default();
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
    assert_eq!(
        custody(&node, 10),
        Some(Freshness::Fresh),
        "a pass did not hand the cell the venue's custody statement"
    );
    // One pass interval is one statement period: three late is past two
    // periods (stale), five is past four (unavailable), and no pass ran in
    // between to state anything.
    assert_eq!(custody(&node, 13), Some(Freshness::Stale));
    assert_eq!(custody(&node, 15), Some(Freshness::Unavailable));

    // And a later pass restores it: the reading is the newest statement.
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(16),
    )?;
    assert_eq!(custody(&node, 16), Some(Freshness::Fresh));
    Ok(())
}

#[test]
fn a_node_not_armed_for_custody_records_no_custody_row() -> Result<()> {
    let config = CellConfig::new("london-1", "europe-west2").with_venue(venue());
    let (mut node, mut gateway, mut feed) = node(config)?;
    let mut stats = PassStats::default();
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
    assert_eq!(
        custody(&node, 10),
        None,
        "a cell that was not armed recorded a custody row"
    );
    Ok(())
}
