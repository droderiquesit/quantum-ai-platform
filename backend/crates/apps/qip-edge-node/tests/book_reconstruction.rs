//! RM-P1-07: a book rebuilt from the simulated venue's feed is the venue's
//! own book, at every sequence number the feed published.
//!
//! `crate::feed` rests its claim to be a paper feed worth trading against on
//! one sentence: "the cell's book is therefore the venue's book". Until this
//! suite that sentence was checked at single points — two published levels,
//! a book rebuilt after a gap — and never across a session in which the
//! venue added, withdrew and matched orders. A feed that forgot to publish a
//! removal, or a size change at a price it had already sent, would leave
//! every one of those points green while the cell priced off levels the
//! venue no longer held.
//!
//! The test is differential. The venue's book is `qip_brokers`' matching
//! engine, which decides every fill; the other is `qip_orderbook`'s
//! `VenueState`, rebuilt from nothing but the canonical ticks the feed
//! captured (`SimulatedFeed::take_captured`, what a tick lake records). The
//! cell's own book, rebuilt live through `Cell::on_bytes`, is held to the
//! same standard. Each step does one thing at the venue and publishes, so
//! each publication is one line and every sequence number is a point at
//! which the venue's book is defined and compared.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Decimal, Duration, SystemClock, dec};
use qip_edge::cell::{CellConfig, Placer};
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::{MAX_LEVELS_PER_SIDE, SimulatedFeed, level_line};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const CELL: &str = "reconstruction-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const OBJECT: &str = "obj-BOOK";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string(OBJECT)
}

/// Bids then asks, each best first, as (price, size).
type Levels = (Vec<(Decimal, Decimal)>, Vec<(Decimal, Decimal)>);

/// The venue's own book, from its matching engine.
fn venue_levels(gateway: &SimulatedGateway) -> Levels {
    gateway
        .quotes()
        .into_iter()
        .find(|depth| depth.object_id == object())
        .map(|depth| {
            (
                depth.bids.iter().map(|l| (l.price, l.size)).collect(),
                depth.asks.iter().map(|l| (l.price, l.size)).collect(),
            )
        })
        .unwrap_or_default()
}

/// A rebuilt book, read through the snapshot every replay test compares.
fn rebuilt_levels(state: &VenueState) -> Levels {
    let book = state.snapshot().book;
    (
        book.bids.iter().map(|l| (l.price, l.size)).collect(),
        book.asks.iter().map(|l| (l.price, l.size)).collect(),
    )
}

/// What one step did at the venue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Add,
    Cancel,
    Fill,
}

/// The state of things after one step's publication.
struct Checkpoint {
    event: Event,
    /// The feed's sequence number once the step was published.
    sequence: u64,
    /// Lines the publication carried.
    lines: usize,
    /// Books the publication rebuilt out of band rather than from lines.
    resynchronised: usize,
    /// What the venue reported traded, cancelled or rested by this step.
    moved: Decimal,
    venue: Levels,
    cell: Levels,
}

/// The assembled node, the simulated venue, its feed, and the record of a
/// session driven through them.
struct Session {
    node: NodeAssembly,
    gateway: SimulatedGateway,
    feed: SimulatedFeed,
    clock: i64,
    ticks: Vec<MarketMessage>,
    checkpoints: Vec<Checkpoint>,
}

impl Session {
    /// A node assembled as `main.rs` assembles one, with the feed's decoder
    /// bound to the cell and no strategy: what is under test is what reaches
    /// the books, before anything decides on them.
    fn new() -> Result<Self> {
        let config = CellConfig::new(CELL, REGION).with_venue(venue());
        let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
        let allocation = RegionCapital::read(Some("1000000000"))?;
        let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
        let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
        let feed = SimulatedFeed::new(venue());
        feed.attach(&mut node.cell)?;
        Ok(Self {
            node,
            gateway,
            feed,
            clock: 0,
            ticks: Vec::new(),
            checkpoints: Vec::new(),
        })
    }

    /// One second on, do one thing at the venue, publish, and record the
    /// lines and both books.
    fn step(
        &mut self,
        event: Event,
        act: impl FnOnce(&mut SimulatedGateway, Timestamp) -> Result<Decimal>,
    ) -> Result<()> {
        self.clock += 1;
        let now = t(self.clock);
        self.gateway.advance_to(now)?;
        let moved = act(&mut self.gateway, now)?;
        let tick = self.feed.publish(&self.gateway, &mut self.node.cell, now)?;
        self.ticks.extend(
            self.feed
                .take_captured()
                .iter()
                .map(|captured| captured.payload().clone()),
        );
        let cell = self
            .node
            .cell
            .liquidity()
            .get(&venue(), &object())
            .map(rebuilt_levels)
            .unwrap_or_default();
        self.checkpoints.push(Checkpoint {
            event,
            sequence: self.feed.sequence(),
            lines: tick.messages,
            resynchronised: tick.resynchronised,
            moved,
            venue: venue_levels(&self.gateway),
            cell,
        });
        Ok(())
    }
}

/// Seed resting venue liquidity.
fn seed(
    side: Side,
    price: Decimal,
    size: Decimal,
) -> impl FnOnce(&mut SimulatedGateway, Timestamp) -> Result<Decimal> {
    move |gateway, at| {
        gateway.seed_touch(&object(), side, price, size, at)?;
        Ok(size)
    }
}

/// Rest, or take with, an order of the cell's. `taking` is the side of the
/// book the order takes: the ask for a buy, the bid for a sell.
fn order(
    id: &'static str,
    taking: BookSide,
    price: Decimal,
    size: Decimal,
) -> impl FnOnce(&mut SimulatedGateway, Timestamp) -> Result<Decimal> {
    move |gateway, at| {
        gateway.place(id, &object(), &venue(), taking, size, price, at)?;
        let filled = gateway
            .execution_reports()
            .iter()
            .fold(Decimal::ZERO, |sum, report| sum + report.quantity);
        Ok(if filled.is_positive() { filled } else { size })
    }
}

/// Withdraw an order of the cell's, returning what the venue withdrew.
fn cancel(id: &'static str) -> impl FnOnce(&mut SimulatedGateway, Timestamp) -> Result<Decimal> {
    move |gateway, at| gateway.cancel(id, &object(), &venue(), at)
}

/// Flow belonging to somebody else that takes from the book.
fn take(
    side: Side,
    price: Decimal,
    size: Decimal,
) -> impl FnOnce(&mut SimulatedGateway, Timestamp) -> Result<Decimal> {
    move |gateway, at| gateway.seed_aggressor(&object(), side, price, size, at)
}

/// How a line changed the levels published before it, judged against a
/// plain map of what the stream had said so far — not against
/// `qip_orderbook`, which is half of what the test compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Add,
    Modify,
    Remove,
}

fn classify(
    published: &mut (BTreeMap<Decimal, Decimal>, BTreeMap<Decimal, Decimal>),
    message: &MarketMessage,
) -> Line {
    let MessageBody::LevelSet {
        side,
        price,
        quantity,
        ..
    } = &message.body
    else {
        panic!(
            "the simulated feed publishes levels and nothing else, got {}",
            message.body.kind()
        );
    };
    let levels = match side {
        BookSide::Bid => &mut published.0,
        BookSide::Ask => &mut published.1,
    };
    if !quantity.is_positive() {
        levels.remove(price);
        Line::Remove
    } else if levels.insert(*price, *quantity).is_some() {
        Line::Modify
    } else {
        Line::Add
    }
}

/// The failure this prevents: a feed whose difference drops a removal or a
/// size change at a price it already published — or a book that applies
/// one wrongly — leaves the cell pricing off depth the venue no longer
/// holds, and every single-point test of the feed still passes.
#[test]
fn a_book_rebuilt_from_the_simulated_venues_feed_is_the_venues_own_book_at_every_sequence()
-> Result<()> {
    let mut session = Session::new()?;
    // Adds: four levels seeded, one joined, two of the cell's orders rested.
    session.step(Event::Add, seed(Side::Buy, dec!("99"), dec!("500")))?;
    session.step(Event::Add, seed(Side::Sell, dec!("101"), dec!("400")))?;
    session.step(Event::Add, seed(Side::Buy, dec!("98.5"), dec!("300")))?;
    session.step(Event::Add, seed(Side::Sell, dec!("101.5"), dec!("200")))?;
    session.step(Event::Add, seed(Side::Buy, dec!("99"), dec!("250")))?;
    session.step(
        Event::Add,
        order("cell-1", BookSide::Ask, dec!("98"), dec!("120")),
    )?;
    session.step(
        Event::Add,
        order("cell-2", BookSide::Bid, dec!("102"), dec!("80")),
    )?;
    // Cancels: one that empties a level, one that shrinks a shared level.
    session.step(Event::Cancel, cancel("cell-1"))?;
    session.step(
        Event::Add,
        order("cell-3", BookSide::Ask, dec!("99"), dec!("100")),
    )?;
    session.step(Event::Cancel, cancel("cell-3"))?;
    // Fills: venue flow partly and wholly taking levels, the cell taking,
    // and the cell's resting order filled as maker; then its remainder
    // withdrawn, which empties the ask side.
    session.step(Event::Fill, take(Side::Sell, dec!("99"), dec!("200")))?;
    session.step(Event::Fill, take(Side::Buy, dec!("101"), dec!("400")))?;
    session.step(
        Event::Fill,
        order("cell-4", BookSide::Ask, dec!("101.5"), dec!("50")),
    )?;
    session.step(Event::Fill, take(Side::Buy, dec!("101.5"), dec!("150")))?;
    session.step(Event::Fill, take(Side::Buy, dec!("102"), dec!("30")))?;
    session.step(Event::Cancel, cancel("cell-2"))?;
    let Session {
        feed,
        ticks,
        checkpoints,
        ..
    } = session;

    // Premise: the stream is the feed's whole output, contiguous from one.
    let last = feed.sequence();
    assert!(last > 0, "premise: the feed published something");
    assert_eq!(
        ticks
            .iter()
            .map(|tick| tick.origin.sequence)
            .collect::<Vec<_>>(),
        (1..=last).collect::<Vec<_>>(),
        "premise: the captured ticks are the feed's sequence, contiguous from one"
    );
    assert_eq!(
        feed.capture_failures(),
        0,
        "premise: every line was captured"
    );
    // Premise: every sequence number closes a publication, so it is a point
    // at which the venue's book is defined and the books below are compared
    // — no line is ever compared mid-frame.
    assert!(
        checkpoints.iter().all(|c| c.lines <= 1),
        "premise: one thing at the venue publishes at most one line"
    );
    let compared: BTreeSet<u64> = checkpoints.iter().map(|c| c.sequence).collect();
    for sequence in 1..=last {
        assert!(
            compared.contains(&sequence),
            "premise: sequence {sequence} is never compared"
        );
    }
    // Premise: both books were built only from lines, never rebuilt out of
    // band, and the venue never held more depth than the feed publishes, so
    // what is compared is the whole book rather than its top.
    assert!(
        checkpoints.iter().all(|c| c.resynchronised == 0),
        "premise: no book was rebuilt out of band"
    );
    assert!(
        checkpoints
            .iter()
            .all(|c| c.venue.0.len() <= MAX_LEVELS_PER_SIDE
                && c.venue.1.len() <= MAX_LEVELS_PER_SIDE),
        "premise: the venue's depth fits what the feed publishes"
    );
    // Premise: every step did what it says at the venue — something rested,
    // was withdrawn or traded.
    for (index, checkpoint) in checkpoints.iter().enumerate() {
        assert!(
            checkpoint.moved.is_positive(),
            "premise: step {} ({:?}) moved nothing at the venue",
            index + 1,
            checkpoint.event
        );
    }
    // Premise: the stream itself carries adds, modifies or cancels, and the
    // lines that fills caused.
    let mut published = Default::default();
    let mut lines: BTreeMap<u64, Line> = BTreeMap::new();
    for tick in &ticks {
        lines.insert(tick.origin.sequence, classify(&mut published, tick));
    }
    let count = |kind: Line| lines.values().filter(|line| **line == kind).count();
    let filled_lines = checkpoints
        .iter()
        .zip(std::iter::once(0).chain(checkpoints.iter().map(|c| c.sequence)))
        .filter(|(checkpoint, _)| checkpoint.event == Event::Fill)
        .map(|(checkpoint, before)| checkpoint.sequence - before)
        .sum::<u64>();
    assert!(count(Line::Add) > 0, "premise: the stream carries adds");
    assert!(
        count(Line::Modify) + count(Line::Remove) > 0,
        "premise: the stream carries modifies or cancels"
    );
    assert!(
        filled_lines > 0,
        "premise: the stream carries the lines fills caused"
    );

    // The property: replay the captured ticks into a fresh book and compare
    // it with the venue's own at every sequence number; the cell's live book
    // must agree too.
    let mut rebuilt = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    let mut next = 0;
    for (index, checkpoint) in checkpoints.iter().enumerate() {
        while next < ticks.len() && ticks[next].origin.sequence <= checkpoint.sequence {
            rebuilt.apply(&ticks[next])?;
            next += 1;
        }
        assert_eq!(
            rebuilt_levels(&rebuilt),
            checkpoint.venue,
            "at sequence {} (step {}, {:?}) the book rebuilt from the feed is not the venue's",
            checkpoint.sequence,
            index + 1,
            checkpoint.event
        );
        assert_eq!(
            checkpoint.cell,
            checkpoint.venue,
            "at sequence {} (step {}, {:?}) the cell's own book is not the venue's",
            checkpoint.sequence,
            index + 1,
            checkpoint.event
        );
    }
    assert_eq!(next, ticks.len(), "every captured tick was replayed");
    Ok(())
}

#[test]
fn a_captured_tick_names_the_wire_line_it_was_decoded_from_by_its_native_hash() -> Result<()> {
    let mut session = Session::new()?;
    session.clock += 1;
    let now = t(session.clock);
    session.gateway.advance_to(now)?;
    session
        .gateway
        .seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), now)?;
    session
        .feed
        .publish(&session.gateway, &mut session.node.cell, now)?;
    let captured = session.feed.take_captured();
    assert!(
        !captured.is_empty(),
        "premise: the publication carried a line"
    );

    for event in &captured {
        let MessageBody::LevelSet {
            side,
            price,
            quantity,
            ..
        } = event.payload().body
        else {
            panic!("the feed publishes levels only");
        };
        let origin = event.origin();
        let wire = level_line(
            origin.sequence,
            event.event_time(),
            OBJECT,
            side,
            price,
            quantity,
        );
        let native = event
            .native_hash()
            .expect("a captured tick keeps the hash of its wire line");
        assert_eq!(native, qip_core::sha256_hex(wire.as_bytes()));
        assert_ne!(
            native,
            event.source_hash(),
            "the wire line and the canonical re-encoding are different bytes"
        );
    }
    Ok(())
}
