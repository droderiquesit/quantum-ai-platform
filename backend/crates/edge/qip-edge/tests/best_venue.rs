//! §27.2's consolidation onto the best venue, as ADR 0078 places it: in
//! `Cell::venue_for`, before the intent exists.
//!
//! In this platform a strategy cannot specify a venue — `Signal` has no
//! venue field — so every intent is in the "did not specify" case, and the
//! venue every one carries was chosen by the cell, per signal, before the
//! intent existed. Within one pass that choice is deterministic on the
//! cell's own books, so N strategies' signals on one instrument resolve to
//! one venue, land on one netting key and become one order. What the row
//! asked for and the cell did not do was the word *best*: it took the first
//! configured venue with a book. These tests drive the criterion — the
//! tightest quoted spread, ties by venue id — and its journal record.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_routing::UNSPECIFIED_VENUE;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
/// Lexically first of the two, so a tie broken by venue id lands here.
const FIRST_BY_ID: &str = "XLON";
/// Lexically second.
const SECOND_BY_ID: &str = "XPAR";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-best-venue-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn level(at: &VenueId, sequence: u64, side: BookSide, price: &str, size: &str) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(at.clone(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        t(1),
        t(1),
    )
}

/// A two-sided book at `name` with the given touch. The mid is the same
/// at every venue here — 100 — so nothing but the spread differs between
/// candidates and a pick can only be explained by it.
fn book(name: &str, bid: &str, ask: &str) -> Result<VenueState> {
    let id = venue(name);
    let mut state = VenueState::aggregated(object(), id.clone(), VenueStatus::Open);
    state.apply(&level(&id, 0, BookSide::Bid, bid, "500"))?;
    state.apply(&level(&id, 1, BookSide::Ask, ask, "400"))?;
    Ok(state)
}

fn firing_strategy(id: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(d("10")),
            Expr::Statistic(0.5),
            10,
        ),
    );
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue(FIRST_BY_ID), venue(SECOND_BY_ID)],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

/// A gateway that accepts every order and remembers where each went.
#[derive(Debug, Default)]
struct RecordingGateway {
    placed: Vec<(String, VenueId)>,
}

impl Placer for RecordingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((order_id.to_string(), venue.clone()));
        Ok(())
    }
}

/// A cell configured for `venues` in that order, holding `books`, with two
/// strategies that both want to buy the instrument on every pass.
fn cell_with(venues: &[&str], books: Vec<VenueState>) -> Result<Cell> {
    let mut config = CellConfig::new(CELL, REGION);
    for name in venues {
        config = config.with_venue(venue(name));
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    for state in books {
        cell.track(state);
    }
    for id in ["alpha", "beta"] {
        let (compiled, program) = firing_strategy(id)?;
        cell.deploy_with_pricing(
            compiled,
            program,
            signed_envelope(id)?,
            PricingPolicy::Marketable,
        )?;
    }
    Ok(cell)
}

/// Every `venue_chosen` entry, as `(venue, candidates)`.
fn choices(cell: &Cell) -> Vec<(String, Vec<(String, String)>)> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::VenueChosen {
                venue, candidates, ..
            } => Some((venue.clone(), candidates.clone())),
            _ => None,
        })
        .collect()
}

fn one_pass(cell: &mut Cell, gateway: &mut RecordingGateway, at: Timestamp) -> Result<WorkReport> {
    let report = cell.work(at, gateway)?;
    assert!(
        report.refusals.is_empty(),
        "the premise is a pass that refuses nothing: {:?}",
        report.refusals
    );
    Ok(report)
}

#[test]
fn two_strategies_on_one_instrument_net_into_one_order_at_the_venue_with_the_tighter_spread()
-> Result<()> {
    // The failure this prevents: "first configured" read as "best". The
    // wider spread is configured first, so a cell that still took the first
    // venue with a book sends the order there and pays a spread twice the
    // size of the one next door.
    let mut cell = cell_with(
        &[FIRST_BY_ID, SECOND_BY_ID],
        vec![
            book(FIRST_BY_ID, "99", "101")?,
            book(SECOND_BY_ID, "99.5", "100.5")?,
        ],
    )?;
    let mut gateway = RecordingGateway::default();

    let report = one_pass(&mut cell, &mut gateway, t(10))?;
    assert_eq!(
        report.orders.len(),
        1,
        "two strategies on one instrument did not net into one order: {:?}",
        report.orders
    );
    let order = &report.orders[0];
    assert_eq!(
        order.contributors.len(),
        2,
        "the premise failed: the one order does not carry both strategies"
    );
    assert_eq!(
        order.venue.as_str(),
        SECOND_BY_ID,
        "the order went to the first configured venue rather than the one with the tighter \
         spread"
    );
    assert_eq!(
        gateway.placed,
        vec![(order.order_id.clone(), venue(SECOND_BY_ID))],
        "the venue the gateway saw is not the venue the order names"
    );

    // Journaled with every candidate and the figure compared, once per
    // signal, so the pick is reproducible from the chain alone. The whole
    // record is compared, not searched: a record that merely *contains* the
    // winner also contains it as a candidate.
    let chosen = choices(&cell);
    assert_eq!(
        chosen.len(),
        2,
        "one choice per signal was not journaled: {chosen:?}"
    );
    let expected = (
        SECOND_BY_ID.to_string(),
        vec![
            (FIRST_BY_ID.to_string(), "2".to_string()),
            (SECOND_BY_ID.to_string(), "1".to_string()),
        ],
    );
    assert_eq!(
        chosen[0], expected,
        "the journal record does not name the pick and both spreads"
    );
    assert_eq!(chosen[1], expected);

    // The same books twice: the same venue twice.
    let again = one_pass(&mut cell, &mut gateway, t(20))?;
    assert_eq!(again.orders.len(), 1);
    assert_eq!(again.orders[0].venue.as_str(), SECOND_BY_ID);
    Ok(())
}

#[test]
fn a_tie_on_spread_is_broken_by_venue_id_order_and_not_by_configured_order() -> Result<()> {
    // Two cells configured in different orders must choose alike, or a
    // replay on a cell whose venue list was written in another order is not
    // a replay. The lexically second venue is configured first here, so a
    // tie broken by configured order lands on it and a tie broken by id does
    // not.
    let mut cell = cell_with(
        &[SECOND_BY_ID, FIRST_BY_ID],
        vec![
            book(SECOND_BY_ID, "99", "101")?,
            book(FIRST_BY_ID, "99", "101")?,
        ],
    )?;
    let mut gateway = RecordingGateway::default();

    let report = one_pass(&mut cell, &mut gateway, t(10))?;
    assert_eq!(report.orders.len(), 1, "{:?}", report.orders);
    let chosen = choices(&cell);
    assert!(
        chosen
            .iter()
            .all(|(_, candidates)| candidates.iter().all(|(_, spread)| spread == "2")),
        "the premise failed: the spreads are not tied: {chosen:?}"
    );
    assert_eq!(
        report.orders[0].venue.as_str(),
        FIRST_BY_ID,
        "a tie was broken by configured order rather than by venue id"
    );
    Ok(())
}

#[test]
fn a_venue_with_no_usable_book_is_not_a_candidate_even_though_it_would_have_won_on_spread()
-> Result<()> {
    // The premise, asserted rather than assumed: with both books usable the
    // tighter venue wins. Then the same tighter venue with no book at all,
    // and with a stale one, is passed over for the wider venue that can
    // actually be traded — the pick is among usable books, and a venue
    // whose book the cell cannot trust is never "best" whatever it quotes.
    let mut both = cell_with(
        &[FIRST_BY_ID, SECOND_BY_ID],
        vec![
            book(FIRST_BY_ID, "99", "101")?,
            book(SECOND_BY_ID, "99.5", "100.5")?,
        ],
    )?;
    let mut gateway = RecordingGateway::default();
    let report = one_pass(&mut both, &mut gateway, t(10))?;
    assert_eq!(
        report.orders[0].venue.as_str(),
        SECOND_BY_ID,
        "the premise failed: the tighter venue does not win when its book is usable"
    );

    // No book at all for the tighter venue.
    let mut absent = cell_with(
        &[FIRST_BY_ID, SECOND_BY_ID],
        vec![book(FIRST_BY_ID, "99", "101")?],
    )?;
    // `cell.work` rather than the refuse-nothing helper from here on: a
    // cell that chose the venue with no book refuses under `book` and sends
    // nothing, and that must fail the assertion about the order, not a
    // helper's premise.
    let mut gateway = RecordingGateway::default();
    let report = absent.work(t(10), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the usable venue was passed over: {:?}",
        report.refusals
    );
    assert_eq!(
        report.orders[0].venue.as_str(),
        FIRST_BY_ID,
        "a venue with no book was chosen"
    );
    assert_eq!(
        choices(&absent)[0].1,
        vec![(FIRST_BY_ID.to_string(), "2".to_string())],
        "a venue with no book was journaled as a candidate"
    );

    // A stale book for the tighter venue: the next usable venue is chosen.
    let mut stale_tight = book(SECOND_BY_ID, "99.5", "100.5")?;
    stale_tight.reset("a sequence gap was abandoned");
    let mut stale = cell_with(
        &[FIRST_BY_ID, SECOND_BY_ID],
        vec![book(FIRST_BY_ID, "99", "101")?, stale_tight],
    )?;
    let mut gateway = RecordingGateway::default();
    let report = stale.work(t(10), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the usable venue was passed over for a stale one: {:?}",
        report.refusals
    );
    assert_eq!(
        report.orders[0].venue.as_str(),
        FIRST_BY_ID,
        "a venue whose book is stale was chosen for its spread"
    );
    Ok(())
}

#[test]
fn a_cell_configured_with_the_reserved_venue_name_is_refused_at_assembly() {
    // ADR 0078, decision three. A cell that could be configured with the
    // consolidator's sentinel would have `venue_for` choose it and send an
    // order to a venue literally named "unspecified". Refused before the
    // cell exists, so the sentinel has nothing to mean here.
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue(FIRST_BY_ID))
        .with_venue(venue(UNSPECIFIED_VENUE));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let error = match Cell::new(config, features) {
        Ok(_) => panic!("a cell was assembled with the reserved venue name"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "invalid", "{}", error.message());
    assert!(
        error.message().contains(UNSPECIFIED_VENUE) && error.message().contains("reserved"),
        "the refusal does not name the venue and why it is refused: {}",
        error.message()
    );
}
