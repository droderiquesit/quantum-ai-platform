//! The self-model's arithmetic, and its refusals.
//!
//! Each test names the failure it prevents. The one that matters most is the
//! first: an estimate reported below the minimum sample would read as a
//! measurement to a consumer that scales by it, and a coin-flip on no
//! evidence is not a measurement.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::testing::approx_eq;
use qip_core::time::{Duration, Timestamp};
use qip_learning_engine::evaluation::{Evaluation, Verdict};
use qip_learning_engine::self_model::{
    CAPABILITY_WINDOW, CapabilityEstimate, ComponentKey, ComponentKind, DRIFT_TOLERANCE,
    MAX_COMPONENTS, MINIMUM_SAMPLE, ScoredOutcome, SelfModel,
};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn at(n: usize) -> Timestamp {
    start().saturating_add(Duration::from_days(n as i64))
}

fn outcome(n: usize, correct: bool) -> ScoredOutcome {
    ScoredOutcome::new(format!("hyp-{n}"), 0.7, correct, at(n)).expect("a probability")
}

fn evaluation(id: &str, verdict: Verdict, confidence: f64) -> Evaluation {
    Evaluation {
        hypothesis_id: id.to_string(),
        class: "price_dislocation".to_string(),
        subject: String::new(),
        verdict,
        expected_move_bps: 100.0,
        realised_move_bps: 90.0,
        magnitude_ratio: 0.9,
        confidence,
        realised_pnl: 0.0,
        contributors: vec!["run-macro-analyst-1".to_string()],
        evaluated_at: start(),
        rationale: "fixture".to_string(),
        horizon: qip_core::time::Duration::from_secs(3600),
    }
}

#[test]
fn an_estimate_is_refused_below_the_minimum_sample_rather_than_reported_as_one_half() {
    // The failure this guards: a component with three outcomes reported at
    // 0.5, indistinguishable from one measured at 0.5 over a hundred, and a
    // consumer halved its weight on no evidence.
    let mut estimate = CapabilityEstimate::new();
    for n in 0..MINIMUM_SAMPLE - 1 {
        estimate.record(outcome(n, true));
    }
    // Premise: the record is one short of the minimum and every outcome in
    // it is a hit, so a formula that reported anyway would report near one.
    assert_eq!(estimate.sample_count(), MINIMUM_SAMPLE - 1);
    assert_eq!(estimate.hits(), MINIMUM_SAMPLE - 1);
    assert!(!estimate.is_estimable());

    let error = estimate
        .estimate()
        .expect_err("an estimate was reported below the minimum sample");
    assert!(
        error
            .message()
            .contains(&format!("{} graded outcome(s)", MINIMUM_SAMPLE - 1)),
        "the refusal must name the sample it has: {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains(&format!("below the {MINIMUM_SAMPLE}")),
        "the refusal must name the minimum: {}",
        error.message()
    );

    // One more outcome and the estimate exists — the bar is the minimum
    // itself, not one past it.
    estimate.record(outcome(MINIMUM_SAMPLE, true));
    assert!(estimate.is_estimable());
    assert!(estimate.estimate().is_ok());
}

#[test]
fn a_component_that_is_always_wrong_estimates_near_zero_and_one_always_right_near_one() -> Result<()>
{
    // The failure this guards: an estimate that could not tell the two apart
    // — a hit rate averaged with something else, or a shrinkage so heavy the
    // record never overcame it — would weight a broken detector like a
    // working one. The formula is (h + k/2) / (n + k) with k = 4; stated
    // here so the numbers below are checkable by hand.
    const N: usize = 40;
    let mut wrong = CapabilityEstimate::new();
    let mut right = CapabilityEstimate::new();
    for n in 0..N {
        wrong.record(outcome(n, false));
        right.record(outcome(n, true));
    }
    assert_eq!(wrong.sample_count(), N);
    assert_eq!(right.sample_count(), N);

    let wrong = wrong.estimate()?;
    let right = right.estimate()?;
    // (0 + 2) / (40 + 4) and (40 + 2) / (40 + 4).
    assert!(
        approx_eq(wrong.accuracy, 2.0 / 44.0, 1e-12),
        "always wrong estimated {}",
        wrong.accuracy
    );
    assert!(
        approx_eq(right.accuracy, 42.0 / 44.0, 1e-12),
        "always right estimated {}",
        right.accuracy
    );
    assert!(
        wrong.accuracy < 0.1,
        "always wrong is not near zero: {}",
        wrong.accuracy
    );
    assert!(
        right.accuracy > 0.9,
        "always right is not near one: {}",
        right.accuracy
    );
    // Never exactly zero or one: the pseudo-counts are what keep a wrong
    // component discounted rather than silenced.
    assert!(wrong.accuracy > 0.0);
    assert!(right.accuracy < 1.0);
    assert!(approx_eq(wrong.hit_rate, 0.0, 1e-12));
    assert!(approx_eq(right.hit_rate, 1.0, 1e-12));
    // Brier at confidence 0.7: (0.7 - 0)^2 = 0.49 when wrong, 0.09 when right.
    assert!(
        approx_eq(wrong.mean_brier, 0.49, 1e-12),
        "{}",
        wrong.mean_brier
    );
    assert!(
        approx_eq(right.mean_brier, 0.09, 1e-12),
        "{}",
        right.mean_brier
    );
    assert_eq!(wrong.sample_count, N);
    assert_eq!(right.last_updated, at(N - 1));
    Ok(())
}

#[test]
fn the_window_is_bounded_and_keeps_the_newest_outcomes() -> Result<()> {
    // The failure this guards: a record that grew without bound, so the
    // estimate averaged a detector's whole life and a retuned detector was
    // judged for a year on the version before — and the working set grew
    // with every resolved thesis until the process was paged out.
    let mut estimate = CapabilityEstimate::new();
    let recorded = CAPABILITY_WINDOW + 10;
    // The first ten are misses; if they survive, the hit rate shows it.
    for n in 0..recorded {
        estimate.record(outcome(n, n >= 10));
    }
    assert!(recorded > CAPABILITY_WINDOW, "the premise is an overflow");
    assert_eq!(estimate.sample_count(), CAPABILITY_WINDOW);
    assert_eq!(
        estimate.hits(),
        CAPABILITY_WINDOW,
        "a miss from before the window survived eviction"
    );
    let oldest = estimate.outcomes().next().expect("a full window");
    assert_eq!(oldest.hypothesis_id, "hyp-10", "the wrong end was evicted");
    assert_eq!(estimate.last_updated(), Some(at(recorded - 1)));
    assert!(approx_eq(estimate.estimate()?.hit_rate, 1.0, 1e-12));
    Ok(())
}

#[test]
fn the_model_evicts_the_least_recently_updated_component_past_its_bound() -> Result<()> {
    // The failure this guards: a component per strategy family per venue
    // per regime, accumulating forever. Past the cap the stalest goes, and
    // a component refreshed since assembly outlives one that was not.
    let mut model = SelfModel::new();
    for n in 0..MAX_COMPONENTS {
        model.record(
            ComponentKey::strategy(format!("s{n:04}"))?,
            outcome(n, true),
        );
    }
    assert_eq!(model.len(), MAX_COMPONENTS, "the premise is a full model");
    // Refresh the oldest, so it is no longer the stalest.
    model.record(
        ComponentKey::strategy("s0000")?,
        outcome(MAX_COMPONENTS, true),
    );
    // One more component tips it over, and s0001 — now the stalest — goes.
    model.record(
        ComponentKey::strategy("overflow")?,
        outcome(MAX_COMPONENTS + 1, true),
    );
    assert_eq!(model.len(), MAX_COMPONENTS);
    assert!(
        model.get(&ComponentKey::strategy("s0000")?).is_some(),
        "the refreshed one was evicted"
    );
    assert!(
        model.get(&ComponentKey::strategy("s0001")?).is_none(),
        "the stalest one survived"
    );
    assert!(model.get(&ComponentKey::strategy("overflow")?).is_some());
    Ok(())
}

#[test]
fn absorbing_an_evaluation_charges_every_named_component_and_skips_an_inconclusive_one()
-> Result<()> {
    // The failure this guards: a quiet tape counted against the detector.
    // An inconclusive verdict says nothing about the component, and charging
    // it as a miss would make every detector look broken in a flat market.
    let mut model = SelfModel::new();
    let detector = ComponentKey::detector("price_dislocation")?;
    let analyst = ComponentKey::analyst("macro-analyst")?;
    let keys = vec![detector.clone(), analyst.clone()];

    let charged = model.absorb(&evaluation("hyp-quiet", Verdict::Inconclusive, 0.7), &keys)?;
    assert_eq!(charged, 0, "an inconclusive verdict charged a component");
    assert!(model.is_empty());

    let charged = model.absorb(&evaluation("hyp-hit", Verdict::Vindicated, 0.7), &keys)?;
    assert_eq!(charged, 2);
    let charged = model.absorb(
        &evaluation("hyp-lucky", Verdict::RightForTheWrongReason, 0.9),
        &keys,
    )?;
    assert_eq!(charged, 2);
    for key in &keys {
        let record = model.get(key).expect("charged");
        assert_eq!(record.sample_count(), 2);
        // Right for the wrong reason is not a hit: counting luck as skill is
        // the evaluation module's founding refusal, and it holds here.
        assert_eq!(record.hits(), 1, "{key} counted a lucky outcome as a hit");
    }
    Ok(())
}

#[test]
fn origin_factors_name_only_measured_detectors_and_analysts_and_keep_the_lower_on_a_clash()
-> Result<()> {
    // The failure this guards: a factor handed to REASON for an origin with
    // two outcomes, or for a rung — which is never an evidence origin — or
    // an ambiguous id resolved to whichever kind was inserted last.
    let mut model = SelfModel::new();
    let measured = ComponentKey::detector("volatility_shift")?;
    let thin = ComponentKey::analyst("credit-analyst")?;
    let rung = ComponentKey::rung("multi_agent_reasoning")?;
    let clash_detector = ComponentKey::detector("shared")?;
    let clash_analyst = ComponentKey::analyst("shared")?;
    for n in 0..MINIMUM_SAMPLE {
        model.record(measured.clone(), outcome(n, n % 2 == 0));
        model.record(rung.clone(), outcome(n, true));
        model.record(clash_detector.clone(), outcome(n, true));
        model.record(clash_analyst.clone(), outcome(n, false));
    }
    model.record(thin.clone(), outcome(0, true));
    assert!(
        model.factor(&thin).is_none(),
        "a thin sample produced a factor"
    );
    assert!(
        model.factor(&rung).is_some(),
        "the premise is a measured rung"
    );

    let factors = model.origin_factors();
    let named: Vec<&String> = factors.keys().collect();
    assert_eq!(
        named,
        vec!["shared", "volatility_shift"],
        "wrong origins offered: {factors:?}"
    );
    // Half right over ten: (5 + 2) / (10 + 4).
    assert!(approx_eq(factors["volatility_shift"], 7.0 / 14.0, 1e-12));
    // The clash keeps the always-wrong analyst's factor, not the detector's.
    assert!(
        approx_eq(factors["shared"], 2.0 / 14.0, 1e-12),
        "{}",
        factors["shared"]
    );
    Ok(())
}

#[test]
fn sample_facts_report_every_component_with_its_count_and_newest_grade_in_key_order() -> Result<()>
{
    // The failure this guards: the §6.2 self-model row derives from exactly
    // these three facts, so a count off by one, a newest instant that was
    // really the oldest, or a component left out would narrow — or fail to
    // narrow — on a record the model does not hold. Key order is what makes
    // the first thin component the table names the same on every replay.
    let mut model = SelfModel::new();
    let detector = ComponentKey::detector("volatility_shift")?;
    let analyst = ComponentKey::analyst("macro-analyst")?;
    model.record(analyst.clone(), outcome(3, true));
    model.record(analyst.clone(), outcome(7, false));
    for n in 0..MINIMUM_SAMPLE {
        model.record(detector.clone(), outcome(n, true));
    }
    // Premise: two components with different counts and different newest
    // instants, the analyst inserted first. `ComponentKey` orders by kind
    // before id and `Detector` precedes `Analyst`, so key order puts the
    // detector first — the opposite of insertion order, which is what makes
    // the ordering assertion below about the key and not the sequence.
    assert_eq!(model.len(), 2);
    assert!(
        detector < analyst,
        "the premise is that key order is not insertion order"
    );

    let facts: Vec<(String, usize, Option<Timestamp>)> = model.sample_facts().collect();
    assert_eq!(
        facts,
        vec![
            (
                "detector:volatility_shift".to_string(),
                MINIMUM_SAMPLE,
                Some(at(MINIMUM_SAMPLE - 1))
            ),
            ("analyst:macro-analyst".to_string(), 2, Some(at(7))),
        ],
        "the facts do not match the record: {facts:?}"
    );
    assert!(
        SelfModel::new().sample_facts().next().is_none(),
        "an empty model reported a component"
    );
    Ok(())
}

#[test]
fn a_component_key_refuses_an_empty_id_and_one_carrying_its_own_separator() -> Result<()> {
    // The failure this guards: an unnamed component pooling every unnamed
    // source, and a key whose id held the `kind:id` separator, which could
    // be written to the journal and never read back as the same key.
    assert!(ComponentKey::new(ComponentKind::Detector, "  ").is_err());
    assert!(ComponentKey::new(ComponentKind::Analyst, "a:b").is_err());
    let key = ComponentKey::analyst("macro-analyst")?;
    assert_eq!(key.to_string(), "analyst:macro-analyst");
    assert!(
        ScoredOutcome::new("hyp", 1.5, true, start()).is_err(),
        "a confidence above one was accepted as an outcome"
    );
    Ok(())
}

/// R12-MODEL-6016: a world-model member's resolved forecast is charged to its
/// own component, the side it took scored against what happened, and a
/// forecast of exactly one half charges nothing. Its record is kept out of
/// the freshness facts: counting it there let a federation's resolutions
/// read the self-model fresh and lift the sizing bound its staleness holds.
/// Mutations: scoring the raw probability as the side (no `says_true`
/// fold) fails the hit count; removing the `sample_facts` filter lists the
/// member.
#[test]
fn a_world_model_forecast_is_charged_by_the_side_it_took_and_never_freshens_the_self_model()
-> Result<()> {
    let mut model = SelfModel::new();
    let member = ComponentKey::new(ComponentKind::WorldModel, "markets.micro")?;
    assert!(model.absorb_forecast(member.clone(), "p1", 0.2, false, at(1))?);
    assert!(model.absorb_forecast(member.clone(), "p2", 0.8, false, at(2))?);
    assert!(!model.absorb_forecast(member.clone(), "p3", 0.5, true, at(3))?);
    assert!(
        model
            .absorb_forecast(member.clone(), "p4", 1.5, true, at(4))
            .is_err()
    );

    let estimate = model.get(&member).expect("the member was charged");
    // Premise: two forecasts took a side; the coin flip did not count.
    assert_eq!(estimate.sample_count(), 2);
    // 0.2 on a false outcome called it; 0.8 on a false outcome missed.
    assert_eq!(estimate.hits(), 1);
    assert_eq!(model.sample_facts().count(), 0);
    Ok(())
}

/// A confident outcome: stated at `confidence`, right or wrong.
fn graded(n: usize, confidence: f64, correct: bool) -> ScoredOutcome {
    ScoredOutcome::new(format!("hyp-{n}"), confidence, correct, at(n)).expect("a probability")
}

/// A component right for its older twenty outcomes and wrong for its newer
/// twenty, all stated at 0.8: the window average still reads it as a coin
/// flip's worth of accuracy, and nothing in that number says the detector or
/// the data behind it has moved. Mutation: make `Capability::weight` return
/// `accuracy` unconditionally — the factor stays at the window's 0.5 and the
/// last assertion fails.
#[test]
fn a_component_that_has_started_missing_is_weighted_by_its_recent_record_not_its_window_average()
-> Result<()> {
    let key = ComponentKey::detector("vol-spike")?;
    let mut model = SelfModel::new();
    for n in 0..20 {
        model.record(key.clone(), graded(n, 0.8, true));
    }
    let steady = model.estimate(&key)?;
    assert!(
        steady.drift.is_none(),
        "premise: twenty straight hits is not drift: {:?}",
        steady.drift
    );
    for n in 20..40 {
        model.record(key.clone(), graded(n, 0.8, false));
    }

    let capability = model.estimate(&key)?;
    let drift = capability.drift.as_ref().expect("the record has worsened");
    assert!(drift.worsening() > 0.5, "{drift:?}");
    // 20 of 40 over the window, shrunk: (20 + 2) / (40 + 4) = 0.5.
    assert!(approx_eq(capability.accuracy, 0.5, 1e-9), "{capability:?}");
    // 0 of 20 lately, shrunk: (0 + 2) / (20 + 4).
    assert!(
        approx_eq(drift.recent_accuracy, 2.0 / 24.0, 1e-9),
        "{drift:?}"
    );
    let factor = model.factor(&key).expect("measured");
    assert!(
        approx_eq(factor, 2.0 / 24.0, 1e-9),
        "a drifting component kept its window weight {factor}"
    );
    assert!(
        approx_eq(model.origin_factors()["vol-spike"], factor, 1e-12),
        "REASON is handed a different factor from the one the model reports"
    );
    Ok(())
}

/// The other side of the tolerance, and of the direction: a component that
/// has improved, and one whose halves differ by less than the tolerance,
/// keep their window accuracy. A drift test that fired on a lucky streak, or
/// on getting better, would discount the platform's best components.
/// Mutation: drop the `<= DRIFT_TOLERANCE` return (always report drift) —
/// the steady component reports drift and the first assertion fails.
#[test]
fn a_component_that_improved_or_wobbled_inside_the_tolerance_is_not_called_drifting() -> Result<()>
{
    let mut steady = CapabilityEstimate::new();
    for n in 0..40 {
        steady.record(graded(n, 0.7, n % 2 == 0));
    }
    assert!(steady.is_estimable(), "premise: the record is long enough");
    assert!(steady.drift().is_none(), "{:?}", steady.drift());

    let mut improving = CapabilityEstimate::new();
    for n in 0..20 {
        improving.record(graded(n, 0.8, false));
    }
    for n in 20..40 {
        improving.record(graded(n, 0.8, true));
    }
    assert!(improving.drift().is_none(), "{:?}", improving.drift());

    let mut short = CapabilityEstimate::new();
    for n in 0..MINIMUM_SAMPLE {
        short.record(graded(n, 0.8, true));
    }
    for n in MINIMUM_SAMPLE..2 * MINIMUM_SAMPLE - 1 {
        short.record(graded(n, 0.8, false));
    }
    assert!(
        short.drift().is_none(),
        "a window under twice the minimum sample was judged: {:?}",
        short.drift()
    );
    Ok(())
}

/// Forty outcomes at 0.7 confidence in which the older twenty miss
/// `older_misses` times and the newer twenty `newer_misses` times. A miss
/// scores 0.49 and a hit 0.09, so each extra miss in the newer half worsens
/// the mean Brier by exactly 0.02.
fn two_halves(older_misses: usize, newer_misses: usize) -> CapabilityEstimate {
    let mut record = CapabilityEstimate::new();
    for n in 0..20 {
        record.record(graded(n, 0.7, n >= older_misses));
    }
    for n in 20..40 {
        record.record(graded(n, 0.7, n - 20 >= newer_misses));
    }
    record
}

/// The tolerance has two sides and the tests held neither: the steady window
/// alternated hit and miss, so both halves scored identically and any
/// tolerance from zero to a half passed. One unlucky outcome (0.02 worse)
/// and two (0.04) are inside the tolerance and must not discount a
/// component; three (0.06) are outside it and must.
///
/// Mutation: compare against `0.0` instead of `DRIFT_TOLERANCE` in
/// `CapabilityEstimate::drift` — the one-extra-miss window is called
/// drifting and the first assertion fails. Against `0.45` — the 0.06 window
/// is not called drifting and the last `expect` fails. Against `0.03` and
/// `0.07` the two-extra-miss and three-extra-miss windows fail likewise.
#[test]
fn a_window_worse_by_less_than_the_tolerance_is_not_drifting_and_one_worse_by_more_is() {
    const {
        assert!(CAPABILITY_WINDOW >= 40, "premise: the window holds forty");
        assert!(
            0.04 < DRIFT_TOLERANCE && DRIFT_TOLERANCE < 0.06,
            "premise: the cases below straddle the tolerance"
        );
    }
    for newer_misses in [3, 4] {
        let record = two_halves(2, newer_misses);
        assert!(record.is_estimable(), "premise: the record is long enough");
        assert!(
            record.drift().is_none(),
            "{} extra misses are inside the tolerance: {:?}",
            newer_misses - 2,
            record.drift()
        );
    }
    let record = two_halves(2, 5);
    let drift = record
        .drift()
        .expect("three extra misses are past the tolerance");
    assert!(
        approx_eq(drift.worsening(), 0.06, 1e-9),
        "premise: the window is worse by 0.06, got {}",
        drift.worsening()
    );
}
