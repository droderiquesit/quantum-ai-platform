//! Verification tests for unretrievable evidence tracking.
//! Tests the Unretrievable variants added to RevisionCheck and LedgerOutcome
//! to handle cases where sources become inaccessible.

use qip_data_finder::ledger::LedgerOutcome;
use qip_data_finder::reference::RevisionCheck;

/// RevisionCheck::Unretrievable is_unretrievable() returns true.
#[test]
fn revision_check_unretrievable_is_marked() {
    let revision = RevisionCheck::Unretrievable;
    assert!(revision.is_unretrievable());
}

/// RevisionCheck::Unretrievable is_revised() returns false.
#[test]
fn revision_check_unretrievable_is_not_revised() {
    let revision = RevisionCheck::Unretrievable;
    assert!(!revision.is_revised());
}

/// RevisionCheck::Unretrievable describe() includes "unretrievable".
#[test]
fn revision_check_unretrievable_describe() {
    let revision = RevisionCheck::Unretrievable;
    assert!(revision.describe().contains("unretrievable"));
}

/// LedgerOutcome::Unretrievable is_unretrievable() returns true.
#[test]
fn ledger_outcome_unretrievable_is_marked() {
    let outcome = LedgerOutcome::Unretrievable;
    assert!(outcome.is_unretrievable());
}

/// LedgerOutcome::Unretrievable is_revised() returns false.
#[test]
fn ledger_outcome_unretrievable_is_not_revised() {
    let outcome = LedgerOutcome::Unretrievable;
    assert!(!outcome.is_revised());
}

/// LedgerOutcome::Unretrievable as_str() returns "unretrievable".
#[test]
fn ledger_outcome_unretrievable_as_str() {
    let outcome = LedgerOutcome::Unretrievable;
    assert_eq!(outcome.as_str(), "unretrievable");
}

/// RevisionCheck::Unretrievable is distinct from Revised.
#[test]
fn revision_check_unretrievable_distinct_from_revised() {
    let unretrievable = RevisionCheck::Unretrievable;
    let revised = RevisionCheck::Revised {
        was: "abc123".to_string(),
        now: "def456".to_string(),
    };

    assert_ne!(unretrievable, revised);
    assert!(unretrievable.is_unretrievable());
    assert!(revised.is_revised());
    assert!(!unretrievable.is_revised());
    assert!(!revised.is_unretrievable());
}

/// RevisionCheck::Unretrievable is distinct from Unchanged.
#[test]
fn revision_check_unretrievable_distinct_from_unchanged() {
    let unretrievable = RevisionCheck::Unretrievable;
    let unchanged = RevisionCheck::Unchanged;

    assert_ne!(unretrievable, unchanged);
    assert!(unretrievable.is_unretrievable());
    assert!(!unchanged.is_unretrievable());
    assert!(!unchanged.is_revised());
    assert!(!unretrievable.is_revised());
}

/// LedgerOutcome::Unretrievable is distinct from Revised.
#[test]
fn ledger_outcome_unretrievable_distinct_from_revised() {
    let unretrievable = LedgerOutcome::Unretrievable;

    // To test distinct from Revised, we just ensure Unretrievable doesn't match Revised
    assert!(!unretrievable.is_revised());
    assert!(unretrievable.is_unretrievable());
}

/// LedgerOutcome::Unretrievable is distinct from Unchanged.
#[test]
fn ledger_outcome_unretrievable_distinct_from_unchanged() {
    let unretrievable = LedgerOutcome::Unretrievable;
    let unchanged = LedgerOutcome::Unchanged;

    assert_ne!(unretrievable, unchanged);
    assert!(unretrievable.is_unretrievable());
    assert!(!unchanged.is_unretrievable());
}

/// Mutation check: removing is_unretrievable() method would cause test failure.
#[test]
fn mutation_test_unretrievable_returns_true() {
    let unretrievable = RevisionCheck::Unretrievable;
    // This test would fail if is_unretrievable() were removed or returned false
    let is_marked = unretrievable.is_unretrievable();
    assert!(is_marked, "is_unretrievable() must return true");
}

/// Mutation check: variant exists and is reachable.
#[test]
fn mutation_test_unretrievable_variant_exists() {
    // This test verifies the variant exists and is reachable.
    let _check = RevisionCheck::Unretrievable;
    let _outcome = LedgerOutcome::Unretrievable;
    // If Unretrievable were removed, this would not compile.
}
