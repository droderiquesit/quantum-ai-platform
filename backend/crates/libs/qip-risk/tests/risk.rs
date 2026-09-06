//! Risk metrics, factor decomposition and the limit engine.

// Exact comparison is deliberate where a degenerate input must yield exactly
// zero rather than something close to it.
#![allow(clippy::float_cmp)]

use qip_core::Decimal;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::approx_eq;
use qip_numerics::matrix::Matrix;
use qip_risk::factor::FactorRisk;
use qip_risk::limits::{
    EXPECTED_SHORTFALL_FIGURE, Limit, LimitKind, LimitSet, RiskState, Severity,
    VALUE_AT_RISK_FIGURE, VOLATILITY_FIGURE,
};
use qip_risk::metrics::{
    self, DrawdownProfile, RiskMetrics, TailRisk, drawdown_profile, expected_shortfall,
    historical_var, parametric_var,
};
use std::collections::BTreeMap;

fn normal_returns(seed: u64, n: usize, mean: f64, sd: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n).map(|_| rng.normal_with(mean, sd)).collect()
}

// --- value at risk ----------------------------------------------------------

#[test]
fn historical_var_is_the_empirical_quantile() {
    // A hand-checkable series: returns from -0.10 to 0.09 in steps of 0.01.
    let returns: Vec<f64> = (0..20).map(|i| (i as f64 - 10.0) / 100.0).collect();
    let var_95 = historical_var(&returns, 0.95);
    // The 5th percentile of twenty ordered points sits near the worst.
    assert!((0.08..=0.10).contains(&var_95), "var {var_95}");
    // VaR is reported as a positive loss.
    assert!(var_95 > 0.0);
    // A higher confidence reaches further into the tail.
    assert!(historical_var(&returns, 0.99) >= var_95);
}

#[test]
fn expected_shortfall_always_exceeds_value_at_risk() {
    // The mean of the tail cannot be closer to zero than the tail's boundary.
    let returns = normal_returns(1, 2000, 0.0004, 0.012);
    for confidence in [0.90, 0.95, 0.99] {
        let var = historical_var(&returns, confidence);
        let shortfall = expected_shortfall(&returns, confidence);
        assert!(
            shortfall >= var - 1e-12,
            "at {confidence}: shortfall {shortfall} below var {var}"
        );
    }
}

#[test]
fn a_normal_model_understates_a_fat_tail() {
    // The reason both are computed and compared rather than one being chosen.
    // The tail has to be heavy *throughout*, not merely contaminated by rare
    // extremes: contamination that rare inflates the variance more than it
    // moves the 99% quantile, and a normal fit then overstates the loss.
    let mut rng = Xoshiro256::seeded(9);
    let fat_tailed: Vec<f64> = (0..6000).map(|_| rng.student_t(5.0) * 0.008).collect();

    let historical = historical_var(&fat_tailed, 0.99);
    let parametric = parametric_var(&fat_tailed, 0.99);
    assert!(
        historical > parametric,
        "historical {historical} should exceed parametric {parametric} on a fat tail"
    );

    let tail = TailRisk::compute(&fat_tailed);
    assert!(tail.tail_fatness > 1.0, "fatness {}", tail.tail_fatness);
    assert!(tail.understated_by_normal_model());
    assert!(
        tail.excess_kurtosis > 1.0,
        "kurtosis {}",
        tail.excess_kurtosis
    );
}

#[test]
fn rare_extreme_contamination_is_not_the_same_as_a_fat_tail() {
    // Worth pinning down, because it is a natural but wrong way to construct a
    // fat-tailed series: 1% of observations at ten times the volatility raises
    // the variance sharply while barely moving the 99% quantile, so the normal
    // fit ends up *over*stating the loss at that confidence.
    let mut rng = Xoshiro256::seeded(29);
    let contaminated: Vec<f64> = (0..6000)
        .map(|i| {
            if i % 100 == 0 {
                rng.normal_with(0.0, 0.08)
            } else {
                rng.normal_with(0.0, 0.008)
            }
        })
        .collect();
    assert!(
        parametric_var(&contaminated, 0.99) > historical_var(&contaminated, 0.99),
        "the normal fit is inflated by the rare extremes"
    );
    // The excess kurtosis still detects the non-normality, which is why the
    // tail assessment looks at both.
    assert!(TailRisk::compute(&contaminated).excess_kurtosis > 2.0);
}

#[test]
fn a_gaussian_series_is_not_flagged_as_fat_tailed() {
    let returns = normal_returns(21, 6000, 0.0, 0.01);
    let tail = TailRisk::compute(&returns);
    assert!(
        !tail.understated_by_normal_model(),
        "fatness {} kurtosis {}",
        tail.tail_fatness,
        tail.excess_kurtosis
    );
}

#[test]
fn degenerate_series_produce_zero_rather_than_nonsense() {
    assert_eq!(historical_var(&[], 0.95), 0.0);
    assert_eq!(parametric_var(&[0.01], 0.95), 0.0);
    assert_eq!(expected_shortfall(&[], 0.95), 0.0);
    assert_eq!(TailRisk::compute(&[]).var_99, 0.0);
}

// --- drawdown ---------------------------------------------------------------

#[test]
fn the_drawdown_profile_locates_the_peak_the_trough_and_the_recovery() {
    // Up to 120, down to 90, back to 130.
    let equity = vec![100.0, 110.0, 120.0, 105.0, 90.0, 100.0, 125.0, 130.0];
    let profile = drawdown_profile(&equity);

    assert!(
        approx_eq(profile.max_drawdown, 0.25, 1e-12),
        "dd {}",
        profile.max_drawdown
    );
    assert_eq!(profile.peak_index, 2);
    assert_eq!(profile.trough_index, 4);
    assert_eq!(profile.decline_periods, 2);
    assert_eq!(
        profile.recovery_periods,
        Some(2),
        "regained 120 two periods after the trough"
    );
    assert_eq!(
        profile.current_drawdown, 0.0,
        "the series ends at a new high"
    );
}

#[test]
fn an_unrecovered_drawdown_reports_no_recovery() {
    let equity = vec![100.0, 120.0, 80.0, 85.0, 90.0];
    let profile = drawdown_profile(&equity);
    assert!(approx_eq(profile.max_drawdown, 1.0 / 3.0, 1e-12));
    assert_eq!(profile.recovery_periods, None);
    assert!(approx_eq(profile.current_drawdown, 0.25, 1e-12));
}

#[test]
fn the_longest_underwater_period_is_tracked_separately_from_depth() {
    // A shallow but very long drawdown is harder to hold than a deep short one,
    // and depth alone cannot distinguish them.
    let mut shallow_long = vec![100.0];
    shallow_long.extend((0..50).map(|_| 95.0));
    shallow_long.push(105.0);
    let long = drawdown_profile(&shallow_long);

    let deep_short = vec![100.0, 60.0, 105.0];
    let short = drawdown_profile(&deep_short);

    assert!(
        short.max_drawdown > long.max_drawdown,
        "the deep one is deeper"
    );
    assert!(
        long.longest_underwater > short.longest_underwater,
        "the long one is longer: {} vs {}",
        long.longest_underwater,
        short.longest_underwater
    );
}

#[test]
fn a_monotonically_rising_curve_has_no_drawdown() {
    let equity: Vec<f64> = (0..50).map(|i| 100.0 + f64::from(i)).collect();
    let profile = drawdown_profile(&equity);
    assert_eq!(profile.max_drawdown, 0.0);
    assert_eq!(profile.longest_underwater, 0);
    assert_eq!(
        profile,
        DrawdownProfile {
            max_drawdown: 0.0,
            ..profile.clone()
        }
    );
}

// --- performance metrics ----------------------------------------------------

#[test]
fn sharpe_matches_a_hand_computed_value() {
    // Constant 0.1% daily return with no variation: the ratio is undefined,
    // and reporting zero is better than reporting infinity.
    let flat = vec![0.001; 100];
    let metrics = RiskMetrics::daily(&flat, 0.0);
    assert_eq!(metrics.sharpe_ratio, 0.0, "no variation means no ratio");
    assert!(
        metrics.annualised_return > 0.2,
        "0.1% daily compounds substantially"
    );

    // A series with known mean and standard deviation.
    let alternating: Vec<f64> = (0..252)
        .map(|i| if i % 2 == 0 { 0.02 } else { -0.01 })
        .collect();
    let metrics = RiskMetrics::daily(&alternating, 0.0);
    let mean = 0.005;
    let sd = 0.015;
    let expected = mean / sd * metrics::TRADING_DAYS.sqrt();
    assert!(
        approx_eq(metrics.sharpe_ratio, expected, 0.02),
        "sharpe {}",
        metrics.sharpe_ratio
    );
}

#[test]
fn sortino_exceeds_sharpe_when_the_downside_is_mild() {
    // Big upside moves and small downside ones: penalising only the downside
    // gives a higher ratio, which is the whole point of the measure.
    let returns: Vec<f64> = (0..252)
        .map(|i| if i % 3 == 0 { 0.03 } else { -0.005 })
        .collect();
    let metrics = RiskMetrics::daily(&returns, 0.0);
    assert!(
        metrics.sortino_ratio > metrics.sharpe_ratio,
        "sortino {} should exceed sharpe {}",
        metrics.sortino_ratio,
        metrics.sharpe_ratio
    );
}

#[test]
fn the_risk_free_rate_reduces_the_sharpe_ratio() {
    let returns = normal_returns(5, 1000, 0.0006, 0.01);
    let without = RiskMetrics::daily(&returns, 0.0);
    let with = RiskMetrics::daily(&returns, 0.05);
    assert!(with.sharpe_ratio < without.sharpe_ratio);
}

#[test]
fn a_short_record_is_flagged_as_statistically_meaningless() {
    // A Sharpe from thirty observations has a standard error near 0.2;
    // quoting it to two decimals is false precision.
    let short = RiskMetrics::daily(&normal_returns(7, 30, 0.001, 0.01), 0.0);
    assert!(!short.is_statistically_meaningful());
    assert!(
        short.sharpe_standard_error() > 2.0,
        "se {}",
        short.sharpe_standard_error()
    );

    let long = RiskMetrics::daily(&normal_returns(7, 2000, 0.001, 0.01), 0.0);
    assert!(long.is_statistically_meaningful());
    assert!(long.sharpe_standard_error() < short.sharpe_standard_error());
}

#[test]
fn a_noise_series_does_not_produce_a_significant_sharpe() {
    let noise = normal_returns(11, 1500, 0.0, 0.01);
    let metrics = RiskMetrics::daily(&noise, 0.0);
    assert!(
        !metrics.sharpe_is_significant(),
        "zero-mean noise gave a significant sharpe of {} with standard error {}",
        metrics.sharpe_ratio,
        metrics.sharpe_standard_error()
    );
    // Six years of daily data leaves roughly 0.4 of annualised Sharpe as noise.
    assert!(
        approx_eq(metrics.sharpe_standard_error(), 0.41, 0.05),
        "standard error {}",
        metrics.sharpe_standard_error()
    );

    let real = normal_returns(11, 1500, 0.0015, 0.01);
    assert!(RiskMetrics::daily(&real, 0.0).sharpe_is_significant());
}

#[test]
fn calmar_relates_return_to_the_worst_drawdown() {
    let returns: Vec<f64> = (0..504)
        .map(|i| if i == 100 { -0.15 } else { 0.0008 })
        .collect();
    let metrics = RiskMetrics::daily(&returns, 0.0);
    assert!(metrics.drawdown.max_drawdown > 0.10);
    assert!(metrics.calmar_ratio > 0.0);
    assert!(approx_eq(
        metrics.calmar_ratio,
        metrics.annualised_return / metrics.drawdown.max_drawdown,
        1e-9
    ));
}

#[test]
fn hit_rate_and_gain_to_pain_describe_the_shape_of_returns() {
    // Rarely right but large when it is: the classic trend-following shape.
    let returns: Vec<f64> = (0..100)
        .map(|i| if i % 5 == 0 { 0.06 } else { -0.01 })
        .collect();
    let metrics = RiskMetrics::daily(&returns, 0.0);
    assert!(approx_eq(metrics.hit_rate, 0.2, 1e-12));
    assert!(approx_eq(metrics.gain_to_pain, 6.0, 1e-9));
}

#[test]
fn benchmark_relative_measures_behave() {
    let benchmark = normal_returns(13, 800, 0.0004, 0.010);
    // A portfolio that is 1.5x the benchmark plus noise.
    let mut rng = Xoshiro256::seeded(17);
    let portfolio: Vec<f64> = benchmark
        .iter()
        .map(|b| 1.5 * b + rng.normal_with(0.0002, 0.003))
        .collect();

    let beta = metrics::beta(&portfolio, &benchmark);
    assert!(approx_eq(beta, 1.5, 0.1), "beta {beta}");

    let alpha = metrics::alpha(&portfolio, &benchmark, metrics::TRADING_DAYS);
    assert!(alpha > 0.0, "alpha {alpha}");

    let tracking = metrics::tracking_error(&portfolio, &benchmark, metrics::TRADING_DAYS);
    assert!(tracking > 0.0);
    assert!(metrics::information_ratio(&portfolio, &benchmark, metrics::TRADING_DAYS).is_finite());

    // A portfolio identical to the benchmark has zero tracking error and beta 1.
    assert!(approx_eq(metrics::beta(&benchmark, &benchmark), 1.0, 1e-9));
    assert!(approx_eq(
        metrics::tracking_error(&benchmark, &benchmark, metrics::TRADING_DAYS),
        0.0,
        1e-12
    ));
    assert_eq!(
        metrics::information_ratio(&benchmark, &benchmark, metrics::TRADING_DAYS),
        0.0
    );
}

// --- factor risk ------------------------------------------------------------

fn two_factor_model() -> FactorRisk {
    // Three assets on two factors. The first two load heavily on factor one.
    let exposures = Matrix::from_rows(&[vec![1.2, 0.1], vec![1.1, 0.0], vec![0.2, 0.9]]).unwrap();
    let factor_covariance =
        Matrix::from_rows(&[vec![0.0400, 0.0020], vec![0.0020, 0.0225]]).unwrap();
    FactorRisk::new(
        vec!["A".into(), "B".into(), "C".into()],
        vec!["market".into(), "rates".into()],
        exposures,
        factor_covariance,
        vec![0.0100, 0.0090, 0.0150],
    )
    .unwrap()
}

#[test]
fn factor_decomposition_splits_systematic_from_specific_risk() {
    let model = two_factor_model();
    let decomposition = model.decompose(&[0.4, 0.4, 0.2]).unwrap();

    assert!(decomposition.total_volatility > 0.0);
    // Variance adds, so the two components combine in quadrature.
    let recombined = (decomposition.factor_volatility.powi(2)
        + decomposition.specific_volatility.powi(2))
    .sqrt();
    assert!(approx_eq(recombined, decomposition.total_volatility, 1e-12));
    assert!((0.0..=1.0).contains(&decomposition.factor_share));
    assert!(decomposition.factor_share > 0.5, "a factor-driven book");
}

#[test]
fn asset_risk_contributions_sum_to_total_variance() {
    let model = two_factor_model();
    let weights = [0.5, 0.3, 0.2];
    let decomposition = model.decompose(&weights).unwrap();
    let sum: f64 = decomposition.asset_contributions.values().sum();
    assert!(
        approx_eq(sum, decomposition.total_volatility.powi(2), 1e-12),
        "contributions {sum} against variance {}",
        decomposition.total_volatility.powi(2)
    );
}

#[test]
fn factor_concentration_is_detected_despite_many_holdings() {
    // The failure this catches: a book of many names that is really one bet.
    let model = two_factor_model();
    let concentrated = model.decompose(&[0.5, 0.5, 0.0]).unwrap();
    assert!(
        concentrated.is_factor_concentrated(0.8),
        "{:?}",
        concentrated.factor_contributions
    );
    assert_eq!(concentrated.dominant_factor().unwrap().0, "market");

    // Effective bets is far below the position count when everything co-moves.
    assert!(
        concentrated.effective_bets() < 2.5,
        "effective bets {}",
        concentrated.effective_bets()
    );
}

#[test]
fn the_model_reconstructs_an_asset_covariance_matrix() {
    let model = two_factor_model();
    let covariance = model.asset_covariance().unwrap();
    assert!(covariance.is_symmetric(1e-12));
    assert!(covariance.is_positive_semidefinite(1e-9));
    // The diagonal is each asset's total variance and exceeds its specific part.
    for index in 0..3 {
        assert!(covariance.get(index, index) > model.specific_variance[index]);
    }
}

#[test]
fn the_model_is_estimated_from_return_histories() {
    let mut rng = Xoshiro256::seeded(23);
    let periods = 500;
    let mut factor_rows = Vec::with_capacity(periods);
    let mut asset_rows = Vec::with_capacity(periods);
    for _ in 0..periods {
        let market = rng.normal_with(0.0, 0.012);
        let rates = rng.normal_with(0.0, 0.006);
        factor_rows.push(vec![market, rates]);
        asset_rows.push(vec![
            1.3 * market + 0.1 * rates + rng.normal_with(0.0, 0.004),
            0.4 * market + 0.8 * rates + rng.normal_with(0.0, 0.003),
        ]);
    }

    let model = FactorRisk::estimate(
        vec!["A".into(), "B".into()],
        &Matrix::from_rows(&asset_rows).unwrap(),
        vec!["market".into(), "rates".into()],
        &Matrix::from_rows(&factor_rows).unwrap(),
    )
    .unwrap();

    assert!(
        approx_eq(model.exposures.get(0, 0), 1.3, 0.06),
        "beta {}",
        model.exposures.get(0, 0)
    );
    assert!(approx_eq(model.exposures.get(1, 1), 0.8, 0.06));
    assert!(model.specific_variance.iter().all(|v| *v > 0.0));
}

#[test]
fn a_mismatched_model_is_rejected_at_construction() {
    let exposures = Matrix::from_rows(&[vec![1.0, 0.0]]).unwrap();
    let covariance = Matrix::identity(2);
    assert!(
        FactorRisk::new(
            vec!["A".into(), "B".into()],
            vec!["f1".into(), "f2".into()],
            exposures.clone(),
            covariance.clone(),
            vec![0.01, 0.01],
        )
        .is_err(),
        "one exposure row for two assets"
    );
    assert!(
        FactorRisk::new(
            vec!["A".into()],
            vec!["f1".into(), "f2".into()],
            exposures,
            covariance,
            vec![-0.01],
        )
        .is_err(),
        "negative specific variance"
    );
}

// --- the limit engine -------------------------------------------------------

fn state() -> RiskState {
    RiskState {
        equity: Decimal::from_int(1_000_000),
        cash: Decimal::from_int(100_000),
        gross_exposure: Decimal::from_int(900_000),
        net_exposure: Decimal::from_int(700_000),
        position_notionals: BTreeMap::from([
            ("AAPL".to_string(), Decimal::from_int(80_000)),
            ("MSFT".to_string(), Decimal::from_int(60_000)),
        ]),
        axis_exposures: BTreeMap::from([(
            "sector".to_string(),
            BTreeMap::from([
                (
                    "information_technology".to_string(),
                    Decimal::from_int(300_000),
                ),
                ("financials".to_string(), Decimal::from_int(300_000)),
                ("energy".to_string(), Decimal::from_int(300_000)),
            ]),
        )]),
        volatility: 0.18,
        value_at_risk: BTreeMap::from([("0.99".to_string(), 0.03)]),
        expected_shortfall: BTreeMap::from([("0.97".to_string(), 0.05)]),
        drawdown: 0.05,
        daily_loss: 0.01,
        days_to_liquidate: BTreeMap::from([("AAPL".to_string(), 1.0)]),
        liquidatable_within: BTreeMap::from([("5".to_string(), 0.95)]),
        order_notional: None,
        order_subject: None,
        // Every figure above was computed, so nothing is filed as
        // unevaluated. A fixture with an entry here is a state no order may
        // pass, which is the property `pretrade.rs` tests separately.
        unevaluated: BTreeMap::new(),
    }
}

#[test]
fn a_compliant_state_passes_every_limit() {
    let limits = LimitSet::conservative_default();
    let check = limits.check(&state());
    assert!(!check.is_blocked(), "{}", check.reason());
    assert_eq!(check.evaluated, limits.len());
    assert_eq!(check.reason(), "within all limits");
}

#[test]
fn a_leverage_breach_blocks_and_forces_reduction() {
    let mut state = state();
    state.gross_exposure = Decimal::from_int(2_500_000);

    let check = LimitSet::conservative_default().check(&state);
    assert!(check.is_blocked());
    assert!(
        check.requires_reduction(),
        "excess leverage must force an unwind, not just a halt"
    );
    let blocking = check.blocking();
    assert!(blocking.iter().any(|b| b.limit_kind == "max_leverage"));
    assert!(check.reason().contains("leverage"), "{}", check.reason());
}

#[test]
fn an_axis_weight_breach_names_the_offending_bucket() {
    let mut state = state();
    state.axis_exposures.insert(
        "sector".to_string(),
        BTreeMap::from([
            (
                "information_technology".to_string(),
                Decimal::from_int(850_000),
            ),
            ("financials".to_string(), Decimal::from_int(50_000)),
        ]),
    );

    let check = LimitSet::conservative_default().check(&state);
    let breach = check
        .blocking()
        .into_iter()
        .find(|b| b.limit_kind == "max_axis_weight")
        .expect("a sector-weight breach");
    assert_eq!(breach.subject.as_deref(), Some("information_technology"));
    // 850k of a million in equity. The exact value, not `> 0.9`: the old
    // assertion held only because the denominator was the axis total (900k),
    // and a loose inequality would have survived the denominator changing
    // under it without anyone noticing which number was being read.
    assert!((breach.observed - 0.85).abs() < 1e-9, "{}", breach.observed);
    assert!(breach.detail.contains("sector"));
}

#[test]
fn no_limit_in_the_default_set_divides_by_a_number_the_order_itself_moves() {
    let limits = LimitSet::conservative_default();
    // Premise: the set is not empty, so an empty filter below would not be
    // mistaken for a set that satisfies the property.
    assert!(limits.len() >= 12);
    let offenders: Vec<&str> = limits
        .limits
        .iter()
        .filter(|limit| limit.kind.denominator_moves_with_the_order())
        .map(|limit| limit.name.as_str())
        .collect();
    // `sector-concentration` and `country-concentration` were share-of-gross
    // caps here until ADR 0027, and a share of gross is 100% for the first
    // position in an empty book — so the shipped set refused the first order
    // of every deployment that fed it a catalogue. This is the assertion that
    // would have caught it before a deployment did.
    assert!(
        offenders.is_empty(),
        "the shipped set holds a pre-trade veto whose answer does not depend on \
         the order's size: {offenders:?}"
    );
}

#[test]
fn a_severe_breach_is_escalated_to_critical() {
    let mut state = state();
    // Well beyond the 1.5x leverage limit.
    state.gross_exposure = Decimal::from_int(10_000_000);
    let check = LimitSet::conservative_default().check(&state);
    let worst = check.blocking()[0];
    assert_eq!(worst.severity, Severity::Critical);
    assert!(worst.utilisation > 4.0);
}

#[test]
fn approaching_a_limit_warns_without_blocking() {
    let mut state = state();
    // 1.4x against a 1.5x limit is inside but close.
    state.gross_exposure = Decimal::from_int(1_400_000);
    let check = LimitSet::conservative_default().check(&state);
    assert!(!check.is_blocked(), "a warning must not block");
    assert!(
        check
            .warnings()
            .iter()
            .any(|w| w.limit_kind == "max_leverage"),
        "expected a leverage warning"
    );
}

#[test]
fn a_minimum_limit_binds_from_below() {
    let mut state = state();
    state.cash = Decimal::from_int(5_000); // 0.5% against a 2% floor
    let check = LimitSet::conservative_default().check(&state);
    let breach = check
        .blocking()
        .into_iter()
        .find(|b| b.limit_kind == "min_cash_buffer")
        .expect("the cash floor should bind");
    assert!(breach.observed < breach.bound);

    let mut illiquid = state.clone();
    illiquid.cash = Decimal::from_int(100_000);
    illiquid.liquidatable_within.insert("5".to_string(), 0.30);
    let check = LimitSet::conservative_default().check(&illiquid);
    assert!(
        check
            .blocking()
            .iter()
            .any(|b| b.limit_kind == "min_liquidity"),
        "a book that cannot be exited must be blocked"
    );
}

#[test]
fn an_order_limit_is_only_evaluated_when_an_order_is_present() {
    let limits = LimitSet::new("orders").with(Limit::new(
        "order-size",
        LimitKind::MaxOrderNotional {
            limit: Decimal::from_int(100_000),
        },
    ));

    assert!(
        !limits.check(&state()).is_blocked(),
        "no order, nothing to check"
    );

    let mut with_order = state();
    with_order.order_notional = Some(Decimal::from_int(500_000));
    with_order.order_subject = Some("AAPL".to_string());
    let check = limits.check(&with_order);
    assert!(check.is_blocked());
    assert_eq!(check.blocking()[0].subject.as_deref(), Some("AAPL"));
}

#[test]
fn limit_checks_are_deterministic() {
    let limits = LimitSet::conservative_default();
    let state = state();
    let first = limits.check(&state);
    let second = limits.check(&state);
    assert_eq!(
        first, second,
        "the same inputs must give the same answer, always"
    );
}

#[test]
fn a_zero_equity_book_is_blocked_rather_than_dividing_by_zero() {
    let mut state = state();
    state.equity = Decimal::ZERO;
    let check = LimitSet::conservative_default().check(&state);
    assert!(
        check.is_blocked(),
        "an insolvent book cannot pass a ratio limit"
    );
    assert!(
        check
            .blocking()
            .iter()
            .all(|b| b.observed.is_infinite() || b.observed >= 0.0)
    );
}

#[test]
fn an_empty_limit_set_blocks_nothing_but_says_so() {
    let check = LimitSet::new("empty").check(&state());
    assert!(!check.is_blocked());
    assert_eq!(
        check.evaluated, 0,
        "an unconfigured limit set must be visible as such"
    );
}

#[test]
fn every_shipped_limit_explains_why_it_exists() {
    for limit in &LimitSet::conservative_default().limits {
        assert!(
            !limit.rationale.is_empty(),
            "{} must state why it exists so a breach explains itself",
            limit.name
        );
    }
}

// --- the tail limits --------------------------------------------------------
//
// `.claude/rules/domains/risk-and-execution.md` named `MaxExpectedShortfall`
// as the template of a control that cannot fire: `RiskState::expected_shortfall`
// was always empty, so the limit took its `None` arm on every book. These
// fixtures pin the lib-side derivation that fills the maps from a return
// series, keyed the way the limit reads them.

/// Mostly small gains, then a run of losses far outside them: the shape
/// expected shortfall exists to price and a volatility figure reports as
/// merely elevated.
fn returns_with_a_tail() -> Vec<f64> {
    vec![
        0.004, 0.005, 0.003, 0.003, -0.004, 0.005, 0.004, -0.002, 0.005, -0.09, -0.11, -0.13,
    ]
}

fn quiet_returns() -> Vec<f64> {
    (0..12)
        .map(|step| 0.001 + f64::from(step) * 0.0001)
        .collect()
}

/// The shared fixture with its tail maps emptied, so that a figure found
/// under a key can only have been put there by the derivation under test.
/// The first draft of these fixtures inherited the seeded `0.05` and its
/// "computed, not skipped" premise held with the derivation deleted.
fn state_with_no_tail_figures() -> RiskState {
    let mut state = state();
    state.expected_shortfall.clear();
    state.value_at_risk.clear();
    state
}

/// The shared fixture with `volatility` reset to zero, so a nonzero figure
/// found after `with_tail_risk` can only have come from the derivation under
/// test rather than the `0.18` the fixture otherwise carries.
fn state_with_no_volatility_figure() -> RiskState {
    let mut state = state();
    state.volatility = 0.0;
    state
}

#[test]
fn a_book_whose_expected_shortfall_breaches_the_limit_is_refused() {
    let limits = LimitSet::conservative_default();
    let state = state_with_no_tail_figures().with_tail_risk(&limits, &returns_with_a_tail());

    // Premise: the figure exists under the key the default limit reads. If
    // the map is empty the breach assertion measures nothing — which is the
    // state this fixture exists to end.
    let shortfall = state
        .expected_shortfall
        .get("0.97")
        .copied()
        .unwrap_or_else(|| {
            panic!(
                "no expected shortfall under the default limit's key; the map holds {:?}",
                state.expected_shortfall.keys().collect::<Vec<_>>()
            )
        });
    assert!(
        shortfall > 0.0,
        "a book with a tail has a shortfall of {shortfall}"
    );
    // And it is expected shortfall, not value at risk under another name: the
    // mean beyond the threshold is strictly worse than the threshold on this
    // series. A mutation swapping the two metrics passed until this held.
    let var = metrics::historical_var(&returns_with_a_tail(), 0.975);
    assert!(
        shortfall > var,
        "shortfall {shortfall} is not beyond value at risk {var}"
    );

    let check = limits.check(&state);
    let breach = check
        .blocking()
        .into_iter()
        .find(|b| b.limit_kind == "max_expected_shortfall")
        .unwrap_or_else(|| panic!("expected shortfall did not bind: {}", check.reason()));
    assert!(breach.observed > breach.bound);
    assert!(check.is_blocked());
}

#[test]
fn a_book_whose_expected_shortfall_sits_below_the_limit_passes() {
    let limits = LimitSet::conservative_default();
    let state = state_with_no_tail_figures().with_tail_risk(&limits, &quiet_returns());

    // Premise: computed, not skipped. A quiet book has a shortfall of zero
    // under a key the limit finds — the limit evaluated and found nothing.
    assert!(
        state.expected_shortfall.contains_key("0.97"),
        "nothing was computed, so nothing can be said about passing"
    );
    let check = limits.check(&state);
    assert!(
        !check
            .breaches
            .iter()
            .any(|b| b.limit_kind == "max_expected_shortfall"),
        "a book that only gained breached the tail limit: {}",
        check.reason()
    );
}

#[test]
fn value_at_risk_is_keyed_and_bound_the_same_way() {
    // The same defect, the other limit. VaR shared the empty map.
    let limits = LimitSet::conservative_default();
    let state = state_with_no_tail_figures().with_tail_risk(&limits, &returns_with_a_tail());
    let var = state
        .value_at_risk
        .get("0.99")
        .copied()
        .unwrap_or_else(|| panic!("no value at risk under the default limit's key"));
    assert!(
        var > 0.05,
        "premise: the tail exceeds the 5% bound, got {var}"
    );
    let check = limits.check(&state);
    assert!(
        check
            .blocking()
            .iter()
            .any(|b| b.limit_kind == "max_value_at_risk"),
        "{}",
        check.reason()
    );
}

#[test]
fn a_series_too_short_to_measure_leaves_the_maps_empty_rather_than_recording_zero() {
    // A zero nobody computed would pass the limit and read as evidence the
    // book has no tail.
    let limits = LimitSet::conservative_default();
    // Premise: the derivation does fill these maps on a series it can
    // measure, so an empty map below is a decision and not a no-op.
    let measurable = state_with_no_tail_figures().with_tail_risk(&limits, &[0.01, -0.02]);
    assert!(!measurable.expected_shortfall.is_empty());
    let bare = state_with_no_tail_figures().with_tail_risk(&limits, &[0.01]);
    assert!(bare.expected_shortfall.is_empty());
    assert!(bare.value_at_risk.is_empty());
}

// --- liquidity horizons: this crate does not derive them ---------------------
//
// `LimitKind::MinLiquidity` reads `liquidatable_within`, keyed by horizon,
// exactly the way `MaxExpectedShortfall` read `expected_shortfall` before
// `RiskState::with_tail_risk` existed. `with_tail_risk` closed that gap here
// because the tail *is* derivable here: a return series in, a quantile out.
//
// The liquidity figure is not. It needs average daily volume and market
// depth, which this crate has none of, so `RiskState::with_liquidity_horizons`
// derived it by refiltering day counts a caller had supplied — and handed a
// book with holdings and no counts it returned the state untouched. An
// untouched state is the same state a passing floor produces: empty map, empty
// `unevaluated`, `reason()` of "within all limits". It was the same fail-open
// that shipped once already, in the derivation `qip-kernel` uses, and closing
// it there cost sixty-one mutations.
//
// It is not repaired here, it is gone, and the reason it is gone rather than
// repaired matters: filing `RiskState::unevaluated` from inside that method
// would have meant inventing the reason. "Nobody ran a liquidity model" and
// "the liquidity model ran and refused" are facts about the caller, and
// `with_unevaluated`'s whole contract is that the producer states its own.
// The producer is `qip-kernel`'s `Platform::liquidatable_within`, over a
// ladder whose construction proved its own monotonicity, and it does file the
// refusal.
//
// The test below is the guard against the second derivation coming back.

#[test]
fn nothing_in_this_crate_fills_the_figure_the_liquidity_floor_reads() {
    // Every producer `qip-risk` offers, composed the way `qip-kernel`'s
    // `Platform::risk_state_from` composes them, over a book that holds
    // positions and has been marked with exit times inside the floor's own
    // horizon. If a liquidity derivation is ever re-added here, this is the
    // fixture that would fill the map, and this test fires.
    let limits = LimitSet::conservative_default();
    let mut state = state();
    state.value_at_risk.clear();
    state.expected_shortfall.clear();
    state.liquidatable_within.clear();
    state.days_to_liquidate =
        BTreeMap::from([("AAPL".to_string(), 1.0), ("MSFT".to_string(), 2.0)]);

    // Premise, in three parts, because an absence proves nothing on its own.
    // The book holds something to be illiquid; the floor is in the set and
    // would read the key `5`; and the composition below does fill the other
    // keyed figures, so an empty liquidity map is a decision and not a
    // no-op that emptied everything.
    assert!(
        !state.position_notionals.is_empty(),
        "premise: the book holds positions"
    );
    assert!(
        limits.limits.iter().any(|limit| matches!(
            limit.kind,
            LimitKind::MinLiquidity { days, .. } if (days - 5.0).abs() < 1e-12
        )),
        "premise: the shipped set carries a five-day liquidity floor"
    );
    let derived = state.with_tail_risk(&limits, &[0.01, -0.02, 0.015, -0.03]);
    assert!(
        !derived.value_at_risk.is_empty() && !derived.expected_shortfall.is_empty(),
        "premise: the crate's own producers did run and did fill what they can derive"
    );

    assert!(
        derived.liquidatable_within.is_empty(),
        "something in qip-risk derived the liquidity figure: {:?}. There is one producer of \
         it — qip-kernel's ladder — and it is the one that can file a refusal when it cannot \
         compute. A second writer here abstains silently, and MinLiquidity reads an abstention \
         and a pass as the same event.",
        derived.liquidatable_within
    );
    // And the floor therefore records nothing, which is only safe because the
    // real producer files `unevaluated` and `PreTradeChecker::check` refuses
    // on it. Asserted so that a future reader meets the whole bargain here
    // rather than half of it.
    assert!(
        !limits
            .check(&derived)
            .breaches
            .iter()
            .any(|b| b.limit_kind == "min_liquidity"),
        "the floor bound on a figure nobody in this crate computed"
    );
    assert!(
        derived.unevaluated.is_empty(),
        "this crate filed a refusal on someone else's behalf: {:?}",
        derived.unevaluated
    );
}

// --- volatility --------------------------------------------------------
//
// `RiskState::volatility` had exactly the defect
// `RiskState::expected_shortfall` once had, just without a map to make the
// absence visible: `RiskState::from_figures` never touches it and
// `PreTradeChecker::project` says outright that it leaves volatility as it
// stands, so `MaxVolatility` shipped in `LimitSet::conservative_default` and
// took the seeded default on every book that never happened to set the
// field by hand. `RiskState::with_tail_risk` now derives it from the same
// return series it already uses for value at risk and expected shortfall.

#[test]
fn a_book_whose_volatility_breaches_the_limit_is_refused() {
    let limits = LimitSet::conservative_default();
    let state = state_with_no_volatility_figure().with_tail_risk(&limits, &returns_with_a_tail());

    // Premise: the derivation actually computed something, and it is well
    // past the default 25% annualised bound — not a value the fixture's
    // zeroed starting point could have produced by accident.
    assert!(
        state.volatility > 0.25,
        "a tail this sharp should annualise well past the default bound, got {}",
        state.volatility
    );

    let check = limits.check(&state);
    let breach = check
        .blocking()
        .into_iter()
        .find(|b| b.limit_kind == "max_volatility")
        .unwrap_or_else(|| panic!("volatility did not bind: {}", check.reason()));
    assert!(breach.observed > breach.bound);
    assert!(check.is_blocked());
}

#[test]
fn a_book_whose_volatility_sits_below_the_limit_passes() {
    let limits = LimitSet::conservative_default();
    let state = state_with_no_volatility_figure().with_tail_risk(&limits, &quiet_returns());

    // Premise: computed, not left at the zero the fixture was reset to, and
    // still comfortably under the bound.
    assert!(
        state.volatility > 0.0,
        "nothing was computed, so nothing can be said about passing"
    );
    assert!(state.volatility < 0.25);

    let check = limits.check(&state);
    assert!(
        !check
            .breaches
            .iter()
            .any(|b| b.limit_kind == "max_volatility"),
        "a quiet book breached the volatility limit: {}",
        check.reason()
    );
}

#[test]
fn a_volatility_series_too_short_to_measure_leaves_the_field_untouched() {
    // A zero nobody computed would pass the limit and read as evidence the
    // book is calm, the same failure a recorded-zero tail figure would be.
    let limits = LimitSet::conservative_default();
    let seeded = state_with_no_volatility_figure();
    assert_eq!(
        seeded.volatility, 0.0,
        "premise: the fixture starts at zero"
    );

    let measurable = seeded
        .clone()
        .with_tail_risk(&limits, &returns_with_a_tail());
    assert!(measurable.volatility > 0.0);

    let bare = seeded.with_tail_risk(&limits, &[0.01]);
    assert_eq!(
        bare.volatility, 0.0,
        "a series too short to measure must leave the field alone, not record zero"
    );
}

// --- a figure that is not a number -------------------------------------------
//
// `Limit::assess` decided every breach with bare `<` and `>` on `f64`, and the
// file held no finiteness check at all. IEEE-754 makes both comparisons false
// against a `NaN`, so a poisoned figure came back as "no breach" while the
// limit went on counting in `LimitCheck::evaluated` — an evaluated, passing
// control that could not fire. This crate has shipped that shape twice
// already: `MaxExpectedShortfall` over an always-empty map, and the
// `daily-loss` cap over a field no producer wrote. A `NaN` is worse than
// either, because it arrives from arithmetic rather than from a missing
// writer, so it disarms whichever limit reads it on a book that is otherwise
// fully populated and looks healthy.

/// One poisoning of the shared fixture: the `limit_kind` label of the rule
/// that reads the figure, and the mutation that makes the figure unreadable.
type StatePoisoning = (&'static str, fn(&mut RiskState));

/// One poisoning of a limit's own configuration: the field name, for the
/// failure message, and the mutation.
type LimitPoisoning = (&'static str, fn(&mut Limit));

#[test]
fn a_shipped_limit_refuses_a_figure_that_is_not_a_number_instead_of_passing_it() {
    let limits = LimitSet::conservative_default();

    // Premise: the fixture passes the whole shipped set, so anything that
    // blocks below is the poisoned figure and not the fixture.
    let clean = limits.check(&state());
    assert!(
        !clean.is_blocked(),
        "premise: the clean fixture must pass, got {}",
        clean.reason()
    );

    let poisonings: [StatePoisoning; 6] = [
        ("max_volatility", |s| s.volatility = f64::NAN),
        ("max_drawdown", |s| s.drawdown = f64::NAN),
        ("max_daily_loss", |s| s.daily_loss = f64::NAN),
        ("max_value_at_risk", |s| {
            s.value_at_risk.insert("0.99".to_string(), f64::NAN);
        }),
        ("max_expected_shortfall", |s| {
            s.expected_shortfall.insert("0.97".to_string(), f64::NAN);
        }),
        ("min_liquidity", |s| {
            s.liquidatable_within.insert("5".to_string(), f64::NAN);
        }),
    ];

    for (limit_kind, poison) in poisonings {
        let mut poisoned = state();
        poison(&mut poisoned);
        let check = limits.check(&poisoned);
        assert!(
            check.is_blocked(),
            "a {limit_kind} figure of NaN read as a passing check: {}",
            check.reason()
        );
        let breach = check
            .blocking()
            .into_iter()
            .find(|b| b.limit_kind == limit_kind)
            .unwrap_or_else(|| {
                panic!("{limit_kind} did not refuse its own poisoned figure: {check:?}")
            });
        assert_eq!(
            breach.severity,
            Severity::Critical,
            "{limit_kind}: a figure nobody could compute is not fixed by sending less, which \
             is what Breach means and Critical does not"
        );
        assert!(
            breach.detail.contains("is not a comparison"),
            "{limit_kind} reported an ordinary breach rather than a refusal: {}",
            breach.detail
        );
        assert!(
            breach.observed.is_nan(),
            "{limit_kind} substituted {} for the figure it could not read; a substituted \
             number reads downstream as a measurement",
            breach.observed
        );
    }
}

#[test]
fn a_limit_whose_own_threshold_is_not_a_number_refuses_rather_than_going_quiet() {
    // The two thresholds are read by the same comparisons and fail the same
    // way: `bound * NaN` is `NaN` and `observed > NaN` is false, so a warning
    // threshold nobody validated silences the warning arm, and `ratio >= NaN`
    // is false, so a critical multiple of `NaN` downgrades every critical
    // breach to an ordinary one. Neither reads as wrong anywhere.
    let poisonings: [LimitPoisoning; 2] = [
        ("warning_threshold", |l| l.warning_threshold = f64::NAN),
        ("critical_multiple", |l| l.critical_multiple = f64::NAN),
    ];

    for (field, poison) in poisonings {
        let mut limit =
            Limit::new("leverage", LimitKind::MaxLeverage { limit: 1.5 }).with_rationale("fixture");

        // Premise: with both thresholds finite the fixture book (900k gross
        // on 1m of equity) is inside this limit, so the block below is the
        // threshold and not the book.
        let premise = LimitSet::new("fixture").with(limit.clone()).check(&state());
        assert_eq!(premise.evaluated, 1);
        assert!(
            !premise.is_blocked(),
            "premise for {field}: {}",
            premise.reason()
        );

        poison(&mut limit);
        let check = LimitSet::new("fixture").with(limit).check(&state());
        assert!(
            check.is_blocked(),
            "a limit whose {field} is NaN evaluated anyway and passed"
        );
    }
}

#[test]
fn an_insolvent_book_no_longer_passes_the_cash_buffer_floor() {
    // `RiskState::ratio` answers infinity when there is no equity to divide
    // by. On a ceiling that breached by ordinary arithmetic, which is what
    // the sentinel was for; on a floor it passed in silence, because `inf` is
    // not less than a cash bound of 0.02. So the one limit that exists to
    // notice a book has run out of money reported it inside its buffer.
    // `a_zero_equity_book_is_blocked_rather_than_dividing_by_zero` above could
    // not see this: the ceilings blocked, and the floor's abstention was
    // invisible behind them.
    let limits = LimitSet::conservative_default();

    // Premise: a solvent book passes the floor, so the breach below is the
    // insolvency and not a floor that refuses every book.
    let solvent = limits.check(&state());
    assert!(
        !solvent
            .breaches
            .iter()
            .any(|b| b.limit_kind == "min_cash_buffer"),
        "premise: {}",
        solvent.reason()
    );

    let mut insolvent = state();
    insolvent.equity = Decimal::ZERO;
    let check = limits.check(&insolvent);
    assert!(
        check
            .blocking()
            .iter()
            .any(|b| b.limit_kind == "min_cash_buffer"),
        "the cash floor abstained on a book with no equity: {}",
        check.reason()
    );
}

#[test]
fn a_return_series_carrying_a_value_that_is_not_a_number_refuses_the_tail_figures() {
    let limits = LimitSet::conservative_default();
    let bare = || {
        let mut state = state();
        state.value_at_risk.clear();
        state.expected_shortfall.clear();
        state.volatility = 0.0;
        state
    };
    let clean = [0.004, -0.09, 0.005, -0.11, 0.003, -0.13];

    // Premise: on a series it can measure the derivation fills all three
    // figures and files nothing, so an empty map below is a decision.
    let measured = bare().with_tail_risk(&limits, &clean);
    assert!(!measured.value_at_risk.is_empty());
    assert!(!measured.expected_shortfall.is_empty());
    assert!(measured.volatility > 0.0);
    assert!(measured.unevaluated.is_empty());

    let mut poisoned = clean.to_vec();
    poisoned[2] = f64::NAN;
    let refused = bare().with_tail_risk(&limits, &poisoned);

    // The hazard this catches and `Limit::assess` cannot see:
    // `qip_numerics::stats::quantile` *filters* non-finite values before it
    // sorts, so a value at risk derived from this series comes back finite and
    // plausible — a measurement of a book that does not exist — while
    // `stats::stddev` propagates, so volatility comes back NaN. One poisoned
    // series, two figures disagreeing about whether it could be measured at
    // all, and only the second visible to any comparison downstream.
    assert!(
        refused.value_at_risk.is_empty() && refused.expected_shortfall.is_empty(),
        "a tail figure was derived from a series that cannot be measured: {:?} / {:?}",
        refused.value_at_risk,
        refused.expected_shortfall
    );
    assert_eq!(
        refused.volatility, 0.0,
        "volatility was written from an unmeasurable series"
    );

    let named: Vec<&str> = refused.unevaluated.keys().map(String::as_str).collect();
    assert_eq!(
        named,
        vec![
            EXPECTED_SHORTFALL_FIGURE,
            VALUE_AT_RISK_FIGURE,
            VOLATILITY_FIGURE
        ],
        "the refusal must name every figure the limit set asked this producer for; an \
         unnamed one is a control whose silence nothing explains"
    );
    for (figure, refusal) in &refused.unevaluated {
        assert!(
            refusal.contains("return 2 of 6"),
            "{figure} does not say which return it could not use: {refusal}"
        );
    }
}
