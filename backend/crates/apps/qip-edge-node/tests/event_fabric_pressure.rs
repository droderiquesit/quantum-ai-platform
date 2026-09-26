//! The spool-pressure gauge's mapping to `qip_edge::pressure::JournalPressure`
//! (ADR 0100 §6, SLICE-54).
//!
//! `qip-edge`'s own tests drive `Cell::apply_journal_pressure` with readings
//! they build by hand; what they cannot see is how the node turns a spool's
//! bytes, bits and heartbeat into one, which is where a wrong answer is
//! silent — a fenced producer read as `Normal` keeps adding exposure whose
//! record the broker will refuse forever, and a disconnected drain read as
//! `Narrow` turns every broker restart into a trading change. Every test
//! here drives the two publishers the way the spool writer and the drain
//! will, and asserts the reading the decision thread would hand the cell.

use qip_core::{Clock, Decimal, Duration, ManualClock, Timestamp};
use qip_edge::pressure::{Exhaustion, JournalPressure, Narrowing};
use qip_edge_node::event_fabric::pressure::{
    DrainPublisher, PressureGauge, SpoolPublisher, Thresholds,
};
use qip_edge_node::event_fabric::telemetry::PressureState;
use std::sync::Arc;

const BUDGET: u64 = 1_000;
const NARROW_LINE: u64 = 600;
const EXHAUST_LINE: u64 = 900;

fn fraction(text: &str) -> Decimal {
    Decimal::parse(text).expect("a literal fraction parses")
}

fn bound() -> Duration {
    Duration::from_secs(5)
}

fn thresholds() -> Thresholds {
    Thresholds::new(BUDGET, fraction("0.6"), fraction("0.9"))
        .expect("0.6 below 0.9 of a thousand bytes is a valid pair of lines")
}

fn gauge() -> (
    Arc<ManualClock>,
    PressureGauge,
    SpoolPublisher,
    DrainPublisher,
) {
    let clock = Arc::new(ManualClock::new(Timestamp::from_secs(1_760_000_000)));
    let handed: Arc<dyn Clock> = clock.clone();
    let (gauge, spool, drain) =
        PressureGauge::new(thresholds(), handed, bound()).expect("a positive bound is accepted");
    (clock, gauge, spool, drain)
}

fn exhausted(cause: Exhaustion) -> JournalPressure {
    JournalPressure::Exhausted(cause)
}

fn narrow() -> JournalPressure {
    JournalPressure::Narrow(Narrowing::half())
}

/// RES-062: the spool narrows sizing before it halts new exposure, never
/// jumps from `Normal` to a halt; and a producer a newer epoch has fenced
/// (ADR 0100 §4) halts whatever its fill, because nothing it spools will
/// ever be accepted.
#[test]
fn spool_pressure_reads_narrow_before_exhausted_and_a_fenced_producer_reads_exhausted() {
    // The lines are what the thresholds say, and a pair that would skip the
    // narrowing is refused rather than nudged into shape.
    let lines = thresholds();
    assert_eq!(lines.narrow_bytes(), NARROW_LINE);
    assert_eq!(lines.exhaust_bytes(), EXHAUST_LINE);
    // Each refusal names its own cause: an inverted pair told it "coincides
    // in whole bytes" would send the operator to the budget, not the lines.
    for (narrow_at, exhaust_at) in [("0.9", "0.9"), ("0.95", "0.9")] {
        let refused = Thresholds::new(BUDGET, fraction(narrow_at), fraction(exhaust_at))
            .expect_err("a narrowing line not below the exhaustion line is refused");
        assert!(
            refused
                .message()
                .contains("is not below the exhaustion line"),
            "{narrow_at} against {exhaust_at}: {}",
            refused.message()
        );
    }
    // Distinct fractions that floor to the same whole byte skip it too.
    let floored = Thresholds::new(3, fraction("0.5"), fraction("0.6"))
        .expect_err("lines that coincide in whole bytes are refused");
    assert!(
        floored.message().contains("coincide in whole bytes"),
        "{}",
        floored.message()
    );

    let (_clock, gauge, mut spool, mut drain) = gauge();
    spool.set_used_bytes(Some(0));
    spool
        .beat()
        .expect("the first beat advances the generation");
    drain.set_connected(true);
    // Premise: an empty, live, unfenced spool reads Normal.
    assert_eq!(gauge.read().pressure, JournalPressure::Normal);
    assert_eq!(gauge.read().state(), PressureState::Normal);

    // Walk every fill from empty to past the budget, one byte at a time.
    let mut first_narrow = None;
    let mut first_exhausted = None;
    for used in 0..=BUDGET + 1 {
        spool.set_used_bytes(Some(used));
        let reading = gauge.read();
        match reading.pressure {
            JournalPressure::Normal => {
                assert!(
                    first_narrow.is_none() && first_exhausted.is_none(),
                    "Normal again at {used} bytes after the spool had narrowed or halted"
                );
            }
            JournalPressure::Narrow(narrowing) => {
                assert_eq!(narrowing, Narrowing::half());
                assert_eq!(reading.state(), PressureState::Narrow);
                assert!(first_exhausted.is_none(), "Narrow at {used} after a halt");
                first_narrow.get_or_insert(used);
            }
            JournalPressure::Exhausted(cause) => {
                assert_eq!(cause, Exhaustion::OverBudget, "at {used} bytes");
                assert_eq!(reading.state(), PressureState::Exhausted);
                first_exhausted.get_or_insert(used);
            }
        }
    }
    assert_eq!(first_narrow, Some(NARROW_LINE));
    assert_eq!(first_exhausted, Some(EXHAUST_LINE));

    // Fenced, on an empty spool that just read Normal.
    spool.set_used_bytes(Some(0));
    assert_eq!(gauge.read().pressure, JournalPressure::Normal);
    drain.mark_fenced();
    let fenced = gauge.read();
    assert_eq!(fenced.pressure, exhausted(Exhaustion::Fenced));
    assert_eq!(fenced.state(), PressureState::Exhausted);
    // And across every fill, including the narrowing band.
    for used in [1, NARROW_LINE, EXHAUST_LINE - 1] {
        spool.set_used_bytes(Some(used));
        assert_eq!(
            gauge.read().pressure,
            exhausted(Exhaustion::Fenced),
            "{used} bytes"
        );
    }
}

/// ADR 0100 §8 proving test 2: a fabric outage leaves decisions identical,
/// because the spool absorbs it. What an outage costs is the spool filling,
/// which is measured already; disconnection alone must not narrow a cell.
#[test]
fn a_disconnected_drain_below_the_narrowing_line_reads_normal() {
    let (_clock, gauge, mut spool, mut drain) = gauge();
    spool
        .beat()
        .expect("the first beat advances the generation");
    drain.set_connected(true);
    spool.set_used_bytes(Some(NARROW_LINE - 1));
    // Premise: connected, one byte below the line, Normal.
    let connected = gauge.read();
    assert!(connected.connected);
    assert_eq!(connected.pressure, JournalPressure::Normal);

    drain.set_connected(false);
    for used in 0..NARROW_LINE {
        spool.set_used_bytes(Some(used));
        let reading = gauge.read();
        assert!(!reading.connected, "the bit is carried for the gauge");
        assert_eq!(
            reading.pressure,
            JournalPressure::Normal,
            "disconnected at {used} bytes"
        );
        assert_eq!(reading.state(), PressureState::Normal);
        assert_eq!(reading.used_bytes, Some(used));
    }
    // At the line, the fill narrows it — the same as it would connected.
    spool.set_used_bytes(Some(NARROW_LINE));
    assert_eq!(gauge.read().pressure, narrow());
}

/// The last good reading is exactly what a dead writer leaves behind, so a
/// heartbeat older than the bound overrides it; and the bound is judged on
/// the writer's heartbeat, so a fresh beat releases it. A gauge nobody has
/// ever beaten reads stale from the start rather than as a quiet spool.
#[test]
fn a_heartbeat_older_than_the_bound_reads_exhausted_stale_and_a_fresh_one_does_not() {
    let zero = PressureGauge::new(
        thresholds(),
        Arc::new(ManualClock::new(Timestamp::from_secs(0))),
        Duration::ZERO,
    );
    assert!(zero.is_err(), "a zero bound judges every heartbeat stale");

    // Never beaten on a clock that has barely started: the unset instant is
    // within the bound of `now`, so age alone would call it fresh. The
    // generation is what says no heartbeat has ever happened.
    let (early, mut early_spool, _early_drain) = PressureGauge::new(
        thresholds(),
        Arc::new(ManualClock::new(Timestamp::from_secs(1))),
        bound(),
    )
    .expect("a positive bound is accepted");
    early_spool.set_used_bytes(Some(0));
    assert_eq!(early.read().generation, 0);
    assert_eq!(early.read().pressure, exhausted(Exhaustion::Stale));

    let (clock, gauge, mut spool, _drain) = gauge();
    spool.set_used_bytes(Some(100));
    let never = gauge.read();
    assert_eq!(never.generation, 0);
    assert_eq!(never.pressure, exhausted(Exhaustion::Stale));

    assert_eq!(spool.beat().expect("first beat"), 1);
    // Premise: fresh on the beat and at exactly the bound.
    assert_eq!(gauge.read().pressure, JournalPressure::Normal);
    clock.advance(bound());
    assert_eq!(gauge.read().pressure, JournalPressure::Normal);

    // One nanosecond past the bound: stale, in the Normal band and in the
    // narrowing band alike.
    clock.advance(Duration::from_nanos(1));
    assert_eq!(gauge.read().pressure, exhausted(Exhaustion::Stale));
    assert_eq!(gauge.read().state(), PressureState::Exhausted);
    spool.set_used_bytes(Some(NARROW_LINE));
    assert_eq!(gauge.read().pressure, exhausted(Exhaustion::Stale));

    // A fresh beat, and the fill decides again.
    assert_eq!(spool.beat().expect("second beat"), 2);
    assert_eq!(gauge.read().pressure, narrow());
    spool.set_used_bytes(Some(100));
    assert_eq!(gauge.read().pressure, JournalPressure::Normal);
    assert_eq!(gauge.read().generation, 2);
}

/// A spool that refused a write cannot take the record of new exposure,
/// however empty it is; nor can one whose size the writer could not read,
/// which is never taken for an empty spool.
#[test]
fn an_unwritable_spool_reads_exhausted_whatever_its_fill() {
    let (_clock, gauge, mut spool, mut drain) = gauge();
    spool
        .beat()
        .expect("the first beat advances the generation");
    drain.set_connected(true);
    let fills = [
        0,
        1,
        NARROW_LINE - 1,
        NARROW_LINE,
        EXHAUST_LINE - 1,
        EXHAUST_LINE,
        BUDGET,
        u64::MAX,
    ];
    // Premise: writable, the fills span every band.
    let writable: Vec<JournalPressure> = fills
        .iter()
        .map(|used| {
            spool.set_used_bytes(Some(*used));
            gauge.read().pressure
        })
        .collect();
    assert!(writable.contains(&JournalPressure::Normal));
    assert!(writable.contains(&narrow()));
    assert!(writable.contains(&exhausted(Exhaustion::OverBudget)));

    spool.set_unwritable(true);
    for used in fills {
        spool.set_used_bytes(Some(used));
        let reading = gauge.read();
        assert_eq!(
            reading.pressure,
            exhausted(Exhaustion::Unwritable),
            "unwritable at {used} bytes"
        );
        assert_eq!(reading.state(), PressureState::Exhausted);
    }

    spool.set_unwritable(false);
    spool.set_used_bytes(Some(0));
    assert_eq!(gauge.read().pressure, JournalPressure::Normal);

    // A size the writer could not read: exhausted, and no byte count shown.
    spool.set_used_bytes(None);
    let unread = gauge.read();
    assert_eq!(unread.used_bytes, None);
    assert_eq!(unread.pressure, exhausted(Exhaustion::Unwritable));
}
