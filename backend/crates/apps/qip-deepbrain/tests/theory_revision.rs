//! R12-EXPAND-6044: the theory delta the kernel journals on a contradicted
//! veto reaches the deep brain's research loop as a contradiction gap — the
//! revision task — naming the rule and wanting the model.
//!
//! The failure prevented: the kernel could record that its theory was
//! contradicted and nothing read the record, so no research was ever queued
//! on it. The test drives the real seam — a veto in REASON, the grade in
//! LEARN, then one sweep of the loop as `main.rs` builds it — and asserts
//! its premise (a delta on the log) before the hop.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::expansion::GapClass;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_deepbrain::discovery::{DiscoveryConfig, DiscoveryDesk};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop};
use qip_expansion::detectors::Origin;
use qip_expansion::gap::RaisedGap;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::cycle::Stage;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_storage::MemoryBlobStore;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// The kernel suite's review floor (`valuation_seam.rs`), for its reason:
/// the veto is reached only through a thesis review approved, and the
/// shipped 0.50 is not cleared on a synthetic tape. Not the shipped value.
const REVIEW_FLOOR: f64 = 0.10;

/// `AAA` with a million-share lot: a hundred million against a million of
/// cash, so the pack's `cash_short` stop vetoes the review-approved thesis.
fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object("AAA"),
                "AAA",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .geography("US")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .lot_size(dec!("1000000"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .unwrap(),
        )
        .unwrap();
    universe
}

fn platform() -> Platform {
    let config = PlatformConfig {
        initial_equity: dec!("1000000"),
        review: ReviewPolicy {
            minimum_surviving_confidence: REVIEW_FLOOR,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    };
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let limits = LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    );
    Platform::new(config, context, Telemetry::silent(), universe(), limits).unwrap()
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).unwrap(),
        high: Decimal::from_f64(open.max(close) * 1.002).unwrap(),
        low: Decimal::from_f64(open.min(close) * 0.998).unwrap(),
        close: Decimal::from_f64(close).unwrap(),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

const TAPE: usize = 120;
const DROP: f64 = -0.09;

/// A quiet series that drops nine percent on its final bar, so the return
/// detector reads an `Undervalued` claim off the drop.
fn tape(symbol: &str) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let drop = if i == TAPE - 1 { DROP } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// Daily closes back at the pre-drop level after `start()`, so the claim's
/// reversion comes true and the grader credits it.
fn recovered(symbol: &str, days: i64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            bar(
                symbol,
                start().saturating_add(Duration::from_days(day)),
                100.0,
                100.0,
            )
        })
        .collect()
}

/// The environment a deployment that set nothing hands `main.rs`.
fn unset(_: &str) -> Option<String> {
    None
}

fn deployed() -> ResearchLoop {
    ResearchLoop::new(
        ResearchConfig::from_lookup(&unset).unwrap(),
        BTreeMap::new(),
    )
    .unwrap()
    .with_package_store(Arc::new(MemoryBlobStore::new()))
}

fn deployed_bounds() -> (BTreeSet<String>, BTreeSet<String>) {
    let desk = DiscoveryDesk::new(DiscoveryConfig::from_lookup(&unset).unwrap(), vec![]);
    (
        desk.licensed_sources().clone(),
        desk.eligible_jurisdictions().clone(),
    )
}

fn queued_gaps(loop_: &ResearchLoop) -> Vec<RaisedGap> {
    loop_
        .desk()
        .queue()
        .ranked()
        .iter()
        .filter_map(|entry| entry.origin().cloned())
        .collect()
}

/// Mutation: delete the `theory_contradictions` call in `sweep_over` — the
/// delta is counted on the round and no gap names the rule, so the
/// `len() == 1` assertion fails; or make `Platform::theory_deltas` return
/// an empty vector — `round.theory_deltas` is 0 and the premise fails.
#[test]
fn a_journaled_theory_delta_reaches_the_sweep_as_a_contradiction_gap_naming_the_rule() {
    let mut platform = platform();
    platform.observe(tape("AAA"));
    let first = platform.run_cycle(start());
    let reason = first.stage(Stage::Reason).unwrap();
    assert!(
        reason
            .problems
            .iter()
            .any(|p| p.contains("vetoed by the symbolic rule pack")),
        "premise: the pack vetoed the thesis: {:?}",
        reason.problems
    );
    let resolves_at = platform
        .predictions()
        .first()
        .expect("premise: a claim was recorded")
        .proposition
        .resolves_at;
    let days = resolves_at.since(start()).as_nanos() / Duration::from_days(1).as_nanos() + 2;
    platform.observe(recovered("AAA", days));
    let settle = resolves_at.saturating_add(Duration::from_hours(1));
    platform.run_cycle(settle);
    let deltas = platform.theory_deltas().unwrap();
    assert_eq!(deltas.len(), 1, "premise: one delta on the log: {deltas:?}");
    let rule = deltas[0]
        .rules
        .iter()
        .find(|r| r.as_str() == "r61-cash-short-of-one-lot")
        .expect("premise: the delta names the cash stop")
        .clone();

    let mut loop_ = deployed();
    let (sources, jurisdictions) = deployed_bounds();
    let round = loop_
        .sweep(&platform, settle, &sources, &jurisdictions)
        .unwrap();
    assert_eq!(round.theory_deltas, 1, "{round:?}");
    assert_eq!(round.theory_deltas_refused, None, "{round:?}");
    assert!(
        round.describe().contains("theory deltas 1"),
        "{}",
        round.describe()
    );

    let gaps = queued_gaps(&loop_);
    let theory: Vec<&RaisedGap> = gaps
        .iter()
        .filter(|g| {
            g.signal.evidence.first().map(String::as_str)
                == Some(Origin::TheoryContradiction.as_str())
        })
        .collect();
    assert_eq!(
        theory.len(),
        deltas[0].rules.len(),
        "one revision task per rule the delta names: {gaps:?}"
    );
    let cash = theory
        .iter()
        .find(|g| {
            g.signal
                .evidence
                .iter()
                .any(|e| e == &format!("rule:{rule}"))
        })
        .unwrap_or_else(|| panic!("a task names the cash stop: {theory:?}"));
    assert_eq!(cash.signal.gap_class, GapClass::Model);
    assert!(
        cash.signal
            .evidence
            .iter()
            .any(|e| e == &format!("hypothesis:{}", deltas[0].hypothesis_id)),
        "the task cites the contradicted hypothesis: {:?}",
        cash.signal.evidence
    );
    assert!(
        cash.signal.observation.contains("is not moved"),
        "the task says it proposes and does not apply: {}",
        cash.signal.observation
    );
}
