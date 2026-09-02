//! The simulated venue and the books behind it.
//!
//! What is being defended here is the claim that a fill from this venue can be
//! trusted as an oracle. That needs four things to be true at once:
//!
//! * **Every acknowledgement adds up.** Filled plus remaining equals what was
//!   sent, exactly, including after an amendment and after somebody else's
//!   taker hits a resting order.
//! * **The books reconcile against the fill stream.** Replaying the venue's own
//!   fills into an independent ledger reproduces its positions and its cash to
//!   the last unit — and the cash figure is checked against the arithmetic
//!   directly, not against the ledger that produced it.
//! * **The same seed produces the same run.** Two venues given the same seed
//!   and the same instructions return byte-identical acknowledgements, jitter
//!   and unexplained rejections included.
//! * **The unpleasant paths are real.** A venue that goes quiet degrades, a
//!   degraded venue refuses orders and still accepts cancels, an off-lot or
//!   off-tick order is refused, and an execution algorithm is refused with a
//!   message saying where it should have been worked instead.

#![allow(clippy::panic_in_result_fn)]

use qip_brokers::adapter::VenueAdapter;
use qip_brokers::credential::{RequirementKind, requirements_of_kind, standard_requirements};
use qip_brokers::exchange::{BookableFill, ExchangeSettings, SimulatedExchange};
use qip_brokers::ledger::{AccountLedger, MarginPolicy};
use qip_brokers::{ConnectionPhase, VenueCredential};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal, dec};
use qip_execution_engine::broker::Broker;
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_financial::asset_class::InstrumentType;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;

const VENUE: &str = "XSIM";
const ACCOUNT: &str = "book-under-test";

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("OBJ00000000000000000000AAA")
}

fn instrument() -> FinancialObject {
    FinancialObject::builder(object(), "AAA", InstrumentType::CommonStock)
        .name("Instrument A")
        .venue(VENUE)
        .price(dec!("100"))
        .lot_size(Decimal::ONE)
        .tick_size(dec!("0.01"))
        .provenance(Provenance::synthetic("qip-brokers test", start()))
        .build(start())
        .expect("a structurally valid instrument")
}

/// A credential satisfying exactly what a simulator can honestly enforce.
///
/// It names environment variables and carries no values, which is the ordinary
/// shape: the simulator checks that an account and a session secret were
/// *named*, because those are the two mistakes that carry straight over to a
/// real venue.
fn credential() -> VenueCredential {
    let enforced = requirements_of_kind(
        &standard_requirements(&venue()),
        &[RequirementKind::Account, RequirementKind::SessionCredential],
    );
    VenueCredential::satisfying(VENUE, ACCOUNT, &enforced).expect("a named venue and account")
}

/// A venue listed, seeded on both sides, logged on and heartbeating.
fn live_venue(settings: ExchangeSettings, seed: u64) -> Result<SimulatedExchange> {
    let mut exchange = SimulatedExchange::new(venue(), settings, seed, start());
    exchange.list(instrument());
    for (side, price, size) in [
        (Side::Sell, dec!("100.02"), 20),
        (Side::Sell, dec!("100.03"), 40),
        (Side::Sell, dec!("100.05"), 80),
        (Side::Buy, dec!("99.98"), 20),
        (Side::Buy, dec!("99.97"), 40),
        (Side::Buy, dec!("99.95"), 80),
    ] {
        exchange.seed_liquidity(&object(), side, price, Decimal::from_int(size), start())?;
    }
    exchange.bring_up(&credential(), start())?;
    Ok(exchange)
}

fn order_at(label: &str, side: Side, quantity: i64, order_type: OrderType) -> Order {
    Order::new(
        OrderId::from_string(label),
        object(),
        side,
        Decimal::from_int(quantity),
        order_type,
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    )
}

// --- acknowledgements add up ------------------------------------------------

#[test]
fn a_partial_fill_and_its_residual_are_the_original_quantity() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 11)?;
    let ticket = exchange.ready(start())?;

    // Twenty rest at the touch and the rest of the book is dearer than the
    // limit, so a buy of fifty takes twenty and leaves thirty working.
    let order = order_at(
        "partial",
        Side::Buy,
        50,
        OrderType::Limit {
            price: dec!("100.02"),
        },
    );
    let ack = exchange.submit_order(&ticket, &order, start())?;

    assert_eq!(ack.filled_quantity(), Decimal::from_int(20));
    assert_eq!(ack.remaining, Decimal::from_int(30));
    // The identity, stated the way a reconciliation would: nothing is created
    // and nothing goes missing between the fill and the residual.
    assert_eq!(ack.filled_quantity() + ack.remaining, order.quantity);
    assert_eq!(ack.state.as_str(), "partially_filled");

    let venue_order = exchange.query_order(&order.order_id)?;
    assert_eq!(
        venue_order.filled + venue_order.remaining(),
        venue_order.original_quantity
    );
    assert_eq!(venue_order.original_quantity, order.quantity);
    assert_eq!(venue_order.state.as_str(), "partially_filled");

    // The residual is not merely reported: it is in the book, at the limit.
    let depth: Decimal = exchange
        .market_data(&object(), start())?
        .book
        .bids
        .iter()
        .filter(|level| level.price == dec!("100.02"))
        .map(|level| level.size)
        .sum();
    assert_eq!(
        depth, ack.remaining,
        "the residual must be working, not merely counted"
    );
    Ok(())
}

#[test]
fn venue_flow_fills_a_resting_client_order_and_the_order_record_and_the_fill_stream_agree()
-> Result<()> {
    // A client order resting below the offers, hit by somebody else's
    // flow. The order record the client would poll and the fill stream the
    // client's books are built from must both show the trade, at the
    // resting price, as a buy — and the flow's own remainder must not stay
    // in the book as liquidity nobody seeded.
    let mut exchange = live_venue(ExchangeSettings::orderly(), 11)?;
    let ticket = exchange.ready(start())?;
    let order = order_at(
        "maker",
        Side::Buy,
        50,
        OrderType::Limit {
            price: dec!("99.99"),
        },
    );
    let ack = exchange.submit_order(&ticket, &order, start())?;
    assert_eq!(
        ack.remaining,
        Decimal::from_int(50),
        "the premise is an order that rests in full"
    );
    // Drained so what follows is the flow's fills alone.
    assert!(exchange.drain_bookable_fills().is_empty());
    let resting_before = exchange.resting_count();

    let taken = exchange.seed_aggressor(
        &object(),
        Side::Sell,
        dec!("99.99"),
        Decimal::from_int(130),
        start(),
    )?;
    // The client's fifty at 99.99 is all the flow can take at its price:
    // the seeded bids are all below it, so the other eighty go nowhere.
    assert_eq!(
        taken,
        Decimal::from_int(50),
        "the flow did not take the resting order"
    );
    assert_eq!(
        exchange.resting_count(),
        resting_before - 1,
        "the flow's remainder rested, or the maker was not consumed"
    );

    let record = exchange.query_order(&order.order_id)?;
    assert_eq!(
        record.filled,
        Decimal::from_int(50),
        "the order record was not advanced"
    );
    assert_eq!(record.state.as_str(), "filled");

    let fills: Vec<BookableFill> = exchange.drain_bookable_fills();
    assert_eq!(fills.len(), 1, "{fills:?}");
    assert_eq!(fills[0].fill.order_id, order.order_id);
    assert_eq!(
        fills[0].side,
        Side::Buy,
        "the maker was booked on the taker's side"
    );
    assert_eq!(fills[0].fill.quantity, Decimal::from_int(50));
    assert_eq!(
        fills[0].fill.price,
        dec!("99.99"),
        "a maker trades at its own price"
    );
    let position = exchange
        .ledger()
        .positions()
        .into_iter()
        .find(|position| position.object_id == object())
        .expect("the maker fill was booked");
    assert_eq!(position.quantity, Decimal::from_int(50));
    Ok(())
}

#[test]
fn an_unpriced_remainder_is_cancelled_rather_than_quietly_filled() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 12)?;
    let ticket = exchange.ready(start())?;

    // The ask side holds 140. A market order for 200 cannot be filled, and the
    // sixty it cannot take must not appear from nowhere.
    let order = order_at("outruns-the-book", Side::Buy, 200, OrderType::Market);
    let ack = exchange.submit_order(&ticket, &order, start())?;

    assert_eq!(ack.filled_quantity(), Decimal::from_int(140));
    assert_eq!(ack.remaining, Decimal::from_int(60));
    assert_eq!(ack.filled_quantity() + ack.remaining, order.quantity);
    assert_eq!(ack.state.as_str(), "cancelled");
    assert_eq!(
        exchange.resting_count(),
        3,
        "a market order must not join the book"
    );
    Ok(())
}

#[test]
fn quantity_survives_an_amendment() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 13)?;
    let ticket = exchange.ready(start())?;

    let order = order_at(
        "amended",
        Side::Buy,
        30,
        OrderType::Limit {
            price: dec!("100.02"),
        },
    );
    let ack = exchange.submit_order(&ticket, &order, start())?;
    let filled = ack.filled_quantity();
    assert_eq!(filled, Decimal::from_int(20), "the touch holds twenty");

    let ack = exchange.replace_order(
        &ticket,
        &order.order_id,
        Decimal::from_int(50),
        None,
        start(),
    )?;
    assert_eq!(ack.remaining, Decimal::from_int(50) - filled);

    let venue_order = exchange.query_order(&order.order_id)?;
    assert_eq!(venue_order.filled, filled);
    assert_eq!(
        venue_order.filled + venue_order.remaining(),
        venue_order.quantity
    );
    assert_eq!(venue_order.original_quantity, Decimal::from_int(30));
    assert_eq!(venue_order.revision, 1);
    Ok(())
}

#[test]
fn an_amendment_below_what_has_already_filled_is_refused() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 14)?;
    let ticket = exchange.ready(start())?;
    let order = order_at(
        "shrink",
        Side::Buy,
        30,
        OrderType::Limit {
            price: dec!("100.02"),
        },
    );
    exchange.submit_order(&ticket, &order, start())?;

    let error = exchange
        .replace_order(
            &ticket,
            &order.order_id,
            Decimal::from_int(5),
            None,
            start(),
        )
        .expect_err("twenty are already done; five is not a quantity this order can have");
    assert_eq!(error.code(), "invalid");
    Ok(())
}

// --- the books reconcile ----------------------------------------------------

/// Replay a fill stream into a ledger that shares nothing with the venue's.
fn replay(fills: &[BookableFill], opening: Decimal) -> Result<AccountLedger> {
    let mut ledger = AccountLedger::new(
        "replayed",
        Currency::USD,
        opening,
        MarginPolicy::default(),
        start(),
    );
    ledger.register(instrument());
    for bookable in fills {
        ledger.apply(&bookable.object_id, &bookable.fill, bookable.side)?;
    }
    Ok(ledger)
}

/// Cash implied by the fill stream alone: every fill moves cash by its notional
/// and its costs, and nothing else moves cash at all.
fn cash_from_fills(fills: &[BookableFill], opening: Decimal) -> Decimal {
    fills.iter().fold(opening, |cash, bookable| {
        let signed = match bookable.side {
            Side::Buy => bookable.fill.quantity,
            Side::Sell => -bookable.fill.quantity,
        };
        cash - (signed * bookable.fill.price) - bookable.fill.costs
    })
}

#[test]
fn the_ledger_reconciles_against_the_fill_stream() -> Result<()> {
    let settings = ExchangeSettings::orderly();
    let opening = settings.opening_cash;
    let mut exchange = live_venue(settings, 21)?;
    let mut rng = Xoshiro256::seeded(0x9E37_79B9);

    // A run mixing takers, resting limits that later get hit, amendments and
    // cancels — so the stream contains maker fills the acknowledgements never
    // mentioned, which is the half a naive reconciliation loses.
    for index in 0..60u32 {
        let ticket = exchange.ready(start())?;
        let side = if rng.bernoulli(0.5) {
            Side::Buy
        } else {
            Side::Sell
        };
        let quantity = 1 + rng.below(25) as i64;
        let order_type = match rng.below(3) {
            0 => OrderType::Market,
            1 => OrderType::Limit {
                price: dec!("100.02"),
            },
            _ => OrderType::Limit {
                price: dec!("99.98"),
            },
        };
        let order = order_at(&format!("run-{index:03}"), side, quantity, order_type);
        // A refusal is a legitimate outcome and must not disturb the books.
        let _ = exchange.submit_order(&ticket, &order, start());
        if rng.bernoulli(0.2) {
            let _ = exchange.cancel_order(&order.order_id, start());
        }
    }

    let fills = exchange.drain_bookable_fills();
    assert!(
        !fills.is_empty(),
        "the run must actually trade, or it proves nothing"
    );

    let replayed = replay(&fills, opening)?;
    assert_eq!(
        replayed.positions(),
        exchange.query_positions()?,
        "replaying the venue's own fills must reproduce its positions exactly"
    );

    let venue_cash = exchange.query_cash()?;
    assert_eq!(replayed.cash().settled, venue_cash.settled);
    assert_eq!(replayed.cash().costs, venue_cash.costs);
    assert_eq!(
        cash_from_fills(&fills, opening),
        venue_cash.settled,
        "cash must be the fill stream's arithmetic and nothing else"
    );
    assert_eq!(venue_cash.opening, opening);
    assert_eq!(venue_cash.movement(), venue_cash.settled - opening);

    // Net position per instrument is the signed sum of the stream.
    let net: Decimal = fills
        .iter()
        .map(|bookable| match bookable.side {
            Side::Buy => bookable.fill.quantity,
            Side::Sell => -bookable.fill.quantity,
        })
        .sum();
    let held: Decimal = exchange
        .query_positions()?
        .iter()
        .map(|position| position.quantity)
        .sum();
    assert_eq!(held, net);

    exchange.ledger().verify(start())?;
    Ok(())
}

/// Drive a venue through a randomised session and return it with its stream.
///
/// Deliberately messy: market and limit orders on both sides, amendments,
/// cancels, and orders that will be refused. The books have to balance after
/// all of it, not after the tidy subset.
fn randomised_run(seed: u64) -> Result<(SimulatedExchange, Vec<BookableFill>, Vec<String>)> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), seed)?;
    let mut rng = Xoshiro256::seeded(seed);
    let mut breaks = Vec::new();

    for index in 0..40u32 {
        let ticket = exchange.ready(start())?;
        let side = if rng.bernoulli(0.5) {
            Side::Buy
        } else {
            Side::Sell
        };
        let quantity = 1 + rng.below(25) as i64;
        let order_type = match rng.below(4) {
            0 => OrderType::Market,
            1 => OrderType::Limit {
                price: dec!("100.02"),
            },
            2 => OrderType::Limit {
                price: dec!("99.98"),
            },
            // Refused: an execution algorithm is not something a venue works.
            _ => OrderType::Participation { rate: 0.1 },
        };
        let order = order_at(&format!("case-{index:03}"), side, quantity, order_type);
        if let Ok(ack) = exchange.submit_order(&ticket, &order, start()) {
            // Every acknowledgement accounts for the whole order.
            if ack.filled_quantity() + ack.remaining != order.quantity {
                breaks.push(format!(
                    "order {} filled {} and left {} of {}",
                    order.order_id.as_str(),
                    ack.filled_quantity(),
                    ack.remaining,
                    order.quantity
                ));
            }
        }
        if rng.bernoulli(0.25) {
            let grown = Decimal::from_int(quantity + 1 + rng.below(10) as i64);
            let _ = exchange.replace_order(&ticket, &order.order_id, grown, None, start());
        }
        if rng.bernoulli(0.25) {
            let _ = exchange.cancel_order(&order.order_id, start());
        }
    }

    let fills = exchange.drain_bookable_fills();
    Ok((exchange, fills, breaks))
}

#[test]
fn the_books_balance_after_any_run() {
    Property::new("books balance after a randomised session")
        .cases(40)
        .for_all(
            |rng| rng.next_u64(),
            |seed| {
                let (exchange, fills, breaks) =
                    randomised_run(*seed).map_err(|error| error.to_string())?;
                if let Some(first) = breaks.first() {
                    return Err(format!(
                        "an acknowledgement did not account for its order: {first}"
                    ));
                }
                if fills.is_empty() {
                    return Err("the run never traded, so it proves nothing".to_string());
                }

                let opening = exchange.settings().opening_cash;
                let cash = exchange.query_cash().map_err(|error| error.to_string())?;
                if cash_from_fills(&fills, opening) != cash.settled {
                    return Err(format!(
                        "cash {} is not the fill stream's arithmetic {}",
                        cash.settled,
                        cash_from_fills(&fills, opening)
                    ));
                }

                // The independent statement of the same claim: a position is the
                // signed sum of the fills that made it, and nothing else.
                let net: Decimal = fills
                    .iter()
                    .map(|bookable| match bookable.side {
                        Side::Buy => bookable.fill.quantity,
                        Side::Sell => -bookable.fill.quantity,
                    })
                    .sum();
                let expected = exchange
                    .query_positions()
                    .map_err(|error| error.to_string())?;
                let held: Decimal = expected.iter().map(|position| position.quantity).sum();
                if held != net {
                    return Err(format!(
                        "the venue holds {held}, the fill stream says {net}"
                    ));
                }

                let replayed = replay(&fills, opening).map_err(|error| error.to_string())?;
                if replayed.positions() != expected {
                    return Err(format!(
                        "replaying the stream gave {:?}, the venue holds {expected:?}",
                        replayed.positions()
                    ));
                }

                exchange
                    .ledger()
                    .verify(start())
                    .map_err(|error| format!("the books do not balance: {error}"))
            },
        );
}

#[test]
fn a_fill_in_an_unregistered_instrument_is_refused_rather_than_guessed_at() -> Result<()> {
    let mut ledger = AccountLedger::new(
        "unregistered",
        Currency::USD,
        Decimal::from_int(1_000),
        MarginPolicy::default(),
        start(),
    );
    let mut exchange = live_venue(ExchangeSettings::orderly(), 22)?;
    let ticket = exchange.ready(start())?;
    exchange.submit_order(
        &ticket,
        &order_at("one", Side::Buy, 5, OrderType::Market),
        start(),
    )?;
    let fills = exchange.drain_bookable_fills();
    let bookable = fills.first().expect("the order traded");

    let error = ledger
        .apply(&bookable.object_id, &bookable.fill, bookable.side)
        .expect_err("a contract multiplier must never be guessed");
    assert_eq!(error.code(), "not_found");
    assert!(ledger.positions().is_empty());
    Ok(())
}

#[test]
fn margin_excludes_a_position_it_cannot_price_and_says_so() -> Result<()> {
    let exchange = live_venue(ExchangeSettings::orderly(), 23)?;
    let margin = exchange.query_margin(start())?;
    // Nothing has traded, so there is nothing to mark and nothing to require.
    assert_eq!(margin.equity, margin.cash);
    assert!(margin.unpriced.is_empty());
    assert!(margin.permits_new_risk());
    assert!(!margin.is_call());

    let mut ledger = AccountLedger::new(
        "unpriced",
        Currency::USD,
        Decimal::from_int(1_000),
        MarginPolicy::cash_account(),
        start(),
    );
    ledger.register(instrument());
    // A position registered but never traded and never marked has no mark, and
    // must be reported rather than treated as free.
    assert!(ledger.margin(start()).unpriced.is_empty());
    assert_eq!(ledger.margin(start()).gross_exposure, Decimal::ZERO);
    Ok(())
}

// --- the same seed produces the same run ------------------------------------

/// Drive a venue through a fixed script and return everything it said.
fn scripted_run(seed: u64) -> Result<(Vec<String>, Vec<BookableFill>)> {
    // Default settings: latency jitter and unexplained rejections both on, so
    // the run genuinely depends on the seeded generator.
    let mut exchange = live_venue(ExchangeSettings::default(), seed)?;
    let mut transcript = Vec::new();
    for index in 0..40u32 {
        let at = start().saturating_add(Duration::from_millis(i64::from(index)));
        let heartbeat = exchange.heartbeat(at)?;
        transcript.push(format!(
            "hb {} {:?}",
            heartbeat.sequence, heartbeat.round_trip
        ));
        let Ok(ticket) = exchange.ready(at) else {
            transcript.push("not ready".to_string());
            continue;
        };
        let side = if index % 2 == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        let order = order_at(
            &format!("script-{index:03}"),
            side,
            1 + i64::from(index % 7),
            OrderType::Limit {
                price: dec!("100.02"),
            },
        );
        match exchange.submit_order(&ticket, &order, at) {
            Ok(ack) => transcript.push(format!("{ack:?}")),
            Err(error) => transcript.push(format!("refused {error}")),
        }
    }
    Ok((transcript, exchange.drain_bookable_fills()))
}

#[test]
fn the_same_seed_produces_the_same_run() -> Result<()> {
    let first = scripted_run(0x51E5_D001)?;
    let second = scripted_run(0x51E5_D001)?;
    assert_eq!(
        first.0, second.0,
        "the same seed must produce the same acknowledgements"
    );
    assert_eq!(
        first.1, second.1,
        "the same seed must produce the same fills"
    );
    Ok(())
}

#[test]
fn a_different_seed_produces_a_different_run() -> Result<()> {
    let first = scripted_run(0x51E5_D001)?;
    let other = scripted_run(0x51E5_D002)?;
    // If these agreed, the seed would not be reaching the venue's coin-flips
    // and the determinism test above would be proving nothing.
    assert_ne!(first.0, other.0);
    Ok(())
}

// --- the unpleasant paths are real ------------------------------------------

#[test]
fn a_venue_that_goes_quiet_degrades_and_stops_accepting_orders() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 31)?;
    let ticket = exchange.ready(start())?;
    exchange.stop_heartbeat();

    // Well past the thirty seconds the session allows.
    let later = start().saturating_add(Duration::from_secs(90));
    assert_eq!(
        exchange.connection().effective_phase(later),
        ConnectionPhase::Degraded
    );

    let error = exchange
        .heartbeat(later)
        .expect_err("a stopped venue does not answer");
    assert_eq!(error.code(), "timeout");

    let order = order_at("into-the-dark", Side::Buy, 5, OrderType::Market);
    let error = exchange
        .submit_order(&ticket, &order, later)
        .expect_err("a ticket is proof about a moment, and the moment has passed");
    assert_eq!(error.code(), "denied");

    let health = exchange.health(later);
    assert!(!health.accepts_orders);
    assert!(health.is_degraded());
    assert!(health.describe().contains("degraded"));
    Ok(())
}

#[test]
fn a_degraded_venue_still_accepts_a_cancel() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 32)?;
    let ticket = exchange.ready(start())?;
    let order = order_at(
        "resting",
        Side::Buy,
        10,
        OrderType::Limit {
            price: dec!("99.90"),
        },
    );
    exchange.submit_order(&ticket, &order, start())?;

    exchange.stop_heartbeat();
    let later = start().saturating_add(Duration::from_secs(90));
    assert_eq!(
        exchange.connection().effective_phase(later),
        ConnectionPhase::Degraded
    );

    // The whole point: the safe direction never waits for permission.
    let ack = exchange.cancel_order(&order.order_id, later)?;
    assert_eq!(ack.state.as_str(), "cancelled");
    assert_eq!(ack.remaining, Decimal::from_int(10));
    Ok(())
}

#[test]
fn a_heartbeat_recovers_a_degraded_session() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 33)?;
    exchange.stop_heartbeat();
    let quiet = start().saturating_add(Duration::from_secs(90));
    assert!(exchange.heartbeat(quiet).is_err());
    assert_eq!(exchange.connection().phase(), ConnectionPhase::Degraded);

    exchange.start_heartbeat();
    let recovered = quiet.saturating_add(Duration::from_secs(1));
    let heartbeat = exchange.heartbeat(recovered)?;
    assert_eq!(heartbeat.phase, ConnectionPhase::Ready);
    assert!(exchange.ready(recovered).is_ok());
    Ok(())
}

#[test]
fn the_venue_refuses_what_it_cannot_honestly_trade() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 34)?;
    let ticket = exchange.ready(start())?;

    let unlisted = Order::new(
        OrderId::from_string("unlisted"),
        ObjectId::from_string("OBJ00000000000000000000ZZZ"),
        Side::Buy,
        Decimal::from_int(1),
        OrderType::Market,
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    );
    assert_eq!(
        exchange
            .submit_order(&ticket, &unlisted, start())
            .expect_err("not listed")
            .code(),
        "denied"
    );

    let off_lot = order_at("off-lot", Side::Buy, 1, OrderType::Market);
    let off_lot = Order {
        quantity: dec!("1.5"),
        ..off_lot
    };
    assert_eq!(
        exchange
            .submit_order(&ticket, &off_lot, start())
            .expect_err("off lot")
            .code(),
        "denied"
    );

    let off_tick = order_at(
        "off-tick",
        Side::Buy,
        1,
        OrderType::Limit {
            price: dec!("100.005"),
        },
    );
    assert_eq!(
        exchange
            .submit_order(&ticket, &off_tick, start())
            .expect_err("off tick")
            .code(),
        "denied"
    );

    let algorithmic = order_at(
        "twap",
        Side::Buy,
        10,
        OrderType::TimeWeighted { minutes: 30 },
    );
    let error = exchange
        .submit_order(&ticket, &algorithmic, start())
        .expect_err("an algorithm is not an order");
    assert_eq!(error.code(), "denied");
    assert!(
        error.message().contains("child orders"),
        "the refusal must say where the work belongs: {}",
        error.message()
    );

    let duplicate = order_at(
        "duplicate",
        Side::Buy,
        5,
        OrderType::Limit {
            price: dec!("99.90"),
        },
    );
    exchange.submit_order(&ticket, &duplicate, start())?;
    assert_eq!(
        exchange
            .submit_order(&ticket, &duplicate, start())
            .expect_err("reused id")
            .code(),
        "invalid"
    );

    assert_eq!(exchange.rejected_count(), 5);
    Ok(())
}

#[test]
fn a_venue_that_refuses_everything_refuses_cleanly() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::always_refuses(), 35)?;
    let ticket = exchange.ready(start())?;
    let order = order_at("refused", Side::Buy, 10, OrderType::Market);

    let error = exchange
        .submit_order(&ticket, &order, start())
        .expect_err("this venue refuses");
    assert_eq!(error.code(), "denied");
    // A refusal is not a fill and not a position.
    assert!(exchange.query_positions()?.is_empty());
    assert_eq!(
        exchange.query_cash()?.settled,
        exchange.settings().opening_cash
    );
    assert!(exchange.drain_bookable_fills().is_empty());
    assert_eq!(
        exchange
            .query_order(&order.order_id)
            .expect_err("nothing was accepted")
            .code(),
        "not_found"
    );
    Ok(())
}

#[test]
fn market_data_shows_the_book_that_is_actually_resting() -> Result<()> {
    let mut exchange = live_venue(ExchangeSettings::orderly(), 36)?;
    let data = exchange.market_data(&object(), start())?;
    let quote = data.quote.as_ref().expect("both sides are seeded");
    assert_eq!(quote.bid, dec!("99.98"));
    assert_eq!(quote.ask, dec!("100.02"));
    assert_eq!(data.book.bids.len(), 3);
    assert_eq!(data.book.asks.len(), 3);
    assert!(data.last_trade.is_none(), "nothing has traded yet");
    assert!(data.simulated);

    let ticket = exchange.ready(start())?;
    exchange.submit_order(
        &ticket,
        &order_at("taker", Side::Buy, 20, OrderType::Market),
        start(),
    )?;
    let data = exchange.market_data(&object(), start())?;
    assert_eq!(data.last_trade, Some(dec!("100.02")));
    assert_eq!(
        data.quote.as_ref().expect("still two-sided").ask,
        dec!("100.03"),
        "the touch must move once the level behind it is the best offer"
    );
    Ok(())
}

#[test]
fn the_broker_port_cannot_reach_a_venue_that_is_not_ready() -> Result<()> {
    let mut exchange = SimulatedExchange::new(venue(), ExchangeSettings::orderly(), 37, start());
    exchange.list(instrument());
    exchange.seed_liquidity(
        &object(),
        Side::Sell,
        dec!("100.02"),
        Decimal::from_int(50),
        start(),
    )?;

    // Never brought up: the narrow OMS port mints its own ticket and fails to.
    let order = order_at("through-the-port", Side::Buy, 5, OrderType::Market);
    let error = exchange
        .submit(&order, start())
        .expect_err("no session, no ticket, no order");
    assert_eq!(error.code(), "denied");

    exchange.bring_up(&credential(), start())?;
    let fills = exchange.submit(&order, start())?;
    assert_eq!(fills.len(), 1);
    assert!(fills.iter().all(|fill| fill.simulated));
    Ok(())
}

#[test]
fn capabilities_and_requirements_describe_a_venue_nobody_could_mistake_for_real() -> Result<()> {
    let exchange = live_venue(ExchangeSettings::orderly(), 38)?;
    let capabilities = exchange.capabilities();
    assert_eq!(capabilities.name, VENUE);
    assert_eq!(
        capabilities.supported_types,
        vec!["market".to_string(), "limit".to_string()]
    );
    assert!(capabilities.partial_fills);
    assert_eq!(capabilities.lot_size, Decimal::ONE);

    // A simulator is complete for what it is and incomplete as a venue, and it
    // reports the second.
    assert_eq!(
        exchange.missing_requirements().len(),
        standard_requirements(&venue()).len()
    );
    let summary = exchange.requirement_summary();
    assert!(summary.contains("QIP_XSIM_ENDPOINT"), "{summary}");
    assert!(summary.contains("QIP_XSIM_ENABLED"), "{summary}");
    assert!(summary.contains("Nothing is defaulted"), "{summary}");
    assert_eq!(exchange.requirement(), summary);
    Ok(())
}
