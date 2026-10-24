//! The matching engine, tested as a set of laws rather than as a set of cases.
//!
//! Three claims carry this file, and each is checked over generated books
//! rather than over one hand-picked one:
//!
//! * **Price-time priority is total.** Given any book and any aggressor, the
//!   sequence of executions is *fully determined*: best price first, earliest
//!   arrival within a price. The test computes the whole expected trade
//!   sequence and compares it, so a partial ordering that happens to look right
//!   on the first trade cannot pass.
//! * **Quantity is conserved exactly.** Filled plus resting plus cancelled
//!   equals the original quantity, in `Decimal`, with no tolerance. A residual
//!   that is off by one unit in the ninth decimal is a position that never
//!   closes.
//! * **An amendment costs what a venue charges for it.** Reducing size keeps
//!   queue position; adding size or moving the price goes to the back — and
//!   "the back" is checked by seeing who fills first afterwards, not by reading
//!   the sequence number the engine happens to have written.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_brokers::matching::{MatchingEngine, Participant, Trade};
use qip_core::ids::{ObjectId, OrderId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::time::Timestamp;
use qip_core::{Decimal, dec};
use qip_execution_engine::order::Side;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn object() -> ObjectId {
    ObjectId::from_string("OBJ00000000000000000000AAA")
}

fn order(label: &str) -> OrderId {
    OrderId::from_string(label)
}

/// An engine with `object()`'s book open. Since R12-COVERAGE-5107 the engine
/// creates no book on demand: an instrument is seeded and traded only once
/// its book is opened, which the simulated exchange does when it lists it.
fn opened() -> qip_core::error::Result<MatchingEngine> {
    let mut engine = MatchingEngine::new();
    engine.open(&object())?;
    Ok(engine)
}

/// One resting maker: its insertion index, its price and its size.
type Maker = (usize, Decimal, Decimal);

/// A book of `n` asks over a four-tick grid, plus a quantity to take.
fn generated_book(rng: &mut Xoshiro256) -> (Vec<Maker>, Decimal) {
    let count = 3 + rng.below(8) as usize;
    let makers = (0..count)
        .map(|index| {
            let ticks = i128::from(rng.below(4) as u32);
            let price = dec!("100") + Decimal::from_raw(ticks * 10_000_000);
            let quantity = Decimal::from_int(1 + rng.below(5) as i64);
            (index, price, quantity)
        })
        .collect();
    (makers, Decimal::from_int(1 + rng.below(40) as i64))
}

/// Rest every maker, in insertion order, on the ask side of an empty book.
fn rest_makers(engine: &mut MatchingEngine, makers: &[Maker]) -> Result<(), String> {
    for (index, price, quantity) in makers {
        let outcome = engine
            .execute(
                &object(),
                &order(&format!("maker-{index:03}")),
                Side::Sell,
                *quantity,
                Some(*price),
                now(),
                Participant::Venue,
            )
            .map_err(|error| error.message().to_string())?;
        if !outcome.resting || !outcome.trades.is_empty() {
            return Err(format!(
                "maker {index} at {price} should have rested untouched on a one-sided book, but \
                 traded {} and resting is {}",
                outcome.trades.len(),
                outcome.resting
            ));
        }
    }
    Ok(())
}

/// The executions price-time priority *requires*, given the book and the size.
///
/// Computed independently of the engine — sort by price then arrival, then walk
/// — so the test is a second opinion rather than a restatement.
fn expected_executions(makers: &[Maker], take: Decimal) -> Vec<(String, Decimal, Decimal)> {
    let mut queue: Vec<Maker> = makers.to_vec();
    queue.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut remaining = take;
    let mut executions = Vec::new();
    for (index, price, quantity) in queue {
        if remaining <= Decimal::ZERO {
            break;
        }
        let traded = quantity.min(remaining);
        remaining -= traded;
        executions.push((format!("maker-{index:03}"), price, traded));
    }
    executions
}

fn as_executions(trades: &[Trade]) -> Vec<(String, Decimal, Decimal)> {
    trades
        .iter()
        .map(|trade| {
            (
                trade.maker.as_str().to_string(),
                trade.price,
                trade.quantity,
            )
        })
        .collect()
}

#[test]
fn price_time_priority_determines_the_whole_trade_sequence() {
    Property::new("price-time priority")
        .cases(400)
        .for_all(generated_book, |(makers, take)| {
            let mut engine = opened().map_err(|error| error.message().to_string())?;
            rest_makers(&mut engine, makers)?;

            let outcome = engine
                .execute(
                    &object(),
                    &order("taker"),
                    Side::Buy,
                    *take,
                    None,
                    now(),
                    Participant::Client,
                )
                .map_err(|error| error.message().to_string())?;

            let expected = expected_executions(makers, *take);
            let actual = as_executions(&outcome.trades);
            if actual != expected {
                return Err(format!(
                    "expected {expected:?} but the engine produced {actual:?}"
                ));
            }
            Ok(())
        });
}

#[test]
fn a_taker_never_pays_worse_than_the_price_before_it() {
    Property::new("monotone price walk")
        .cases(400)
        .for_all(generated_book, |(makers, take)| {
            let mut engine = opened().map_err(|error| error.message().to_string())?;
            rest_makers(&mut engine, makers)?;
            let outcome = engine
                .execute(
                    &object(),
                    &order("taker"),
                    Side::Buy,
                    *take,
                    None,
                    now(),
                    Participant::Client,
                )
                .map_err(|error| error.message().to_string())?;
            // A buyer walks up the offers. Any dip means the engine skipped a
            // better price and came back to it, which is a venue giving a queue
            // position away.
            for pair in outcome.trades.windows(2) {
                let [earlier, later] = pair else { continue };
                if later.price < earlier.price {
                    return Err(format!(
                        "traded at {} after {}, so a better offer was passed over",
                        later.price, earlier.price
                    ));
                }
            }
            Ok(())
        });
}

#[test]
fn quantity_is_conserved_exactly() {
    Property::new("filled + resting + cancelled == quantity")
        .cases(400)
        .for_all(
            |rng| {
                let (makers, take) = generated_book(rng);
                // Sometimes a market order, sometimes a limit that crosses part
                // of the book, sometimes one that rests entirely.
                let limit = match rng.below(3) {
                    0 => None,
                    1 => Some(
                        dec!("100")
                            + Decimal::from_raw(i128::from(rng.below(4) as u32) * 10_000_000),
                    ),
                    _ => Some(dec!("99")),
                };
                (makers, take, limit)
            },
            |(makers, take, limit)| {
                let mut engine = opened().map_err(|error| error.message().to_string())?;
                rest_makers(&mut engine, makers)?;
                let taker = order("taker");
                let outcome = engine
                    .execute(
                        &object(),
                        &taker,
                        Side::Buy,
                        *take,
                        *limit,
                        now(),
                        Participant::Client,
                    )
                    .map_err(|error| error.message().to_string())?;

                let resting = engine
                    .resting(&object(), &taker)
                    .map_or(Decimal::ZERO, |resting| resting.remaining());
                if outcome.resting != (resting > Decimal::ZERO) {
                    return Err(format!(
                        "the outcome says resting={} and the book holds {resting}",
                        outcome.resting
                    ));
                }

                let accounted = outcome.filled + outcome.cancelled + resting;
                if accounted != *take {
                    return Err(format!(
                        "filled {} + cancelled {} + resting {resting} = {accounted}, not {take}",
                        outcome.filled, outcome.cancelled
                    ));
                }

                // The trades themselves must sum to the filled figure: a fill
                // total that does not decompose into executions is a number
                // nobody can reconcile.
                let traded: Decimal = outcome.trades.iter().map(|trade| trade.quantity).sum();
                if traded != outcome.filled {
                    return Err(format!(
                        "{} of executions do not sum to the filled figure {}",
                        traded, outcome.filled
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn a_market_order_never_rests() {
    Property::new("an unpriced order does not rest")
        .cases(200)
        .for_all(generated_book, |(makers, take)| {
            let mut engine = opened().map_err(|error| error.message().to_string())?;
            rest_makers(&mut engine, makers)?;
            let taker = order("taker");
            let outcome = engine
                .execute(
                    &object(),
                    &taker,
                    Side::Buy,
                    *take,
                    None,
                    now(),
                    Participant::Client,
                )
                .map_err(|error| error.message().to_string())?;
            if outcome.resting || engine.is_resting(&object(), &taker) {
                return Err("a market order became a limit order at a price nobody chose".into());
            }
            if outcome.filled + outcome.cancelled != *take {
                return Err("an unfilled remainder was neither cancelled nor rested".into());
            }
            Ok(())
        });
}

#[test]
fn reducing_size_keeps_queue_position_and_adding_size_loses_it() {
    let mut engine = opened().expect("a fresh engine opens a book");
    let first = order("first");
    let second = order("second");

    // Two sell orders at the same price. `first` arrived first, so it is ahead.
    engine
        .execute(
            &object(),
            &first,
            Side::Sell,
            Decimal::from_int(10),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect("the book is open");
    engine
        .execute(
            &object(),
            &second,
            Side::Sell,
            Decimal::from_int(10),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect("the book is open");

    // Reducing the working size is free.
    engine
        .replace(&object(), &first, Decimal::from_int(4), None, now())
        .expect("a reduction is always accepted");

    let outcome = engine
        .execute(
            &object(),
            &order("taker-a"),
            Side::Buy,
            Decimal::from_int(4),
            None,
            now(),
            Participant::Venue,
        )
        .expect("the book is open");
    assert_eq!(
        outcome
            .trades
            .first()
            .map(|trade| trade.maker.as_str().to_string()),
        Some("first".to_string()),
        "a reduction must not cost queue position"
    );

    // `first` is now gone; re-establish two orders and grow the front one.
    let third = order("third");
    engine
        .execute(
            &object(),
            &third,
            Side::Sell,
            Decimal::from_int(10),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect("the book is open");
    engine
        .replace(&object(), &second, Decimal::from_int(20), None, now())
        .expect("growing a resting order is accepted, at a price");

    let outcome = engine
        .execute(
            &object(),
            &order("taker-b"),
            Side::Buy,
            Decimal::from_int(5),
            None,
            now(),
            Participant::Venue,
        )
        .expect("the book is open");
    assert_eq!(
        outcome
            .trades
            .first()
            .map(|trade| trade.maker.as_str().to_string()),
        Some("third".to_string()),
        "adding quantity must go to the back of the queue"
    );
}

#[test]
fn moving_the_price_loses_queue_position() {
    let mut engine = opened().expect("a fresh engine opens a book");
    let first = order("first");
    let second = order("second");
    engine
        .execute(
            &object(),
            &first,
            Side::Sell,
            Decimal::from_int(5),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect("the book is open");
    engine
        .execute(
            &object(),
            &second,
            Side::Sell,
            Decimal::from_int(5),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect("the book is open");

    // Same size, moved price and moved back: still an amendment, still the back.
    engine
        .replace(
            &object(),
            &first,
            Decimal::from_int(5),
            Some(dec!("100.01")),
            now(),
        )
        .expect("repricing is accepted");
    engine
        .replace(
            &object(),
            &first,
            Decimal::from_int(5),
            Some(dec!("100")),
            now(),
        )
        .expect("repricing back is accepted");

    let outcome = engine
        .execute(
            &object(),
            &order("taker"),
            Side::Buy,
            Decimal::from_int(5),
            None,
            now(),
            Participant::Venue,
        )
        .expect("the book is open");
    assert_eq!(
        outcome
            .trades
            .first()
            .map(|trade| trade.maker.as_str().to_string()),
        Some("second".to_string()),
        "a repriced order must not keep the position it had before the move"
    );
}

#[test]
fn replacing_to_nothing_is_refused_rather_than_treated_as_a_cancel() {
    let mut engine = opened().expect("a fresh engine opens a book");
    let resting = order("resting");
    engine
        .execute(
            &object(),
            &resting,
            Side::Sell,
            Decimal::from_int(5),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect("the book is open");

    let error = engine
        .replace(&object(), &resting, Decimal::ZERO, None, now())
        .expect_err("a zero-quantity amendment is a cancel wearing a costume");
    assert_eq!(error.code(), "invalid");
    assert!(
        engine.is_resting(&object(), &resting),
        "the order must survive a refused amendment"
    );
}

#[test]
fn depth_reports_what_is_left_to_trade_not_what_was_sent() {
    let mut engine = opened().expect("a fresh engine opens a book");
    engine
        .seed(
            &object(),
            Side::Sell,
            dec!("100"),
            Decimal::from_int(10),
            now(),
        )
        .expect("seeding a positive quantity at a positive price");
    engine
        .execute(
            &object(),
            &order("taker"),
            Side::Buy,
            Decimal::from_int(3),
            None,
            now(),
            Participant::Client,
        )
        .expect("the book is open");

    let best = engine
        .best(&object(), Side::Sell)
        .expect("the level survives a partial take");
    assert_eq!(
        best.size,
        Decimal::from_int(7),
        "depth must show the residual, not the original size"
    );
    assert_eq!(engine.resting_count(), 1);
}

#[test]
fn seeding_refuses_a_non_positive_size_or_price() {
    let mut engine = opened().expect("a fresh engine opens a book");
    assert_eq!(
        engine
            .seed(&object(), Side::Sell, dec!("100"), Decimal::ZERO, now())
            .expect_err("zero size")
            .code(),
        "invalid"
    );
    assert_eq!(
        engine
            .seed(
                &object(),
                Side::Sell,
                Decimal::ZERO,
                Decimal::from_int(1),
                now()
            )
            .expect_err("zero price")
            .code(),
        "invalid"
    );
    assert_eq!(engine.resting_count(), 0);
}

#[test]
fn cancelling_something_that_is_not_there_names_what_is_missing() {
    let mut engine = MatchingEngine::new();
    let error = engine
        .cancel(&object(), &order("ghost"))
        .expect_err("no book at all");
    assert_eq!(error.code(), "not_found");

    engine.open(&object()).expect("the book opens");
    engine
        .seed(
            &object(),
            Side::Buy,
            dec!("99"),
            Decimal::from_int(1),
            now(),
        )
        .expect("seed");
    let error = engine
        .cancel(&object(), &order("ghost"))
        .expect_err("book, but no such order");
    assert_eq!(error.code(), "not_found");
    assert!(
        error.message().contains("ghost"),
        "the refusal must name the order: {}",
        error.message()
    );
}

/// R12-COVERAGE-5107: the engine creates no book on demand. Seeding
/// liquidity into, or resting an order in, an instrument whose book was never
/// opened is refused and leaves nothing behind. Until 2026-10-24 either call
/// conjured the book (`entry().or_default()`), so a market existed at the
/// venue because a caller had named an object id, whatever the listing
/// register said.
///
/// Mutation: put `entry().or_default()` back in `seed` and the first
/// `expect_err` fails; put it back at the top of `execute` and the second
/// does; drop the already-open refusal from `open` and the reopen succeeds,
/// discarding what rests.
#[test]
fn an_instrument_whose_book_was_never_opened_is_neither_seeded_nor_traded() {
    let mut engine = MatchingEngine::new();
    assert!(
        !engine.is_open(&object()),
        "premise: a new engine holds no book"
    );

    let refused = engine
        .seed(
            &object(),
            Side::Sell,
            dec!("100"),
            Decimal::from_int(5),
            now(),
        )
        .expect_err("liquidity was seeded into a book nobody opened");
    assert_eq!(refused.code(), "not_found", "{}", refused.message());
    assert!(
        refused.message().contains("no book is open for"),
        "refused for another reason: {}",
        refused.message()
    );
    let refused = engine
        .execute(
            &object(),
            &order("resting"),
            Side::Sell,
            Decimal::from_int(5),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect_err("an order rested in a book nobody opened");
    assert_eq!(refused.code(), "not_found", "{}", refused.message());
    assert!(
        !engine.is_open(&object()),
        "a refused call opened the book anyway"
    );
    assert_eq!(
        engine.resting_count(),
        0,
        "a refused call left interest resting"
    );

    // Contrast: once the book is open the same two calls are taken, so the
    // refusals above were the missing book and not the arguments.
    engine.open(&object()).expect("the book opens");
    engine
        .seed(
            &object(),
            Side::Sell,
            dec!("100"),
            Decimal::from_int(5),
            now(),
        )
        .expect("seeded into an open book");
    let outcome = engine
        .execute(
            &object(),
            &order("resting"),
            Side::Sell,
            Decimal::from_int(5),
            Some(dec!("100")),
            now(),
            Participant::Client,
        )
        .expect("an order reaches an open book");
    assert!(outcome.resting, "a sell beside a sell rests");
    assert_eq!(engine.resting_count(), 2);

    // And a book is opened once: a second open is refused rather than
    // allowed to replace the book and everything resting in it.
    let refused = engine
        .open(&object())
        .expect_err("an open book was opened again");
    assert_eq!(refused.code(), "invalid", "{}", refused.message());
    assert_eq!(
        engine.resting_count(),
        2,
        "reopening the book discarded what rested in it"
    );
}
