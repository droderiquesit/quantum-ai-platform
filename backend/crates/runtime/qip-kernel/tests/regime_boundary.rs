//! Blueprint §9.4's first handling, end to end: "edges carry a confidence.
//! Low-confidence edges inform exploration, not sizing."
//!
//! The confidence half has shipped for a long while. The exploration half had
//! no feed at all: `ProbeKind::RegimeBoundary` — "enter a regime-boundary
//! trade, to learn how the causal edges behave at the transition" — was
//! declared and left unfed, and `qip_kernel::exploration`'s own module doc
//! said why. `Platform::regime_label` names the regime **in force**, never the
//! instant it changed, so a `RegimeBoundary` candidate would have been "a
//! probe sized against a figure nobody computed".
//!
//! This drives the whole path through the cycle rather than asserting about
//! the marker in isolation: a real temporal-precedence edge is written under
//! one regime, the instrument's quoted spread then blows out, and the DECIDE
//! stage's exploration desk is asked whether it funded a probe of the right
//! kind on the right subject. Both halves of the gate are proven — the pass
//! before the change funds no such probe, and the pass after it does —
//! because a feed that fires on every pass is indistinguishable from a
//! working one when only the firing side is checked.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_kernel::regime_transition::SUBJECT_PREFIX;
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Quote;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::causal::ConditionStanding;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> qip_core::ObjectId {
    qip_core::ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe(symbols: &[&str]) -> Universe {
    let mut universe = Universe::new();
    for symbol in symbols {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    *symbol,
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
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

fn platform(symbols: &[&str]) -> Result<Platform> {
    // A share is set aside deliberately: with none, the desk reports "no
    // budget" and every candidate — right or wrong — is unfundable, so the
    // assertion below would pass on a platform that never computed a
    // boundary at all.
    let config = PlatformConfig::default().with_exploration_share(dec!("0.05"));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(symbols),
        LimitSet::new("regime-boundary-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
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

fn bars_from_returns(symbol: &str, returns: &[f64], count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let open = price;
            price *= returns[i].exp();
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// A deterministic, bounded pseudo-random sequence — the same fixture shape
/// `causal_precedence.rs` uses, so the edge this test relies on is written by
/// the same production pass and not by a synthetic seed.
fn noise(seed: u64, count: usize, scale: f64) -> Vec<f64> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state % 1_000_000) as f64 / 1_000_000.0;
            (unit - 0.5) * 2.0 * scale
        })
        .collect()
}

/// A quote whose half-spread is `bps` basis points of a hundred-dollar mid.
fn quote(symbol: &str, at: Timestamp, bps: f64) -> SensedRecord {
    let half = 100.0 * bps / 20_000.0;
    SensedRecord::Quote(Quote {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        at,
        bid: Decimal::from_f64(100.0 - half).expect("a price"),
        ask: Decimal::from_f64(100.0 + half).expect("a price"),
        bid_size: dec!("1000"),
        ask_size: dec!("1000"),
        quality: DataQuality::default(),
    })
}

/// The exploration clause of a report's DECIDE stage.
fn decide_detail(report: &qip_kernel::cycle::CycleReport) -> String {
    let decide = report
        .stage(Stage::Decide)
        .expect("DECIDE runs on every cycle");
    assert!(decide.ran, "DECIDE did not run: {decide:?}");
    decide.detail.clone()
}

#[test]
fn a_subject_whose_regime_turns_becomes_the_regime_boundary_probe_the_desk_funds() -> Result<()> {
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }

    let mut platform = platform(&["AAA", "BBB"])?;
    platform.observe(bars_from_returns("AAA", &cause_returns, count));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));
    // Three tight quotes, so the median the spread ratio divides by is a
    // tight one. Fewer than three and `spread_ratio` answers `None` and the
    // regime could never turn on liquidity at all.
    for day in 0..3 {
        platform.observe(vec![quote(
            "BBB",
            start().saturating_sub(Duration::from_days(3 - day)),
            10.0,
        )]);
    }

    let first = platform.run_cycle(start());
    let settled_regime = platform.regime_context("obj-BBB");

    // Premise one: the production pass wrote an edge into the subject, and
    // conditioned it on the regime then in force. Without an edge there is
    // nothing for a regime-boundary probe to be uncertain about, and the
    // assertion below would be vacuous.
    let edges = platform.world().causal().edges().to_vec();
    let into_subject: Vec<_> = edges.iter().filter(|e| e.effect == "obj-BBB").collect();
    assert!(
        !into_subject.is_empty(),
        "premise: the temporal-precedence pass wrote an edge into obj-BBB, among {edges:?}"
    );
    assert!(
        into_subject
            .iter()
            .all(|e| e.in_regime(&settled_regime) == ConditionStanding::Holds),
        "premise: every edge into the subject was tested in the regime in force ({settled_regime})"
    );

    // Premise two, and the half a one-sided test would miss: a platform whose
    // regime has not turned funds no regime-boundary probe. A feed that
    // proposed one on every pass would satisfy the final assertion and would
    // be worthless.
    let quiet = decide_detail(&first);
    assert!(
        !quiet.contains("regime_boundary on"),
        "a probe of the boundary kind was funded on a pass where nothing crossed: {quiet}"
    );

    // The turn: one quote three hundred basis points wide against a
    // ten-basis-point median. The subject's market regime becomes illiquid,
    // which is a regime this subject's edges have never been tested under.
    platform.observe(vec![quote("BBB", start(), 300.0)]);
    let later = start().saturating_add(Duration::from_secs(60));

    let second = platform.run_cycle(later);
    let turned_regime = platform.regime_context("obj-BBB");
    assert_ne!(
        settled_regime, turned_regime,
        "premise: the subject's regime key actually changed; without that there is no boundary \
         and this test proves nothing"
    );

    let understood = second
        .stage(Stage::Understand)
        .expect("UNDERSTAND runs on every cycle");
    assert!(
        understood
            .detail
            .contains("crossed a regime boundary this pass"),
        "the UNDERSTAND stage did not report the crossing it marked: {}",
        understood.detail
    );

    // And the whole point: the desk funded a probe of the boundary kind, on
    // the subject that crossed. The subject is matched in full, prefix and
    // all, rather than by a bare instrument id — the plan line names every
    // selected probe, and `contains("obj-BBB")` would be true of a capacity
    // probe on the same instrument.
    let turned = decide_detail(&second);
    let expected = format!("regime_boundary on {SUBJECT_PREFIX}obj-BBB");
    assert!(
        turned.contains(&expected),
        "the desk funded no {expected}; §9.4's exploration handling is unfed again: {turned}"
    );
    Ok(())
}
