//! Differencing: the attack the cohort threshold does not stop on its own.
//!
//! Ask about eight cells, ask about seven of the same eight, subtract. Both
//! questions are comfortably above any threshold and the difference is one
//! cell's number. Every test here starts by showing that the subtraction really
//! does recover the cell — on the raw values, where nothing is hidden — before
//! showing what the gate does about it.
//!
//! The last two tests show where the defence ends. They are not aspirational
//! and they are not disabled: they demonstrate attacks that work.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)]

use qip_confidential::{
    Bounds, Budget, CellId, CohortId, Contribution, ContributionSet, Epsilon, Policy, Query,
    ReleaseGate, Statistic,
};
use qip_core::error::{Error, Result};

/// Nine cells. Enough to build a tracker out of, which the last test needs.
const BOOK: [(&str, f64); 9] = [
    ("amer-1", 19.75),
    ("amer-2", 22.0),
    ("amer-3", 6.5),
    ("apac-1", 8.25),
    ("apac-2", 44.0),
    ("emea-1", 12.0),
    ("emea-2", 31.5),
    ("emea-3", 41.25),
    ("latam-1", 63.0),
];

fn contributions(rows: &[(&str, f64)]) -> Result<ContributionSet> {
    let mut set = ContributionSet::new();
    for (cell, value) in rows {
        set.insert(Contribution::new(CellId::new(*cell)?, *value)?)?;
    }
    Ok(set)
}

fn sum_over(cohort: &str) -> Result<Query> {
    Query::new(
        CohortId::new(cohort)?,
        Statistic::Sum,
        Bounds::new(0.0, 100.0)?,
        Epsilon::new(0.25)?,
    )
}

/// A threshold of three, so that a five-cell and a six-cell cohort are each
/// individually answerable. Without that, a refusal below could be the cohort
/// threshold doing the work and the differencing gate doing nothing.
fn permissive() -> Result<Policy> {
    Policy::new(3, Budget::new(2.0)?)
}

#[test]
fn subtracting_two_nested_releases_would_hand_over_one_cell_and_is_therefore_refused() -> Result<()>
{
    let six = &BOOK[..6];
    let five = &BOOK[..5];
    let target = BOOK[5];

    // The premise, on the raw numbers: the subtraction is exact. This is what
    // the gate has to stop, and it is arithmetic, not a vulnerability in the
    // arithmetic.
    let sum_six: f64 = six.iter().map(|(_, value)| value).sum();
    let sum_five: f64 = five.iter().map(|(_, value)| value).sum();
    assert_eq!(sum_six - sum_five, target.1);

    // The second premise: five cells is a perfectly answerable question. A
    // fresh gate answers it, so the refusal below is not the cohort threshold.
    let mut fresh = ReleaseGate::new(permissive()?, 41);
    assert!(
        fresh
            .release(&sum_over("subset")?, &contributions(five)?)
            .is_ok()
    );

    // Now both, in one gate. The first is released; the second would complete
    // the pair, and is refused.
    let mut gate = ReleaseGate::new(permissive()?, 41);
    gate.release(&sum_over("wide")?, &contributions(six)?)?;
    let error = gate
        .release(&sum_over("subset")?, &contributions(five)?)
        .expect_err("the difference of the two releases is one cell's number");
    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
    Ok(())
}

#[test]
fn the_refusal_names_the_release_it_would_have_been_differenced_against_and_the_cell_in_the_gap()
-> Result<()> {
    let mut gate = ReleaseGate::new(permissive()?, 41);
    let first = gate.release(&sum_over("wide")?, &contributions(&BOOK[..6])?)?;

    let error = gate
        .release(&sum_over("subset")?, &contributions(&BOOK[..5])?)
        .expect_err("nested cohorts");
    let message = error.message();
    assert!(
        message.contains(&first.id().to_string()),
        "a caller cannot audit a refusal that does not name the release it collided with: \
         {message}"
    );
    assert!(
        message.contains("emea-1"),
        "the cell in the gap is the whole point of the refusal: {message}"
    );
    assert!(message.contains("threshold"), "{message}");
    Ok(())
}

#[test]
fn two_cohorts_far_enough_apart_are_both_released() -> Result<()> {
    // The vacuity guard. A gate that refused every second question would pass
    // every other test in this file.
    let mut gate = ReleaseGate::new(permissive()?, 41);
    let first = gate.release(&sum_over("amer-and-apac")?, &contributions(&BOOK[..5])?)?;
    let second = gate.release(&sum_over("emea-and-latam")?, &contributions(&BOOK[5..])?)?;

    assert_eq!(first.contributors(), 5);
    assert_eq!(second.contributors(), 4);
    assert_eq!(gate.records().len(), 2);

    // Their difference is an aggregate over all nine cells, which isolates
    // nobody — which is exactly why it is allowed.
    Ok(())
}

#[test]
fn a_second_statistic_over_the_same_cells_is_allowed_because_its_difference_isolates_nobody()
-> Result<()> {
    // Two questions about the *same* cells cannot be differenced into a
    // statement about one cell: the difference is a comparison of two
    // measurements over the whole cohort. The gate has to allow this, or the
    // fabric answers one question per cohort ever and is useless.
    let mut gate = ReleaseGate::new(permissive()?, 41);
    let cells = contributions(&BOOK[..6])?;
    let bounds = Bounds::new(0.0, 100.0)?;
    let epsilon = Epsilon::new(0.25)?;
    let cohort = CohortId::new("wide")?;

    gate.release(
        &Query::new(cohort.clone(), Statistic::Sum, bounds, epsilon)?,
        &cells,
    )?;
    gate.release(
        &Query::new(cohort.clone(), Statistic::Mean, bounds, epsilon)?,
        &cells,
    )?;
    gate.release(
        &Query::new(
            cohort,
            Statistic::CountAbove { threshold: 20.0 },
            bounds,
            epsilon,
        )?,
        &cells,
    )?;

    assert_eq!(gate.records().len(), 3);
    Ok(())
}

#[test]
fn the_gate_compares_cell_sets_and_not_the_labels_they_were_asked_under() -> Result<()> {
    // Cohort labels are chosen by the caller. If the gate scoped its history by
    // label, escaping it would cost one rename.
    let mut gate = ReleaseGate::new(permissive()?, 41);
    gate.release(&sum_over("risk-view")?, &contributions(&BOOK[..6])?)?;

    let error = gate
        .release(
            &sum_over("a-completely-different-cohort")?,
            &contributions(&BOOK[..5])?,
        )
        .expect_err("renaming the question does not change which cells it is about");
    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
    Ok(())
}

#[test]
fn the_gate_refuses_the_wider_cohort_after_the_narrower_one_as_well() -> Result<()> {
    // Order does not matter: the pair is what is dangerous, not the sequence.
    let mut gate = ReleaseGate::new(permissive()?, 41);
    gate.release(&sum_over("subset")?, &contributions(&BOOK[..5])?)?;
    assert!(
        gate.release(&sum_over("wide")?, &contributions(&BOOK[..6])?)
            .is_err()
    );
    Ok(())
}

#[test]
fn a_second_gate_has_no_memory_of_the_first_and_will_answer_the_other_half_of_the_pair()
-> Result<()> {
    // A documented limit, demonstrated rather than described: the history is
    // the gate's own. Two aggregators, or one that restarted, will between them
    // answer both halves of a differencing pair.
    let mut first = ReleaseGate::new(permissive()?, 41);
    first.release(&sum_over("wide")?, &contributions(&BOOK[..6])?)?;

    let mut second = ReleaseGate::new(permissive()?, 41);
    let other_half = second.release(&sum_over("subset")?, &contributions(&BOOK[..5])?)?;

    assert_eq!(other_half.contributors(), 5);
    // And because the seed is the same, the two answers are as clean a
    // differencing pair as if one gate had answered both.
    Ok(())
}

#[test]
fn the_pairwise_gate_does_not_stop_a_three_query_tracker() -> Result<()> {
    // The classic tracker, and it works. Ask about all nine cells; then about
    // two disjoint groups that between them cover everything except one cell.
    // Every pair of those three questions differs by five or eight cells, so
    // every pair passes the gate — and the three answers together isolate the
    // one cell that was left out.
    //
    // Run at the epsilon ceiling, where the noise is smallest. That is not the
    // point: the point is that the gate does not refuse the attack at any
    // epsilon, and epsilon only decides how sharp the recovered fact is.
    let target = ("apac-2", 44.0);
    let group_one = [BOOK[0], BOOK[1], BOOK[2], BOOK[3]];
    let group_two = [BOOK[5], BOOK[6], BOOK[7], BOOK[8]];
    assert_eq!(BOOK[4], target);

    let bounds = Bounds::new(0.0, 100.0)?;
    let epsilon = Epsilon::new(Epsilon::MAXIMUM)?;
    let statistic = Statistic::CountAbove { threshold: 40.0 };
    // The fact being stolen: is this one cell above the limit? It is.
    let truth = f64::from(u8::from(target.1 > 40.0));

    // The premise, on the raw counts: the three-way subtraction is exact.
    let count = |rows: &[(&str, f64)]| -> f64 {
        rows.iter().filter(|(_, value)| *value > 40.0).count() as f64
    };
    assert_eq!(count(&BOOK) - count(&group_one) - count(&group_two), truth);

    // The attack, against the gate, over two hundred observation periods. Each
    // period is one attacker's one shot; the sweep is how this test shows the
    // recovered figure is a real estimate of one cell's bit rather than luck.
    let mut recovered = Vec::with_capacity(200);
    for seed in 0..200 {
        let mut gate = ReleaseGate::new(Policy::new(3, Budget::new(8.0)?)?, seed);
        let everything = gate.release(
            &Query::new(CohortId::new("all")?, statistic, bounds, epsilon)?,
            &contributions(&BOOK)?,
        )?;
        let one = gate.release(
            &Query::new(CohortId::new("group-one")?, statistic, bounds, epsilon)?,
            &contributions(&group_one)?,
        )?;
        let two = gate.release(
            &Query::new(CohortId::new("group-two")?, statistic, bounds, epsilon)?,
            &contributions(&group_two)?,
        )?;
        recovered.push(everything.value() - one.value() - two.value());
    }

    // Nothing was refused: three thousand cells' worth of budget was spent and
    // not one of the six hundred releases was withheld.
    assert_eq!(recovered.len(), 200);

    let mean = recovered.iter().sum::<f64>() / recovered.len() as f64;
    assert!(
        (mean - truth).abs() < 0.15,
        "the tracker's estimate should centre on the cell's true bit: {mean} against {truth}"
    );

    let correct = recovered
        .iter()
        .filter(|estimate| (estimate.round() - truth).abs() < f64::EPSILON)
        .count();
    assert!(
        correct * 100 > recovered.len() * 55,
        "the tracker recovered the bit in {correct} of {} periods, which should be well above the \
         hundred a coin would manage",
        recovered.len()
    );
    Ok(())
}
