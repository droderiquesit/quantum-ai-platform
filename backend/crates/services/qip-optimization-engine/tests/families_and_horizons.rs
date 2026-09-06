//! Tests for blueprint §23.1 LEVEL 1 (family clustering) and §23.4
//! (multi-horizon reconciliation).
//!
//! Two properties carry most of the weight here. The first is that families
//! are keyed on **stress** correlation: the blueprint's own note on this
//! capability is that calm-market correlation understates stress correlation,
//! so a clustering built on the full sample separates two strategies that turn
//! out to be one bet in a drawdown. The second is that a horizon's commitment
//! is checked against *its own* pool, because a single total that looks
//! unbreached is exactly how a multi-year capital call gets funded out of a
//! market maker's inventory.
//!
//! Nothing here is a claim that the platform has clustered a real strategy
//! population. Every series below is a fixture built in this file to exercise
//! a refusal or a property; no family in this repository has been evaluated on
//! real data.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::decimal::Decimal;
use qip_core::error::Result;
use qip_core::ids::StrategyId;
use qip_numerics::Matrix;
use qip_numerics::stats;
use qip_optimization_engine::families::{
    FamilyClustering, FamilyId, Linkage, MAX_STRATEGIES, StrategyReturns, StressAxis,
    StressCorrelation, StressWindow,
};
use qip_optimization_engine::horizons::{
    CapitalPools, FamilyBudget, Horizon, family_horizons, reconcile,
};
use std::collections::{BTreeMap, BTreeSet};

// --- fixtures ---------------------------------------------------------------

const OBSERVATIONS: usize = 40;
const STRESS_LENGTH: usize = 16;

/// A deterministic pseudo-noise series. Not an RNG: a fixed recurrence, so the
/// fixture is byte-identical on every machine and the premises asserted below
/// are stable facts about this file rather than about a seed.
fn noise(seed: u64, n: usize) -> Vec<f64> {
    let mut state = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (0..n)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let bits = (state >> 33) as u32;
            f64::from(bits) / f64::from(u32::MAX) * 2.0 - 1.0
        })
        .collect()
}

fn strategy(name: &str) -> StrategyId {
    StrategyId::from_string(name)
}

fn stress_indices() -> Vec<usize> {
    (0..STRESS_LENGTH).collect()
}

fn window() -> Result<StressWindow> {
    StressWindow::explicit(
        OBSERVATIONS,
        stress_indices(),
        "fixture: the first sixteen observations are the drawdown",
    )
}

/// A strategy that follows `shock` inside the stress window and its own
/// idiosyncratic series outside it. This is the shape the blueprint warns
/// about: independent in calm, one bet in stress.
fn regime_switching(shock: &[f64], idiosyncratic: &[f64], stress: &BTreeSet<usize>) -> Vec<f64> {
    (0..OBSERVATIONS)
        .map(|t| {
            if stress.contains(&t) {
                shock[t] + 0.15 * idiosyncratic[t]
            } else {
                idiosyncratic[t]
            }
        })
        .collect()
}

/// Four strategies: `a` and `b` share one stress shock, `c` and `d` share
/// another, and in calm markets all four are idiosyncratic.
fn decoupling_population() -> Result<Vec<StrategyReturns>> {
    let stress: BTreeSet<usize> = stress_indices().into_iter().collect();
    let first_shock = noise(1, OBSERVATIONS);
    let second_shock = noise(2, OBSERVATIONS);
    let idiosyncratic: Vec<Vec<f64>> = (0..4).map(|i| noise(10 + i, OBSERVATIONS)).collect();
    let shocks = [&first_shock, &first_shock, &second_shock, &second_shock];
    ["strategy-a", "strategy-b", "strategy-c", "strategy-d"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            StrategyReturns::new(
                strategy(name),
                regime_switching(shocks[i], &idiosyncratic[i], &stress),
            )
        })
        .collect()
}

fn decoupling_correlation() -> Result<StressCorrelation> {
    StressCorrelation::from_returns(&decoupling_population()?, &window()?)
}

fn pools(
    total: i64,
    inventory: i64,
    deployable: i64,
    unreserved: i64,
    reserved: i64,
) -> Result<CapitalPools> {
    CapitalPools::new(
        Decimal::from_int(total),
        Decimal::from_int(inventory),
        Decimal::from_int(deployable),
        Decimal::from_int(unreserved),
        Decimal::from_int(reserved),
        Decimal::ZERO,
    )
}

// --- the stress-versus-calm decision ----------------------------------------

#[test]
fn two_strategies_that_decouple_in_calm_and_move_together_in_stress_land_in_one_family()
-> Result<()> {
    let correlation = decoupling_correlation()?;
    let index_of = |name: &str| {
        correlation
            .strategies()
            .iter()
            .position(|s| s.as_str() == name)
    };
    let (a, b) = (
        index_of("strategy-a").expect("strategy-a is in the population"),
        index_of("strategy-b").expect("strategy-b is in the population"),
    );

    // Assert the premise: this fixture really does have the property the test
    // is about. Without this the assertion below passes on any clustering of
    // any four series.
    let calm = correlation.calm().get(a, b);
    let stressed = correlation.stress().get(a, b);
    assert!(
        calm.abs() < 0.4,
        "the fixture must look near-independent in calm markets, but a and b correlate {calm}"
    );
    assert!(
        stressed > 0.8,
        "the fixture must move together in stress, but a and b correlate {stressed}"
    );

    let assignment = FamilyClustering::new(2)?.cluster(&correlation)?;
    assert_eq!(assignment.family_count(), 2);
    assert_eq!(
        assignment.family_of(&strategy("strategy-a")),
        assignment.family_of(&strategy("strategy-b")),
        "a and b are one bet in a drawdown and must be one family"
    );
    Ok(())
}

#[test]
fn the_calm_keyed_clustering_of_the_same_population_files_pairs_differently() -> Result<()> {
    let correlation = decoupling_correlation()?;
    let clustering = FamilyClustering::new(2)?;
    let stress_keyed = clustering.cluster(&correlation)?;

    // The same population clustered on its calm matrix, by handing the calm
    // estimate in as if it were the stress one. This is the version the
    // blueprint warns against, built here only to show it disagrees.
    let calm_only = StressCorrelation::from_matrices(
        correlation.strategies().to_vec(),
        correlation.calm().clone(),
        correlation.calm().clone(),
        STRESS_LENGTH,
        OBSERVATIONS - STRESS_LENGTH,
    )?;
    let calm_keyed = clustering.cluster(&calm_only)?;

    // Premise: both clusterings produced the families they were asked for, so
    // a difference between them is a difference of opinion and not of size.
    assert_eq!(stress_keyed.family_count(), 2);
    assert_eq!(calm_keyed.family_count(), 2);

    let together = |assignment: &qip_optimization_engine::families::FamilyAssignment,
                    left: &str,
                    right: &str| {
        assignment.family_of(&strategy(left)) == assignment.family_of(&strategy(right))
    };
    assert!(
        together(&stress_keyed, "strategy-a", "strategy-b"),
        "the stress view must join a and b"
    );
    assert!(
        !together(&calm_keyed, "strategy-a", "strategy-b"),
        "the calm view must separate a and b — if it does not, this fixture no longer \
         demonstrates the understatement the design is built around"
    );
    Ok(())
}

#[test]
fn the_diagnostics_count_the_pairs_the_calm_view_would_have_misfiled() -> Result<()> {
    let correlation = decoupling_correlation()?;
    let assignment = FamilyClustering::new(2)?.cluster(&correlation)?;
    let diagnostics = assignment.diagnostics();

    // Premise: there are pairs to disagree about at all.
    assert_eq!(diagnostics.pairs_total, 6, "four strategies make six pairs");
    assert!(
        diagnostics.pairs_calm_would_have_misfiled > 0,
        "the calm view must disagree somewhere on this fixture, otherwise the number reports \
         nothing"
    );
    assert!(diagnostics.pairs_calm_would_have_misfiled <= diagnostics.pairs_total);
    assert!(
        diagnostics.mean_stress_excess > 0.0,
        "this population's co-movement rises under stress, so the calm view understates it; the \
         diagnostic must say so"
    );
    Ok(())
}

#[test]
fn a_population_whose_stress_and_calm_views_agree_reports_no_misfiled_pairs() -> Result<()> {
    // The counterpart to the test above: when calm and stress agree, the
    // diagnostic must read zero rather than always finding a disagreement.
    let correlation = decoupling_correlation()?;
    let identical = StressCorrelation::from_matrices(
        correlation.strategies().to_vec(),
        correlation.stress().clone(),
        correlation.stress().clone(),
        STRESS_LENGTH,
        OBSERVATIONS - STRESS_LENGTH,
    )?;
    let assignment = FamilyClustering::new(2)?.cluster(&identical)?;
    assert_eq!(assignment.diagnostics().pairs_total, 6);
    assert_eq!(assignment.diagnostics().pairs_calm_would_have_misfiled, 0);
    Ok(())
}

#[test]
fn strategies_inside_a_family_are_more_correlated_in_stress_than_strategies_across_families()
-> Result<()> {
    let assignment = FamilyClustering::new(2)?.cluster(&decoupling_correlation()?)?;
    let diagnostics = assignment.diagnostics();

    // Premise: both figures were computed from a non-empty set of pairs. With
    // one family there are no inter-family pairs and the comparison is vacuous.
    assert_eq!(assignment.family_count(), 2);
    assert!(
        assignment.families().values().any(|m| m.len() > 1),
        "at least one family must hold more than one member for an intra-family mean to exist"
    );
    assert!(
        diagnostics.mean_intra_family_correlation > diagnostics.mean_inter_family_correlation,
        "a clustering that has done any work separates the less correlated pairs: intra {} vs \
         inter {}",
        diagnostics.mean_intra_family_correlation,
        diagnostics.mean_inter_family_correlation
    );
    Ok(())
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_clustering_does_not_depend_on_the_order_the_strategies_were_supplied_in() -> Result<()> {
    let forwards = decoupling_population()?;
    let mut backwards = forwards.clone();
    backwards.reverse();

    // Premise: the two inputs really are in different orders, and the answer
    // has enough structure for an ordering bug to show up in it.
    assert_ne!(
        forwards[0].strategy(),
        backwards[0].strategy(),
        "the reversed population must actually start with a different strategy"
    );

    let clustering = FamilyClustering::new(2)?;
    let one = clustering.cluster(&StressCorrelation::from_returns(&forwards, &window()?)?)?;
    let other = clustering.cluster(&StressCorrelation::from_returns(&backwards, &window()?)?)?;

    assert_eq!(
        one.family_count(),
        2,
        "a single family would hide a reordering"
    );
    assert_eq!(
        one.families(),
        other.families(),
        "family assignment must be a function of the set of strategies, not of the sequence the \
         caller happened to pass them in — a replay that reorders is not a replay"
    );
    Ok(())
}

/// Three strategies whose stress matrix says the first and the third are one
/// bet, in the row order the labels are given in.
///
/// The labels are deliberately not in sorted order: `z-first` is row 0 and
/// `m-third` is row 2, and those two correlate 0.99 under stress while
/// `a-second` is independent of both.
fn unsorted_matrices() -> Result<(Vec<StrategyId>, Matrix, Matrix)> {
    let rows = vec![
        vec![1.0, 0.0, 0.99],
        vec![0.0, 1.0, 0.0],
        vec![0.99, 0.0, 1.0],
    ];
    let stress = Matrix::from_rows(&rows)?;
    // Premise: the fixture is admissible on every other ground, so anything
    // that goes wrong with it is about the row order and nothing else.
    assert!(
        stress.is_positive_semidefinite(1e-9),
        "the fixture must be a real correlation matrix"
    );
    Ok((
        vec![
            strategy("z-first"),
            strategy("a-second"),
            strategy("m-third"),
        ],
        stress,
        Matrix::identity(3),
    ))
}

#[test]
fn a_family_boundary_follows_the_strategy_and_not_the_row_the_caller_put_it_in() -> Result<()> {
    // This is the failure the module exists to prevent, produced by the module
    // itself. `from_matrices` sorted the labels and passed the matrices
    // through untouched, so row 0 went on describing z-first while
    // `strategies[0]` became a-second. The result: the two strategies that are
    // one bet in a drawdown landed in different families, the two independent
    // ones landed together, and the diagnostics looked entirely plausible.
    // Both in-tree callers happened to hand their rows in sorted order, so no
    // test could see it.
    let (labels, stress, calm) = unsorted_matrices()?;
    let correlation = StressCorrelation::from_matrices(labels, stress, calm, 20, 20)?;

    // Premise: the canonical order really did move the labels, so an
    // unpermuted matrix would now be mislabelled.
    let ordered: Vec<&str> = correlation
        .strategies()
        .iter()
        .map(qip_core::ids::Id::as_str)
        .collect();
    assert_eq!(
        ordered,
        vec!["a-second", "m-third", "z-first"],
        "the canonical order is the sorted one"
    );

    // The 0.99 must have travelled with its strategies: it belongs at
    // (m-third, z-first), which is now (1, 2).
    assert!(
        (correlation.stress().get(1, 2) - 0.99).abs() < 1e-12,
        "the stress correlation between m-third and z-first is {}, not the 0.99 the caller stated",
        correlation.stress().get(1, 2)
    );
    assert!(
        correlation.stress().get(0, 2).abs() < 1e-12,
        "a-second and z-first are independent; the matrix says {}",
        correlation.stress().get(0, 2)
    );

    let assignment = FamilyClustering::new(2)?.cluster(&correlation)?;
    assert_eq!(assignment.family_count(), 2);
    assert_eq!(
        assignment.family_of(&strategy("z-first")),
        assignment.family_of(&strategy("m-third")),
        "z-first and m-third are one bet in a drawdown and must be one family"
    );
    assert_ne!(
        assignment.family_of(&strategy("a-second")),
        assignment.family_of(&strategy("z-first")),
        "a-second is independent of both and must not be filed with either"
    );
    Ok(())
}

#[test]
fn matrices_supplied_in_two_row_orders_produce_the_same_families() -> Result<()> {
    // The header claims family assignment is "a function of the *set*, not the
    // sequence". Sorting the labels without permuting the matrices falsified
    // that for every supplied estimate; this is the claim, asserted.
    let (labels, stress, calm) = unsorted_matrices()?;
    let one = FamilyClustering::new(2)?.cluster(&StressCorrelation::from_matrices(
        labels, stress, calm, 20, 20,
    )?)?;

    // The same estimate with rows 0 and 1 exchanged, labels and all.
    let swapped_rows = vec![
        vec![1.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.99],
        vec![0.0, 0.99, 1.0],
    ];
    let other = FamilyClustering::new(2)?.cluster(&StressCorrelation::from_matrices(
        vec![
            strategy("a-second"),
            strategy("z-first"),
            strategy("m-third"),
        ],
        Matrix::from_rows(&swapped_rows)?,
        Matrix::identity(3),
        20,
        20,
    )?)?;

    assert_eq!(one.family_count(), 2, "a single family would hide a swap");
    assert_eq!(
        one.families(),
        other.families(),
        "the same estimate presented in two row orders is the same estimate — a replay that \
         reorders is not a replay"
    );
    Ok(())
}

#[test]
fn a_strategy_labelling_two_rows_of_a_supplied_matrix_is_refused() -> Result<()> {
    // With a duplicate label there is no answer to which row the family
    // boundary was drawn from, and the sort alone would have silently kept
    // both rows under one name.
    let (_, stress, calm) = unsorted_matrices()?;
    let error = StressCorrelation::from_matrices(
        vec![strategy("s-a"), strategy("s-b"), strategy("s-a")],
        stress,
        calm,
        20,
        20,
    )
    .expect_err("one row per strategy");
    assert!(
        error.message().contains("labels two rows"),
        "the refusal must name the defect: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn family_names_sort_lexicographically_in_the_order_their_indices_sort_numerically() -> Result<()> {
    // The name is a journal key segment downstream, where sorting is textual.
    // Without the zero padding, family-10 would sort before family-2.
    let second = FamilyId::new(2).name();
    let tenth = FamilyId::new(10).name();
    assert_eq!(second, "family-002");
    assert_eq!(tenth, "family-010");
    assert!(second < tenth, "{second} must sort before {tenth}");
    assert!(
        tenth
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "a family name must stay inside the character set a journal key segment allows"
    );
    Ok(())
}

// --- clustering refusals ----------------------------------------------------

#[test]
fn a_target_of_more_families_than_strategies_is_refused() -> Result<()> {
    let correlation = decoupling_correlation()?;
    assert_eq!(correlation.len(), 4, "the fixture holds four strategies");
    let error = FamilyClustering::new(9)?
        .cluster(&correlation)
        .expect_err("nine families cannot be drawn from four strategies");
    assert!(
        error
            .message()
            .contains("exceeds the 4 strategies available"),
        "the refusal must name the shortfall: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_target_of_zero_families_is_refused() {
    let error = FamilyClustering::new(0).expect_err("zero families allocates across nothing");
    assert!(error.message().contains("zero families"));
}

#[test]
fn a_correlation_matrix_that_is_not_positive_semi_definite_is_refused() -> Result<()> {
    // Three strategies where a and b, and a and c, are both strongly positive
    // but b and c are strongly negative. No joint distribution produces that,
    // and the distances it implies violate the triangle inequality.
    let rows = vec![
        vec![1.0, 0.9, 0.9],
        vec![0.9, 1.0, -0.9],
        vec![0.9, -0.9, 1.0],
    ];
    let indefinite = Matrix::from_rows(&rows)?;
    // Premise: the fixture is genuinely indefinite, not merely ill-conditioned.
    assert!(
        !indefinite.is_positive_semidefinite(1e-9),
        "the fixture must actually be indefinite or this test proves nothing"
    );

    let error = StressCorrelation::from_matrices(
        vec![strategy("s-a"), strategy("s-b"), strategy("s-c")],
        indefinite,
        Matrix::identity(3),
        20,
        20,
    )
    .expect_err("an indefinite correlation matrix is not a metric");
    assert!(
        error.message().contains("positive semi-definite"),
        "the refusal must name the defect: {}",
        error.message()
    );
    assert!(
        error.message().contains("will not repair it silently"),
        "the refusal must say it is not repairing the input: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_covariance_matrix_passed_where_a_correlation_matrix_belongs_is_refused() -> Result<()> {
    let variances = Matrix::diagonal(&[0.04, 0.04, 0.04]);
    // Premise: this is positive semi-definite, so only the unit-diagonal check
    // can be what refuses it.
    assert!(variances.is_positive_semidefinite(1e-9));
    let error = StressCorrelation::from_matrices(
        vec![strategy("s-a"), strategy("s-b"), strategy("s-c")],
        variances,
        Matrix::identity(3),
        20,
        20,
    )
    .expect_err("a covariance is not a correlation");
    assert!(
        error.message().contains("unit diagonal"),
        "the refusal must name what is wrong: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_non_finite_return_is_refused_rather_than_carried_into_a_correlation() {
    let mut returns = vec![0.01; OBSERVATIONS];
    returns[7] = f64::NAN;
    let error = StrategyReturns::new(strategy("strategy-a"), returns)
        .expect_err("a NaN return must not reach a covariance");
    assert!(
        error.message().contains("observation 7"),
        "the refusal must name the observation: {}",
        error.message()
    );
}

#[test]
fn a_strategy_supplied_twice_is_refused() -> Result<()> {
    let mut population = decoupling_population()?;
    let duplicate = population[0].clone();
    population.push(duplicate);
    let error = StressCorrelation::from_returns(&population, &window()?)
        .expect_err("a duplicate is perfectly correlated with itself");
    assert!(
        error.message().contains("appears twice"),
        "the refusal must name the duplication: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_strategy_that_does_not_move_inside_the_stress_window_is_refused() -> Result<()> {
    let stress: BTreeSet<usize> = stress_indices().into_iter().collect();
    let idiosyncratic = noise(21, OBSERVATIONS);
    let flat_in_stress: Vec<f64> = (0..OBSERVATIONS)
        .map(|t| {
            if stress.contains(&t) {
                0.0
            } else {
                idiosyncratic[t]
            }
        })
        .collect();
    // Premise: the series itself is valid — every value is finite — so only the
    // window check can be what refuses it.
    let series = StrategyReturns::new(strategy("strategy-flat"), flat_in_stress)?;
    assert_eq!(series.len(), OBSERVATIONS);

    let mut population = decoupling_population()?;
    population.push(series);
    let error = StressCorrelation::from_returns(&population, &window()?)
        .expect_err("a strategy with no stress variance has no measurable stress correlation");
    assert!(
        error
            .message()
            .contains("does not move inside the stress window"),
        "the refusal must name the defect: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn series_of_different_lengths_are_refused() -> Result<()> {
    let mut population = decoupling_population()?;
    let short = StrategyReturns::new(strategy("strategy-e"), vec![0.01; OBSERVATIONS - 3])?;
    population.push(short);
    let error = StressCorrelation::from_returns(&population, &window()?)
        .expect_err("series must be aligned to one calendar");
    assert!(
        error.message().contains("align the series to one calendar"),
        "the refusal must name the fix: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_population_larger_than_the_working_set_bound_is_refused_rather_than_clustered() -> Result<()> {
    let window = window()?;
    let mut population: Vec<StrategyReturns> = Vec::new();
    for i in 0..=MAX_STRATEGIES {
        population.push(StrategyReturns::new(
            strategy(&format!("strategy-{i:05}")),
            noise(i as u64 + 1_000, OBSERVATIONS),
        )?);
    }
    // Premise: the population is over the bound by exactly one, so the refusal
    // is about the bound and not about anything else in the fixture.
    assert_eq!(population.len(), MAX_STRATEGIES + 1);
    let error = StressCorrelation::from_returns(&population, &window)
        .expect_err("an unbounded working set is refused, not grown");
    // Matched on the phrase only the *entry* guard produces. `assemble` guards
    // the same bound a second time and says something shorter, so an assertion
    // on the shared "pre-partition the population" would survive the entry
    // guard being deleted — and the entry guard is the one that matters, since
    // it refuses before the pairwise estimation rather than after it. Deleting
    // both is not a mutation this suite runs: it admits the population and the
    // cubic merge loop then runs for over twenty minutes of CPU in a debug
    // build, which was measured, not guessed.
    assert!(
        error.message().contains("rather than raising the bound"),
        "the refusal must name the alternative and refuse it before the estimation work: {}",
        error.message()
    );
    Ok(())
}

// --- the stress window ------------------------------------------------------

#[test]
fn a_stress_window_that_leaves_too_little_calm_to_compare_against_is_refused() -> Result<()> {
    let benchmark: Vec<f64> = (0..OBSERVATIONS).map(|t| -(t as f64)).collect();
    // Premise: a moderate quantile on this very benchmark is admitted, so the
    // refusal below is about the quantile and not about the series.
    let admitted = StressWindow::worst_quantile(&benchmark, 0.4, "fixture")?;
    assert_eq!(admitted.stress_indices().len(), 16);

    let error = StressWindow::worst_quantile(&benchmark, 0.95, "fixture")
        .expect_err("a window covering almost everything leaves no calm sample");
    assert!(
        error.message().contains("calm complement"),
        "the refusal must name the side that is short: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_stress_window_too_thin_to_estimate_a_correlation_from_is_refused() -> Result<()> {
    let benchmark: Vec<f64> = (0..OBSERVATIONS).map(|t| -(t as f64)).collect();
    let error = StressWindow::worst_quantile(&benchmark, 0.05, "fixture")
        .expect_err("two observations do not make a correlation");
    assert!(
        error
            .message()
            .contains("rather than clustering on the calm sample"),
        "the refusal must say what it is refusing to fall back to: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_repeated_stress_observation_is_refused() {
    let mut indices = stress_indices();
    indices.push(3);
    let error = StressWindow::explicit(OBSERVATIONS, indices, "fixture")
        .expect_err("a repeat weights one observation twice");
    assert!(
        error.message().contains("repeats an observation"),
        "the refusal must name the defect: {}",
        error.message()
    );
}

#[test]
fn a_stress_quantile_outside_the_open_unit_interval_is_refused() {
    let benchmark: Vec<f64> = (0..OBSERVATIONS).map(|t| -(t as f64)).collect();
    for quantile in [0.0, 1.0, -0.2, 1.5, f64::NAN] {
        let error = StressWindow::worst_quantile(&benchmark, quantile, "fixture")
            .expect_err("a quantile must lie strictly inside (0, 1)");
        assert!(
            error.message().contains("strictly between 0 and 1"),
            "the refusal must name the range: {}",
            error.message()
        );
    }
}

#[test]
fn a_benchmark_whose_high_readings_are_the_stress_has_its_high_tail_cut() -> Result<()> {
    // `worst_quantile`'s own doc names a volatility index and a funding spread
    // as valid benchmarks, and both are worst at their *highest*. Taking the
    // low tail of one selects the calmest sessions in the sample and labels
    // them stress, and the families keyed on them are calm-keyed families
    // reported as stress-keyed — the one outcome this module exists to
    // prevent, arrived at from the other end.
    let index: Vec<f64> = (0..OBSERVATIONS).map(|t| t as f64).collect();

    // Premise: on this benchmark the two axes cannot agree, so the assertion
    // below is about the axis and not about the series.
    let low = StressWindow::worst_quantile(&index, 0.4, "fixture: a return series")?;
    assert_eq!(low.stress_indices(), &(0..16).collect::<Vec<_>>()[..]);

    let high = StressWindow::worst_quantile_on(
        &index,
        0.4,
        StressAxis::HighReadingsAreStress,
        "fixture: a volatility index",
    )?;
    assert_eq!(
        high.stress_indices(),
        &(24..40).collect::<Vec<_>>()[..],
        "a volatility index is worst at its highest readings"
    );
    assert_eq!(high.calm_indices(), (0..24).collect::<Vec<_>>());

    // And the record says which tail was cut, so a reader of the clustering
    // does not have to know which constructor the caller reached for.
    assert!(
        high.provenance().contains("the highest readings"),
        "the provenance must record the axis: {}",
        high.provenance()
    );
    assert!(
        low.provenance().contains("the lowest readings"),
        "the provenance must record the axis: {}",
        low.provenance()
    );
    Ok(())
}

#[test]
fn a_window_with_no_provenance_is_refused_before_the_axis_clause_could_supply_one() {
    // The axis clause is appended to the provenance. If it were appended first
    // it would be what made an empty provenance non-empty, and the guard
    // asking how stress was decided would answer itself.
    let benchmark: Vec<f64> = (0..OBSERVATIONS).map(|t| -(t as f64)).collect();
    let error = StressWindow::worst_quantile(&benchmark, 0.4, "   ")
        .expect_err("the record has to say how stress was decided");
    assert!(
        error.message().contains("needs a provenance"),
        "the refusal must name what is missing: {}",
        error.message()
    );
}

#[test]
fn a_strategy_that_does_not_move_outside_the_stress_window_is_refused() -> Result<()> {
    // A tail hedge: it moves in the drawdown and sits still the rest of the
    // time. `stats::correlation` fails soft — it returns exactly 0.0 when
    // either series is flat — so the calm matrix recorded a correlation of
    // zero that nobody estimated, and that zero is subtracted in
    // `mean_stress_excess` and decides `pairs_calm_would_have_misfiled`. Those
    // two numbers are what this design offers as its own evidence, so a
    // non-measurement inside them is worse than a gap.
    let stress: BTreeSet<usize> = stress_indices().into_iter().collect();
    let idiosyncratic = noise(31, OBSERVATIONS);
    let flat_in_calm: Vec<f64> = (0..OBSERVATIONS)
        .map(|t| {
            if stress.contains(&t) {
                idiosyncratic[t]
            } else {
                0.0
            }
        })
        .collect();

    // Premise: the series is valid and moves inside the window, so the calm
    // guard is the only thing that can refuse it — and the soft zero really is
    // what the statistics would have produced.
    let hedge = StrategyReturns::new(strategy("strategy-tail-hedge"), flat_in_calm.clone())?;
    assert_eq!(hedge.len(), OBSERVATIONS);
    let inside: Vec<f64> = stress_indices().iter().map(|t| flat_in_calm[*t]).collect();
    assert!(
        stats::stddev(&inside) > 0.0,
        "the hedge must move inside the stress window or the stress guard refuses it first"
    );
    let outside: Vec<f64> = (STRESS_LENGTH..OBSERVATIONS)
        .map(|t| flat_in_calm[t])
        .collect();
    // Compared exactly, through `total_cmp` rather than `==` because the
    // exactness is the whole point: `stats::correlation` does not approximate
    // zero here, it *returns the literal* when either side is flat, and a
    // tolerance would let this premise hold for a real correlation that
    // happened to be small.
    assert_eq!(
        stats::correlation(&outside, &noise(32, OBSERVATIONS - STRESS_LENGTH)).total_cmp(&0.0),
        std::cmp::Ordering::Equal,
        "the premise: a flat series correlates 0.0 with anything, which is a non-measurement \
         wearing a measurement's clothes"
    );

    let mut population = decoupling_population()?;
    population.push(hedge);
    let error = StressCorrelation::from_returns(&population, &window()?)
        .expect_err("a strategy with no calm variance has no measurable calm correlation");
    assert!(
        error
            .message()
            .contains("does not move outside the stress window"),
        "the refusal must name the defect: {}",
        error.message()
    );
    assert!(
        error.message().contains("strategy-tail-hedge"),
        "the refusal must name the strategy to exclude: {}",
        error.message()
    );

    // The other half: the population without it is still admitted, so this is
    // a guard on one strategy and not a refusal of the whole design.
    let clean = StressCorrelation::from_returns(&decoupling_population()?, &window()?)?;
    assert_eq!(clean.len(), 4);
    Ok(())
}

#[test]
fn the_worst_quantile_window_selects_the_worst_observations_and_nothing_else() -> Result<()> {
    // A benchmark that improves monotonically, so the worst readings are the
    // first ones and the expected answer is not a matter of opinion.
    let benchmark: Vec<f64> = (0..OBSERVATIONS).map(|t| t as f64).collect();
    let selected = StressWindow::worst_quantile(&benchmark, 0.5, "fixture: monotone benchmark")?;
    assert_eq!(selected.stress_indices(), &(0..20).collect::<Vec<_>>()[..]);
    assert_eq!(selected.calm_indices(), (20..40).collect::<Vec<_>>());
    Ok(())
}

// --- the partition ----------------------------------------------------------

#[test]
fn every_strategy_reaches_exactly_one_family_and_the_families_partition_the_population()
-> Result<()> {
    let correlation = decoupling_correlation()?;
    let assignment = FamilyClustering::new(2)?
        .with_linkage(Linkage::Complete)
        .cluster(&correlation)?;

    // Premise: there is more than one family and more than zero strategies, so
    // "no two families overlap" is not true by vacuity.
    assert_eq!(assignment.family_count(), 2);
    assert_eq!(assignment.strategy_count(), 4);

    let mut seen: BTreeSet<StrategyId> = BTreeSet::new();
    for members in assignment.families().values() {
        assert!(!members.is_empty(), "a family must not be empty");
        for member in members {
            assert!(
                seen.insert(member.clone()),
                "{member} reached two families; the assignment is not a partition"
            );
        }
    }
    assert_eq!(seen.len(), 4, "every strategy must reach a family");
    for member in &seen {
        assert!(
            assignment.family_of(member).is_some(),
            "the reverse index must agree with the forward one for {member}"
        );
    }
    Ok(())
}

#[test]
fn a_clustering_asked_for_one_family_per_strategy_merges_nothing() -> Result<()> {
    let correlation = decoupling_correlation()?;
    let assignment = FamilyClustering::new(4)?.cluster(&correlation)?;
    assert_eq!(assignment.family_count(), 4);
    for members in assignment.families().values() {
        assert_eq!(members.len(), 1);
    }
    Ok(())
}

// --- capital pools ----------------------------------------------------------

#[test]
fn pools_that_do_not_sum_to_the_total_are_refused() {
    let error = CapitalPools::new(
        Decimal::from_int(1_000),
        Decimal::from_int(100),
        Decimal::from_int(200),
        Decimal::from_int(300),
        Decimal::from_int(300),
        Decimal::ZERO,
    )
    .expect_err("capital belonging to no horizon is capital two horizons will both spend");
    assert!(
        error.message().contains("sum to 900"),
        "the refusal must name the sum it found: {}",
        error.message()
    );
    assert!(
        error.message().contains("a difference of -100"),
        "the refusal must name the gap: {}",
        error.message()
    );
}

#[test]
fn pools_that_sum_exactly_to_the_total_are_admitted() -> Result<()> {
    // The other half of the gate. A validator that refuses everything looks
    // identical to one that works until something legitimate arrives.
    let admitted = pools(1_000, 100, 200, 300, 400)?;
    assert_eq!(admitted.total(), Decimal::from_int(1_000));
    assert_eq!(
        admitted.pool_for(Horizon::MicrosecondsToMinutes),
        Decimal::from_int(100)
    );
    assert_eq!(admitted.pool_for(Horizon::Years), Decimal::from_int(400));
    Ok(())
}

#[test]
fn a_pool_short_by_one_unit_in_the_ninth_decimal_is_refused() -> Result<()> {
    // No tolerance, deliberately: these are exact decimals, so a nano is a
    // real difference and a tolerance here is the seam a unit gets spent
    // twice through.
    let one_nano = Decimal::from_raw(1);
    let error = CapitalPools::new(
        Decimal::from_int(1_000),
        Decimal::from_int(100) - one_nano,
        Decimal::from_int(200),
        Decimal::from_int(300),
        Decimal::from_int(400),
        Decimal::ZERO,
    )
    .expect_err("a nano is a real difference in exact arithmetic");
    assert!(error.message().contains("sum exactly"));
    Ok(())
}

#[test]
fn a_negative_pool_is_refused() {
    let error = CapitalPools::new(
        Decimal::from_int(1_000),
        Decimal::from_int(-100),
        Decimal::from_int(400),
        Decimal::from_int(300),
        Decimal::from_int(400),
        Decimal::ZERO,
    )
    .expect_err("a negative pool is a shortfall wearing a pool's clothes");
    assert!(
        error.message().contains("available_inventory"),
        "the refusal must name the pool: {}",
        error.message()
    );
}

// --- reconciliation ---------------------------------------------------------

#[test]
fn a_years_commitment_within_the_total_but_beyond_its_own_pool_is_still_a_breach() -> Result<()> {
    // The failure this whole capability exists to prevent: the total looks
    // fine, so a single capital check passes, and a multi-year position has
    // quietly been funded out of the inventory a market maker needs today.
    let pools = pools(1_000, 700, 100, 100, 100)?;
    let budgets = vec![FamilyBudget::from_money(
        FamilyId::new(0),
        Horizon::Years,
        Decimal::from_int(500),
    )?];
    let reconciliation = reconcile(&pools, &budgets)?;

    // Premise: the total is not breached, so nothing but a per-horizon check
    // could find this.
    assert!(
        reconciliation.allocated() < reconciliation.total(),
        "the premise is that a total-only check passes: allocated {} against total {}",
        reconciliation.allocated(),
        reconciliation.total()
    );

    assert!(!reconciliation.is_balanced());
    let breaches = reconciliation.breaches();
    assert_eq!(breaches.len(), 1);
    assert_eq!(breaches[0].horizon, Horizon::Years);
    assert_eq!(breaches[0].pool, Decimal::from_int(100));
    assert_eq!(breaches[0].committed, Decimal::from_int(500));

    let error = reconciliation
        .into_plan()
        .expect_err("an over-committed horizon yields no plan");
    assert!(
        error
            .message()
            .contains("years is over its pool of 100 by 400"),
        "the refusal must name the horizon and the overage: {}",
        error.message()
    );
    assert!(
        error.message().contains("will not trim them for you"),
        "the refusal must say it is not choosing which strategy goes unfunded: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_unfunded_commitment_beyond_the_reserved_pool_breaches_the_years_horizon() -> Result<()> {
    let budgets = vec![FamilyBudget::from_money(
        FamilyId::new(0),
        Horizon::Years,
        Decimal::from_int(80),
    )?];

    // Premise: with no unfunded commitment the same budget is balanced, so the
    // breach below is caused by the liability and by nothing else.
    let without = CapitalPools::new(
        Decimal::from_int(1_000),
        Decimal::from_int(400),
        Decimal::from_int(300),
        Decimal::from_int(200),
        Decimal::from_int(100),
        Decimal::ZERO,
    )?;
    assert!(reconcile(&without, &budgets)?.is_balanced());

    let with = CapitalPools::new(
        Decimal::from_int(1_000),
        Decimal::from_int(400),
        Decimal::from_int(300),
        Decimal::from_int(200),
        Decimal::from_int(100),
        Decimal::from_int(50),
    )?;
    let reconciliation = reconcile(&with, &budgets)?;
    let years = reconciliation
        .position(Horizon::Years)
        .expect("the years horizon is always reported");
    assert_eq!(years.liability, Decimal::from_int(50));
    assert_eq!(
        years.committed,
        Decimal::from_int(130),
        "the reserved pool must meet the budget and the liability together"
    );
    assert!(
        !reconciliation.is_balanced(),
        "a commitment nobody allocated against is exactly the one that surprises a desk"
    );
    Ok(())
}

#[test]
fn the_unfunded_commitment_liability_is_charged_only_to_the_years_horizon() -> Result<()> {
    let pools = CapitalPools::new(
        Decimal::from_int(1_000),
        Decimal::from_int(400),
        Decimal::from_int(300),
        Decimal::from_int(200),
        Decimal::from_int(100),
        Decimal::from_int(40),
    )?;
    let reconciliation = reconcile(&pools, &[])?;
    // Premise: there is a liability at all.
    assert_eq!(pools.unfunded_commitments(), Decimal::from_int(40));
    for horizon in Horizon::ALL {
        let position = reconciliation
            .position(horizon)
            .expect("every horizon is reported even with no budgets");
        let expected = if horizon == Horizon::Years {
            Decimal::from_int(40)
        } else {
            Decimal::ZERO
        };
        assert_eq!(
            position.liability, expected,
            "{horizon} carried the wrong liability"
        );
    }
    Ok(())
}

#[test]
fn a_balanced_reconciliation_yields_a_plan_holding_each_family_its_own_budget() -> Result<()> {
    let pools = pools(1_000, 400, 300, 200, 100)?;
    let budgets = vec![
        FamilyBudget::from_money(
            FamilyId::new(0),
            Horizon::MicrosecondsToMinutes,
            Decimal::from_int(350),
        )?,
        FamilyBudget::from_money(
            FamilyId::new(1),
            Horizon::HoursToDays,
            Decimal::from_int(250),
        )?,
        FamilyBudget::from_money(FamilyId::new(2), Horizon::Years, Decimal::from_int(90))?,
    ];
    let reconciliation = reconcile(&pools, &budgets)?;
    assert!(reconciliation.is_balanced());
    assert!(reconciliation.breaches().is_empty());

    let plan = reconciliation.into_plan()?;
    assert_eq!(plan.allocations().len(), 3);
    assert_eq!(
        plan.allocation_for(FamilyId::new(0)),
        Some(Decimal::from_int(350))
    );
    assert_eq!(
        plan.allocation_for(FamilyId::new(2)),
        Some(Decimal::from_int(90))
    );
    assert_eq!(plan.allocated(), Decimal::from_int(690));
    assert_eq!(plan.total(), Decimal::from_int(1_000));
    Ok(())
}

#[test]
fn over_committing_the_whole_total_always_shows_up_as_at_least_one_breached_horizon() -> Result<()>
{
    let pools = pools(1_000, 400, 300, 200, 100)?;
    let budgets = vec![
        FamilyBudget::from_money(
            FamilyId::new(0),
            Horizon::MicrosecondsToMinutes,
            Decimal::from_int(400),
        )?,
        FamilyBudget::from_money(
            FamilyId::new(1),
            Horizon::HoursToDays,
            Decimal::from_int(700),
        )?,
    ];
    let reconciliation = reconcile(&pools, &budgets)?;
    // Premise: the families really did claim more than exists.
    assert!(reconciliation.unallocated().is_negative());
    assert!(
        !reconciliation.breaches().is_empty(),
        "the pools sum to the total, so over-claiming the total must breach a pool"
    );
    Ok(())
}

#[test]
fn a_family_budgeted_twice_is_refused() -> Result<()> {
    let pools = pools(1_000, 400, 300, 200, 100)?;
    let budgets = vec![
        FamilyBudget::from_money(
            FamilyId::new(0),
            Horizon::MicrosecondsToMinutes,
            Decimal::from_int(100),
        )?,
        FamilyBudget::from_money(
            FamilyId::new(0),
            Horizon::HoursToDays,
            Decimal::from_int(100),
        )?,
    ];
    let error = reconcile(&pools, &budgets)
        .expect_err("two claims on the same capital cannot both be the record");
    assert!(
        error.message().contains("budgeted twice"),
        "the refusal must name the defect: {}",
        error.message()
    );
    Ok(())
}

// --- the crossing from statistics to money ----------------------------------

#[test]
fn a_non_finite_weight_is_refused_at_the_crossing_from_statistics_to_money() {
    for weight in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = FamilyBudget::from_weight(
            FamilyId::new(0),
            Horizon::HoursToDays,
            weight,
            Decimal::from_int(1_000),
        )
        .expect_err("a non-finite weight cannot become capital");
        // Deliberately not an `or` across two possible messages. Every one of
        // these three must be refused by the non-finite check specifically:
        // an infinity that fell through to the "above the whole of capital"
        // arm would be an accident, and a NaN has no arm to fall through to.
        assert!(
            error.message().contains("non-finite weight"),
            "the refusal must name the defect: {}",
            error.message()
        );
    }
}

#[test]
fn a_weight_above_the_whole_of_capital_is_refused_rather_than_clamped() {
    let error = FamilyBudget::from_weight(
        FamilyId::new(3),
        Horizon::WeeksToMonths,
        1.4,
        Decimal::from_int(1_000),
    )
    .expect_err("one family cannot claim more capital than exists");
    assert!(
        error.message().contains("rescale the allocation"),
        "the refusal must name the fix: {}",
        error.message()
    );
}

#[test]
fn a_negative_weight_is_refused() {
    let error = FamilyBudget::from_weight(
        FamilyId::new(3),
        Horizon::WeeksToMonths,
        -0.1,
        Decimal::from_int(1_000),
    )
    .expect_err("a weight is a non-negative fraction of capital");
    assert!(error.message().contains("non-negative fraction"));
}

#[test]
fn the_rounding_residue_of_the_weight_to_money_crossing_is_visible_and_is_not_a_breach()
-> Result<()> {
    // Three families at a third each. The crossing rounds to nine decimals, so
    // the allocations cannot sum to the total exactly. That residue must
    // appear as unallocated capital, not as a phantom over-commitment.
    let pools = pools(90, 30, 30, 30, 0)?;
    let third = 1.0 / 3.0;
    let budgets = vec![
        FamilyBudget::from_weight(
            FamilyId::new(0),
            Horizon::MicrosecondsToMinutes,
            third,
            pools.total(),
        )?,
        FamilyBudget::from_weight(FamilyId::new(1), Horizon::HoursToDays, third, pools.total())?,
        FamilyBudget::from_weight(
            FamilyId::new(2),
            Horizon::WeeksToMonths,
            third,
            pools.total(),
        )?,
    ];
    let reconciliation = reconcile(&pools, &budgets)?;

    // Premise: the residue exists at all — the allocations really did not sum
    // to the total.
    assert!(
        reconciliation.unallocated().is_positive(),
        "a third of ninety three times must leave a residue for this test to be about anything"
    );
    assert!(
        reconciliation.unallocated() < Decimal::from_scaled(1, 6).unwrap_or(Decimal::ONE),
        "the residue must be a rounding artefact, not a real gap: {}",
        reconciliation.unallocated()
    );
    assert!(
        reconciliation.is_balanced(),
        "a nine-decimal rounding residue must not read as an over-commitment"
    );
    Ok(())
}

// --- the horizon by family seam ---------------------------------------------

#[test]
fn a_family_whose_members_straddle_two_horizons_is_refused() -> Result<()> {
    let assignment = FamilyClustering::new(2)?.cluster(&decoupling_correlation()?)?;
    let members: Vec<StrategyId> = assignment.strategies().cloned().collect();
    assert_eq!(members.len(), 4);

    // Premise: with one horizon for everyone the seam is admitted, so the
    // refusal below is about the straddle and not about the fixture.
    let uniform: BTreeMap<StrategyId, Horizon> = members
        .iter()
        .map(|s| (s.clone(), Horizon::HoursToDays))
        .collect();
    let admitted = family_horizons(&assignment, &uniform)?;
    assert_eq!(admitted.len(), 2);

    // Now give one family's two members different horizons.
    let (family, pair) = assignment
        .families()
        .iter()
        .find(|(_, m)| m.len() > 1)
        .expect("the fixture produces a family with more than one member");
    let mut straddled = uniform;
    let first = pair.iter().next().expect("the family has a first member");
    straddled.insert(first.clone(), Horizon::Years);

    let error = family_horizons(&assignment, &straddled)
        .expect_err("a family straddling two pools cannot be reconciled against either");
    assert!(
        error.message().contains(&format!("{family} spans the")),
        "the refusal must name the family: {}",
        error.message()
    );
    assert!(
        error.message().contains("cluster within a horizon bucket"),
        "the refusal must name the fix: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_clustered_strategy_that_declares_no_horizon_is_refused() -> Result<()> {
    let assignment = FamilyClustering::new(2)?.cluster(&decoupling_correlation()?)?;
    let mut declared: BTreeMap<StrategyId, Horizon> = assignment
        .strategies()
        .map(|s| (s.clone(), Horizon::HoursToDays))
        .collect();
    // Premise: complete declarations are admitted.
    assert_eq!(family_horizons(&assignment, &declared)?.len(), 2);

    declared.remove(&strategy("strategy-a"));
    let error = family_horizons(&assignment, &declared)
        .expect_err("a strategy with no horizon has no pool to be allocated against");
    assert!(
        error.message().contains("declares no horizon"),
        "the refusal must name the defect: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_horizon_names_are_delimited_tokens_rather_than_substrings_of_one_another() {
    // `contains("years")` is true of nothing else here only because the names
    // were chosen that way; this test is what keeps it true.
    let names: Vec<&str> = Horizon::ALL.iter().map(Horizon::as_str).collect();
    assert_eq!(names.len(), 4);
    for (i, outer) in names.iter().enumerate() {
        for (j, inner) in names.iter().enumerate() {
            if i == j {
                continue;
            }
            assert!(
                !outer.contains(inner),
                "{outer} contains {inner}; a substring match on one horizon would match the other"
            );
        }
    }
}

// --- the fixture itself -----------------------------------------------------

#[test]
fn the_fixture_population_really_does_decouple_in_calm_and_couple_in_stress() -> Result<()> {
    // Every clustering test above rests on this. If the fixture drifts, this
    // fails first and says so, rather than the clustering tests failing in a
    // way that looks like a clustering bug.
    let population = decoupling_population()?;
    let stress: BTreeSet<usize> = stress_indices().into_iter().collect();
    let slice = |series: &StrategyReturns, want_stress: bool| -> Vec<f64> {
        (0..OBSERVATIONS)
            .filter(|t| stress.contains(t) == want_stress)
            .map(|t| series.returns()[t])
            .collect()
    };
    let stressed = stats::correlation(&slice(&population[0], true), &slice(&population[1], true));
    let calm = stats::correlation(&slice(&population[0], false), &slice(&population[1], false));
    assert!(
        stressed > 0.8,
        "a and b must move together in stress: {stressed}"
    );
    assert!(
        calm.abs() < 0.4,
        "a and b must look independent in calm: {calm}"
    );
    assert!(
        stressed - calm > 0.5,
        "the understatement must be large enough to change a family boundary: {stressed} vs {calm}"
    );
    Ok(())
}
