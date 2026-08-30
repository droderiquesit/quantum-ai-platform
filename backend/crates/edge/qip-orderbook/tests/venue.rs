//! Per-instrument venue state.
//!
//! The assertions that matter here are about what the state refuses to say. A
//! book discarded after a gap must not serve a price, a print that the venue
//! marked as not updating the last sale must not update it, and an auction must
//! be visible as an auction rather than as an unusually wide continuous market.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::{
    BookSide, MarketMessage, MessageBody, Origin, TradeCondition, VenueId, VenueStatus,
};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_orderbook::{BookCondition, BookView, VenueState};

fn px(value: &str) -> Decimal {
    Decimal::parse(value).expect("test price parses")
}

fn qty(units: i64) -> Decimal {
    Decimal::from_int(units)
}

fn venue() -> VenueId {
    VenueId::new("XNAS")
}

fn instrument() -> ObjectId {
    ObjectId::from_string("obj-aapl")
}

fn at(sequence: u64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_millis(sequence as i64))
}

fn message(sequence: u64, body: MessageBody) -> MarketMessage {
    MarketMessage::new(
        instrument(),
        Origin::new(venue(), "itch-a", 1, sequence),
        body,
        at(sequence),
        at(sequence),
    )
}

fn level(side: BookSide, price: &str, quantity: i64) -> MessageBody {
    MessageBody::LevelSet {
        side,
        price: px(price),
        quantity: qty(quantity),
        order_count: Some(1),
    }
}

fn two_sided() -> Result<VenueState> {
    let mut state = VenueState::aggregated(instrument(), venue(), VenueStatus::Open);
    state.apply(&message(1, level(BookSide::Bid, "100.00", 500)))?;
    state.apply(&message(2, level(BookSide::Ask, "100.10", 500)))?;
    Ok(state)
}

#[test]
fn a_reset_discards_the_book_and_withholds_every_price_until_resynchronisation() -> Result<()> {
    let mut state = two_sided()?;
    assert_eq!(state.mid(), Some(px("100.05")));

    state.apply(&message(
        3,
        MessageBody::Reset {
            reason: "sequence gap 4102 -> 4109".into(),
        },
    ))?;

    assert!(state.is_stale());
    assert_eq!(state.reset_reason(), Some("sequence gap 4102 -> 4109"));
    assert!(state.book().is_empty());
    assert!(state.mid().is_none());
    assert!(state.microprice().is_none());
    assert!(state.spread().is_none());
    assert!(state.best_bid().is_none());
    assert!(state.sweep_cost(BookSide::Ask, qty(10)).is_none());
    assert!(!state.continuous_trading());
    assert!(!state.prices_are_usable());

    // Rebuilding the book is not enough on its own: nothing about an ordinary
    // update proves the book is whole again.
    state.apply(&message(4, level(BookSide::Bid, "100.00", 500)))?;
    state.apply(&message(5, level(BookSide::Ask, "100.10", 500)))?;
    assert!(state.is_stale());
    assert!(state.mid().is_none());

    state.resynchronised(at(6));
    assert!(!state.is_stale());
    assert_eq!(state.reset_reason(), None);
    assert_eq!(state.mid(), Some(px("100.05")));
    assert!(state.continuous_trading());
    Ok(())
}

#[test]
fn a_book_known_to_be_wrong_is_distinguishable_from_one_that_is_merely_empty() -> Result<()> {
    let fresh = VenueState::aggregated(instrument(), venue(), VenueStatus::Open);
    let mut broken = two_sided()?;
    broken.reset("gap");

    // The levels are identical; the states are not, and that is the whole point.
    assert_eq!(fresh.book().snapshot(), broken.book().snapshot());
    assert!(!fresh.is_stale());
    assert!(broken.is_stale());
    assert_ne!(fresh.snapshot(), broken.snapshot());
    assert_ne!(fresh.snapshot().digest(), broken.snapshot().digest());
    Ok(())
}

#[test]
fn only_prints_the_venue_marks_as_a_last_sale_update_the_last_sale() -> Result<()> {
    let mut state = two_sided()?;

    let trade = |price: &str, quantity: i64, condition: TradeCondition| MessageBody::Trade {
        price: px(price),
        quantity: qty(quantity),
        condition,
        aggressor: Some(BookSide::Ask),
    };

    state.apply(&message(3, trade("100.05", 100, TradeCondition::Regular)))?;
    assert_eq!(state.last_trade().map(|t| t.price), Some(px("100.05")));

    // An odd lot prints and counts toward volume, but must not drag the last
    // sale — this is exactly how a mid gets pulled by a one-share cross.
    state.apply(&message(4, trade("99.00", 1, TradeCondition::OddLot)))?;
    assert_eq!(state.last_trade().map(|t| t.price), Some(px("100.05")));
    assert_eq!(state.session_volume(), qty(101));

    // A correction updates neither.
    state.apply(&message(5, trade("90.00", 50, TradeCondition::Correction)))?;
    assert_eq!(state.last_trade().map(|t| t.price), Some(px("100.05")));
    assert_eq!(state.session_volume(), qty(101));
    assert_eq!(state.trade_count(), 2);

    // An auction print is a real last sale.
    state.apply(&message(6, trade("100.20", 5000, TradeCondition::Auction)))?;
    assert_eq!(state.last_trade().map(|t| t.price), Some(px("100.20")));
    assert_eq!(state.last_trade().map(|t| t.at), Some(at(6)));
    assert_eq!(state.session_volume(), qty(5101));
    Ok(())
}

#[test]
fn session_volume_and_vwap_are_taken_over_the_prints_that_count() -> Result<()> {
    let mut state = two_sided()?;
    for (sequence, price, size) in [(3u64, "100.00", 100i64), (4, "101.00", 300)] {
        state.apply(&message(
            sequence,
            MessageBody::Trade {
                price: px(price),
                quantity: qty(size),
                condition: TradeCondition::Regular,
                aggressor: None,
            },
        ))?;
    }
    assert_eq!(state.session_volume(), qty(400));
    assert_eq!(state.session_notional(), px("40300"));
    assert_eq!(state.session_vwap(), Some(px("100.75")));

    state.roll_session(at(5));
    assert_eq!(state.session_volume(), Decimal::ZERO);
    assert_eq!(state.session_vwap(), None);
    // A new session does not un-print the last trade.
    assert!(state.last_trade().is_some());
    Ok(())
}

#[test]
fn an_auction_is_visible_and_suspends_continuous_trading() -> Result<()> {
    let mut state = two_sided()?;
    assert!(state.continuous_trading());

    state.apply(&message(
        3,
        MessageBody::StatusChange {
            status: VenueStatus::Auction,
        },
    ))?;
    state.apply(&message(
        4,
        MessageBody::AuctionUpdate {
            indicative_price: Some(px("100.25")),
            paired: qty(40_000),
            imbalance: qty(10_000),
            imbalance_side: Some(BookSide::Bid),
        },
    ))?;

    assert_eq!(state.status(), VenueStatus::Auction);
    assert!(!state.continuous_trading());
    // Prices are still usable during an auction — they are just not the
    // continuous market's prices.
    assert!(state.prices_are_usable());
    assert!(state.status().accepts_orders());

    let auction = state.auction().copied().expect("an auction is running");
    assert_eq!(auction.indicative_price, Some(px("100.25")));
    assert_eq!(auction.paired, qty(40_000));
    assert_eq!(auction.signed_imbalance(), qty(10_000));
    assert_eq!(auction.total_interest(), qty(50_000));
    assert!(auction.imbalance_ratio() > 0.19 && auction.imbalance_ratio() < 0.21);
    assert!(auction.is_indicative());
    assert_eq!(auction.at, at(4));

    // Once the venue reopens, the indicative price must not linger: it
    // describes an uncross that has already happened.
    state.apply(&message(
        5,
        MessageBody::StatusChange {
            status: VenueStatus::Open,
        },
    ))?;
    assert!(state.auction().is_none());
    assert!(state.continuous_trading());
    Ok(())
}

#[test]
fn a_halted_or_closed_venue_serves_no_price_but_keeps_its_book() -> Result<()> {
    let mut state = two_sided()?;
    state.apply(&message(
        3,
        MessageBody::StatusChange {
            status: VenueStatus::Halted,
        },
    ))?;

    assert!(!state.prices_are_usable());
    assert!(state.mid().is_none());
    assert!(!state.continuous_trading());
    // The book itself is intact and still inspectable — a halt is not a gap.
    assert!(!state.is_stale());
    assert_eq!(state.book().mid(), Some(px("100.05")));
    assert_eq!(state.book().level_count(BookSide::Bid), 1);
    Ok(())
}

#[test]
fn a_crossed_book_is_reported_by_the_venue_state_even_when_it_cannot_be_traded() -> Result<()> {
    let mut state = two_sided()?;
    state.apply(&message(3, level(BookSide::Bid, "100.20", 100)))?;

    assert_eq!(state.condition(), BookCondition::Crossed);
    assert_eq!(state.book().crossed_by(), Some(px("0.10")));
    // The condition is reported whatever the status, because a crossed book is
    // most worth knowing about when it cannot be traded on.
    state.apply(&message(
        4,
        MessageBody::StatusChange {
            status: VenueStatus::Halted,
        },
    ))?;
    assert_eq!(state.condition(), BookCondition::Crossed);
    assert!(state.mid().is_none());
    Ok(())
}

#[test]
fn a_message_from_another_venue_is_refused_and_changes_nothing() -> Result<()> {
    let mut state = two_sided()?;
    let before = state.snapshot();

    let foreign = MarketMessage::new(
        instrument(),
        Origin::new(VenueId::new("XNYS"), "itch-a", 1, 3),
        level(BookSide::Bid, "500.00", 100),
        at(3),
        at(3),
    );
    let refusal = state
        .apply(&foreign)
        .expect_err("a message from another venue must be refused");

    assert_eq!(refusal.code(), "invalid");
    assert_eq!(state.snapshot(), before);
    assert_eq!(state.applied(), 2);
    Ok(())
}

#[test]
fn a_refused_update_does_not_advance_the_states_bookkeeping() -> Result<()> {
    let mut state = VenueState::order_by_order(instrument(), venue(), VenueStatus::Open);
    state.apply(&message(
        1,
        MessageBody::OrderAdded {
            order_ref: 1,
            side: BookSide::Bid,
            price: px("100.00"),
            quantity: qty(100),
        },
    ))?;
    let before = state.snapshot();

    // Aggregated depth on an order-by-order feed is a misconfiguration, not a
    // message to absorb quietly.
    let refusal = state
        .apply(&message(2, level(BookSide::Bid, "100.00", 900)))
        .expect_err("an order-by-order state must refuse aggregated depth");
    assert_eq!(refusal.code(), "invalid");

    assert_eq!(state.snapshot(), before);
    assert_eq!(state.applied(), 1);
    assert_eq!(state.last_sequence(), Some(1));
    assert_eq!(state.last_update(), Some(at(1)));
    Ok(())
}
