//! The read surface both book flavours share.
//!
//! The properties asserted here are the ones a strategy assumes without ever
//! checking: that a sweep cannot invent liquidity, that the microprice lies
//! between the quotes, and that a book which is inconsistent says so instead of
//! serving a number that looks fine.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::{BookSide, MessageBody};
use qip_core::error::Result;
use qip_core::rng::Rng;
use qip_core::testing::{Property, approx_eq};
use qip_core::{Decimal, dec};
use qip_orderbook::{Book, BookCondition, BookView, L2Book, L3Book, Level};

fn px(value: &str) -> Decimal {
    Decimal::parse(value).expect("test price parses")
}

fn qty(units: i64) -> Decimal {
    Decimal::from_int(units)
}

fn level(side: BookSide, price: &str, quantity: i64, orders: u32) -> MessageBody {
    MessageBody::LevelSet {
        side,
        price: px(price),
        quantity: qty(quantity),
        order_count: Some(orders),
    }
}

/// A five-deep two-sided aggregated book.
fn aggregated() -> Result<L2Book> {
    let mut book = L2Book::new();
    for (offset, size) in [(0, 100), (1, 200), (2, 300), (3, 400), (4, 500)] {
        book.apply(&level(
            BookSide::Bid,
            &format!("99.{:02}", 96 - offset),
            size,
            2,
        ))?;
        book.apply(&level(
            BookSide::Ask,
            &format!("100.{:02}", 4 + offset),
            size,
            3,
        ))?;
    }
    Ok(book)
}

/// The same shape, built order by order.
fn order_by_order() -> Result<L3Book> {
    let mut book = L3Book::new();
    let mut order_ref = 0u64;
    for (offset, size) in [(0, 100), (1, 200), (2, 300), (3, 400), (4, 500)] {
        for side in [BookSide::Bid, BookSide::Ask] {
            order_ref += 1;
            let price = match side {
                BookSide::Bid => format!("99.{:02}", 96 - offset),
                BookSide::Ask => format!("100.{:02}", 4 + offset),
            };
            book.apply(&MessageBody::OrderAdded {
                order_ref,
                side,
                price: px(&price),
                quantity: qty(size),
            })?;
        }
    }
    Ok(book)
}

#[test]
fn the_touch_is_the_first_level_of_each_side_however_the_book_was_built() -> Result<()> {
    let l2 = aggregated()?;
    let l3 = order_by_order()?;

    for book in [&l2.snapshot(), &l3.snapshot()] {
        assert_eq!(book.bids[0].price, px("99.96"));
        assert_eq!(book.asks[0].price, px("100.04"));
        // Levels come back in book order, dearest bid and cheapest ask first.
        assert!(book.bids.windows(2).all(|w| w[0].price > w[1].price));
        assert!(book.asks.windows(2).all(|w| w[0].price < w[1].price));
    }

    assert_eq!(l2.spread(), Some(px("0.08")));
    assert_eq!(l2.mid(), Some(px("100.00")));
    assert_eq!(l3.mid(), l2.mid());
    assert_eq!(l3.total_size(BookSide::Bid), l2.total_size(BookSide::Bid));
    assert_eq!(l2.levels(BookSide::Ask, 2).len(), 2);
    assert_eq!(l2.size_at(BookSide::Ask, px("100.06")), qty(300));
    assert_eq!(l2.size_at(BookSide::Ask, px("100.09")), Decimal::ZERO);
    Ok(())
}

#[test]
fn depth_to_a_price_counts_every_level_at_least_as_good_as_it() -> Result<()> {
    let book = aggregated()?;

    assert_eq!(book.depth_to(BookSide::Ask, px("100.04")), qty(100));
    assert_eq!(book.depth_to(BookSide::Ask, px("100.06")), qty(600));
    // A price between levels includes everything strictly better than it.
    assert_eq!(book.depth_to(BookSide::Ask, px("100.055")), qty(300));
    assert_eq!(book.depth_to(BookSide::Ask, px("999")), qty(1500));
    assert_eq!(book.depth_to(BookSide::Ask, px("1")), Decimal::ZERO);

    // The bid side counts downward, which is the comparison most often got
    // backwards.
    assert_eq!(book.depth_to(BookSide::Bid, px("99.96")), qty(100));
    assert_eq!(book.depth_to(BookSide::Bid, px("99.94")), qty(600));
    assert_eq!(book.depth_to(BookSide::Bid, px("0")), qty(1500));
    Ok(())
}

#[test]
fn a_sweep_of_the_whole_book_matches_the_levels_summed_by_hand() -> Result<()> {
    let book = aggregated()?;
    let levels = book.levels(BookSide::Ask, usize::MAX);

    let expected_size: Decimal = levels.iter().map(|l| l.size).sum();
    let expected_notional: Decimal = levels.iter().map(|l| l.price * l.size).sum();

    let sweep = book.sweep_cost(BookSide::Ask, expected_size);
    assert_eq!(sweep.filled, expected_size);
    assert_eq!(sweep.notional, expected_notional);
    assert_eq!(
        sweep.average_price(),
        expected_notional.checked_div(expected_size)
    );
    assert_eq!(sweep.levels_consumed, levels.len());
    assert_eq!(sweep.worst_price, Some(px("100.08")));
    assert!(sweep.is_complete());
    assert_eq!(sweep.shortfall(), Decimal::ZERO);

    // The order-by-order book with the same shape must cost the same.
    let l3 = order_by_order()?;
    let same = l3.sweep_cost(BookSide::Ask, expected_size);
    assert_eq!(same.notional, sweep.notional);
    assert_eq!(same.filled, sweep.filled);
    Ok(())
}

#[test]
fn a_sweep_never_claims_more_liquidity_than_the_levels_hold() -> Result<()> {
    let book = aggregated()?;
    let available = book.total_size(BookSide::Ask);

    // Asking for twice the book returns the book, and says so.
    let greedy = book.sweep_cost(BookSide::Ask, available * qty(2));
    assert_eq!(greedy.filled, available);
    assert!(!greedy.is_complete());
    assert_eq!(greedy.shortfall(), available);

    // An empty book fills nothing and refuses to name a price for it.
    let empty = Book::aggregated();
    let nothing = empty.sweep_cost(BookSide::Bid, qty(10));
    assert_eq!(nothing.filled, Decimal::ZERO);
    assert!(nothing.average_price().is_none());
    assert!(nothing.worst_price.is_none());
    assert!(!nothing.is_complete());

    Property::new("a sweep is bounded by the book")
        .cases(400)
        .for_all(
            |rng| {
                let side = if rng.bernoulli(0.5) {
                    BookSide::Bid
                } else {
                    BookSide::Ask
                };
                (
                    side,
                    Decimal::from_raw(rng.below(4_000_000_000_000) as i128),
                )
            },
            |(side, wanted)| {
                let sweep = book.sweep_cost(*side, *wanted);
                let resting = book.total_size(*side);
                if sweep.filled > resting {
                    return Err(format!("filled {} of {resting} resting", sweep.filled));
                }
                if sweep.filled > *wanted {
                    return Err(format!("filled {} for a request of {wanted}", sweep.filled));
                }
                if sweep.levels_consumed > book.level_count(*side) {
                    return Err(format!("walked {} levels", sweep.levels_consumed));
                }
                // Whatever filled must have cost at least the touch price and
                // at most the worst price it reached.
                match (sweep.average_price(), sweep.worst_price) {
                    (Some(average), Some(worst)) => {
                        let touch = match side {
                            BookSide::Bid => book.best_bid(),
                            BookSide::Ask => book.best_ask(),
                        };
                        let touch = touch.map(|l| l.price).unwrap_or_default();
                        let (lo, hi) = (touch.min(worst), touch.max(worst));
                        if average < lo || average > hi {
                            return Err(format!("average {average} outside [{lo}, {hi}]"));
                        }
                        Ok(())
                    }
                    (None, None) => Ok(()),
                    (average, worst) => {
                        Err(format!("half-priced sweep: {average:?} at worst {worst:?}"))
                    }
                }
            },
        );
    Ok(())
}

#[test]
fn a_partial_sweep_reports_the_shortfall_rather_than_a_flattering_price() -> Result<()> {
    let book = aggregated()?;
    // The whole book is 1,500; ask for 1,000 and then for 2,000.
    let inside = book.sweep_cost(BookSide::Ask, qty(1000));
    let beyond = book.sweep_cost(BookSide::Ask, qty(2000));

    assert!(inside.is_complete());
    assert!(!beyond.is_complete());
    assert_eq!(beyond.shortfall(), qty(500));
    // The deeper sweep must be worse, never better, than the shallower one.
    let (Some(near), Some(far)) = (inside.average_price(), beyond.average_price()) else {
        panic!("both sweeps fill something");
    };
    assert!(far >= near, "a deeper sweep priced better: {far} vs {near}");

    // Slippage is signed so that positive is always worse for the taker.
    let mid = book.mid().expect("a two-sided book has a mid");
    let buy = book.sweep_cost(BookSide::Ask, qty(1000));
    let sell = book.sweep_cost(BookSide::Bid, qty(1000));
    assert!(buy.slippage_bps(mid).unwrap_or_default() > 0.0);
    assert!(sell.slippage_bps(mid).unwrap_or_default() > 0.0);
    assert!(buy.slippage_bps(Decimal::ZERO).is_none());
    Ok(())
}

#[test]
fn the_microprice_lies_between_the_quotes_and_equals_the_mid_when_sizes_match() -> Result<()> {
    let mut balanced = L2Book::new();
    balanced.apply(&level(BookSide::Bid, "100.00", 500, 1))?;
    balanced.apply(&level(BookSide::Ask, "100.10", 500, 1))?;
    assert_eq!(balanced.mid(), Some(px("100.05")));
    assert_eq!(balanced.microprice(), balanced.mid());

    let mut lopsided = L2Book::new();
    lopsided.apply(&level(BookSide::Bid, "100.00", 5000, 1))?;
    lopsided.apply(&level(BookSide::Ask, "100.10", 100, 1))?;
    let micro = lopsided.microprice().expect("a two-sided book has one");
    assert!(micro > px("100.05"), "weight of bids must lift it: {micro}");
    assert!(micro < px("100.10"));

    Property::new("the microprice sits between the quotes")
        .cases(300)
        .for_all(
            |rng| {
                (
                    1 + rng.below(100_000) as i64,
                    1 + rng.below(100_000) as i64,
                    1 + rng.below(500) as i64,
                )
            },
            |(bid_size, ask_size, ticks)| {
                let mut book = L2Book::new();
                let bid = px("100.00");
                let ask = bid + Decimal::from_raw(i128::from(*ticks) * 10_000_000);
                book.set_level(BookSide::Bid, bid, qty(*bid_size), Some(1))
                    .map_err(|e| e.to_string())?;
                book.set_level(BookSide::Ask, ask, qty(*ask_size), Some(1))
                    .map_err(|e| e.to_string())?;
                let Some(micro) = book.microprice() else {
                    return Err("a two-sided book gave no microprice".to_string());
                };
                if micro < bid || micro > ask {
                    return Err(format!("microprice {micro} outside [{bid}, {ask}]"));
                }
                if bid_size == ask_size && Some(micro) != book.mid() {
                    return Err(format!("equal sizes gave {micro}, not the mid"));
                }
                Ok(())
            },
        );
    Ok(())
}

#[test]
fn a_crossed_book_is_reported_and_its_derived_prices_are_withheld() -> Result<()> {
    let mut book = L2Book::new();
    book.apply(&level(BookSide::Bid, "100.20", 100, 1))?;
    book.apply(&level(BookSide::Ask, "100.10", 100, 1))?;

    assert_eq!(book.condition(), BookCondition::Crossed);
    assert!(book.is_crossed());
    assert!(!book.condition().is_consistent());
    assert_eq!(book.crossed_by(), Some(px("0.10")));

    // The levels are still there — the book normalises nothing.
    assert_eq!(book.best_bid().map(|l| l.price), Some(px("100.20")));
    assert_eq!(book.best_ask().map(|l| l.price), Some(px("100.10")));

    // What a strategy would size against is withheld, so a negative spread
    // cannot reach a width filter as the tightest market of the day.
    assert!(book.spread().is_none());
    assert!(book.spread_bps().is_none());
    assert!(book.mid().is_none());
    assert!(book.microprice().is_none());
    Ok(())
}

#[test]
fn a_locked_book_is_distinguished_from_a_crossed_one() -> Result<()> {
    let mut book = L2Book::new();
    book.apply(&level(BookSide::Bid, "100.10", 100, 1))?;
    book.apply(&level(BookSide::Ask, "100.10", 100, 1))?;

    assert_eq!(book.condition(), BookCondition::Locked);
    assert!(book.is_locked());
    assert!(!book.is_crossed());
    // Locked is unusual, not wrong: there is a real price, and it is the lock.
    assert!(book.condition().is_consistent());
    assert_eq!(book.spread(), Some(Decimal::ZERO));
    assert_eq!(book.mid(), Some(px("100.10")));
    assert_eq!(book.crossed_by(), None);
    Ok(())
}

#[test]
fn a_one_sided_or_empty_book_serves_no_two_sided_price() -> Result<()> {
    let empty = L2Book::new();
    assert_eq!(empty.condition(), BookCondition::Empty);
    assert!(empty.is_empty());
    assert!(empty.mid().is_none());

    let mut one_sided = L2Book::new();
    one_sided.apply(&level(BookSide::Bid, "100.00", 100, 1))?;
    assert_eq!(one_sided.condition(), BookCondition::OneSided);
    assert!(!one_sided.condition().has_two_sides());
    assert!(one_sided.mid().is_none());
    assert!(one_sided.microprice().is_none());
    assert_eq!(one_sided.best_bid().map(|l| l.size), Some(qty(100)));
    Ok(())
}

#[test]
fn an_aggregated_book_takes_levels_in_any_order_and_a_zero_size_removes_one() -> Result<()> {
    let mut book = L2Book::new();
    // A sparse feed: levels arrive out of order, with gaps between them.
    book.apply(&level(BookSide::Bid, "99.50", 300, 4))?;
    book.apply(&level(BookSide::Bid, "99.90", 100, 1))?;
    book.apply(&level(BookSide::Bid, "99.70", 200, 2))?;
    assert_eq!(
        book.levels(BookSide::Bid, 9)
            .iter()
            .map(|l| l.price)
            .collect::<Vec<_>>(),
        vec![px("99.90"), px("99.70"), px("99.50")]
    );
    assert_eq!(book.best_bid().map(|l| l.order_count), Some(1));

    book.apply(&level(BookSide::Bid, "99.70", 0, 0))?;
    assert_eq!(book.level_count(BookSide::Bid), 2);
    assert_eq!(book.size_at(BookSide::Bid, px("99.70")), Decimal::ZERO);

    // Deleting a level that was never published is not an error; venues repeat
    // deletes and a feed handler must not fall over on one.
    book.apply(&level(BookSide::Bid, "12.34", 0, 0))?;
    assert_eq!(book.level_count(BookSide::Bid), 2);

    let refusal = book
        .apply(&MessageBody::OrderAdded {
            order_ref: 1,
            side: BookSide::Bid,
            price: dec!("99.90"),
            quantity: dec!("10"),
        })
        .expect_err("an aggregated book must refuse order-by-order depth");
    assert_eq!(refusal.code(), "invalid");
    Ok(())
}

#[test]
fn a_quote_replaces_the_touch_and_drops_anything_resting_in_front_of_it() -> Result<()> {
    let mut book = aggregated()?;
    book.apply(&MessageBody::Quote {
        bid: Some((px("99.94"), qty(50))),
        ask: Some((px("100.06"), qty(60))),
    })?;

    // Everything more aggressive than the quote was stale by construction.
    assert_eq!(book.best_bid(), Some(Level::new(px("99.94"), qty(50), 0)));
    assert_eq!(book.best_ask(), Some(Level::new(px("100.06"), qty(60), 0)));
    assert_eq!(book.size_at(BookSide::Bid, px("99.96")), Decimal::ZERO);
    assert_eq!(book.size_at(BookSide::Ask, px("100.04")), Decimal::ZERO);
    // Depth behind the quote is untouched: a quote says nothing about it.
    assert_eq!(book.size_at(BookSide::Bid, px("99.92")), qty(500));
    assert_eq!(book.size_at(BookSide::Ask, px("100.08")), qty(500));

    book.apply(&MessageBody::Quote {
        bid: None,
        ask: Some((px("100.06"), qty(60))),
    })?;
    assert_eq!(book.level_count(BookSide::Bid), 0);
    assert_eq!(book.condition(), BookCondition::OneSided);
    Ok(())
}

#[test]
fn the_unified_book_answers_for_either_resolution_and_says_what_it_cannot() -> Result<()> {
    let mut aggregated = Book::aggregated();
    aggregated.apply(&level(BookSide::Bid, "100.00", 100, 2))?;
    aggregated.apply(&level(BookSide::Ask, "100.10", 100, 2))?;

    let mut order_by_order = Book::order_by_order();
    order_by_order.apply(&MessageBody::OrderAdded {
        order_ref: 7,
        side: BookSide::Bid,
        price: px("100.00"),
        quantity: qty(100),
    })?;
    order_by_order.apply(&MessageBody::OrderAdded {
        order_ref: 8,
        side: BookSide::Ask,
        price: px("100.10"),
        quantity: qty(100),
    })?;

    assert_eq!(aggregated.mid(), order_by_order.mid());
    assert!(!aggregated.kind().tracks_orders());
    assert!(order_by_order.kind().tracks_orders());
    assert_eq!(order_by_order.resting_orders(), 2);
    assert_eq!(aggregated.resting_orders(), 0);

    assert_eq!(order_by_order.queue_position(7)?.ahead, Decimal::ZERO);
    let refusal = aggregated
        .queue_position(7)
        .expect_err("an aggregated book cannot answer a queue position");
    assert_eq!(refusal.code(), "unavailable");

    assert!(aggregated.as_aggregated().is_some());
    assert!(aggregated.as_order_by_order().is_none());
    assert!(order_by_order.as_order_by_order().is_some());
    Ok(())
}

#[test]
fn the_spread_in_basis_points_is_the_spread_over_the_mid() -> Result<()> {
    let book = aggregated()?;
    // 0.08 wide on a 100.00 mid is eight basis points.
    let bps = book.spread_bps().expect("a two-sided book has a spread");
    assert!(approx_eq(bps, 8.0, 1e-9), "spread was {bps}bp");
    Ok(())
}
