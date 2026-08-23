//! The security boundary, demonstrated rather than asserted.
//!
//! Every test in this file is an attack that **works**. They are here because
//! the crate documentation makes a narrow claim — inference from released
//! aggregates by an honest-but-curious participant — and the cheapest way for
//! that claim to widen into something false is for nobody to have written down
//! what it excludes in a form that runs.
//!
//! If one of these ever starts failing, the honest response is to find out what
//! changed before celebrating.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)]

use qip_confidential::NOT_DEFENDED_AGAINST;
use qip_confidential::{
    Bounds, Budget, CellId, CohortId, Contribution, ContributionSet, Epsilon, NoiseScale, Policy,
    Query, ReleaseGate, Statistic, noise_for,
};
use qip_core::error::Result;

const FLOW: [(&str, f64); 5] = [
    ("amer-1", 19.75),
    ("apac-1", 8.25),
    ("apac-2", 44.0),
    ("emea-1", 12.0),
    ("emea-2", 31.5),
];

/// The cell the other four are trying to read.
const TARGET: (&str, f64) = ("apac-2", 44.0);

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

#[test]
fn four_colluding_cells_read_the_fifth_because_a_threshold_assumes_they_do_not_talk() -> Result<()>
{
    // A cohort of five clears the default threshold with nothing to spare. But
    // four of those five know their own numbers, and subtracting what you
    // already know from a total leaves what you did not. The threshold
    // contributed nothing here; the only thing left standing is the noise.
    let known: f64 = FLOW
        .iter()
        .filter(|(cell, _)| *cell != TARGET.0)
        .map(|(_, value)| value)
        .sum();

    let epsilon = Epsilon::new(Epsilon::MAXIMUM)?;
    let policy = Policy::new(Policy::DEFAULT_MIN_CONTRIBUTORS, Budget::new(4.0)?)?;
    let query = Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        bounds()?,
        epsilon,
    )?;

    let mut estimates = Vec::with_capacity(200);
    for seed in 0..200 {
        let mut gate = ReleaseGate::new(policy, seed);
        let released = gate.release(&query, &contributions(&FLOW)?)?;
        assert_eq!(
            released.contributors(),
            5,
            "the cohort passed the threshold"
        );
        estimates.push(released.value() - known);
    }

    // The estimator is centred on the target's actual number. One colluding
    // group gets one draw of this; the sweep is how the test shows the draw is
    // an estimate of that cell and not of anything else.
    let mean = estimates.iter().sum::<f64>() / estimates.len() as f64;
    assert!(
        (mean - TARGET.1).abs() < 8.0,
        "four cells subtracting themselves should centre on the fifth: {mean} against {}",
        TARGET.1
    );
    Ok(())
}

#[test]
fn four_colluding_cells_read_a_single_bit_about_the_fifth_almost_every_time() -> Result<()> {
    // The sharper version, and the one that matters operationally: not "what is
    // that cell's exposure" but "is that cell over the limit". A count has a
    // sensitivity of one, so the noise on it is small, and subtracting four
    // known bits from a noisy count of five recovers the fifth bit outright.
    let epsilon = Epsilon::new(Epsilon::MAXIMUM)?;
    let statistic = Statistic::CountAbove { threshold: 40.0 };
    let known = FLOW
        .iter()
        .filter(|(cell, _)| *cell != TARGET.0)
        .filter(|(_, value)| *value > 40.0)
        .count() as f64;
    let truth = f64::from(u8::from(TARGET.1 > 40.0));

    let policy = Policy::new(Policy::DEFAULT_MIN_CONTRIBUTORS, Budget::new(4.0)?)?;
    let query = Query::new(CohortId::new("over-limit")?, statistic, bounds()?, epsilon)?;

    let mut correct = 0;
    for seed in 0..200 {
        let mut gate = ReleaseGate::new(policy, seed);
        let released = gate.release(&query, &contributions(&FLOW)?)?;
        if ((released.value() - known).round() - truth).abs() < f64::EPSILON {
            correct += 1;
        }
    }
    assert!(
        correct * 100 > 200 * 75,
        "the colluding four recovered the bit in {correct} of 200 periods"
    );
    Ok(())
}

#[test]
fn whoever_holds_the_seed_recovers_the_true_statistic() -> Result<()> {
    // The reproducibility the platform requires and the confidentiality this
    // crate provides meet here, and reproducibility wins: the noise is a
    // function of the seed and the question, so anyone holding both computes it
    // and subtracts it. `noise_for` is public so that this is visible rather
    // than merely true.
    let seed = 20_260_823;
    let set = contributions(&FLOW)?;
    let epsilon = Epsilon::new(0.5)?;
    let query = Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        bounds()?,
        epsilon,
    )?;

    let mut gate = ReleaseGate::new(Policy::default(), seed);
    let released = gate.release(&query, &set)?;

    let cells = set.cells().cloned().collect();
    let fingerprint = query.fingerprint(&cells);
    let sensitivity = query.statistic().sensitivity(query.bounds(), 5)?;
    let scale = NoiseScale::calibrate(sensitivity, epsilon)?;
    let recovered = released.value() - noise_for(seed, &fingerprint, scale);

    let truth: f64 = FLOW.iter().map(|(_, value)| value).sum();
    // Not approximately: the only difference is the grid the released value was
    // rounded onto, which is a millionth of the noise scale.
    assert!(
        (recovered - truth).abs() < 1e-3,
        "recovered {recovered} against a true sum of {truth}"
    );
    Ok(())
}

#[test]
fn a_cohort_where_only_one_cell_has_anything_to_report_is_still_released() -> Result<()> {
    // The threshold counts contributors, not contributions. Five cells of which
    // four report zero is arithmetically one cell's number, and this gate
    // cannot tell — refusing on that basis would make the refusal itself a
    // statement about the data, which is a leak with no budget charged against
    // it. The noise is what protects this case, and it protects it exactly as
    // well as epsilon says.
    let sparse = contributions(&[
        ("amer-1", 0.0),
        ("apac-1", 0.0),
        ("apac-2", 44.0),
        ("emea-1", 0.0),
        ("emea-2", 0.0),
    ])?;
    let query = Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        bounds()?,
        Epsilon::new(Epsilon::MAXIMUM)?,
    )?;

    let policy = Policy::new(Policy::DEFAULT_MIN_CONTRIBUTORS, Budget::new(4.0)?)?;
    let mut gate = ReleaseGate::new(policy, 9);
    let released = gate.release(&query, &sparse)?;
    assert_eq!(released.contributors(), 5);

    // And the released figure is one cell's exposure plus a draw.
    let mut within_one_scale = 0;
    for seed in 0..200 {
        let mut gate = ReleaseGate::new(policy, seed);
        let value = gate.release(&query, &sparse)?.value();
        if (value - 44.0).abs() < released.noise_scale() {
            within_one_scale += 1;
        }
    }
    assert!(
        within_one_scale * 100 > 200 * 50,
        "{within_one_scale} of 200 periods put the figure within one noise scale of the single \
         cell that reported"
    );
    Ok(())
}

#[test]
fn a_new_gate_starts_with_a_whole_budget_which_is_what_a_restart_is() -> Result<()> {
    // The ledger cannot be lowered, and it also cannot survive the process. The
    // gap is documented and this is it; `absorb` is what closes it, and only if
    // a deployment actually checkpoints.
    let query = Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        bounds()?,
        Epsilon::new(0.5)?,
    )?;
    let set = contributions(&FLOW)?;

    let mut before = ReleaseGate::new(Policy::default(), 5);
    before.release(&query, &set)?;
    assert_eq!(before.ledger().spent(&CellId::new("apac-1")?).get(), 0.5);

    let after = ReleaseGate::new(Policy::default(), 5);
    assert_eq!(after.ledger().spent(&CellId::new("apac-1")?).get(), 0.0);
    Ok(())
}

#[test]
fn the_process_that_aggregates_holds_every_raw_contribution() -> Result<()> {
    // Stated in the crate documentation and true in the type system: this is
    // not an enclave and not a multi-party protocol. The numbers are in memory,
    // readable by whatever holds them. Hiding the accessor would hide nothing
    // from anyone who mattered.
    let set = contributions(&FLOW)?;
    assert_eq!(set.value(&CellId::new(TARGET.0)?), Some(TARGET.1));
    Ok(())
}

#[test]
fn the_paths_this_does_not_defend_are_published_as_data() {
    // A limitation that lives only in a doc comment is one the person who
    // needed it did not read. This list is what an operator console or a
    // start-up check can render.
    assert_eq!(NOT_DEFENDED_AGAINST.len(), 8);
    for expected in [
        "collusion",
        "malicious operator",
        "no TLS",
        "seed",
        "tracker",
        "clients inside a cell",
        "membership",
        "over time",
    ] {
        assert!(
            NOT_DEFENDED_AGAINST
                .iter()
                .any(|entry| entry.contains(expected)),
            "the published list has stopped mentioning {expected}"
        );
    }
}
