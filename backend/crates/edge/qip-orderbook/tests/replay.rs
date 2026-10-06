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

/// A replay that starts from a checkpoint must be indistinguishable from one
/// that started at the beginning, at every sequence number after it.
///
/// The failure it prevents: a checkpoint of levels alone restores the sizes and
/// loses queue order, so the two replays agree on every level digest while
/// every queue-position answer differs. The comparison therefore includes the
/// checkpoint itself (which holds queue order), not just the snapshot digest.
#[test]
fn a_replay_started_from_a_checkpoint_matches_a_replay_from_the_start_at_every_sequence()
-> Result<()> {
    for (seed, cut) in [(0xC0FFEE_u64, 1_500usize), (0xBADC0DE, 3_333), (11, 4_000)] {
        let stream = l3_stream(seed, 5_000);
        let mut full = VenueState::order_by_order(instrument(), venue(), VenueStatus::Open);
        let mut head = VenueState::order_by_order(instrument(), venue(), VenueStatus::Open);
        for m in &stream[..cut] {
            full.apply(m)?;
            head.apply(m)?;
        }
        // Premise: the checkpoint is taken of a populated book, and it
        // survives a trip through bytes as it would across a process boundary.
        assert!(
            head.book().resting_orders() > 50,
            "seed {seed}: empty checkpoint"
        );
        let bytes = serde_json::to_vec(&head.checkpoint())?;
        let mut resumed = VenueState::restore(&serde_json::from_slice(&bytes)?)?;
        assert_eq!(resumed.snapshot().digest(), full.snapshot().digest());

        for (i, m) in stream[cut..].iter().enumerate() {
            full.apply(m)?;
            resumed.apply(m)?;
            assert_eq!(
                resumed.snapshot().digest(),
                full.snapshot().digest(),
                "seed {seed}: diverged {i} messages after the checkpoint"
            );
        }
        // Queue order is read off the live books, not off either checkpoint,
        // so a checkpoint that scrambled it cannot vouch for itself.
        let (f, r) = (
            full.book().as_order_by_order().expect("order-by-order"),
            resumed.book().as_order_by_order().expect("order-by-order"),
        );
        let bids = full.snapshot().book.bids;
        assert!(bids.len() > 3, "seed {seed}: premise, several levels");
        for level in &bids {
            assert_eq!(
                f.queue_at(BookSide::Bid, level.price),
                r.queue_at(BookSide::Bid, level.price),
                "seed {seed}: queue order diverged at {}",
                level.price
            );
        }
    }
    Ok(())
}

#[test]
fn an_aggregated_book_restored_from_a_checkpoint_replays_to_the_same_book() -> Result<()> {
    let stream = l2_stream(0xA66, 4_000);
    let mut full = Book::aggregated();
    let mut head = Book::aggregated();
    for m in &stream[..2_000] {
        full.apply(&m.body)?;
        head.apply(&m.body)?;
    }
    assert!(head.level_count(BookSide::Bid) > 5, "empty checkpoint");
    let mut resumed = Book::restore(&head.checkpoint())?;
    for m in &stream[2_000..] {
        full.apply(&m.body)?;
        resumed.apply(&m.body)?;
        assert_eq!(resumed.snapshot().digest(), full.snapshot().digest());
    }
    Ok(())
}

#[test]
fn a_checkpoint_that_repeats_an_order_reference_is_refused_not_repaired() -> Result<()> {
    let mut book = L3Book::new();
    book.add(
        1,
        BookSide::Bid,
        Decimal::from_int(100),
        Decimal::from_int(5),
    )?;
    let mut cp = book.checkpoint();
    assert_eq!(cp.orders.len(), 1, "premise: one resting order");
    cp.orders.push(cp.orders[0]);
    assert!(L3Book::restore(&cp).is_err());
    Ok(())
}
