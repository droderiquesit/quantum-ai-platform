//! Replay determinism.
//!
//! The platform's central claim is that a captured session can be replayed to
//! byte-identical state. The book is where that claim is easiest to break: a
//! hash map's iteration order, a tie broken by arrival time rather than by
//! sequence, a level left behind at zero size. These tests apply the same
//! stream twice and demand the two results be indistinguishable — as values, as
//! serialized bytes, and as digests.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{drain, instrument, l2_stream, l3_stream, venue};
use qip_contracts::{BookSide, VenueStatus};
use qip_core::Decimal;
use qip_core::error::Result;
use qip_orderbook::{Book, BookView, L2Book, L3Book, VenueState};

#[test]
fn applying_a_stream_twice_from_a_fresh_book_gives_byte_identical_snapshots() -> Result<()> {
    let stream = l3_stream(0x0B00_C0DE_1234, 20_000);

    let mut first = L3Book::new();
    let mut second = L3Book::new();
    for message in &stream {
        first.apply(&message.body)?;
    }
    for message in &stream {
        second.apply(&message.body)?;
    }

    let (left, right) = (first.snapshot(), second.snapshot());
    assert_eq!(
        serde_json::to_vec(&left)?,
        serde_json::to_vec(&right)?,
        "the same stream produced different serialized books"
    );
    assert_eq!(left.digest(), right.digest());
    assert_eq!(first, second, "the same stream produced different books");

    // The book must be worth comparing: an empty one would pass every
    // assertion above.
    assert!(first.resting_orders() > 100, "the stream built no book");
    assert!(left.bids.len() > 5 && left.asks.len() > 5);

    // Queue order is state too, and it is the part a snapshot of levels alone
    // would not catch.
    for level in &left.bids {
        assert_eq!(
            first.queue_at(BookSide::Bid, level.price),
            second.queue_at(BookSide::Bid, level.price),
            "queue order diverged at {}",
            level.price
        );
    }
    Ok(())
}

#[test]
fn an_aggregated_stream_replays_to_the_same_book_as_well() -> Result<()> {
    let stream = l2_stream(0x00A6_66E6_A7ED_u64, 20_000);

    let mut first = L2Book::new();
    let mut second = L2Book::new();
    for message in &stream {
        first.apply(&message.body)?;
        second.apply(&message.body)?;
    }
    // Replaying into a book that already holds the same state must be a no-op
    // for a level-set stream: every message is absolute, not incremental.
    for message in &stream {
        second.apply(&message.body)?;
    }

    assert_eq!(first, second);
    assert_eq!(first.snapshot().digest(), second.snapshot().digest());
    assert!(first.level_count(BookSide::Bid) > 5);
    Ok(())
}

#[test]
fn replaying_a_stream_into_venue_state_reproduces_every_field() -> Result<()> {
    let stream = l3_stream(0xDEC0DE, 5_000);

    let replay = |stream: &[_]| -> Result<VenueState> {
        let mut state = VenueState::order_by_order(instrument(), venue(), VenueStatus::Open);
        for message in stream {
            state.apply(message)?;
        }
        Ok(state)
    };

    let first = replay(&stream)?;
    let second = replay(&stream)?;

    assert_eq!(first.snapshot(), second.snapshot());
    assert_eq!(first.snapshot().digest(), second.snapshot().digest());
    assert_eq!(
        serde_json::to_vec(&first.snapshot())?,
        serde_json::to_vec(&second.snapshot())?
    );
    assert_eq!(first.applied(), 5_000);
    Ok(())
}

#[test]
fn draining_every_order_leaves_a_book_indistinguishable_from_a_fresh_one() -> Result<()> {
    let stream = l3_stream(0x5EED_5EED, 10_000);
    let mut book = Book::order_by_order();
    for message in &stream {
        book.apply(&message.body)?;
    }
    assert!(book.resting_orders() > 100);

    for message in &drain(&stream) {
        book.apply(&message.body)?;
    }

    let fresh = Book::order_by_order();
    assert_eq!(book, fresh, "a drained book must equal a fresh one");
    assert_eq!(book.snapshot(), fresh.snapshot());
    assert_eq!(book.snapshot().digest(), fresh.snapshot().digest());
    // No level survived at zero size, on either side.
    assert_eq!(book.level_count(BookSide::Bid), 0);
    assert_eq!(book.level_count(BookSide::Ask), 0);
    assert_eq!(book.total_size(BookSide::Ask), Decimal::ZERO);
    assert!(book.is_empty());
    Ok(())
}

#[test]
fn a_snapshot_taken_twice_from_one_book_is_the_same_snapshot() -> Result<()> {
    let stream = l3_stream(7, 2_000);
    let mut book = L3Book::new();
    for message in &stream {
        book.apply(&message.body)?;
    }

    assert_eq!(book.snapshot(), book.snapshot());
    assert_eq!(book.snapshot().digest(), book.snapshot().digest());
    // A shallower snapshot is a prefix of the full one, so a consumer that
    // keeps only the top of book compares against the same values.
    let shallow = book.snapshot_to(3);
    let full = book.snapshot();
    assert_eq!(shallow.bids, full.bids[..3]);
    assert_eq!(shallow.asks, full.asks[..3]);
    assert_ne!(shallow.digest(), full.digest());
    Ok(())
}
