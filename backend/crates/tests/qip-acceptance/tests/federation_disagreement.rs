//! Competing world models disagree in UNDERSTAND, and the disagreement is a
//! record and not a trade (R12-COVERAGE-7180).
//!
//! The kernel's own tests prove each seam of the federation. What no suite
//! held is the composite as a reader of the hash-chained log sees it: members
//! fed divergent views, a recorded `Disagreement` whose views carry branch
//! lineage, and — because the federation is advisory, structurally — no order
//! released by a cycle that formed one. A federation that quietly averaged its
//! members would pass every unit test and fail here.
//!
//! That a verdict never reaches ACT is held by the second test, not the first:
//! the first tape releases nothing whatever the federation does (it carries no
//! catalyst or macro series, so no hypothesis clears the review floor), and a
//! zero there cannot tell an advisory federation from a veto. The second runs
//! the vertical-slice scenario, whose control run fills, and asserts the same
//! order is released with a material disagreement on the log.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::intelligence::MacroObservation;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::federation_stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::federation::{Decision, MATERIAL_DISAGREEMENT_SPREAD};
use qip_world_model::vocabulary::MacroSeries;

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

    // Baseline only: this bare price tape releases nothing with or without a
    // federation, so the zero is the platform's, not evidence the federation
    // is advisory. `a_cycle_that_records_a_material_disagreement_still_releases_its_order`
    // below is the test that holds that.
    assert_eq!(released, 0, "the bare tape unexpectedly released an order");
    assert!(platform.orders().fills().is_empty());
    assert!(platform.positions_at_cost().is_empty());
    Ok(())
}

// --- the control: a scenario that releases an order ---------------------------

const BOOK: i64 = 10_006_000;
const TAPE_BARS: usize = 400;

fn acme() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn acme_universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            acme(),
            "ACME",
            InstrumentType::CommonStock,
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .geography("US")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("federation-disagreement", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

/// The vertical-slice tape: quiet, then a twelve percent drop on the last bar.
fn acme_tape() -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE_BARS)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.003;
            let drop = if i == TAPE_BARS - 1 { -0.12 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE_BARS - i) as i64));
            let mut record = bar("ACME", at, open, price);
            if let SensedRecord::Bar(bar) = &mut record {
                bar.object_id = acme();
            }
            record
        })
        .collect()
}

/// Forty prints of each series the macro analyst reads, ending on a step that
/// reads as supportive for risk assets.
fn acme_macro() -> Vec<SensedRecord> {
    const PRINTS: usize = 40;
    let mut records = Vec::new();
    for (series, level, amplitude, supportive) in [
        (MacroSeries::PolicyRate, 4.0, 0.05, -1.0),
        (MacroSeries::InflationYoy, 3.0, 0.1, -1.0),
        (MacroSeries::GrowthYoy, 2.0, 0.2, 1.0),
        (MacroSeries::CreditSpreadBps, 120.0, 4.0, -1.0),
    ] {
        for i in 0..PRINTS {
            let noise = ((i as f64 * 0.7548776662 + 0.3) % 1.0 - 0.5) * 2.0 * amplitude;
            let step = if i == PRINTS - 1 {
                supportive * 6.0 * amplitude
            } else {
                0.0
            };
            let at = start().saturating_sub(Duration::from_days((PRINTS - i) as i64 + 1));
            records.push(SensedRecord::Macro(Box::new(MacroObservation {
                series_id: series.series_id("US"),
                region: "US".to_string(),
                value: level + noise + step,
                unit: "pct".to_string(),
                reference_date: at,
                consensus: None,
                previous: None,
                is_revision: false,
                provenance: Provenance::synthetic("macro-fixture", at)
                    .with_licensing(LicensingClass::Synthetic),
                quality: DataQuality::default(),
                evidence_unretrievable: false,
            })));
        }
    }
    records
}

fn acme_declaration() -> Vec<SensedRecord> {
    vec![SensedRecord::CorporateAction(Box::new(CorporateAction {
        object_id: acme(),
        ex_date: start().saturating_add(Duration::from_days(30)),
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::CashDividend {
            amount: dec!("0.10"),
        },
        announced_at: start().saturating_sub(Duration::from_days(3)),
    }))]
}

#[test]
fn a_cycle_that_records_a_material_disagreement_still_releases_its_order() -> Result<()> {
    let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(BOOK));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let limits = LimitSet::new("federation-control")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        );
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        acme_universe()?,
        limits,
    )?;
    platform.observe(acme_macro());
    platform.observe(acme_tape());
    platform.observe(acme_declaration());
    let report = platform.run_cycle(start());

    // Premise: ACT released the order this scenario is built to release. A
    // refusal proves nothing about a platform that trades nothing at all.
    let released = report.stage(Stage::Act).expect("act ran").produced;
    assert_eq!(
        released,
        1,
        "the scenario releases its order with the federation present, and a federation verdict withholding sign-off fails here: {}",
        report.summarise()
    );

    // Premise: the federation formed a material disagreement in this very
    // cycle, so a verdict wired into ACT would have had something to act on.
    let reviews = federation_stage::replay(&platform)?;
    assert!(
        reviews
            .iter()
            .flat_map(|r| &r.arbitrations)
            .any(|a| a.disagreement.views.len() >= 2
                && a.disagreement.spread >= MATERIAL_DISAGREEMENT_SPREAD),
        "premise: no material disagreement was recorded in the releasing cycle"
    );

    // Advisory, structurally: the order went out and was booked anyway.
    assert_eq!(platform.orders().fills().len(), 1);
    assert!(!platform.positions_at_cost().is_empty());
    Ok(())
}
