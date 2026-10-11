//! The Temporal Forecast Lattice (blueprint v12.0 §9.7, §9.14; GCP v3.0
//! §11.3): five fixed horizons with their own populations and promotion
//! targets, calibrated distributions, cross-horizon constraints, routing with
//! a fallback, per-horizon and per-regime scoring, and weights that freeze
//! when the calibrator is unavailable.
//!
//! Each test asserts its premise first and names, in its doc comment, the
//! mutation of `src/lattice.rs` that should make it fail.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::forecast::{Calibration, ForecastFields, OutcomeDistribution, QuantilePoint};
use qip_core::Decimal;
use qip_core::time::{Duration, Timestamp};
use qip_world_model::lattice::{
    Calibrator, InconsistencyKind, Lane, LatticeHorizon, LatticeModel, Population, PromotionTarget,
    Question, Reputation, Resolution, TargetInput, WeightUpdate, constrain, issue, route,
    staleness_multiplier,
};
use std::collections::BTreeMap;

fn t0() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn input() -> TargetInput {
    TargetInput {
        target: "obj-AAA/close".into(),
        level: 100.0,
        level_variance: 0.25,
        drift_per_day: Some(0.1),
        variance_per_day: 1.0,
        regime_confidence_bp: 7_000,
        held_quantity: 10.0,
        evidence: vec!["kernel/bar-history#obj-AAA".into()],
    }
}

fn issued(horizon: LatticeHorizon) -> qip_world_model::lattice::Issued {
    let population = Population::standard().unwrap();
    let routed = route(
        &population,
        &Question {
            target: "obj-AAA/close".into(),
            domain: "markets".into(),
            horizon,
        },
        None,
    )
    .unwrap();
    issue(
        Lane::Central,
        &input(),
        horizon,
        &population,
        &routed,
        Calibration::Uncalibrated,
        t0(),
    )
    .unwrap()
}

/// WORLD-3084 and WORLD-3085: a model aimed at any lane but its horizon's
/// promotion target is refused at admission. Mutation: drop the target check
/// in `Population::admit`.
#[test]
fn a_model_aimed_anywhere_but_its_horizons_promotion_target_is_refused_at_admission() {
    let mut population = Population::new();
    // Premise: the correct target is admitted for each row.
    for horizon in LatticeHorizon::ALL {
        population
            .admit(LatticeModel {
                id: format!("{}/ok", horizon.as_str()),
                horizon,
                target: horizon.promotion_target(),
                uses_drift: false,
            })
            .unwrap();
    }
    // Every row against every target it does not promote to. This tried
    // wrong targets for the microsecond and seconds rows only until
    // 2026-10-11, so a check that admitted anything for hours-days,
    // weeks-months or quarters-years — the rows that reach capital and
    // strategic allocation — passed it.
    let targets = [
        PromotionTarget::LocalReflexOnly,
        PromotionTarget::ReflexWarmPackages,
        PromotionTarget::StrategyCapitalRisk,
        PromotionTarget::PortfolioTreasury,
        PromotionTarget::StrategicAllocationResearch,
    ];
    let mut tried = 0;
    for horizon in LatticeHorizon::ALL {
        for wrong in targets {
            if wrong == horizon.promotion_target() {
                continue;
            }
            tried += 1;
            let refused = population.admit(LatticeModel {
                id: format!("{}/wrong/{}", horizon.as_str(), wrong.as_str()),
                horizon,
                target: wrong,
                uses_drift: false,
            });
            assert!(
                refused.is_err(),
                "{horizon:?} admitted a model aimed at {wrong:?}"
            );
        }
    }
    assert_eq!(tried, 20, "premise: five rows, four wrong targets each");
    // Each row's one right target, pinned, so a row whose target moved is a
    // decision made here rather than a test that followed it.
    for (horizon, right) in [
        (
            LatticeHorizon::MicrosecondsMs,
            PromotionTarget::LocalReflexOnly,
        ),
        (
            LatticeHorizon::SecondsMinutes,
            PromotionTarget::ReflexWarmPackages,
        ),
        (
            LatticeHorizon::HoursDays,
            PromotionTarget::StrategyCapitalRisk,
        ),
        (
            LatticeHorizon::WeeksMonths,
            PromotionTarget::PortfolioTreasury,
        ),
        (
            LatticeHorizon::QuartersYears,
            PromotionTarget::StrategicAllocationResearch,
        ),
    ] {
        assert_eq!(horizon.promotion_target(), right, "{horizon:?}");
    }
}

/// WORLD-3084: the central lane may not issue the microsecond row, and the
/// reflex lane may not issue anything else. Mutation: drop the lane check in
/// `issue`.
#[test]
fn the_central_lane_may_not_issue_a_microsecond_forecast() {
    let population = Population::standard().unwrap();
    let q = |horizon| Question {
        target: "obj-AAA/close".into(),
        domain: "markets".into(),
        horizon,
    };
    let micro = route(&population, &q(LatticeHorizon::MicrosecondsMs), None).unwrap();
    assert!(
        issue(
            Lane::Central,
            &input(),
            LatticeHorizon::MicrosecondsMs,
            &population,
            &micro,
            Calibration::Uncalibrated,
            t0()
        )
        .is_err()
    );
    let day = route(&population, &q(LatticeHorizon::HoursDays), None).unwrap();
    assert!(
        issue(
            Lane::LocalReflex,
            &input(),
            LatticeHorizon::HoursDays,
            &population,
            &day,
            Calibration::Uncalibrated,
            t0()
        )
        .is_err()
    );
    // Premise: the right lane issues both.
    assert!(
        issue(
            Lane::LocalReflex,
            &input(),
            LatticeHorizon::MicrosecondsMs,
            &population,
            &micro,
            Calibration::Uncalibrated,
            t0()
        )
        .is_ok()
    );
}

/// WORLD-7197 and R2.5: every issued forecast is lane H's calibrated
/// distribution — quantiles, the uncertainty split, the models and their
/// spread, an expiry at the row's refresh. Mutation: put the between-model
/// variance in the aleatoric half, or issue one model only.
#[test]
fn every_issued_forecast_is_a_calibrated_distribution_with_its_uncertainty_split() {
    for horizon in [
        LatticeHorizon::SecondsMinutes,
        LatticeHorizon::HoursDays,
        LatticeHorizon::WeeksMonths,
        LatticeHorizon::QuartersYears,
    ] {
        let made = issued(horizon);
        let f = made.forecast.fields();
        assert_eq!(f.horizon, horizon.representative());
        assert_eq!(f.expires_at, t0().saturating_add(horizon.refresh()));
        let OutcomeDistribution::Quantiles { points } = &f.distribution else {
            panic!("a lattice forecast states quantiles");
        };
        assert_eq!(points.len(), 3);
        assert_eq!(
            f.models_consulted, 2,
            "both members of the row are consulted"
        );
        // The drift member and the martingale member disagree by the drift.
        let days = horizon.representative().as_days_f64();
        let expected = Decimal::from_f64(0.1 * days).unwrap();
        assert!((f.model_disagreement - expected).abs() < Decimal::from_f64(1e-6).unwrap());
        // Epistemic holds the level's uncertainty and the members' spread;
        // aleatoric holds the variance the horizon itself adds.
        let aleatoric = Decimal::from_f64(days).unwrap();
        assert!((f.uncertainty.aleatoric - aleatoric).abs() < Decimal::from_f64(1e-6).unwrap());
        assert!(f.uncertainty.epistemic > Decimal::from_f64(0.25).unwrap());
        assert!(made.p_up > 0.5, "a positive drift leans up");
    }
}

fn quantiles(target: &str, days: i64, q10: f64, q50: f64, q90: f64) -> ForecastFields {
    let mut f = issued(LatticeHorizon::of(Duration::from_days(days)))
        .forecast
        .into_fields();
    f.target = target.into();
    f.horizon = Duration::from_days(days);
    f.distribution = OutcomeDistribution::Quantiles {
        points: [(1_000, q10), (5_000, q50), (9_000, q90)]
            .into_iter()
            .map(|(bp, v)| QuantilePoint {
                quantile_bp: bp,
                value: Decimal::from_f64(v).unwrap(),
            })
            .collect(),
    };
    f
}

/// WORLD-3083: adjacent horizons that contradict each other are flagged,
/// and a consistent chain is not. Mutation: make `constrain` return nothing.
#[test]
fn adjacent_horizons_that_contradict_each_other_are_flagged() {
    let day = quantiles("t", 1, 99.0, 100.0, 101.0);
    let month_ok = quantiles("t", 30, 95.0, 100.5, 106.0);
    assert!(
        constrain(&[&day, &month_ok]).is_empty(),
        "premise: a consistent chain"
    );
    let month_narrow = quantiles("t", 30, 99.5, 100.0, 100.5);
    let flagged = constrain(&[&month_narrow, &day]);
    assert_eq!(flagged.len(), 1);
    assert_eq!(flagged[0].kind, InconsistencyKind::Narrowing);
    assert_eq!(flagged[0].shorter, "hours_days");
    let month_away = quantiles("t", 30, 110.0, 115.0, 120.0);
    let flagged = constrain(&[&day, &month_away]);
    assert!(
        flagged
            .iter()
            .any(|i| i.kind == InconsistencyKind::MedianOutside)
    );
}

/// WORLD-9022: a question goes to the best-reputed model in its context, and
/// with the cache unavailable the default ensemble answers. Mutation: ignore
/// the cache in `route`.
#[test]
fn a_question_routes_by_reputation_and_falls_back_to_the_default_ensemble() {
    let population = Population::standard().unwrap();
    let question = Question {
        target: "obj-AAA/close".into(),
        domain: "markets".into(),
        horizon: LatticeHorizon::HoursDays,
    };
    let fallback = route(&population, &question, None).unwrap();
    assert!(fallback.fallback);
    assert_eq!(fallback.weights.len(), 2);
    assert!(fallback.weights.values().all(|w| (*w - 0.5).abs() < 1e-12));
    let mut cache = Reputation::new();
    cache.insert(
        (LatticeHorizon::HoursDays, "hours_days/martingale".into()),
        0.2,
    );
    cache.insert((LatticeHorizon::HoursDays, "hours_days/drift".into()), 0.8);
    let routed = route(&population, &question, Some(&cache)).unwrap();
    assert!(!routed.fallback);
    assert_eq!(routed.best(), Some("hours_days/drift"));
    // A cache that does not cover the row is as good as none.
    let mut partial = Reputation::new();
    partial.insert((LatticeHorizon::HoursDays, "hours_days/drift".into()), 1.0);
    assert!(
        route(&population, &question, Some(&partial))
            .unwrap()
            .fallback
    );
}

/// WORLD-9024: a forecast past its expiry is not used, and the multiplier a
/// consumer would size by shrinks with age. Mutation: drop the expiry check
/// in `Forecast::read_at`, or return a constant multiplier.
#[test]
fn a_stale_forecast_is_refused_and_its_staleness_multiplier_shrinks_with_age() {
    let made = issued(LatticeHorizon::HoursDays);
    let fresh = staleness_multiplier(&made.forecast, t0()).unwrap();
    let half = staleness_multiplier(
        &made.forecast,
        t0().saturating_add(Duration::from_hours(12)),
    )
    .unwrap();
    assert!((fresh - 1.0).abs() < 1e-12);
    assert!((half - 0.5).abs() < 1e-9, "{half}");
    assert!(staleness_multiplier(&made.forecast, made.forecast.expires_at).is_err());
}

fn resolution(
    id: &str,
    horizon: LatticeHorizon,
    regime: &str,
    outcome: f64,
    drift_err: f64,
) -> Resolution {
    let due = t0().saturating_add(horizon.representative());
    Resolution {
        forecast_id: id.into(),
        horizon,
        regime: regime.into(),
        model_means: BTreeMap::from([
            (format!("{}/martingale", horizon.as_str()), 100.0),
            (format!("{}/drift", horizon.as_str()), outcome + drift_err),
        ]),
        quantiles: [98.0, 100.0, 102.0],
        p_up: 0.6,
        reference: 100.0,
        issued_at: t0(),
        observed_at: due,
        outcome,
        resolved_at: due,
    }
}

/// WORLD-9026: resolved forecasts are scored — Brier, pinball, coverage —
/// by horizon and by regime. Mutation: drop the regime scorecard update.
#[test]
fn resolved_forecasts_are_scored_by_horizon_and_by_regime() {
    let mut c = Calibrator::new();
    let h = LatticeHorizon::HoursDays;
    let up = c
        .score(&resolution("a", h, "trending/high", 101.0, 0.0))
        .unwrap();
    let down = c
        .score(&resolution("b", h, "ranging/low", 105.0, 0.0))
        .unwrap();
    assert!(
        (up.brier - 0.16).abs() < 1e-12,
        "p_up 0.6 and it went up: {}",
        up.brier
    );
    assert!(up.covered && !down.covered);
    let card = c.scorecard(h).unwrap();
    assert_eq!(card.resolved, 2);
    assert_eq!(card.coverage(), Some(0.5));
    assert_eq!(c.regime_scorecards().len(), 2);
    assert_eq!(c.history().count(), 2);
    assert!(matches!(
        c.calibration(h, t0()),
        Calibration::Measured { resolved: 2, .. }
    ));
    assert!(matches!(
        c.calibration(LatticeHorizon::WeeksMonths, t0()),
        Calibration::Uncalibrated
    ));
}

/// Point in time: an answer before the horizon elapsed, more than a horizon
/// after, or not yet knowable when scored, is refused. Mutation: drop any of
/// the three checks in `Calibrator::score`.
#[test]
fn an_answer_outside_the_horizon_or_not_yet_knowable_is_refused() {
    let mut c = Calibrator::new();
    let h = LatticeHorizon::HoursDays;
    let mut early = resolution("a", h, "r", 101.0, 0.0);
    early.observed_at = t0().saturating_add(Duration::from_hours(1));
    assert!(c.score(&early).is_err());
    let mut late = resolution("b", h, "r", 101.0, 0.0);
    late.observed_at = t0().saturating_add(Duration::from_days(3));
    late.resolved_at = late.observed_at;
    assert!(c.score(&late).is_err());
    let mut unknowable = resolution("c", h, "r", 101.0, 0.0);
    unknowable.resolved_at = t0();
    assert!(c.score(&unknowable).is_err());
    assert!(
        c.score(&resolution("d", h, "r", 101.0, 0.0)).is_ok(),
        "premise: a sound answer scores"
    );
}

/// WORLD-9027: with the calibrator unavailable, the weights equal the last
/// safe set; when it returns, they move again. Mutation: keep updating in
/// `refresh_weights` when unavailable.
#[test]
fn with_the_calibrator_unavailable_the_weights_equal_the_last_safe_set() {
    let mut c = Calibrator::new();
    let h = LatticeHorizon::HoursDays;
    for i in 0..5 {
        c.score(&resolution(&format!("a{i}"), h, "r", 101.0, 0.1))
            .unwrap();
    }
    assert_eq!(c.refresh_weights(true), WeightUpdate::Updated);
    let safe = c.weights().clone();
    assert_eq!(safe.len(), 2, "premise: both members weighed");
    let drift = safe[&(h, "hours_days/drift".to_string())];
    assert!(drift > 0.5, "the closer member weighs more: {drift}");
    // More evidence arrives that would move the weights a long way.
    for i in 0..20 {
        c.score(&resolution(&format!("b{i}"), h, "r", 110.0, 30.0))
            .unwrap();
    }
    assert_eq!(c.refresh_weights(false), WeightUpdate::Frozen);
    assert_eq!(c.weights(), &safe);
    assert!(c.frozen());
    assert_eq!(c.refresh_weights(true), WeightUpdate::Updated);
    assert_ne!(c.weights(), &safe);
}
