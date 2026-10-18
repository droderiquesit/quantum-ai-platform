//! Competing world models disagree in UNDERSTAND, and the disagreement is a
//! record and not a trade (R12-COVERAGE-7180).
//!
//! The kernel's own tests prove each seam of the federation. What no suite
//! held is the composite as a reader of the hash-chained log sees it: members
//! fed divergent views, a recorded `Disagreement` whose views carry branch
//! lineage, and — because the federation is advisory, structurally — no order
//! released by a cycle that formed one. A federation that quietly averaged its
//! members, or one whose verdict reached ACT, would pass every unit test and
//! fail here.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::federation_stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::federation::{Decision, MATERIAL_DISAGREEMENT_SPREAD};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe.insert(
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
            .build(start())?,
        )?;
    }
    let limits = LimitSet::new("federation-disagreement").with(
        Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        )
        .with_rationale("no single name may dominate the book"),
    );
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits)
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

/// Noise plus a periodic jump, so the momentum, reversion and tail members
/// read one tape and answer different questions.
fn tape(symbol: &str, count: usize, end: Timestamp) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i % 17 == 0 { 0.02 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = end.saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

#[test]
fn divergent_members_journal_a_disagreement_with_branch_lineage_and_no_cycle_releases_an_order()
-> Result<()> {
    let cycles: u64 = 14;
    let mut platform = platform()?;
    let total = 120 + cycles as usize;
    let end = start().saturating_add(Duration::from_days(cycles as i64));
    let mut aaa = tape("AAA", total, end);
    let mut bbb = tape("BBB", total, end);
    let later_aaa = aaa.split_off(120);
    let later_bbb = bbb.split_off(120);
    aaa.extend(bbb);
    platform.observe(aaa);
    let mut released = 0;
    for (step, (a, b)) in later_aaa.into_iter().zip(later_bbb).enumerate() {
        platform.observe(vec![a, b]);
        let at = start().saturating_add(Duration::from_days(step as i64 + 1));
        let report = platform.run_cycle(at);
        released += report.stage(Stage::Act).expect("act ran").produced;
    }

    let reviews = federation_stage::replay(&platform)?;
    assert_eq!(reviews.len() as u64, cycles, "one review per cycle");

    // Premise: more than one member held a view, and a spread over the
    // material line was recorded rather than collapsed into one number.
    let material: Vec<_> = reviews
        .iter()
        .flat_map(|r| &r.arbitrations)
        .filter(|a| a.disagreement.views.len() >= 2)
        .filter(|a| a.disagreement.spread >= MATERIAL_DISAGREEMENT_SPREAD)
        .collect();
    assert!(
        !material.is_empty(),
        "no material disagreement was recorded"
    );
    for a in &material {
        let probabilities: std::collections::BTreeSet<u64> = a
            .disagreement
            .views
            .iter()
            .map(|v| v.probability.to_bits())
            .collect();
        assert!(probabilities.len() >= 2, "the views were averaged: {a:?}");
    }

    // A Branch decision on that disagreement left a branch whose view later
    // carries its parent in the arbitration record itself.
    assert!(
        reviews
            .iter()
            .flat_map(|r| &r.arbitrations)
            .any(|a| a.decision == Decision::Branch),
        "no arbitration ever chose to branch"
    );
    let branched = reviews
        .iter()
        .flat_map(|r| &r.arbitrations)
        .flat_map(|a| &a.disagreement.views)
        .find(|v| !v.lineage.parents.is_empty())
        .expect("a branch's view reached a recorded disagreement");
    assert_eq!(branched.lineage.depth, 1, "{branched:?}");
    assert!(branched.lineage.trigger.is_some(), "{branched:?}");

    // Advisory, structurally: forming and journaling all of that released
    // no order and moved no book.
    assert_eq!(released, 0, "a federation verdict reached ACT");
    assert!(platform.orders().fills().is_empty());
    assert!(platform.positions_at_cost().is_empty());
    Ok(())
}
