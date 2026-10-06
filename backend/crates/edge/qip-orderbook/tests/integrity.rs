//! Reconstruction flags what it cannot vouch for, and the flag reaches the
//! replay's output.

#![allow(clippy::panic_in_result_fn)]

#[allow(dead_code)]
mod common;

use common::{instrument, l3_stream, message, venue};
use qip_contracts::{MarketMessage, MessageBody, VenueStatus};
use qip_core::error::Result;
use qip_orderbook::VenueState;
use qip_orderbook::replay::{IntegrityFault, replay};

const GAP_AT: usize = 1_000;
const LEN: usize = 1_200;

fn fresh() -> VenueState {
    VenueState::order_by_order(instrument(), venue(), VenueStatus::Open)
}

fn stream_without_one_message() -> Vec<MarketMessage> {
    let mut stream = l3_stream(0xFACADE, LEN);
    stream.remove(GAP_AT);
    stream
}

#[test]
fn an_unmodified_recorded_sequence_produces_no_flag() -> Result<()> {
    let stream = l3_stream(0xFACADE, LEN);
    let mut state = fresh();
    let output = replay(&mut state, &stream)?;
    assert_eq!(
        output.steps.len(),
        LEN,
        "premise: every message was replayed"
    );
    assert!(output.is_clean());
    assert!(output.steps.iter().all(|s| !s.unreliable));
    assert_eq!(state.applied(), LEN as u64);
    Ok(())
}

#[test]
fn removing_one_message_flags_the_period_from_the_gap_and_the_flag_is_on_the_output() -> Result<()>
{
    let stream = stream_without_one_message();
    let mut state = fresh();
    let output = replay(&mut state, &stream)?;

    assert_eq!(output.periods.len(), 1);
    let period = &output.periods[0];
    assert_eq!(
        period.fault,
        IntegrityFault::Gap {
            missing_from: GAP_AT as u64,
            missing_to: GAP_AT as u64
        }
    );
    // The period opens at the first message after the hole and, with no
    // reset in the input, is still open at its end.
    assert_eq!(period.from, stream[GAP_AT].venue_time);
    assert_eq!(period.until, None);
    assert_eq!(period.messages, LEN - 1 - GAP_AT);

    for step in &output.steps {
        assert_eq!(
            step.unreliable,
            step.stream_sequence > GAP_AT as u64,
            "sequence {} flagged wrongly",
            step.stream_sequence
        );
    }
    // The book edits inside the period were withheld, not applied to a book
    // already known to be wrong.
    assert_eq!(state.applied(), GAP_AT as u64);
    Ok(())
}

#[test]
fn a_reset_closes_the_period_and_the_messages_after_it_are_reliable_again() -> Result<()> {
    let mut stream = stream_without_one_message();
    let reset = message(
        LEN as u64,
        MessageBody::Reset {
            reason: "venue restart".into(),
        },
    );
    let reset_time = reset.venue_time;
    stream.push(reset);

    let mut state = fresh();
    let output = replay(&mut state, &stream)?;

    assert_eq!(
        output.periods.len(),
        1,
        "premise: the gap opened one episode"
    );
    assert_eq!(output.periods[0].until, Some(reset_time));
    let last = output.steps.last().expect("steps");
    assert!(!last.unreliable, "the reset itself is reliable");
    assert!(
        !state.is_stale(),
        "the venue's reset resynchronises the state"
    );
    Ok(())
}

#[test]
fn a_message_behind_one_already_applied_is_flagged_out_of_order() -> Result<()> {
    let mut stream = l3_stream(0xFACADE, LEN);
    let stale = stream[100].clone();
    stream.insert(600, stale);
    let mut state = fresh();
    let output = replay(&mut state, &stream)?;

    assert_eq!(output.periods.len(), 1);
    assert_eq!(
        output.periods[0].fault,
        IntegrityFault::OutOfOrder {
            sequence: 100,
            expected: 600
        }
    );
    assert!(output.steps[600].unreliable);
    Ok(())
}
