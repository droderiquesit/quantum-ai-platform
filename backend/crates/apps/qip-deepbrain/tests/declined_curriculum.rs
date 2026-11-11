//! A refused path resolved in LEARN reaches the deep brain's curriculum
//! (R12-EXPAND-6054): three regretted refusals at one gate are priced on the
//! production cycle, recorded as declined scores, and the next research
//! sweep raises the repeated-veto gap for that gate.
//!
//! The fixtures are the kernel's own (`qip-kernel/tests/learning.rs`):
//! `refuse_one` submits an order that names no proposal the platform knows,
//! which the provenance check refuses, and a rising tape afterwards makes the
//! refusal a regret.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::ids::OrderId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::collections::{BTreeMap, BTreeSet};

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

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
                    fixture_liquidity(),
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

fn limits() -> LimitSet {
    LimitSet::new("kernel-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

fn refuse_one(platform: &mut Platform, proposal: &str, at: Timestamp) -> Result<OrderId> {
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        proposal,
        Vec::new(),
        at,
    );
    let order_id = order.order_id.clone();
    assert!(
        platform.submit_order(order, at).is_err(),
        "an untraceable order was accepted; the fixture is not a refusal"
    );
    Ok(order_id)
}

fn bars_after(symbol: &str, from: Timestamp, days: i64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            let at = from.saturating_add(Duration::from_days(day));
            let open = 100.0 + day as f64;
            bar(symbol, at, open, open + 0.5)
        })
        .collect()
}

fn quiet_bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    (0..count)
        .map(|i| {
            let wiggle = if i % 2 == 0 { 0.3 } else { -0.3 };
            let price = 100.0 + wiggle;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, price, price)
        })
        .collect()
}

/// R12-EXPAND-6054: three refusals at one gate, each a regret once the tape
/// rises, are priced in LEARN on the production cycle and recorded as
/// declined scores, and the deep brain's next research sweep raises the
/// repeated-veto gap for that gate, once, naming the gate and the count.
///
/// The closure also asked that the third resolution move
/// `counterfactual_sizing_multiplier`. It cannot, by design: that multiplier
/// narrows only on declines that were *right*, and a regret (a veto that
/// cost money) is §12.4's case where nothing may loosen automatically. So
/// that clause is not asserted here; the row records why.
///
/// Mutation: drop the `veto_observations` call in `sweep_over`.
#[test]
fn three_regretted_refusals_at_one_gate_raise_the_repeated_veto_curriculum_gap() -> Result<()> {
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 90));
    for proposal in ["prop-1", "prop-2", "prop-3"] {
        refuse_one(&mut platform, proposal, start())?;
    }
    platform.observe(bars_after("AAA", start(), 5));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));

    let scores = platform.declined_scores();
    assert_eq!(
        scores.len(),
        3,
        "premise: three refusals were priced: {scores:?}"
    );
    assert!(
        scores.iter().all(|s| s.regret),
        "premise: each was a regret: {scores:?}"
    );
    let gate = scores[0].gate.clone();
    assert!(scores.iter().all(|s| s.gate == gate), "premise: one gate");

    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())?;
    research.sweep(
        &platform,
        start().saturating_add(Duration::from_days(3)),
        &BTreeSet::new(),
        &BTreeSet::from(["US".to_string()]),
    )?;
    let evidence = format!("declined_scores:gate:{gate}");
    let queued: Vec<String> = research
        .desk()
        .queue()
        .ranked()
        .iter()
        .filter_map(|entry| entry.origin())
        .filter(|origin| origin.signal.evidence.contains(&evidence))
        .map(|origin| origin.signal.observation.clone())
        .collect();
    assert_eq!(
        queued.len(),
        1,
        "the repeated-veto gap was not raised once: {queued:?}"
    );
    assert!(
        queued[0].contains(&format!("gate `{gate}` declined 3 of 3")),
        "{queued:?}"
    );
    Ok(())
}
