//! The universal financial object model, exercised across asset classes.

// Exact float comparison is deliberate: these assert that a missing or
// conflicting value yields exactly zero, not merely something close to it.
#![allow(clippy::float_cmp)]

use qip_core::testing::approx_eq;
use qip_core::{Context, Currency, Decimal, ObjectId, Timestamp, dec};
use qip_financial::asset_class::{AssetClass, InstrumentType, Sector};
use qip_financial::constraints::{Jurisdiction, RegulatoryConstraints, TradingRestriction};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_financial::extensions::*;
use qip_financial::identifiers::{IdentifierKind, Identifiers};
use qip_financial::intelligence::MacroObservation;
use qip_financial::object::{FinancialObject, TradeSide};
use qip_financial::quality::{DECISION_QUALITY_FLOOR, DataQuality, LicensingClass, Provenance};
use qip_financial::risk_profile::{FactorExposures, Greeks, RiskCharacteristics, factors};
use qip_financial::universe::Universe;

fn ctx() -> (Context, Timestamp) {
    let now = Timestamp::from_civil(2026, 8, 22);
    let (ctx, _clock) = Context::deterministic(now, 42);
    (ctx, now)
}

fn equity(ctx: &Context, now: Timestamp, symbol: &str, price: &str) -> FinancialObject {
    FinancialObject::builder(ctx.ids().generate(now), symbol, InstrumentType::CommonStock)
        .name(format!("{symbol} Inc"))
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(Decimal::parse(price).unwrap())
        .liquidity(LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0))
        .transaction_costs(TransactionCostModel::listed(3.0))
        .provenance(Provenance::synthetic("test", now))
        .build(now)
        .expect("valid equity")
}

// --- taxonomy ---------------------------------------------------------------

#[test]
fn every_instrument_type_maps_to_exactly_one_asset_class() {
    // A round trip through the object model must not lose classification.
    for instrument in [
        InstrumentType::CommonStock,
        InstrumentType::GovernmentBond,
        InstrumentType::CreditDefaultSwap,
        InstrumentType::InterestRateSwap,
        InstrumentType::FxForward,
        InstrumentType::CommodityFuture,
        InstrumentType::Option,
        InstrumentType::Cryptocurrency,
        InstrumentType::HedgeFund,
        InstrumentType::PrivateEquityFund,
        InstrumentType::RealEstate,
        InstrumentType::Cash,
        InstrumentType::StructuredNote,
    ] {
        let class = instrument.asset_class();
        assert!(
            AssetClass::ALL.contains(&class),
            "{instrument} mapped outside the taxonomy"
        );
    }
}

#[test]
fn derivative_and_maturity_predicates_agree_with_intuition() {
    assert!(InstrumentType::Option.is_derivative());
    assert!(InstrumentType::CreditDefaultSwap.is_derivative());
    assert!(!InstrumentType::CommonStock.is_derivative());
    assert!(!InstrumentType::Cash.is_derivative());

    assert!(InstrumentType::GovernmentBond.has_maturity());
    assert!(InstrumentType::Future.has_maturity());
    assert!(!InstrumentType::CommonStock.has_maturity());
    assert!(!InstrumentType::RealEstate.has_maturity());
}

#[test]
fn liquidity_and_leverage_flags_are_set_where_they_matter() {
    assert!(AssetClass::Equity.is_typically_liquid());
    assert!(!AssetClass::PrivateMarket.is_typically_liquid());
    assert!(!AssetClass::RealAsset.is_typically_liquid());
    assert!(AssetClass::Derivative.is_leveraged_by_construction());
    assert!(!AssetClass::Equity.is_leveraged_by_construction());
}

// --- identifiers ------------------------------------------------------------

#[test]
fn isin_checksums_are_verified() {
    for valid in [
        "US0378331005",
        "US5949181045",
        "GB0002634946",
        "DE0005557508",
    ] {
        assert!(
            IdentifierKind::Isin.is_well_formed(valid),
            "{valid} should validate"
        );
    }
    for invalid in [
        "US0378331004",
        "US037833100",
        "0S0378331005",
        "US03783310051",
    ] {
        assert!(
            !IdentifierKind::Isin.is_well_formed(invalid),
            "{invalid} should be rejected"
        );
    }
}

#[test]
fn cusip_and_sedol_checksums_are_verified() {
    assert!(IdentifierKind::Cusip.is_well_formed("037833100"));
    assert!(IdentifierKind::Cusip.is_well_formed("912828U57"));
    assert!(!IdentifierKind::Cusip.is_well_formed("037833101"));

    assert!(IdentifierKind::Sedol.is_well_formed("0263494"));
    assert!(IdentifierKind::Sedol.is_well_formed("B1YW440"));
    assert!(!IdentifierKind::Sedol.is_well_formed("0263495"));
    // Vowels are excluded by the standard.
    assert!(!IdentifierKind::Sedol.is_well_formed("A263494"));
}

#[test]
fn identifier_confidence_orders_strong_above_weak() {
    assert!(IdentifierKind::Isin.confidence() > IdentifierKind::Ric.confidence());
    assert!(IdentifierKind::Ric.confidence() > IdentifierKind::ExchangeSymbol.confidence());
}

#[test]
fn primary_identifier_is_the_strongest_available() {
    let ids = Identifiers::new()
        .with(IdentifierKind::ExchangeSymbol, "AAPL")
        .with(IdentifierKind::Isin, "US0378331005")
        .with(IdentifierKind::Ric, "AAPL.O");
    assert_eq!(ids.primary().unwrap().0, IdentifierKind::Isin);
}

#[test]
fn a_conflicting_strong_identifier_is_evidence_of_a_different_instrument() {
    let a = Identifiers::new()
        .with(IdentifierKind::Isin, "US0378331005")
        .with(IdentifierKind::ExchangeSymbol, "AAPL");
    let b = Identifiers::new()
        .with(IdentifierKind::Isin, "US5949181045")
        .with(IdentifierKind::ExchangeSymbol, "AAPL");
    // The shared ticker must not outvote two different ISINs.
    assert_eq!(a.match_strength(&b), 0.0);

    let c = Identifiers::new().with(IdentifierKind::Isin, "US0378331005");
    assert!(approx_eq(a.match_strength(&c), 1.0, 1e-12));
}

#[test]
fn identifier_merge_keeps_the_first_writer() {
    let mut a = Identifiers::new().with(IdentifierKind::Isin, "US0378331005");
    let b = Identifiers::new()
        .with(IdentifierKind::Isin, "US5949181045")
        .with(IdentifierKind::Cusip, "037833100");
    a.merge(&b);
    assert_eq!(
        a.get(IdentifierKind::Isin),
        Some("US0378331005"),
        "must not be overwritten"
    );
    assert_eq!(
        a.get(IdentifierKind::Cusip),
        Some("037833100"),
        "new keys are added"
    );
}

#[test]
fn malformed_identifiers_are_reported() {
    let ids = Identifiers::new()
        .with(IdentifierKind::Isin, "NOTANISIN12")
        .with(IdentifierKind::Cusip, "037833100");
    let bad = ids.malformed();
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].0, IdentifierKind::Isin);
}

// --- object construction and validation -------------------------------------

#[test]
fn a_valid_equity_builds_and_validates() {
    let (ctx, now) = ctx();
    let object = equity(&ctx, now, "AAPL", "195.50");
    assert_eq!(object.asset_class, AssetClass::Equity);
    assert!(object.validate().is_empty());
    assert!(object.maturity().is_none());
    assert!(!object.is_expired(now));
}

#[test]
fn a_derivative_without_an_underlying_is_rejected() {
    let (ctx, now) = ctx();
    let result = FinancialObject::builder(
        ctx.ids().generate(now),
        "AAPL240119C00200000",
        InstrumentType::Option,
    )
    .venue("OPRA")
    .price(dec!("5.25"))
    .extension(Extension::Option(OptionDetails {
        underlying_object_id: "obj-aapl".into(),
        strike: dec!("200"),
        expiry: Timestamp::from_civil(2027, 1, 15),
        option_type: OptionType::Call,
        exercise_style: ExerciseStyle::American,
        contract_multiplier: Decimal::from_int(100),
        settlement: SettlementStyle::Physical,
        implied_volatility: 0.28,
        greeks: Greeks {
            delta: 0.45,
            gamma: 0.02,
            vega: 0.31,
            theta: -0.05,
            ..Greeks::default()
        },
        open_interest: 12_000,
    }))
    .provenance(Provenance::synthetic("test", now))
    .build(now);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("requires an underlying"), "{err}");
}

#[test]
fn a_bond_without_a_maturity_is_rejected() {
    let (ctx, now) = ctx();
    let result = FinancialObject::builder(
        ctx.ids().generate(now),
        "T 4.25 2034",
        InstrumentType::GovernmentBond,
    )
    .venue("OTC")
    .price(dec!("99.5"))
    .provenance(Provenance::synthetic("test", now))
    .build(now);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("requires a maturity"), "{err}");
}

#[test]
fn an_object_without_provenance_is_rejected() {
    let (ctx, now) = ctx();
    let result =
        FinancialObject::builder(ctx.ids().generate(now), "X", InstrumentType::CommonStock)
            .price(Decimal::ONE)
            .build(now);
    assert!(result.unwrap_err().to_string().contains("provenance"));
}

#[test]
fn a_negative_price_is_rejected() {
    let (ctx, now) = ctx();
    let result =
        FinancialObject::builder(ctx.ids().generate(now), "X", InstrumentType::CommonStock)
            .price(dec!("-1"))
            .provenance(Provenance::synthetic("test", now))
            .build(now);
    assert!(result.unwrap_err().to_string().contains("negative"));
}

#[test]
fn every_asset_class_can_be_represented() {
    let (ctx, now) = ctx();
    let maturity = Timestamp::from_civil(2034, 8, 15);
    let underlying = ObjectId::from_string("00000000000000000000000001");

    let specs: Vec<(&str, InstrumentType, Extension, Option<ObjectId>)> = vec![
        (
            "T4.25-34",
            InstrumentType::GovernmentBond,
            Extension::Bond(BondDetails {
                issuer: "US Treasury".into(),
                coupon_rate: 0.0425,
                coupon_frequency: CouponFrequency::SemiAnnual,
                maturity,
                issue_date: Timestamp::from_civil(2024, 8, 15),
                face_value: Decimal::from_int(100),
                day_count: DayCount::ActualActual,
                seniority: Seniority::SeniorUnsecured,
                credit_rating: Some(CreditRating::Aaa),
                yield_to_maturity: 0.0431,
                modified_duration: 6.8,
                convexity: 58.0,
                option_adjusted_spread_bps: 0.0,
                callable: false,
                puttable: false,
                inflation_index: None,
            }),
            None,
        ),
        (
            "CLO-2024-A-MEZZ",
            InstrumentType::StructuredCredit,
            Extension::StructuredCredit(StructuredCreditDetails {
                deal_name: "Harbour CLO 2024-1".into(),
                tranche: "Class C".into(),
                attachment_point: 0.12,
                detachment_point: 0.19,
                weighted_average_life_years: 5.4,
                effective_duration: 0.3,
                prepayment_speed: 0.20,
                collateral_type: "broadly syndicated loans".into(),
                legal_final_maturity: maturity,
                credit_rating: Some(CreditRating::Baa),
            }),
            None,
        ),
        (
            "EURUSD",
            InstrumentType::FxSpot,
            Extension::Fx(FxDetails {
                base_currency: Currency::EUR,
                quote_currency: Currency::USD,
                settlement_date: None,
                forward_points: 0.0,
                rate_differential: -0.012,
            }),
            None,
        ),
        (
            "CLZ6",
            InstrumentType::CommodityFuture,
            Extension::Future(FutureDetails {
                underlying_object_id: "obj-wti".into(),
                expiry: Timestamp::from_civil(2026, 12, 20),
                contract_size: Decimal::from_int(1000),
                tick_size: dec!("0.01"),
                tick_value: Decimal::from_int(10),
                initial_margin: Decimal::from_int(6000),
                maintenance_margin: Decimal::from_int(5000),
                settlement: SettlementStyle::Physical,
                contract_month_index: 1,
            }),
            Some(underlying.clone()),
        ),
        (
            "BTC",
            InstrumentType::Cryptocurrency,
            Extension::Digital(DigitalAssetDetails {
                chain: "bitcoin".into(),
                contract_address: None,
                decimals: 8,
                circulating_supply: Decimal::from_int(19_700_000),
                max_supply: Some(Decimal::from_int(21_000_000)),
                staking_yield: 0.0,
                is_tokenized_security: false,
            }),
            None,
        ),
        (
            "MERIDIAN-PE-IV",
            InstrumentType::PrivateEquityFund,
            Extension::PrivateAsset(PrivateAssetDetails {
                vintage_year: 2022,
                committed_capital: Decimal::from_int(50_000_000),
                called_capital: Decimal::from_int(30_000_000),
                distributed_capital: Decimal::from_int(12_000_000),
                residual_value: Decimal::from_int(34_000_000),
                stage: "buyout".into(),
                lockup_years: 10.0,
                capital_call_notice_days: 10,
            }),
            None,
        ),
        (
            "USD-CASH",
            InstrumentType::Cash,
            Extension::Cash(CashDetails {
                deposit_rate: 0.0525,
                days_to_settle: 0,
                counterparty: Some("custodian".into()),
            }),
            None,
        ),
    ];

    let mut universe = Universe::new();
    for (symbol, instrument_type, extension, underlying_id) in specs {
        let mut builder =
            FinancialObject::builder(ctx.ids().generate(now), symbol, instrument_type)
                .venue("OTC")
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", now));
        if let Some(id) = underlying_id {
            builder = builder.underlying(id);
        }
        let object = builder.extension(extension).build(now).expect(symbol);
        assert_eq!(object.asset_class, instrument_type.asset_class());
        universe.insert(object).expect(symbol);
    }

    assert_eq!(universe.len(), 7);
    let composition = universe.composition();
    assert!(
        composition.len() >= 6,
        "expected several asset classes: {composition:?}"
    );
}

// --- notional, lots and ticks ----------------------------------------------

#[test]
fn notional_accounts_for_the_contract_multiplier() {
    let (ctx, now) = ctx();
    let object = FinancialObject::builder(ctx.ids().generate(now), "ESZ6", InstrumentType::Future)
        .venue("XCME")
        .price(dec!("5450.25"))
        .contract_multiplier(Decimal::from_int(50))
        .underlying(ObjectId::from_string("00000000000000000000000001"))
        .extension(Extension::Future(FutureDetails {
            underlying_object_id: "obj-spx".into(),
            expiry: Timestamp::from_civil(2026, 12, 18),
            contract_size: Decimal::from_int(50),
            tick_size: dec!("0.25"),
            tick_value: dec!("12.5"),
            initial_margin: Decimal::from_int(13_000),
            maintenance_margin: Decimal::from_int(11_500),
            settlement: SettlementStyle::Cash,
            contract_month_index: 1,
        }))
        .provenance(Provenance::synthetic("test", now))
        .build(now)
        .unwrap();

    // Two contracts at 5450.25 with a 50x multiplier.
    assert_eq!(object.notional(Decimal::from_int(2)), dec!("545025"));
}

#[test]
fn lot_and_tick_rounding_never_rounds_up() {
    let (ctx, now) = ctx();
    let mut object = equity(&ctx, now, "AAPL", "195.50");
    object.lot_size = Decimal::from_int(100);
    object.tick_size = dec!("0.01");

    assert_eq!(object.round_to_lot(dec!("250")), Decimal::from_int(200));
    assert_eq!(object.round_to_lot(dec!("-250")), Decimal::from_int(-200));
    assert_eq!(object.round_to_tick(dec!("195.509")), dec!("195.5"));
}

// --- compliance -------------------------------------------------------------

#[test]
fn sanctions_block_every_direction_including_liquidation() {
    let (ctx, now) = ctx();
    let mut object = equity(&ctx, now, "BLOCKED", "10");
    object.regulatory =
        RegulatoryConstraints::unrestricted().with_restriction(TradingRestriction::Sanctioned {
            authority: "OFAC".into(),
        });

    for side in [TradeSide::Buy, TradeSide::Sell, TradeSide::SellShort] {
        let verdict = object.tradability(side);
        assert!(!verdict.is_allowed(), "{side:?} should be blocked");
        assert!(
            verdict.reasons()[0].contains("OFAC"),
            "{:?}",
            verdict.reasons()
        );
    }
}

#[test]
fn a_short_selling_ban_still_permits_buying_and_exiting() {
    let (ctx, now) = ctx();
    let mut object = equity(&ctx, now, "NOSHORT", "10");
    object.regulatory =
        RegulatoryConstraints::unrestricted().with_restriction(TradingRestriction::NoShortSelling);

    assert!(
        object.tradability(TradeSide::Buy).is_allowed(),
        "buying must remain open"
    );
    assert!(
        object.tradability(TradeSide::Sell).is_allowed(),
        "exiting must remain open"
    );
    assert!(!object.tradability(TradeSide::SellShort).is_allowed());
}

#[test]
fn venue_approval_defaults_to_permissive_but_binds_once_set() {
    let unrestricted = RegulatoryConstraints::unrestricted();
    assert!(unrestricted.permits_venue("ANY"));

    let restricted = RegulatoryConstraints::unrestricted()
        .with_approved_venue("XNYS")
        .with_jurisdiction(Jurisdiction::UnitedStates);
    assert!(restricted.permits_venue("XNYS"));
    assert!(!restricted.permits_venue("DARKPOOL"));
}

// --- data quality -----------------------------------------------------------

#[test]
fn quality_degrades_with_each_validation_failure() {
    let clean = DataQuality::clean();
    assert!(approx_eq(clean.score(), 1.0, 1e-12));
    assert!(clean.meets(0.6));

    let degraded = clean.with_issue("stale quote").with_issue("crossed book");
    assert_eq!(degraded.validation_failures, 2);
    assert!(degraded.score() < 0.3, "score {}", degraded.score());
    assert!(!degraded.meets(0.6));
}

#[test]
fn derived_quality_is_never_better_than_its_worst_input() {
    let good = DataQuality::clean();
    let bad = DataQuality::clean().with_issue("missing field").imputed();
    let combined = good.combined_with(&bad);
    assert!(combined.score() <= bad.score() + 1e-12);
    assert!(combined.is_imputed);
}

#[test]
fn synthetic_data_is_barred_from_production_decisions() {
    assert!(!LicensingClass::Synthetic.allows_production_decisions());
    assert!(LicensingClass::Licensed.allows_production_decisions());
    assert!(!LicensingClass::Restricted.allows_raw_display());
    assert!(LicensingClass::Public.allows_raw_display());
}

#[test]
fn an_intelligence_record_that_states_no_quality_does_not_decode() {
    // The premise. `DataQuality::default` is a perfect, directly observed
    // measurement, and it clears the floor that decides whether a record may
    // drive a capital decision — so a vendor that said nothing about quality
    // used to produce exactly the record a vendor that measured everything
    // would have produced.
    assert!(DataQuality::default().meets(DECISION_QUALITY_FLOOR));
    assert!(!DataQuality::default().is_imputed);
    assert!(approx_eq(DataQuality::default().completeness, 1.0, 1e-12));

    // One record, written twice: everything but the quality block is identical.
    const BODY: &str = r#""series_id":"US.CPI.YOY","region":"US","value":314.5,
      "unit":"index","reference_date":"2026-07-01T00:00:00Z",
      "provenance":{"source":"vendor","event_time":"2026-07-01T00:00:00Z",
        "ingestion_time":"2026-08-12T00:00:00Z","licensing":"licensed"}"#;
    const QUALITY: &str = r#""quality":{"completeness":1.0,"confidence":0.9,
      "validation_failures":0,"is_imputed":true}"#;

    let stated: MacroObservation = serde_json::from_str(&format!("{{{BODY},{QUALITY}}}"))
        .expect("a record that states its quality decodes");
    assert!(
        stated.quality.is_imputed,
        "the vendor said it filled this in, and that survives the decode"
    );

    let silent = format!("{{{BODY}}}");
    assert!(!silent.contains("quality"));
    let error = serde_json::from_str::<MacroObservation>(&silent)
        .expect_err("an unstated quality block became a perfect measurement");
    assert!(
        error.to_string().contains("quality"),
        "the failure has to name the missing field: {error}"
    );
}

#[test]
fn an_equity_that_states_no_listing_status_does_not_decode() {
    // The premise: `Listed` is the only status a default could name, and it is
    // the permissive one — a suspended or delisted line arriving without the
    // field would have read as a tradable one.
    let listed: ListingStatus =
        serde_json::from_str(r#""listed""#).expect("the wire form of the permissive value");
    assert_eq!(listed, ListingStatus::Listed);
    assert_ne!(listed, ListingStatus::Delisted);
    assert_ne!(listed, ListingStatus::Suspended);

    const STATED: &str = r#"{"shares_outstanding":"1000","free_float_shares":"800",
      "dividend_yield":0.01,"book_value_per_share":"12","earnings_per_share":"3",
      "voting_rights_per_share":1.0,"listing_status":"delisted"}"#;
    let details: EquityDetails =
        serde_json::from_str(STATED).expect("a line that states its status decodes");
    assert_eq!(details.listing_status, ListingStatus::Delisted);

    let silent = STATED.replace(r#","listing_status":"delisted""#, "");
    assert!(!silent.contains("listing_status"));
    assert!(
        serde_json::from_str::<EquityDetails>(&silent).is_err(),
        "a delisted line with the field omitted must not decode as a listed one"
    );
}

#[test]
fn an_object_built_on_synthetic_data_is_not_decision_grade() {
    let (ctx, now) = ctx();
    let object = equity(&ctx, now, "SYNTH", "100");
    assert!(
        !object.is_decision_grade(),
        "synthetic provenance must not be decision grade"
    );

    let mut licensed = object.clone();
    licensed.provenance.licensing = LicensingClass::Licensed;
    assert!(licensed.is_decision_grade());
}

#[test]
fn ingested_before_it_happened_is_a_validation_failure() {
    let (ctx, now) = ctx();
    let mut provenance = Provenance::synthetic("test", now);
    provenance.ingestion_time = now.saturating_sub(qip_core::Duration::from_hours(1));
    let result =
        FinancialObject::builder(ctx.ids().generate(now), "X", InstrumentType::CommonStock)
            .price(Decimal::ONE)
            .provenance(provenance)
            .build(now);
    assert!(result.unwrap_err().to_string().contains("ingested before"));
}

// --- risk characteristics ---------------------------------------------------

#[test]
fn incoherent_risk_characteristics_are_caught() {
    let mut risk = RiskCharacteristics::default();
    assert!(risk.is_coherent());
    risk.annualised_volatility = -0.2;
    assert!(!risk.is_coherent());

    let mut risk = RiskCharacteristics {
        default_probability: 1.5,
        ..RiskCharacteristics::default()
    };
    assert!(!risk.is_coherent());
    risk.default_probability = 0.02;
    risk.recovery_rate = 0.4;
    assert!(risk.is_coherent());
    assert!(approx_eq(risk.expected_credit_loss(), 0.012, 1e-12));
}

#[test]
fn the_spread_decomposition_identity_is_exact_in_decimal() {
    let risk = RiskCharacteristics {
        default_probability: 0.02,
        recovery_rate: 0.4,
        ..RiskCharacteristics::default()
    };
    let decomposed = risk.spread_decomposition().expect("valid inputs");

    // Premise first: default_probability and loss_given_default must
    // themselves be the values the identity is built from, not just the
    // final product, before trusting the product.
    assert_eq!(decomposed.default_probability, dec!("0.02"));
    assert_eq!(decomposed.loss_given_default, dec!("0.6"));
    assert_eq!(decomposed.spread, dec!("0.012"));
}

#[test]
fn a_recovery_rate_above_one_is_refused_not_clamped() {
    let risk = RiskCharacteristics {
        default_probability: 0.02,
        recovery_rate: 1.5,
        ..RiskCharacteristics::default()
    };
    let err = risk
        .spread_decomposition()
        .expect_err("recovery_rate outside [0, 1] must be refused");
    let message = err.to_string();
    assert!(message.contains("recovery_rate"));
    assert!(message.contains("1.5"));
}

#[test]
fn a_recovery_rate_of_exactly_one_is_the_admitted_boundary() {
    let risk = RiskCharacteristics {
        default_probability: 0.02,
        recovery_rate: 1.0,
        ..RiskCharacteristics::default()
    };
    let decomposed = risk
        .spread_decomposition()
        .expect("recovery_rate == 1.0 is the closed upper bound, not excluded");
    assert_eq!(decomposed.loss_given_default, Decimal::ZERO);
    assert_eq!(decomposed.spread, Decimal::ZERO);
}

#[test]
fn a_negative_default_probability_is_refused_not_clamped() {
    let risk = RiskCharacteristics {
        default_probability: -0.01,
        recovery_rate: 0.4,
        ..RiskCharacteristics::default()
    };
    let err = risk
        .spread_decomposition()
        .expect_err("negative default_probability must be refused");
    let message = err.to_string();
    assert!(message.contains("default_probability"));
    assert!(message.contains("-0.01"));
}

#[test]
fn greeks_aggregate_linearly() {
    let single = Greeks {
        delta: 0.5,
        gamma: 0.01,
        vega: 0.2,
        theta: -0.03,
        ..Greeks::default()
    };
    let position = single.scaled(10.0);
    assert!(approx_eq(position.delta, 5.0, 1e-12));
    let combined = position.combined(&single);
    assert!(approx_eq(combined.delta, 5.5, 1e-12));
    assert!(combined.is_finite());
}

#[test]
fn factor_exposures_blend_by_weight() {
    let a = FactorExposures::new()
        .with(factors::MARKET, 1.0)
        .with(factors::VALUE, 0.5);
    let b = FactorExposures::new()
        .with(factors::MARKET, 0.5)
        .with(factors::MOMENTUM, -0.3);
    let blended = a.blend(0.6, &b, 0.4);
    assert!(approx_eq(blended.get(factors::MARKET), 0.8, 1e-12));
    assert!(approx_eq(blended.get(factors::VALUE), 0.3, 1e-12));
    assert!(approx_eq(blended.get(factors::MOMENTUM), -0.12, 1e-12));
    assert_eq!(blended.dominant().unwrap().0, factors::MARKET);
    assert_eq!(a.get("nonexistent"), 0.0);
}

// --- extensions -------------------------------------------------------------

#[test]
fn credit_ratings_map_onto_a_unified_scale() {
    assert_eq!(CreditRating::parse("AAA"), Some(CreditRating::Aaa));
    assert_eq!(CreditRating::parse("BBB+"), Some(CreditRating::Baa));
    assert_eq!(
        CreditRating::parse("Ba1"),
        Some(CreditRating::Ba),
        "Moody's numeric modifier"
    );
    assert_eq!(
        CreditRating::parse("AA-"),
        Some(CreditRating::Aa),
        "S&P sign modifier"
    );
    assert_eq!(CreditRating::parse("Caa3"), Some(CreditRating::Caa));
    assert_eq!(CreditRating::parse("nonsense"), None);

    assert!(CreditRating::Baa.is_investment_grade());
    assert!(!CreditRating::Ba.is_investment_grade());
    assert!(
        CreditRating::Caa.indicative_default_probability()
            > CreditRating::Aaa.indicative_default_probability()
    );
}

#[test]
fn day_count_conventions_differ_as_expected() {
    let from = Timestamp::from_civil(2026, 1, 1);
    let to = Timestamp::from_civil(2026, 7, 1);
    let act360 = DayCount::Actual360.year_fraction(from, to);
    let act365 = DayCount::Actual365.year_fraction(from, to);
    let thirty = DayCount::Thirty360.year_fraction(from, to);
    assert!(act360 > act365, "act/360 accrues faster");
    assert!(
        approx_eq(thirty, 0.5, 1e-12),
        "30/360 gives exactly half a year: {thirty}"
    );
}

#[test]
fn private_asset_multiples_are_computed_correctly() {
    let details = PrivateAssetDetails {
        vintage_year: 2022,
        committed_capital: Decimal::from_int(50_000_000),
        called_capital: Decimal::from_int(30_000_000),
        distributed_capital: Decimal::from_int(12_000_000),
        residual_value: Decimal::from_int(34_000_000),
        stage: "buyout".into(),
        lockup_years: 10.0,
        capital_call_notice_days: 10,
    };
    // (12 + 34) / 30
    assert!(approx_eq(details.tvpi().unwrap(), 46.0 / 30.0, 1e-12));
    assert!(approx_eq(details.dpi().unwrap(), 0.4, 1e-12));
    assert_eq!(details.unfunded_commitment(), Decimal::from_int(20_000_000));

    let uncalled = PrivateAssetDetails {
        called_capital: Decimal::ZERO,
        ..details
    };
    assert!(uncalled.tvpi().is_none(), "must not divide by zero");
}

#[test]
fn structured_credit_tranche_thickness() {
    let details = StructuredCreditDetails {
        deal_name: "D".into(),
        tranche: "C".into(),
        attachment_point: 0.12,
        detachment_point: 0.19,
        weighted_average_life_years: 5.0,
        effective_duration: 0.3,
        prepayment_speed: 0.2,
        collateral_type: "loans".into(),
        legal_final_maturity: Timestamp::from_civil(2034, 1, 1),
        credit_rating: None,
    };
    assert!(approx_eq(details.tranche_thickness(), 0.07, 1e-12));
}

#[test]
fn redemption_frequency_drives_liquidity_horizon() {
    assert_eq!(RedemptionFrequency::Daily.days_to_liquidity(), 1);
    assert_eq!(RedemptionFrequency::Quarterly.days_to_liquidity(), 90);
    assert!(RedemptionFrequency::Locked.days_to_liquidity() > 1000);
}
