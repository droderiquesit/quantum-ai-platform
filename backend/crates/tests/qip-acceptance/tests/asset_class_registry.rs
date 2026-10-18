//! Blueprint §17.7 from outside the kernel: the shipped registry holds every
//! asset class the object model can name, and `Platform::new` assembles over
//! an instrument of each class that has an instrument type.
//!
//! `qip-kernel/tests/asset_classes.rs` proves the gate on one class pair; it
//! cannot see a class silently dropped from `AssetClassRegistry::shipped`,
//! because it only ever withdraws the one record it names. This suite walks
//! `AssetClass::ALL`, so a sixteenth class added to the enum without a record
//! (or a record deleted from the table) fails here by name.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::{Context, ObjectId, Timestamp, dec};
use qip_financial::asset_class::{AssetClass, InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::asset_class_registry::{AssetClassRegistry, LifecycleEngine};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn instrument(id: &str, kind: InstrumentType) -> Result<FinancialObject> {
    let liquidity = LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0);
    FinancialObject::builder(ObjectId::from_string(id), id, kind, liquidity)
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("asset-class-registry", start()))
        .build(start())
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

/// One instrument type per class, chosen among the types that need no
/// maturity or extension record to build. `FixedIncome`, `Credit` and
/// `Derivative` are absent for that reason alone: every type of theirs is a
/// dated claim. `Prediction` and `PhysicalProduct` have no `InstrumentType`
/// at all, so no instrument can be in them yet; the registry test below
/// still holds their records.
const CLASS_REPRESENTATIVES: [(AssetClass, InstrumentType); 10] = [
    (AssetClass::Equity, InstrumentType::CommonStock),
    (AssetClass::Rates, InstrumentType::YieldCurvePoint),
    (AssetClass::ForeignExchange, InstrumentType::FxSpot),
    (AssetClass::Commodity, InstrumentType::CommoditySpot),
    (AssetClass::DigitalAsset, InstrumentType::Cryptocurrency),
    (AssetClass::Fund, InstrumentType::MutualFund),
    (AssetClass::PrivateMarket, InstrumentType::PrivateEquityFund),
    (AssetClass::RealAsset, InstrumentType::RealEstate),
    (AssetClass::Cash, InstrumentType::Cash),
    (
        AssetClass::StructuredProduct,
        InstrumentType::SyntheticBasket,
    ),
];

#[test]
fn the_shipped_registry_holds_a_record_for_every_asset_class_the_object_model_names() -> Result<()>
{
    assert!(
        AssetClass::ALL.len() > 1,
        "premise: the enum names more than one class, or the loop below proves nothing"
    );
    let registry = AssetClassRegistry::shipped()?;
    for class in AssetClass::ALL {
        assert!(
            registry.is_registered(class),
            "{} has no record in AssetClassRegistry::shipped",
            class.as_str()
        );
    }
    assert!(registry.unregistered().is_empty());
    assert_eq!(registry.len(), AssetClass::ALL.len());
    // The tokenized representation §17.7 lists under digital assets is the
    // `TokenizedSecurity` type, and it resolves into the class that is
    // registered rather than into a class of its own.
    assert_eq!(
        InstrumentType::TokenizedSecurity.asset_class(),
        AssetClass::DigitalAsset
    );
    Ok(())
}

#[test]
fn withdrawing_one_class_record_is_visible_in_the_unregistered_list() -> Result<()> {
    let registry = AssetClassRegistry::shipped()?;
    assert!(
        registry.is_registered(AssetClass::DigitalAsset),
        "premise: the class is registered before it is withdrawn"
    );
    let narrowed = registry.without(AssetClass::DigitalAsset)?;
    assert_eq!(narrowed.unregistered(), vec![AssetClass::DigitalAsset]);
    Ok(())
}

#[test]
fn an_instrument_of_each_constructible_class_assembles_through_platform_new() -> Result<()> {
    let mut universe = Universe::new();
    for (class, kind) in CLASS_REPRESENTATIVES {
        let object = instrument(&format!("obj-{}", class.as_str()), kind)?;
        assert_eq!(
            object.asset_class,
            class,
            "premise: {} maps to its representative class",
            kind.as_str()
        );
        universe.insert(object)?;
    }
    let platform = assemble(universe)?;
    for (class, _) in CLASS_REPRESENTATIVES {
        assert!(platform.asset_class_registry().is_registered(class));
    }
    Ok(())
}

#[test]
fn withdrawing_a_class_makes_admit_refuse_its_instruments_and_no_other_class() -> Result<()> {
    // The refusing half, through the same `admit` that `Platform::new` calls
    // on every instrument, on a registry with exactly one record withdrawn.
    // `without` itself refuses to withdraw a class a sibling is hedged into
    // (`Equity` is one), so those classes are counted and skipped rather than
    // assumed withdrawable; the count is asserted so the skip cannot swallow
    // the whole loop.
    let registry = AssetClassRegistry::shipped()?;
    let mut withdrawn = 0;
    for (class, kind) in CLASS_REPRESENTATIVES {
        let object = instrument(&format!("obj-{}", class.as_str()), kind)?;
        registry.admit(&object)?;
        let Ok(narrowed) = registry.clone().without(class) else {
            continue;
        };
        withdrawn += 1;
        let refusal = narrowed.admit(&object).err().ok_or_else(|| {
            Error::invalid(format!(
                "{} was admitted with its class withdrawn",
                class.as_str()
            ))
        })?;
        assert!(
            refusal
                .message()
                .contains(&format!("is a {} and no record", class.as_str())),
            "{}",
            refusal.message()
        );
        for (other, other_kind) in CLASS_REPRESENTATIVES {
            if other != class {
                narrowed.admit(&instrument(&format!("obj-{}", other.as_str()), other_kind)?)?;
            }
        }
    }
    assert!(
        withdrawn >= 2,
        "premise: more than one class is withdrawable, got {withdrawn}"
    );
    Ok(())
}

/// R12-ASSET-1015: every class carries one of the three lifecycle engines,
/// the engine matches what the class is, and a class with no record has none
/// rather than a guessed one. Mutation: map `PrivateMarket` to
/// `PortfolioStages` in `LifecycleEngine::for_class` — the private-class
/// assertion below fails.
#[test]
fn every_asset_class_resolves_its_lifecycle_engine_and_an_unregistered_class_is_refused()
-> Result<()> {
    let registry = AssetClassRegistry::shipped()?;
    let mut seen = std::collections::BTreeSet::new();
    for class in AssetClass::ALL {
        let engine = registry.lifecycle_of(class)?;
        assert_eq!(registry.get(class).expect("registered").lifecycle(), engine);
        seen.insert(engine);
    }
    // Premise: the three engines are all in use, or the loop proves one.
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(
        registry.lifecycle_of(AssetClass::Equity)?,
        LifecycleEngine::PortfolioStages
    );
    assert_eq!(
        registry.lifecycle_of(AssetClass::PrivateMarket)?,
        LifecycleEngine::PrivateStages
    );
    assert_eq!(
        registry.lifecycle_of(AssetClass::PhysicalProduct)?,
        LifecycleEngine::CommerceLedger
    );
    assert_eq!(LifecycleEngine::PortfolioStages.opens_with(), "discover");
    assert_eq!(LifecycleEngine::PrivateStages.opens_with(), "committed");
    // A class can be withdrawn only where no sibling is hedged into it, so
    // take the first the registry lets go.
    let (withdrawn, narrowed) = AssetClass::ALL
        .into_iter()
        .find_map(|class| {
            AssetClassRegistry::shipped()
                .ok()?
                .without(class)
                .ok()
                .map(|r| (class, r))
        })
        .expect("at least one class is withdrawable");
    let refused = narrowed
        .lifecycle_of(withdrawn)
        .expect_err("an unregistered class resolved an engine");
    assert!(
        refused.message().contains(withdrawn.as_str()),
        "{}",
        refused.message()
    );
    Ok(())
}
