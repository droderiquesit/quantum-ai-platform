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
    BondDetails, Constituent, CouponFrequency, CreditRating, DayCount, Extension, FxDetails,
    IndexDetails, PerpetualDetails, RepoDetails, Seniority, StructuredProductDetails,
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
            near_settlement_date: None,
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
        detail.contains("funds 2 perpetual(s) from the premium over the underlying")
            && detail.contains(", 2 unvalued"),
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

fn held(object_id: &str, units: Decimal) -> Constituent {
    Constituent {
        object_id: object_id.to_string(),
        units,
    }
}

fn index(id: &str, divisor: Decimal, constituents: Vec<Constituent>) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::Index,
        liquidity(),
    )
    .venue("XNYS")
    .price(dec!("160"))
    .provenance(provenance())
    .extension(Extension::Index(IndexDetails {
        methodology: "price weighted".to_string(),
        constituent_count: u32::try_from(constituents.len()).unwrap_or(u32::MAX),
        rebalance_frequency: "quarterly".to_string(),
        divisor,
        is_total_return: false,
        constituents,
    }))
    .build(start())
}

fn basket(id: &str, constituents: Vec<Constituent>) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::SyntheticBasket,
        liquidity(),
    )
    .venue("XNYS")
    .price(dec!("300"))
    .provenance(provenance())
    .extension(Extension::Structured(StructuredProductDetails {
        payoff_description: "units of two names".to_string(),
        maturity: start().saturating_add(Duration::from_days(30)),
        principal_protected: false,
        participation_rate: 1.0,
        barrier_level: None,
        underlying_object_ids: constituents
            .iter()
            .map(|constituent| constituent.object_id.clone())
            .collect(),
        constituents,
        issuer: "paper".to_string(),
        greeks: None,
    }))
    .build(start())
}

/// R12-COVERAGE-5099, ADR 0397: an index held a count and a divisor and a
/// basket held ids, so nothing could value either from what it holds or say
/// what one unit is exposed to. Each now states its constituents and their
/// units; the register values it from them and the cycle reports it, and
/// one that states none, or names a member the universe lacks, is a problem
/// and not a guess.
///
/// Mutations that fired: ignoring the divisor in `value_composite` puts the
/// index at 320, not 160; taking the constituent's units without its price
/// puts the index at 1.5; valuing a record with no constituents at zero
/// loses the first refusal; skipping a missing member values the basket
/// around it.
#[test]
fn an_index_and_a_basket_are_valued_from_their_stated_constituents_and_one_that_states_none_is_refused()
-> Result<()> {
    let both = || vec![held("obj-AAA", dec!("2")), held("obj-BBB", dec!("1"))];
    let mut platform = assemble(vec![
        coin("obj-AAA", dec!("100"))?,
        coin("obj-BBB", dec!("120"))?,
        index("obj-IDX", dec!("2"), both())?,
        basket("obj-BASKET", both())?,
        index("obj-IDX-BARE", dec!("2"), Vec::new())?,
        basket(
            "obj-BASKET-GONE",
            vec![held("obj-AAA", dec!("1")), held("obj-ABSENT", dec!("1"))],
        )?,
    ])?;
    let register = platform.financing_register();
    let level = register
        .composite("obj-IDX")
        .unwrap_or_else(|| panic!("the index was not valued: {:?}", register.problems()));
    // (2 × 100 + 1 × 120) / 2.
    assert_eq!(level.value, dec!("160"));
    assert_eq!(level.quoted, dec!("160"));
    let whole = register
        .composite("obj-BASKET")
        .expect("the basket was not valued");
    // No divisor: one basket unit is its holdings.
    assert_eq!(whole.value, dec!("320"));
    assert_eq!(whole.quoted, dec!("300"));
    // The look-through of one unit, by member.
    assert_eq!(whole.exposure.get("obj-AAA"), Some(&dec!("200")));
    assert_eq!(whole.exposure.get("obj-BBB"), Some(&dec!("120")));
    assert_eq!(whole.exposure.len(), 2);
    assert!(register.composite("obj-IDX-BARE").is_none());
    assert!(register.composite("obj-BASKET-GONE").is_none());

    let (detail, problems) = understand_problems(&mut platform);
    assert!(
        detail.contains("values 2 index(es) and basket(s) from their constituents, 2 unvalued"),
        "the UNDERSTAND stage does not report the composites: {detail}"
    );
    for (id, reason) in [
        ("obj-IDX-BARE", "states no constituents"),
        (
            "obj-BASKET-GONE",
            "constituent obj-ABSENT is not in the universe",
        ),
    ] {
        assert!(
            problems.iter().any(|problem| problem
                .starts_with(&format!("financing instrument {id} is unvalued:"))
                && problem.contains(reason)),
            "{id} did not reach the cycle report: {problems:?}"
        );
    }
    Ok(())
}

fn reverse_repo(id: &str, collateral: &str, is_reverse: bool) -> Result<FinancialObject> {
    repo_at_rate(id, collateral, is_reverse, 0.05)
}

fn repo_at_rate(
    id: &str,
    collateral: &str,
    is_reverse: bool,
    repo_rate: f64,
) -> Result<FinancialObject> {
    repo_maturing(id, collateral, is_reverse, repo_rate, 30)
}

fn repo_maturing(
    id: &str,
    collateral: &str,
    is_reverse: bool,
    repo_rate: f64,
    days: i64,
) -> Result<FinancialObject> {
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
        maturity: start().saturating_add(Duration::from_days(days)),
        repo_rate,
        haircut: 0.02,
        is_reverse,
        counterparty: "dealer-a".into(),
    }))
    .build(start())
}

fn repo_desk_over(path: &std::path::Path, at: Timestamp) -> Result<Platform> {
    let mut universe = Universe::new();
    let mut objects = flat_curve("UST", Currency::USD, 0.05)?;
    objects.push(reverse_repo("obj-REVERSE", "obj-UST-10Y", true)?);
    objects.push(reverse_repo("obj-BORROW", "obj-UST-10Y", false)?);
    objects.push(repo_maturing("obj-LATER", "obj-UST-10Y", true, 0.05, 60)?);
    objects.push(repo_at_rate("obj-NEGATIVE", "obj-UST-10Y", true, -0.01)?);
    objects.push(repo_at_rate(
        "obj-UNNUMBERED",
        "obj-UST-10Y",
        true,
        f64::NAN,
    )?);
    for object in objects {
        universe.insert(object)?;
    }
    let config = PlatformConfig::default().with_event_log_file(path);
    let (context, _clock) = Context::deterministic(at, config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
}

/// A desk over a fresh event log of its own, removed when dropped.
struct RepoDesk {
    directory: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl RepoDesk {
    fn new(name: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "qip-kernel-financing-{name}-{}-{}",
            std::process::id(),
            start().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("events.jsonl");
        Self { directory, path }
    }

    fn open(&self, at: Timestamp) -> Result<Platform> {
        repo_desk_over(&self.path, at)
    }
}

impl Drop for RepoDesk {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Declare the treasury's repo permission for `counterparty`, the only way a
/// repo order is admitted (ADR 0398). The permission is the treasury's, in
/// the one registry DECIDE's funding reads too.
fn permit_repo(platform: &mut Platform, counterparty: &str) -> Result<()> {
    platform.declare_treasury(qip_kernel::treasury::TreasuryDeclaration {
        permissions: vec![(
            qip_treasury::treasury::FinancingFunction::Repo,
            counterparty.to_owned(),
        )],
        ..qip_kernel::treasury::TreasuryDeclaration::default()
    })
}

fn repo_order(
    platform: &mut Platform,
    id: &str,
    side: qip_execution_engine::order::Side,
    at: Timestamp,
) -> Result<qip_execution_engine::order::Order> {
    platform.order_for(
        ObjectId::from_string(id),
        side,
        dec!("10"),
        dec!("100"),
        "prop-repo",
        vec!["hyp-repo".to_string()],
        at,
    )
}

fn maturities(platform: &Platform) -> Vec<qip_contracts::ledger_operation::LedgerOperation> {
    platform
        .pending_desk_operations()
        .iter()
        .filter(|operation| {
            matches!(
                operation.kind,
                qip_contracts::ledger_operation::OperationKind::RepoMaturity { .. }
            )
        })
        .cloned()
        .collect()
}

fn lent(platform: &Platform) -> Decimal {
    platform
        .orders()
        .fills()
        .iter()
        .map(|fill| fill.quantity * fill.price)
        .fold(Decimal::ZERO, |sum, notional| sum + notional)
}

/// R12-COVERAGE-5100, ADR 0398. The first gate built for a repo held a
/// second permission registry in `Platform`, beside the treasury's, and was
/// reverted for it: two registries answer "may the platform finance with
/// this counterparty" twice, and the one an operator did not update answers
/// wrong. The failures this prevents: a repo order placed while the
/// treasury holds no repo permission, by either route to the venue; one
/// placed under a permission for another counterparty; and a gate that
/// refuses everything, which the last assertion rules out.
///
/// Mutations that fired, each restored byte-for-byte: `financing_gate`
/// dropped from `order_for` ("an undeclared treasury admitted a repo
/// order"); dropped from `submit_released` ("an order built around the gate
/// was submitted"); `Treasury::require_repo` answering `Ok(())` (the first
/// assertion); `FinancingPermissions::require_repo` asking for
/// `FinancingFunction::BorrowLend` (the first refusal names `BorrowLend`,
/// not `Repo`).
#[test]
fn a_repo_order_is_refused_unless_the_treasury_holds_the_repo_permission_for_its_own_counterparty()
-> Result<()> {
    use qip_execution_engine::order::Side;
    let desk = RepoDesk::new("permission");
    let mut platform = desk.open(start())?;
    assert!(
        platform.treasury().require_repo("dealer-a").is_err(),
        "premise: the assembled treasury permits nothing"
    );
    let refusal = repo_order(&mut platform, "obj-REVERSE", Side::Buy, start())
        .expect_err("an undeclared treasury admitted a repo order");
    assert!(
        refusal.message().contains("Repo is not permitted")
            && refusal.message().contains("dealer-a"),
        "{}",
        refusal.message()
    );
    let around = platform.order_from(
        ObjectId::from_string("obj-REVERSE"),
        Side::Buy,
        dec!("10"),
        dec!("100"),
        "prop-repo",
        vec!["hyp-repo".to_string()],
        start(),
    );
    platform
        .submit_order(around, start())
        .expect_err("an order built around the gate was submitted");

    permit_repo(&mut platform, "dealer-b")?;
    let refusal = repo_order(&mut platform, "obj-REVERSE", Side::Buy, start())
        .expect_err("a permission for another counterparty admitted the order");
    assert!(
        refusal.message().contains("dealer-a"),
        "{}",
        refusal.message()
    );
    assert_eq!(
        platform.orders().fills().len(),
        0,
        "premise: nothing filled"
    );

    permit_repo(&mut platform, "dealer-a")?;
    repo_order(&mut platform, "obj-REVERSE", Side::Buy, start())
        .expect("the permitted counterparty was refused");
    Ok(())
}

/// ADR 0398 decision 6. A repo the desk borrows under delivers collateral,
/// and ADR 0153 decision 3 books a pledge only from a venue that states one;
/// no simulated venue does. The failure this prevents: a borrowing admitted
/// because the permission is held, booking cash the desk received against
/// collateral no ledger operation says left.
///
/// Mutation that fired: the `is_reverse` refusal removed ("a borrowing repo
/// got an order under the permission").
#[test]
fn a_repo_the_desk_would_borrow_under_is_refused_even_with_the_permission() -> Result<()> {
    use qip_execution_engine::order::Side;
    let desk = RepoDesk::new("borrow");
    let mut platform = desk.open(start())?;
    permit_repo(&mut platform, "dealer-a")?;
    repo_order(&mut platform, "obj-REVERSE", Side::Buy, start())
        .expect("premise: the lending repo with the same counterparty is admitted");
    let refusal = repo_order(&mut platform, "obj-BORROW", Side::Buy, start())
        .expect_err("a borrowing repo got an order under the permission");
    assert!(
        refusal.message().contains("the desk borrows"),
        "{}",
        refusal.message()
    );
    Ok(())
}

/// ADR 0398 decision 6, by another name. Selling the lender's claim the book
/// does not hold leaves the desk short a repo, which is borrowing cash under
/// it. The failure this prevents: the borrowing arm reached through a sale.
///
/// Mutation that fired: the short refusal removed ("a sale of an unheld
/// repo got an order").
#[test]
fn selling_more_of_a_repo_than_the_book_holds_is_refused_as_a_borrowing() -> Result<()> {
    use qip_execution_engine::order::Side;
    let desk = RepoDesk::new("short");
    let mut platform = desk.open(start())?;
    permit_repo(&mut platform, "dealer-a")?;
    assert!(
        !platform.positions_at_cost().contains_key("obj-REVERSE"),
        "premise: the book holds none"
    );
    let refusal = repo_order(&mut platform, "obj-REVERSE", Side::Sell, start())
        .expect_err("a sale of an unheld repo got an order");
    assert!(refusal.message().contains("short"), "{}", refusal.message());
    Ok(())
}

/// ADR 0398 decision 8. The far leg repays principal and interest of zero or
/// more; a rate that is negative or not a number cannot be repaid on. The
/// failure this prevents: a repo lent under a rate its own maturity then
/// refuses, leaving a claim the book holds and the ledger can never close.
///
/// Mutation that fired: the rate refusal removed ("a negative rate got an
/// order", then the NaN arm).
#[test]
fn a_repo_whose_rate_is_negative_or_not_a_number_is_refused_an_order() -> Result<()> {
    use qip_execution_engine::order::Side;
    let desk = RepoDesk::new("rate");
    let mut platform = desk.open(start())?;
    permit_repo(&mut platform, "dealer-a")?;
    repo_order(&mut platform, "obj-REVERSE", Side::Buy, start())
        .expect("premise: a repo at a positive rate is admitted");
    for (id, why) in [
        ("obj-NEGATIVE", "a negative rate got an order"),
        ("obj-UNNUMBERED", "a rate that is not a number got an order"),
    ] {
        let refusal = repo_order(&mut platform, id, Side::Buy, start()).expect_err(why);
        assert!(
            refusal.message().contains("repo rate"),
            "{id}: {}",
            refusal.message()
        );
    }
    Ok(())
}

/// ADR 0398 decision 8. A matured repo is repaid, not traded. The failure
/// this prevents: cash lent at or after the instant it was due back, which
/// the far leg would repay at once with no term to earn over.
///
/// Mutation that fired: the maturity refusal removed ("a matured repo got
/// an order").
#[test]
fn a_repo_at_or_past_its_maturity_is_refused_an_order() -> Result<()> {
    use qip_execution_engine::order::Side;
    let maturity = start().saturating_add(Duration::from_days(30));
    let desk = RepoDesk::new("matured");
    let mut platform = desk.open(start())?;
    permit_repo(&mut platform, "dealer-a")?;
    repo_order(
        &mut platform,
        "obj-REVERSE",
        Side::Buy,
        maturity.saturating_sub(Duration::from_hours(1)),
    )
    .expect("premise: an hour before maturity the repo is admitted");
    let refusal = repo_order(&mut platform, "obj-REVERSE", Side::Buy, maturity)
        .expect_err("a matured repo got an order");
    assert!(
        refusal.message().contains("matured"),
        "{}",
        refusal.message()
    );
    Ok(())
}

/// ADR 0398 decision 2. The near leg is the fill: the cash lent is what the
/// fill paid and the claim is the unit the book holds. The failures this
/// prevents: a second operation restating the opening, which is a second
/// claim about one fact, and a repayment booked before the cash is due.
///
/// Mutation that fired: `settle_matured_repos` comparing `now >=
/// maturity.saturating_sub(two days)` ("repaid before maturity").
#[test]
fn a_permitted_reverse_repo_fills_as_its_near_leg_and_nothing_is_repaid_before_maturity()
-> Result<()> {
    use qip_execution_engine::order::Side;
    let maturity = start().saturating_add(Duration::from_days(30));
    let desk = RepoDesk::new("near");
    let mut platform = desk.open(start())?;
    permit_repo(&mut platform, "dealer-a")?;
    let cash_before = platform.cash();
    let lend = repo_order(&mut platform, "obj-REVERSE", Side::Buy, start())?;
    platform.submit_order(lend, start())?;
    let (held, _) = *platform
        .positions_at_cost()
        .get("obj-REVERSE")
        .expect("the venue did not fill the permitted repo");
    assert!(held.is_positive(), "the desk does not hold the claim");
    let lent = lent(&platform);
    assert!(lent.is_positive(), "premise: the fill lent cash");
    assert!(
        platform.cash() <= cash_before - lent,
        "the cash lent did not leave the book: {} then {}",
        cash_before,
        platform.cash()
    );
    assert!(maturities(&platform).is_empty(), "the fill was restated");

    platform.run_cycle(maturity.saturating_sub(Duration::from_days(1)));
    assert!(maturities(&platform).is_empty(), "repaid before maturity");
    assert!(platform.positions_at_cost().contains_key("obj-REVERSE"));
    Ok(())
}

/// ADR 0398 decisions 3 to 5. The far leg repays what the fill lent plus the
/// interest its own rate earns, once, and a restart neither loses it nor
/// books it again. The failures this prevents: a matured repo left in the
/// book; principal restated from anything but the fill; interest that is
/// not the record's rate over the days lent; the repayment booked twice; a
/// restart that takes the repayment back, repeats it, or queues no far leg;
/// and the book's cash disagreeing with the ledger's postings.
///
/// Mutations that fired, each restored byte-for-byte: `settle_matured_repos`
/// dropped from SENSE ("one far leg at maturity"); `repo_interest` answering
/// zero (the interest assertion); the `Entry::Maturity` fold skipped on
/// resume ("the restarted book still holds the repaid repo"); the re-queue
/// on resume dropped ("the restarted process did not re-queue");
/// `repo_maturity_operations` not resumed ("a far leg after a restart
/// reused a journalled sequence").
#[test]
fn a_held_repo_is_repaid_once_at_maturity_with_principal_and_interest_and_not_again_after_a_restart()
-> Result<()> {
    use qip_contracts::ledger_operation::OperationKind;
    use qip_execution_engine::order::Side;
    let maturity = start().saturating_add(Duration::from_days(30));
    let desk = RepoDesk::new("far");
    let (booked, cash_repaid) = {
        let mut first = desk.open(start())?;
        permit_repo(&mut first, "dealer-a")?;
        let lend = repo_order(&mut first, "obj-REVERSE", Side::Buy, start())?;
        first.submit_order(lend, start())?;
        // The near leg of obj-REVERSE is the first fill: what it lent.
        let lent = lent(&first);
        // A second repo, maturing after the restart, so the restarted
        // process must continue the far legs' sequence rather than reuse it.
        let later = repo_order(&mut first, "obj-LATER", Side::Buy, start())?;
        first.submit_order(later, start())?;
        assert!(
            first.positions_at_cost().contains_key("obj-LATER"),
            "premise: the second repo filled"
        );
        let (held, _) = *first
            .positions_at_cost()
            .get("obj-REVERSE")
            .expect("premise: the venue filled the repo");
        let fills_before = first.orders().fills().len();
        let cash_before = first.cash();
        first.run_cycle(maturity);
        assert_eq!(
            first.orders().fills().len(),
            fills_before,
            "premise: the maturity cycle filled nothing, so the cash moved is the repayment"
        );
        let repaid = maturities(&first);
        assert_eq!(repaid.len(), 1, "one far leg at maturity: {repaid:?}");
        let OperationKind::RepoMaturity {
            counterparty,
            repo,
            units,
            cash_unit,
            principal,
            interest,
            ..
        } = &repaid[0].kind
        else {
            unreachable!("filtered to repo maturities");
        };
        assert_eq!((repo.as_str(), *units), ("obj-REVERSE", held));
        assert_eq!(
            (counterparty.as_str(), cash_unit.as_str()),
            ("dealer-a", "USD")
        );
        // What was lent is what the fill paid, not a figure restated here.
        assert_eq!(*principal, lent);
        // 5% on the 365.25-day year over the whole days the cash was lent,
        // counted from the fill's own instant and not from the order's.
        let filled_at = first.orders().fills()[0].at;
        let days = maturity.since(filled_at).as_millis() / 86_400_000;
        assert!((29..=30).contains(&days), "premise: about a month, {days}");
        let expected = lent * dec!("0.05") * Decimal::from_int(days) / dec!("365.25");
        assert!(
            (*interest - expected).abs() < dec!("0.0001") && interest.is_positive(),
            "interest {interest} is not {days} days at 5% on {lent}: {expected}"
        );
        assert!(
            !first.positions_at_cost().contains_key("obj-REVERSE"),
            "the matured repo is still in the book"
        );
        // The book received exactly what the ledger's postings say.
        assert_eq!(first.cash(), cash_before + *principal + *interest);

        let after = maturity.saturating_add(Duration::from_hours(1));
        first.run_cycle(after);
        assert_eq!(maturities(&first), repaid, "the repayment was booked twice");
        (repaid, first.cash())
    };

    let later = maturity.saturating_add(Duration::from_hours(2));
    let mut second = desk.open(later)?;
    assert_eq!(
        maturities(&second),
        booked,
        "the restarted process did not re-queue the far leg the first journalled"
    );
    assert!(
        !second.positions_at_cost().contains_key("obj-REVERSE"),
        "the restarted book still holds the repaid repo"
    );
    assert_eq!(
        second.cash(),
        cash_repaid,
        "the restart moved the repaid cash"
    );
    second.run_cycle(later);
    assert_eq!(
        maturities(&second),
        booked,
        "the restart repaid the repo again"
    );
    second.run_cycle(start().saturating_add(Duration::from_days(60)));
    let both = maturities(&second);
    assert_eq!(both.len(), 2, "the later repo was not repaid: {both:?}");
    assert!(
        both[1].sequence > both[0].sequence,
        "a far leg after a restart reused a journalled sequence: {} then {}",
        both[0].sequence,
        both[1].sequence
    );
    Ok(())
}

/// A spot pair on `base`/`quote` at `price`.
fn spot_pair(id: &str, base: Currency, quote: Currency, price: Decimal) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::FxSpot,
        liquidity(),
    )
    .venue("OTC")
    .currency(quote)
    .price(price)
    .tick_size(dec!("0.00001"))
    .provenance(provenance())
    .extension(Extension::Fx(FxDetails {
        base_currency: base,
        quote_currency: quote,
        settlement_date: None,
        near_settlement_date: None,
        forward_points: 0.0,
        rate_differential: 0.0,
    }))
    .build(start())
}

/// An EUR/USD swap exchanging at `near` and reversing at `far`.
fn eurusd_swap(id: &str, near: Option<Timestamp>, far: Timestamp) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::FxSwap,
        liquidity(),
    )
    .venue("OTC")
    .currency(Currency::USD)
    .price(dec!("1.10"))
    .tick_size(dec!("0.00001"))
    .provenance(provenance())
    .underlying(ObjectId::from_string("obj-EURUSD"))
    .extension(Extension::Fx(FxDetails {
        base_currency: Currency::EUR,
        quote_currency: Currency::USD,
        settlement_date: Some(far),
        near_settlement_date: near,
        forward_points: 0.0,
        rate_differential: 0.0,
    }))
    .build(start())
}

/// Parity on the fixture's flat curves: spot × e^((r_usd − r_eur) × t).
fn parity(days: f64) -> f64 {
    1.10 * ((0.05 - 0.03) * days / 365.25_f64).exp()
}

/// R12-COVERAGE-5098 (2): a swap is two exchanges, and the register values
/// both, where `FxDetails` once carried one date and a swap could only have
/// been read as a forward on its far leg — the points, which are the swap's
/// whole economics, dropped.
///
/// Mutation that fired: valuing the near leg on the far date in
/// `FinancingRegister::value_swap` makes the two legs one, and the tenor
/// assertion fails (`left: (120, 120)`, right `(30, 120)`).
#[test]
fn an_fx_swap_is_valued_as_its_two_legs_and_one_stating_a_single_date_is_refused() -> Result<()> {
    let mut objects = flat_curve("UST", Currency::USD, 0.05)?;
    objects.extend(flat_curve("BUND", Currency::EUR, 0.03)?);
    objects.push(eurusd("obj-EURUSD", dec!("1.10"), None)?);
    let day = |days: i64| start().saturating_add(Duration::from_days(days));
    objects.push(eurusd_swap("obj-SWAP-1M3M", Some(day(30)), day(120))?);
    // Near leg inside a day of the valuation instant: a spot leg.
    objects.push(eurusd_swap(
        "obj-SWAP-SPOT3M",
        Some(start().saturating_add(Duration::from_secs(3_600))),
        day(90),
    )?);
    objects.push(eurusd_swap("obj-SWAP-ONE-DATE", None, day(90))?);
    objects.push(eurusd_swap("obj-SWAP-BACKWARDS", Some(day(90)), day(30))?);
    let mut platform = assemble(objects)?;
    let register = platform.financing_register();

    let swap = register
        .swap("obj-SWAP-1M3M")
        .unwrap_or_else(|| panic!("the swap was not valued: {:?}", register.problems()));
    assert_eq!((swap.near_days, swap.far.days), (30, 120));
    assert_eq!(swap.spot_object, "obj-EURUSD");
    let near = swap.near_outright.to_f64();
    assert!(
        (near - parity(30.0)).abs() < 1e-5,
        "near outright {near} is not the 30-day parity figure {}",
        parity(30.0)
    );
    let far = swap.far.outright.to_f64();
    assert!(
        (far - parity(120.0)).abs() < 1e-5,
        "far outright {far} is not the 120-day parity figure {}",
        parity(120.0)
    );
    assert_eq!(
        swap.swap_points,
        swap.far.outright - swap.near_outright,
        "the points are the far leg less the near"
    );
    assert!(swap.swap_points.is_positive(), "{swap:?}");

    let spot_start = register.swap("obj-SWAP-SPOT3M").unwrap_or_else(|| {
        panic!(
            "the spot-start swap was not valued: {:?}",
            register.problems()
        )
    });
    assert_eq!(spot_start.near_days, 0);
    assert_eq!(spot_start.near_outright, dec!("1.10"));

    assert!(register.swap("obj-SWAP-ONE-DATE").is_none());
    assert!(register.swap("obj-SWAP-BACKWARDS").is_none());
    let (detail, problems) = understand_problems(&mut platform);
    assert!(detail.contains("and 2 FX swap(s)"), "{detail}");
    let unvalued = |id: &str, reason: &str| {
        problems.iter().any(|problem| {
            problem.starts_with(&format!("financing instrument {id} is unvalued:"))
                && problem.contains(reason)
        })
    };
    assert!(
        unvalued("obj-SWAP-ONE-DATE", "states no near leg"),
        "{problems:?}"
    );
    assert!(
        unvalued("obj-SWAP-BACKWARDS", "does not settle after its near leg"),
        "{problems:?}"
    );
    Ok(())
}

/// R12-COVERAGE-5098 (3): a currency with no pair against the base is priced
/// through the one pair that joins it to a directly quoted currency, and a
/// currency two pairs reach stays unpriced, because two routes are two
/// figures for one rate.
///
/// Mutations that fired: skipping the cross pass in
/// `FinancingRegister::fx_rates` leaves sterling unpriced ("no rate for GBP
/// against the USD base"); taking the first of several routes (`routes.first()`) prices the
/// franc and the refusal assertion fails ("a currency two routes reach was
/// priced").
#[test]
fn a_cross_is_priced_through_the_one_pair_that_reaches_it_and_two_routes_leave_it_unpriced()
-> Result<()> {
    let mut objects = flat_curve("UST", Currency::USD, 0.05)?;
    objects.push(spot_pair(
        "obj-EURUSD",
        Currency::EUR,
        Currency::USD,
        dec!("1.10"),
    )?);
    objects.push(spot_pair(
        "obj-AUDUSD",
        Currency::AUD,
        Currency::USD,
        dec!("0.65"),
    )?);
    objects.push(spot_pair(
        "obj-EURGBP",
        Currency::EUR,
        Currency::GBP,
        dec!("0.85"),
    )?);
    objects.push(spot_pair(
        "obj-EURCHF",
        Currency::EUR,
        Currency::CHF,
        dec!("0.95"),
    )?);
    objects.push(spot_pair(
        "obj-AUDCHF",
        Currency::AUD,
        Currency::CHF,
        dec!("0.58"),
    )?);
    let platform = assemble(objects)?;
    let rates = platform.financing_register().fx_rates(Currency::USD)?;

    // Premise: the two legs of the cross are each quoted directly.
    assert_eq!(rates.to_base(dec!("1"), Currency::EUR)?, dec!("1.10"));
    assert_eq!(rates.to_base(dec!("1"), Currency::AUD)?, dec!("0.65"));
    // One pound is 1/0.85 euros, each 1.10 dollars.
    let pound = rates.to_base(dec!("1"), Currency::GBP)?.to_f64();
    assert!(
        (pound - 1.10 / 0.85).abs() < 1e-8,
        "sterling {pound} is not 1.10 / 0.85"
    );
    let franc = rates.to_base(dec!("1"), Currency::CHF);
    assert!(
        franc.is_err(),
        "a currency two routes reach was priced: {franc:?}"
    );
    Ok(())
}
