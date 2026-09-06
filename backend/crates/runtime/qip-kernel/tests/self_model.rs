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
use qip_reasoning_engine::{Hypothesis, ReasoningEngine, ReviewPolicy, SynthesisInput};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::causal::Mechanism;
use std::collections::BTreeMap;

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
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
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
            bar("AAA", at, open, close)
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
