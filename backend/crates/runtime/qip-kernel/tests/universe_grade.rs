//! A degraded universe is visible at assembly, not after its first bad trade.
//!
//! `Universe::not_decision_grade` documented that the kernel logged it at
//! start-up, and nothing did: a universe assembled entirely from
//! research-only instruments looked exactly like one fit to trade until a
//! proposal was sized on it.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_kernel::series;
use qip_observability::Telemetry;
use qip_observability::metrics::labels;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str, provenance: Provenance) -> FinancialObject {
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::CommonStock,
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(provenance)
    .build(start())
    .expect("valid object")
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

#[test]
fn a_research_only_instrument_is_counted_and_named_at_assembly() -> Result<()> {
    // One instrument licensed for production decisions, one synthetic — the
    // licensing class that never may drive one. Premise: the universe itself
    // says exactly one is unfit, so the gauge below measures the kernel's
    // reading of it and not an empty list.
    let mut universe = Universe::new();
    universe.insert(object(
        "AAA",
        Provenance::new("vendor", start(), start()).with_licensing(LicensingClass::Licensed),
    ))?;
    universe.insert(object("ZZZ", Provenance::synthetic("research", start())))?;
    assert_eq!(
        universe.not_decision_grade().len(),
        1,
        "the fixture must hold exactly one degraded instrument"
    );

    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let platform = Platform::new(config, context, Telemetry::silent(), universe, limits())?;

    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .gauge(series::UNIVERSE_NOT_DECISION_GRADE, &labels([])),
        Some(1.0),
        "the degraded count did not reach the registry at assembly"
    );
    let degraded = platform.universe_not_decision_grade();
    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0].0, "obj-ZZZ");
    assert!(
        degraded[0].1.contains("licensing class") && degraded[0].1.contains("Synthetic"),
        "the reason must name the licensing class that disqualified it: {}",
        degraded[0].1
    );
    Ok(())
}
