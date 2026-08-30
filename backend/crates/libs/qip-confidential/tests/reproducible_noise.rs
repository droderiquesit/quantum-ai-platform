//! Noise that is calibrated, seeded and reproducible.
//!
//! Two properties are being held at once and they pull against each other. The
//! platform requires that a replay of the same inputs produces byte-identical
//! output, which means the noise cannot be secret from anyone holding the seed.
//! Confidentiality requires that the noise cannot be averaged away, which is
//! why it is a function of the question rather than of a running stream. These
//! tests pin both, and `boundary.rs` demonstrates what the first one costs.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)]

use qip_confidential::release::ReleaseGate;
use qip_confidential::{
    Bounds, CellId, CohortId, Contribution, ContributionSet, Epsilon, NoiseScale, Policy, Query,
    Sensitivity, Statistic, noise_for, snap,
};
use qip_core::error::Result;

const FLOW: [(&str, f64); 5] = [
    ("amer-1", 19.75),
    ("apac-1", 8.25),
    ("apac-2", 44.0),
    ("emea-1", 12.0),
    ("emea-2", 31.5),
];

/// The true sum of the five contributions above: 115.5.
const TRUE_SUM: f64 = 115.5;

/// The exact figure the fabric releases for the sum of `FLOW` at seed
/// 20_260_823, epsilon 0.5 and bounds [0, 100].
///
/// Pinned as a literal on purpose. A test that only checks the value is "near"
/// the truth would pass just as happily against an implementation that had
/// stopped adding noise, or against one whose stream had silently changed with
/// a compiler version. This is the number, and it does not move.
const RELEASED_SUM_AT_SEED: f64 = -123.968_139_648_437_5;

fn contributions(rows: &[(&str, f64)]) -> Result<ContributionSet> {
    let mut set = ContributionSet::new();
    for (cell, value) in rows {
        set.insert(Contribution::new(CellId::new(*cell)?, *value)?)?;
    }
    Ok(set)
}

fn sum_query(epsilon: f64) -> Result<Query> {
    Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        Bounds::new(0.0, 100.0)?,
        Epsilon::new(epsilon)?,
    )
}

#[test]
fn the_released_value_is_exactly_what_the_seed_says_it_is() -> Result<()> {
    let mut gate = ReleaseGate::new(Policy::default(), 20_260_823);
    let released = gate.release(&sum_query(0.5)?, &contributions(&FLOW)?)?;
    assert_eq!(released.value(), RELEASED_SUM_AT_SEED);
    assert_eq!(released.noise_scale(), 200.0);
    Ok(())
}

#[test]
fn two_gates_with_the_same_seed_release_the_same_number_and_a_different_seed_does_not() -> Result<()>
{
    let set = contributions(&FLOW)?;
    let query = sum_query(0.5)?;

    let mut one = ReleaseGate::new(Policy::default(), 20_260_823);
    let mut same = ReleaseGate::new(Policy::default(), 20_260_823);
    let mut other = ReleaseGate::new(Policy::default(), 20_260_824);

    let a = one.release(&query, &set)?.value();
    let b = same.release(&query, &set)?.value();
    let c = other.release(&query, &set)?.value();

    assert_eq!(
        a, b,
        "the same seed and the same question is the same answer"
    );
    assert_ne!(a, c, "a different observation period draws its own noise");
    Ok(())
}

#[test]
fn the_order_the_cells_reported_in_does_not_change_the_release() -> Result<()> {
    // Floating-point addition is not associative. Without a fixed iteration
    // order the same five numbers, submitted in a different order, would sum to
    // a value differing in the last bits — and the platform would be unable to
    // reproduce its own audit trail from its own inputs.
    let forwards = contributions(&FLOW)?;
    let mut backwards_rows = FLOW.to_vec();
    backwards_rows.reverse();
    let backwards = contributions(&backwards_rows)?;

    let query = sum_query(0.5)?;
    let mut one = ReleaseGate::new(Policy::default(), 5);
    let mut two = ReleaseGate::new(Policy::default(), 5);

    assert_eq!(
        one.release(&query, &forwards)?.value(),
        two.release(&query, &backwards)?.value()
    );
    Ok(())
}

#[test]
fn relabelling_the_cohort_does_not_buy_a_second_noise_draw() -> Result<()> {
    // If the label were part of the question's identity, this would be the
    // cheapest attack in the file: ask twice under two names, average the two
    // answers, and the noise falls by a third.
    let set = contributions(&FLOW)?;
    let bounds = Bounds::new(0.0, 100.0)?;
    let epsilon = Epsilon::new(0.5)?;
    let desk = Query::new(CohortId::new("desk-view")?, Statistic::Sum, bounds, epsilon)?;
    let risk = Query::new(CohortId::new("risk-view")?, Statistic::Sum, bounds, epsilon)?;

    // Across two gates on the same seed: the same number.
    let mut one = ReleaseGate::new(Policy::default(), 11);
    let mut two = ReleaseGate::new(Policy::default(), 11);
    assert_eq!(
        one.release(&desk, &set)?.value(),
        two.release(&risk, &set)?.value()
    );

    // Within one gate: the same release, charged once, still filed under the
    // label it was first asked under.
    let mut gate = ReleaseGate::new(Policy::default(), 11);
    let first = gate.release(&desk, &set)?;
    let second = gate.release(&risk, &set)?;
    assert_eq!(second.id(), first.id());
    assert_eq!(second.cohort().as_str(), "desk-view");
    assert_eq!(gate.ledger().spent(&CellId::new("apac-1")?).get(), 0.5);
    Ok(())
}

#[test]
fn sweeping_epsilon_draws_fresh_noise_rather_than_rescaling_one_draw() -> Result<()> {
    // Epsilon is part of the question's identity, and this is why. If one draw
    // were reused at two scales, a caller would hold `truth + b1*Z` and
    // `truth + b2*Z`: two linear equations in two unknowns, and the truth falls
    // out exactly.
    let set = contributions(&FLOW)?;
    let mut one = ReleaseGate::new(Policy::default(), 77);
    let mut two = ReleaseGate::new(Policy::default(), 77);

    let coarse = one.release(&sum_query(0.5)?, &set)?;
    let fine = two.release(&sum_query(1.0)?, &set)?;

    let noise_coarse = coarse.value() - TRUE_SUM;
    let noise_fine = fine.value() - TRUE_SUM;
    assert_eq!(coarse.noise_scale(), 200.0);
    assert_eq!(fine.noise_scale(), 100.0);

    // The scales are exactly 2:1. If the draw were shared, the noises would be
    // too — solving for the truth would then need no guesswork at all.
    let rescaled = noise_coarse / 2.0;
    assert!(
        (rescaled - noise_fine).abs() > 1e-6,
        "the two releases share a noise draw: {noise_coarse} and {noise_fine}"
    );

    // What stops the sweep is the ledger, not the noise: each step of it is a
    // different question and each is charged. (Each step has to be a different
    // epsilon for that reason — repeating one is free, and returns the answer
    // already given.)
    let mut gate = ReleaseGate::new(Policy::default(), 77);
    gate.release(&sum_query(0.5)?, &set)?;
    gate.release(&sum_query(0.25)?, &set)?;
    gate.release(&sum_query(0.2)?, &set)?;
    assert!(gate.release(&sum_query(0.1)?, &set).is_err());
    Ok(())
}

#[test]
fn the_noise_added_has_the_scale_the_release_declares() -> Result<()> {
    // A Laplace draw of scale b has mean zero and mean absolute value b. Four
    // thousand independent seeds, one draw each, is enough to tell b from 2b
    // or from zero by a wide margin.
    let scale = NoiseScale::calibrate(Sensitivity::new(100.0)?, Epsilon::new(0.5)?)?;
    assert_eq!(scale.get(), 200.0);
    assert_eq!(scale.standard_deviation(), 200.0 * std::f64::consts::SQRT_2);

    let query = sum_query(0.5)?;
    let cells = contributions(&FLOW)?.cells().cloned().collect();
    let fingerprint = query.fingerprint(&cells);

    let draws: Vec<f64> = (0..4_000)
        .map(|seed| noise_for(seed, &fingerprint, scale))
        .collect();
    let mean = draws.iter().sum::<f64>() / draws.len() as f64;
    let mean_absolute = draws.iter().map(|x| x.abs()).sum::<f64>() / draws.len() as f64;

    assert!(
        mean.abs() < 0.15 * scale.get(),
        "the noise is biased: mean {mean} against a scale of {}",
        scale.get()
    );
    assert!(
        (mean_absolute - scale.get()).abs() < 0.10 * scale.get(),
        "the noise is not the scale the release declares: mean |x| {mean_absolute} against {}",
        scale.get()
    );
    Ok(())
}

#[test]
fn the_released_value_is_not_the_true_value() -> Result<()> {
    // The premise for everything else: something is actually added. A release
    // that happened to equal the truth would satisfy most of this file.
    let set = contributions(&FLOW)?;
    let mut moved = 0;
    for seed in 0..64 {
        let mut gate = ReleaseGate::new(Policy::default(), seed);
        if gate.release(&sum_query(0.5)?, &set)?.value() != TRUE_SUM {
            moved += 1;
        }
    }
    assert_eq!(moved, 64, "some releases came back exactly true");
    Ok(())
}

#[test]
fn a_release_reports_the_spread_a_reader_should_quote_with_it() -> Result<()> {
    let mut gate = ReleaseGate::new(Policy::default(), 3);
    let released = gate.release(&sum_query(0.5)?, &contributions(&FLOW)?)?;
    assert_eq!(
        released.standard_deviation(),
        released.noise_scale() * std::f64::consts::SQRT_2
    );
    // At five contributors the honest spread on a sum is comparable to the sum
    // itself. That is the cost of the guarantee, and it is on the record rather
    // than in a footnote.
    assert!(released.standard_deviation() > TRUE_SUM);
    Ok(())
}

#[test]
fn a_mean_is_noised_by_the_range_over_the_contributor_count() -> Result<()> {
    // The sensitivity of each statistic, checked where it is visible: a mean
    // over five cells moves five times less than the sum does when one cell
    // changes, so it is noised five times less.
    let set = contributions(&FLOW)?;
    let bounds = Bounds::new(0.0, 100.0)?;
    let epsilon = Epsilon::new(0.5)?;
    let cohort = CohortId::new("global-net-exposure")?;

    let mut gate = ReleaseGate::new(Policy::default(), 3);
    let mean = gate.release(
        &Query::new(cohort.clone(), Statistic::Mean, bounds, epsilon)?,
        &set,
    )?;
    assert_eq!(mean.noise_scale(), 40.0);

    let count = gate.release(
        &Query::new(
            cohort,
            Statistic::CountAbove { threshold: 20.0 },
            bounds,
            epsilon,
        )?,
        &set,
    )?;
    assert_eq!(count.noise_scale(), 2.0);
    Ok(())
}

#[test]
fn a_release_can_be_reproduced_end_to_end_from_the_seed_and_the_inputs() -> Result<()> {
    // What replay means here: given the seed, the question and the same
    // contributions, every step of the released figure can be recomputed by a
    // third party — the clamped truth, the draw, and the grid it was rounded
    // onto. Nothing in the path is hidden state.
    let seed = 20_260_823;
    let set = contributions(&FLOW)?;
    let query = sum_query(0.5)?;

    let mut gate = ReleaseGate::new(Policy::default(), seed);
    let released = gate.release(&query, &set)?;

    let cells = set.cells().cloned().collect();
    let fingerprint = query.fingerprint(&cells);
    let scale = NoiseScale::calibrate(Sensitivity::new(100.0)?, Epsilon::new(0.5)?)?;
    let recomputed = snap(TRUE_SUM + noise_for(seed, &fingerprint, scale), scale);

    assert_eq!(recomputed, released.value());
    assert_eq!(recomputed, RELEASED_SUM_AT_SEED);
    Ok(())
}
