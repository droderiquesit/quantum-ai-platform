//! A throughput floor for the hot path.
//!
//! Not a latency benchmark and not a claim about one: this is a wall-clock
//! measurement of a whole test binary, taken on whatever machine happens to run
//! it, and the assertion is deliberately a floor loose enough that only a
//! genuine regression — an accidental clone of the book per message, a linear
//! scan where there was a lookup — can trip it. The measured rate is printed so
//! a reader can see the real figure rather than infer one from the bound.
//!
//! Run with `--release` for a number worth quoting; `cargo test` builds
//! unoptimised and is several times slower.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{drain, instrument, l2_stream, l3_stream, venue};
use qip_contracts::{BookSide, VenueStatus};
use qip_core::Decimal;
use qip_core::error::Result;
use qip_orderbook::{BookView, L2Book, VenueState};
use std::time::Instant;

/// Messages applied per measured run.
const MESSAGES: usize = 100_000;

/// A ceiling loose enough to survive a loaded shared machine and tight enough
/// that an accidentally quadratic book cannot hide under it.
const CEILING_SECONDS: f64 = 60.0;

fn report(label: &str, messages: usize, elapsed: std::time::Duration) {
    let seconds = elapsed.as_secs_f64();
    let rate = messages as f64 / seconds;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "{label}: {messages} messages in {seconds:.3}s = {rate:.0} msg/s \
         ({:.0} ns/message, {profile} profile)",
        seconds * 1e9 / messages as f64
    );
    assert!(
        seconds < CEILING_SECONDS,
        "{label} took {seconds:.3}s for {messages} messages"
    );
}

#[test]
fn a_hundred_thousand_order_by_order_messages_apply_through_venue_state() -> Result<()> {
    let stream = l3_stream(0xF00D_5EED, MESSAGES);
    let mut state = VenueState::order_by_order(instrument(), venue(), VenueStatus::Open);

    let started = Instant::now();
    for message in &stream {
        state.apply(message)?;
    }
    let elapsed = started.elapsed();

    assert_eq!(state.applied(), MESSAGES as u64);
    assert!(state.book().resting_orders() > 1_000);
    report("L3 apply (through VenueState)", MESSAGES, elapsed);

    println!(
        "  book: {} resting orders across {} bid and {} ask levels",
        state.book().resting_orders(),
        state.book().level_count(BookSide::Bid),
        state.book().level_count(BookSide::Ask),
    );
    Ok(())
}

#[test]
fn a_hundred_thousand_aggregated_messages_apply_to_a_level_book() -> Result<()> {
    let stream = l2_stream(0xBEEF_5EED, MESSAGES);
    let mut book = L2Book::new();

    let started = Instant::now();
    for message in &stream {
        book.apply(&message.body)?;
    }
    let elapsed = started.elapsed();

    assert!(book.level_count(BookSide::Bid) > 1);
    report("L2 apply", MESSAGES, elapsed);
    Ok(())
}

#[test]
fn reading_the_book_is_fast_enough_to_do_on_every_message() -> Result<()> {
    let stream = l3_stream(0xC0FFEE, 20_000);
    let mut state = VenueState::order_by_order(instrument(), venue(), VenueStatus::Open);
    for message in &stream {
        state.apply(message)?;
    }

    let reads = MESSAGES;
    let size = Decimal::from_int(2_000);
    let started = Instant::now();
    let mut sink = Decimal::ZERO;
    for index in 0..reads {
        let taking = if index % 2 == 0 {
            BookSide::Ask
        } else {
            BookSide::Bid
        };
        // The three reads a strategy actually takes per message.
        if let Some(mid) = state.mid() {
            sink += mid;
        }
        if let Some(micro) = state.microprice() {
            sink += micro;
        }
        if let Some(sweep) = state.sweep_cost(taking, size) {
            sink += sweep.filled;
        }
    }
    let elapsed = started.elapsed();

    assert!(sink.is_positive(), "the reads must not be optimised away");
    report("mid + microprice + sweep_cost", reads, elapsed);
    Ok(())
}

#[test]
fn draining_a_large_book_stays_linear_in_the_orders_it_holds() -> Result<()> {
    let stream = l3_stream(0x0DD_5EED, MESSAGES / 2);
    let mut state = VenueState::order_by_order(instrument(), venue(), VenueStatus::Open);
    for message in &stream {
        state.apply(message)?;
    }
    let removals = drain(&stream);
    let count = removals.len();
    assert!(count > 1_000);

    let started = Instant::now();
    for message in &removals {
        state.apply(message)?;
    }
    let elapsed = started.elapsed();

    assert_eq!(state.book().resting_orders(), 0);
    assert!(state.book().is_empty());
    report("L3 cancel", count, elapsed);
    Ok(())
}
