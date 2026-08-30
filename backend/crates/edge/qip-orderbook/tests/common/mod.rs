//! Deterministic message streams shared by the replay and throughput tests.
//!
//! Generated from a seeded [`Xoshiro256`] rather than captured from a file, so
//! the streams are reproducible from the seed alone and a failure names the
//! seed that produced it. The generator tracks what is resting so every message
//! it emits is one a venue could legally send: no reduction that raises a
//! quantity, no removal of an order that never existed.

use qip_contracts::{BookSide, MarketMessage, MessageBody, Origin, VenueId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};

pub(crate) fn instrument() -> ObjectId {
    ObjectId::from_string("obj-aapl")
}

pub(crate) fn venue() -> VenueId {
    VenueId::new("XNAS")
}

pub(crate) fn message(sequence: u64, body: MessageBody) -> MarketMessage {
    let at =
        Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_micros(sequence as i64));
    MarketMessage::new(
        instrument(),
        Origin::new(venue(), "itch-a", 1, sequence),
        body,
        at,
        at,
    )
}

/// A price in whole cents, as an exact decimal.
fn cents(value: i128) -> Decimal {
    Decimal::from_raw(value * 10_000_000)
}

fn side_of(rng: &mut Xoshiro256) -> BookSide {
    if rng.bernoulli(0.5) {
        BookSide::Bid
    } else {
        BookSide::Ask
    }
}

/// An order-by-order stream of adds, reductions, replaces and removals.
///
/// Roughly half the messages are adds, so over a hundred thousand messages the
/// book grows to tens of thousands of resting orders spread over about fifty
/// levels. That is deeper per level than a real venue, deliberately: queue
/// insertion and removal are the operations worth stressing, and a wide ladder
/// with three orders a level would flatter any implementation of them.
pub(crate) fn l3_stream(seed: u64, count: usize) -> Vec<MarketMessage> {
    let mut rng = Xoshiro256::seeded(seed);
    let mut resting: Vec<(u64, BookSide, Decimal, i64)> = Vec::new();
    let mut next_ref = 1u64;
    let mut stream = Vec::with_capacity(count);

    for sequence in 0..count as u64 {
        let body = if resting.is_empty() || rng.bernoulli(0.5) {
            let side = side_of(&mut rng);
            let ticks = rng.below(25) as i128;
            let price = match side {
                BookSide::Bid => cents(10_000 - ticks),
                BookSide::Ask => cents(10_010 + ticks),
            };
            let quantity = 1 + rng.below(500) as i64;
            let order_ref = next_ref;
            next_ref += 1;
            resting.push((order_ref, side, price, quantity));
            MessageBody::OrderAdded {
                order_ref,
                side,
                price,
                quantity: Decimal::from_int(quantity),
            }
        } else {
            let index = rng.below(resting.len() as u64) as usize;
            let (order_ref, side, price, quantity) = resting[index];
            match rng.below(4) {
                0 => {
                    resting.swap_remove(index);
                    MessageBody::OrderRemoved { order_ref }
                }
                1 => {
                    let remaining = rng.below(quantity as u64) as i64;
                    if remaining == 0 {
                        resting.swap_remove(index);
                    } else {
                        resting[index].3 = remaining;
                    }
                    MessageBody::OrderReduced {
                        order_ref,
                        remaining: Decimal::from_int(remaining),
                    }
                }
                2 => {
                    // A price move: the order loses its place in the queue.
                    let ticks = rng.below(25) as i128;
                    let moved = match side {
                        BookSide::Bid => cents(10_000 - ticks),
                        BookSide::Ask => cents(10_010 + ticks),
                    };
                    resting[index].2 = moved;
                    MessageBody::OrderReplaced {
                        order_ref,
                        price: moved,
                        quantity: Decimal::from_int(quantity),
                    }
                }
                _ => {
                    // A size change at the same price: up loses priority, down
                    // keeps it, and the stream exercises both.
                    let size = 1 + rng.below(500) as i64;
                    resting[index].3 = size;
                    MessageBody::OrderReplaced {
                        order_ref,
                        price,
                        quantity: Decimal::from_int(size),
                    }
                }
            }
        };
        stream.push(message(sequence, body));
    }
    stream
}

/// An aggregated stream, published sparsely and out of order, with deletes.
pub(crate) fn l2_stream(seed: u64, count: usize) -> Vec<MarketMessage> {
    let mut rng = Xoshiro256::seeded(seed);
    let mut stream = Vec::with_capacity(count);

    for sequence in 0..count as u64 {
        let side = side_of(&mut rng);
        let ticks = rng.below(40) as i128;
        let price = match side {
            BookSide::Bid => cents(10_000 - ticks),
            BookSide::Ask => cents(10_010 + ticks),
        };
        // One update in eight deletes the level, which is roughly what a real
        // aggregated feed looks like once a book is populated.
        let quantity = if rng.bernoulli(0.125) {
            Decimal::ZERO
        } else {
            Decimal::from_int(1 + rng.below(5_000) as i64)
        };
        stream.push(message(
            sequence,
            MessageBody::LevelSet {
                side,
                price,
                quantity,
                order_count: Some(1 + rng.below(20) as u32),
            },
        ));
    }
    stream
}

/// Removals for everything an L3 stream left resting.
pub(crate) fn drain(stream: &[MarketMessage]) -> Vec<MarketMessage> {
    let mut resting: Vec<u64> = Vec::new();
    for message in stream {
        match &message.body {
            MessageBody::OrderAdded { order_ref, .. } => resting.push(*order_ref),
            MessageBody::OrderRemoved { order_ref } => resting.retain(|r| r != order_ref),
            MessageBody::OrderReduced {
                order_ref,
                remaining,
            } if remaining.is_zero() => resting.retain(|r| r != order_ref),
            _ => {}
        }
    }
    let base = stream.len() as u64;
    resting
        .into_iter()
        .enumerate()
        .map(|(index, order_ref)| {
            message(base + index as u64, MessageBody::OrderRemoved { order_ref })
        })
        .collect()
}
