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
    Calibrator, CatalystExpectation, FLOW_SHIFT_PER_SD, ForecastContext, InconsistencyKind, Lane,
    LatticeHorizon, LatticeModel, ModelFamily, Population, PromotionTarget, Question, Reputation,
    ReputationKey, Resolution, StructuralClaim, TargetInput, WeightUpdate, constrain, issue, route,
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
        context: ForecastContext::default(),
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
                family: ModelFamily::Martingale,
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
                family: ModelFamily::Martingale,
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
            f.models_consulted, 3,
            "both generic members and the row's specialist are consulted"
        );
        // The drift member and the martingale member disagree by the drift;
        // a specialist given no signal states the level, or the NOW drift
        // at the regime row, so the spread is still the drift.
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
    assert_eq!(fallback.weights.len(), 3);
    assert!(
        fallback
            .weights
            .values()
            .all(|w| (*w - 1.0 / 3.0).abs() < 1e-12)
    );
    let key = |domain: &str, model: &str| ReputationKey {
        domain: domain.into(),
        horizon: LatticeHorizon::HoursDays,
        model: model.into(),
    };
    let mut cache = Reputation::new();
    cache.insert(key("markets", "hours_days/martingale"), 0.1);
    cache.insert(key("markets", "hours_days/drift"), 0.8);
    cache.insert(key("markets", "hours_days/event_reaction"), 0.1);
    let routed = route(&population, &question, Some(&cache)).unwrap();
    assert!(!routed.fallback);
    assert_eq!(routed.best(), Some("hours_days/drift"));
    // A cache that does not cover the row is as good as none.
    let mut partial = Reputation::new();
    partial.insert(key("markets", "hours_days/drift"), 1.0);
    assert!(
        route(&population, &question, Some(&partial))
            .unwrap()
            .fallback
    );
    // WORLD-9022: the reputation is earned per domain. The same row asked in
    // the macro domain, where the martingale has done best, routes to the
    // martingale; and a domain the cache has not scored falls back, however
    // well the row is known in markets. Mutation: look the cache up under a
    // fixed domain in `route`.
    cache.insert(key("macro", "hours_days/martingale"), 0.7);
    cache.insert(key("macro", "hours_days/drift"), 0.2);
    cache.insert(key("macro", "hours_days/event_reaction"), 0.1);
    let macro_question = Question {
        domain: "macro".into(),
        ..question.clone()
    };
    let macro_routed = route(&population, &macro_question, Some(&cache)).unwrap();
    assert!(!macro_routed.fallback);
    assert_eq!(macro_routed.best(), Some("hours_days/martingale"));
    assert_eq!(
        route(&population, &question, Some(&cache)).unwrap().best(),
        Some("hours_days/drift"),
        "premise: the markets answer is unchanged by the macro entries"
    );
    let unknown = Question {
        domain: "weather".into(),
        ..question
    };
    assert!(route(&population, &unknown, Some(&cache)).unwrap().fallback);
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
        domain: "markets".into(),
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
    let drift = safe[&ReputationKey {
        domain: "markets".into(),
        horizon: h,
        model: "hours_days/drift".into(),
    }];
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

/// Issue the row with the given context, routed by the default ensemble.
fn issued_with(
    horizon: LatticeHorizon,
    context: ForecastContext,
) -> qip_world_model::lattice::Issued {
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
    let input = TargetInput { context, ..input() };
    issue(
        Lane::Central,
        &input,
        horizon,
        &population,
        &routed,
        Calibration::Uncalibrated,
        t0(),
    )
    .unwrap()
}

fn width(made: &qip_world_model::lattice::Issued) -> f64 {
    made.quantiles[2] - made.quantiles[0]
}

/// WORLD-3085: the seconds-minutes row holds an order-flow member whose
/// distribution moves with the imbalance — a buy imbalance lifts its mean
/// above the level by the stated share of the horizon's standard deviation
/// and widens the row; a sell imbalance lowers it; and an imbalance outside
/// `[-1, 1]` is refused, not clamped. Mutation: ignore the imbalance in
/// `project` for `ModelFamily::OrderFlow`.
#[test]
fn the_seconds_row_holds_a_flow_member_whose_distribution_moves_with_the_imbalance() {
    let h = LatticeHorizon::SecondsMinutes;
    let flat = issued_with(h, ForecastContext::default());
    assert!(
        flat.model_means.contains_key("seconds_minutes/order_flow"),
        "premise: the row holds a flow member: {:?}",
        flat.model_means.keys().collect::<Vec<_>>()
    );
    assert!(
        (flat.model_means["seconds_minutes/order_flow"] - 100.0).abs() < 1e-12,
        "premise: with no flow the member states the level"
    );
    let with = |imbalance: f64| {
        issued_with(
            h,
            ForecastContext {
                flow_imbalance: Some(imbalance),
                ..ForecastContext::default()
            },
        )
    };
    let buy = with(0.8);
    let sell = with(-0.8);
    let days = h.representative().as_days_f64();
    let sd = (1.0 * days).sqrt();
    let flow_mean = buy.model_means["seconds_minutes/order_flow"];
    assert!(
        (flow_mean - (100.0 + 0.8 * FLOW_SHIFT_PER_SD * sd)).abs() < 1e-9,
        "buy flow lifts the member by its share of the horizon sd: {flow_mean}"
    );
    assert!(sell.model_means["seconds_minutes/order_flow"] < 100.0);
    assert!(
        buy.quantiles[1] > flat.quantiles[1],
        "the row's median leans with the flow"
    );
    assert!(sell.quantiles[1] < flat.quantiles[1]);
    assert!(width(&buy) > width(&flat), "an imbalance widens the row");
    assert!(
        buy.forecast.fields().uncertainty.aleatoric > flat.forecast.fields().uncertainty.aleatoric
    );
    let population = Population::standard().unwrap();
    let routed = route(
        &population,
        &Question {
            target: "obj-AAA/close".into(),
            domain: "markets".into(),
            horizon: h,
        },
        None,
    )
    .unwrap();
    let bad = TargetInput {
        context: ForecastContext {
            flow_imbalance: Some(1.7),
            ..ForecastContext::default()
        },
        ..input()
    };
    assert!(
        issue(
            Lane::Central,
            &bad,
            h,
            &population,
            &routed,
            Calibration::Uncalibrated,
            t0()
        )
        .is_err(),
        "an imbalance outside [-1, 1] is refused"
    );
}

/// WORLD-3086: the hours-days row holds an event-reaction member, and a
/// pending catalyst widens the row by the expected reaction without moving
/// its median, because the direction is the thing nobody knows yet.
/// Mutation: ignore the catalyst in `project` for `ModelFamily::EventReaction`.
#[test]
fn a_pending_catalyst_widens_the_hours_row_without_moving_its_median() {
    let h = LatticeHorizon::HoursDays;
    let quiet = issued_with(h, ForecastContext::default());
    assert!(
        quiet.model_means.contains_key("hours_days/event_reaction"),
        "premise: the row holds an event-reaction member"
    );
    let pending = issued_with(
        h,
        ForecastContext {
            pending_catalyst: Some(CatalystExpectation {
                source: "macro_surprise/US.CPI.YOY".into(),
                expected_abs_move: 0.05,
            }),
            ..ForecastContext::default()
        },
    );
    assert!(
        (pending.quantiles[1] - quiet.quantiles[1]).abs() < 1e-9,
        "a catalyst of unknown direction does not move the median"
    );
    assert!(
        width(&pending) > width(&quiet),
        "a pending catalyst widens the row: {} vs {}",
        width(&pending),
        width(&quiet)
    );
    // The widening is the member's share of (0.05 * 100)^2 = 25 at a third
    // of the weight: the aleatoric variance grows by 25 / 3.
    let grew = (pending.forecast.fields().uncertainty.aleatoric
        - quiet.forecast.fields().uncertainty.aleatoric)
        .to_f64();
    assert!((grew - 25.0 / 3.0).abs() < 1e-6, "{grew}");
}

/// WORLD-3087: the weeks-months row holds a regime-path member that projects
/// the drift the current regime label has shown, so a regime change moves
/// the row's median while every other input is unchanged. Mutation: ignore
/// the regime in `project` for `ModelFamily::RegimePath`.
#[test]
fn a_regime_change_moves_the_weeks_rows_median_through_the_regime_path_member() {
    let h = LatticeHorizon::WeeksMonths;
    let drifts = BTreeMap::from([("bull".to_string(), 0.4), ("bear".to_string(), -0.4)]);
    let under = |regime: &str| {
        issued_with(
            h,
            ForecastContext {
                regime: Some(regime.into()),
                regime_drifts: drifts.clone(),
                ..ForecastContext::default()
            },
        )
    };
    let bull = under("bull");
    let bear = under("bear");
    assert!(
        bull.model_means.contains_key("weeks_months/regime_path"),
        "premise: the row holds a regime-path member"
    );
    let days = h.representative().as_days_f64();
    assert!((bull.model_means["weeks_months/regime_path"] - (100.0 + 0.4 * days)).abs() < 1e-9);
    assert!((bear.model_means["weeks_months/regime_path"] - (100.0 - 0.4 * days)).abs() < 1e-9);
    assert!(
        bull.quantiles[1] > bear.quantiles[1],
        "the regime label moves the median: {} vs {}",
        bull.quantiles[1],
        bear.quantiles[1]
    );
    // A label the drifts do not cover falls back to the NOW drift, like the
    // drift member, rather than inventing a regime.
    let unknown = under("sideways");
    assert!(
        (unknown.model_means["weeks_months/regime_path"]
            - unknown.model_means["weeks_months/drift"])
            .abs()
            < 1e-9
    );
}

/// WORLD-3088: the quarters-years row holds a structural member fed by the
/// live causal claims bearing on the target; each adds its transmission
/// squared of the row's variance, so a claim retired (absent from the live
/// set) narrows the row. Mutation: ignore the claims in `project` for
/// `ModelFamily::Structural`.
#[test]
fn a_retired_structural_claim_narrows_the_quarters_row() {
    let h = LatticeHorizon::QuartersYears;
    let claim = |cause: &str, transmission: f64| StructuralClaim {
        cause: cause.into(),
        mechanism: "supply_chain".into(),
        transmission,
        lag: Duration::from_days(90),
    };
    let with = |claims: Vec<StructuralClaim>| {
        issued_with(
            h,
            ForecastContext {
                structural_claims: claims,
                ..ForecastContext::default()
            },
        )
    };
    let none = with(vec![]);
    assert!(
        none.model_means.contains_key("quarters_years/structural"),
        "premise: the row holds a structural member"
    );
    let two = with(vec![claim("kestrel", 0.6), claim("rates", -0.3)]);
    let one = with(vec![claim("kestrel", 0.6)]);
    assert!(
        width(&two) > width(&one),
        "{} vs {}",
        width(&two),
        width(&one)
    );
    assert!(
        width(&one) > width(&none),
        "{} vs {}",
        width(&one),
        width(&none)
    );
    // The widening is exact: (0.36 + 0.09) of the horizon variance (365) at
    // a third of the weight.
    let days = h.representative().as_days_f64();
    let grew = (two.forecast.fields().uncertainty.aleatoric
        - none.forecast.fields().uncertainty.aleatoric)
        .to_f64();
    assert!((grew - 0.45 * days / 3.0).abs() < 1e-6, "{grew}");
    assert!(
        (two.quantiles[1] - none.quantiles[1]).abs() < 1e-9,
        "claims of unknown direction over the horizon do not move the median"
    );
}
