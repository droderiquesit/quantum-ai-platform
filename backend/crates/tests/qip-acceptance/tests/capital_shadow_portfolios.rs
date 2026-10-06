//! CAPITAL-036: Shadow portfolios run counterfactual allocations beside every paper allocation.
//!
//! For every allocation, counterfactual shadow portfolios hold alternate sizes,
//! hedges, timing, venues and rejected opportunities, so the realised choice
//! is scored against what it declined.
//!
//! Verification check: One allocation yields at least one alternate-size and one
//! rejected-opportunity shadow, scored after resolution.

use qip_capital::{
    Allocation, AllocationLimits, CapacityModel, CapitalAllocator, DrawdownSchedule,
    ShadowPortfolioUniverse, StrategyProposal,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Timestamp, dec};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};

/// Helper to create a test allocation.
fn make_test_allocation(strategy: &str) -> Allocation {
    Allocation {
        strategy: StrategyId::new(strategy),
        cell: "CELL-A".to_string(),
        venue: VenueId::new("VENUE-1"),
        notional: Decimal::from_int(100_000),
        indicated: Decimal::from_int(120_000),
        risk_adjusted_edge: 0.015,
        binding_constraints: vec!["risk_limit".to_string()],
    }
}

#[test]
fn one_allocation_yields_at_least_one_alternate_size_shadow() {
    let now = Timestamp::from_secs(1_700_000_000);
    let actual = make_test_allocation("momentum-v3");

    let universe = ShadowPortfolioUniverse::new(now)
        .with_alternate_size(
            actual,
            Decimal::from_int(80_000),
            "liquidity constraint limited size".to_string(),
            now,
        )
        .expect("add alternate-size shadow");

    assert_eq!(
        universe.alternate_size_shadows(),
        1,
        "should have one alternate-size shadow"
    );
    assert!(
        universe.shadows_for_strategy("momentum-v3").len() > 0,
        "shadow exists"
    );
}

#[test]
fn one_allocation_yields_at_least_one_rejected_opportunity_shadow() {
    let now = Timestamp::from_secs(1_700_000_000);

    let universe = ShadowPortfolioUniverse::new(now)
        .with_refused(
            "arbitrage-v2".to_string(),
            "capacity exhausted in venue".to_string(),
            now,
        )
        .expect("add refused shadow");

    assert_eq!(
        universe.refused_shadows(),
        1,
        "should have one refused shadow"
    );
    assert!(
        universe.shadows_for_strategy("arbitrage-v2").len() > 0,
        "shadow exists"
    );
}

#[test]
fn a_shadow_portfolio_universe_with_both_types_meets_verification_requirement() {
    let now = Timestamp::from_secs(1_700_000_000);
    let actual = make_test_allocation("momentum-v3");

    let universe = ShadowPortfolioUniverse::new(now)
        .with_alternate_size(
            actual,
            Decimal::from_int(75_000),
            "reduced for market conditions".to_string(),
            now,
        )
        .expect("add alternate-size shadow")
        .with_refused(
            "carry-strategy".to_string(),
            "no capacity available".to_string(),
            now,
        )
        .expect("add refused shadow");

    assert!(
        universe.has_required_shadows(),
        "universe has both alternate-size and refused shadows"
    );
}

#[test]
fn shadows_are_scored_after_resolution_with_pnl_figures() {
    let now = Timestamp::from_secs(1_700_000_000);
    let later = Timestamp::from_secs(1_700_003_600);
    let actual = make_test_allocation("momentum-v3");

    let mut universe = ShadowPortfolioUniverse::new(now)
        .with_alternate_size(
            actual,
            Decimal::from_int(80_000),
            "smaller due to liquidity".to_string(),
            now,
        )
        .expect("add alternate-size shadow");

    // After fills are known, score the shadow
    universe
        .score_shadow(
            "momentum-v3",
            0,
            Decimal::from_int(5_000), // hypothetical: 5k profit
            Decimal::from_int(4_500), // actual: 4.5k profit
            later,
        )
        .expect("score shadow after resolution");

    let shadows = universe.shadows_for_strategy("momentum-v3");
    assert_eq!(shadows.len(), 1);

    let score = shadows[0].score.as_ref().expect("shadow should be scored");
    assert_eq!(score.scored_at, later, "scored at resolution time");
    assert_eq!(
        score.simulated_pnl,
        Decimal::from_int(5_000),
        "simulated outcome"
    );
    assert_eq!(score.actual_pnl, Decimal::from_int(4_500), "actual outcome");
    assert!(
        score.regret_bp > 0,
        "regret when shadow would have been better"
    );
}

#[test]
fn multiple_shadows_per_strategy_track_different_what_ifs() {
    let now = Timestamp::from_secs(1_700_000_000);
    let actual = make_test_allocation("momentum-v3");

    let universe = ShadowPortfolioUniverse::new(now)
        .with_alternate_size(
            actual.clone(),
            Decimal::from_int(120_000),
            "if we had used full indicated size".to_string(),
            now,
        )
        .expect("add larger shadow")
        .with_alternate_size(
            actual,
            Decimal::from_int(60_000),
            "if we had been more conservative".to_string(),
            now,
        )
        .expect("add smaller shadow");

    let shadows = universe.shadows_for_strategy("momentum-v3");
    assert_eq!(shadows.len(), 2, "two alternate scenarios tracked");
}

#[test]
fn shadow_portfolio_universe_tracks_multiple_strategies() {
    let now = Timestamp::from_secs(1_700_000_000);

    let universe = ShadowPortfolioUniverse::new(now)
        .with_alternate_size(
            make_test_allocation("momentum-v3"),
            Decimal::from_int(75_000),
            "momentum reduced".to_string(),
            now,
        )
        .expect("add momentum shadow")
        .with_refused(
            "arbitrage-v2".to_string(),
            "arbitrage capacity exhausted".to_string(),
            now,
        )
        .expect("add arbitrage refused");

    assert_eq!(universe.shadows_for_strategy("momentum-v3").len(), 1);
    assert_eq!(universe.shadows_for_strategy("arbitrage-v2").len(), 1);
    assert_eq!(universe.alternate_size_shadows(), 1);
    assert_eq!(universe.refused_shadows(), 1);
}

#[test]
fn regret_calculation_reflects_value_of_the_choice_not_taken() {
    let now = Timestamp::from_secs(1_700_000_000);
    let later = Timestamp::from_secs(1_700_003_600);
    let actual = make_test_allocation("momentum-v3");

    let mut universe = ShadowPortfolioUniverse::new(now)
        .with_alternate_size(
            actual,
            Decimal::from_int(80_000),
            "smaller size".to_string(),
            now,
        )
        .expect("add shadow");

    // Shadow would have done much better
    universe
        .score_shadow(
            "momentum-v3",
            0,
            Decimal::from_int(12_000), // hypothetical: much better
            Decimal::from_int(4_000),  // actual: much worse
            later,
        )
        .expect("score shadow");

    let score = universe.shadows_for_strategy("momentum-v3")[0]
        .score
        .as_ref()
        .unwrap();

    assert!(score.regret_bp > 1000, "significant regret for large miss");
    assert!(
        score.regret_bp <= 10_000,
        "regret bounded to basis points scale"
    );
}

#[test]
fn a_paper_allocation_can_generate_shadow_portfolios_without_live_execution() {
    let now = Timestamp::from_secs(1_700_000_000);
    let allocator = CapitalAllocator::new(
        AllocationLimits::new(
            dec!("10000000"),
            dec!("4000000"),
            dec!("6000000"),
            dec!("8000000"),
        )
        .expect("create allocation limits"),
        DrawdownSchedule::default(),
    );

    let proposal = StrategyProposal {
        strategy: StrategyId::new("momentum-v3"),
        cell: "cell-lon-1".to_string(),
        venue: VenueId::new("XNYS"),
        expected_sharpe: 1.8,
        sharpe_standard_error: 0.3,
        capacity: CapacityModel::new(
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 4.0),
            TransactionCostModel::listed(4.0),
            45.0,
            dec!("100"),
            0.5,
        )
        .expect("create capacity model"),
        capacity_uncertainty: 0.2,
    };

    let plan = allocator
        .allocate(&[proposal], 0.0, now)
        .expect("allocate capital");

    // Shadow portfolio universe created from the plan
    let mut universe = ShadowPortfolioUniverse::new(now);

    if let Some(allocation) = plan.for_strategy(&StrategyId::new("momentum-v3")) {
        // Add shadow scenarios
        universe = universe
            .with_alternate_size(
                allocation.clone(),
                Decimal::from_int(450_000),
                "half the allocation".to_string(),
                now,
            )
            .expect("add shadow");

        // Add refused opportunity
        universe = universe
            .with_refused(
                "mean-reversion-v1".to_string(),
                "no capacity after momentum".to_string(),
                now,
            )
            .expect("add refused");
    }

    // All of this is paper/simulation — no live execution, no real market impact
    assert!(universe.has_required_shadows(), "both shadow types present");
    assert!(plan.is_within_budget(), "allocation respects paper budget");
}

#[test]
fn no_transfer_leaves_the_shadow_portfolio_universe() {
    let now = Timestamp::from_secs(1_700_000_000);
    let actual = make_test_allocation("momentum-v3");

    let universe = ShadowPortfolioUniverse::new(now)
        .with_alternate_size(
            actual,
            Decimal::from_int(80_000),
            "test scenario".to_string(),
            now,
        )
        .expect("add shadow");

    // Verify shadows are recorded in memory/memory structures only
    // (The universe holds serialized data, not live trading state)
    let shadows_json = serde_json::to_string(&universe).expect("shadows serialize to JSON");

    // Verify structure is intact
    assert!(shadows_json.contains("momentum-v3"), "strategy recorded");
    assert!(
        shadows_json.contains("alternate_size"),
        "scenario type recorded"
    );
    assert!(shadows_json.contains("80000"), "amount recorded");

    // No transfer/execution state should be in the shadow universe
    assert!(!shadows_json.contains("transfer"), "no transfer state");
    assert!(!shadows_json.contains("execution"), "no execution state");
}
