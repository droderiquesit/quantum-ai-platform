//! R12-RISK-3022: the order path asks the cost router for a deterministic
//! routing before any limit is read.
//!
//! `Determinism::Required` was constructed nowhere in production, so the
//! structural guarantee that a pre-trade check never routes to a model was
//! proven on the type and not on the call. `Platform::submit_order` now obtains
//! its routing from `Router::determine`; these tests fail if it stops.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_cost_router::{Horizon, IntelligenceTier};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string("obj-AAA"),
            "AAA",
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
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

fn order(platform: &mut Platform, quantity: Decimal) -> qip_execution_engine::order::Order {
    platform.order_from(
        ObjectId::from_string("obj-AAA"),
        Side::Buy,
        quantity,
        dec!("100"),
        "prop-determinism",
        vec!["hyp-determinism".to_string()],
        start(),
    )
}

#[test]
fn an_accepted_order_was_guarded_by_a_deterministic_routing_that_names_no_model() -> Result<()> {
    let mut platform = platform()?;
    assert!(
        platform.pretrade_routing().is_none(),
        "premise: nothing routed yet"
    );
    let order = order(&mut platform, dec!("10"));
    platform.submit_order(order, start())?;
    let routing = platform
        .pretrade_routing()
        .expect("submit_order must route");
    assert_eq!(routing.charge().tier, IntelligenceTier::DeterministicCode);
    assert_eq!(routing.charge().tier.model_tier(), None);
    assert_eq!(routing.conditions().horizon, Horizon::Microsecond);
    assert!(routing.rationale().contains("pre-trade limits for obj-AAA"));
    Ok(())
}

#[test]
fn a_refused_order_was_still_routed_deterministically_before_the_limits_spoke() -> Result<()> {
    let mut platform = platform()?;
    // Far past any conservative cap, so the limits refuse it.
    let order = order(&mut platform, dec!("1000000000"));
    let result = platform.submit_order(order, start());
    assert!(
        result.is_err(),
        "premise: the limits must refuse this order"
    );
    let routing = platform
        .pretrade_routing()
        .expect("routing precedes the limits");
    assert_eq!(routing.charge().tier, IntelligenceTier::DeterministicCode);
    Ok(())
}
