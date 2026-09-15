//! §8.1: the authority a thesis was settled against is a node in the world
//! model, written from the LEARN stage that graded it.
//!
//! The gap this closes: `ResolutionSource` existed only on a `Proposition` in
//! `qip-prediction`. Nothing could traverse to it, so "who settled this claim"
//! was answerable only by knowing which struct held the field — and an
//! explanation that depends on knowing where to look is not one the audit
//! trail can reproduce.
//!
//! The property that would rot silently is the second one. The node carries
//! the **proposition's** instants, not the settlement's. A cycle grading a
//! thesis months after the claim was made must not report the authority as
//! first known on the day of the grading: `Fact::holds` would then hide the
//! edge from every replay before that instant, and the decision the platform
//! made back then would have no recorded provenance at all.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
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
use qip_world_model::graph::NodeKind;
use qip_world_model::relationship::RelationshipKind;
use qip_world_model::resolution_source::RESOLUTION_SOURCE_PREFIX;

// --- fixtures, the same shape `learning.rs` feeds ---------------------------

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

/// A price series with a jump partway through, so the detectors have something
/// real to find — the same shape the kernel's founding test feeds.
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

// --- the seam ---------------------------------------------------------------

#[test]
fn a_cycle_that_grades_a_thesis_puts_its_resolving_authority_in_the_world_model() -> Result<()> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());

    // Premise one: the first cycle made a claim. Without this everything
    // below would be asserting about an empty registry.
    assert!(
        !platform.predictions().is_empty(),
        "no claim was written, so there is nothing to settle:\n{}",
        first.summarise()
    );
    let prediction = platform.predictions()[0].clone();
    assert!(prediction.is_open(), "a fresh claim must be open");
    let hypothesis = prediction.hypothesis.clone();
    let expected_id = format!(
        "{RESOLUTION_SOURCE_PREFIX}:{}",
        prediction.proposition.source.name
    );

    // Premise two: nothing of this kind is in the graph before the grading,
    // so a passing assertion afterwards is about this cycle's write and not
    // about a node some other seam had already put there.
    assert!(
        platform
            .world()
            .graph()
            .nodes_of_kind(NodeKind::ResolutionSource)
            .is_empty(),
        "a resolution source was in the graph before anything resolved"
    );

    // Move the world far enough past the reference that the verdict is
    // informative either way, exactly as `learning.rs` does: twenty swinging
    // bars move every observable a claim here can name at once.
    let horizon = prediction.proposition.resolves_at;
    assert!(
        horizon > start(),
        "a claim resolving in the past is not a claim"
    );
    let swings: Vec<SensedRecord> = (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            bar("AAA", at, open, close)
        })
        .collect();
    platform.observe(swings);

    // Premise three: the settlement instant and the proposition's own instant
    // are different instants. If they coincided, the stamping assertion below
    // would pass against a node stamped with either and would guard nothing.
    let settled_at = horizon.saturating_add(Duration::from_mins(1));
    assert!(
        settled_at > prediction.recorded_at,
        "the grading runs at {} and the claim was recorded at {}; a test that \
         cannot tell the two apart proves nothing about which one was written",
        settled_at.to_rfc3339(),
        prediction.recorded_at.to_rfc3339()
    );

    platform.run_cycle(settled_at);

    // The authority is a node, once, under the name the proposition gave it.
    let world = platform.world();
    let sources = world.graph().nodes_of_kind(NodeKind::ResolutionSource);
    assert_eq!(
        sources.len(),
        1,
        "the graded thesis put {} resolving authorities in the graph",
        sources.len()
    );
    assert_eq!(sources[0].id, expected_id);
    assert_eq!(
        sources[0].attributes.get("authority").map(String::as_str),
        Some(prediction.proposition.source.kind.as_str()),
        "the node does not carry the kind of authority the proposition named"
    );

    // The bitemporal claim: the proposition's own instant, not the cycle's.
    assert_eq!(
        sources[0].recorded_at,
        prediction.recorded_at,
        "the authority is stamped {} — the settlement's clock — rather than \
         the proposition's own {}",
        sources[0].recorded_at.to_rfc3339(),
        prediction.recorded_at.to_rfc3339()
    );

    // And the consequence that makes it matter: a replay standing at the
    // instant the claim was made already sees who would settle it.
    let hops = world.graph().neighbours(
        &hypothesis,
        Some(RelationshipKind::ResolvedBy),
        prediction.recorded_at,
        prediction.recorded_at,
    );
    assert_eq!(
        hops.len(),
        1,
        "standing at the instant the claim was recorded, the platform cannot \
         traverse from the thesis to the authority that settles it"
    );
    assert_eq!(hops[0].relationship.to, expected_id);
    Ok(())
}
