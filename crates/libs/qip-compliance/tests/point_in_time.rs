//! Control 1 — point-in-time truth.
//!
//! Every test here tries to read something the platform could not have known
//! yet, by whichever route looks most plausible: asking the reader directly,
//! widening its horizon, or smuggling an input past it.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_compliance::pit::{LeakageDetector, PointInTime};
use qip_contracts::time::Stamped;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

/// A dividend announced at `valid`, which the platform learned at `known`.
fn fact(value: i64, valid: i64, known: i64) -> Stamped<i64> {
    Stamped::new(value, t(valid), t(known))
}

#[test]
fn a_reader_never_holds_a_fact_that_was_not_yet_knowable() {
    // The fact is true from second 10 but only reaches the platform at 100.
    // A reader as of 50 must behave as though it does not exist — not filter
    // it on the way out, but not have it.
    let reader = PointInTime::as_of(t(50), [fact(7, 10, 100)]);

    assert!(reader.is_empty());
    assert_eq!(reader.withheld(), 1);
    assert!(reader.latest().is_none());
    assert!(reader.in_force_at(t(50)).is_none());
    // Asking for the fact's own valid-time does not resurrect it: valid-time
    // is not the filter, and that is exactly the mistake this prevents.
    assert!(reader.in_force_at(t(10)).is_none());
    assert!(reader.known().is_empty());
    assert!(reader.require_latest().is_err());
}

#[test]
fn the_same_facts_read_later_become_visible() {
    // The complement of the test above: the control is about *when*, not about
    // suppressing data, so a reader past the known-time sees everything.
    let facts = [fact(7, 10, 100), fact(9, 20, 110)];
    let early = PointInTime::as_of(t(50), facts);
    let late = PointInTime::as_of(t(200), facts);

    assert_eq!(early.len(), 0);
    assert_eq!(late.len(), 2);
    assert_eq!(late.withheld(), 0);
    assert_eq!(late.latest().map(|f| *f.value()), Some(9));
}

#[test]
fn a_reader_cannot_be_widened_to_see_the_future() -> Result<()> {
    // The only way a caller holding a reader could reach a later fact would be
    // to move its horizon forward. That operation is refused, and the refusal
    // names both horizons so the mistake is obvious in a log.
    let reader = PointInTime::as_of(t(50), [fact(7, 10, 100)]);

    let widened = reader.restrict_to(t(500));
    let error = widened.expect_err("widening a point-in-time reader must be refused");
    assert!(error.message().contains("cannot be widened"));

    // Narrowing is allowed, and takes effect.
    let narrow = PointInTime::as_of(t(200), [fact(7, 10, 100), fact(9, 20, 190)])
        .restrict_to(t(150))?;
    assert_eq!(narrow.horizon(), t(150));
    assert_eq!(narrow.len(), 1);
    assert_eq!(narrow.withheld(), 1);
    Ok(())
}

#[test]
fn a_reader_returns_the_fact_in_force_rather_than_the_latest_one() -> Result<()> {
    // Both times matter. The fact in force at second 15 is the one valid from
    // second 10, even though a later fact is also knowable.
    let reader = PointInTime::as_of(
        t(500),
        [fact(1, 10, 12), fact(2, 30, 31), fact(3, 60, 61)],
    );

    assert_eq!(*reader.require_in_force_at(t(15))?.value(), 1);
    assert_eq!(*reader.require_in_force_at(t(45))?.value(), 2);
    assert_eq!(*reader.require_latest()?.value(), 3);
    // Nothing was in force before the first fact became true.
    assert!(reader.in_force_at(t(5)).is_none());
    Ok(())
}

#[test]
fn a_reader_reports_how_late_its_feed_is_without_revealing_withheld_values() {
    // The lag statistic is computed only from facts already visible, so it
    // cannot be used to infer anything about the ones held back.
    let reader = PointInTime::as_of(t(500), [fact(1, 10, 12), fact(2, 30, 90)]);
    assert_eq!(reader.worst_latency(), Duration::from_secs(60));
}

#[test]
fn the_detector_catches_an_input_that_was_deliberately_leaked() -> Result<()> {
    // The realistic leak: three honest features and one that quietly carries
    // tomorrow's close, assembled by hand outside any reader.
    let detector = LeakageDetector::new(t(100));
    let open = fact(1, 10, 20);
    let volume = fact(2, 10, 30);
    let spread = fact(3, 10, 40);
    let tomorrows_close = fact(4, 10, 100_000);

    let report = detector.audit([
        ("open", &open),
        ("volume", &volume),
        ("spread", &spread),
        ("tomorrows_close", &tomorrows_close),
    ]);

    assert_eq!(report.inspected(), 4);
    assert!(!report.is_clean());
    assert_eq!(report.findings().len(), 1);
    assert_eq!(report.findings()[0].input, "tomorrows_close");

    let error = report
        .require_clean()
        .expect_err("a leaked input must fail the audit");
    // The refusal has to name the column; "leakage detected" is unactionable.
    assert!(error.message().contains("tomorrows_close"));
    Ok(())
}

#[test]
fn the_detector_names_every_leaked_input_not_merely_the_first() {
    // A fix that addresses one leak and leaves two is worse than none, because
    // the next run looks clean.
    let detector = LeakageDetector::new(t(100));
    let a = fact(1, 10, 500);
    let b = fact(2, 10, 20);
    let c = fact(3, 10, 600);

    let report = detector.audit([("a", &a), ("b", &b), ("c", &c)]);
    let message = report
        .require_clean()
        .expect_err("two leaks must fail")
        .message()
        .to_string();

    assert!(message.contains("`a`"));
    assert!(message.contains("`c`"));
    assert!(!message.contains("`b`"));
}

#[test]
fn a_fact_known_exactly_at_the_as_of_is_knowable() {
    // The boundary is inclusive: a fact that arrived at the instant being
    // reasoned about was available. Getting this backwards silently drops the
    // most recent observation from every read.
    let reader = PointInTime::as_of(t(100), [fact(5, 10, 100)]);
    assert_eq!(reader.len(), 1);
    assert!(LeakageDetector::new(t(100)).inspect("x", &fact(5, 10, 100)).is_none());
    assert!(LeakageDetector::new(t(99)).inspect("x", &fact(5, 10, 100)).is_some());
}
