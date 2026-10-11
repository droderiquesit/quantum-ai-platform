//! R12-GOV-6121: every DECIDE journals the code, policy, model, data and
//! control versions it ran under, and an episode carries the world model's
//! version rather than `None`.
//!
//! Each test names the mutation that should make it fail.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::decision_record::{self, DecisionVersions};
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::causal::{CausalEdge, CausalGraph, Mechanism};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

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
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn limits(weight: f64) -> LimitSet {
    LimitSet::new("kernel-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: weight },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

fn platform(weight: f64) -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        limits(weight),
    )
}

fn bars(count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object("AAA"),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).expect("a price"),
                high: Decimal::from_f64(open.max(price) * 1.002).expect("a price"),
                low: Decimal::from_f64(open.min(price) * 0.998).expect("a price"),
                close: Decimal::from_f64(price).expect("a price"),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

/// A DECIDE leaves one versions record, all five filled, in the chain.
/// Mutation: delete the `decision_record::journal` call in `stage_decide` —
/// nothing is recorded and the first assertion fails.
#[test]
fn every_decide_journals_the_five_versions_it_ran_under() -> Result<()> {
    let mut platform = platform(0.10)?;
    assert!(decision_record::recorded(&platform)?.is_empty(), "premise");
    platform.run_cycle(start());
    platform.run_cycle(start().saturating_add(Duration::from_mins(1)));
    let recorded = decision_record::recorded(&platform)?;
    assert_eq!(recorded.len(), 2, "one per DECIDE");
    for (index, versions) in recorded.iter().enumerate() {
        assert_eq!(versions.cycle, index as u64 + 1);
        assert!(versions.code_version.starts_with("qip-kernel "));
        assert!(versions.policy_version.starts_with("limits:sha256:"));
        assert!(versions.model_version.starts_with("world-model:sha256:"));
        assert!(versions.data_version.starts_with("log:"));
        assert!(versions.control_version.contains("kill_switch:false"));
    }
    // The second DECIDE read a longer log than the first.
    assert_ne!(recorded[0].data_version, recorded[1].data_version);
    assert!(platform.event_log().verify_chain().is_ok());
    Ok(())
}

/// The policy version is a fact about the limits. Mutation: hash a constant
/// in place of the limit set — the two versions compare equal.
#[test]
fn a_different_limit_set_is_a_different_policy_version() -> Result<()> {
    let tight = decision_record::versions(&platform(0.05)?, start())?;
    let loose = decision_record::versions(&platform(0.10)?, start())?;
    let again = decision_record::versions(&platform(0.10)?, start())?;
    assert_eq!(
        loose.policy_version, again.policy_version,
        "premise: deterministic"
    );
    assert_ne!(tight.policy_version, loose.policy_version);
    Ok(())
}

/// A record missing any one version does not replay. Mutation: give any
/// field of `DecisionVersions` a `#[serde(default)]` — its removal then
/// decodes.
#[test]
fn a_versions_record_missing_any_one_identifier_does_not_decode() -> Result<()> {
    let versions = decision_record::versions(&platform(0.10)?, start())?;
    let whole = serde_json::to_value(&versions)?;
    assert!(
        serde_json::from_value::<DecisionVersions>(whole.clone()).is_ok(),
        "premise"
    );
    for field in [
        "code_version",
        "policy_version",
        "model_version",
        "data_version",
        "control_version",
    ] {
        let mut dropped = whole.clone();
        dropped.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<DecisionVersions>(dropped).is_err(),
            "{field}"
        );
    }
    Ok(())
}

/// The model version moves with the graph and only with it. Mutation: hash
/// the edge count instead of the edges — re-estimating one edge's strength
/// then leaves the version unchanged.
#[test]
fn the_world_model_version_changes_when_an_edge_changes_and_not_otherwise() -> Result<()> {
    let edge = |strength: f64| {
        CausalEdge::new(
            "AAA",
            "BBB",
            Mechanism::SupplyChain,
            strength,
            Duration::from_days(1),
            start(),
        )
    };
    let empty = CausalGraph::new();
    let mut one = CausalGraph::new();
    one.add(edge(0.4)?);
    let mut same = CausalGraph::new();
    same.add(edge(0.4)?);
    let mut stronger = CausalGraph::new();
    stronger.add(edge(0.6)?);
    let v = |g: &CausalGraph| decision_record::model_version(g, start());
    assert_eq!(
        v(&one),
        v(&same),
        "premise: a version is a function of the edges"
    );
    assert_ne!(v(&empty), v(&one));
    assert_ne!(v(&one), v(&stronger));
    Ok(())
}

/// The episode REASON forms carries the world model's version. Mutation:
/// restore `model_version: None` at formation in `platform.rs`.
#[test]
fn an_episode_formed_by_the_cycle_carries_the_world_model_version() -> Result<()> {
    let mut platform = platform(0.10)?;
    platform.observe(bars(120));
    platform.run_cycle(start());
    assert!(
        !platform.predictions().is_empty(),
        "premise: the cycle made a claim, so an episode was formed"
    );
    let horizon = platform.predictions()[0].proposition.resolves_at;
    // Twenty swinging bars up to the horizon, as the episodic suite feeds,
    // so the claim resolves informatively and LEARN remembers it.
    let swings: Vec<SensedRecord> = (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object("AAA"),
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
        })
        .collect();
    platform.observe(swings);
    let later = horizon.saturating_add(Duration::from_mins(1));
    platform.run_cycle(later);
    // Read a second after the resolving cycle, as the episodic suite does:
    // LEARN stamps the episode `known_at = later`, and the store's one read
    // rule is *strictly* before (`EpisodeStore::episodes`), so an episode is
    // not knowable at the instant it was written. Reading at `later` itself
    // found nothing although LEARN reported "1 episode(s) remembered".
    let read_at = later.saturating_add(Duration::from_secs(1));
    let remembered: Vec<_> = platform.remembered_episodes(read_at).collect();
    assert!(
        !remembered.is_empty(),
        "premise: LEARN remembered the resolved episode"
    );
    for episode in remembered {
        let version = episode
            .model_version
            .as_deref()
            .expect("an episode formed by the cycle carries a model version");
        assert!(version.starts_with("world-model:sha256:"), "{version}");
    }
    Ok(())
}
