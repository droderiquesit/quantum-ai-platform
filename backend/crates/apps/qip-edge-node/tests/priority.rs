//! §29.2's priority allocation on the node's own seam: which resting order
//! the requote loop is offered the next message for.
//!
//! The failure this closes. Everything else in §29.2 decides *whether* a
//! message may be sent — the per-venue token bucket, the requote threshold,
//! and the widening the bucket's depletion imposes. Nothing decided *which*
//! order goes first, so the answer was the order the cell holds its open
//! orders in, which is the order their ids sort in. `Requoter::reprice`
//! considers orders one at a time and each consideration may spend the
//! venue's last message, so under a constrained budget that ordering *was*
//! the allocation: a session with one requote left spent it on whichever
//! instrument happened to be alphabetically first.
//!
//! [`Requoter::allocate`] is the function the loop iterates, driven here
//! against a real `Cell` holding a real book. Nothing here reaches a
//! deployed process: the requote loop runs only under
//! `QIP_VENUE_FEED=simulated`, and `execution_nodes = {}` in all four
//! environments.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_edge::cell::{Cell, CellConfig, OpenOrder};
use qip_edge_node::reprice::Requoter;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(symbol)
}

/// A two-sided book for `symbol`, built through the feed path.
fn book(symbol: &str, bid: &str, ask: &str) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(symbol), venue(), VenueStatus::Open);
    for (index, (side, price)) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(symbol),
            Origin::new(venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: Decimal::parse(price).expect("a decimal literal"),
                quantity: dec!("1000"),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

fn cell_holding(books: Vec<VenueState>) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    for state in books {
        cell.track(state);
    }
    Ok(cell)
}

/// A resting buy: it consumes the ask, so it rests on the bid and falls
/// behind when the bid rises above the price it was sent at.
fn resting_buy(order_id: &str, symbol: &str, quantity: &str, price: &str) -> OpenOrder {
    OpenOrder {
        order_id: order_id.to_string(),
        venue: venue(),
        object_id: object(symbol),
        side: BookSide::Ask,
        quantity: Decimal::parse(quantity).expect("a decimal literal"),
        price: Decimal::parse(price).expect("a decimal literal"),
        filled: Decimal::ZERO,
        simulated: true,
        sent_at: t(5),
        release_at: t(5),
        expires_at: Some(t(600)),
        closed: None,
    }
}

fn ids(ranked: &[OpenOrder]) -> Vec<&str> {
    ranked.iter().map(|order| order.order_id.as_str()).collect()
}

#[test]
fn the_last_message_is_offered_to_the_update_worth_most_and_not_to_the_lowest_order_id()
-> Result<()> {
    // One tick behind on a thousand lots recovers ten; ten ticks behind on
    // one lot recovers a tenth. In the ticks the repricer reasons in the
    // second order wins by ten to one, and that is exactly the ranking this
    // replaces.
    let cell = cell_holding(vec![
        book("alpha", "100.01", "100.03")?,
        book("zulu", "100.10", "100.12")?,
    ])?;
    let alpha = resting_buy("alpha-order", "alpha", "1", "100.00");
    let zulu = resting_buy("zulu-order", "zulu", "1000", "100.09");

    // Premise: the alphabetically first order is the one the old ordering
    // would have spent the message on, and it is the cheaper update.
    let open = vec![alpha.clone(), zulu.clone()];
    assert_eq!(
        ids(&open),
        vec!["alpha-order", "zulu-order"],
        "the premise failed: the fixture is not in the order the ids sort in"
    );
    assert!(
        alpha.remaining() < zulu.remaining(),
        "the premise failed: the alphabetically first order is not the smaller one"
    );

    let ranked = Requoter::allocate(&cell, &open);
    assert_eq!(
        ids(&ranked),
        vec!["zulu-order", "alpha-order"],
        "the message went to the cheaper update because its id sorted first"
    );
    Ok(())
}

#[test]
fn an_order_whose_book_prices_nothing_keeps_its_place_in_the_queue_rather_than_losing_its_turn()
-> Result<()> {
    // Load-bearing: the ranking is an ordering and never a gate. An order
    // the ranking cannot value must still reach `Repricer::consider` and the
    // budget, which are the things entitled to refuse it — and the loop is
    // what reports it as `Unmodelled` or holds it. A ranking that dropped it
    // would be a second refusal nobody wrote a reason for.
    let cell = cell_holding(vec![book("alpha", "100.01", "100.03")?])?;
    let priced = resting_buy("alpha-order", "alpha", "100", "100.00");
    // No book is tracked for this instrument at all, so there is no touch to
    // measure it against.
    let unpriced = resting_buy("omega-order", "omega", "100", "100.00");

    let ranked = Requoter::allocate(&cell, &[priced, unpriced]);
    assert_eq!(
        ranked.len(),
        2,
        "the ranking dropped an order it could not value"
    );
    assert_eq!(
        ids(&ranked),
        vec!["alpha-order", "omega-order"],
        "an order with no book outranked one the cell can price"
    );
    Ok(())
}

#[test]
fn an_order_the_cell_has_finished_with_is_not_offered_a_message_at_all() -> Result<()> {
    // The loop's own filter, moved ahead of the ranking: a closed order and
    // a marketable one that never rests have nothing to reprice, and ranking
    // them would put them ahead of orders that do.
    let cell = cell_holding(vec![book("alpha", "100.10", "100.12")?])?;
    let mut settled = resting_buy("aaa-settled", "alpha", "1000", "100.00");
    settled.closed = Some("filled".to_string());
    let mut marketable = resting_buy("bbb-marketable", "alpha", "1000", "100.00");
    marketable.expires_at = None;
    let resting = resting_buy("ccc-resting", "alpha", "1", "100.00");

    let ranked = Requoter::allocate(&cell, &[settled, marketable, resting]);
    assert_eq!(
        ids(&ranked),
        vec!["ccc-resting"],
        "an order the cell has finished with was offered a requote message"
    );
    Ok(())
}
