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

    // And the refusing half. A crypto spot is the case §16.1's engine table
    // does not reach: continuously quoted, its mark is the quote, and none of
    // the six paradigms unlocks `DigitalAsset`. So no record registers the
    // class, and §17.7 says an unregistered class cannot be traded — the
    // platform holds reference data for it and may not act on it.
    assert!(
        !good
            .asset_class_registry()
            .is_registered(AssetClass::DigitalAsset),
        "premise: the registry does not hold DigitalAsset, or this proves nothing"
    );
    let error = assemble(universe_of(vec![
        object("obj-AAA", InstrumentType::CommonStock, None)?,
        object("obj-BTC", InstrumentType::Cryptocurrency, None)?,
    ])?)
    .err()
    .ok_or_else(|| {
        qip_core::Error::invalid("a universe holding an unregistered class assembled")
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
