//! A recorded world event and its measured tick reaction narrow REASON
//! (R12-TICK-2083, ADR 0151 decision 3).
//!
//! `fusion.rs` proves input-to-belief and `outside_prior.rs` proves
//! prior-to-confidence. Neither proves the chain: a seam dropped between them
//! (the shape the register once claimed, "computed and dropped") would pass
//! both. This drives the recorded episode through `fuse`, hands the result to
//! the platform exactly as the deep brain's node does, and reads the
//! confidence the claim was written at through the production cycle.

#![allow(clippy::panic_in_result_fn)]
// Exact comparison is the claim: a prior that leans for the hypothesis must
// leave the confidence the same number, bit for bit.
#![allow(clippy::float_cmp)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::fusion::{FusionConfig, MarketTrace, TracePoint, fuse};
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::graph::{EntityKind, Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

// --- fixtures, the same shape `learning.rs` feeds -----------------------------

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

fn fresh() -> Result<Platform> {
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
/// something real to find.
fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

fn at(s: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + s)
}

fn config() -> FusionConfig {
    FusionConfig {
        max_hops: 2,
        reaction_threshold_bps: 20.0,
        reaction_window: Duration::from_secs(120),
        expect_reaction_above: 0.15,
        weak_evidence_below: 0.4,
        max_events: 16,
    }
}

/// An event about AAA expecting `direction`, recorded at `at(120)`.
fn graph(direction: &str) -> Result<KnowledgeGraph> {
    let mut graph = KnowledgeGraph::new();
    graph.add_node(
        Node::event("evt-news", "evt-news", at(100), at(120))
            .with_attribute("expected_direction", direction)
            .with_attribute("evidence_quality", "0.9"),
    )?;
    graph.add_node(Node::entity("aaa", EntityKind::Company, "aaa", at(0)))?;
    graph.add_node(
        Node::new("inst-aaa", NodeKind::FinancialObject, "inst-aaa", at(0))
            .with_attribute("object_id", "obj-AAA"),
    )?;
    graph.assert_fact(Fact::new(
        Relationship::new(
            "evt-news",
            "aaa",
            RelationshipKind::ConcernsEntity,
            1.0,
            "wire:1",
        ),
        at(100),
        at(120),
        0.9,
    )?);
    graph.assert_fact(Fact::new(
        Relationship::new(
            "aaa",
            "inst-aaa",
            RelationshipKind::Issues,
            1.0,
            "registry:1",
        ),
        at(0),
        at(0),
        1.0,
    )?);
    Ok(graph)
}

/// AAA's mid moves 100 bps in `sign`'s direction at `at(150)`.
fn traces(sign: f64) -> Vec<MarketTrace> {
    vec![MarketTrace {
        instrument: "obj-AAA".into(),
        venue: "XNYS".into(),
        points: (0..=60)
            .map(|i| TracePoint {
                at: at(i * 10),
                mid: if i * 10 < 150 { 100.0 } else { 100.0 + sign },
                spread_bps: 2.0,
                depth: None,
                imbalance: None,
            })
            .collect(),
    }]
}

/// One REASON over the jump tape, handing the platform what the node hands it.
fn claim_confidence(priors: Vec<qip_kernel::fusion::OutsidePrior>) -> Result<(f64, f64)> {
    let mut platform = fresh()?;
    platform.set_outside_priors(priors)?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());
    let claim = platform
        .predictions()
        .first()
        .and_then(|prediction| prediction.claim.as_ref())
        .unwrap_or_else(|| panic!("premise: the cycle formed a claim:\n{}", report.summarise()));
    Ok((claim.confidence, claim.expected_move_bps))
}

#[test]
fn a_recorded_event_and_its_tick_reaction_narrow_the_claim_they_contradict_and_not_the_one_they_support()
-> Result<()> {
    let (baseline, expected_move) = claim_confidence(Vec::new())?;
    assert!(
        baseline > 0.0,
        "premise: an unnarrowed claim has a confidence"
    );
    assert!(
        expected_move != 0.0,
        "premise: the claim expects a signed move"
    );
    let claim_sign = expected_move.signum();

    // The event expects the instrument to go the claim's own way: it supports.
    let sign = claim_sign;
    let direction = if sign > 0.0 { "up" } else { "down" };
    let state = fuse(&graph(direction)?, &traces(sign), at(600), &config())?;
    assert!(
        !state.outside_priors().is_empty(),
        "premise: the episode produced a belief to hand REASON"
    );
    let (supported, _) = claim_confidence(state.outside_priors())?;
    assert_eq!(
        supported, baseline,
        "a reaction for the claim moved its confidence"
    );

    // The event expects the opposite: the same measured reaction contradicts.
    let sign = -claim_sign;
    let direction = if sign > 0.0 { "up" } else { "down" };
    let state = fuse(&graph(direction)?, &traces(sign), at(600), &config())?;
    let posterior = state.posteriors()["obj-AAA"];
    assert!(
        posterior > 0.5,
        "premise: the reaction confirmed a strong prior: {posterior}"
    );
    let (narrowed, _) = claim_confidence(state.outside_priors())?;
    let multiplier = 1.0 - (2.0 * posterior - 1.0);
    assert!(
        (narrowed - baseline * multiplier).abs() < 1e-9,
        "the fused posterior {posterior} must scale {baseline} by {multiplier}, got {narrowed}"
    );

    // No tick reaction measured: nothing to hand REASON, so it is unnarrowed.
    let silent = fuse(&graph(direction)?, &[], at(600), &config())?;
    let (without_ticks, _) = claim_confidence(silent.outside_priors())?;
    assert_eq!(
        without_ticks, baseline,
        "no measured reaction still narrowed REASON"
    );
    Ok(())
}
