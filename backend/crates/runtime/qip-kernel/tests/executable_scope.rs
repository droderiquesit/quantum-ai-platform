//! The executable scope at the mouth of the queue (R12-GOV-6117,
//! R12-GOV-6122).
//!
//! `qip-contracts` proves the type: a `ShadowRecord` has no path to an
//! `ActionIntent`. This proves the half it cannot see — that the kernel's
//! DISCOVER stage routes every detection on an instrument through the scope,
//! so an opportunity on something the universe does not hold is journaled
//! as shadowed and never queued, on this pass or any later one that
//! re-detects and re-scores it.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::action::Routed;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::{EventFilter, Topic};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::executable_scope::OpportunityShadowed;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

/// Two instruments. `ZZZ` is deliberately absent: it is the instrument the
/// detectors will find an opportunity on and the scope must refuse.
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

/// A price series with a jump partway through, so the detectors have
/// something real to find; the last bar closes at `from`.
fn bars(symbol: &str, from: Timestamp, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = from.saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

fn shadowed(platform: &Platform) -> Result<Vec<OpportunityShadowed>> {
    platform
        .replay_journal(&EventFilter::new().topic(Topic::OpportunityRanked))?
        .iter()
        .filter_map(|envelope| envelope.decode::<OpportunityShadowed>().ok())
        .map(|decoded| Ok(decoded.body))
        .collect()
}

fn names(opportunity: &qip_opportunity_engine::opportunity::Opportunity, symbol: &str) -> bool {
    opportunity.affected_objects.contains(&object(symbol))
}

/// Mutation: in `Platform::route_detected` push the `Routed::Shadow` arm's
/// opportunity onto `executable` as the `Executable` arm does — the queue
/// then carries `obj-ZZZ` and no shadow record is journaled.
#[test]
fn an_opportunity_outside_the_executable_scope_is_shadowed_and_never_queued_however_often_it_is_rescored()
-> Result<()> {
    let mut platform = platform()?;
    assert!(
        matches!(
            platform
                .executable_scope()
                .route("any", object("ZZZ").as_str()),
            Routed::Shadow(_)
        ),
        "the premise: ZZZ is outside the scope the universe defines"
    );
    assert!(
        matches!(
            platform
                .executable_scope()
                .route("any", object("AAA").as_str()),
            Routed::Executable(_)
        ),
        "the premise: AAA is inside it"
    );

    let mut found_on_zzz = 0usize;
    let mut queued_on_aaa = 0usize;
    for pass in 0..3i64 {
        // Fresh history every pass, so each pass re-detects and re-scores.
        let now = start().saturating_add(Duration::from_days(pass * 30));
        platform.observe(bars("ZZZ", now, 120));
        platform.observe(bars("AAA", now, 120));
        let report = platform.run_cycle(now);
        let discover = report.stage(Stage::Discover).expect("discover ran");
        assert!(
            discover.detail.contains("found"),
            "the premise: DISCOVER reported: {}",
            discover.detail
        );
        found_on_zzz += shadowed(&platform)?
            .iter()
            .filter(|record| record.instrument == object("ZZZ").as_str())
            .count();
        queued_on_aaa += usize::from(platform.queue().iter().any(|o| names(o, "AAA")));
        assert!(
            !platform.queue().iter().any(|o| names(o, "ZZZ")),
            "pass {pass}: an opportunity on ZZZ reached the queue: {}",
            discover.detail
        );
        if let Some(reason) = report.stage(Stage::Reason) {
            assert!(
                !reason.detail.contains(object("ZZZ").as_str()),
                "pass {pass}: REASON took up an opportunity on ZZZ: {}",
                reason.detail
            );
        }
    }
    let records = shadowed(&platform)?;
    assert!(
        found_on_zzz > 0,
        "the premise: the detectors never found an opportunity on ZZZ"
    );
    assert!(
        records
            .iter()
            .all(|record| record.instrument == object("ZZZ").as_str()),
        "something inside the scope was shadowed: {records:?}"
    );
    assert!(
        queued_on_aaa > 0,
        "the scope refused everything: no opportunity on AAA was ever queued"
    );
    assert!(
        platform
            .run_cycle(start().saturating_add(Duration::from_days(100)))
            .stage(Stage::Discover)
            .expect("discover ran")
            .detail
            .contains("shadowed as outside the executable scope"),
        "DISCOVER does not report the shadow count"
    );
    Ok(())
}
