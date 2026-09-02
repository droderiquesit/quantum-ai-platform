//! A passing and a vetoing fixture for every limit rule.
//!
//! The blueprint's delivery gate requires that every financial gate rule has
//! both halves: a fixture the rule admits and one it refuses. One half alone
//! proves nothing — a rule that refuses everything passes every veto test,
//! and a rule that was never wired passes every pass test. Each pass fixture
//! therefore first proves the rule *read* the state, by tightening the same
//! rule until it binds on the same state; each veto fixture first proves the
//! rule *can* pass, by loosening it. The premise is asserted before the
//! conclusion in every test.
//!
//! | Rule (`LimitKind`) | Pass fixture | Veto fixture |
//! |---|---|---|
//! | `MaxOrderNotional` | `an_order_inside_the_notional_limit_passes` | `an_order_beyond_the_notional_limit_is_vetoed` |
//! | `MaxPositionNotional` | `a_position_inside_the_notional_limit_passes` | `a_position_beyond_the_notional_limit_is_vetoed` |
//! | `MaxPositionWeight` | `a_position_inside_the_weight_limit_passes` | `a_position_beyond_the_weight_limit_is_vetoed` |
//! | `MaxLeverage` | `a_book_inside_the_leverage_limit_passes` | `a_book_beyond_the_leverage_limit_is_vetoed` |
//! | `MaxNetExposure` | `a_book_inside_the_net_exposure_limit_passes` | `a_book_beyond_the_net_exposure_limit_is_vetoed` |
//! | `MaxConcentration` | `a_spread_book_inside_the_concentration_limit_passes` | `a_bucket_beyond_the_concentration_limit_is_vetoed` |
//! | `MaxBucketExposure` | `a_bucket_inside_its_exposure_limit_passes` | `a_bucket_beyond_its_exposure_limit_is_vetoed` |
//! | `MaxVolatility` | `a_book_inside_the_volatility_limit_passes` | `a_book_beyond_the_volatility_limit_is_vetoed` |
//! | `MaxValueAtRisk` | `a_book_inside_the_value_at_risk_limit_passes` | `a_book_beyond_the_value_at_risk_limit_is_vetoed` |
//! | `MaxExpectedShortfall` | `a_book_inside_the_expected_shortfall_limit_passes` | `a_book_beyond_the_expected_shortfall_limit_is_vetoed` |
//! | `MaxDrawdown` | `a_book_inside_the_drawdown_limit_passes` | `a_book_beyond_the_drawdown_limit_is_vetoed` |
//! | `MaxDailyLoss` | `a_book_inside_the_daily_loss_limit_passes` | `a_book_beyond_the_daily_loss_limit_is_vetoed` |
//! | `MinLiquidity` | `a_book_above_the_liquidity_floor_passes` | `a_book_below_the_liquidity_floor_is_vetoed` |
//! | `MaxDaysToLiquidate` | `a_position_inside_the_days_to_liquidate_limit_passes` | `a_position_beyond_the_days_to_liquidate_limit_is_vetoed` |
//! | `MaxCounterpartyExposure` | `a_counterparty_inside_its_exposure_limit_passes` | `a_counterparty_beyond_its_exposure_limit_is_vetoed` |
//! | `MinCashBuffer` | `a_book_above_the_cash_floor_passes` | `a_book_below_the_cash_floor_is_vetoed` |
//!
//! `every_limit_kind_has_both_fixtures` closes the table: it matches on every
//! arm of `LimitKind`, so adding an arm without a row here fails to compile.

use qip_core::{Decimal, dec};
use qip_risk::limits::{Limit, LimitBreach, LimitCheck, LimitKind, LimitSet, RiskState};
use std::collections::BTreeMap;

/// One million of equity, 900k gross, 700k net, two names, three equal
/// sectors, a modest tail, and every map the limits read populated.
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
                ("energy".to_string(), Decimal::from_int(300_000)),
                ("financials".to_string(), Decimal::from_int(300_000)),
                (
                    "information_technology".to_string(),
                    Decimal::from_int(300_000),
                ),
            ]),
        )]),
        volatility: 0.18,
        value_at_risk: BTreeMap::from([("0.99".to_string(), 0.03)]),
        expected_shortfall: BTreeMap::from([("0.97".to_string(), 0.05)]),
        drawdown: 0.05,
        daily_loss: 0.01,
        days_to_liquidate: BTreeMap::from([("AAPL".to_string(), 1.0)]),
        liquidatable_within: BTreeMap::from([("5".to_string(), 0.95)]),
        counterparty_exposures: BTreeMap::from([("prime".to_string(), Decimal::from_int(200_000))]),
        order_notional: Some(Decimal::from_int(50_000)),
        order_subject: Some("AAPL".to_string()),
    }
}

fn check(kind: LimitKind, state: &RiskState) -> LimitCheck {
    LimitSet::new("fixture")
        .with(Limit::new("fixture", kind))
        .check(state)
}

/// The pass half. `tightened` is the same rule with a bound the state does
/// not satisfy; that it binds is the proof the rule read the state at all,
/// and without it a rule whose lookup silently missed would pass here.
fn passes(kind: LimitKind, tightened: LimitKind, state: &RiskState) {
    assert_eq!(
        kind.label(),
        tightened.label(),
        "the premise must be tested with the same rule"
    );
    let premise = check(tightened, state);
    assert_eq!(premise.evaluated, 1);
    assert!(
        premise.is_blocked(),
        "premise failed: the rule did not read the state, so its passing proves nothing"
    );

    let verdict = check(kind, state);
    assert_eq!(verdict.evaluated, 1);
    assert!(!verdict.is_blocked(), "{}", verdict.reason());
}

/// The veto half. `loosened` is the same rule with a bound the state
/// satisfies; that it passes is the proof the rule is not refusing
/// everything. Returns the breach so a caller can check what it named.
fn vetoes(kind: LimitKind, loosened: LimitKind, state: &RiskState) -> LimitBreach {
    assert_eq!(kind.label(), loosened.label());
    let premise = check(loosened, state);
    assert_eq!(premise.evaluated, 1);
    assert!(
        !premise.is_blocked(),
        "premise failed: the rule refuses even a compliant state: {}",
        premise.reason()
    );

    let label = kind.label();
    let verdict = check(kind, state);
    assert!(verdict.is_blocked(), "the rule admitted a breaching state");
    let breach = verdict
        .blocking()
        .into_iter()
        .find(|b| b.limit_kind == label)
        .unwrap_or_else(|| panic!("no breach carries the label {label}"))
        .clone();
    assert!(breach.blocks());
    breach
}

// --- MaxOrderNotional -------------------------------------------------------

#[test]
fn an_order_inside_the_notional_limit_passes() {
    let at = |limit: i64| LimitKind::MaxOrderNotional {
        limit: Decimal::from_int(limit),
    };
    passes(at(100_000), at(10_000), &state());
}

#[test]
fn an_order_beyond_the_notional_limit_is_vetoed() {
    let at = |limit: i64| LimitKind::MaxOrderNotional {
        limit: Decimal::from_int(limit),
    };
    let breach = vetoes(at(10_000), at(100_000), &state());
    assert_eq!(breach.subject.as_deref(), Some("AAPL"));
}

// --- MaxPositionNotional ----------------------------------------------------

#[test]
fn a_position_inside_the_notional_limit_passes() {
    let at = |limit: i64| LimitKind::MaxPositionNotional {
        limit: Decimal::from_int(limit),
    };
    passes(at(100_000), at(70_000), &state());
}

#[test]
fn a_position_beyond_the_notional_limit_is_vetoed() {
    let at = |limit: i64| LimitKind::MaxPositionNotional {
        limit: Decimal::from_int(limit),
    };
    let breach = vetoes(at(70_000), at(100_000), &state());
    assert_eq!(
        breach.subject.as_deref(),
        Some("AAPL"),
        "the 80k name is the one over 70k"
    );
}

// --- MaxPositionWeight ------------------------------------------------------

#[test]
fn a_position_inside_the_weight_limit_passes() {
    let at = |limit: f64| LimitKind::MaxPositionWeight { limit };
    passes(at(0.10), at(0.05), &state());
}

#[test]
fn a_position_beyond_the_weight_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxPositionWeight { limit };
    let breach = vetoes(at(0.05), at(0.10), &state());
    assert_eq!(breach.subject.as_deref(), Some("AAPL"));
    assert!((breach.observed - 0.08).abs() < 1e-9);
}

// --- MaxLeverage ------------------------------------------------------------

#[test]
fn a_book_inside_the_leverage_limit_passes() {
    let at = |limit: f64| LimitKind::MaxLeverage { limit };
    passes(at(1.5), at(0.5), &state());
}

#[test]
fn a_book_beyond_the_leverage_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxLeverage { limit };
    let breach = vetoes(at(0.5), at(1.5), &state());
    assert!((breach.observed - 0.9).abs() < 1e-9);
}

// --- MaxNetExposure ---------------------------------------------------------

#[test]
fn a_book_inside_the_net_exposure_limit_passes() {
    let at = |limit: f64| LimitKind::MaxNetExposure { limit };
    passes(at(1.0), at(0.5), &state());
}

#[test]
fn a_book_beyond_the_net_exposure_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxNetExposure { limit };
    let breach = vetoes(at(0.5), at(1.0), &state());
    assert!((breach.observed - 0.7).abs() < 1e-9);

    // Net is read unsigned: a book that is short 70% of equity is as far
    // from flat as one that is long it.
    let mut short = state();
    short.net_exposure = -short.net_exposure;
    let breach = vetoes(at(0.5), at(1.0), &short);
    assert!((breach.observed - 0.7).abs() < 1e-9);
}

// --- MaxConcentration -------------------------------------------------------

#[test]
fn a_spread_book_inside_the_concentration_limit_passes() {
    let at = |limit: f64| LimitKind::MaxConcentration {
        axis: "sector".into(),
        limit,
    };
    passes(at(0.35), at(0.30), &state());
}

#[test]
fn a_bucket_beyond_the_concentration_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxConcentration {
        axis: "sector".into(),
        limit,
    };
    let mut state = state();
    state.axis_exposures.insert(
        "sector".into(),
        BTreeMap::from([
            ("energy".to_string(), Decimal::from_int(600_000)),
            ("financials".to_string(), Decimal::from_int(300_000)),
        ]),
    );
    let breach = vetoes(at(0.35), at(0.70), &state);
    assert_eq!(breach.subject.as_deref(), Some("energy"));
    assert!(breach.detail.contains("sector"));
}

// --- MaxBucketExposure ------------------------------------------------------

#[test]
fn a_bucket_inside_its_exposure_limit_passes() {
    let at = |limit: f64| LimitKind::MaxBucketExposure {
        axis: "sector".into(),
        bucket: "energy".into(),
        limit,
    };
    passes(at(0.50), at(0.20), &state());
}

#[test]
fn a_bucket_beyond_its_exposure_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxBucketExposure {
        axis: "sector".into(),
        bucket: "energy".into(),
        limit,
    };
    let breach = vetoes(at(0.20), at(0.50), &state());
    assert_eq!(breach.subject.as_deref(), Some("energy"));
    assert!((breach.observed - 0.3).abs() < 1e-9);
}

// --- MaxVolatility ----------------------------------------------------------

#[test]
fn a_book_inside_the_volatility_limit_passes() {
    let at = |limit: f64| LimitKind::MaxVolatility { limit };
    passes(at(0.25), at(0.10), &state());
}

#[test]
fn a_book_beyond_the_volatility_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxVolatility { limit };
    let breach = vetoes(at(0.10), at(0.25), &state());
    assert!((breach.observed - 0.18).abs() < 1e-9);
}

// --- MaxValueAtRisk ---------------------------------------------------------

#[test]
fn a_book_inside_the_value_at_risk_limit_passes() {
    let at = |limit: f64| LimitKind::MaxValueAtRisk {
        confidence: 0.99,
        limit,
    };
    passes(at(0.05), at(0.02), &state());
}

#[test]
fn a_book_beyond_the_value_at_risk_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxValueAtRisk {
        confidence: 0.99,
        limit,
    };
    let breach = vetoes(at(0.02), at(0.05), &state());
    assert!((breach.observed - 0.03).abs() < 1e-9);
}

// --- MaxExpectedShortfall ---------------------------------------------------

#[test]
fn a_book_inside_the_expected_shortfall_limit_passes() {
    // 0.975 is the confidence the shipped limit uses, and `{:.2}` of it is
    // `0.97` — the key the fixture carries. A fixture keyed `0.98` would pass
    // this test forever with the rule never evaluating.
    let at = |limit: f64| LimitKind::MaxExpectedShortfall {
        confidence: 0.975,
        limit,
    };
    passes(at(0.08), at(0.04), &state());
}

#[test]
fn a_book_beyond_the_expected_shortfall_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxExpectedShortfall {
        confidence: 0.975,
        limit,
    };
    let breach = vetoes(at(0.04), at(0.08), &state());
    assert!((breach.observed - 0.05).abs() < 1e-9);
}

// --- MaxDrawdown ------------------------------------------------------------

#[test]
fn a_book_inside_the_drawdown_limit_passes() {
    let at = |limit: f64| LimitKind::MaxDrawdown { limit };
    passes(at(0.15), at(0.02), &state());
}

#[test]
fn a_book_beyond_the_drawdown_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxDrawdown { limit };
    let breach = vetoes(at(0.02), at(0.15), &state());
    assert!((breach.observed - 0.05).abs() < 1e-9);
}

// --- MaxDailyLoss -----------------------------------------------------------

#[test]
fn a_book_inside_the_daily_loss_limit_passes() {
    let at = |limit: f64| LimitKind::MaxDailyLoss { limit };
    passes(at(0.04), at(0.005), &state());
}

#[test]
fn a_book_beyond_the_daily_loss_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxDailyLoss { limit };
    let breach = vetoes(at(0.005), at(0.04), &state());
    assert!((breach.observed - 0.01).abs() < 1e-9);
}

// --- MinLiquidity -----------------------------------------------------------

#[test]
fn a_book_above_the_liquidity_floor_passes() {
    let at = |fraction: f64| LimitKind::MinLiquidity {
        days: 5.0,
        fraction,
    };
    passes(at(0.80), at(0.99), &state());
}

#[test]
fn a_book_below_the_liquidity_floor_is_vetoed() {
    let at = |fraction: f64| LimitKind::MinLiquidity {
        days: 5.0,
        fraction,
    };
    let breach = vetoes(at(0.99), at(0.80), &state());
    assert!(breach.observed < breach.bound, "a floor binds from below");
}

// --- MaxDaysToLiquidate -----------------------------------------------------

#[test]
fn a_position_inside_the_days_to_liquidate_limit_passes() {
    let at = |limit: f64| LimitKind::MaxDaysToLiquidate { limit };
    passes(at(5.0), at(0.5), &state());
}

#[test]
fn a_position_beyond_the_days_to_liquidate_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxDaysToLiquidate { limit };
    let breach = vetoes(at(0.5), at(5.0), &state());
    assert_eq!(breach.subject.as_deref(), Some("AAPL"));
}

// --- MaxCounterpartyExposure ------------------------------------------------

#[test]
fn a_counterparty_inside_its_exposure_limit_passes() {
    let at = |limit: f64| LimitKind::MaxCounterpartyExposure { limit };
    passes(at(0.30), at(0.10), &state());
}

#[test]
fn a_counterparty_beyond_its_exposure_limit_is_vetoed() {
    let at = |limit: f64| LimitKind::MaxCounterpartyExposure { limit };
    let breach = vetoes(at(0.10), at(0.30), &state());
    assert_eq!(breach.subject.as_deref(), Some("prime"));
    assert!((breach.observed - 0.2).abs() < 1e-9);
}

// --- MinCashBuffer ----------------------------------------------------------

#[test]
fn a_book_above_the_cash_floor_passes() {
    let at = |limit: f64| LimitKind::MinCashBuffer { limit };
    passes(at(0.02), at(0.50), &state());
}

#[test]
fn a_book_below_the_cash_floor_is_vetoed() {
    let at = |limit: f64| LimitKind::MinCashBuffer { limit };
    let breach = vetoes(at(0.50), at(0.02), &state());
    assert!(breach.observed < breach.bound);
}

// --- the table is closed ----------------------------------------------------

#[test]
fn every_limit_kind_has_both_fixtures() {
    // One value of every arm. The `match` below is exhaustive with no
    // wildcard, so a new arm is a compile error here until it has a row in
    // the table and a pair of fixtures above.
    let kinds = [
        LimitKind::MaxOrderNotional { limit: dec!("1") },
        LimitKind::MaxPositionNotional { limit: dec!("1") },
        LimitKind::MaxPositionWeight { limit: 1.0 },
        LimitKind::MaxLeverage { limit: 1.0 },
        LimitKind::MaxNetExposure { limit: 1.0 },
        LimitKind::MaxConcentration {
            axis: "sector".into(),
            limit: 1.0,
        },
        LimitKind::MaxBucketExposure {
            axis: "sector".into(),
            bucket: "energy".into(),
            limit: 1.0,
        },
        LimitKind::MaxVolatility { limit: 1.0 },
        LimitKind::MaxValueAtRisk {
            confidence: 0.99,
            limit: 1.0,
        },
        LimitKind::MaxExpectedShortfall {
            confidence: 0.975,
            limit: 1.0,
        },
        LimitKind::MaxDrawdown { limit: 1.0 },
        LimitKind::MaxDailyLoss { limit: 1.0 },
        LimitKind::MinLiquidity {
            days: 5.0,
            fraction: 0.0,
        },
        LimitKind::MaxDaysToLiquidate { limit: 1.0 },
        LimitKind::MaxCounterpartyExposure { limit: 1.0 },
        LimitKind::MinCashBuffer { limit: 0.0 },
    ];
    let fixtures: [(&str, &str); 16] = [
        (
            "an_order_inside_the_notional_limit_passes",
            "an_order_beyond_the_notional_limit_is_vetoed",
        ),
        (
            "a_position_inside_the_notional_limit_passes",
            "a_position_beyond_the_notional_limit_is_vetoed",
        ),
        (
            "a_position_inside_the_weight_limit_passes",
            "a_position_beyond_the_weight_limit_is_vetoed",
        ),
        (
            "a_book_inside_the_leverage_limit_passes",
            "a_book_beyond_the_leverage_limit_is_vetoed",
        ),
        (
            "a_book_inside_the_net_exposure_limit_passes",
            "a_book_beyond_the_net_exposure_limit_is_vetoed",
        ),
        (
            "a_spread_book_inside_the_concentration_limit_passes",
            "a_bucket_beyond_the_concentration_limit_is_vetoed",
        ),
        (
            "a_bucket_inside_its_exposure_limit_passes",
            "a_bucket_beyond_its_exposure_limit_is_vetoed",
        ),
        (
            "a_book_inside_the_volatility_limit_passes",
            "a_book_beyond_the_volatility_limit_is_vetoed",
        ),
        (
            "a_book_inside_the_value_at_risk_limit_passes",
            "a_book_beyond_the_value_at_risk_limit_is_vetoed",
        ),
        (
            "a_book_inside_the_expected_shortfall_limit_passes",
            "a_book_beyond_the_expected_shortfall_limit_is_vetoed",
        ),
        (
            "a_book_inside_the_drawdown_limit_passes",
            "a_book_beyond_the_drawdown_limit_is_vetoed",
        ),
        (
            "a_book_inside_the_daily_loss_limit_passes",
            "a_book_beyond_the_daily_loss_limit_is_vetoed",
        ),
        (
            "a_book_above_the_liquidity_floor_passes",
            "a_book_below_the_liquidity_floor_is_vetoed",
        ),
        (
            "a_position_inside_the_days_to_liquidate_limit_passes",
            "a_position_beyond_the_days_to_liquidate_limit_is_vetoed",
        ),
        (
            "a_counterparty_inside_its_exposure_limit_passes",
            "a_counterparty_beyond_its_exposure_limit_is_vetoed",
        ),
        (
            "a_book_above_the_cash_floor_passes",
            "a_book_below_the_cash_floor_is_vetoed",
        ),
    ];
    let source = include_str!("limit_fixtures.rs");
    for (kind, (pass, veto)) in kinds.iter().zip(fixtures) {
        let row = match kind {
            LimitKind::MaxOrderNotional { .. } => "MaxOrderNotional",
            LimitKind::MaxPositionNotional { .. } => "MaxPositionNotional",
            LimitKind::MaxPositionWeight { .. } => "MaxPositionWeight",
            LimitKind::MaxLeverage { .. } => "MaxLeverage",
            LimitKind::MaxNetExposure { .. } => "MaxNetExposure",
            LimitKind::MaxConcentration { .. } => "MaxConcentration",
            LimitKind::MaxBucketExposure { .. } => "MaxBucketExposure",
            LimitKind::MaxVolatility { .. } => "MaxVolatility",
            LimitKind::MaxValueAtRisk { .. } => "MaxValueAtRisk",
            LimitKind::MaxExpectedShortfall { .. } => "MaxExpectedShortfall",
            LimitKind::MaxDrawdown { .. } => "MaxDrawdown",
            LimitKind::MaxDailyLoss { .. } => "MaxDailyLoss",
            LimitKind::MinLiquidity { .. } => "MinLiquidity",
            LimitKind::MaxDaysToLiquidate { .. } => "MaxDaysToLiquidate",
            LimitKind::MaxCounterpartyExposure { .. } => "MaxCounterpartyExposure",
            LimitKind::MinCashBuffer { .. } => "MinCashBuffer",
        };
        // Each named fixture exists as a test in this file, and the table
        // row names both. Matched on the whole declaration so a fixture
        // whose name is a prefix of another's cannot stand in for it.
        for name in [pass, veto] {
            assert!(
                source.contains(&format!("fn {name}()")),
                "{row}: fixture `{name}` is named in the table but not defined"
            );
        }
        assert!(
            source.contains(&format!("| `{row}` | `{pass}` | `{veto}` |")),
            "{row}: the table row does not name its fixtures"
        );
    }
}
