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
    // Premise for the `all` below: anomaly -> hypothesis -> deadline is
    // always posed, so an empty list would mean the timeline never ran.
    assert!(
        derivation.temporal.links.len() >= 2,
        "{:?}",
        derivation.temporal.links
    );
    assert!(
        derivation
            .temporal
            .links
            .iter()
            .all(|link| matches!(link.admitted, Step::Derived(()))),
        "{:?}",
        derivation.temporal.links
    );
    // R12-REASON-1133: the timeline orders the anomaly before the hypothesis
    // it gave rise to. In the cycle the two can share an instant, and then
    // only the admitted causal link orders them — so a link posed backwards
    // is admitted just the same and only this ordering tells the difference.
    assert_eq!(
        derivation.temporal.anomaly_to_hypothesis,
        qip_reasoning_engine::temporal::Order::Before,
        "{:?}",
        derivation.temporal
    );
    // R12-REASON-1136: the constraint solver's answer follows the rule
    // pack's eligibility — acting is satisfiable exactly when the pack
    // derived eligibility, and otherwise the conflict names the rule that
    // blocks it. An answer that ignored either is not a derivation.
    let eligible = matches!(
        &derivation.rules.verdict,
        Step::Derived(v) if v.facts.get("eligible") == Some(&qip_reasoning_engine::rules::Value::Bool(true))
    );
    match derivation.acting.derived().expect("acting was derived") {
        qip_reasoning_engine::constraint::Answer::Satisfied(_) => {
            assert!(
                eligible,
                "acting was satisfiable for an ineligible hypothesis"
            )
        }
        qip_reasoning_engine::constraint::Answer::Unsatisfiable { conflict } => {
            assert!(!eligible, "acting was blocked for an eligible hypothesis");
            assert!(
                conflict.iter().any(|c| c == "acting-requires-eligibility"),
                "{conflict:?}"
            );
        }
    }
    Ok(())
}

/// `count` deterministic daily bars ending the day before `end`, with a 2%
/// jump every seventeenth bar so momentum and reversion have a real move to
/// disagree about rather than `bars`' single late jump.
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
fn competing_world_models_are_scored_against_reality_and_a_material_disagreement_reaches_the_log_as_a_research_task()
-> Result<()> {
    // R12-WORLD-9004, -1003, -1023, -1129, -1130, -3039, -3040 and -1181,
    // read from the hash-chained log a shipped binary's Platform writes.
    //
    // The failure this guards is the federation that exists and decides
    // nothing: the three-cycle test above sees only unscored members asking
    // for more information, which is also exactly what a federation whose
    // forecasts were never scored, or whose disagreements were averaged,
    // would journal. Here reality answers every pass, so members carry a
    // Brier record, arbitration weighs all four dimensions, and a spread over
    // the material threshold must become a research task citing the record.
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
    for (step, (a, b)) in later_aaa.into_iter().zip(later_bbb).enumerate() {
        // One new close per instrument per pass, so the previous pass's
        // forecasts are answered by an observation that did not exist when
        // they were made.
        platform.observe(vec![a, b]);
        let at = start().saturating_add(Duration::from_days(step as i64 + 1));
        platform.run_cycle(at);
    }

    let reviews = federation_stage::replay(&platform)?;
    assert_eq!(reviews.len() as u64, cycles, "one review per cycle");

    // More than one model, with distinct scopes, live in the cycle (1003,
    // 1129): five tape members on every instrument proposition.
    let last = reviews.last().expect("reviews");
    let scopes: std::collections::BTreeSet<String> = last
        .population
        .iter()
        .map(|m| format!("{:?}", m.scope))
        .collect();
    assert_eq!(
        scopes.len(),
        last.population.len(),
        "two members share a scope"
    );
    assert!(last.population.len() >= 5, "{:?}", last.population);

    // Reality answered: forecasts were resolved, and members are scored.
    let resolved: usize = reviews.iter().map(|r| r.resolved.len()).sum();
    assert!(
        resolved > 0,
        "no forecast was ever resolved against a later close"
    );
    let scored: Vec<_> = reviews
        .iter()
        .flat_map(|r| &r.arbitrations)
        .filter(|a| a.proposition.contains(":next-close-up@"))
        .filter(|a| {
            a.disagreement
                .views
                .iter()
                .all(|v| v.predictive_performance.is_some())
        })
        .collect();
    assert!(
        !scored.is_empty(),
        "no instrument arbitration ever weighed scored members"
    );
    for a in &scored {
        // Every view kept, minority included, on all four dimensions, with
        // the composite present only because performance is (1130, 3040).
        assert_eq!(a.disagreement.views.len(), 5, "{a:?}");
        for v in &a.disagreement.views {
            assert!(v.evidence_fit.is_finite() && v.causal_coherence.is_finite());
            assert!(v.uncertainty.is_finite() && v.composite.is_some(), "{v:?}");
        }
        // A scored arbitration is a decision, not a request for scores.
        assert!(
            !matches!(a.decision, Decision::RequestMoreInformation(_)),
            "scored members still asked for more information: {a:?}"
        );
        // An arbitrated number exists only beside its disagreement (3039).
        if a.arbitrated_probability.is_some() {
            assert!(matches!(a.decision, Decision::Merge), "{a:?}");
        }
    }

    // A material disagreement becomes research, citing the record (9004).
    let material: Vec<_> = reviews
        .iter()
        .flat_map(|r| r.arbitrations.iter().map(move |a| (r, a)))
        .filter(|(_, a)| a.research.is_some())
        .collect();
    assert!(
        !material.is_empty(),
        "no arbitration raised a research task; spreads seen: {:?}",
        reviews
            .iter()
            .flat_map(|r| &r.arbitrations)
            .map(|a| a.disagreement.spread)
            .collect::<Vec<_>>()
    );
    for (review, a) in &material {
        let task = a.research.as_ref().expect("filtered");
        assert!(
            task.spread >= qip_world_model::federation::MATERIAL_DISAGREEMENT_SPREAD,
            "{task:?}"
        );
        assert_eq!(task.spread.to_bits(), a.disagreement.spread.to_bits());
        assert_eq!(task.models.len(), a.disagreement.views.len());
        // The task reached the log in the federation's own journal, and the
        // record it cites is the decision it came from.
        let raised = review.events.iter().any(
            |e| matches!(e, qip_world_model::federation::Event::ResearchRaised(t) if t == task),
        );
        assert!(raised, "the research task is not in the journaled events");
        let cited = task
            .cites
            .checked_sub(review.journal_base)
            .and_then(|i| review.events.get(i));
        assert!(
            matches!(
                cited,
                Some(qip_world_model::federation::Event::Decided { proposition, .. })
                    if *proposition == task.proposition
            ),
            "the task cites {:?}, not its decision",
            cited
        );
    }
    Ok(())
}
