//! What a cell offers its peers is the touch its book shows (MESH-5060).
//!
//! Until ADR 0116 this was half of `peer_mesh.rs`, which built the edge state
//! from an assembled cell inside the peer session. The session now lives in
//! `qip-peer-link`, which cannot link the cell, so the cell-to-touch half is
//! proven here, where the cell is, and the touch-to-peer half there.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::{SystemClock, Timestamp, dec};
use qip_edge::cell::CellConfig;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::edge_quotes::{cell_offer, executable_quotes, executable_quotes_counted};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassStats, run_pass};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_peer_link::boundary::MAX_QUOTES;
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
    let mut gateway =
        permitting_to_seed(SimulatedGateway::new(venue.clone(), 7, at(0))?, &[&object])?;
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

/// A cell seeded with `books` two-sided books at `venue`, after one pass.
fn priced_cell(venues: &[(&str, usize)], allocation: &str) -> Result<qip_edge_node::NodeAssembly> {
    let mut config = CellConfig::new("americas", "us-east4");
    for (venue, _) in venues {
        config = config.with_venue(VenueId::new(*venue));
    }
    let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
    let mut node = assemble(
        config,
        features,
        Arc::new(SystemClock),
        RegionCapital::read(Some(allocation))?,
        None,
    )?;
    for (venue, books) in venues {
        let venue = VenueId::new(*venue);
        let objects: Vec<ObjectId> = (0..*books)
            .map(|i| ObjectId::from_string(format!("obj-{i:03}")))
            .collect();
        let listed: Vec<&ObjectId> = objects.iter().collect();
        let mut gateway =
            permitting_to_seed(SimulatedGateway::new(venue.clone(), 7, at(0))?, &listed)?;
        let mut feed = SimulatedFeed::new(venue);
        feed.attach(&mut node.cell)?;
        for (i, object) in objects.iter().enumerate() {
            // Each book a little larger than the last, so value orders them.
            let size = Decimal::from_int(10 + i as i64);
            gateway.seed_touch(object, Side::Buy, dec!("99"), size, at(1))?;
            gateway.seed_touch(object, Side::Sell, dec!("101"), size, at(1))?;
        }
        let mut stats = PassStats::default();
        run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            None,
            &mut stats,
            at(2),
        )?;
    }
    Ok(node)
}

/// A cell holding an allocation whose share the centre has not yet set has
/// nothing free, and authorises no leg though it still shows its touch: an
/// authority is capital the session may reserve, and capital nobody granted
/// is not there to reserve. The funded case, and the halted one, are proven
/// on the decision loop's own rig in `decision_loop.rs`.
///
/// Mutation (run): drop the free-capital check from `leg_authorities` and
/// size each authority at the touch instead, and legs appear.
#[test]
fn a_cell_with_no_capital_free_shows_its_touch_and_authorises_no_leg() -> Result<()> {
    let node = priced_cell(&[("XLON", 2)], "1000")?;
    assert_eq!(
        node.cell.region_allocation_free(),
        Some(Decimal::ZERO),
        "premise: an allocation with no share set has nothing free"
    );
    let offer = cell_offer(&node.cell, at(3));
    assert_eq!(offer.quotes.len(), 2, "premise: two books priced");
    assert!(offer.legs.is_empty(), "authorised {:?}", offer.legs);
    Ok(())
}

/// Past the bound an edge state may carry, the cell keeps its most valuable
/// books and counts the rest (MESH-5060): sixty-four books at one venue and
/// two at another make sixty-six, and the two smallest are withheld — not
/// whichever two the cell happened to iterate last — and said to be.
///
/// Mutation: replace the sort-and-count with `.take(MAX_QUOTES)` and a zero
/// count, and both the count and the kept set fail.
#[test]
fn a_cell_with_more_books_than_an_edge_state_carries_keeps_the_largest_and_counts_the_rest()
-> Result<()> {
    let node = priced_cell(&[("XLON", 64), ("XPAR", 2)], "1000000")?;
    let (quotes, withheld) = executable_quotes_counted(&node.cell);
    assert_eq!(
        quotes.len() as u64 + withheld,
        66,
        "premise: the cell priced sixty-six books"
    );
    assert_eq!(quotes.len(), MAX_QUOTES);
    assert_eq!(withheld, 2);
    // Each venue's obj-000 is the smallest book there; every other book is
    // larger, so these two and no others are left out.
    for venue in ["XLON", "XPAR"] {
        assert!(
            !quotes
                .iter()
                .any(|q| q.venue == venue && q.instrument == "obj-000"),
            "{venue} obj-000 is among the smallest and was kept"
        );
        assert!(
            quotes
                .iter()
                .any(|q| q.venue == venue && q.instrument == "obj-001"),
            "{venue} obj-001 is larger than both withheld books and was left out"
        );
    }
    assert_eq!(cell_offer(&node.cell, at(3)).withheld, 2);
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
