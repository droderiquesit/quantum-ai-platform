//! The feed adapter's two duties to the book, driven through the node's own
//! seam: sequence checking (REFLEX-018) and timestamping (REFLEX-019).
//!
//! `qip_sequencing::tracker::Sequencer` has always been held by the cell and
//! tested in isolation. In the node it could not fire. The only decoder a
//! composition root reaches — the simulated feed's — numbered each line as
//! it decoded it, so every stream the sequencer was shown was contiguous by
//! construction; a reset it did synthesise named a stream and was looked up
//! as an instrument, so it reached no book; and nothing told it the time, so
//! a gap followed by silence was held for ever. Three controls that read as
//! present. Each test here replays a session through `Cell::on_bytes` with
//! the registered decoder — the path a packet takes — and injects the fault
//! the control exists for.
//!
//! The lines are written with `qip_edge_node::feed::level_line`, the feed's
//! own encoder, so nothing here is a second copy of the wire. The lost frame
//! in the last test is lost the way a frame is: the publisher publishes it,
//! and it reaches somebody else.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Duration, SystemClock, dec};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::journal::Decision;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::{SimulatedFeed, level_line};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use std::sync::Arc;

const CELL: &str = "sequence-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const OBJECT: &str = "obj-SEQ";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string(OBJECT)
}

/// A node assembled as `main.rs` assembles one, with the simulated feed's
/// decoder bound to the cell and no strategy: what is under test is what
/// reaches the book, before anything decides on it.
fn assembled() -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    Ok((node, gateway, feed))
}

fn book(cell: &Cell) -> &VenueState {
    cell.liquidity()
        .get(&venue(), &object())
        .expect("the feed tracked the instrument on its first publication")
}

/// The sequence observations the cell journaled, by their leading word.
fn observed(cell: &Cell, word: &str) -> Vec<String> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::GapDetected { detail, .. } if detail.starts_with(&format!("{word}:")) => {
                Some(detail.clone())
            }
            _ => None,
        })
        .collect()
}

/// A 99 × 500 bid and a 101 × 400 ask, published by the feed itself at
/// `t(10)` as sequences 1 and 2 — the contiguous stream every test below
/// then breaks. Asserted, because a test that injects a fault into a stream
/// that never started proves nothing about the fault.
fn two_published_levels() -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let (mut node, mut gateway, mut feed) = assembled()?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    gateway.advance_to(t(10))?;
    let tick = feed.publish(&gateway, &mut node.cell, t(10))?;
    assert_eq!(tick.messages, 2, "the premise is two published levels");
    assert_eq!(feed.sequence(), 2, "the premise is sequences 1 and 2");
    let state = book(&node.cell);
    assert_eq!(state.last_sequence(), Some(2));
    assert_eq!(state.applied(), 2);
    assert_eq!(state.best_bid().map(|level| level.size), Some(dec!("500")));
    assert_eq!(state.best_ask().map(|level| level.size), Some(dec!("400")));
    assert!(
        observed(&node.cell, "gap").is_empty()
            && observed(&node.cell, "duplicate").is_empty()
            && observed(&node.cell, "reorder").is_empty(),
        "the premise is a stream nothing has gone wrong on yet"
    );
    Ok((node, gateway, feed))
}

#[test]
fn a_level_delivered_twice_is_applied_once_and_the_cell_journals_the_duplicate() -> Result<()> {
    let (mut node, _gateway, feed) = two_published_levels()?;

    // Sequence 2 again, carrying a different size. A decoder that numbered
    // what it decoded would call this sequence 3 and the book would show an
    // ask of one — a level nobody sent a second time.
    let again = level_line(2, t(10), OBJECT, BookSide::Ask, dec!("101"), dec!("1"));
    let decoded = node.cell.on_bytes(feed.key(), again.as_bytes(), t(11))?;
    assert_eq!(decoded, 1, "the premise is a line the decoder read");

    let state = book(&node.cell);
    assert_eq!(
        state.best_ask().map(|level| level.size),
        Some(dec!("400")),
        "a level delivered twice was applied twice"
    );
    assert_eq!(state.applied(), 2, "the duplicate reached the book");
    let duplicates = observed(&node.cell, "duplicate");
    assert_eq!(
        duplicates.len(),
        1,
        "the cell dropped a duplicate and said nothing: {duplicates:?}"
    );
    assert!(
        duplicates[0].contains("first at sequence 2"),
        "the duplicate signal does not name the sequence: {}",
        duplicates[0]
    );
    Ok(())
}

#[test]
fn a_level_that_overtakes_its_predecessor_is_held_until_the_stream_is_contiguous() -> Result<()> {
    let (mut node, _gateway, feed) = two_published_levels()?;

    // Sequence 4 before sequence 3.
    let later = level_line(4, t(10), OBJECT, BookSide::Bid, dec!("99"), dec!("50"));
    node.cell.on_bytes(feed.key(), later.as_bytes(), t(11))?;
    let state = book(&node.cell);
    assert_eq!(
        state.best_bid().map(|level| level.size),
        Some(dec!("500")),
        "a level was applied ahead of the one before it"
    );
    assert_eq!(state.last_sequence(), Some(2));
    let gaps = observed(&node.cell, "gap");
    assert_eq!(gaps.len(), 1, "the hole was not journaled: {gaps:?}");
    assert!(gaps[0].contains("3..=3"), "{}", gaps[0]);
    assert!(
        observed(&node.cell, "reorder").is_empty(),
        "the premise is a hole still open"
    );

    // Its predecessor arrives, inside the deadline.
    let earlier = level_line(3, t(10), OBJECT, BookSide::Ask, dec!("101"), dec!("40"));
    node.cell.on_bytes(feed.key(), earlier.as_bytes(), t(11))?;
    let state = book(&node.cell);
    assert_eq!(state.best_ask().map(|level| level.size), Some(dec!("40")));
    assert_eq!(state.best_bid().map(|level| level.size), Some(dec!("50")));
    assert_eq!(
        state.last_sequence(),
        Some(4),
        "the held level was not released behind its predecessor"
    );
    assert!(!state.is_stale(), "a recovered reorder reset the book");
    let reorders = observed(&node.cell, "reorder");
    assert_eq!(
        reorders.len(),
        1,
        "the reorder was recovered and never signalled: {reorders:?}"
    );
    assert!(
        observed(&node.cell, "abandoned").is_empty(),
        "a hole that filled was reported as lost"
    );
    Ok(())
}

#[test]
fn a_dropped_frame_is_never_papered_over_and_the_book_it_left_stale_is_rebuilt_whole() -> Result<()>
{
    let (mut node, mut gateway, mut feed) = two_published_levels()?;
    let mut stats = PassStats::default();

    // A better bid arrives at the venue and its frame is lost: the publisher
    // puts it on the wire — its sequence and its remembered snapshot both
    // move — and it reaches another cell, not this one. (`assembled` binds a
    // decoder for the same venue and feed name, so the other cell can read
    // the frame; that is all it is here for.)
    let (mut elsewhere, _, _) = assembled()?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99.5"), dec!("100"), t(11))?;
    gateway.advance_to(t(11))?;
    let lost = feed.publish(&gateway, &mut elsewhere.cell, t(11))?;
    assert_eq!(lost.messages, 1, "the premise is one lost level");
    assert_eq!(feed.sequence(), 3);

    // The venue moves again and this frame does arrive, as sequence 4.
    gateway.seed_touch(&object(), Side::Sell, dec!("100.5"), dec!("50"), t(12))?;
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(12),
    )?;
    assert_eq!(feed.sequence(), 4);
    let state = book(&node.cell);
    assert_eq!(
        state.best_ask().map(|level| level.price),
        Some(dec!("101")),
        "a level past the hole was applied as if the stream were contiguous"
    );
    assert_eq!(state.last_sequence(), Some(2));
    assert!(
        !state.is_stale(),
        "the hole may still fill inside its deadline"
    );
    let gaps = observed(&node.cell, "gap");
    assert_eq!(gaps.len(), 1, "the hole was not journaled: {gaps:?}");
    assert!(gaps[0].contains("3..=3"), "{}", gaps[0]);

    // A second later the venue has said nothing more and the hole has not
    // filled. The stream is quiet, so only the pass can notice the deadline.
    let quiet = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(13),
    )?;
    let PassOutcome::Ran { feed: tick, .. } = quiet else {
        panic!("a running node reported its pass as halted: {quiet:?}");
    };
    assert_eq!(
        tick.messages, 0,
        "the premise is a pass on which the feed had nothing to say"
    );
    let state = book(&node.cell);
    assert!(
        state.is_stale(),
        "a gap that will not fill left the book serving what it held before the loss"
    );
    assert_eq!(
        state.best_bid().map(|level| level.price),
        None,
        "the reset did not reach the book: it still shows the bid from before the gap"
    );
    let abandoned = observed(&node.cell, "abandoned");
    assert_eq!(
        abandoned.len(),
        1,
        "the gap was given up on and not journaled: {abandoned:?}"
    );

    // The next publication rebuilds the book from the venue's whole depth,
    // the lost level included, rather than from a difference against a
    // snapshot the cell no longer holds.
    let rebuilt = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(14),
    )?;
    let PassOutcome::Ran { feed: tick, .. } = rebuilt else {
        panic!("a running node reported its pass as halted: {rebuilt:?}");
    };
    assert_eq!(tick.resynchronised, 1, "the stale book was not rebuilt");
    assert_eq!(
        tick.messages, 4,
        "the rebuild was not the venue's whole depth"
    );
    let state = book(&node.cell);
    assert!(!state.is_stale(), "the rebuilt book is still held as stale");
    let best_bid = state.best_bid().expect("a rebuilt book has a bid");
    let best_ask = state.best_ask().expect("a rebuilt book has an ask");
    assert_eq!(
        (best_bid.price, best_bid.size),
        (dec!("99.5"), dec!("100")),
        "the level whose frame was lost is still missing"
    );
    assert_eq!((best_ask.price, best_ask.size), (dec!("100.5"), dec!("50")));
    Ok(())
}

#[test]
fn the_venues_instant_reaches_the_book_as_the_venue_stated_it_and_not_as_the_receipt_stamp()
-> Result<()> {
    let (mut node, mut gateway, mut feed) = assembled()?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.advance_to(t(10))?;

    // The node receives the frame three milliseconds after the venue's
    // clock read `t(10)`.
    let received = t(10).saturating_add(Duration::from_millis(3));
    assert_ne!(
        gateway.now(),
        received,
        "the premise is two different instants"
    );
    feed.publish(&gateway, &mut node.cell, received)?;

    assert_eq!(
        book(&node.cell).last_update(),
        Some(t(10)),
        "the book's venue time is the node's receipt stamp: the two fields were written from \
         one value"
    );
    Ok(())
}
