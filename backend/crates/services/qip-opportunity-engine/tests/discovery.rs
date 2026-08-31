//! Detection and opportunity ranking.

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::approx_eq;
use qip_core::{Context, Duration, Timestamp};
use qip_opportunity_engine::detector::{
    AnomalyKind, CorrelationBreakdownDetector, DetectionContext, Detector, DetectorRegistry,
    LiquidityDetector, ObservationDetector, RegimeDetector, ReturnAnomalyDetector,
    StructuralBreakDetector, VolatilityShiftDetector,
};
use qip_opportunity_engine::engine::{EngineConfig, OpportunityEngine};
use qip_opportunity_engine::opportunity::Investigation;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn context() -> Context {
    let (context, _clock) = Context::deterministic(now(), 11);
    context
}

/// A calm price path with no anomalies.
fn calm_prices(seed: u64, n: usize) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    let mut price = 100.0;
    (0..n)
        .map(|_| {
            price *= (rng.normal_with(0.0002, 0.006)).exp();
            price
        })
        .collect()
}

// --- individual detectors ---------------------------------------------------

#[test]
fn the_return_detector_finds_a_large_move_and_ignores_a_calm_series() {
    let detector = ReturnAnomalyDetector::default();

    let calm = DetectionContext::new(now()).with_prices("CALM", calm_prices(1, 200));
    assert!(
        detector.detect(&calm).is_empty(),
        "a calm series should not fire"
    );

    let mut shocked = calm_prices(1, 200);
    let last = shocked[shocked.len() - 1];
    shocked.push(last * 0.85); // a 15% drop
    let shocked = DetectionContext::new(now()).with_prices("SHOCK", shocked);

    let anomalies = detector.detect(&shocked);
    assert_eq!(anomalies.len(), 1);
    assert_eq!(anomalies[0].kind, AnomalyKind::PriceMove);
    assert!(anomalies[0].z_score < -5.0, "z {}", anomalies[0].z_score);
    // A 15% price fall is a -16.25% log return; the detector works in logs.
    assert!(
        approx_eq(anomalies[0].observed, (0.85f64).ln(), 1e-9),
        "{}",
        anomalies[0].observed
    );
}

#[test]
fn the_return_detector_uses_robust_statistics() {
    // With a plain mean and standard deviation, the outlier being searched for
    // inflates the deviation and hides itself.
    let mut prices = calm_prices(2, 120);
    // Two earlier shocks that would inflate a classical estimate.
    for index in [40, 80] {
        prices[index] *= 1.2;
    }
    let last = prices[prices.len() - 1];
    prices.push(last * 0.88);

    let anomalies = ReturnAnomalyDetector::default()
        .detect(&DetectionContext::new(now()).with_prices("X", prices));
    assert!(
        !anomalies.is_empty(),
        "the robust estimator should still see the final move"
    );
}

#[test]
fn the_volatility_detector_finds_a_shift_in_dispersion() {
    let mut rng = Xoshiro256::seeded(3);
    let mut price = 100.0;
    let mut prices = Vec::new();
    for i in 0..200 {
        let sd = if i < 160 { 0.005 } else { 0.030 };
        price *= (rng.normal_with(0.0, sd)).exp();
        prices.push(price);
    }

    let anomalies = VolatilityShiftDetector::default()
        .detect(&DetectionContext::new(now()).with_prices("VOL", prices));
    assert_eq!(anomalies.len(), 1);
    assert_eq!(anomalies[0].kind, AnomalyKind::VolatilityShift);
    assert!(anomalies[0].z_score > 0.0, "volatility rose");
    assert!(anomalies[0].observed > anomalies[0].expected * 2.0);

    // A calm series does not fire.
    assert!(
        VolatilityShiftDetector::default()
            .detect(&DetectionContext::new(now()).with_prices("CALM", calm_prices(4, 200)))
            .is_empty()
    );
}

#[test]
fn the_structural_break_detector_separates_drift_from_a_single_outlier() {
    // A single large move is not a break.
    let mut spike = calm_prices(5, 200);
    let last = spike[spike.len() - 1];
    spike.push(last * 1.20);
    let from_spike = StructuralBreakDetector::default()
        .detect(&DetectionContext::new(now()).with_prices("SPIKE", spike));
    assert!(
        from_spike.is_empty(),
        "one outlier is not a structural break"
    );

    // A sustained shift in the mean is. The shift has to be large relative to
    // the noise — around one standard deviation — because the detector is tuned
    // so that a false alarm takes thousands of observations to arrive.
    let mut rng = Xoshiro256::seeded(6);
    let mut price = 100.0;
    let mut drifting = Vec::new();
    for i in 0..260 {
        let drift = if i < 200 { 0.0 } else { -0.008 };
        price *= (rng.normal_with(drift, 0.006)).exp();
        drifting.push(price);
    }
    let from_drift = StructuralBreakDetector::default()
        .detect(&DetectionContext::new(now()).with_prices("DRIFT", drifting));
    assert_eq!(from_drift.len(), 1, "sustained drift is a break");
    assert!(from_drift[0].z_score < 0.0, "the drift was downward");
    assert_eq!(from_drift[0].kind, AnomalyKind::StructuralBreak);
}

#[test]
fn the_regime_detector_fires_on_a_transition_and_not_on_a_stable_series() {
    // The model needs enough of both states to estimate them, and the
    // transition has to be recent to count as news rather than as the state of
    // the world: an earlier stress episode, a calm stretch, then a fresh turn.
    let mut rng = Xoshiro256::seeded(7);
    let mut price = 100.0;
    let mut prices = Vec::new();
    for i in 0..420 {
        let stressed = (250..320).contains(&i) || i >= 408;
        let sd = if stressed { 0.035 } else { 0.004 };
        price *= (rng.normal_with(0.0, sd)).exp();
        prices.push(price);
    }

    let anomalies = RegimeDetector::default()
        .detect(&DetectionContext::new(now()).with_prices("REGIME", prices));
    assert_eq!(anomalies.len(), 1, "a fresh transition should be reported");
    assert_eq!(anomalies[0].kind, AnomalyKind::RegimeChange);
    assert!(
        anomalies[0].observed > 0.75,
        "state probability {}",
        anomalies[0].observed
    );
    assert!(anomalies[0].description.contains("persist"));

    // A stable series has no transition to report.
    assert!(
        RegimeDetector::default()
            .detect(&DetectionContext::new(now()).with_prices("STABLE", calm_prices(8, 400)))
            .is_empty()
    );
}

#[test]
fn the_correlation_detector_finds_a_broken_relationship() {
    let mut rng = Xoshiro256::seeded(9);
    let mut a_price = 100.0;
    let mut b_price = 50.0;
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for i in 0..300 {
        let common = rng.normal_with(0.0, 0.01);
        a_price *= (common + rng.normal_with(0.0, 0.003)).exp();
        // The second instrument stops tracking the common factor at the end.
        let coupling = if i < 260 { 1.0 } else { 0.0 };
        b_price *= (coupling * common + rng.normal_with(0.0, 0.003)).exp();
        a.push(a_price);
        b.push(b_price);
    }

    let anomalies = CorrelationBreakdownDetector::default().detect(
        &DetectionContext::new(now())
            .with_prices("A", a)
            .with_prices("B", b),
    );
    assert_eq!(anomalies.len(), 1);
    assert_eq!(anomalies[0].kind, AnomalyKind::CorrelationBreakdown);
    assert!(
        anomalies[0].expected > 0.7,
        "baseline {}",
        anomalies[0].expected
    );
    // Thirty observations give a noisy estimate, so the bar is that it fell a
    // long way, not that it reached any particular level.
    assert!(
        anomalies[0].observed < anomalies[0].expected - 0.3,
        "recent {} against baseline {}",
        anomalies[0].observed,
        anomalies[0].expected
    );
    assert!(anomalies[0].subject.contains('|'), "the pair is named");
}

#[test]
fn an_uncorrelated_pair_is_not_watched() {
    let mut rng = Xoshiro256::seeded(10);
    let a: Vec<f64> = (0..300)
        .scan(100.0, |p, _| {
            *p *= (rng.normal_with(0.0, 0.01)).exp();
            Some(*p)
        })
        .collect();
    let b: Vec<f64> = (0..300)
        .scan(50.0, |p, _| {
            *p *= (rng.normal_with(0.0, 0.01)).exp();
            Some(*p)
        })
        .collect();

    let anomalies = CorrelationBreakdownDetector::default().detect(
        &DetectionContext::new(now())
            .with_prices("A", a)
            .with_prices("B", b),
    );
    assert!(anomalies.is_empty(), "there was no relationship to break");
}

#[test]
fn the_liquidity_detector_fires_on_widening_but_not_tightening() {
    // Real spreads vary; a perfectly constant series has zero dispersion and
    // no z-score is computable from it.
    let mut spreads: Vec<f64> = (0..60).map(|i| 3.0 + f64::from(i % 4) * 0.25).collect();
    spreads.push(45.0);
    let widened = LiquidityDetector::default()
        .detect(&DetectionContext::new(now()).with_spreads("WIDE", spreads));
    assert_eq!(widened.len(), 1);
    assert_eq!(widened[0].kind, AnomalyKind::LiquidityDeterioration);

    let mut tightened = vec![20.0; 60];
    // Vary slightly so the robust scale is non-zero.
    for (index, value) in tightened.iter_mut().enumerate() {
        *value += (index % 5) as f64;
    }
    tightened.push(1.0);
    assert!(
        LiquidityDetector::default()
            .detect(&DetectionContext::new(now()).with_spreads("TIGHT", tightened))
            .is_empty(),
        "a tighter spread is not a risk"
    );
}

#[test]
fn the_observation_detector_finds_a_surprise() {
    let mut surprises = vec![
        0.01, -0.02, 0.005, 0.0, -0.01, 0.015, -0.005, 0.02, 0.0, -0.015, 0.01, 0.0,
    ];
    surprises.push(0.35);
    let anomalies = ObservationDetector::new(AnomalyKind::FundamentalSurprise).detect(
        &DetectionContext::new(now())
            .with_observations("ent-northwind/revenue_surprise", surprises),
    );
    assert_eq!(anomalies.len(), 1);
    assert_eq!(anomalies[0].kind, AnomalyKind::FundamentalSurprise);
    assert!(anomalies[0].z_score > 3.0);
}

#[test]
fn detectors_do_nothing_with_insufficient_history() {
    let short = DetectionContext::new(now())
        .with_prices("X", vec![100.0, 101.0, 99.0])
        .with_spreads("X", vec![3.0, 4.0])
        .with_observations("Y", vec![0.1]);
    for anomalies in [
        ReturnAnomalyDetector::default().detect(&short),
        VolatilityShiftDetector::default().detect(&short),
        StructuralBreakDetector::default().detect(&short),
        RegimeDetector::default().detect(&short),
        CorrelationBreakdownDetector::default().detect(&short),
        LiquidityDetector::default().detect(&short),
        ObservationDetector::new(AnomalyKind::MacroSurprise).detect(&short),
    ] {
        assert!(
            anomalies.is_empty(),
            "a detector fired on three observations"
        );
    }
}

#[test]
fn every_detector_declares_its_noise_rate() {
    let registry = DetectorRegistry::standard();
    assert!(registry.len() >= 6);
    // Stated honestly: returns are fat-tailed, so a 3-sigma threshold fires far
    // more often than a normal distribution would imply.
    assert!(
        registry.expected_noise_rate() > 10.0,
        "an aggregate noise rate of {} looks implausibly low",
        registry.expected_noise_rate()
    );
    assert!(ReturnAnomalyDetector::default().expected_false_positive_rate() > 1.0);
}

// --- anomaly properties -----------------------------------------------------

#[test]
fn confidence_reflects_both_magnitude_and_sample_size() {
    let anomalies =
        ReturnAnomalyDetector::default().detect(&DetectionContext::new(now()).with_prices("X", {
            let mut prices = calm_prices(12, 200);
            let last = prices[prices.len() - 1];
            prices.push(last * 0.85);
            prices
        }));
    let large_sample = anomalies[0].confidence();

    let short = ReturnAnomalyDetector {
        window: 15,
        threshold: 3.0,
    }
    .detect(&DetectionContext::new(now()).with_prices("X", {
        let mut prices = calm_prices(12, 20);
        let last = prices[prices.len() - 1];
        prices.push(last * 0.85);
        prices
    }));
    assert!(!short.is_empty());
    assert!(
        large_sample > short[0].confidence(),
        "a large z-score on a short history is mostly evidence that the history is short"
    );
}

#[test]
fn importance_ranks_a_regime_change_above_a_volume_spike() {
    assert!(
        AnomalyKind::RegimeChange.base_importance() > AnomalyKind::VolumeSpike.base_importance()
    );
    assert!(
        AnomalyKind::StructuralBreak.base_importance() > AnomalyKind::PriceMove.base_importance(),
        "a persistent change matters more than a single move"
    );
}

// --- the engine -------------------------------------------------------------

fn shocked_context() -> DetectionContext {
    let mut prices = calm_prices(21, 250);
    let last = prices[prices.len() - 1];
    prices.push(last * 0.82);
    DetectionContext::new(now()).with_prices("OBJ-NWSC", prices)
}

#[test]
fn the_engine_produces_a_ranked_opportunity() {
    let mut engine = OpportunityEngine::default();
    let opportunities = engine.scan(&shocked_context(), &context());

    assert_eq!(opportunities.len(), 1);
    let opportunity = &opportunities[0];
    assert_eq!(opportunity.affected_entities, vec!["OBJ-NWSC".to_string()]);
    assert!(opportunity.rank.score > 0.0);
    assert!(
        approx_eq(opportunity.rank.novelty, 1.0, 1e-9),
        "the first alert is fully novel"
    );
    assert!(opportunity.rank.confidence > 0.4);
    assert!(!opportunity.required_investigation.is_empty());
    assert!(
        opportunity
            .required_investigation
            .contains(&Investigation::DataVerification)
    );
    assert!(opportunity.investigation_cost() > 0.0);
    assert!(opportunity.is_live(now()));
    assert!(opportunity.brief().contains("Investigation required"));
    assert_eq!(engine.emitted_count(), 1);
}

#[test]
fn repeated_alerts_on_the_same_subject_lose_novelty() {
    // A research queue that ranks the fifth alert about one instrument as
    // highly as the first fills up with a single story.
    let mut engine = OpportunityEngine::default();
    let context = context();
    let detection = shocked_context();

    let first = engine.scan(&detection, &context);
    let first_score = first[0].rank.score;

    let mut later = DetectionContext::new(now().saturating_add(Duration::from_days(1)));
    later.prices = detection.prices.clone();
    let second = engine.scan(&later, &context);
    assert_eq!(second.len(), 1);
    assert!(
        second[0].rank.novelty < 1.0,
        "novelty {} should have decayed",
        second[0].rank.novelty
    );
    assert!(second[0].rank.score < first_score);
    assert!(second[0].historical_context.contains("prior alert"));
}

#[test]
fn corroborating_detectors_raise_confidence_above_any_one_of_them() {
    // Several independent detectors agreeing is stronger evidence than one
    // firing loudly.
    // Volatility rises and then a large move lands: two independent detectors
    // should see the same event.
    let mut rng = Xoshiro256::seeded(23);
    let mut price = 100.0;
    let mut prices = Vec::new();
    for i in 0..300 {
        let sd = if i < 250 { 0.004 } else { 0.030 };
        price *= (rng.normal_with(0.0, sd)).exp();
        prices.push(price);
    }
    let last = prices[prices.len() - 1];
    prices.push(last * 0.80);

    let mut engine = OpportunityEngine::default();
    let opportunities = engine.scan(
        &DetectionContext::new(now()).with_prices("MULTI", prices),
        &context(),
    );
    assert_eq!(opportunities.len(), 1);
    let opportunity = &opportunities[0];
    assert!(
        opportunity.detectors.len() >= 2,
        "expected corroboration, got {:?}",
        opportunity.detectors
    );
    let strongest_alone = opportunity
        .anomalies
        .iter()
        .map(|a| a.confidence())
        .fold(0.0f64, f64::max);
    assert!(
        opportunity.rank.confidence > strongest_alone,
        "combined {} should exceed the strongest single {strongest_alone}",
        opportunity.rank.confidence
    );
}

#[test]
fn the_queue_is_capped_and_the_suppressed_count_is_visible() {
    let mut engine = OpportunityEngine::new(
        DetectorRegistry::standard(),
        EngineConfig {
            max_per_cycle: 2,
            ..EngineConfig::default()
        },
    );

    let mut detection = DetectionContext::new(now());
    for index in 0..8 {
        let mut prices = calm_prices(100 + index, 250);
        let last = prices[prices.len() - 1];
        prices.push(last * 0.82);
        detection.prices.insert(format!("OBJ-{index}"), prices);
    }

    let opportunities = engine.scan(&detection, &context());
    assert_eq!(opportunities.len(), 2, "the cap must be hard, not advisory");
    assert!(
        engine.suppressed_count() > 0,
        "what was dropped must be visible"
    );
}

#[test]
fn the_queue_is_ordered_by_value_per_unit_of_investigation_cost() {
    let mut engine = OpportunityEngine::new(
        DetectorRegistry::standard(),
        EngineConfig {
            max_per_cycle: 10,
            ..EngineConfig::default()
        },
    );
    let mut detection = DetectionContext::new(now());
    for (index, magnitude) in [0.70, 0.85, 0.92].iter().enumerate() {
        let mut prices = calm_prices(200 + index as u64, 250);
        let last = prices[prices.len() - 1];
        prices.push(last * magnitude);
        detection.prices.insert(format!("OBJ-{index}"), prices);
    }

    let opportunities = engine.scan(&detection, &context());
    assert!(opportunities.len() >= 2);
    for pair in opportunities.windows(2) {
        assert!(
            pair[0].rank.value_per_cost() >= pair[1].rank.value_per_cost() - 1e-12,
            "queue out of order"
        );
    }
    // The largest move does not automatically lead: ranking is cost-adjusted,
    // so a finding that is cheap to confirm can outrank a bigger one that is
    // expensive. What must hold is that the raw importance still tracks size.
    let biggest = opportunities
        .iter()
        .find(|o| o.affected_entities[0] == "OBJ-0")
        .expect("the largest move should be in the queue");
    assert!(
        biggest.rank.importance
            >= opportunities
                .iter()
                .map(|o| o.rank.importance)
                .fold(0.0f64, f64::max)
                - 1e-9
    );
}

#[test]
fn a_calm_market_produces_no_opportunities() {
    let mut engine = OpportunityEngine::default();
    let mut detection = DetectionContext::new(now());
    for index in 0..5 {
        detection
            .prices
            .insert(format!("OBJ-{index}"), calm_prices(300 + index, 250));
    }
    assert!(
        engine.scan(&detection, &context()).is_empty(),
        "a quiet market should produce a quiet queue"
    );
    assert_eq!(engine.emitted_count(), 0);
}

#[test]
fn scanning_is_deterministic() {
    let run = || {
        let mut engine = OpportunityEngine::default();
        engine
            .scan(&shocked_context(), &context())
            .into_iter()
            .map(|o| format!("{}:{:.9}", o.affected_entities[0], o.rank.score))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

#[test]
fn an_opportunity_expires() {
    let mut engine = OpportunityEngine::default();
    let opportunities = engine.scan(&shocked_context(), &context());
    let opportunity = &opportunities[0];
    assert!(opportunity.is_live(now()));
    assert!(!opportunity.is_live(now().saturating_add(Duration::from_days(30))));
}

#[test]
fn resetting_clears_the_novelty_history() {
    let mut engine = OpportunityEngine::default();
    let context = context();
    engine.scan(&shocked_context(), &context);
    engine.reset();
    let after = engine.scan(&shocked_context(), &context);
    assert!(
        approx_eq(after[0].rank.novelty, 1.0, 1e-9),
        "reset must restore novelty"
    );
    assert_eq!(engine.emitted_count(), 1);
}

#[test]
fn the_regime_detector_fits_a_bounded_recent_window_however_long_the_series_grows() {
    // The failure this prevents has happened: this was the only detector
    // whose lookback was the whole series, and its fit is rebuilt from
    // scratch at O(window x EM iterations) per instrument per scan — so on
    // the deployed fastbrain its cost grew with uptime until the cycle
    // breached its 50ms ceiling. The fit window is the detector's stated
    // lookback, like every other detector's, and the anomaly's sample_size
    // is where that statement is visible from outside.
    let detector = RegimeDetector::default();

    // The firing fixture from the transition test, with two hundred further
    // calm observations in front of it, so the series is longer than the
    // window while the stress episode and the fresh turn both sit inside it.
    let mut rng = Xoshiro256::seeded(7);
    let mut price = 100.0;
    let mut prices = Vec::new();
    for _ in 0..200 {
        price *= (rng.normal_with(0.0, 0.004)).exp();
        prices.push(price);
    }
    for i in 0..420 {
        let stressed = (250..320).contains(&i) || i >= 408;
        let sd = if stressed { 0.035 } else { 0.004 };
        price *= (rng.normal_with(0.0, sd)).exp();
        prices.push(price);
    }
    assert!(
        prices.len() - 1 > detector.fit_window,
        "the premise: the series carries more returns than the fit window"
    );

    let anomalies = detector.detect(&DetectionContext::new(now()).with_prices("REGIME", prices));
    assert_eq!(
        anomalies.len(),
        1,
        "the fresh transition inside the window should still be reported"
    );
    assert_eq!(
        anomalies[0].sample_size, detector.fit_window,
        "the fit read {} returns where its stated lookback is {}; an unbounded fit is the \
         detector whose cost grows with uptime",
        anomalies[0].sample_size, detector.fit_window
    );
}

#[test]
fn the_regime_fit_work_per_instrument_is_bounded_by_its_stated_budget() {
    // The control being asserted is the configuration itself, the way a risk
    // limit is asserted at its configured value: the fit's worst case is
    // exactly `EM_MAX_ITERATIONS x fit_window` per instrument — the
    // tolerance can only end a fit early — and that product is what has to
    // fit inside a 50ms cycle alongside four other instruments and seven
    // other detectors. The budget was measured at ~21ms across five
    // instruments in an unoptimised build at 25 x 250; the previous
    // configuration (120 iterations over an unbounded series) was 250ms and
    // growing when the deployed fastbrain breached its ceiling.
    let detector = RegimeDetector::default();
    assert!(
        detector.fit_window >= detector.minimum_history,
        "the premise: the window serves at least the detector's own minimum history"
    );
    assert!(
        RegimeDetector::EM_MAX_ITERATIONS * detector.fit_window <= 25 * 250,
        "the regime fit may spend {} iteration-observations per instrument per scan, past \
         the 6,250 the cycle budget was measured against; whoever raises this owns the \
         re-measurement",
        RegimeDetector::EM_MAX_ITERATIONS * detector.fit_window
    );
}
