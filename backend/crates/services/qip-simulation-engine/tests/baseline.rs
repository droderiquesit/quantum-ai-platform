//! The unconditional baseline and the out-of-sample comparison behind Gate 8.
//!
//! The property under test is that the platform can no longer say "we sized
//! on a regime model" without also saying what the same sizing did without
//! the model, on data neither was fitted on, from a split fixed before the
//! run. Each refusal here is one that, absent, would let a comparison be
//! reported that nobody could check.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_simulation_engine::baseline::{
    ComparisonPolicy, RegimeComparison, RegimeTerm, ReturnObservation, SplitDeclaration, size,
};

const PERIODS: usize = 480;
const BOUNDARY_INDEX: usize = 240;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn day(index: usize) -> Timestamp {
    start().saturating_add(Duration::from_days(index as i64))
}

/// Every observation knowable at its own instant — a bar keyed on its close.
fn observed(values: &[f64]) -> Result<Vec<ReturnObservation>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| ReturnObservation::new(day(index), day(index), *value))
        .collect()
}

/// The two states both fixtures draw from: a calm market drifting up and a
/// turbulent one drifting down. The fixtures differ only in whether the
/// state *persists*, which is the one thing a regime model can exploit.
fn draw(rng: &mut Xoshiro256, turbulent: bool) -> f64 {
    if turbulent {
        rng.normal_with(-0.0004, 0.03)
    } else {
        rng.normal_with(0.0008, 0.006)
    }
}

/// Sixty-period blocks alternating the two states. The regime is real, so a
/// sizing rule that reads it has something to gain.
fn regime_structured(seed: u64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..PERIODS)
        .map(|index| draw(&mut rng, (index / 60) % 2 == 1))
        .collect()
}

/// The same two states, chosen independently every period. The marginal
/// distribution matches the structured fixture — same fat tails, same means
/// — and nothing persists, so whatever a regime model reads into it, it read
/// into noise. This is the null a regime detector has to beat, not Gaussian
/// noise: on plain Gaussian noise the fitted states barely differ and the
/// two arms collapse into one.
fn regime_free(seed: u64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..PERIODS)
        .map(|_| {
            let turbulent = rng.bernoulli(0.5);
            draw(&mut rng, turbulent)
        })
        .collect()
}

/// The seeds both paired tests run over. Fixed, so the tests are
/// deterministic; forty, so a verdict is a distribution and not one draw.
const SEEDS: std::ops::RangeInclusive<u64> = 1..=40;

/// A single structured fixture for the tests that only need something to run.
fn one_structured() -> Result<Vec<ReturnObservation>> {
    observed(&regime_structured(0x5EED_0001))
}

fn declared() -> SplitDeclaration {
    SplitDeclaration::new(day(BOUNDARY_INDEX), day(BOUNDARY_INDEX - 1))
}

fn run_at() -> Timestamp {
    day(PERIODS + 1)
}

/// One policy for every comparison in this file. The two paired tests differ
/// only in their data-generating process; a cost or weight chosen per test
/// would be a thumb on the scale.
fn policy() -> Result<ComparisonPolicy> {
    ComparisonPolicy::new(0.5, 5.0, 252.0, 1)
}

#[test]
fn regime_conditional_allocation_beats_the_baseline_on_a_regime_structured_holdout() -> Result<()> {
    let mut wins = 0usize;
    let mut total_advantage = 0.0;
    let mut seeds = 0usize;
    for seed in SEEDS {
        let observations = observed(&regime_structured(seed))?;
        let comparison = RegimeComparison::run(&observations, &declared(), &policy()?, run_at())?;

        // Premise, every seed: the holdout contained turbulence the model
        // recognised, so the conditional arm had something to condition on,
        // and it did move its weight — the two arms were not the same arm.
        let turbulent = comparison.regime_occupancy.get("turbulent").copied();
        assert!(
            turbulent.is_some_and(|count| count > 0),
            "seed {seed}: the holdout should hold turbulence the model saw, got {turbulent:?}"
        );
        assert_eq!(comparison.holdout_observations, PERIODS - BOUNDARY_INDEX);
        assert!(comparison.conditional.turnover > comparison.unconditional.turnover);

        if comparison.conditional_beats_unconditional() {
            wins += 1;
        }
        total_advantage += comparison.advantage;
        seeds += 1;
    }
    assert_eq!(seeds, 40);

    // Observed over these seeds: 38 of 40 and a mean advantage of +0.57.
    // The bars sit a margin inside that so a change to the fit's iteration
    // count cannot flip the test, and well clear of what noise produces —
    // the regime-free null below manages 12 of 40 and a negative mean under
    // the same policy.
    let mean_advantage = total_advantage / seeds as f64;
    assert!(
        wins * 10 >= seeds * 9,
        "the conditional arm won {wins} of {seeds} seeds; a real regime should carry nine in ten"
    );
    assert!(
        mean_advantage > 0.4,
        "mean advantage over the baseline was {mean_advantage:+.3}"
    );
    Ok(())
}

#[test]
fn the_unconditional_baseline_beats_regime_conditioning_on_a_regime_free_holdout() -> Result<()> {
    let mut baseline_wins = 0usize;
    let mut total_advantage = 0.0;
    let mut seeds = 0usize;
    for seed in SEEDS {
        let observations = observed(&regime_free(seed))?;
        let comparison = RegimeComparison::run(&observations, &declared(), &policy()?, run_at())?;

        // Premise, every seed: the conditional arm read regimes into the
        // noise and traded on them — it varied its weight and held less than
        // the base — while the baseline held the base weight throughout.
        assert!(comparison.conditional.turnover > comparison.unconditional.turnover);
        assert!((comparison.unconditional.average_weight - 0.5).abs() < 1e-12);
        assert!(comparison.conditional.average_weight < 0.5);

        if !comparison.conditional_beats_unconditional() {
            baseline_wins += 1;
        }
        total_advantage += comparison.advantage;
        seeds += 1;
    }
    assert_eq!(seeds, 40);

    // Observed over these seeds: the baseline ahead in 28 of 40, mean
    // advantage −0.22. Stated as a majority and a negative mean rather than
    // as a per-seed sign, because a 240-period holdout gives an annualised
    // Sharpe a standard error near one and the per-seed difference swings
    // ±0.5 around its mean; a single seed asserting the sign would be a coin
    // the test had chosen. Frictionless the null is a coin flip outright (the
    // Jensen penalty scales with a base Sharpe that is only 0.15 here), so
    // what the baseline wins by is what a regime term reading noise pays in
    // turnover — which is the honest finding, and the reason the cost is the
    // same one the structured test is charged.
    let mean_advantage = total_advantage / seeds as f64;
    assert!(
        baseline_wins * 2 > seeds,
        "the baseline was ahead in {baseline_wins} of {seeds} seeds; on a regime-free market it should hold a majority"
    );
    assert!(
        mean_advantage < -0.1,
        "mean advantage of the conditional arm was {mean_advantage:+.3}; on a regime-free market conditioning should cost"
    );
    Ok(())
}

#[test]
fn a_holdout_observation_knowable_before_the_boundary_is_refused_as_leakage() -> Result<()> {
    let mut observations = one_structured()?;
    // Premise: the clean stamping runs.
    assert!(RegimeComparison::run(&observations, &declared(), &policy()?, run_at()).is_ok());

    // A holdout bar keyed on its open rather than its close: its value is
    // readable a day before the period it describes, which puts it on the
    // fitting side of the boundary.
    observations[BOUNDARY_INDEX].known_at = day(BOUNDARY_INDEX - 1);
    let refused = RegimeComparison::run(&observations, &declared(), &policy()?, run_at());
    let error = refused.expect_err("a leaked holdout must be refused");
    assert!(
        error.message().contains("before the boundary"),
        "unexpected refusal: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_training_observation_restated_after_the_boundary_is_refused() -> Result<()> {
    let mut observations = one_structured()?;
    assert!(RegimeComparison::run(&observations, &declared(), &policy()?, run_at()).is_ok());

    // A training value corrected by the vendor a week after the boundary: a
    // fit at the boundary could not have seen the corrected figure.
    observations[BOUNDARY_INDEX - 3].known_at = day(BOUNDARY_INDEX + 7);
    let refused = RegimeComparison::run(&observations, &declared(), &policy()?, run_at());
    let error = refused.expect_err("a restated training value must be refused");
    assert!(
        error.message().contains("after the boundary"),
        "unexpected refusal: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_split_not_declared_before_the_run_is_refused() -> Result<()> {
    let observations = one_structured()?;
    assert!(RegimeComparison::run(&observations, &declared(), &policy()?, run_at()).is_ok());

    // Declared at the instant of the run: the boundary could have been chosen
    // with the result in view, so the result cannot be checked against it.
    let late = SplitDeclaration::new(day(BOUNDARY_INDEX), run_at());
    let refused = RegimeComparison::run(&observations, &late, &policy()?, run_at());
    let error = refused.expect_err("a split declared at the run must be refused");
    assert!(
        error.message().contains("not fixed in advance"),
        "unexpected refusal: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_record_without_its_declared_split_does_not_decode() -> Result<()> {
    let observations = one_structured()?;
    let comparison = RegimeComparison::run(&observations, &declared(), &policy()?, run_at())?;

    // Premise: the record round-trips whole, both arms named, and the split
    // it carries is the one declared.
    let encoded = serde_json::to_value(&comparison).expect("serialisable");
    let decoded: RegimeComparison =
        serde_json::from_value(encoded.clone()).expect("a whole record decodes");
    assert_eq!(decoded, comparison);
    assert_eq!(decoded.split, declared());
    assert_eq!(decoded.conditional.arm, "regime_conditional");
    assert_eq!(decoded.unconditional.arm, "unconditional");

    // Strip the split. A record that decoded without one would be a result
    // with no boundary to check it against — the undeclared split, in the
    // form the event log would actually meet it.
    let mut stripped = encoded;
    let object = stripped.as_object_mut().expect("a record is an object");
    assert!(object.remove("split").is_some());
    assert!(serde_json::from_value::<RegimeComparison>(stripped).is_err());
    Ok(())
}

#[test]
fn the_baseline_is_the_same_sizing_with_the_regime_term_removed() -> Result<()> {
    // The sizing rule, in the open: a term of one is the base weight, a term
    // below one scales it, and the removed term is exactly one.
    assert!((size(0.5, RegimeTerm::Removed) - 0.5).abs() < 1e-12);
    assert!((size(0.5, RegimeTerm::Conditional(0.2)) - 0.1).abs() < 1e-12);
    assert!(
        (size(0.5, RegimeTerm::Conditional(1.0)) - size(0.5, RegimeTerm::Removed)).abs() < 1e-12
    );

    // And the record shows it: the baseline arm held the base weight every
    // period and traded only its entry, while the fold is the crate's one
    // notion of a split, cut at the declared boundary.
    let observations = one_structured()?;
    let comparison = RegimeComparison::run(&observations, &declared(), &policy()?, run_at())?;
    assert!((comparison.unconditional.average_weight - 0.5).abs() < 1e-12);
    assert!((comparison.unconditional.turnover - 0.5).abs() < 1e-12);
    let fold = comparison.fold();
    assert!(fold.is_valid());
    assert_eq!(fold.train.len(), BOUNDARY_INDEX);
    assert_eq!(fold.test.first().copied(), Some(BOUNDARY_INDEX));
    assert_eq!(fold.test.len(), PERIODS - BOUNDARY_INDEX);

    // Same inputs, same record: the comparison is replayable from the log.
    let again = RegimeComparison::run(&observations, &declared(), &policy()?, run_at())?;
    assert_eq!(again, comparison);
    Ok(())
}

#[test]
fn a_policy_that_is_not_declared_in_full_is_refused() {
    assert!(ComparisonPolicy::new(0.5, 5.0, 252.0, 1).is_ok());
    assert!(ComparisonPolicy::new(0.0, 5.0, 252.0, 1).is_err());
    assert!(ComparisonPolicy::new(1.5, 5.0, 252.0, 1).is_err());
    assert!(ComparisonPolicy::new(0.5, -1.0, 252.0, 1).is_err());
    assert!(ComparisonPolicy::new(0.5, 5.0, 0.0, 1).is_err());
    assert!(ComparisonPolicy::new(0.5, 5.0, 252.0, 0).is_err());
    assert!(ReturnObservation::new(day(0), day(0), f64::NAN).is_err());
}
