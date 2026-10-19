//! Margin utilisation and the funding buffer reach the Risk Gate as limits
//! (R12-RISK-4098).
//!
//! The cross-margin review raised a finding and a stage problem on a call,
//! and the survival waterfall withheld the funding-maturity tier, and neither
//! figure was compared against an order: both were measurements that read as
//! controls. These tests drive orders through `Platform::submit_order` — the
//! governed path every order takes — against a platform assembled on a limit
//! set naming each rule, and show the kernel's `risk_state_from` fills the
//! figure the rule reads: the order that deepens a call is refused, the one
//! that sheds gross is not, and a book with nothing observed is untouched.
//!
//! Paper boundary: nothing here reaches a venue. Orders go to the simulated
//! broker, the limits are pre-trade vetoes, and no autonomy level is touched.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::aggregate::AggregateFigures;
use qip_risk::limits::{COUNTERPARTY_AXIS, Limit, LimitKind, LimitSet};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn listed() -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string("obj-ACME"),
        "ACME",
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(Provenance::synthetic("margin-funding", start()))
    .build(start())
}

fn private_fund() -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string("obj-FUND"),
        "FUND",
        InstrumentType::PrivateEquityFund,
        LiquidityProfile::illiquid(90.0, 250.0),
    )
    .venue("OTC")
    .geography("US")
    .price(dec!("100"))
    .extension(Extension::PrivateAsset(PrivateAssetDetails {
        vintage_year: 2024,
        committed_capital: dec!("400000"),
        called_capital: dec!("150000"),
        distributed_capital: Decimal::ZERO,
        residual_value: dec!("160000"),
        stage: "buyout".to_string(),
        lockup_years: 7.0,
        capital_call_notice_days: 10,
        call_schedule: None,
    }))
    .provenance(Provenance::synthetic("administrator", start()))
    .build(start())
}

fn wide() -> LimitSet {
    LimitSet::new("margin-funding-test").with(
        Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional {
                limit: Decimal::from_int(100_000_000),
            },
        )
        .with_rationale("wide enough that only the rule under test binds"),
    )
}

fn platform(universe: Universe, equity: Decimal, limits: LimitSet) -> Result<Platform> {
    let config = PlatformConfig::default().with_initial_equity(equity);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits)
}

fn order(platform: &mut Platform, side: Side, quantity: Decimal) -> Result<()> {
    let order = platform.order_from(
        ObjectId::from_string("obj-ACME"),
        side,
        quantity,
        dec!("100"),
        "prop-margin-funding",
        vec!["hyp-margin-funding".to_string()],
        start(),
    );
    platform.submit_order(order, start())
}

/// A platform that holds 2000 ACME at 100 and the counterparty its fill was
/// charged to, read back off the aggregate rather than written down.
fn traded(limits: LimitSet) -> Result<(Platform, VenueId)> {
    let mut universe = Universe::new();
    universe.insert(listed()?)?;
    let mut platform = platform(universe, Decimal::from_int(10_000_000), limits)?;
    order(&mut platform, Side::Buy, dec!("2000"))?;
    let counterparty = platform
        .risk_figures()
        .axis_exposures()
        .get(COUNTERPARTY_AXIS)
        .and_then(|axis| axis.keys().next().cloned())
        .expect("the premise failed: the fill was charged to no counterparty");
    Ok((platform, VenueId::new(counterparty)))
}

fn margin_limits() -> LimitSet {
    wide().with(
        Limit::new(
            "margin-utilisation",
            LimitKind::MaxMarginUtilisation { limit: 1.0 },
        )
        .with_rationale("maintenance may not exceed the cover the statements show"),
    )
}

/// Mutations that make it fail: leave `margin_utilisation` unset in
/// `Platform::with_margin_and_funding` (the thin-statement buy is admitted);
/// compute it over `maintenance_required` including unobserved venues (the
/// no-statement premise fails first); drop the `sheds_gross` exemption in
/// `qip-risk` (the sale is refused).
#[test]
fn a_book_whose_statement_cannot_cover_its_maintenance_is_refused_more_risk_but_may_sell()
-> Result<()> {
    let (mut platform, venue) = traded(margin_limits())?;

    // Premise: with no statement handed in nothing is observed, the figure is
    // absent and the rule records nothing, so the book trades on.
    assert!(platform.holdings_observed().is_empty());
    assert!(
        platform
            .risk_state_from(platform.risk_figures())
            .margin_utilisation
            .is_none(),
        "the premise failed: a book with no statement carried a utilisation figure"
    );
    order(&mut platform, Side::Buy, dec!("100"))?;

    // A dollar against a book carrying a quarter of its gross as maintenance.
    platform.observe_statement(venue.clone(), "USD", dec!("1"), dec!("1"), start())?;
    let utilisation = platform
        .risk_state_from(platform.risk_figures())
        .margin_utilisation
        .expect("a statement was handed in, so the figure is filed");
    assert!(
        utilisation > 1.0,
        "the premise failed: a dollar of cover read as {utilisation} of maintenance"
    );

    let refused = order(&mut platform, Side::Buy, dec!("100"))
        .expect_err("an order that deepens a margin call was admitted");
    assert!(
        refused.message().contains("margin-utilisation"),
        "the refusal did not name the margin rule: {}",
        refused.message()
    );

    // The way out is never closed: selling does not raise gross.
    order(&mut platform, Side::Sell, dec!("100"))?;

    // And a statement that covers the book opens it again.
    let gross = platform.risk_figures().gross_exposure();
    platform.observe_statement(venue, "USD", gross, dec!("1"), start())?;
    order(&mut platform, Side::Buy, dec!("100"))?;
    Ok(())
}

#[test]
fn a_venue_nobody_handed_a_statement_for_is_not_counted_against_the_margin_rule() -> Result<()> {
    // The cross-margin finding names an unread venue; the gate does not
    // refuse over it, because a desk is entitled to have handed in nothing.
    // A statement at some *other* venue makes the figure present and finite.
    let (mut platform, _venue) = traded(margin_limits())?;
    platform.observe_statement(
        VenueId::new("some-other-prime"),
        "USD",
        dec!("1000000"),
        dec!("1"),
        start(),
    )?;
    let utilisation = platform
        .risk_state_from(platform.risk_figures())
        .margin_utilisation
        .expect("a statement was handed in, so the figure is filed");
    assert!(
        utilisation.abs() < 1e-12,
        "unobserved maintenance leaked into the figure: {utilisation}"
    );
    order(&mut platform, Side::Buy, dec!("100"))?;
    Ok(())
}

/// Mutations that make it fail: in `Platform::with_margin_and_funding`
/// divide `review.maintenance_required` by `review.cover_observed` again (the
/// pooled ratio reads 0.05 and the buy is admitted); in `qip-risk`'s
/// `MarginBasis::worst_utilisation` take the sum of covers instead of each
/// venue's own.
#[test]
fn spare_cover_at_another_prime_does_not_hide_a_margin_call_at_the_desk_venue() -> Result<()> {
    let (mut platform, venue) = traded(margin_limits())?;
    platform.observe_statement(venue.clone(), "USD", dec!("1"), dec!("1"), start())?;
    platform.observe_statement(
        VenueId::new("some-other-prime"),
        "USD",
        dec!("1000000"),
        dec!("1"),
        start(),
    )?;
    // Premise: the cross-margin review itself lists the desk venue in a call,
    // so the refusal below is the gate agreeing with a finding already made.
    let review = qip_kernel::cross_margin::review(
        platform.holdings_observed(),
        platform.risk_figures(),
        &qip_capital::margin::MarginModel::default(),
        qip_core::Currency::parse("USD")?,
    )?;
    assert_eq!(
        review.calls,
        vec![venue],
        "premise: the desk venue is in a call"
    );
    // Premise: pooled, the figure would sit far under the bound.
    let pooled = (review.maintenance_required - review.unobserved_maintenance()).to_f64()
        / review.cover_observed.to_f64();
    assert!(pooled < 0.1, "premise: pooled utilisation was {pooled}");

    let refused = order(&mut platform, Side::Buy, dec!("100"))
        .expect_err("a buy into a venue in a call was admitted on another prime's surplus");
    assert!(
        refused.message().contains("margin-utilisation"),
        "the refusal did not name the margin rule: {}",
        refused.message()
    );
    Ok(())
}

/// Mutations that make it fail: in `MarginBasis::worst_utilisation` read
/// gross from the unprojected state (pass `None` for the counterparty axis in
/// the limit's arm and the 2000-share buy is admitted); drop the
/// `margin_basis` filing in `with_margin_and_funding`.
#[test]
fn the_order_that_takes_a_covered_book_into_a_margin_call_is_refused_itself() -> Result<()> {
    let (mut platform, venue) = traded(margin_limits())?;
    // 2000 ACME at 100 is 200k gross, 50k maintenance at the default 0.25
    // rate; 55k of cover leaves the book inside the bound before any order.
    platform.observe_statement(venue, "USD", dec!("55000"), dec!("1"), start())?;
    let before = platform
        .risk_state_from(platform.risk_figures())
        .margin_utilisation
        .expect("a statement was handed in, so the figure is filed");
    assert!(
        before > 0.0 && before < 1.0,
        "premise: the book stands inside the bound before the order, at {before}"
    );
    // A small buy stays inside the cover once charged.
    order(&mut platform, Side::Buy, dec!("100"))?;
    // One that doubles the gross takes maintenance to 100k against 55k.
    let refused = order(&mut platform, Side::Buy, dec!("2000"))
        .expect_err("an order that creates the margin call was admitted");
    assert!(
        refused.message().contains("margin-utilisation"),
        "the refusal did not name the margin rule: {}",
        refused.message()
    );
    Ok(())
}

fn funding_desk(floor: f64) -> Result<Platform> {
    let mut universe = Universe::new();
    universe.insert(listed()?)?;
    universe.insert(private_fund()?)?;
    let limits = wide().with(
        Limit::new(
            "funding-buffer",
            LimitKind::MinFundingBuffer { limit: floor },
        )
        .with_rationale("equity left after unfunded commitments needs headroom"),
    );
    platform(universe, Decimal::from_int(1_000_000), limits)
}

/// Mutations that make it fail: drop the `funding_maturity_required` filter
/// in `with_margin_and_funding` (the no-commitment premise fails); read
/// `free_cash` without subtracting the funding tier (the 0.9 floor admits).
#[test]
fn a_book_owing_more_than_its_floor_allows_is_refused_more_risk() -> Result<()> {
    let mut desk = funding_desk(0.90)?;
    let (_, maturing) = desk
        .survival_funding_read()
        .expect("the premise failed: assembly seeded no survival read");
    assert_eq!(
        maturing,
        dec!("250000"),
        "the premise failed: the fund's unfunded balance is not on the funding tier"
    );
    let buffer = desk
        .risk_state_from(desk.risk_figures())
        .funding_buffer
        .expect("a commitment stands, so the figure is filed");
    assert!(
        (buffer - 0.75).abs() < 1e-9,
        "a million less 250,000 owed is 0.75 of equity, got {buffer}"
    );
    let refused = order(&mut desk, Side::Buy, dec!("100"))
        .expect_err("an order was admitted below the funding floor");
    assert!(
        refused.message().contains("funding-buffer"),
        "the refusal did not name the funding rule: {}",
        refused.message()
    );

    // The same book against a floor it clears.
    let mut easy = funding_desk(0.50)?;
    order(&mut easy, Side::Buy, dec!("100"))?;
    Ok(())
}

#[test]
fn a_book_with_no_commitment_carries_no_funding_figure() -> Result<()> {
    let (platform, _venue) = traded(margin_limits())?;
    assert!(
        platform.commitments().unfunded_total(start())?.is_zero(),
        "the premise failed: the plain book owes something"
    );
    assert!(
        platform
            .risk_state_from(platform.risk_figures())
            .funding_buffer
            .is_none(),
        "a book with nothing maturing was given a funding ratio nobody asked about"
    );
    Ok(())
}
