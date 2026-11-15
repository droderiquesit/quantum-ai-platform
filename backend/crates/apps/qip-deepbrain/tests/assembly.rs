//! R12-GCP-8106: the stress analysis the Risk brain produces comes from the
//! platform the deployed node assembles, not from a platform a test built
//! beside it. `assemble` is the function `main` calls; this drives a held book
//! through it to SIMULATE and reads the stress report back.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::deployment::Deployment;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_deepbrain::assembly::{Inputs, assemble};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::sync::Arc;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

fn inputs(deployment: Deployment, key: Option<&str>) -> Inputs {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Inputs {
        config,
        context,
        telemetry: Telemetry::silent(),
        universe: universe(),
        limits: LimitSet::new("assembly-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
        language_model: Arc::new(qip_ai::language::DeterministicModel::default()),
        envelope_key: key.map(str::to_string),
        deployment,
    }
}

fn observations(symbol: &str, phase: f64) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..120)
        .map(|i| {
            let noise = ((f64::from(i) * 0.7548776662 + phase) % 1.0 - 0.5) * 0.02;
            let open = price;
            price *= 1.0 + noise;
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: start().saturating_sub(Duration::from_days(120 - i64::from(i))),
                open: Decimal::from_f64(open).expect("finite"),
                high: Decimal::from_f64(open.max(price) * 1.002).expect("finite"),
                low: Decimal::from_f64(open.min(price) * 0.998).expect("finite"),
                close: Decimal::from_f64(price).expect("finite"),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

#[test]
fn the_assembled_node_stresses_the_book_it_holds() -> Result<()> {
    let mut node = assemble(inputs(Deployment::Local, None))?.platform;
    // Premise: before a position and history exist there is no report, so the
    // report below was produced by the held book and not by assembly itself.
    node.run_cycle(start());
    assert!(node.stress_report().is_none());

    node.observe(observations("AAA", 0.0));
    node.observe(observations("BBB", 0.31));
    let order = node.order_from(
        object("AAA"),
        Side::Buy,
        dec!("100"),
        dec!("100"),
        "prop-assembly",
        vec!["hyp-assembly".to_string()],
        start(),
    );
    node.submit_order(order, start())?;
    let report = node.run_cycle(start().saturating_add(Duration::from_days(1)));

    let stress = node.stress_report().expect("the held book is stressed");
    assert!(!stress.scenarios.is_empty());
    assert!(stress.worst().unwrap().loss_fraction > 0.0);
    assert!(report.stage(Stage::Simulate).unwrap().produced > 0);
    Ok(())
}

#[test]
fn assembly_refuses_a_deployment_that_would_sign_on_the_seed_derived_key() {
    // The trust root is part of the composition: a deployed process with no
    // operator key stops here, which is what `main` relies on.
    let refused = assemble(inputs(Deployment::Dev, None)).unwrap_err();
    assert!(refused.message().contains("seed"), "{}", refused.message());
    let keyed = assemble(inputs(
        Deployment::Dev,
        Some("an-operator-key-for-the-test-00000000"),
    ));
    if let Err(error) = &keyed {
        panic!("{}", error.message());
    }
}
