//! The privacy budget: a ledger that only goes up, and stops answering.
//!
//! The attack these tests are about is not one query. It is a thousand:
//! "mean exposure" asked over and over with slightly different filters until
//! the answers determine the rows. Nothing about any single answer is wrong.
//! The only thing that can stop it is something that remembers what has already
//! been given away.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)]

use qip_confidential::{
    Bounds, Budget, CellId, CohortId, Contribution, ContributionSet, Epsilon, Policy, Query,
    ReleaseGate, Statistic,
};
use qip_core::error::{Error, Result};

const FLOW: [(&str, f64); 5] = [
    ("amer-1", 19.75),
    ("apac-1", 8.25),
    ("apac-2", 44.0),
    ("emea-1", 12.0),
    ("emea-2", 31.5),
];

const SECOND_REGION: [(&str, f64); 5] = [
    ("amer-2", 22.0),
    ("amer-3", 6.5),
    ("emea-3", 41.25),
    ("emea-4", 3.0),
    ("latam-1", 17.5),
];

fn contributions(rows: &[(&str, f64)]) -> Result<ContributionSet> {
    let mut set = ContributionSet::new();
    for (cell, value) in rows {
        set.insert(Contribution::new(CellId::new(*cell)?, *value)?)?;
    }
    Ok(set)
}

fn bounds() -> Result<Bounds> {
    Bounds::new(0.0, 100.0)
}

fn cohort() -> Result<CohortId> {
    CohortId::new("global-net-exposure")
}

/// A family of questions that differ only in a filter — the shape of the
/// attack. Each is a genuinely different measurement, so each honestly gets
/// fresh noise; the budget is the only thing that ends the sequence.
fn count_above(threshold: f64, epsilon: f64) -> Result<Query> {
    Query::new(
        cohort()?,
        Statistic::CountAbove { threshold },
        bounds()?,
        Epsilon::new(epsilon)?,
    )
}

fn gate() -> ReleaseGate {
    ReleaseGate::new(Policy::default(), 31)
}

#[test]
fn a_release_charges_every_cell_that_contributed_to_it() -> Result<()> {
    let mut gate = gate();
    let set = contributions(&FLOW)?;
    for (cell, _) in FLOW {
        assert_eq!(gate.ledger().spent(&CellId::new(cell)?).get(), 0.0);
    }

    gate.release(&count_above(20.0, 0.25)?, &set)?;

    // Every cell, not just the ones whose number moved the answer: a cell that
    // reported below the threshold is as identifiable from the count as one
    // that reported above it.
    for (cell, _) in FLOW {
        let cell = CellId::new(cell)?;
        assert_eq!(gate.ledger().spent(&cell).get(), 0.25);
        assert_eq!(gate.ledger().releases(&cell), 1);
        assert_eq!(gate.ledger().remaining(&cell), 0.75);
    }
    Ok(())
}

#[test]
fn asking_the_same_family_of_questions_until_the_budget_is_gone_ends_in_a_refusal() -> Result<()> {
    let mut gate = gate();
    let set = contributions(&FLOW)?;

    // Four questions at a quarter of the budget each. All four are answered —
    // the premise, without which the refusal below would prove nothing.
    for (index, threshold) in [10.0, 20.0, 30.0, 40.0].into_iter().enumerate() {
        let release = gate.release(&count_above(threshold, 0.25)?, &set)?;
        assert_eq!(release.id().get(), index as u64 + 1);
    }
    assert_eq!(
        gate.ledger().spent(&CellId::new("apac-1")?).get(),
        Budget::DEFAULT
    );

    // The fifth is refused. Not answered with more noise, not answered with a
    // warning: refused, with the reason.
    let error = gate
        .release(&count_above(50.0, 0.25)?, &set)
        .expect_err("the budget is spent");
    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
    let message = error.message();
    assert!(message.contains("budget"), "{message}");
    assert!(
        message.contains("does not reset"),
        "the refusal should say the obvious next move is not available: {message}"
    );
    Ok(())
}

#[test]
fn there_is_no_call_that_lowers_a_spend_so_the_only_way_out_is_a_new_ledger() -> Result<()> {
    let mut gate = gate();
    let set = contributions(&FLOW)?;
    gate.release(&count_above(10.0, 0.5)?, &set)?;
    let after_one = gate.ledger().spent(&CellId::new("apac-1")?).get();

    // Absorbing this gate's own report — the only mutator besides a release —
    // leaves it exactly where it was.
    let report = gate.ledger().report();
    gate.absorb(&report);
    assert_eq!(
        gate.ledger().spent(&CellId::new("apac-1")?).get(),
        after_one
    );

    // And absorbing a report from a *fresh* ledger, which records nothing,
    // cannot take the figure back to zero.
    let empty = ReleaseGate::new(Policy::default(), 1);
    gate.absorb(&empty.ledger().report());
    assert_eq!(
        gate.ledger().spent(&CellId::new("apac-1")?).get(),
        after_one
    );
    Ok(())
}

#[test]
fn a_refused_release_spends_nothing() -> Result<()> {
    let mut gate = gate();
    let four = contributions(&FLOW[..4])?;

    assert!(gate.release(&count_above(20.0, 0.25)?, &four).is_err());

    for (cell, _) in FLOW {
        assert_eq!(gate.ledger().spent(&CellId::new(cell)?).get(), 0.0);
    }
    assert!(gate.records().is_empty());
    Ok(())
}

#[test]
fn asking_the_identical_question_twice_returns_the_first_release_and_charges_once() -> Result<()> {
    let mut gate = gate();
    let set = contributions(&FLOW)?;
    let query = count_above(20.0, 0.25)?;

    let first = gate.release(&query, &set)?;
    let second = gate.release(&query, &set)?;

    assert_eq!(first, second, "the same question is the same answer");
    assert_eq!(second.id(), first.id(), "and it is the same release");
    assert_eq!(gate.records().len(), 1);
    assert_eq!(gate.ledger().spent(&CellId::new("apac-1")?).get(), 0.25);
    assert_eq!(gate.ledger().releases(&CellId::new("apac-1")?), 1);
    Ok(())
}

#[test]
fn the_account_is_the_cell_and_the_cohort_total_is_only_reported() -> Result<()> {
    // Two questions over the same cells, filed under two different labels. If
    // the budget were kept per cohort, each label would show a quarter spent
    // and the cells would have paid half with nothing to show it.
    let mut gate = gate();
    let set = contributions(&FLOW)?;
    let desk = CohortId::new("desk-view")?;
    let risk = CohortId::new("risk-view")?;

    gate.release(
        &Query::new(desk.clone(), Statistic::Sum, bounds()?, Epsilon::new(0.25)?)?,
        &set,
    )?;
    gate.release(
        &Query::new(
            risk.clone(),
            Statistic::Mean,
            bounds()?,
            Epsilon::new(0.25)?,
        )?,
        &set,
    )?;

    let cell = CellId::new("apac-1")?;
    assert_eq!(gate.ledger().spent(&cell).get(), 0.5, "the cell paid twice");
    assert_eq!(gate.ledger().spent_on_cohort(&desk).get(), 0.25);
    assert_eq!(gate.ledger().spent_on_cohort(&risk).get(), 0.25);
    Ok(())
}

#[test]
fn a_question_is_refused_when_any_one_of_its_cells_is_spent_and_the_refusal_names_them()
-> Result<()> {
    let mut gate = gate();
    let five = contributions(&FLOW)?;
    let mut ten_rows = FLOW.to_vec();
    ten_rows.extend_from_slice(&SECOND_REGION);
    let ten = contributions(&ten_rows)?;

    // The five original cells pay twice: once for their own cohort, once for
    // the wider one. The five new cells pay once.
    gate.release(&count_above(20.0, 0.5)?, &five)?;
    gate.release(&count_above(20.0, 0.5)?, &ten)?;
    assert_eq!(gate.ledger().spent(&CellId::new("apac-1")?).get(), 1.0);
    assert_eq!(gate.ledger().spent(&CellId::new("latam-1")?).get(), 0.5);

    // A further question over all ten is refused because five of them are
    // spent, even though the other five could afford it. There is no partial
    // answer: an aggregate missing the exhausted cells is a different cohort,
    // and the caller has to be told which cells stopped it.
    let error = gate
        .release(&count_above(30.0, 0.5)?, &ten)
        .expect_err("half the cohort is out of budget");
    let message = error.message();
    assert!(message.contains("5 of 10"), "{message}");
    assert!(message.contains("apac-1"), "{message}");
    assert!(!message.contains("latam-1"), "{message}");
    Ok(())
}

#[test]
fn a_spend_report_folded_into_another_ledger_raises_figures_and_never_lowers_them() -> Result<()> {
    // The checkpoint case: a gate is rebuilt after a restart and the recorded
    // spend is folded back in.
    let mut spent_a_lot = gate();
    let set = contributions(&FLOW)?;
    spent_a_lot.release(&count_above(10.0, 0.5)?, &set)?;
    spent_a_lot.release(&count_above(20.0, 0.25)?, &set)?;
    let checkpoint = spent_a_lot.ledger().report();

    let mut restarted = ReleaseGate::new(Policy::default(), 31);
    assert_eq!(restarted.ledger().spent(&CellId::new("apac-1")?).get(), 0.0);
    restarted.absorb(&checkpoint);
    assert_eq!(
        restarted.ledger().spent(&CellId::new("apac-1")?).get(),
        0.75
    );

    // A stale checkpoint cannot undo a later spend.
    restarted.release(&count_above(30.0, 0.25)?, &set)?;
    assert_eq!(restarted.ledger().spent(&CellId::new("apac-1")?).get(), 1.0);
    restarted.absorb(&checkpoint);
    assert_eq!(restarted.ledger().spent(&CellId::new("apac-1")?).get(), 1.0);
    Ok(())
}

#[test]
fn an_epsilon_small_enough_to_vanish_in_the_running_total_is_refused() -> Result<()> {
    // Without the floor this is the cheapest way to empty the fabric: ask a
    // billion questions, each priced below the resolution of the accumulator,
    // and the ledger never moves.
    let error = Epsilon::new(1e-12).expect_err("an epsilon below the floor is not chargeable");
    assert!(error.message().contains("floor"), "{}", error.message());

    // The floor itself is chargeable, so the refusal above is about the size.
    let floor = Epsilon::new(Epsilon::MINIMUM)?;
    let mut gate = gate();
    gate.release(
        &Query::new(cohort()?, Statistic::Sum, bounds()?, floor)?,
        &contributions(&FLOW)?,
    )?;
    assert_eq!(
        gate.ledger().spent(&CellId::new("apac-1")?).get(),
        Epsilon::MINIMUM
    );
    Ok(())
}

#[test]
fn an_epsilon_large_enough_to_make_the_noise_decorative_is_refused() {
    let error = Epsilon::new(Epsilon::MAXIMUM * 2.0).expect_err("above the ceiling");
    assert!(error.message().contains("ceiling"), "{}", error.message());
    assert!(Epsilon::new(Epsilon::MAXIMUM).is_ok());
}

#[test]
fn a_budget_that_would_not_bound_anything_is_refused_when_it_is_built() {
    assert!(Budget::new(Budget::MAXIMUM * 10.0).is_err());
    assert!(Budget::new(f64::INFINITY).is_err());
    assert!(Budget::new(0.0).is_err());
    assert!(Budget::new(Budget::DEFAULT).is_ok());
}

#[test]
fn the_planning_check_agrees_with_the_release_and_costs_nothing_to_ask() -> Result<()> {
    let mut gate = gate();
    let set = contributions(&FLOW)?;
    let query = count_above(20.0, 0.5)?;

    assert!(gate.admits(&query, &set).is_ok());
    gate.release(&query, &set)?;
    gate.release(&count_above(30.0, 0.5)?, &set)?;

    // Out of budget: `admits` says so without changing anything.
    let further = count_above(40.0, 0.5)?;
    assert!(gate.admits(&further, &set).is_err());
    assert_eq!(gate.ledger().spent(&CellId::new("apac-1")?).get(), 1.0);
    assert_eq!(gate.records().len(), 2);
    Ok(())
}
