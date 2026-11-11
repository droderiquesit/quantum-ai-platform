//! `ModelEligibility` (ADR 0386) is a record of what the model registry said
//! at one instant. These hold the three ways a record could claim more than
//! it was told.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::model_eligibility::{ModelEligibility, ModelVerdict};
use qip_core::Timestamp;
use qip_core::error::Result;

fn at() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

/// A model nobody recorded is `Unrecorded`, never `Eligible`. A reader
/// that treated silence as clearance would pass every model the composition
/// forgot to ask about.
///
/// Mutation: return `ModelVerdict::Eligible` from the `None` arm of
/// `ModelEligibility::verdict`.
#[test]
fn a_model_the_record_does_not_name_is_unrecorded_and_the_three_readings_stay_distinct()
-> Result<()> {
    let mut record = ModelEligibility::taken_at(at());
    record.record_eligible("ranker@2.1")?;
    record.record_refused("sizer@1.0", "sizer@1.0 may not drive a decision: retired")?;
    assert_eq!(record.instant(), at());
    assert_eq!(record.verdict("ranker@2.1"), ModelVerdict::Eligible);
    assert_eq!(
        record.verdict("sizer@1.0"),
        ModelVerdict::Refused("sizer@1.0 may not drive a decision: retired")
    );
    assert_eq!(record.verdict("ranker@2.2"), ModelVerdict::Unrecorded);
    // The delimited reference, not a prefix of it.
    assert_eq!(record.verdict("ranker@2"), ModelVerdict::Unrecorded);
    Ok(())
}

/// One model has one verdict in one record. A second write is refused and
/// the first stands: a refusal overwritten by a later clearance in the same
/// record would be a refusal nobody ever saw.
///
/// Mutation: remove the `contains_key` refusal in `ModelEligibility::record`.
#[test]
fn a_second_verdict_on_one_model_is_refused_and_the_first_stands() -> Result<()> {
    let mut record = ModelEligibility::taken_at(at());
    record.record_refused("ranker@2.1", "overdue for review")?;
    let error = record
        .record_eligible("ranker@2.1")
        .expect_err("a second verdict is refused");
    assert!(
        error.message().contains("already has a verdict"),
        "{}",
        error.message()
    );
    assert_eq!(
        record.verdict("ranker@2.1"),
        ModelVerdict::Refused("overdue for review")
    );
    Ok(())
}

/// A refusal carries the registry's reason and a verdict names its model.
///
/// Mutations: remove the empty-reason refusal in `record_refused`; remove
/// the empty-reference refusal in `record`.
#[test]
fn a_refusal_without_a_reason_and_a_verdict_without_a_model_are_refused() {
    let mut record = ModelEligibility::taken_at(at());
    let error = record
        .record_refused("ranker@2.1", "  ")
        .expect_err("a blank reason is refused");
    assert!(error.message().contains("reason"), "{}", error.message());
    assert_eq!(record.verdict("ranker@2.1"), ModelVerdict::Unrecorded);
    let error = record
        .record_eligible(" ")
        .expect_err("a blank reference is refused");
    assert!(error.message().contains("reference"), "{}", error.message());
}
