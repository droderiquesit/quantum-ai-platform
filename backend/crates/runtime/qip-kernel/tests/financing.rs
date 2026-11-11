//! The financing register, from the universe to the cycle report
//! (R12-COVERAGE-5098, R12-COVERAGE-5100).
//!
//! The failure these prevent: `covered_interest_forward` priced a forward and
//! nothing called it, a repo was an instrument nothing valued, and the kernel
//! converted balances through a table that priced the dollar alone. Each test
//! drives the register through `Platform::new` — the path every binary takes
//! — and checks the figure against arithmetic done here, independently of the
//! function under test.

#![allow(clippy::panic_in_result_fn, clippy::expect_used)] // integration tests may expect: a panic is the failure report

use qip_capital_fabric::{CapitalLocation, DemandKind, RealisedDemand, Region as CapitalRegion};
use qip_contracts::VenueId;
use qip_core::error::Result;
use qip_core::time::Duration;
use qip_core::{Context, Currency, Decimal, ObjectId, Timestamp, dec};
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::extensions::{
    BondDetails, CouponFrequency, CreditRating, DayCount, Extension, FxDetails, PerpetualDetails,
    RepoDetails, Seniority,
};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::cycle::Stage;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn liquidity() -> LiquidityProfile {
    LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn provenance() -> Provenance {
    Provenance::synthetic("financing-test", start())
}

/// A sovereign issue in `currency` at `tenor_years`, quoting `yield_`: what
/// the credit register builds that currency's curve from.
fn govvie(id: &str, currency: Currency, tenor_years: i64, yield_: f64) -> Result<FinancialObject> {
    let maturity = start().saturating_add(Duration::from_days(tenor_years * 365 + tenor_years / 4));
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::GovernmentBond,
        liquidity(),
    )
    .venue("OTC")
    .currency(currency)
    .price(dec!("100"))
    .provenance(provenance())
    .extension(Extension::Bond(BondDetails {
        issuer: "Sovereign".into(),
        coupon_rate: yield_,
        coupon_frequency: CouponFrequency::SemiAnnual,
        maturity,
        issue_date: start(),
        face_value: dec!("100"),
        day_count: DayCount::ActualActual,
        seniority: Seniority::SeniorUnsecured,
        credit_rating: Some(CreditRating::Aaa),
        yield_to_maturity: yield_,
        modified_duration: tenor_years as f64,
        convexity: 0.0,
        option_adjusted_spread_bps: 0.0,
        callable: false,
        puttable: false,
        inflation_index: None,
    }))
    .build(start())
}

/// A flat curve in `currency` at `yield_`: two benchmarks so the curve forms.
fn flat_curve(prefix: &str, currency: Currency, yield_: f64) -> Result<Vec<FinancialObject>> {
    Ok(vec![
        govvie(&format!("obj-{prefix}-2Y"), currency, 2, yield_)?,
        govvie(&format!("obj-{prefix}-10Y"), currency, 10, yield_)?,
    ])
}

/// An FX instrument on EUR/USD; `settles_in_days` makes it a forward.
fn eurusd(id: &str, price: Decimal, settles_in_days: Option<i64>) -> Result<FinancialObject> {
    let kind = if settles_in_days.is_some() {
        InstrumentType::FxForward
    } else {
        InstrumentType::FxSpot
    };
    let mut builder = FinancialObject::builder(ObjectId::from_string(id), id, kind, liquidity())
        .venue("OTC")
        .currency(Currency::USD)
        .price(price)
        .tick_size(dec!("0.00001"))
        .provenance(provenance())
        .extension(Extension::Fx(FxDetails {
            base_currency: Currency::EUR,
            quote_currency: Currency::USD,
            settlement_date: settles_in_days
                .map(|days| start().saturating_add(Duration::from_days(days))),
            forward_points: 0.0,
            rate_differential: 0.0,
        }));
    if settles_in_days.is_some() {
        builder = builder.underlying(ObjectId::from_string("obj-EURUSD"));
    }
    builder.build(start())
}

fn repo(id: &str, collateral: &str, haircut: f64) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::Repo,
        liquidity(),
    )
    .venue("OTC")
    .price(dec!("100"))
    .provenance(provenance())
    .extension(Extension::Repo(RepoDetails {
        collateral_object: collateral.to_string(),
        maturity: start().saturating_add(Duration::from_days(30)),
        repo_rate: 0.05,
        haircut,
        is_reverse: false,
        counterparty: "Dealer A".into(),
    }))
    .build(start())
}

fn assemble(objects: Vec<FinancialObject>) -> Result<Platform> {
    let mut universe = Universe::new();
    for object in objects {
        universe.insert(object)?;
    }
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
}

fn understand_problems(platform: &mut Platform) -> (String, Vec<String>) {
    let report = platform.run_cycle(start());
    let understand = report
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Understand)
        .expect("the cycle ran no UNDERSTAND stage");
    assert!(understand.ran, "the UNDERSTAND stage did not run");
    (understand.detail.clone(), understand.problems.clone())
}

/// Mutations that fired: passing the base currency's rate as the domestic
/// one in `FinancingRegister::value_forward` puts the outright below spot
/// and fails the first comparison; passing the curve's zero rate through
/// `simple_rate` unconverted moves the outright off the continuous-
/// compounding figure by more than the tolerance.
#[test]
fn an_fx_forward_is_carried_from_the_universes_spot_along_the_two_sovereign_curves() -> Result<()> {
    let mut objects = flat_curve("UST", Currency::USD, 0.05)?;
    objects.extend(flat_curve("BUND", Currency::EUR, 0.03)?);
    objects.push(eurusd("obj-EURUSD", dec!("1.10"), None)?);
    objects.push(eurusd("obj-EURUSD-3M", dec!("1.10"), Some(90))?);
    let mut platform = assemble(objects)?;

    let register = platform.financing_register();
    // Premise: both curves formed, so the rates below were read and not
    // refused.
    assert!(platform.credit_register().curve(Currency::USD).is_some());
    assert!(platform.credit_register().curve(Currency::EUR).is_some());
    let forward = register
        .forward("obj-EURUSD-3M")
        .unwrap_or_else(|| panic!("the forward was not valued: {:?}", register.problems()));
    assert_eq!(forward.spot_object, "obj-EURUSD");
    assert_eq!(forward.spot, dec!("1.10"));
    assert_eq!(forward.days, 90);
    assert_eq!(
        (forward.base, forward.quote),
        (Currency::EUR, Currency::USD)
    );

    // Dollars earn more than euros here, so the euro is dearer forward.
    assert!(
        forward.outright > forward.spot,
        "the higher-yielding quote currency did not put the forward above spot: {forward:?}"
    );
    // The independent figure: both curves are flat and continuously
    // compounded, so parity is spot × e^((r_usd − r_eur) × t) with t on the
    // curves' own 365.25-day year. Statistics arithmetic, compared within a
    // tenth of a pip.
    let expected = 1.10 * ((0.05 - 0.03) * 90.0 / 365.25_f64).exp();
    let got = forward.outright.to_f64();
    assert!(
        (got - expected).abs() < 1e-5,
        "outright {got} is not the parity figure {expected}"
    );

    let (detail, problems) = understand_problems(&mut platform);
    assert!(
        detail.contains("financing register values 1 FX forward(s)"),
        "the UNDERSTAND stage does not report the register: {detail}"
    );
    assert!(
        !problems
            .iter()
            .any(|problem| problem.starts_with("financing instrument ")),
        "{problems:?}"
    );
    Ok(())
}

/// Mutation that fired: making `simple_rate` answer zero for a currency with
/// no curve values the forward at an invented rate, and both assertions on
/// the refusal fail.
#[test]
fn a_forward_whose_currency_has_no_curve_is_refused_and_the_cycle_says_so() -> Result<()> {
    let mut objects = flat_curve("UST", Currency::USD, 0.05)?;
    objects.push(eurusd("obj-EURUSD", dec!("1.10"), None)?);
    objects.push(eurusd("obj-EURUSD-3M", dec!("1.10"), Some(90))?);
    let mut platform = assemble(objects)?;

    // Premise: the dollar curve exists, so the refusal is the euro's.
    assert!(platform.credit_register().curve(Currency::USD).is_some());
    assert!(platform.credit_register().curve(Currency::EUR).is_none());
    assert!(
        platform
            .financing_register()
            .forward("obj-EURUSD-3M")
            .is_none(),
        "a forward was priced with no euro rate"
    );
    let (_, problems) = understand_problems(&mut platform);
    assert!(
        problems.iter().any(|problem| problem
            .starts_with("financing instrument obj-EURUSD-3M is unvalued:")
            && problem.contains("no EUR sovereign curve")),
        "the unvalued forward did not reach the cycle report: {problems:?}"
    );
    Ok(())
}

/// Mutations that fired: dropping the haircut from `value_repo` lends the
/// full collateral value (cash 100, not 98); admitting a haircut of one
/// values a repo that lends nothing.
#[test]
fn a_repo_is_valued_from_its_own_terms_and_one_that_lends_nothing_is_refused() -> Result<()> {
    let mut objects = flat_curve("UST", Currency::USD, 0.05)?;
    objects.push(repo("obj-REPO", "obj-UST-10Y", 0.02)?);
    objects.push(repo("obj-REPO-FULL-HAIRCUT", "obj-UST-10Y", 1.0)?);
    objects.push(repo("obj-REPO-NO-COLLATERAL", "obj-ABSENT", 0.02)?);
    let mut platform = assemble(objects)?;

    let register = platform.financing_register();
    let valued = register
        .repo("obj-REPO")
        .unwrap_or_else(|| panic!("the repo was not valued: {:?}", register.problems()));
    assert_eq!(valued.collateral_value, dec!("100"));
    assert_eq!(valued.currency, Currency::USD);
    assert_eq!(valued.days, 30);
    // 100 of collateral less a 2% haircut.
    assert_eq!(valued.cash, dec!("98"));
    // 98 at 5% for 30 days of a 365.25-day year, done here in exact
    // arithmetic: 98 × 0.05 × 120 / 1461.
    let expected_interest = dec!("98")
        .checked_mul(dec!("0.05"))
        .and_then(|annual| annual.checked_mul(Decimal::from_int(120)))
        .and_then(|scaled| scaled.checked_div(Decimal::from_int(1461)))
        .expect("the fixture's interest fits a decimal");
    assert!(expected_interest.is_positive());
    let difference = (valued.interest.to_f64() - expected_interest.to_f64()).abs();
    assert!(
        difference < 1e-6,
        "interest {} is not {expected_interest}",
        valued.interest
    );
    assert_eq!(
        valued.repurchase,
        valued.cash.checked_add(valued.interest).expect("fits")
    );
    assert!(!valued.is_reverse);
    assert_eq!(valued.counterparty, "Dealer A");

    assert!(register.repo("obj-REPO-FULL-HAIRCUT").is_none());
    assert!(register.repo("obj-REPO-NO-COLLATERAL").is_none());
    let (_, problems) = understand_problems(&mut platform);
    let unvalued = |id: &str, reason: &str| {
        problems.iter().any(|problem| {
            problem.starts_with(&format!("financing instrument {id} is unvalued:"))
                && problem.contains(reason)
        })
    };
    assert!(
        unvalued("obj-REPO-FULL-HAIRCUT", "outside [0, 1)"),
        "{problems:?}"
    );
    assert!(
        unvalued(
            "obj-REPO-NO-COLLATERAL",
            "obj-ABSENT is not in the universe"
        ),
        "{problems:?}"
    );
    Ok(())
}

/// Mutation that fired: restoring `FxRates::new(Currency::USD)` in
/// `Platform::pre_position` leaves the euro unpriced, and scoring the plan
/// fails with "no rate for EUR against the USD base".
#[test]
fn a_balance_is_converted_at_the_universes_spot_pair_and_an_unquoted_currency_is_still_refused()
-> Result<()> {
    let mut objects = flat_curve("UST", Currency::USD, 0.05)?;
    objects.push(eurusd("obj-EURUSD", dec!("1.10"), None)?);
    let mut platform = assemble(objects)?;

    let rates = platform.financing_register().fx_rates(Currency::USD)?;
    assert_eq!(
        rates.to_base(dec!("100"), Currency::EUR)?,
        dec!("110"),
        "a euro balance is converted at the spot pair's price"
    );
    let error = rates
        .to_base(dec!("100"), Currency::JPY)
        .expect_err("a currency the universe does not quote was converted");
    assert!(error.message().contains("JPY"), "{}", error.message());

    // The same table reaches the pre-positioning plan: a plan over a euro
    // lane is scored against a euro need at the spot pair's price, where a
    // dollar-only table stops the scoring on an unpriced currency.
    let (desk_venue, jurisdiction) = platform
        .settlement_book()
        .venues()
        .map(|(venue, region)| (venue.as_str().to_string(), region.as_str().to_string()))
        .next()
        .expect("premise: the desk declares the venue it sends to");
    let euro_lane = CapitalLocation::new(
        CapitalRegion::new(jurisdiction.as_str()),
        Currency::EUR,
        VenueId::new(desk_venue.as_str()),
    );
    for day in 0..2 {
        platform.record_capital_demand(
            euro_lane.clone(),
            DemandKind::Cash,
            start().saturating_add(Duration::from_days(day)),
            dec!("100000"),
        );
    }
    let at = start().saturating_add(Duration::from_days(2));
    let horizon = Duration::from_days(7);
    assert!(
        platform
            .forecast_capital_demand(at, horizon)
            .iter()
            .any(|forecast| forecast.location.currency == Currency::EUR),
        "premise: the euro lane is forecast"
    );
    let realised = RealisedDemand::new().with(euro_lane.clone(), DemandKind::Cash, dec!("1000"));
    let score = platform.evaluate_pre_positioning(&realised, at, horizon)?;
    // 1,000 euros needed and nothing positioned there: the whole need is the
    // shortfall, in dollars at 1.10.
    assert_eq!(score.shortfall, dec!("1100"), "{}", score.describe());
    Ok(())
}

/// A perpetual on `underlying`, priced at `price`, funding every
/// `interval_hours`.
fn perpetual(
    id: &str,
    underlying: &str,
    price: Decimal,
    interval_hours: u32,
) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::PerpetualFuture,
        liquidity(),
    )
    .venue("XNYS")
    .price(price)
    .underlying(ObjectId::from_string(underlying))
    .provenance(provenance())
    .extension(Extension::Perpetual(PerpetualDetails {
        underlying_object_id: underlying.to_string(),
        contract_size: dec!("2"),
        tick_size: dec!("0.01"),
        initial_margin: dec!("10"),
        maintenance_margin: dec!("8"),
        funding_interval_hours: interval_hours,
    }))
    .build(start())
}

fn coin(id: &str, price: Decimal) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::Cryptocurrency,
        liquidity(),
    )
    .venue("XNYS")
    .price(price)
    .provenance(provenance())
    .build(start())
}

/// R12-COVERAGE-5102, ADR 0395: a perpetual had no representation, and no
/// funding reached the centre. It is now its own instrument type with no
/// expiry; its funding is the premium of its own price over its
/// underlying's, per the interval its own record states; the cycle reports
/// it; and it is a derivative first, so no order exists until the desk holds
/// the `PerpetualFuture` permission.
///
/// Mutations that fired: taking the premium over the perpetual's own price
/// instead of the underlying's in `fund_perpetual` moves the rate off 0.01;
/// admitting a zero funding interval values the perpetual that never funds;
/// dropping the perpetual count from `FinancingRegister::summary` loses the
/// stage's report; making `Platform::derivative_gate` skip the per-type
/// permission lets a futures grant admit the perpetual; dropping the
/// `Derivative` record from `AssetClassRegistry::shipped` refuses assembly
/// ("equity is hedged with derivative, which this registry does not hold").
#[test]
fn a_perpetual_assembles_is_funded_from_its_premium_and_trades_on_paper_only_once_granted()
-> Result<()> {
    use qip_execution_engine::order::Side;
    use qip_financial::asset_class::AssetClass;
    use qip_financial::constraints::Jurisdiction;
    use qip_financial::derivative_permissions::DerivativePermissions;

    let perp = perpetual("obj-BTC-PERP", "obj-BTC", dec!("101"), 8)?;
    // Premise: the instrument is a derivative with no maturity, so what is
    // funded and filled below is a perpetual and not a dated future.
    assert_eq!(perp.asset_class, AssetClass::Derivative);
    assert_eq!(perp.extension.maturity(), None);
    let mut platform = assemble(vec![
        coin("obj-BTC", dec!("100"))?,
        perp,
        perpetual("obj-BTC-PERP-DISCOUNT", "obj-BTC", dec!("99"), 8)?,
        perpetual("obj-BTC-PERP-NEVER", "obj-BTC", dec!("101"), 0)?,
        perpetual("obj-GONE-PERP", "obj-ABSENT", dec!("101"), 8)?,
    ])?;

    let register = platform.financing_register();
    let funding = register
        .perpetual("obj-BTC-PERP")
        .unwrap_or_else(|| panic!("the perpetual was not funded: {:?}", register.problems()));
    assert_eq!(funding.underlying_object, "obj-BTC");
    assert_eq!((funding.mark, funding.index), (dec!("101"), dec!("100")));
    assert_eq!(funding.interval_hours, 8);
    // (101 − 100) / 100, and one long contract of two units pays 2.
    assert_eq!(funding.rate, dec!("0.01"));
    assert_eq!(funding.payment_per_contract, dec!("2"));
    // Below its underlying the sign turns: the long receives.
    let discount = register
        .perpetual("obj-BTC-PERP-DISCOUNT")
        .expect("the discounted perpetual was not funded");
    assert_eq!(discount.rate, dec!("-0.01"));
    assert_eq!(discount.payment_per_contract, dec!("-2"));
    // Neither refused perpetual is funded at an assumed interval or price.
    assert!(register.perpetual("obj-BTC-PERP-NEVER").is_none());
    assert!(register.perpetual("obj-GONE-PERP").is_none());

    let (detail, problems) = understand_problems(&mut platform);
    assert!(
        detail.contains("funds 2 perpetual(s) from the premium over the underlying, 2 unvalued"),
        "the UNDERSTAND stage does not report the funding: {detail}"
    );
    for (id, reason) in [
        ("obj-BTC-PERP-NEVER", "funding interval is zero hours"),
        (
            "obj-GONE-PERP",
            "underlying obj-ABSENT is not in the universe",
        ),
    ] {
        assert!(
            problems.iter().any(|problem| problem
                .starts_with(&format!("financing instrument {id} is unvalued:"))
                && problem.contains(reason)),
            "{id} did not reach the cycle report: {problems:?}"
        );
    }

    let order = |platform: &mut Platform| {
        platform.order_for(
            ObjectId::from_string("obj-BTC-PERP"),
            Side::Buy,
            dec!("10"),
            dec!("101"),
            "prop-perp",
            vec!["hyp-perp".to_string()],
            start(),
        )
    };
    let error = order(&mut platform).expect_err("an ungranted perpetual got an order");
    assert!(
        error.message().contains("obj-BTC-PERP"),
        "{}",
        error.message()
    );
    // A grant for dated futures is not a grant for perpetuals.
    let mut permissions = DerivativePermissions::none();
    permissions.grant("desk-a", Jurisdiction::UnitedStates, InstrumentType::Future)?;
    platform.authorise_derivatives("desk-a", Jurisdiction::UnitedStates, permissions.clone());
    order(&mut platform).expect_err("a futures grant admitted a perpetual");
    permissions.grant(
        "desk-a",
        Jurisdiction::UnitedStates,
        InstrumentType::PerpetualFuture,
    )?;
    platform.authorise_derivatives("desk-a", Jurisdiction::UnitedStates, permissions);

    // Premise: nothing had filled, so the fill counted is the perpetual's.
    assert_eq!(platform.orders().fills().len(), 0);
    let order = order(&mut platform)?;
    let order_id = order.order_id.clone();
    platform.submit_order(order, start())?;
    let fills = platform.orders().fills();
    assert!(
        fills.iter().any(|fill| fill.order_id == order_id),
        "the perpetual did not fill"
    );
    assert!(
        fills.iter().all(|fill| fill.simulated),
        "a perpetual fill was not stamped simulated"
    );
    Ok(())
}
