//! The cohort threshold: an aggregate over too few cells is refused.
//!
//! Every test here works the same way — show the question being answered when
//! the cohort is large enough, then take one cell away and show the refusal —
//! so that a gate which refused everything would fail these tests rather than
//! pass them.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)]

use qip_confidential::{
    Bounds, Budget, CellId, CohortId, Contribution, ContributionSet, Epsilon, Policy, Query,
    ReleaseGate, Statistic,
};
use qip_core::error::{Error, Result};

/// Five cells and their exposures, the shape the fabric is built for.
const FLOW: [(&str, f64); 5] = [
    ("amer-1", 19.75),
    ("apac-1", 8.25),
    ("apac-2", 44.0),
    ("emea-1", 12.0),
    ("emea-2", 31.5),
];

fn contributions(rows: &[(&str, f64)]) -> Result<ContributionSet> {
    let mut set = ContributionSet::new();
    for (cell, value) in rows {
        set.insert(Contribution::new(CellId::new(*cell)?, *value)?)?;
    }
    Ok(set)
}

fn sum_query() -> Result<Query> {
    Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        Bounds::new(0.0, 100.0)?,
        Epsilon::new(0.5)?,
    )
}

fn gate() -> ReleaseGate {
    ReleaseGate::new(Policy::default(), 7)
}

#[test]
fn a_cohort_at_the_threshold_is_released_and_one_cell_short_of_it_is_refused() -> Result<()> {
    let query = sum_query()?;

    // The premise: at five contributors this question is answered, so the
    // refusal below is about the cohort size and not about the question.
    let mut permitted = gate();
    let released = permitted.release(&query, &contributions(&FLOW)?)?;
    assert_eq!(released.contributors(), 5);

    // One cell fewer, and there is no answer at all — not a wider interval, not
    // a suppressed field, nothing.
    let mut refusing = gate();
    let four = contributions(&FLOW[..4])?;
    let error = refusing
        .release(&query, &four)
        .expect_err("an aggregate over four cells is below the threshold of five");
    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
    Ok(())
}

#[test]
fn the_refusal_names_the_contributor_count_and_the_threshold_it_fell_short_of() -> Result<()> {
    let mut gate = gate();
    let error = gate
        .release(&sum_query()?, &contributions(&FLOW[..3])?)
        .expect_err("three cells is below the threshold");
    let message = error.message();
    assert!(message.contains('3'), "{message}");
    assert!(message.contains('5'), "{message}");
    assert!(
        message.contains("threshold"),
        "a refusal that does not say which control refused is a refusal nobody can act on: \
         {message}"
    );
    Ok(())
}

#[test]
fn a_cell_contributing_twice_cannot_pad_a_cohort_up_to_the_threshold() -> Result<()> {
    // The premise: a naive count of four rows plus one repeat is five, which is
    // the threshold. If the repeat were accepted, this cohort would be answered
    // and the answer would weight one cell twice.
    let mut padded = contributions(&FLOW[..4])?;
    assert_eq!(padded.contributors(), 4);
    let repeat = Contribution::new(CellId::new("emea-1")?, 12.0)?;
    let error = padded
        .insert(repeat)
        .expect_err("a cell cannot contribute twice to one release");
    assert!(error.message().contains("emea-1"), "{}", error.message());

    // And the set is still four, so the release is still refused.
    assert_eq!(padded.contributors(), 4);
    let mut gate = gate();
    assert!(gate.release(&sum_query()?, &padded).is_err());
    Ok(())
}

#[test]
fn a_threshold_that_could_not_gate_anything_is_refused_when_the_policy_is_built() -> Result<()> {
    // Two contributors is not a cohort: either one subtracts its own number.
    let error =
        Policy::new(2, Budget::default()).expect_err("a threshold of two gates nothing at all");
    assert!(error.message().contains('2') || error.message().contains("threshold"));

    // Three is the smallest thing that is not self-evidently pointless, and it
    // is accepted — so the check above is about the number and not about
    // `Policy::new` refusing everything.
    assert_eq!(Policy::new(3, Budget::default())?.min_contributors(), 3);
    Ok(())
}

#[test]
fn the_default_threshold_is_five_which_is_why_four_cells_are_not_enough() {
    assert_eq!(Policy::DEFAULT_MIN_CONTRIBUTORS, 5);
    assert_eq!(Policy::default().min_contributors(), 5);
    assert_eq!(Policy::MINIMUM_CONTRIBUTORS, 3);
}

#[test]
fn a_lower_threshold_answers_the_question_the_default_refuses() -> Result<()> {
    // The threshold is a policy parameter, and this is what setting it does. A
    // deployment with more contributors, or a lower appetite, moves this number
    // and everything downstream — including the differencing gate, which reads
    // the same number — moves with it.
    let mut permissive = ReleaseGate::new(Policy::new(3, Budget::default())?, 7);
    let released = permissive.release(&sum_query()?, &contributions(&FLOW[..3])?)?;
    assert_eq!(released.contributors(), 3);
    Ok(())
}

#[test]
fn an_empty_cohort_is_refused_before_anything_is_computed() -> Result<()> {
    let mut gate = gate();
    let error = gate
        .release(&sum_query()?, &ContributionSet::new())
        .expect_err("there is nothing to aggregate");
    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
    Ok(())
}
