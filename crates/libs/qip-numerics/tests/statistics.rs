//! Estimators, distributions and regression.

// Exact float comparison is deliberate here: these assertions cover degenerate
// cases and identities where the function must return an exact sentinel value,
// not merely something close to it.
#![allow(clippy::float_cmp)]

use qip_core::testing::{Property, approx_eq};
use qip_numerics::distributions as dist;
use qip_numerics::matrix::Matrix;
use qip_numerics::stats;

#[test]
fn moments_match_hand_computed_values() {
    let x = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
    assert!(approx_eq(stats::mean(&x), 5.0, 1e-12));
    assert!(approx_eq(stats::variance_population(&x), 4.0, 1e-12));
    // Sample variance uses n-1: 32/7.
    assert!(approx_eq(stats::variance(&x), 32.0 / 7.0, 1e-12));
    assert!(approx_eq(stats::median(&x), 4.5, 1e-12));
}

#[test]
fn degenerate_inputs_do_not_panic() {
    assert_eq!(stats::mean(&[]), 0.0);
    assert_eq!(stats::variance(&[1.0]), 0.0);
    assert_eq!(stats::stddev(&[]), 0.0);
    assert_eq!(stats::skewness(&[1.0, 2.0]), 0.0);
    assert_eq!(stats::excess_kurtosis(&[1.0, 2.0, 3.0]), 0.0);
    assert!(stats::quantile(&[], 0.5).is_nan());
    assert_eq!(stats::correlation(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]), 0.0);
}

#[test]
fn quantiles_interpolate_between_order_statistics() {
    let x = [1.0, 2.0, 3.0, 4.0];
    assert!(approx_eq(stats::quantile(&x, 0.0), 1.0, 1e-12));
    assert!(approx_eq(stats::quantile(&x, 1.0), 4.0, 1e-12));
    assert!(approx_eq(stats::quantile(&x, 0.5), 2.5, 1e-12));
    // Type-7: position = q*(n-1) = 0.75 -> between 1.0 and 2.0.
    assert!(approx_eq(stats::quantile(&x, 0.25), 1.75, 1e-12));
}

#[test]
fn quantiles_ignore_input_order() {
    let sorted = [1.0, 2.0, 3.0, 4.0, 5.0];
    let jumbled = [3.0, 1.0, 5.0, 2.0, 4.0];
    for q in [0.0, 0.1, 0.25, 0.5, 0.9, 1.0] {
        assert!(approx_eq(
            stats::quantile(&sorted, q),
            stats::quantile(&jumbled, q),
            1e-12
        ));
    }
}

#[test]
fn correlation_is_bounded_and_exact_for_perfect_relationships() {
    let x = [1.0, 2.0, 3.0, 4.0, 5.0];
    let same: Vec<f64> = x.to_vec();
    let opposite: Vec<f64> = x.iter().map(|v| -v).collect();
    assert!(approx_eq(stats::correlation(&x, &same), 1.0, 1e-12));
    assert!(approx_eq(stats::correlation(&x, &opposite), -1.0, 1e-12));
}

#[test]
fn rank_correlation_survives_a_monotone_transform() {
    let x = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let y: Vec<f64> = x.iter().map(|v: &f64| v.exp()).collect();
    // Pearson is degraded by the non-linearity; Spearman is not.
    assert!(approx_eq(stats::rank_correlation(&x, &y), 1.0, 1e-12));
    assert!(stats::correlation(&x, &y) < 1.0);
}

#[test]
fn ranks_average_ties() {
    let r = stats::ranks(&[10.0, 20.0, 20.0, 30.0]);
    assert!(approx_eq(r[0], 1.0, 1e-12));
    assert!(approx_eq(r[1], 2.5, 1e-12), "{r:?}");
    assert!(approx_eq(r[2], 2.5, 1e-12), "{r:?}");
    assert!(approx_eq(r[3], 4.0, 1e-12));
}

#[test]
fn median_absolute_deviation_ignores_an_outlier() {
    let clean = [1.0, 2.0, 3.0, 4.0, 5.0];
    let mut polluted = clean.to_vec();
    polluted.push(1000.0);
    let mad_clean = stats::median_absolute_deviation(&clean);
    let mad_polluted = stats::median_absolute_deviation(&polluted);
    assert!(
        (mad_clean - mad_polluted).abs() < mad_clean * 0.6,
        "MAD moved too far"
    );
    // The standard deviation, by contrast, is destroyed.
    assert!(stats::stddev(&polluted) > 10.0 * stats::stddev(&clean));
}

#[test]
fn covariance_matrix_matches_pairwise_covariance() {
    let observations = Matrix::from_rows(&[
        vec![0.01, 0.02, -0.01],
        vec![-0.005, 0.01, 0.002],
        vec![0.02, -0.01, 0.005],
        vec![0.0, 0.005, 0.001],
        vec![0.015, 0.0, -0.003],
    ])
    .unwrap();
    let cov = stats::covariance_matrix(&observations).unwrap();
    for i in 0..3 {
        for j in 0..3 {
            let expected = stats::covariance(&observations.column(i), &observations.column(j));
            assert!(approx_eq(cov.get(i, j), expected, 1e-14), "({i},{j})");
        }
    }
    assert!(cov.is_symmetric(1e-15));
}

#[test]
fn correlation_from_covariance_has_unit_diagonal() {
    let observations = Matrix::from_rows(&[
        vec![0.01, 0.02],
        vec![-0.005, 0.01],
        vec![0.02, -0.01],
        vec![0.0, 0.005],
    ])
    .unwrap();
    let cov = stats::covariance_matrix(&observations).unwrap();
    let corr = stats::correlation_from_covariance(&cov).unwrap();
    assert!(approx_eq(corr.get(0, 0), 1.0, 1e-12));
    assert!(approx_eq(corr.get(1, 1), 1.0, 1e-12));
    assert!(corr.get(0, 1).abs() <= 1.0);
}

#[test]
fn shrinkage_moves_toward_the_identity_and_conditions_the_matrix() {
    let noisy = Matrix::from_rows(&[
        vec![0.04, 0.0399, 0.0398],
        vec![0.0399, 0.04, 0.0399],
        vec![0.0398, 0.0399, 0.04],
    ])
    .unwrap();
    let before = noisy.condition_number();
    let shrunk = stats::shrink_covariance(&noisy, 0.3).unwrap();
    assert!(
        shrunk.condition_number() < before,
        "shrinkage should improve conditioning"
    );
    // Trace is preserved: shrinking toward a scaled identity keeps total variance.
    assert!(approx_eq(shrunk.trace(), noisy.trace(), 1e-12));
    // Zero intensity is a no-op.
    let unchanged = stats::shrink_covariance(&noisy, 0.0).unwrap();
    assert!(approx_eq(unchanged.get(0, 1), noisy.get(0, 1), 1e-15));
}

#[test]
fn ewma_weights_recent_observations_more() {
    let rising = [1.0, 2.0, 3.0, 4.0, 100.0];
    let ewma = stats::ewma(&rising, 0.94);
    let plain = stats::mean(&rising);
    assert!(ewma > plain, "EWMA {ewma} should exceed the mean {plain}");
    // A constant series has the same EWMA regardless of decay.
    assert!(approx_eq(stats::ewma(&[5.0; 20], 0.5), 5.0, 1e-9));
}

#[test]
fn autocorrelation_detects_a_trend_and_ignores_noise() {
    let trend: Vec<f64> = (0..100).map(|i| i as f64).collect();
    assert!(stats::autocorrelation(&trend, 1) > 0.9);
    assert!(approx_eq(stats::autocorrelation(&trend, 0), 1.0, 1e-12));

    let alternating: Vec<f64> = (0..100)
        .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    assert!(stats::autocorrelation(&alternating, 1) < -0.9);
}

#[test]
fn running_stats_match_the_batch_computation() {
    let data: Vec<f64> = (0..500)
        .map(|i| (i as f64 * 0.37).sin() * 12.0 + 3.0)
        .collect();
    let mut running = stats::RunningStats::new();
    for v in &data {
        running.push(*v);
    }
    assert_eq!(running.count(), 500);
    assert!(approx_eq(running.mean(), stats::mean(&data), 1e-9));
    assert!(approx_eq(running.stddev(), stats::stddev(&data), 1e-9));
    assert!(approx_eq(
        running.min(),
        data.iter().cloned().fold(f64::INFINITY, f64::min),
        1e-12
    ));
}

#[test]
fn running_stats_ignore_non_finite_input() {
    let mut running = stats::RunningStats::new();
    running.push(1.0);
    running.push(f64::NAN);
    running.push(f64::INFINITY);
    running.push(3.0);
    assert_eq!(running.count(), 2);
    assert!(approx_eq(running.mean(), 2.0, 1e-12));
}

#[test]
fn returns_conversions_are_consistent() {
    let prices = [100.0, 110.0, 99.0];
    let simple = stats::simple_returns(&prices);
    assert!(approx_eq(simple[0], 0.1, 1e-12));
    assert!(approx_eq(simple[1], -0.1, 1e-12));

    let log = stats::log_returns(&prices);
    // Log returns compound additively back to the total simple return.
    let total_log: f64 = log.iter().sum();
    let total_simple = prices[2] / prices[0] - 1.0;
    assert!(approx_eq(total_log.exp() - 1.0, total_simple, 1e-12));

    let growth = stats::cumulative_growth(&simple);
    assert!(approx_eq(
        growth[growth.len() - 1],
        prices[2] / prices[0],
        1e-12
    ));
}

// --- regression -------------------------------------------------------------

#[test]
fn ols_recovers_known_coefficients() {
    // y = 2.5 + 1.5*x1 - 0.75*x2, noiseless.
    let n = 60;
    let mut rows = Vec::new();
    let mut y = Vec::new();
    for i in 0..n {
        let x1 = i as f64 * 0.1;
        let x2 = (i as f64 * 0.37).sin();
        rows.push(vec![x1, x2]);
        y.push(2.5 + 1.5 * x1 - 0.75 * x2);
    }
    let design = Matrix::from_rows(&rows).unwrap();
    let fit = stats::ols(&design, &y).unwrap();

    assert!(
        approx_eq(fit.coefficients[0], 2.5, 1e-8),
        "intercept {}",
        fit.coefficients[0]
    );
    assert!(
        approx_eq(fit.coefficients[1], 1.5, 1e-8),
        "beta1 {}",
        fit.coefficients[1]
    );
    assert!(
        approx_eq(fit.coefficients[2], -0.75, 1e-8),
        "beta2 {}",
        fit.coefficients[2]
    );
    assert!(fit.r_squared > 0.999_999, "R2 {}", fit.r_squared);
    assert!(fit.residual_stddev < 1e-8);
}

#[test]
fn ols_detects_a_real_effect() {
    use qip_core::rng::{Rng, Xoshiro256};
    let mut rng = Xoshiro256::seeded(4);
    let n = 400;
    let mut rows = Vec::new();
    let mut y = Vec::new();
    for _ in 0..n {
        let signal = rng.normal();
        let unrelated = rng.normal();
        rows.push(vec![signal, unrelated]);
        y.push(0.8 * signal + 0.5 * rng.normal());
    }
    let fit = stats::ols(&Matrix::from_rows(&rows).unwrap(), &y).unwrap();
    assert!(
        fit.is_significant(1, 0.001),
        "a 0.8 loading on 400 points must be significant"
    );
    assert!(
        approx_eq(fit.coefficients[1], 0.8, 0.1),
        "beta {}",
        fit.coefficients[1]
    );
    let p = fit.p_values();
    assert!(
        p.iter().all(|v| (0.0..=1.0).contains(v)),
        "p-values out of range: {p:?}"
    );
}

#[test]
fn ols_p_values_are_calibrated_under_the_null() {
    // A single trial can reject by chance, so assert the *rate* instead: with a
    // pure-noise regressor, rejections at alpha should occur about alpha of the
    // time. This is what makes the p-values usable as a research guardrail.
    use qip_core::rng::{Rng, Xoshiro256};
    let trials = 400;
    let alpha = 0.05;
    let mut rejections = 0;
    for trial in 0..trials {
        let mut rng = Xoshiro256::seeded(9_000 + trial as u64);
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for _ in 0..80 {
            rows.push(vec![rng.normal()]);
            y.push(rng.normal());
        }
        let fit = stats::ols(&Matrix::from_rows(&rows).unwrap(), &y).unwrap();
        if fit.is_significant(1, alpha) {
            rejections += 1;
        }
    }
    let rate = rejections as f64 / trials as f64;
    // Binomial standard error at alpha=0.05 over 400 trials is ~1.1pp; a
    // +/-3.5pp band is roughly three standard errors.
    assert!(
        (0.015..0.085).contains(&rate),
        "false positive rate {rate} is far from the nominal {alpha}"
    );
}

#[test]
fn ols_refuses_an_underdetermined_system() {
    let design = Matrix::from_rows(&[vec![1.0, 2.0], vec![3.0, 4.0]]).unwrap();
    assert!(stats::ols(&design, &[1.0, 2.0]).is_err());
}

// --- distributions ----------------------------------------------------------

#[test]
fn normal_cdf_matches_published_values() {
    let cases = [
        (-3.0, 0.001_349_898_031_630_09),
        (-1.96, 0.024_997_895_148_220_43),
        (-1.0, 0.158_655_253_931_457_05),
        (0.0, 0.5),
        (1.0, 0.841_344_746_068_542_9),
        (1.645, 0.950_015_094_460_878_6),
        (2.326, 0.989_990_724_659_132_4),
        (3.0, 0.998_650_101_968_369_9),
    ];
    for (x, want) in cases {
        let got = dist::normal_cdf(x);
        assert!(
            approx_eq(got, want, 1e-12),
            "normal_cdf({x}) = {got}, want {want}"
        );
    }
}

#[test]
fn normal_cdf_keeps_precision_in_the_far_tail() {
    // At -6 sigma the true value is ~9.87e-10; a naive 0.5*(1+erf) cancels here.
    let got = dist::normal_cdf(-6.0);
    assert!(
        approx_eq(got, 9.865_876_449_133_282e-10, 1e-12),
        "got {got}"
    );
    assert!(
        dist::normal_cdf(-8.0) > 0.0,
        "must not underflow to zero prematurely"
    );
}

#[test]
fn normal_quantiles_invert_the_cdf() {
    for p in [
        0.001, 0.01, 0.025, 0.05, 0.1, 0.5, 0.9, 0.95, 0.975, 0.99, 0.999,
    ] {
        let x = dist::normal_inv_cdf(p);
        assert!(
            approx_eq(dist::normal_cdf(x), p, 1e-12),
            "p={p} round trip gave {}",
            dist::normal_cdf(x)
        );
    }
    // The quantiles risk reporting is built on.
    assert!(approx_eq(
        dist::normal_inv_cdf(0.95),
        1.644_853_626_951_47,
        1e-9
    ));
    assert!(approx_eq(
        dist::normal_inv_cdf(0.99),
        2.326_347_874_040_84,
        1e-9
    ));
    assert!(approx_eq(
        dist::normal_inv_cdf(0.05),
        -1.644_853_626_951_47,
        1e-9
    ));
}

#[test]
fn normal_pdf_and_symmetry() {
    assert!(approx_eq(
        dist::normal_pdf(0.0),
        0.398_942_280_401_432_7,
        1e-15
    ));
    assert!(approx_eq(
        dist::normal_pdf(1.0),
        dist::normal_pdf(-1.0),
        1e-15
    ));
    assert!(approx_eq(
        dist::normal_cdf(0.7) + dist::normal_cdf(-0.7),
        1.0,
        1e-14
    ));
}

#[test]
fn erf_matches_published_values() {
    assert!(approx_eq(dist::erf(0.0), 0.0, 1e-15));
    assert!(approx_eq(dist::erf(0.5), 0.520_499_877_813_046_5, 1e-13));
    assert!(approx_eq(dist::erf(1.0), 0.842_700_792_949_714_9, 1e-13));
    assert!(approx_eq(dist::erf(2.0), 0.995_322_265_018_952_7, 1e-13));
    assert!(approx_eq(dist::erf(-1.0), -0.842_700_792_949_714_9, 1e-13));
    assert!(approx_eq(
        dist::erfc(1.0),
        1.0 - 0.842_700_792_949_714_9,
        1e-13
    ));
    assert!(approx_eq(dist::erfc(3.0), 2.209_049_699_858_544e-5, 1e-15));
}

#[test]
fn ln_gamma_matches_factorials() {
    // ln_gamma(n+1) = ln(n!).
    assert!(approx_eq(dist::ln_gamma(1.0), 0.0, 1e-12));
    assert!(approx_eq(dist::ln_gamma(5.0), 24.0_f64.ln(), 1e-11));
    assert!(approx_eq(dist::ln_gamma(11.0), 3_628_800.0_f64.ln(), 1e-10));
    // Gamma(1/2) = sqrt(pi).
    assert!(approx_eq(
        dist::ln_gamma(0.5),
        std::f64::consts::PI.sqrt().ln(),
        1e-12
    ));
}

#[test]
fn student_t_approaches_the_normal_as_degrees_of_freedom_grow() {
    let x = 1.5;
    let normal = dist::normal_cdf(x);
    assert!(
        (dist::student_t_cdf(x, 5.0) - normal).abs() > 1e-3,
        "t(5) should differ"
    );
    assert!(
        (dist::student_t_cdf(x, 10_000.0) - normal).abs() < 1e-3,
        "t(10000) should converge"
    );
    // Symmetry.
    assert!(approx_eq(
        dist::student_t_cdf(-x, 8.0),
        1.0 - dist::student_t_cdf(x, 8.0),
        1e-12
    ));
    // Known 95% quantile for 10 degrees of freedom.
    assert!(approx_eq(
        dist::student_t_inv_cdf(0.95, 10.0),
        1.812_461,
        1e-5
    ));
}

#[test]
fn chi_square_cdf_matches_known_quantiles() {
    // 95th percentile of chi-square with 1 df is 3.8415.
    assert!(approx_eq(dist::chi_square_cdf(3.841_459, 1.0), 0.95, 1e-6));
    // With 2 df it is 5.9915.
    assert!(approx_eq(dist::chi_square_cdf(5.991_465, 2.0), 0.95, 1e-6));
    assert_eq!(dist::chi_square_cdf(-1.0, 2.0), 0.0);
}

#[test]
fn property_normal_cdf_is_monotone_and_bounded() {
    Property::new("normal cdf monotone").for_all(
        |r| {
            use qip_core::rng::Rng;
            r.uniform(-8.0, 8.0)
        },
        |x| {
            let p = dist::normal_cdf(*x);
            if !(0.0..=1.0).contains(&p) {
                return Err(format!("cdf({x}) = {p} out of [0,1]"));
            }
            if dist::normal_cdf(x + 0.01) < p {
                return Err(format!("not monotone at {x}"));
            }
            Ok(())
        },
    );
}
