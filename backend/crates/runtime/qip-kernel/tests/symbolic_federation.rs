//! The World Model Federation and the symbolic reasoners, in the cycle
//! (blueprint §9.3 and §9.4).
//!
//! Each seam is proven by driving a real cycle and reading the record back
//! from the journal alone — the property the register asks for is not that
//! the code exists but that a run left a replayable trace of it.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::federation_stage::{self, ON_CHAIN, REQUIRED_FAMILIES};
use qip_kernel::platform::Platform;
use qip_kernel::symbolic_stage::{self, Step};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::federation::Decision;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

// --- fixtures, the shape `tests/learning.rs` feeds ----------------------------

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

#[test]
fn every_cycle_journals_one_federation_review_and_each_replays_and_reproduces_its_arbitrations()
-> Result<()> {
    // The failure this guards: the federation was built, tested and
    // constructed by no composition root, so the cycle had one world model
    // and nothing in the log could say the models disagreed.
    let mut platform = platform()?;
    let mut records = bars("AAA", 120);
    records.extend(bars("BBB", 120));
    platform.observe(records);
    for step in 0..3 {
        let at = start().saturating_add(Duration::from_mins(5 * step));
        let report = platform.run_cycle(at);
        let understand = report.stage(Stage::Understand).expect("understand ran");
        assert!(
            understand.detail.contains("; federation of "),
            "UNDERSTAND did not report the federation: {}",
            understand.detail
        );
    }
    let reviews = federation_stage::replay(&platform)?;
    assert_eq!(reviews.len(), 3, "one review per cycle, read from the log");
    let first = &reviews[0];
    // Premise: the population exists and every family is covered.
    assert_eq!(first.population.len(), 7, "{:?}", first.population);
    for family in REQUIRED_FAMILIES {
        assert!(
            !first.uncovered.contains(&family.to_string()),
            "{family} uncovered"
        );
    }
    // No chain was observed, so the on-chain member is the one silent member
    // — present, unexpired, and honest that it produced nothing.
    assert_eq!(first.silent, vec![ON_CHAIN.to_string()]);
    let instrument: Vec<_> = first
        .arbitrations
        .iter()
        .filter(|a| a.proposition.contains(":next-close-up@"))
        .collect();
    assert_eq!(instrument.len(), 2, "one proposition per instrument");
    for a in &instrument {
        assert_eq!(a.disagreement.views.len(), 5, "{a:?}");
        // Unscored members cannot be weighed, and the record says so rather
        // than averaging them.
        assert!(matches!(a.decision, Decision::RequestMoreInformation(_)));
        assert_eq!(a.arbitrated_probability, None);
    }
    Ok(())
}

/// A steady climb: one percent a day with a little deterministic noise, so
/// the short-window momentum member and the long-window reversion member read
/// the same tape and answer opposite questions.
fn trend(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.002;
            let open = price;
            price *= 1.01 + noise;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// R12-MODEL-4009 and R12-MODEL-3091 through a real cycle, read back from the
/// log alone: two members holding opposite views produce a recorded
/// disagreement — both views kept, the spread measured, no merged number —
/// and that material disagreement becomes a research task citing the record.
///
/// The failure this prevents is the one an ensemble invites: averaging a
/// near-certain "up" and a near-certain "down" into a confident-looking 0.5,
/// which says "no view" when the truth is "two strong views, irreconciled".
#[test]
fn opposite_member_views_are_journaled_as_a_disagreement_and_raise_research_citing_it() -> Result<()>
{
    use qip_world_model::federation::Event;
    let mut platform = platform()?;
    platform.observe(trend("AAA", 120));
    platform.run_cycle(start());

    let reviews = federation_stage::replay(&platform)?;
    assert_eq!(reviews.len(), 1, "one review, read from the log");
    let review = &reviews[0];
    let arbitration = review
        .arbitrations
        .iter()
        .find(|a| a.proposition.contains(":next-close-up@"))
        .expect("the instrument was arbitrated");
    let view = |model: &str| {
        arbitration
            .disagreement
            .views
            .iter()
            .find(|v| v.model == model)
            .unwrap_or_else(|| panic!("{model} has no view: {arbitration:?}"))
            .probability
    };
    let (momentum, reversion) = (
        view(federation_stage::MOMENTUM),
        view(federation_stage::REVERSION),
    );
    // Premise: the two members really are on opposite sides of even odds.
    assert!(
        momentum > 0.5 && reversion < 0.5,
        "momentum {momentum}, reversion {reversion}"
    );
    assert!(
        arbitration.disagreement.spread >= momentum - reversion,
        "the spread covers both views: {arbitration:?}"
    );
    assert!(
        arbitration.disagreement.spread
            >= qip_world_model::federation::MATERIAL_DISAGREEMENT_SPREAD,
        "{arbitration:?}"
    );
    assert_eq!(
        arbitration.arbitrated_probability, None,
        "no merged number stands in for two irreconciled views: {arbitration:?}"
    );

    // The research task, journaled in the same review, cites the decision
    // record the disagreement is in — by absolute position.
    let task = arbitration
        .research
        .as_ref()
        .unwrap_or_else(|| panic!("a material disagreement raised no research: {arbitration:?}"));
    assert!(
        review
            .events
            .iter()
            .any(|e| matches!(e, Event::ResearchRaised(t) if t == task)),
        "the task is in the journaled events"
    );
    let cited = task
        .cites
        .checked_sub(review.journal_base)
        .and_then(|i| review.events.get(i))
        .unwrap_or_else(|| panic!("task cites {} outside the review", task.cites));
    match cited {
        Event::Decided {
            proposition,
            disagreement,
            ..
        } => {
            assert_eq!(proposition, &arbitration.proposition);
            assert_eq!(disagreement, &arbitration.disagreement);
        }
        other => panic!("the task cites {other:?}, not the decision"),
    }
    assert!(
        task.models
            .contains(&federation_stage::MOMENTUM.to_string())
    );
    assert!(
        task.models
            .contains(&federation_stage::REVERSION.to_string())
    );
    Ok(())
}

#[test]
fn a_formed_hypothesis_is_journaled_with_a_symbolic_derivation_whose_rule_verdict_reproduces()
-> Result<()> {
    // The failure this guards: rules, timeline, network, constraint solver,
    // lifecycle, abduction and intervention were each tested and none was
    // called by REASON, so no hypothesis carried a derivation a person could
    // re-run.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());
    // Premise: a hypothesis was formed — the same fixture the self-model
    // suite relies on to write a claim.
    assert!(
        !platform.predictions().is_empty(),
        "no hypothesis was formed:\n{}",
        report.summarise()
    );
    let reason = report.stage(Stage::Reason).expect("reason ran");
    assert!(
        reason.detail.contains("; symbolic: rules "),
        "REASON did not report the derivation: {}",
        reason.detail
    );
    let derivations = symbolic_stage::replay(&platform)?;
    assert_eq!(derivations.len(), 1, "one derivation per formed hypothesis");
    let derivation = &derivations[0];
    assert!(derivation.hypothesis_id.starts_with("hyp-1-"));
    assert!(
        matches!(derivation.rules.verdict, Step::Derived(_)),
        "{:?}",
        derivation.rules.verdict
    );
    assert!(derivation.rules.verified);
    assert!(derivation.lifecycle.derived().is_some());
    assert!(derivation.posterior.derived().is_some());
    assert!(derivation.acting.derived().is_some());
    assert!(
        derivation
            .temporal
            .links
            .iter()
            .all(|link| matches!(link.admitted, Step::Derived(()))),
        "{:?}",
        derivation.temporal.links
    );
    Ok(())
}
