//! CAPITAL-035: Survival Kernel withholds reserves before releasing opportunity capital.
//!
//! The Capital Survival Kernel accounts for NAV, reserved cash, margin,
//! collateral, settlement and funding maturity, and withholds hedge and
//! stressed-liquidity capital before releasing opportunity capital.
//!
//! Verification check: A paper book withholds the stressed-liquidity reserve
//! before releasing any opportunity capital, and no transfer leaves the simulator.

use qip_capital::CapitalSurvivalState;
use qip_core::{Decimal, dec};

/// A paper book withholds all reserves in priority order before releasing opportunity capital.
#[test]
fn a_paper_book_withholds_reserves_in_priority_order_before_releasing_opportunity_capital() {
    let book_capital = dec!("10000000");

    let state = CapitalSurvivalState::new(book_capital)
        .expect("create survival state with total capital")
        .with_nav_floor(dec!("1000000"))
        .expect("set NAV floor")
        .with_reserved_cash(dec!("1500000"))
        .expect("set reserved cash floor")
        .with_margin_required(dec!("750000"))
        .expect("set margin required")
        .with_collateral_posted(dec!("500000"))
        .expect("set collateral posted")
        .with_settlement_required(dec!("250000"))
        .expect("set settlement required")
        .with_funding_maturity(dec!("500000"))
        .expect("set funding maturity required")
        .with_hedge_capacity(dec!("2000000"))
        .expect("set hedge capacity")
        .with_stressed_liquidity_reserve(dec!("2000000"))
        .expect("set stressed-liquidity reserve");

    // Calculate total withheld reserves
    let _total_withheld = dec!("1000000") // NAV floor
        + dec!("1500000") // reserved cash
        + dec!("750000") // margin
        + dec!("500000") // collateral
        + dec!("250000") // settlement
        + dec!("500000") // funding maturity
        + dec!("2000000") // hedge capacity
        + dec!("2000000"); // stressed-liquidity reserve
    // = 8,500,000

    let opportunity = state
        .opportunity_capital()
        .expect("calculate opportunity capital");

    // 10,000,000 - 8,500,000 = 1,500,000
    assert_eq!(opportunity, dec!("1500000"));
}

/// A paper book refuses to release opportunity capital when reserves exceed total capital.
#[test]
fn a_paper_book_refuses_opportunity_capital_when_reserves_exceed_total_capital() {
    let book_capital = dec!("1000000");

    let state = CapitalSurvivalState::new(book_capital)
        .expect("create survival state")
        .with_nav_floor(dec!("400000"))
        .expect("set NAV floor")
        .with_reserved_cash(dec!("300000"))
        .expect("set reserved cash")
        .with_hedge_capacity(dec!("400000"))
        .expect("set hedge capacity");

    // Total withheld = 400,000 + 300,000 + 400,000 = 1,100,000
    // This exceeds 1,000,000 total capital
    let result = state.opportunity_capital();

    assert!(
        result.is_err(),
        "opportunity capital should be refused when reserves exceed total"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("capital survival check failed"),
        "error should name the survival check failure: {}",
        err
    );
}

/// A paper book correctly computes opportunity capital with multiple tiers populated.
#[test]
fn a_paper_book_correctly_computes_opportunity_capital_across_all_tiers() {
    let book_capital = dec!("5000000");

    let state = CapitalSurvivalState::new(book_capital)
        .expect("create survival state")
        .with_nav_floor(dec!("500000"))
        .expect("set NAV floor")
        .with_reserved_cash(dec!("500000"))
        .expect("set reserved cash")
        .with_margin_required(dec!("300000"))
        .expect("set margin required")
        .with_collateral_posted(dec!("400000"))
        .expect("set collateral posted")
        .with_settlement_required(dec!("200000"))
        .expect("set settlement required")
        .with_funding_maturity(dec!("300000"))
        .expect("set funding maturity")
        .with_hedge_capacity(dec!("800000"))
        .expect("set hedge capacity")
        .with_stressed_liquidity_reserve(dec!("600000"))
        .expect("set stressed-liquidity reserve");

    let opportunity = state
        .opportunity_capital()
        .expect("calculate opportunity capital");

    // Total withheld = 500 + 500 + 300 + 400 + 200 + 300 + 800 + 600 = 3,600,000 (in thousands)
    // Opportunity = 5,000,000 - 3,600,000 = 1,400,000
    assert_eq!(opportunity, dec!("1400000"));
}

/// A paper book accepts zero opportunity capital when fully reserved.
#[test]
fn a_paper_book_accepts_zero_opportunity_capital_when_fully_reserved() {
    let book_capital = dec!("1000000");

    let state = CapitalSurvivalState::new(book_capital)
        .expect("create survival state")
        .with_nav_floor(dec!("200000"))
        .expect("set NAV floor")
        .with_reserved_cash(dec!("250000"))
        .expect("set reserved cash")
        .with_margin_required(dec!("150000"))
        .expect("set margin required")
        .with_collateral_posted(dec!("100000"))
        .expect("set collateral posted")
        .with_settlement_required(dec!("50000"))
        .expect("set settlement required")
        .with_funding_maturity(dec!("50000"))
        .expect("set funding maturity")
        .with_hedge_capacity(dec!("100000"))
        .expect("set hedge capacity")
        .with_stressed_liquidity_reserve(dec!("100000"))
        .expect("set stressed-liquidity reserve");

    // Total withheld = 200 + 250 + 150 + 100 + 50 + 50 + 100 + 100 = 1,000,000
    let opportunity = state
        .opportunity_capital()
        .expect("calculate opportunity capital when fully reserved");

    assert_eq!(opportunity, Decimal::ZERO);
}

/// A paper book's survival report names all withheld amounts and remaining opportunity.
#[test]
fn a_paper_books_survival_report_is_complete_and_accurate() {
    let book_capital = dec!("10000000");

    let state = CapitalSurvivalState::new(book_capital)
        .expect("create survival state")
        .with_nav_floor(dec!("1000000"))
        .expect("set NAV floor")
        .with_reserved_cash(dec!("1500000"))
        .expect("set reserved cash")
        .with_hedge_capacity(dec!("2000000"))
        .expect("set hedge capacity")
        .with_stressed_liquidity_reserve(dec!("2000000"))
        .expect("set stressed-liquidity reserve");

    let report = state.report().expect("generate survival report");

    assert_eq!(report.total_capital, dec!("10000000"));
    assert_eq!(report.nav_floor, dec!("1000000"));
    assert_eq!(report.reserved_cash, dec!("1500000"));
    assert_eq!(report.margin_required, Decimal::ZERO);
    assert_eq!(report.collateral_posted, Decimal::ZERO);
    assert_eq!(report.settlement_required, Decimal::ZERO);
    assert_eq!(report.funding_maturity_required, Decimal::ZERO);
    assert_eq!(report.hedge_capacity, dec!("2000000"));
    assert_eq!(report.stressed_liquidity_reserve, dec!("2000000"));
    // 10,000,000 - (1,000,000 + 1,500,000 + 2,000,000 + 2,000,000) = 3,500,000
    assert_eq!(report.opportunity_capital, dec!("3500000"));
}

/// A paper book's survival kernel refuses negative total capital.
#[test]
fn a_paper_book_refuses_negative_total_capital() {
    let result = CapitalSurvivalState::new(dec!("-1000000"));
    assert!(
        result.is_err(),
        "survival state should refuse negative total capital"
    );
}

/// A paper book's survival kernel refuses negative reserve amounts.
#[test]
fn a_paper_book_refuses_negative_reserve_amounts() {
    assert!(
        CapitalSurvivalState::new(dec!("1000000"))
            .expect("create survival state")
            .with_nav_floor(dec!("-100000"))
            .is_err(),
        "should refuse negative NAV floor"
    );
    assert!(
        CapitalSurvivalState::new(dec!("1000000"))
            .expect("create survival state")
            .with_reserved_cash(dec!("-100000"))
            .is_err(),
        "should refuse negative reserved cash"
    );
    assert!(
        CapitalSurvivalState::new(dec!("1000000"))
            .expect("create survival state")
            .with_margin_required(dec!("-100000"))
            .is_err(),
        "should refuse negative margin"
    );
    assert!(
        CapitalSurvivalState::new(dec!("1000000"))
            .expect("create survival state")
            .with_hedge_capacity(dec!("-100000"))
            .is_err(),
        "should refuse negative hedge capacity"
    );
    assert!(
        CapitalSurvivalState::new(dec!("1000000"))
            .expect("create survival state")
            .with_stressed_liquidity_reserve(dec!("-100000"))
            .is_err(),
        "should refuse negative stressed-liquidity reserve"
    );
}

/// A paper book's survival kernel correctly chains builder methods.
#[test]
fn a_paper_books_survival_kernel_builder_methods_chain() {
    let result = CapitalSurvivalState::new(dec!("1000000"))
        .and_then(|s| s.with_nav_floor(dec!("100000")))
        .and_then(|s| s.with_reserved_cash(dec!("200000")))
        .and_then(|s| s.with_hedge_capacity(dec!("300000")))
        .and_then(|s| s.with_stressed_liquidity_reserve(dec!("150000")))
        .and_then(|s| s.opportunity_capital());

    assert!(result.is_ok(), "builder methods should chain correctly");
    let opportunity = result.expect("unwrap opportunity capital");
    // 1,000,000 - (100,000 + 200,000 + 300,000 + 150,000) = 250,000
    assert_eq!(opportunity, dec!("250000"));
}

/// A paper book's stressed-liquidity reserve is withheld before opportunity capital is released.
#[test]
fn a_paper_book_withholds_stressed_liquidity_reserve_before_opportunity_capital() {
    // Scenario: book needs 100k stressed-liquidity reserve
    let book_capital = dec!("500000");
    let stressed_reserve = dec!("100000");

    let state_with_reserve = CapitalSurvivalState::new(book_capital)
        .expect("create state")
        .with_stressed_liquidity_reserve(stressed_reserve)
        .expect("set stressed-liquidity reserve");

    let opportunity_with_reserve = state_with_reserve
        .opportunity_capital()
        .expect("calculate opportunity with reserve");

    // Verify the reserve was withheld
    assert_eq!(opportunity_with_reserve, dec!("400000"));

    // Scenario: book without stressed-liquidity reserve (should have more opportunity)
    let state_without_reserve = CapitalSurvivalState::new(book_capital)
        .expect("create state without reserve")
        .with_stressed_liquidity_reserve(Decimal::ZERO)
        .expect("set zero stressed-liquidity reserve");

    let opportunity_without_reserve = state_without_reserve
        .opportunity_capital()
        .expect("calculate opportunity without reserve");

    // Verify reserve was the difference
    assert_eq!(opportunity_without_reserve, book_capital);
    assert_eq!(
        opportunity_without_reserve - opportunity_with_reserve,
        stressed_reserve
    );
}
