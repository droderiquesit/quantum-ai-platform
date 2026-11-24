//! The self-model in the loop: fed by LEARN, read by REASON.
//!
//! Two seams, each proven by driving the thing that should reach it. The
//! first is the cycle: a thesis the platform formed resolves, LEARN grades
//! it, and the components that produced it are charged. The second is the
//! factor: the reasoning engine forms a hypothesis with each origin's
//! evidence scaled by its measured accuracy — only where the sample is
//! sufficient — and records the factor on the hypothesis so a replay
//! recomputes the same confidence.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::ids::{EvidenceId, HypothesisId, ObjectId};
use qip_core::testing::approx_eq;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_learning_engine::self_model::{ComponentKey, MINIMUM_SAMPLE, ScoredOutcome, SelfModel};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet, Stance};
use qip_reasoning_engine::hypothesis::{CausalChain, CausalStep, Claim, HypothesisDraft};
use qip_reasoning_engine::{
    Hypothesis, ReasoningEngine, ReviewPolicy, SynthesisInput, UncertaintyType,
};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::causal::Mechanism;
use std::collections::BTreeMap;

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

/// A day bar opening at `at`.
fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    bar_over(symbol, at, Interval::Day, open, close)
}

/// An hour bar opening at `at`, so it closes — and becomes knowable — an hour
/// later.
///
/// R12-TICK-2077: the fixtures that feed bars an hour apart used to stamp
/// them `Interval::Day`, which describes twenty overlapping day buckets, the
/// last closing twenty-three hours after the horizon the claim is graded at.
/// While the as-of rule read a bar from its open time that mislabel was
/// invisible; a rule on the close time rightly leaves such a bar out. The
/// fixture is corrected to say what it always meant — hourly bars — rather
/// than the rule being loosened to admit a bar that had not closed.
fn hourly_bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    bar_over(symbol, at, Interval::Hour, open, close)
}

fn bar_over(
    symbol: &str,
    at: Timestamp,
    interval: Interval,
    open: f64,
    close: f64,
) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval,
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

// --- LEARN feeds the self-model ------------------------------------------------

#[test]
fn grading_a_resolved_thesis_moves_the_self_model_for_the_components_that_produced_it() -> Result<()>
{
    // The failure this guards: LEARN graded the thesis, the calibration
    // moved, and nothing was charged to the detector that raised it or the
    // analysts that ran on it — so the platform held a Brier score about
    // itself as a whole and no estimate of any part.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());

    // Premise: a claim was written, it carries a class and contributors, and
    // the self-model is empty. Without this the assertions below could pass
    // against a model that was never empty.
    assert!(
        !platform.predictions().is_empty(),
        "no claim was written, so there is nothing to resolve:\n{}",
        first.summarise()
    );
    let prediction = platform.predictions()[0].clone();
    let claim = prediction
        .claim
        .clone()
        .expect("a claim records what it was made from, or it cannot be charged");
    assert!(
        !claim.class.is_empty(),
        "a claim with no class charges no detector"
    );
    assert!(
        platform.self_model().is_empty(),
        "the self-model held something before anything resolved"
    );
    assert!(
        platform.self_model().origin_factors().is_empty(),
        "the REASON stage was handed a factor before anything resolved"
    );

    // The world moves far enough past the reference that the verdict is
    // informative — the same swing `tests/learning.rs` feeds.
    let horizon = prediction.proposition.resolves_at;
    let swings: Vec<SensedRecord> = (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            hourly_bar("AAA", at, open, close)
        })
        .collect();
    platform.observe(swings);
    let second = platform.run_cycle(horizon.saturating_add(Duration::from_mins(1)));
    let learn = second.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("graded"),
        "LEARN did not report grading anything: {}",
        learn.detail
    );
    assert!(
        platform
            .evaluations()
            .iter()
            .any(|e| e.verdict.is_informative()),
        "an informative verdict is the premise"
    );

    // The detector is charged under the hypothesis class.
    let detector = ComponentKey::detector(&claim.class)?;
    let record = platform.self_model().get(&detector).unwrap_or_else(|| {
        panic!(
            "{detector} was not charged; the model holds {:?}",
            platform
                .self_model()
                .iter()
                .map(|(k, _)| k.to_string())
                .collect::<Vec<_>>()
        )
    });
    assert_eq!(
        record.sample_count(),
        1,
        "one resolved thesis is one outcome"
    );
    assert_eq!(
        record.last_updated(),
        Some(platform.evaluations()[0].evaluated_at),
        "the outcome is stamped with the instant it was graded"
    );
    // And every analyst whose run contributed is charged under its id.
    let analysts: Vec<String> = platform
        .self_model()
        .iter()
        .filter(|(key, _)| key.kind == qip_learning_engine::self_model::ComponentKind::Analyst)
        .map(|(key, _)| key.id.clone())
        .collect();
    let expected: Vec<String> = claim
        .contributors
        .iter()
        .filter_map(|run| run.strip_prefix("run-"))
        .filter_map(|rest| rest.rsplit_once('-').map(|(id, _)| id.to_string()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(
        !expected.is_empty(),
        "the claim names no analyst run to charge"
    );
    assert_eq!(
        analysts, expected,
        "the analysts charged are not the ones that ran"
    );
    // One outcome is below the minimum, so no factor reached REASON.
    assert!(
        platform.self_model().origin_factors().is_empty(),
        "a factor was handed to REASON on a single outcome"
    );
    Ok(())
}

#[test]
fn the_self_model_survives_the_journal_and_refuses_a_key_it_cannot_read_back() -> Result<()> {
    // The failure this guards: a component key that serialised to something
    // the deserialiser could not turn back into the same key, so the model
    // the API served was not the model the platform held.
    let mut model = SelfModel::new();
    model.record(
        ComponentKey::analyst("macro-analyst")?,
        ScoredOutcome::new("hyp-1", 0.7, true, start())?,
    );
    let text = serde_json::to_string(&model).expect("serialisable");
    assert!(
        text.contains("\"analyst:macro-analyst\""),
        "the key is not serialised as kind:id: {text}"
    );
    let back: SelfModel = serde_json::from_str(&text).expect("deserialisable");
    assert_eq!(
        back, model,
        "the model did not survive its own serialisation"
    );
    assert!(
        serde_json::from_str::<SelfModel>(r#"{"components":{"nowhere":{"window":[]}}}"#).is_err(),
        "a key of no kind was read back"
    );
    Ok(())
}

// --- REASON consumes the self-model -------------------------------------------

fn now() -> Timestamp {
    start()
}

fn evidence(id: &str, origin: &str, stance: Stance) -> Evidence {
    Evidence::new(
        EvidenceId::from_string(id),
        EvidenceKind::Filing,
        stance,
        format!("statement for {id}"),
        format!("rec-{id}"),
        origin,
        now(),
        now(),
    )
    .with_reliability(0.8)
    .with_diagnosticity(0.7)
}

fn sound_chain() -> CausalChain {
    CausalChain::new(vec![CausalStep::new(
        "funding-cost",
        "obj-ACME",
        Mechanism::CreditConditions,
        "gross margin compresses",
        Duration::from_days(20),
        0.8,
    )])
}

fn draft(evidence: EvidenceSet) -> HypothesisDraft {
    HypothesisDraft {
        hypothesis_id: HypothesisId::from_string("hyp-1"),
        opportunity_id: None,
        formed_at: now(),
        as_of: now(),
        class: "funding-cost-pass-through".to_string(),
        claim: Claim::Overvalued,
        statement: "ACME's margin guidance does not reflect its funding".to_string(),
        subjects: vec![object("ACME")],
        chain: sound_chain(),
        evidence,
        prior: 0.25,
        uncertainty_type: UncertaintyType::Epistemic,
        falsifiers: vec!["the next report shows flat gross margin".to_string()],
        leading_alternative: "the market has priced the margin path".to_string(),
        horizon: Duration::from_days(60),
        contributors: Vec::new(),
        models: Vec::new(),
    }
}

fn two_origins() -> EvidenceSet {
    EvidenceSet::from_items(vec![
        evidence("e1", "macro-analyst", Stance::Supports),
        evidence("e2", "equity-analyst", Stance::Supports),
    ])
}

#[test]
fn the_reason_factor_scales_an_origin_only_with_a_sufficient_sample_and_is_recorded_on_the_hypothesis()
-> Result<()> {
    // The failure this guards, in three parts: an origin with two outcomes
    // scaled as if measured; a measured origin left at full weight because
    // the factor was computed and never applied; and a factor applied and
    // not written down, so a replay recomputed a different confidence from
    // the same evidence and `validate` rejected the record as drifted.
    let mut model = SelfModel::new();
    let measured = ComponentKey::analyst("macro-analyst")?;
    let thin = ComponentKey::analyst("equity-analyst")?;
    for n in 0..MINIMUM_SAMPLE {
        // Four hits in ten: (4 + 2) / (10 + 4).
        model.record(
            measured.clone(),
            ScoredOutcome::new(format!("h{n}"), 0.7, n < 4, now())?,
        );
    }
    model.record(thin.clone(), ScoredOutcome::new("t0", 0.7, true, now())?);
    model.record(thin.clone(), ScoredOutcome::new("t1", 0.7, true, now())?);
    let factors = model.origin_factors();
    assert_eq!(
        factors.len(),
        1,
        "the premise is one measured origin: {factors:?}"
    );
    let expected_factor = 6.0 / 14.0;
    assert!(approx_eq(factors["macro-analyst"], expected_factor, 1e-12));

    let unweighted = Hypothesis::form(draft(two_origins()))?;
    assert!(
        unweighted.origin_factors.is_empty(),
        "a hypothesis formed without factors recorded some"
    );
    let weighted = Hypothesis::form_with_factors(draft(two_origins()), &factors)?;

    // Recorded: exactly the measured origin, at exactly the factor applied.
    assert_eq!(
        weighted.origin_factors.keys().collect::<Vec<_>>(),
        vec!["macro-analyst"],
        "the factors recorded are not the ones applied: {:?}",
        weighted.origin_factors
    );
    assert!(approx_eq(
        weighted.origin_factors["macro-analyst"],
        expected_factor,
        1e-12
    ));

    // Scaled: the measured origin's contribution is the unweighted one times
    // the factor in weight space, so its log-likelihood ratio fell; the thin
    // origin's did not move at all.
    let llr = |hypothesis: &Hypothesis, id: &str| -> f64 {
        hypothesis
            .belief
            .contributions
            .iter()
            .find(|c| c.evidence_id == id)
            .map(|c| c.log_likelihood_ratio)
            .unwrap_or_else(|| panic!("{id} contributed nothing"))
    };
    assert!(
        llr(&weighted, "e1") < llr(&unweighted, "e1"),
        "the measured origin was not discounted: {} vs {}",
        llr(&weighted, "e1"),
        llr(&unweighted, "e1")
    );
    assert!(
        approx_eq(llr(&weighted, "e2"), llr(&unweighted, "e2"), 1e-12),
        "the unmeasured origin moved: {} vs {}",
        llr(&weighted, "e2"),
        llr(&unweighted, "e2")
    );
    assert!(
        weighted.confidence < unweighted.confidence,
        "discounting an origin did not lower the confidence"
    );
    // Toward the prior, never below it: a discount is less information,
    // not a claim the thesis is less likely than its base rate.
    assert!(weighted.confidence > weighted.prior);
    // The prior is untouched by the factor.
    assert!(approx_eq(weighted.prior, unweighted.prior, 1e-12));

    // Replay: the record survives the journal with its factors, validates on
    // its own, and re-forming from the same draft with the *recorded*
    // factors — not today's self-model — reproduces the same confidence.
    weighted.validate()?;
    let text = serde_json::to_string(&weighted).expect("serialisable");
    let replayed: Hypothesis = serde_json::from_str(&text).expect("deserialisable");
    assert_eq!(replayed, weighted);
    replayed.validate()?;
    let reformed = Hypothesis::form_with_factors(draft(two_origins()), &replayed.origin_factors)?;
    assert!(
        approx_eq(reformed.confidence, weighted.confidence, 1e-12),
        "the recorded factors did not reproduce the confidence: {} vs {}",
        reformed.confidence,
        weighted.confidence
    );
    // And the factor is load-bearing: recomputing the belief with the
    // factors edited away moves the confidence off the record.
    let mut stripped = weighted.clone();
    stripped.origin_factors = BTreeMap::new();
    stripped.revise();
    assert!(
        !approx_eq(stripped.confidence, weighted.confidence, 1e-9),
        "recomputing without the factor reproduced the same confidence, so the factor is \
         not part of the arithmetic"
    );
    assert!(approx_eq(stripped.confidence, unweighted.confidence, 1e-12));
    Ok(())
}

#[test]
fn the_reasoning_engine_forms_with_the_factors_it_was_handed_and_with_none_until_it_is()
-> Result<()> {
    // The failure this guards: the engine held factors nothing set, or the
    // kernel set them and `reason` formed the hypothesis by the path that
    // ignored them.
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    assert!(
        engine.origin_factors().is_empty(),
        "the premise is no factor"
    );
    let input = || SynthesisInput {
        hypothesis_id: HypothesisId::from_string("hyp-1"),
        opportunity_id: None,
        as_of: now(),
        now: now(),
        class: "funding-cost-pass-through".to_string(),
        claim: Claim::Overvalued,
        statement: "ACME's margin guidance does not reflect its funding".to_string(),
        subjects: vec![object("ACME")],
        chain: sound_chain(),
        findings: Vec::new(),
        direct_evidence: two_origins(),
        prior: 0.25,
        uncertainty_type: UncertaintyType::Epistemic,
        falsifiers: vec!["the next report shows flat gross margin".to_string()],
        leading_alternative: "the market has priced the margin path".to_string(),
        horizon: Duration::from_days(60),
        market_priced_in: None,
        models: Vec::new(),
    };
    let before = engine.reason(input())?;
    assert!(before.hypothesis.origin_factors.is_empty());

    let mut factors = BTreeMap::new();
    factors.insert("macro-analyst".to_string(), 0.5);
    engine.set_origin_factors(factors.clone());
    assert_eq!(engine.origin_factors(), &factors);
    let after = engine.reason(input())?;
    assert_eq!(after.hypothesis.origin_factors, factors);
    assert!(after.hypothesis.confidence < before.hypothesis.confidence);

    // Replaced whole: an origin that fell below the minimum loses its factor.
    engine.set_origin_factors(BTreeMap::new());
    let reset = engine.reason(input())?;
    assert!(reset.hypothesis.origin_factors.is_empty());
    assert!(approx_eq(
        reset.hypothesis.confidence,
        before.hypothesis.confidence,
        1e-12
    ));

    // A factor that is not an accuracy is refused, not clamped.
    let mut bad = BTreeMap::new();
    bad.insert("macro-analyst".to_string(), 1.5);
    engine.set_origin_factors(bad);
    assert!(
        engine.reason(input()).is_err(),
        "a factor above one was accepted"
    );
    Ok(())
}

// --- R12-WORLD-3036: REASON reads health, drift, blind spots, the boundary ----

/// A universe holding AAA (common stock, equity) and CCC (a cryptocurrency,
/// digital asset), so a class can be served or graded while the other is not.
fn two_class_universe() -> Universe {
    let mut universe = universe();
    universe
        .insert(
            FinancialObject::builder(
                object("CCC"),
                "CCC",
                InstrumentType::Cryptocurrency,
                fixture_liquidity(),
            )
            .venue("XNYS")
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn two_class_platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        two_class_universe(),
        limits(),
    )
}

/// `bars`, ending `ago` before `start()`.
fn bars_ending(symbol: &str, count: usize, ago: Duration) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start()
                .saturating_sub(ago)
                .saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

fn reason_detail(report: &qip_kernel::cycle::CycleReport) -> String {
    let stage = report.stage(Stage::Reason).expect("reason ran");
    format!("{} | {}", stage.detail, stage.problems.join(" | "))
}

/// The confidence REASON gave the one thesis the cycle formed.
fn thesis_confidence(platform: &Platform, report: &qip_kernel::cycle::CycleReport) -> f64 {
    platform
        .predictions()
        .last()
        .and_then(|p| p.claim.as_ref())
        .map(|claim| claim.confidence)
        .unwrap_or_else(|| panic!("no thesis was written:\n{}", report.summarise()))
}

/// The failure this guards: the self-model read the platform as halted and
/// REASON weighed every detector and analyst at its graded weight anyway, so
/// a thesis formed while the kill switch was tripped carried the same
/// confidence as one formed on a healthy platform.
///
/// Mutation: drop the `health.halted` arm in `SelfModel::shared_withholding`
/// -- the halted cycle withholds nothing and the confidence matches the
/// healthy one.
#[test]
fn a_tripped_kill_switch_withholds_every_producing_components_evidence_in_reason() -> Result<()> {
    let mut healthy = platform()?;
    healthy.observe(bars("AAA", 120));
    let healthy_report = healthy.run_cycle(start());
    let healthy_detail = reason_detail(&healthy_report);
    assert!(
        !healthy_detail.contains("self-model withheld"),
        "premise: a healthy platform withholds nothing: {healthy_detail}"
    );
    let healthy_confidence = thesis_confidence(&healthy, &healthy_report);

    let mut halted = platform()?;
    halted.observe(bars("AAA", 120));
    halted.autonomy_mut().kill_switch_mut().trip_global(
        start(),
        "operator",
        "a halt REASON must weigh".to_string(),
    );
    let halted_report = halted.run_cycle(start());
    let detail = reason_detail(&halted_report);
    assert!(
        detail.contains("self-model withheld the evidence of")
            && detail.contains("(unhealthy: the kill switch was tripped)"),
        "a halted platform weighed its components as healthy: {detail}"
    );
    let halted_confidence = thesis_confidence(&halted, &halted_report);
    assert!(
        halted_confidence < healthy_confidence,
        "withholding did not lower the confidence: halted {halted_confidence} vs healthy \
         {healthy_confidence}"
    );
    Ok(())
}

fn quiet_bars(symbol: &str, count: i64) -> Vec<SensedRecord> {
    (0..count)
        .map(|i| {
            let at = start().saturating_sub(Duration::from_days(count - i));
            bar(symbol, at, 100.0, 100.0 + (i % 2) as f64 * 0.01)
        })
        .collect()
}

/// The failure this guards: one input's bars stopped while the rest of the
/// feed kept flowing, and REASON weighed the detector reading that frozen
/// series as if it were reading the market.
///
/// Mutation: drop the `stale_inputs` arm in `SelfModel::shared_withholding`
/// -- the AAA thesis withholds nothing.
#[test]
fn an_input_whose_bars_stopped_is_a_stale_area_and_reason_withholds_the_evidence_read_from_it()
-> Result<()> {
    use qip_contracts::degradation::Capability;
    use qip_kernel::platform::FEED_HORIZON;
    let mut platform = platform()?;
    platform.observe(bars_ending(
        "AAA",
        120,
        FEED_HORIZON + Duration::from_days(1),
    ));
    platform.observe(quiet_bars("BBB", 40));
    let report = platform.run_cycle(start());
    assert!(
        platform
            .self_model()
            .stale_inputs()
            .contains_key(object("AAA").as_str())
            && !platform
                .self_model()
                .stale_inputs()
                .contains_key(object("BBB").as_str()),
        "premise: AAA and only AAA is a stale area: {:?}",
        platform.self_model().stale_inputs()
    );
    assert!(
        !platform
            .self_model()
            .health()
            .expect("a reading")
            .degraded
            .contains_key(&Capability::Feed),
        "premise: the feed as a whole is flowing"
    );
    let detail = reason_detail(&report);
    assert!(
        detail.contains(&format!(
            "(stale: the input {} has had no bar since",
            object("AAA").as_str()
        )),
        "a stale input's evidence was weighed: {detail}"
    );

    let mut fresh = platform_fresh()?;
    let fresh_report = fresh.run_cycle(start());
    assert!(fresh.self_model().stale_inputs().is_empty());
    assert!(!reason_detail(&fresh_report).contains("self-model withheld"));
    Ok(())
}

/// The failure this guards: the whole feed stopped and the self-model never
/// said so (`central_degradation` observes no feed row), and REASON went on
/// forming theses from the series it left behind. On a one-class universe —
/// the committed one holds only common stock — a stopped feed serves no
/// class, so the boundary refuses the thesis outright.
///
/// Mutation: read the feed as fresh whatever the age (`> FEED_HORIZON`
/// replaced by `> Duration::from_days(10_000)` in the feed row) -- the health
/// reading has no feed row.
#[test]
fn a_feed_older_than_its_horizon_reads_stale_in_health_and_reason_forms_no_thesis_on_it()
-> Result<()> {
    use qip_contracts::degradation::{Capability, Freshness};
    use qip_kernel::platform::FEED_HORIZON;
    let late = start().saturating_add(FEED_HORIZON + Duration::from_days(1));
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(late);
    let health = platform
        .self_model()
        .health()
        .expect("a reading after the cycle");
    assert_eq!(
        health.degraded.get(&Capability::Feed),
        Some(&Freshness::Stale),
        "the feed row did not read stale {:?} after the last bar: {health:?}",
        late.since(start())
    );
    let detail = reason_detail(&report);
    assert!(
        detail.contains(&format!(
            "no thesis was formed on {}: its asset class equity lies outside the capability \
             boundary the feed serves;",
            object("AAA").as_str()
        )),
        "{detail}"
    );
    Ok(())
}

fn platform_fresh() -> Result<Platform> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    Ok(platform)
}

/// A series whose second half is far more volatile than its first, with the
/// same late jump the detector fires on.
fn shifted_bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let scale = if i < count / 2 { 0.002 } else { 0.03 };
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * scale;
            let jump = if i == count - 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// The failure this guards: a component's input distribution moved under it
/// — the drift the record of graded outcomes cannot see until the misses
/// arrive — and REASON weighed it by a record earned on a different market.
///
/// Mutation: `observe_input_drift` keeps nothing (the filter returns false)
/// -- the drifted cycle withholds nothing.
#[test]
fn an_input_shifted_past_the_psi_threshold_reads_drifted_and_its_evidence_is_withheld() -> Result<()>
{
    use qip_learning_engine::self_model::INPUT_DRIFT_PSI;
    let mut steady = platform()?;
    steady.observe(bars("AAA", 260));
    let steady_report = steady.run_cycle(start());
    assert!(
        steady.self_model().drifted_inputs().is_empty(),
        "premise: a stationary series reads as no drift: {:?}",
        steady.self_model().drifted_inputs()
    );
    assert!(!reason_detail(&steady_report).contains("drifted"));

    let mut platform = platform()?;
    platform.observe(shifted_bars("AAA", 260));
    let report = platform.run_cycle(start());
    let index = platform
        .self_model()
        .drifted_inputs()
        .get(object("AAA").as_str())
        .copied()
        .unwrap_or_else(|| {
            panic!(
                "the shifted series did not read drifted: {:?}",
                platform.self_model().drifted_inputs()
            )
        });
    assert!(index >= INPUT_DRIFT_PSI, "drifted at {index}");
    let detail = reason_detail(&report);
    assert!(
        detail.contains(&format!(
            "(drifted: the input {} shifted by a PSI of {index:.3})",
            object("AAA").as_str()
        )),
        "a drifted input's evidence was weighed: {detail}"
    );
    Ok(())
}

/// The failure this guards: the boundary was declared at assembly from the
/// universe and read by nothing, so a class whose bars had stopped arriving
/// still had theses formed in it from the series it left behind.
///
/// Mutation: drop the boundary refusal in `reason_about_the_queue` -- a
/// thesis is formed on CCC.
#[test]
fn reason_refuses_a_thesis_in_an_asset_class_the_feed_no_longer_serves() -> Result<()> {
    use qip_kernel::platform::FEED_HORIZON;
    use qip_learning_engine::self_model::BoundaryReading;
    let mut platform = two_class_platform()?;
    // AAA is served now and quiet; CCC's bars stopped FEED_HORIZON + 1 day ago
    // and its old series still holds the jump a detector fires on.
    let quiet: Vec<SensedRecord> = (0..40)
        .map(|i| {
            let at = start().saturating_sub(Duration::from_days(40 - i));
            bar("AAA", at, 100.0, 100.0 + (i % 2) as f64 * 0.01)
        })
        .collect();
    platform.observe(quiet);
    platform.observe(bars_ending(
        "CCC",
        120,
        FEED_HORIZON + Duration::from_days(1),
    ));
    let report = platform.run_cycle(start());
    assert_eq!(
        platform.self_model().boundary_of("equity"),
        BoundaryReading::Inside,
        "premise: the class the feed serves now is inside"
    );
    assert_eq!(
        platform.self_model().boundary_of("digital_asset"),
        BoundaryReading::Outside,
        "the class whose bars stopped is still inside: {:?}",
        platform.self_model().boundary()
    );
    let detail = reason_detail(&report);
    assert!(
        detail.contains(&format!(
            "no thesis was formed on {}: its asset class digital_asset lies outside the \
             capability boundary the feed serves;",
            object("CCC").as_str()
        )),
        "{detail}\n{}",
        report.summarise()
    );
    assert!(
        platform
            .predictions()
            .iter()
            .all(|p| !p.hypothesis.ends_with(object("CCC").as_str())),
        "a thesis was written on CCC"
    );
    Ok(())
}

/// The catalogue every central root reads, relative to this crate so the test
/// reads the committed artefact and not a copy of it.
const COMMITTED_UNIVERSE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

/// The committed catalogue's instruments: NWSC and VNTG in
/// information_technology, MRDN in energy.
const NWSC: &str = "OBJ00000000000000000NWSC";
const VNTG: &str = "OBJ00000000000000000VNTG";
const MRDN: &str = "OBJ00000000000000000MRDN";

/// When the committed-catalogue runs start: after the catalogue's own `as_of`,
/// which refuses to be read before it was published.
fn committed_start() -> Timestamp {
    Timestamp::from_civil(2026, 10, 1)
}

/// A default platform on the committed catalogue, so the blind-spot cell is
/// read against the sectors a deployed root actually holds.
fn committed_platform() -> Result<Platform> {
    let text = std::fs::read_to_string(COMMITTED_UNIVERSE).expect("the catalogue is readable");
    let universe = qip_financial::catalogue::load(&text, committed_start())?.universe;
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(committed_start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits())
}

/// `records` with every bar re-addressed to `object_id` and shifted by `by`.
fn readdressed(records: Vec<SensedRecord>, object_id: &str, by: Duration) -> Vec<SensedRecord> {
    records
        .into_iter()
        .map(|record| match record {
            SensedRecord::Bar(mut b) => {
                b.object_id = ObjectId::from_string(object_id.to_string());
                b.open_time = b.open_time.saturating_add(by);
                SensedRecord::Bar(b)
            }
            other => other,
        })
        .collect()
}

/// What one run of [`reason_after_grading_on_nwsc`] saw.
struct BlindRun {
    /// REASON's detail on the cycle that formed the thesis on `second`.
    detail: String,
    /// The claim class the NWSC thesis was graded under: the detector's key.
    graded_class: String,
    /// The detector and the anomaly kind of the opportunity worked on `second`.
    worked_detector: String,
    worked_kind: String,
}

/// What REASON said about the first thesis it formed on `second`, after the
/// platform had graded one resolved thesis on NWSC (information_technology).
fn reason_after_grading_on_nwsc(second: &str) -> Result<BlindRun> {
    let mut platform = committed_platform()?;
    let shift = committed_start().since(start());
    platform.observe(readdressed(big_jump_bars("AAA", 120), NWSC, shift));
    let first = platform.run_cycle(committed_start());
    let prediction = platform
        .predictions()
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("no claim:\n{}", first.summarise()));
    assert!(
        prediction.hypothesis.ends_with(NWSC),
        "premise: the first thesis is on NWSC: {}",
        prediction.hypothesis
    );
    let horizon = prediction.proposition.resolves_at;
    let swings: Vec<SensedRecord> = (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 101.0)
            } else {
                (101.0, 100.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            hourly_bar("AAA", at, open, close)
        })
        .collect();
    platform.observe(readdressed(swings, NWSC, Duration::from_secs(0)));
    let graded_at = horizon.saturating_add(Duration::from_mins(1));
    platform.run_cycle(graded_at);
    let claim = prediction.claim.expect("a claim");
    let detector = ComponentKey::detector(&claim.class)?;
    assert!(
        platform.self_model().get(&detector).is_some(),
        "premise: grading the NWSC thesis charged its detector"
    );
    let sectors = platform
        .self_model()
        .graded_sectors(&detector)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        sectors.into_iter().collect::<Vec<_>>(),
        vec!["information_technology".to_string()],
        "premise: the detector is graded in information_technology and nothing else"
    );

    // `second` is served now, with the jump the same detector fires on.
    // NWSC's bars stopped, and the cycles run after every opportunity the
    // grading cycle queued on it has lapsed. The grading swings are kept to
    // one per cent so the residual NWSC anomaly ranks below the new one and
    // REASON works `second` first.
    let lapse = platform
        .queue()
        .iter()
        .map(|o| o.horizon)
        .max()
        .unwrap_or(Duration::from_secs(0));
    let later = graded_at
        .saturating_add(lapse.max(qip_kernel::platform::FEED_HORIZON) + Duration::from_days(1));
    platform.observe(readdressed(
        big_jump_bars("AAA", 120),
        second,
        later.since(start()),
    ));
    let on_second = |platform: &Platform| {
        platform
            .predictions()
            .iter()
            .any(|p| p.hypothesis.ends_with(second))
    };
    let mut detail = String::new();
    let mut worked = None;
    for step in 0..6 {
        let report = platform.run_cycle(later.saturating_add(Duration::from_mins(step)));
        if on_second(&platform) {
            detail = reason_detail(&report);
            // REASON works the head of the queue, and a worked opportunity
            // stays queued until it lapses.
            if let Some(head) = platform.queue().first()
                && head.affected_objects.first().map(ObjectId::as_str) == Some(second)
                && let Some(anomaly) = head.anomalies.first()
            {
                worked = Some((anomaly.detector.clone(), anomaly.kind.as_str().to_string()));
            }
            break;
        }
    }
    assert!(
        on_second(&platform),
        "premise: REASON formed a thesis on {second}"
    );
    let (worked_detector, worked_kind) =
        worked.expect("premise: the opportunity on the second name was at the queue's head");
    Ok(BlindRun {
        detail,
        graded_class: detector.id,
        worked_detector,
        worked_kind,
    })
}

/// The failure this guards: a component graded only on information-technology
/// names was weighed at that record on a thesis in energy, where nothing it
/// produced has ever resolved. On the committed catalogue, which is common
/// stock only, a blind spot keyed on asset class could never fire; the
/// (component, sector) cell the decision record defines does.
///
/// Mutation: `SelfModel::is_blind_in` returns false -- the MRDN thesis
/// withholds nothing.
#[test]
fn a_component_graded_in_one_sector_is_blind_in_another_on_the_committed_universe() -> Result<()> {
    let run = reason_after_grading_on_nwsc(MRDN)?;
    let detail = &run.detail;
    assert_eq!(
        run.worked_kind, run.graded_class,
        "premise: the energy thesis is raised by the kind of anomaly graded on NWSC"
    );
    assert_ne!(
        run.worked_detector, run.worked_kind,
        "premise: the detector's evidence name differs from the class it is graded under"
    );
    assert!(
        detail.contains("self-model withheld the evidence of "),
        "nothing was withheld on the energy thesis: {detail}"
    );
    // The analyst graded on NWSC, under its own name.
    assert!(
        detail.contains(
            "equity-analyst (blind spot: equity-analyst has graded outcomes only outside the \
             energy sector)"
        ),
        "the analyst blind in energy was weighed: {detail}"
    );
    // The detector, charged under its claim class and carrying its evidence
    // under its own name: the withholding is read under the one and filed
    // under the other.
    assert!(
        detail.contains(&format!(
            "{} (blind spot: {} has graded outcomes only outside the energy sector)",
            run.worked_detector, run.graded_class
        )),
        "the detector blind in energy was weighed: {detail}"
    );
    Ok(())
}

/// The other half: the same detector, graded on NWSC, is not blind on VNTG,
/// which shares NWSC's sector. Without it a rule that withheld the detector
/// everywhere would pass the test above.
///
/// Mutation: `SelfModel::is_blind_in` ignores the graded sectors and
/// returns true for every graded component -- the VNTG thesis withholds it.
#[test]
fn a_component_graded_in_a_sector_is_not_blind_on_another_name_in_that_sector() -> Result<()> {
    let run = reason_after_grading_on_nwsc(VNTG)?;
    assert_eq!(
        run.worked_kind, run.graded_class,
        "premise: the VNTG thesis is raised by the kind of anomaly graded on NWSC"
    );
    assert!(
        !run.detail.contains("(blind spot: "),
        "a component was called blind in the sector it was graded in: {}",
        run.detail
    );
    Ok(())
}

/// `bars` with the jump on the newest bar, which the return detector reads.
fn big_jump_bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count - 1 { 0.30 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}
