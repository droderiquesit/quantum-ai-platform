//! What a cell offers its peers is the touch its book shows (MESH-5060).
//!
//! Until ADR 0116 this was half of `peer_mesh.rs`, which built the edge state
//! from an assembled cell inside the peer session. The session now lives in
//! `qip-peer-link`, which cannot link the cell, so the cell-to-touch half is
//! proven here, where the cell is, and the touch-to-peer half there.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::{SystemClock, Timestamp, dec};
use qip_edge::cell::CellConfig;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::edge_quotes::executable_quotes;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassStats, run_pass};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use std::sync::Arc;

const OPENED: i64 = 1_760_000_000_000_000_000;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_nanos(OPENED + ms * 1_000_000)
}

/// The quote a cell offers is its book's best bid and ask with their sizes,
/// for the venue and instrument the book is — not a default, and not depth.
///
/// Mutation: swap `bid.size` and `ask.size` in `executable_quotes` and the
/// sizes assertion fails; return an empty vector and the count premise
/// fails.
#[test]
fn a_cells_offered_quote_is_the_touch_its_book_shows() -> Result<()> {
    let venue = VenueId::new("XLON");
    let object = ObjectId::from_string("obj-ACME");
    let config = CellConfig::new("americas", "us-east4").with_venue(venue.clone());
    let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
    let mut node = assemble(
        config,
        features,
        Arc::new(SystemClock),
        RegionCapital::read(Some("1000000"))?,
        None,
    )?;
    let mut gateway = SimulatedGateway::new(venue.clone(), 7, at(0))?;
    let mut feed = SimulatedFeed::new(venue);
    feed.attach(&mut node.cell)?;
    assert!(
        executable_quotes(&node.cell).is_empty(),
        "premise: a cell with no priced book offers nothing"
    );
    gateway.seed_touch(&object, Side::Buy, dec!("99"), dec!("500"), at(1))?;
    gateway.seed_touch(&object, Side::Sell, dec!("101"), dec!("400"), at(1))?;
    let mut stats = PassStats::default();
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        at(2),
    )?;

    let quotes = executable_quotes(&node.cell);
    assert_eq!(quotes.len(), 1, "premise: the cell holds one book");
    let quote = &quotes[0];
    assert_eq!(
        (quote.venue.as_str(), quote.instrument.as_str()),
        ("XLON", "obj-ACME")
    );
    assert_eq!(
        (quote.bid, quote.bid_size, quote.ask, quote.ask_size),
        (dec!("99"), dec!("500"), dec!("101"), dec!("400")),
        "the quote is the touch the cell's book shows"
    );
    assert!(quote.as_of_ns > 0, "the book's update instant is carried");
    Ok(())
}
