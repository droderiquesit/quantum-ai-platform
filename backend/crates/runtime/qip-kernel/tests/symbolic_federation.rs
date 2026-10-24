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
    // Ten members plus the one domain member the platform now declares for
    // the universe's single asset class (MODEL-4009): the composition names
    // the classes itself, so a deployed cycle carries the member too.
    assert_eq!(first.population.len(), 11, "{:?}", first.population);
    assert!(
        first
            .population
            .iter()
            .any(|m| m.id == federation_stage::FederationStage::domain_member("equity")),
        "no domain member for the universe's asset class: {:?}",
        first.population
    );
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
        assert_eq!(a.disagreement.views.len(), 9, "{a:?}");
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
    // 1129): seven tape members on every instrument proposition.
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
        // Eight roots (seven tape members and the causal model); a branch the stage opened on an earlier split
        // (WORLD-1181) is an eighth voice and carries its parent.
        let roots = a
            .disagreement
            .views
            .iter()
            .filter(|v| v.lineage.parents.is_empty())
            .count();
        assert_eq!(roots, 9, "{a:?}");
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

/// REASON-3056: a derivation explains the surprise counterfactually, not
/// only by naming a cause. The failure this guards: `CausalGraph::intervene`
/// ran in REASON and said what the subject reaches, while
/// `CausalGraph::counterfactual` existed with no caller, so no record ever
/// said what the subject would have done had its abduced cause stayed flat.
///
/// Kestrel supplies Northwind at strength 0.5 and confidence 0.8 (transmission
/// 0.4) and fell 5%; Northwind was expected flat and fell 4%. Had Kestrel not
/// moved, the graph implies Northwind at -0.04 - (-0.05) * 0.4 = -0.02.
#[test]
fn a_derivation_records_what_the_subject_would_have_done_had_its_abduced_cause_stayed_flat()
-> Result<()> {
    use qip_kernel::symbolic_stage::{AnomalyFacts, SymbolicInput, derive};
    use qip_world_model::causal::{CausalEdge, CausalGraph, Mechanism};
    use std::collections::BTreeMap;

    let now = start();
    let recorded = now.saturating_sub(Duration::from_days(10));
    let mut causal = CausalGraph::new();
    causal.add(
        CausalEdge::new(
            "kestrel",
            "northwind",
            Mechanism::SupplyChain,
            0.5,
            Duration::from_days(1),
            recorded,
        )?
        .with_confidence(0.8)?
        .with_evidence(vec!["ev-supply".into()]),
    );
    let input = SymbolicInput {
        cycle: 1,
        hypothesis_id: "hyp-1-northwind".into(),
        subject: "northwind".into(),
        status: "draft".into(),
        confidence: 0.6,
        contested: false,
        dissenters: 0,
        evidence_items: 1,
        kill_switch: false,
        live_authority: false,
        anomaly: AnomalyFacts {
            detector: "zscore".into(),
            detected_at: now,
            observed: -0.04,
            expected: 0.0,
            confidence: 0.9,
        },
        catalyst: None,
        formed_at: now,
        horizon: Duration::from_days(5),
        moves: BTreeMap::from([("kestrel".to_string(), -0.05)]),
        settlement: None,
        subject_class: Some("equity".into()),
        lot_headroom: None,
        cash_headroom: None,
    };
    let derivation = derive(input, &causal, now);
    // Premise: abduction named Kestrel, seen to move, so there is a cause to
    // hold flat; and the intervention step itself ran.
    let abduction = derivation
        .abduction
        .derived()
        .ok_or_else(|| qip_core::error::Error::invalid("abduction refused"))?;
    assert_eq!(
        abduction.best().map(|c| c.edge.cause.as_str()),
        Some("kestrel"),
        "premise: kestrel is the abduced cause"
    );
    let intervention = derivation
        .intervention
        .derived()
        .ok_or_else(|| qip_core::error::Error::invalid("intervention refused"))?;
    assert_eq!(intervention.counterfactuals.len(), 1, "{intervention:?}");
    let cf = &intervention.counterfactuals[0];
    assert_eq!(cf.cause, "kestrel");
    assert!((cf.observed_move + 0.05).abs() < 1e-12);
    assert!((cf.observed_subject + 0.04).abs() < 1e-12);
    assert!((cf.total_effect - 0.4).abs() < 1e-12, "{cf:?}");
    assert!(
        (cf.counterfactual_subject + 0.02).abs() < 1e-12,
        "had kestrel stayed flat, northwind reads -0.02: {cf:?}"
    );
    assert_eq!(cf.paths, 1);
    // The record round-trips, and one written before the field existed
    // still decodes, as none rather than a refusal.
    let json = serde_json::to_value(&derivation)?;
    let back: qip_kernel::symbolic_stage::SymbolicDerivation = serde_json::from_value(json)?;
    assert_eq!(back, derivation);
    Ok(())
}

/// R12-MODEL-6016: a world-model federation member is scored after its
/// outcome resolves, charged to its own component in the self-model, as a
/// detector or an analyst is. The federation kept its own Brier record and
/// the self-model never heard of a world model. Mutation: dropping the
/// charge loop in `Platform`'s federation pass leaves every member's
/// component absent.
#[test]
fn a_resolved_federation_forecast_is_charged_to_its_world_model_in_the_self_model() -> Result<()> {
    use qip_learning_engine::self_model::{ComponentKey, ComponentKind};

    let cycles: u64 = 8;
    let mut platform = platform()?;
    let total = 120 + cycles as usize;
    let end = start().saturating_add(Duration::from_days(cycles as i64));
    let mut aaa = tape("AAA", total, end);
    let later = aaa.split_off(120);
    platform.observe(aaa);
    for (step, a) in later.into_iter().enumerate() {
        platform.observe(vec![a]);
        platform.run_cycle(start().saturating_add(Duration::from_days(step as i64 + 1)));
    }

    // Premise: reality answered forecasts that named the members who made
    // them, on a side each took.
    let reviews = federation_stage::replay(&platform)?;
    let mut charged: std::collections::BTreeMap<String, usize> = Default::default();
    for resolution in reviews.iter().flat_map(|r| &r.resolved) {
        for (member, probability) in &resolution.forecasts {
            if probability.total_cmp(&0.5).is_ne() {
                *charged.entry(member.clone()).or_default() += 1;
            }
        }
    }
    assert!(!charged.is_empty(), "no resolved forecast named a member");

    for (member, count) in &charged {
        let key = ComponentKey::new(ComponentKind::WorldModel, member.as_str())?;
        let estimate = platform.self_model().get(&key).unwrap_or_else(|| {
            panic!("{member} resolved {count} forecast(s) and holds no estimate")
        });
        assert_eq!(estimate.sample_count(), *count, "{member}");
    }
    Ok(())
}

/// The one journaled derivation of the fixture every replay test reads, and
/// the platform whose causal graph re-runs its intervention.
fn journaled_derivation() -> Result<(Platform, symbolic_stage::SymbolicDerivation)> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());
    let mut derivations = symbolic_stage::replay(&platform)?;
    assert_eq!(derivations.len(), 1, "one derivation per formed hypothesis");
    let derivation = derivations.remove(0);
    // Premise: the intact record passes the checker every corruption below
    // is put through, so a refusal is the corruption's doing.
    symbolic_stage::verify(&derivation, platform.world().causal())?;
    Ok((platform, derivation))
}

/// Refused, and refused for the part that was corrupted: the message names
/// that part, so a checker refusing the record for some other reason does
/// not pass for one that catches this corruption.
fn assert_refused_for(
    platform: &Platform,
    corrupted: &symbolic_stage::SymbolicDerivation,
    part: &str,
) {
    let refusal = symbolic_stage::verify(corrupted, platform.world().causal())
        .expect_err("a corrupted derivation was accepted as reproducible");
    let expected = format!("the {part} is not reproducible");
    assert!(
        refusal.message().ends_with(&expected),
        "refused, but not for the {part}: {}",
        refusal.message()
    );
}

#[test]
fn a_journaled_timeline_that_does_not_rebuild_from_its_recorded_instants_is_refused_on_replay()
-> Result<()> {
    // R12-REASON-1133. The failure this guards: replay re-checked only the
    // rule verdict, so a journaled ordering could say anything — an anomaly
    // placed after the hypothesis it gave rise to replayed as a proof.
    let (platform, derivation) = journaled_derivation()?;
    assert!(
        derivation.temporal.inputs.is_some(),
        "the record keeps the instants its timeline was built from"
    );
    // R12-REASON-3055: the platform hands the subject's settlement
    // convention from its asset-class record, and the timeline poses it
    // against the deadline.
    let settlement = derivation
        .temporal
        .inputs
        .as_ref()
        .and_then(|inputs| inputs.settlement.clone())
        .expect("the platform handed the subject's settlement convention");
    let posed = format!("settlement:{}:{}", settlement.0, derivation.hypothesis_id);
    assert!(
        derivation.temporal.links.iter().any(|l| l.cause == posed),
        "{:?}",
        derivation.temporal.links
    );
    assert_eq!(
        derivation.temporal.anomaly_to_hypothesis,
        qip_reasoning_engine::temporal::Order::Before
    );
    let mut corrupted = derivation.clone();
    corrupted.temporal.anomaly_to_hypothesis = qip_reasoning_engine::temporal::Order::After;
    assert_refused_for(&platform, &corrupted, "temporal derivation");
    let mut moved = derivation;
    if let Some(inputs) = moved.temporal.inputs.as_mut() {
        inputs.formed_at = inputs.formed_at.saturating_add(Duration::from_secs(1));
    }
    assert_refused_for(&platform, &moved, "temporal derivation");
    Ok(())
}

#[test]
fn a_journaled_posterior_that_the_recorded_network_does_not_yield_is_refused_on_replay()
-> Result<()> {
    // R12-REASON-1134: the posterior was journaled and never recomputed, so
    // an edited probability replayed as the network's own conclusion.
    let (platform, derivation) = journaled_derivation()?;
    let mut corrupted = derivation.clone();
    let symbolic_stage::Step::Derived(posterior) = &mut corrupted.posterior else {
        panic!(
            "premise: the posterior was derived: {:?}",
            derivation.posterior
        );
    };
    let first = posterior
        .values_mut()
        .next()
        .expect("premise: the posterior names at least one state");
    *first = (*first + 0.25) % 1.0;
    assert_refused_for(&platform, &corrupted, "posterior");
    Ok(())
}

#[test]
fn a_journaled_acting_answer_the_recorded_verdict_does_not_solve_to_is_refused_on_replay()
-> Result<()> {
    // R12-REASON-1136: the constraint answer was journaled and never
    // re-solved, so a record could say acting was satisfiable for a
    // hypothesis the rule pack held ineligible.
    let (platform, derivation) = journaled_derivation()?;
    assert!(
        derivation.acting.derived().is_some(),
        "premise: {:?}",
        derivation.acting
    );
    let mut corrupted = derivation;
    corrupted.acting = symbolic_stage::Step::Refused("edited".into());
    assert_refused_for(&platform, &corrupted, "constraint derivation");
    Ok(())
}

#[test]
fn a_journaled_intervention_the_causal_graph_does_not_reach_at_its_instant_is_refused_on_replay()
-> Result<()> {
    // R12-REASON-1132: the intervention was journaled and replay never
    // re-ran it, so what the record said an intervention on the subject
    // reaches was taken on trust.
    let (platform, derivation) = journaled_derivation()?;
    let symbolic_stage::Step::Derived(summary) = &derivation.intervention else {
        panic!(
            "premise: the intervention ran: {:?}",
            derivation.intervention
        );
    };
    let mut corrupted = derivation.clone();
    if let symbolic_stage::Step::Derived(edited) = &mut corrupted.intervention {
        edited.paths = summary.paths + 1;
    }
    assert_refused_for(&platform, &corrupted, "intervention");
    let mut reached = derivation;
    if let symbolic_stage::Step::Derived(edited) = &mut reached.intervention {
        edited
            .affected
            .insert("an instrument nothing reaches".into());
    }
    assert_refused_for(&platform, &reached, "intervention");
    Ok(())
}

#[test]
fn a_derivation_records_its_violated_assumptions_and_uncertainty_and_replay_refuses_either_edited()
-> Result<()> {
    // R12-REASON-3062. The failure this guards: a derivation said what it
    // concluded and never what failed beneath the conclusion or how
    // undecided it was, so a reader could not tell a confident record from
    // a coin flip, or a clean one from one whose acting was blocked.
    let (platform, derivation) = journaled_derivation()?;
    let posterior = derivation
        .posterior
        .derived()
        .expect("premise: the posterior was derived");
    assert_eq!(posterior.len(), 2, "premise: claim_holds is binary");
    let uncertainty = derivation
        .uncertainty
        .expect("a derived posterior carries its spread");
    assert!(
        uncertainty > 0.0 && uncertainty < 1.0,
        "a posterior strictly between certain and even: {uncertainty}"
    );
    // The acting conflict, when there is one, is named constraint by
    // constraint; a satisfiable answer contributes nothing.
    match derivation.acting.derived() {
        Some(qip_reasoning_engine::constraint::Answer::Unsatisfiable { conflict }) => {
            for id in conflict {
                let token = format!("constraint: {id}");
                assert!(
                    derivation.violated_assumptions.contains(&token),
                    "{token} missing from {:?}",
                    derivation.violated_assumptions
                );
            }
        }
        Some(_) => assert!(
            !derivation
                .violated_assumptions
                .iter()
                .any(|v| v.starts_with("constraint: ")),
            "{:?}",
            derivation.violated_assumptions
        ),
        None => panic!("premise: acting was solved: {:?}", derivation.acting),
    }
    let mut hidden = derivation.clone();
    hidden.violated_assumptions = vec!["constraint: an-edit".into()];
    assert_refused_for(&platform, &hidden, "violated-assumptions list");
    let mut steadied = derivation;
    steadied.uncertainty = Some(0.0);
    assert_refused_for(&platform, &steadied, "uncertainty");
    Ok(())
}

#[test]
fn the_self_system_member_is_fed_the_previous_cycles_stage_latency_and_the_degradation_table()
-> Result<()> {
    // R12-WORLD-3052. The failure this guards: the self/system member read
    // the kill switch, the autonomy level and expired manifests, and never
    // how slow the platform ran or how stale its own capabilities were, so
    // a platform degrading quietly forecast itself as healthy.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    for step in 0..2 {
        platform.run_cycle(start().saturating_add(Duration::from_mins(5 * step)));
    }
    let reviews = federation_stage::replay(&platform)?;
    assert_eq!(reviews.len(), 2, "premise: one review per cycle");
    // The first pass has no finished cycle behind it, and says so.
    assert_eq!(reviews[0].system.slowest_stage, None);
    assert!(
        reviews[1].system.slowest_stage.is_some(),
        "the second pass did not read the first cycle's stage latency"
    );
    for review in &reviews {
        let degraded = review
            .system
            .degraded_capabilities
            .expect("the degradation table was read");
        assert!(
            degraded <= qip_contracts::degradation::Capability::all().len(),
            "{degraded}"
        );
    }
    Ok(())
}

/// R12-REASON-1131: a reasoning task uses the ontology. The symbolic stage
/// types every hypothesis's subject against `platform_ontology`, from the
/// asset class the platform registered it under, and journals the typing on
/// the derivation; a subject the ontology cannot type is recorded as
/// refused rather than reasoned about as if it were known. Until this,
/// `OntologyRegistry` was constructed only in its own tests.
///
/// Mutations: in `Platform::reason_symbolically`, pass `subject_class: None`
/// — the shipped cycle's derivation is untyped and the first assertion
/// fires; in `symbolic_stage::derive`, record `Step::Derived` whatever
/// `type_subject` returns — the untyped subjects read as typed and the
/// refusal assertions fire.
#[test]
fn the_symbolic_stage_types_its_subject_against_the_ontology_and_records_an_untyped_subject_as_refused()
-> Result<()> {
    use qip_kernel::symbolic_stage::{Step, derive};

    // The shipped cycle: AAA is a common stock, so an equity.
    let (platform, derivation) = journaled_derivation()?;
    let typing = derivation
        .subject_type
        .as_ref()
        .expect("a derivation written by this build records its subject's typing");
    assert_eq!(typing.class.as_deref(), Some("equity"));
    assert_eq!(typing.typed, Step::Derived("asset/equity v1".to_string()));

    // A subject with no registered class, and one whose class the ontology
    // does not hold: both refused, each naming why.
    let mut input = {
        use qip_kernel::symbolic_stage::{AnomalyFacts, SymbolicInput};
        SymbolicInput {
            cycle: 1,
            hypothesis_id: "hyp-1-unknown".into(),
            subject: "unknown".into(),
            status: "draft".into(),
            confidence: 0.6,
            contested: false,
            dissenters: 0,
            evidence_items: 1,
            kill_switch: false,
            live_authority: false,
            anomaly: AnomalyFacts {
                detector: "zscore".into(),
                detected_at: start(),
                observed: -0.04,
                expected: 0.0,
                confidence: 0.9,
            },
            catalyst: None,
            formed_at: start(),
            horizon: Duration::from_days(5),
            moves: std::collections::BTreeMap::new(),
            settlement: None,
            subject_class: None,
            lot_headroom: None,
            cash_headroom: None,
        }
    };
    let unregistered = derive(input.clone(), platform.world().causal(), start());
    match unregistered.subject_type.as_ref().map(|t| &t.typed) {
        Some(Step::Refused(why)) => assert!(why.contains("no registered asset class"), "{why}"),
        other => panic!("a subject with no class was typed: {other:?}"),
    }
    input.subject_class = Some("weather_derivative".into());
    let foreign = derive(input, platform.world().causal(), start());
    match foreign.subject_type.as_ref().map(|t| &t.typed) {
        Some(Step::Refused(why)) => {
            assert!(
                why.contains("`weather_derivative` is not a type the ontology holds"),
                "{why}"
            )
        }
        other => panic!("a class the ontology does not hold was typed: {other:?}"),
    }
    assert!(
        foreign.detail().ends_with(", subject untyped"),
        "the stage line does not say the subject was untyped: {}",
        foreign.detail()
    );

    // And the typing is part of the proof: a journaled typing the ontology
    // does not reproduce is refused on replay.
    let mut corrupted = derivation;
    if let Some(typing) = corrupted.subject_type.as_mut() {
        typing.typed = Step::Derived("asset/credit v1".to_string());
    }
    assert_refused_for(&platform, &corrupted, "subject typing");
    Ok(())
}

// --- the federation's disagreement narrows DECIDE's bound (R12-RES-7105) ----

#[test]
fn a_material_federation_disagreement_narrows_the_instruments_bound_and_an_agreeing_one_does_not()
-> Result<()> {
    // The failure this guards: the federation kept every view, measured the
    // spread, raised research, and the order path never heard of any of it, so
    // two strong irreconciled views sized exactly as one confident one.
    use qip_reasoning_engine::redteam::ReviewPolicy;
    let config = PlatformConfig {
        review: ReviewPolicy {
            minimum_surviving_confidence: 0.10,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    };
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    // Premise: the journaled arbitration really is a material disagreement,
    // and the platform kept a narrowing for exactly that spread.
    let reviews = federation_stage::replay(&platform)?;
    let arbitration = reviews[0]
        .arbitrations
        .iter()
        .find(|a| a.proposition.contains("obj-AAA:next-close-up@"))
        .expect("the instrument was arbitrated");
    let spread = arbitration.disagreement.spread;
    assert!(
        spread >= qip_world_model::federation::MATERIAL_DISAGREEMENT_SPREAD,
        "premise: a material disagreement, got {spread}"
    );
    let (narrowing, kept) = platform
        .federation_narrowing()
        .get("obj-AAA")
        .copied()
        .expect("a material disagreement earned no narrowing");
    assert!(
        (kept - spread).abs() < f64::EPSILON,
        "the narrowing rests on the journaled spread: {kept} against {spread}"
    );
    assert!(narrowing.multiplier() > 0.0 && narrowing.multiplier() < 1.0);

    // And DECIDE says so: the cycle sized the thesis and named the narrowing.
    let decide = report.stage(Stage::Decide).expect("DECIDE ran");
    assert!(
        decide.detail.contains("the world models disagree by"),
        "DECIDE did not name the federation's narrowing: {}",
        decide.detail
    );

    // An agreeing population earns nothing: below the material spread there is
    // no schedule band, and a spread nobody could measure earns none either.
    assert!(federation_stage::Narrowing::of_spread(0.0).is_none());
    assert!(
        federation_stage::Narrowing::of_spread(
            qip_world_model::federation::MATERIAL_DISAGREEMENT_SPREAD - 0.01
        )
        .is_none()
    );
    assert!(federation_stage::Narrowing::of_spread(f64::NAN).is_none());
    // Nothing in the schedule enlarges a bound.
    assert!(
        federation_stage::NARROWING_SCHEDULE
            .iter()
            .all(|(_, multiplier)| *multiplier > 0.0 && *multiplier < 1.0)
    );
    Ok(())
}

#[test]
fn a_federation_narrowing_multiplies_into_the_cap_once_per_instrument_and_an_agreeing_one_leaves_it_at_one()
-> Result<()> {
    // The failure this guards: the sibling test reads DECIDE's detail line, and
    // `narrow` pushes that line whether or not it multiplied, so a narrowing
    // that was named and never applied passed it. The cap is the number the
    // constructor reads, so it is asserted here directly (ADR 0125 decisions 4
    // and 6).
    let (narrowing, spread) = federation_stage::Narrowing::of_spread(
        qip_world_model::federation::MATERIAL_DISAGREEMENT_SPREAD,
    )
    .map(|n| (n, qip_world_model::federation::MATERIAL_DISAGREEMENT_SPREAD))
    .expect("premise: the material spread earns a narrowing");
    assert!(narrowing.multiplier() < 1.0, "premise: it narrows");
    let narrowings =
        std::collections::BTreeMap::from([("obj-AAA".to_string(), (narrowing, spread))]);
    let objects = || ["obj-AAA", "obj-AAA", "obj-BBB"].map(String::from);

    // A disagreeing instrument compounds with an existing cap, once however
    // many theses name it; an instrument with no arbitration is untouched.
    let mut caps = std::collections::BTreeMap::from([("obj-AAA".to_string(), 0.8)]);
    let mut notes = Vec::new();
    federation_stage::narrow(&mut caps, &mut notes, objects(), &narrowings);
    assert!(
        (caps["obj-AAA"] - 0.8 * narrowing.multiplier()).abs() < 1e-12,
        "narrowed once, not squared: {}",
        caps["obj-AAA"]
    );
    assert!(!caps.contains_key("obj-BBB"), "absence is not disagreement");
    assert_eq!(notes.len(), 1, "one note per instrument: {notes:?}");

    // An agreeing population leaves every cap where it was.
    let mut caps = std::collections::BTreeMap::new();
    let mut notes = Vec::new();
    federation_stage::narrow(
        &mut caps,
        &mut notes,
        objects(),
        &std::collections::BTreeMap::new(),
    );
    assert!(caps.is_empty() && notes.is_empty());
    Ok(())
}

/// R12-QUANT-4040. The symbolic reasoner's constraint encoder produced a QUBO
/// for the quantum path's solver port and nothing ever ran a quantum solver on
/// one, so "the constraint problem is compared to classical solvers" was a
/// sentence about a type signature.
///
/// The problem here is a selection the encoder admits - four 0/1 variables
/// and one linear equality - and not `symbolic_stage::acting()`'s, which the
/// encoder correctly refuses: its eligibility and halt facts are Boolean
/// domains and its guards are conditions, not linear equalities. The quantum
/// entrant is the in-tree statevector simulator behind `ProviderSolver`, the
/// baseline is exhaustive search, and the decoded quantum answer must equal
/// the symbolic engine's own proven minimiser.
///
/// Mutation: drop the penalty term for the equality in `Problem::to_qubo`
/// (skip the `(sum - bound)^2` block) - the QUBO's minimum selects nothing or
/// everything, the decode refuses it as violating the constraint, and the
/// assertion that the quantum entrant decodes fails.
#[test]
fn the_constraint_encoders_qubo_is_solved_by_a_quantum_entrant_and_decodes_to_the_symbolic_optimum()
-> Result<()> {
    use qip_quantum::benchmark::SolverBenchmark;
    use qip_quantum::provider::SimulatedProvider;
    use qip_quantum::qaoa::QaoaSettings;
    use qip_quantum::solver::{ClassicalSolver, ProviderSolver, SolverKind};
    use qip_reasoning_engine::constraint::{Constraint, Form, Problem, Variable};
    use qip_reasoning_engine::rules::{Op, Value};
    use std::sync::Arc;

    let bits = ["a", "b", "c", "d"];
    let problem = Problem {
        variables: bits
            .iter()
            .map(|name| Variable {
                name: (*name).to_string(),
                domain: vec![Value::Int(0), Value::Int(1)],
            })
            .collect(),
        constraints: vec![Constraint {
            id: "choose-exactly-two".into(),
            form: Form::Linear {
                terms: bits.iter().map(|name| (1, (*name).to_string())).collect(),
                op: Op::Eq,
                bound: 2,
            },
        }],
    };
    let objective: Vec<(i64, String)> = [3, 1, 4, 2]
        .iter()
        .zip(bits)
        .map(|(cost, name)| (*cost, name.to_string()))
        .collect();

    let optimum = problem
        .minimise(&objective)?
        .expect("the selection has a solution");
    assert_eq!(
        optimum.objective, 3,
        "premise: the cheapest two of 3, 1, 4, 2 cost 1 + 2"
    );

    let qubo = problem.to_qubo(&objective)?;
    let report = SolverBenchmark::new(ClassicalSolver::exhaustive(20))
        .with_solver(Arc::new(ProviderSolver::new(
            Arc::new(SimulatedProvider::new(7)),
            QaoaSettings {
                layers: 2,
                optimiser_iterations: 80,
                shots: 0,
            },
        )))
        .with_repeats(1)
        .run(&qubo)?;

    let quantum = report
        .entrants
        .iter()
        .find(|record| record.kind == SolverKind::Quantum)
        .expect("a quantum entrant ran on the constraint QUBO");
    let solution = quantum
        .usable_solution()
        .expect("the quantum entrant produced a validated answer");
    let decoded = problem.decode(&objective, solution.assignment())?;
    assert_eq!(
        decoded.objective, optimum.objective,
        "the quantum answer decodes to a feasible assignment, but not the proven optimum"
    );
    assert_eq!(decoded.assignment, optimum.assignment);
    Ok(())
}
