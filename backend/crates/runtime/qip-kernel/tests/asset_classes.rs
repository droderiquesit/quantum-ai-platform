//! Blueprint §17.7's gate, from both sides: a universe of registered classes
//! assembles, and one holding a class the platform is not registered to trade
//! stops assembly naming the class.
//!
//! The second half without the first is a test that would pass against a
//! platform that refused every universe, which is not a gate. Both halves are
//! here for the reason the infrastructure domain's rules give about a
//! validation change: what distinguishes a working gate from one that refuses
//! everything is that it admits a good value.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Context, ObjectId, Timestamp, dec};
use qip_financial::asset_class::{AssetClass, InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn liquidity() -> LiquidityProfile {
    LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

/// One instrument of the given type, with the platform's default grid unless
/// `tick` says otherwise.
fn object(
    id: &str,
    kind: InstrumentType,
    tick: Option<qip_core::Decimal>,
) -> Result<FinancialObject> {
    let mut builder = FinancialObject::builder(ObjectId::from_string(id), id, kind, liquidity())
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("asset-class-test", start()));
    if let Some(tick) = tick {
        builder = builder.tick_size(tick);
    }
    builder.build(start())
}

/// A future on `underlying`, carrying the underlying and expiry the object
/// model demands of a derivative.
fn future(id: &str, underlying: &str) -> Result<FinancialObject> {
    use qip_financial::extensions::{Extension, FutureDetails, SettlementStyle};
    let expiry = start().saturating_add(qip_core::time::Duration::from_days(90));
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::Future,
        liquidity(),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .underlying(ObjectId::from_string(underlying))
    .extension(Extension::Future(FutureDetails {
        underlying_object_id: underlying.to_string(),
        expiry,
        contract_size: dec!("1"),
        tick_size: dec!("0.01"),
        tick_value: dec!("0.01"),
        initial_margin: dec!("10"),
        maintenance_margin: dec!("8"),
        settlement: SettlementStyle::Cash,
        contract_month_index: 1,
    }))
    .provenance(Provenance::synthetic("asset-class-test", start()))
    .build(start())
}

fn universe_of(objects: Vec<FinancialObject>) -> Result<Universe> {
    let mut universe = Universe::new();
    for object in objects {
        universe.insert(object)?;
    }
    Ok(universe)
}

fn assemble(universe: Universe) -> Result<Platform> {
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

#[test]
fn a_universe_of_registered_classes_assembles_and_one_holding_an_unregistered_class_does_not()
-> Result<()> {
    // The admitting half first, and it is the half that makes the refusal
    // below mean something: a common share is an `Equity`, the registry
    // holds `Equity`, and the platform assembles over it exactly as it did
    // before §17.7's gate existed.
    let good = assemble(universe_of(vec![object(
        "obj-AAA",
        InstrumentType::CommonStock,
        None,
    )?])?)?;
    assert!(
        good.asset_class_registry()
            .is_registered(AssetClass::Equity),
        "premise: the registry holds the class the good universe is in"
    );

    // And the refusing half. Every `AssetClass` variant is registered since
    // R12-COVERAGE-5098/-5103, so the shipped table no longer leaves a class
    // out to refuse with; this used `DigitalAsset`, which is now supported.
    // The refusal is proven on the table the platform assembles against with
    // one record withdrawn, through the same `admit` `Platform::new` calls on
    // every instrument — and the message an operator reads is unchanged.
    let coin = object("obj-BTC", InstrumentType::Cryptocurrency, None)?;
    let narrowed = good
        .asset_class_registry()
        .clone()
        .without(AssetClass::DigitalAsset)?;
    assert!(
        !narrowed.is_registered(AssetClass::DigitalAsset),
        "premise: the narrowed registry does not hold DigitalAsset, or this proves nothing"
    );
    let error = narrowed.admit(&coin).err().ok_or_else(|| {
        qip_core::Error::invalid("an instrument in an unregistered class was admitted")
    })?;
    // The class named in its own delimited spelling and attached to the
    // instrument, rather than a bare `contains("digital_asset")` that the
    // remedy sentence would satisfy on its own. The object id and the remedy
    // are what an operator needs: a message naming only the count would send
    // them to read the catalogue by hand.
    let message = error.message();
    assert!(message.contains("obj-BTC"), "{message}");
    assert!(
        message.contains("is a digital_asset and no record registers"),
        "{message}"
    );
    assert!(
        message.contains("valuation engine")
            && message.contains("settlement convention")
            && message.contains("eligible strategy family"),
        "the refusal must name what registering a class requires: {message}"
    );
    Ok(())
}

/// R12-COVERAGE-5098 and -5103: an FX spot, a cash equivalent and a crypto
/// spot assemble into the platform beside an equity, and an FX and a crypto
/// order go through the full desk control path and fill at the simulated
/// venue — paper, by the broker's own stamp.
///
/// Mutation: delete the `ForeignExchange` (or `DigitalAsset`, or `Cash`)
/// record from `AssetClassRegistry::shipped` and `assemble` refuses the
/// universe naming the instrument, so this test fails at its first `?`.
#[test]
fn fx_cash_and_digital_asset_instruments_assemble_and_trade_on_paper() -> Result<()> {
    let mut platform = assemble(universe_of(vec![
        object("obj-AAA", InstrumentType::CommonStock, None)?,
        object("obj-EURUSD", InstrumentType::FxSpot, Some(dec!("0.00001")))?,
        // `Cash` rather than `MoneyMarketInstrument`: the object model has
        // always refused a money-market instrument without a maturity (it is
        // a dated claim, `InstrumentType::has_maturity`), and this fixture
        // gave it none, so the test died at `build` before reaching the
        // registry it exists to prove. Plain cash is the same `Cash` class
        // with no redemption date to invent.
        object("obj-USD-CASH", InstrumentType::Cash, None)?,
        object("obj-BTC", InstrumentType::Cryptocurrency, None)?,
    ])?)?;
    for class in [
        AssetClass::ForeignExchange,
        AssetClass::Cash,
        AssetClass::DigitalAsset,
    ] {
        assert!(
            platform.asset_class_registry().is_registered(class),
            "{} is not registered",
            class.as_str()
        );
    }
    let fills_before = platform.orders().fills().len();
    for id in ["obj-EURUSD", "obj-BTC"] {
        let order = platform.order_for(
            ObjectId::from_string(id),
            qip_execution_engine::order::Side::Buy,
            dec!("10"),
            dec!("100"),
            "prop-coverage",
            vec!["hyp-coverage".to_string()],
            start(),
        )?;
        platform.submit_order(order, start())?;
    }
    let fills = platform.orders().fills();
    // Premise: nothing had filled before, so the fills counted are these.
    assert_eq!(fills_before, 0);
    // A fill names its order, not its instrument; the instrument is read off
    // the order the fill belongs to.
    let filled: Vec<&str> = fills
        .iter()
        .filter_map(|fill| {
            platform
                .orders()
                .orders()
                .find(|order| order.order_id == fill.order_id)
                .map(|order| order.object_id.as_str())
        })
        .collect();
    assert!(filled.contains(&"obj-EURUSD"), "{filled:?}");
    assert!(filled.contains(&"obj-BTC"), "{filled:?}");
    assert!(
        fills.iter().all(|fill| fill.simulated),
        "a fill on the coverage path was not stamped simulated"
    );
    Ok(())
}

#[test]
fn an_instrument_quoted_finer_than_its_class_expresses_is_refused_at_assembly() -> Result<()> {
    // The direction that matters. A reference record claiming a tick finer
    // than the class trades in tells §18.1's feasibility gate that a price is
    // expressible which no venue would accept, so the platform sizes to a
    // grid the market refuses and discovers it at the book.
    //
    // Premise: the same instrument at the catalogue's own tick assembles, so
    // the refusal below is about the grid and not about the instrument.
    assert!(
        assemble(universe_of(vec![object(
            "obj-AAA",
            InstrumentType::CommonStock,
            Some(dec!("0.01")),
        )?])?)
        .is_ok(),
        "premise: a hundredth-tick equity is admitted"
    );
    let error = assemble(universe_of(vec![object(
        "obj-AAA",
        InstrumentType::CommonStock,
        Some(dec!("0.00000001")),
    )?])?)
    .err()
    .ok_or_else(|| qip_core::Error::invalid("an equity quoted in hundred-millionths assembled"))?;
    let message = error.message();
    assert!(message.contains("finer than the"), "{message}");
    assert!(message.contains("obj-AAA"), "{message}");
    Ok(())
}

#[test]
fn physical_products_are_a_registered_coverage_class_whose_support_model_spans_commerce_and_inventory()
-> Result<()> {
    // COMMERCE-021: Physical products must be a supported coverage class whose
    // support model spans SKU-level commerce, auctions, collectibles,
    // wholesale/retail arbitrage and inventory — all spanning COMMERCE-008 to
    // -020 components, not just the IlliquidValuation engine.

    let platform = assemble(universe_of(vec![])?)?;
    let registry = platform.asset_class_registry();

    // PhysicalProduct is registered.
    assert!(
        registry.is_registered(AssetClass::PhysicalProduct),
        "PhysicalProduct class must be registered to be supported"
    );

    // The class is registered with IlliquidValuation, periodic settlement and
    // negotiated terms — appropriate for goods moving through purchase, customs,
    // logistics, resale and settlement.
    let record = registry
        .get(AssetClass::PhysicalProduct)
        .expect("PhysicalProduct is registered");
    assert_eq!(
        record.valuation_engine(),
        qip_kernel::asset_class_registry::ValuationEngine::IlliquidValuation,
        "Physical products are illiquid and use IlliquidValuation"
    );
    assert!(
        matches!(
            record.settlement(),
            qip_kernel::asset_class_registry::ClassSettlement::PeriodicStatement
        ),
        "Physical products settle periodically as purchase → logistics → resale → settlement"
    );
    assert!(
        matches!(
            record.grid(),
            qip_kernel::asset_class_registry::GridRule::Negotiated
        ),
        "Physical products are negotiated, not listed"
    );
    assert!(
        matches!(
            record.calendar(),
            qip_kernel::asset_class_registry::TradingCalendar::Negotiated
        ),
        "Physical products trade by appointment, not on a schedule"
    );

    // Carry family is eligible — physical products generate carry costs
    // (storage, insurance) and financing.
    assert!(
        record.admits_family(qip_optimization_engine::universe::AlphaFamily::Carry),
        "Carry family must be eligible for physical product inventory"
    );

    // The class names itself: Commodity hedges physical products (raw materials,
    // agricultural goods). This relationship is asymmetric because Commodity
    // positions are liquid and PhysicalProduct positions are not.
    assert!(
        record.hedge_classes().contains(&AssetClass::Commodity),
        "PhysicalProduct can be hedged with Commodity exposure"
    );

    Ok(())
}

#[test]
fn a_derivative_order_is_refused_before_it_exists_until_the_desk_is_granted_that_type() -> Result<()>
{
    use qip_execution_engine::order::Side;
    use qip_financial::constraints::Jurisdiction;
    use qip_financial::derivative_permissions::DerivativePermissions;

    let mut platform = assemble(universe_of(vec![
        object("obj-AAA", InstrumentType::CommonStock, None)?,
        future("obj-VAR", "obj-AAA")?,
    ])?)?;
    let order = |platform: &mut Platform, id: &str| {
        platform.order_for(
            ObjectId::from_string(id),
            Side::Buy,
            dec!("10"),
            dec!("100"),
            "prop-1",
            vec!["hyp-1".to_string()],
            start(),
        )
    };
    // Premise: the platform assembled over the derivative, so research and
    // simulation can hold it, and the equity is orderable; the refusals below
    // are the permission and not a missing instrument.
    assert!(
        platform
            .asset_class_registry()
            .is_registered(AssetClass::Derivative)
    );
    assert!(order(&mut platform, "obj-AAA").is_ok());

    // No permissions configured: refused.
    let error = order(&mut platform, "obj-VAR").expect_err("an ungranted derivative got an order");
    assert!(error.message().contains("obj-VAR"), "{}", error.message());

    // Granted to another entity than the desk: refused.
    let mut permissions = DerivativePermissions::none();
    permissions.grant("desk-a", Jurisdiction::UnitedStates, InstrumentType::Future)?;
    platform.authorise_derivatives("desk-b", Jurisdiction::UnitedStates, permissions.clone());
    assert!(order(&mut platform, "obj-VAR").is_err());

    // Granted to this desk: the same order is admitted.
    platform.authorise_derivatives("desk-a", Jurisdiction::UnitedStates, permissions);
    assert!(order(&mut platform, "obj-VAR").is_ok());
    Ok(())
}

/// A repo, an index and a synthetic basket, each with the extension the object
/// model demands of its type.
fn financing_and_composite_instruments() -> Result<Vec<FinancialObject>> {
    use qip_financial::extensions::{
        Extension, IndexDetails, RepoDetails, StructuredProductDetails,
    };
    let maturity = start().saturating_add(qip_core::time::Duration::from_days(30));
    let repo = FinancialObject::builder(
        ObjectId::from_string("obj-REPO-1M"),
        "obj-REPO-1M",
        InstrumentType::Repo,
        liquidity(),
    )
    .venue("XNYS")
    .sector(Sector::Financials)
    .price(dec!("100"))
    .extension(Extension::Repo(RepoDetails {
        collateral_object: "obj-AAA".to_string(),
        maturity,
        repo_rate: 0.045,
        haircut: 0.02,
        is_reverse: false,
        counterparty: "cp-1".to_string(),
    }))
    .provenance(Provenance::synthetic("asset-class-test", start()))
    .build(start())?;
    let index = FinancialObject::builder(
        ObjectId::from_string("obj-IDX"),
        "obj-IDX",
        InstrumentType::Index,
        liquidity(),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .extension(Extension::Index(IndexDetails {
        methodology: "price weighted".to_string(),
        constituent_count: 2,
        rebalance_frequency: "quarterly".to_string(),
        divisor: dec!("2"),
        is_total_return: false,
    }))
    .provenance(Provenance::synthetic("asset-class-test", start()))
    .build(start())?;
    let basket = FinancialObject::builder(
        ObjectId::from_string("obj-BASKET"),
        "obj-BASKET",
        InstrumentType::SyntheticBasket,
        liquidity(),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .extension(Extension::Structured(StructuredProductDetails {
        payoff_description: "equal weight of two names".to_string(),
        maturity,
        principal_protected: false,
        participation_rate: 1.0,
        barrier_level: None,
        underlying_object_ids: vec!["obj-AAA".to_string(), "obj-BBB".to_string()],
        issuer: "paper".to_string(),
        greeks: None,
    }))
    .provenance(Provenance::synthetic("asset-class-test", start()))
    .build(start())?;
    Ok(vec![repo, index, basket])
}

/// R12-COVERAGE-5099 and -5100: a repo (Rates), an index (Equity) and a
/// synthetic basket (StructuredProduct) assemble into the platform beside an
/// equity, an order in each goes through the desk control path and fills at
/// the simulated venue, and withdrawing the class record refuses the
/// instrument naming it.
///
/// What this proves and what it does not: these instruments are *tradable
/// records* on paper. It does not value an index from constituents, settle a
/// repo's legs, or post a corporate action to the ledger; the register rows
/// say so.
///
/// Mutation: delete the `Rates` (or `Equity`, or `StructuredProduct`) record
/// from `AssetClassRegistry::shipped` and `assemble` refuses the universe at
/// its first `?`; make `admit` return `Ok` and the narrowed-registry refusal
/// below fails.
#[test]
fn a_repo_an_index_and_a_basket_assemble_trade_on_paper_and_are_refused_without_their_class()
-> Result<()> {
    let mut objects = vec![object("obj-AAA", InstrumentType::CommonStock, None)?];
    objects.extend(financing_and_composite_instruments()?);
    let mut platform = assemble(universe_of(objects)?)?;
    for class in [
        AssetClass::Rates,
        AssetClass::Equity,
        AssetClass::StructuredProduct,
    ] {
        assert!(
            platform.asset_class_registry().is_registered(class),
            "premise: {} is registered",
            class.as_str()
        );
    }
    assert_eq!(
        platform.orders().fills().len(),
        0,
        "premise: nothing filled"
    );
    for id in ["obj-REPO-1M", "obj-IDX", "obj-BASKET"] {
        let order = platform.order_for(
            ObjectId::from_string(id),
            qip_execution_engine::order::Side::Buy,
            dec!("10"),
            dec!("100"),
            "prop-coverage",
            vec!["hyp-coverage".to_string()],
            start(),
        )?;
        platform.submit_order(order, start())?;
    }
    let fills = platform.orders().fills();
    let filled: Vec<&str> = fills
        .iter()
        .filter_map(|fill| {
            platform
                .orders()
                .orders()
                .find(|order| order.order_id == fill.order_id)
                .map(|order| order.object_id.as_str())
        })
        .collect();
    for id in ["obj-REPO-1M", "obj-IDX", "obj-BASKET"] {
        assert!(filled.contains(&id), "{id} did not fill: {filled:?}");
    }
    assert!(
        fills.iter().all(|fill| fill.simulated),
        "a fill on the coverage path was not stamped simulated"
    );

    // And the refusing half, per class, on the table the platform assembled
    // against with that one record withdrawn. A class another record hedges
    // with cannot be withdrawn at all (`without` refuses, naming the hedge
    // map), which is a stronger guard than the instrument's refusal; the
    // test requires one of the two for every class and records which.
    let mut refused_at_instrument = 0;
    for (class, id) in [
        (AssetClass::Rates, "obj-REPO-1M"),
        (AssetClass::Equity, "obj-IDX"),
        (AssetClass::StructuredProduct, "obj-BASKET"),
    ] {
        match platform.asset_class_registry().clone().without(class) {
            Err(error) => assert!(
                error.message().contains(class.as_str()) && error.message().contains("hedged with"),
                "withdrawing {} was refused for another reason: {}",
                class.as_str(),
                error.message()
            ),
            Ok(narrowed) => {
                let instrument = financing_and_composite_instruments()?
                    .into_iter()
                    .find(|o| o.object_id.as_str() == id)
                    .expect("fixture holds it");
                let message = narrowed
                    .admit(&instrument)
                    .err()
                    .ok_or_else(|| qip_core::Error::invalid("an unregistered class was admitted"))?
                    .message()
                    .to_string();
                assert!(
                    message.contains(id)
                        && message.contains(&format!("is a {} and no record", class.as_str())),
                    "{message}"
                );
                refused_at_instrument += 1;
            }
        }
    }
    assert!(
        refused_at_instrument >= 1,
        "no class could be withdrawn, so the instrument-level refusal was never exercised"
    );
    Ok(())
}

/// R12-COVERAGE-5105: private and illiquid holdings are registered classes.
/// A private equity fund and a real-estate holding each assemble beside a
/// listed name and trade on paper, and the fund's commitment reaches the
/// commitment book. Withdraw either class's record, and its instrument is
/// refused by name.
///
/// The class-refusal test above withdraws `DigitalAsset`, so nothing proved
/// the private class's own record was what admitted a fund. Without that
/// proof, a registry that admitted private funds for some other reason
/// would read as supporting them.
///
/// Mutation: delete the `PrivateMarket` record from
/// `AssetClassRegistry::shipped`. `assemble` then refuses the universe,
/// naming the fund, and this test fails at its first `?`.
#[test]
fn a_private_fund_and_a_real_asset_assemble_and_trade_on_paper_and_withdrawing_either_class_refuses_it()
-> Result<()> {
    use qip_financial::extensions::{Extension, PrivateAssetDetails};
    let fund = || {
        FinancialObject::builder(
            ObjectId::from_string("obj-FUND"),
            "obj-FUND",
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
            distributed_capital: qip_core::Decimal::ZERO,
            residual_value: dec!("160000"),
            stage: "buyout".to_string(),
            lockup_years: 7.0,
            capital_call_notice_days: 10,
            call_schedule: None,
        }))
        .provenance(Provenance::synthetic("administrator", start()))
        .build(start())
    };
    let property = || {
        FinancialObject::builder(
            ObjectId::from_string("obj-PROPERTY"),
            "obj-PROPERTY",
            InstrumentType::RealEstate,
            LiquidityProfile::illiquid(180.0, 200.0),
        )
        .venue("OTC")
        .geography("US")
        .price(dec!("100"))
        .provenance(Provenance::synthetic("appraiser", start()))
        .build(start())
    };
    // A private-holdings mandate: leverage capped and no five-day
    // liquidity floor, which a private fund or a building cannot meet by
    // construction — the conservative default refuses both orders under
    // `liquidity`, as it should for a desk that never agreed to hold them.
    // The limit set is the one `tests/capital_calls.rs` books its fund under.
    let mandate = LimitSet::new("private-holdings").with(
        qip_risk::limits::Limit::new(
            "max-leverage",
            qip_risk::limits::LimitKind::MaxLeverage { limit: 2.0 },
        )
        .with_rationale("gross exposure is capped at 2x equity"),
    );
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe_of(vec![
            object("obj-AAA", InstrumentType::CommonStock, None)?,
            fund()?,
            property()?,
        ])?,
        mandate,
    )?;
    assert!(
        platform
            .asset_class_registry()
            .is_registered(AssetClass::PrivateMarket),
        "premise: the shipped registry holds the private-market class"
    );
    // On paper: the fund's undrawn commitment, 400,000 committed less
    // 150,000 called, is what the platform's commitment book carries.
    assert_eq!(
        platform.commitments().unfunded_total(start())?,
        dec!("250000"),
        "the fund assembled but its commitment never reached the book"
    );

    // Each trades on paper: an order goes through the desk's whole control
    // path and fills at the simulated venue, by the broker's own stamp.
    assert_eq!(
        platform.orders().fills().len(),
        0,
        "premise: nothing filled"
    );
    for id in ["obj-FUND", "obj-PROPERTY"] {
        let order = platform.order_for(
            ObjectId::from_string(id),
            qip_execution_engine::order::Side::Buy,
            dec!("1"),
            dec!("100"),
            "prop-coverage",
            vec!["hyp-coverage".to_string()],
            start(),
        )?;
        platform.submit_order(order, start())?;
    }
    let fills = platform.orders().fills();
    for id in ["obj-FUND", "obj-PROPERTY"] {
        let filled = fills.iter().any(|fill| {
            fill.simulated
                && platform
                    .orders()
                    .orders()
                    .any(|order| order.order_id == fill.order_id && order.object_id.as_str() == id)
        });
        assert!(filled, "{id} did not fill on paper: {fills:?}");
    }

    // And the refusing half, on the table the platform assembled against
    // with each class's record withdrawn.
    // Real assets are what commodities hedge with, so the registry refuses
    // to withdraw the record at all, naming the hedge map: a stronger guard
    // than the instrument's own refusal, and the one the sibling test above
    // accepts for the classes it withdraws.
    match platform
        .asset_class_registry()
        .clone()
        .without(AssetClass::RealAsset)
    {
        Err(error) => assert!(
            error.message().contains("real_asset") && error.message().contains("hedged with"),
            "withdrawing real_asset was refused for another reason: {}",
            error.message()
        ),
        Ok(estate) => {
            let message = estate
                .admit(&property()?)
                .err()
                .ok_or_else(|| {
                    qip_core::Error::invalid("a real asset was admitted without its class")
                })?
                .message()
                .to_string();
            assert!(
                message.contains("obj-PROPERTY")
                    && message.contains("is a real_asset and no record registers"),
                "{message}"
            );
        }
    }
    let narrowed = platform
        .asset_class_registry()
        .clone()
        .without(AssetClass::PrivateMarket)?;
    assert!(
        !narrowed.is_registered(AssetClass::PrivateMarket),
        "premise: the narrowed registry does not hold PrivateMarket"
    );
    let message = narrowed
        .admit(&fund()?)
        .err()
        .ok_or_else(|| qip_core::Error::invalid("a private fund was admitted without its class"))?
        .message()
        .to_string();
    assert!(
        message.contains("obj-FUND")
            && message.contains("is a private_market and no record registers"),
        "{message}"
    );
    Ok(())
}
