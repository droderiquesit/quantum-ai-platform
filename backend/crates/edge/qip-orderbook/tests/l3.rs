//! Order-by-order book behaviour.
//!
//! Almost everything here is about one thing: time priority, and what a replace
//! does to it. A book that loses track of queue order still produces a
//! plausible touch and a plausible depth profile, so nothing downstream
//! notices — it just makes every passive fill estimate optimistic. These tests
//! are the only place that mistake is visible.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::{BookSide, MessageBody};
use qip_core::error::Result;
use qip_core::{Decimal, dec};
use qip_orderbook::{BookView, L3Book};

fn px(value: &str) -> Decimal {
    Decimal::parse(value).expect("test price parses")
}

fn qty(units: i64) -> Decimal {
    Decimal::from_int(units)
}

fn added(order_ref: u64, side: BookSide, price: &str, quantity: i64) -> MessageBody {
    MessageBody::OrderAdded {
        order_ref,
        side,
        price: px(price),
        quantity: qty(quantity),
    }
}

fn replaced(order_ref: u64, price: &str, quantity: i64) -> MessageBody {
    MessageBody::OrderReplaced {
        order_ref,
        price: px(price),
        quantity: qty(quantity),
    }
}

/// Three orders resting at one price, added in reference order.
fn queued_level() -> Result<L3Book> {
    let mut book = L3Book::new();
    book.apply(&added(1, BookSide::Bid, "100.00", 100))?;
    book.apply(&added(2, BookSide::Bid, "100.00", 200))?;
    book.apply(&added(3, BookSide::Bid, "100.00", 300))?;
    Ok(book)
}

#[test]
fn an_order_added_later_at_the_same_price_rests_behind_one_added_earlier() -> Result<()> {
    let book = queued_level()?;

    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![1, 2, 3]);
    assert_eq!(book.size_ahead_of(1)?, Decimal::ZERO);
    assert_eq!(book.size_ahead_of(2)?, qty(100));
    assert_eq!(book.size_ahead_of(3)?, qty(300));

    let third = book.queue_position(3)?;
    assert_eq!(third.orders_ahead, 2);
    assert_eq!(third.behind, Decimal::ZERO);
    assert_eq!(third.level_size, qty(600));
    assert!(!third.is_at_front());
    assert!(book.queue_position(1)?.is_at_front());
    Ok(())
}

#[test]
fn a_replace_that_raises_size_goes_to_the_back_of_the_queue() -> Result<()> {
    let mut book = queued_level()?;
    book.apply(&replaced(1, "100.00", 150))?;

    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![2, 3, 1]);
    // The size that was in front of it is now behind: 500 of the other two.
    assert_eq!(book.size_ahead_of(1)?, qty(500));
    assert_eq!(book.size_at(BookSide::Bid, px("100.00")), qty(650));
    Ok(())
}

#[test]
fn a_replace_that_lowers_size_keeps_its_place_in_the_queue() -> Result<()> {
    let mut book = queued_level()?;
    book.apply(&replaced(2, "100.00", 50))?;

    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![1, 2, 3]);
    assert_eq!(book.size_ahead_of(2)?, qty(100));
    assert_eq!(book.size_at(BookSide::Bid, px("100.00")), qty(450));

    // An unchanged size is not an increase, so it keeps its place too.
    book.apply(&replaced(2, "100.00", 50))?;
    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![1, 2, 3]);
    Ok(())
}

#[test]
fn a_replace_that_moves_the_price_joins_the_back_of_its_new_level() -> Result<()> {
    let mut book = queued_level()?;
    book.apply(&added(4, BookSide::Bid, "99.99", 400))?;
    // Even a move to a better price starts at the back of that price's queue.
    book.apply(&replaced(4, "100.00", 400))?;

    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![1, 2, 3, 4]);
    assert!(book.queue_at(BookSide::Bid, px("99.99")).is_empty());
    assert_eq!(book.level_count(BookSide::Bid), 1);
    assert_eq!(book.size_ahead_of(4)?, qty(600));
    Ok(())
}

#[test]
fn a_reduction_keeps_time_priority_and_a_reduction_to_zero_removes_the_order() -> Result<()> {
    let mut book = queued_level()?;
    book.apply(&MessageBody::OrderReduced {
        order_ref: 1,
        remaining: qty(40),
    })?;

    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![1, 2, 3]);
    assert_eq!(book.size_ahead_of(2)?, qty(40));
    assert_eq!(book.size_at(BookSide::Bid, px("100.00")), qty(540));

    book.apply(&MessageBody::OrderReduced {
        order_ref: 1,
        remaining: Decimal::ZERO,
    })?;
    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![2, 3]);
    assert!(book.order(1).is_none());
    assert_eq!(book.size_at(BookSide::Bid, px("100.00")), qty(500));
    Ok(())
}

#[test]
fn queue_position_falls_monotonically_as_orders_ahead_leave() -> Result<()> {
    let mut book = L3Book::new();
    for order_ref in 1..=5u64 {
        book.apply(&added(order_ref, BookSide::Ask, "50.25", 100))?;
    }
    let watched = 5;
    let mut ahead = book.size_ahead_of(watched)?;
    assert_eq!(ahead, qty(400));

    // A partial execution of the front order, a cancellation, then the rest.
    let events = [
        MessageBody::OrderReduced {
            order_ref: 1,
            remaining: qty(30),
        },
        MessageBody::OrderRemoved { order_ref: 1 },
        MessageBody::OrderReduced {
            order_ref: 2,
            remaining: qty(10),
        },
        MessageBody::OrderRemoved { order_ref: 3 },
        MessageBody::OrderRemoved { order_ref: 4 },
        MessageBody::OrderRemoved { order_ref: 2 },
    ];
    for event in &events {
        book.apply(event)?;
        let now = book.size_ahead_of(watched)?;
        assert!(
            now <= ahead,
            "queue position rose from {ahead} to {now} after {}",
            event.kind()
        );
        ahead = now;
    }

    let position = book.queue_position(watched)?;
    assert!(position.is_at_front());
    assert_eq!(position.ahead, Decimal::ZERO);
    assert!(qip_core::testing::approx_eq(
        position.queue_fraction(),
        0.0,
        1e-12
    ));
    Ok(())
}

#[test]
fn removing_every_order_returns_the_book_to_its_initial_state() -> Result<()> {
    let fresh = L3Book::new();
    let mut book = L3Book::new();
    let refs: Vec<u64> = (1..=20).collect();
    for order_ref in &refs {
        let side = if order_ref % 2 == 0 {
            BookSide::Bid
        } else {
            BookSide::Ask
        };
        let price = format!("{}.{:02}", 100 + order_ref % 3, order_ref);
        book.apply(&added(*order_ref, side, &price, 10 * *order_ref as i64))?;
    }
    // A reduction leaves a partially filled order behind; removing it must
    // still leave nothing at all.
    book.apply(&MessageBody::OrderReduced {
        order_ref: 4,
        remaining: qty(1),
    })?;
    assert!(!book.is_empty());

    for order_ref in &refs {
        book.apply(&MessageBody::OrderRemoved {
            order_ref: *order_ref,
        })?;
    }

    assert_eq!(book, fresh, "a fully drained book must equal a fresh one");
    assert_eq!(book.snapshot(), fresh.snapshot());
    assert_eq!(book.level_count(BookSide::Bid), 0);
    assert_eq!(book.level_count(BookSide::Ask), 0);
    assert_eq!(book.resting_orders(), 0);
    assert_eq!(book.total_size(BookSide::Bid), Decimal::ZERO);
    assert!(book.mid().is_none());
    Ok(())
}

#[test]
fn an_aggregated_update_is_refused_rather_than_silently_dropped() -> Result<()> {
    let mut book = queued_level()?;
    let refusal = book
        .apply(&MessageBody::LevelSet {
            side: BookSide::Bid,
            price: dec!("100.00"),
            quantity: dec!("900"),
            order_count: Some(4),
        })
        .expect_err("an L3 book must refuse aggregated depth");

    assert_eq!(refusal.code(), "invalid");
    // The refusal must leave the book untouched, not half-applied.
    assert_eq!(book.size_at(BookSide::Bid, px("100.00")), qty(600));
    Ok(())
}

#[test]
fn malformed_order_updates_are_refused_with_the_reason_they_are_malformed() -> Result<()> {
    let mut book = queued_level()?;

    let duplicate = book
        .apply(&added(1, BookSide::Bid, "100.00", 10))
        .expect_err("a reused order reference must be refused");
    assert_eq!(duplicate.code(), "invalid");
    assert!(duplicate.message().contains("already resting"));

    let increase = book
        .apply(&MessageBody::OrderReduced {
            order_ref: 1,
            remaining: qty(500),
        })
        .expect_err("a reduction that raises the quantity must be refused");
    assert!(increase.message().contains("increase"));

    let unknown = book
        .apply(&MessageBody::OrderRemoved { order_ref: 999 })
        .expect_err("removing an unknown order must be refused");
    assert_eq!(unknown.code(), "not_found");

    let empty = book
        .apply(&added(9, BookSide::Bid, "100.00", 0))
        .expect_err("a zero-quantity add must be refused");
    assert_eq!(empty.code(), "invalid");

    // None of the refusals may have disturbed the book.
    assert_eq!(book.queue_at(BookSide::Bid, px("100.00")), vec![1, 2, 3]);
    assert_eq!(book.size_at(BookSide::Bid, px("100.00")), qty(600));
    Ok(())
}

#[test]
fn a_queue_position_is_only_available_for_an_order_the_book_holds() -> Result<()> {
    let book = queued_level()?;
    let missing = book
        .queue_position(42)
        .expect_err("an unknown reference has no queue position");
    assert_eq!(missing.code(), "not_found");
    Ok(())
}
