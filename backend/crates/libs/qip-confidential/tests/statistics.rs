//! What each statistic means, and what the declared range does to it.
//!
//! The noise calibration is only as good as the sensitivity it is computed
//! from, and the sensitivity is only bounded because contributions are clamped
//! into a range somebody declared in advance. These tests pin that chain.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)]

use qip_confidential::{
    Bounds, CellId, CohortId, Contribution, ContributionSet, Epsilon, NoiseScale, Policy, Query,
    ReleaseGate, Statistic, noise_for,
};
use qip_core::error::Result;

fn contributions(rows: &[(&str, f64)]) -> Result<ContributionSet> {
    let mut set = ContributionSet::new();
    for (cell, value) in rows {
        set.insert(Contribution::new(CellId::new(*cell)?, *value)?)?;
    }
    Ok(set)
}

/// The exact statistic behind a release, obtained by subtracting the noise the
/// seed determines. Available to a test for the same reason it is available to
/// an attacker holding the seed — see `boundary.rs`.
fn truth_behind(gate_seed: u64, query: &Query, set: &ContributionSet, value: f64) -> Result<f64> {
    let cells = set.cells().cloned().collect();
    let fingerprint = query.fingerprint(&cells);
    let sensitivity = query
        .statistic()
        .sensitivity(query.bounds(), set.contributors())?;
    let scale = NoiseScale::calibrate(sensitivity, query.epsilon())?;
    Ok(value - noise_for(gate_seed, &fingerprint, scale))
}

#[test]
fn a_contribution_outside_the_declared_range_is_clamped_rather_than_widening_the_range()
-> Result<()> {
    // A cell reports 120 against a range that tops out at 50. The range wins:
    // widening it to fit would mean calibrating the noise from the largest
    // contribution, which is the single most disclosive number in the set.
    let rows = [
        ("amer-1", 10.0),
        ("apac-1", 20.0),
        ("apac-2", 120.0),
        ("emea-1", 5.0),
        ("emea-2", 15.0),
    ];
    let set = contributions(&rows)?;
    let raw: f64 = rows.iter().map(|(_, value)| value).sum();
    let clamped: f64 = rows.iter().map(|(_, value)| value.min(50.0)).sum();
    assert_eq!(raw, 170.0);
    assert_eq!(clamped, 100.0);

    let query = Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        Bounds::new(0.0, 50.0)?,
        Epsilon::new(1.0)?,
    )?;
    let mut gate = ReleaseGate::new(Policy::default(), 4);
    let released = gate.release(&query, &set)?;

    // Exact to the grid the released value was rounded onto — 2^-20 of the
    // noise scale, and nothing else stands between the two figures.
    let behind = truth_behind(4, &query, &set, released.value())?;
    assert!(
        (behind - clamped).abs() <= released.noise_scale() * 2f64.powi(-20),
        "the statistic behind the release should be the clamped total {clamped}, not {behind}"
    );
    // And the noise is sized by the declared width, not by the outlier: 50/1.
    assert_eq!(released.noise_scale(), 50.0);
    Ok(())
}

#[test]
fn the_mean_divides_by_the_contributor_count_and_says_so_in_its_noise_scale() -> Result<()> {
    let rows = [
        ("amer-1", 10.0),
        ("apac-1", 20.0),
        ("apac-2", 30.0),
        ("emea-1", 40.0),
        ("emea-2", 50.0),
    ];
    let set = contributions(&rows)?;
    let query = Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Mean,
        Bounds::new(0.0, 100.0)?,
        Epsilon::new(1.0)?,
    )?;
    let mut gate = ReleaseGate::new(Policy::default(), 4);
    let released = gate.release(&query, &set)?;

    assert_eq!(
        released.noise_scale(),
        20.0,
        "range 100 over five cells, at epsilon 1"
    );
    let behind = truth_behind(4, &query, &set, released.value())?;
    assert!(
        (behind - 30.0).abs() <= released.noise_scale() * 2f64.powi(-20),
        "the mean behind the release is {behind}"
    );
    Ok(())
}

#[test]
fn a_count_threshold_outside_the_declared_bounds_is_refused() -> Result<()> {
    let bounds = Bounds::new(0.0, 100.0)?;
    let cohort = CohortId::new("over-limit")?;
    let epsilon = Epsilon::new(1.0)?;

    // Above the top of the range the count is zero for every possible dataset;
    // below the bottom it is the contributor count. Either way the budget would
    // be spent on a number that carries nothing.
    for threshold in [100.0, 250.0, -1.0] {
        let error = Query::new(
            cohort.clone(),
            Statistic::CountAbove { threshold },
            bounds,
            epsilon,
        )
        .expect_err("a threshold outside the bounds answers itself");
        assert!(error.message().contains("bounds"), "{}", error.message());
    }

    // Inside the range it is a real question, so the refusals above are about
    // the threshold and not about `CountAbove`.
    assert!(
        Query::new(
            cohort,
            Statistic::CountAbove { threshold: 40.0 },
            bounds,
            epsilon
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn a_range_that_is_not_a_range_is_refused() {
    assert!(Bounds::new(50.0, 50.0).is_err(), "zero width");
    assert!(Bounds::new(50.0, 10.0).is_err(), "inverted");
    assert!(
        Bounds::new(0.0, f64::INFINITY).is_err(),
        "unbounded sensitivity"
    );
    assert!(Bounds::new(f64::NAN, 1.0).is_err());
    assert!(Bounds::new(-100.0, 100.0).is_ok());
}

#[test]
fn a_contribution_that_is_not_a_number_is_refused_at_the_door() -> Result<()> {
    // Refused here, at construction, and not inside the release path. A
    // refusal that depended on the values would hand a caller one bit about the
    // data per attempt, with no budget charged for it.
    let cell = CellId::new("apac-2")?;
    assert!(Contribution::new(cell.clone(), f64::NAN).is_err());
    assert!(Contribution::new(cell.clone(), f64::INFINITY).is_err());
    let error = Contribution::new(cell, f64::NAN).expect_err("NaN is not a contribution");
    assert!(error.message().contains("apac-2"), "{}", error.message());
    Ok(())
}

#[test]
fn an_unnamed_cell_or_cohort_is_refused() {
    assert!(CellId::new("").is_err());
    assert!(CellId::new("   ").is_err());
    assert!(CohortId::new("").is_err());
    assert!(CellId::new("apac-2").is_ok());
}

#[test]
fn the_sensitivity_of_each_statistic_is_the_one_its_noise_is_calibrated_from() -> Result<()> {
    let bounds = Bounds::new(0.0, 100.0)?;
    assert_eq!(Statistic::Sum.sensitivity(bounds, 5)?.get(), 100.0);
    assert_eq!(Statistic::Mean.sensitivity(bounds, 5)?.get(), 20.0);
    assert_eq!(
        Statistic::CountAbove { threshold: 40.0 }
            .sensitivity(bounds, 5)?
            .get(),
        1.0
    );
    // A statistic over nobody has no sensitivity, rather than a sensitivity of
    // zero that would divide into an infinite noise scale.
    assert!(Statistic::Sum.sensitivity(bounds, 0).is_err());
    Ok(())
}

#[test]
fn a_release_records_the_question_it_answered() -> Result<()> {
    let set = contributions(&[
        ("amer-1", 10.0),
        ("apac-1", 20.0),
        ("apac-2", 30.0),
        ("emea-1", 40.0),
        ("emea-2", 50.0),
    ])?;
    let query = Query::new(
        CohortId::new("global-net-exposure")?,
        Statistic::Sum,
        Bounds::new(0.0, 100.0)?,
        Epsilon::new(0.5)?,
    )?;
    let mut gate = ReleaseGate::new(Policy::default(), 4);
    let released = gate.release(&query, &set)?;

    assert_eq!(released.cohort().as_str(), "global-net-exposure");
    assert_eq!(released.statistic(), Statistic::Sum);
    assert_eq!(released.epsilon(), 0.5);
    assert_eq!(released.id().get(), 1);
    assert_eq!(released.fingerprint().to_hex().len(), 64);

    // The gate's own record carries the contributor set, which is what the
    // differencing gate compares future questions against.
    let record = gate.records().first().expect("one release");
    assert_eq!(record.cells().len(), 5);
    assert_eq!(record.release(), &released);
    Ok(())
}
