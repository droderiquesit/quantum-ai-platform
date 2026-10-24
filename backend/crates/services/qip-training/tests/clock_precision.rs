//! A window of recorded events is promoted for labelling or replay only if
//! its clock is as precise as the consumer declared it needs (R12-RES-7103).

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_training::clock_precision::{
    ClockPrecision, ImpreciseWindow, LABEL_HORIZON_PRECISION_DIVISOR,
};
use qip_training::microstructure::LabelPlan;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

fn plan(horizons: Vec<Duration>) -> LabelPlan {
    LabelPlan {
        horizons,
        sample_every: 1,
        trailing: 5,
        volatile_above_bps: 5.0,
    }
}

/// The failure this prevents: a capture path widened an event's clock
/// uncertainty to two seconds when the cell's clock degraded, and a label at
/// a one-second horizon was still built and promoted from it as if its
/// instant were exact — the widened bound was recorded and read by nothing.
///
/// Mutation: delete `|| uncertainty > self.max_uncertainty` from
/// `ClockPrecision::admit` — the degraded window is admitted and the test
/// fails at its `expect_err`.
#[test]
fn a_window_holding_an_event_less_precise_than_the_label_needs_is_refused_whole_by_name()
-> Result<()> {
    // The desk labels at one and ten seconds; the shortest sets the need.
    let precision =
        plan(vec![Duration::from_secs(10), Duration::from_secs(1)]).clock_precision()?;
    assert_eq!(LABEL_HORIZON_PRECISION_DIVISOR, 10, "the premise failed");
    assert_eq!(precision.max_uncertainty(), Duration::from_millis(100));

    // Premise: a window captured under a level clock is admitted.
    let level = [
        (at(0), Duration::ZERO),
        (at(10), Duration::from_millis(5)),
        (at(20), Duration::from_millis(100)),
    ];
    let admitted = precision
        .admit("tick-desk-1", level)
        .map_err(qip_core::error::Error::from)?;
    assert_eq!(admitted.events, 3);
    assert_eq!(admitted.widest, Duration::from_millis(100));

    // One event widened to two seconds by a degraded clock row.
    let degraded = [
        (at(30), Duration::ZERO),
        (at(10), Duration::from_millis(2_000)),
        (at(50), Duration::from_millis(5)),
    ];
    let refused: ImpreciseWindow = precision
        .admit("tick-desk-2", degraded)
        .expect_err("a window holding a two-second doubt was promoted for a one-second label");
    assert_eq!(
        refused,
        ImpreciseWindow {
            window: "tick-desk-2".into(),
            from: at(10),
            to: at(50),
            events: 3,
            imprecise: 1,
            widest: Duration::from_millis(2_000),
            required: Duration::from_millis(100),
        }
    );
    let error = qip_core::error::Error::from(refused);
    assert!(
        error.message().contains("window tick-desk-2 ("),
        "the refusal does not name the window: {}",
        error.message()
    );

    // A negative half-width is a clock nobody can place, not an exact one.
    let backwards = [(at(0), Duration::from_millis(-1))];
    assert!(precision.admit("tick-desk-3", backwards).is_err());
    Ok(())
}

/// The failure this prevents: a precision of zero would refuse every window
/// a degraded clock ever touched and so declare nothing, and a horizon too
/// short to divide would round up to one nanosecond nobody chose.
///
/// Mutation: change `max_uncertainty <= Duration::ZERO` in
/// `ClockPrecision::new` to `< Duration::ZERO` — both refusals below pass
/// construction and the test fails.
#[test]
fn a_precision_that_declares_nothing_is_refused_at_construction() {
    let zero = ClockPrecision::new(Duration::ZERO).expect_err("a zero precision was accepted");
    assert!(
        zero.message().contains("clock precision of 0 ns"),
        "{}",
        zero.message()
    );
    let tiny = plan(vec![Duration::from_nanos(9)])
        .clock_precision()
        .expect_err("a nine-nanosecond horizon produced a precision");
    assert!(
        tiny.message()
            .contains("label horizon of 9 ns leaves no positive clock precision"),
        "{}",
        tiny.message()
    );
    // And a plan that cannot label at all is refused for that, first.
    let empty = plan(Vec::new())
        .clock_precision()
        .expect_err("a plan with no horizon produced a precision");
    assert!(
        empty.message().contains("at least one positive horizon"),
        "{}",
        empty.message()
    );
}
