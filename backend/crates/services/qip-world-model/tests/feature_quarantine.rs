//! R12-RES-7102: a feature derived from an unreliable book is quarantined in
//! the world model's store, and the contaminated window stays out after the
//! quarantine lifts. The cell quarantines its own features; this store is
//! where the platform's longer-lived copies are read from, and nothing here
//! marked them, so a value computed across a feed gap read as ordinary.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_world_model::features::{FeatureLookup, FeatureStore, FeatureValue};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_000 + secs)
}

fn store() -> FeatureStore {
    let mut store = FeatureStore::new();
    for (secs, value) in [(10, 1.0), (20, 2.0), (30, 3.0), (40, 4.0)] {
        store.record("mid", "ACME", FeatureValue::immediate(value, at(secs)));
    }
    store.record("mid", "OTHER", FeatureValue::immediate(9.0, at(25)));
    store
}

#[test]
fn a_quarantined_subject_reads_as_quarantined_and_another_subject_is_untouched() {
    let mut store = store();
    assert!(
        store
            .lookup_as_of("mid", "ACME", at(35), at(35))
            .value()
            .is_some(),
        "premise: the value is readable before the quarantine"
    );
    store.quarantine_subject("ACME", at(25));
    assert_eq!(
        store.lookup_as_of("mid", "ACME", at(35), at(35)),
        FeatureLookup::Quarantined { since: at(25) }
    );
    assert!(
        store
            .lookup_as_of("mid", "ACME", at(15), at(15))
            .value()
            .is_some()
    );
    assert_eq!(
        store
            .lookup_as_of("mid", "OTHER", at(35), at(35))
            .value()
            .map(|v| v.value),
        Some(9.0),
        "a quarantine marked a subject it was not raised for"
    );
}

#[test]
fn a_lifted_quarantine_keeps_the_contaminated_window_out_and_never_reaches_back_across_it()
-> Result<()> {
    let mut store = store();
    store.quarantine_subject("ACME", at(25));
    assert_eq!(store.lift_quarantine("ACME", at(35))?, Some(at(25)));
    // The value stamped at 30 fell in the hole and stays out.
    assert!(
        store
            .history("mid", "ACME", at(100))
            .iter()
            .all(|v| (v.value - 3.0).abs() > 1e-9)
    );
    // A read after the lift is not answered from the value before the hole.
    assert_eq!(
        store.lookup_as_of("mid", "ACME", at(36), at(36)),
        FeatureLookup::NoValue
    );
    // The first value stamped after the lift reads normally.
    assert_eq!(
        store
            .lookup_as_of("mid", "ACME", at(40), at(40))
            .value()
            .map(|v| v.value),
        Some(4.0)
    );
    Ok(())
}

#[test]
fn a_second_report_keeps_the_first_instant_and_a_lift_before_the_start_is_refused() {
    let mut store = store();
    store.quarantine_subject("ACME", at(25));
    store.quarantine_subject("ACME", at(33));
    assert_eq!(store.quarantined_since("ACME"), Some(at(25)));
    assert!(store.lift_quarantine("ACME", at(20)).is_err());
    assert_eq!(store.lift_quarantine("NEVER", at(50)).ok(), Some(None));
}
