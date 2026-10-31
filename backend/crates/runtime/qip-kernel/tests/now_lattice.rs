//! The NOW Brain and the Temporal Forecast Lattice as the cycle runs them
//! (blueprint v12.0 §9.6, §9.7; WORLD-7196, WORLD-7197). Every test drives
//! `Platform::run_cycle` — the call `qip-deepbrain`, `qip-fastbrain` and
//! `qip-api` make from their composition roots — and reads what the cycle
//! left in the hash-chained log, so what is proven is the composed
//! behaviour and not a library in isolation.
//!
//! Each test names, in its doc comment, the mutation that should make it fail.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_events::{EventBody, Topic};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::lattice_stage::LatticeReview;
use qip_kernel::now_stage::{NowReview, Specialist};
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;
use qip_world_model::lattice::{LatticeHorizon, WeightUpdate};
use qip_world_model::now::ScopeKind;
use std::collections::BTreeSet;

const SYMBOLS: [&str; 2] = ["AAA", "BBB"];

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn day(n: i64) -> Timestamp {
    start().saturating_add(Duration::from_days(n))
}

fn object(symbol: &str) -> qip_core::ObjectId {
    qip_core::ObjectId::from_string(format!("obj-{symbol}"))
}

fn platform() -> Platform {
    let mut universe = Universe::new();
    for symbol in SYMBOLS {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
                )
                .venue("XNYS")
                .geography("US")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .unwrap(),
            )
            .unwrap();
    }
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::new("now-lattice-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
    .unwrap()
}

/// A daily bar for `symbol` closing at `closes_at`.
fn bar(symbol: &str, closes_at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap();
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: closes_at.saturating_sub(Duration::from_days(1)),
        open: price(close),
        high: price(close * 1.002),
        low: price(close * 0.998),
        close: price(close),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: None,
        quality: DataQuality::default(),
    }))
}

/// Thirty calm days for each symbol, the last closing at `start()`.
fn calm_history() -> Vec<SensedRecord> {
    let mut out = Vec::new();
    for symbol in SYMBOLS {
        for i in 0..30_i64 {
            let close = if i % 2 == 0 { 100.2 } else { 99.8 };
            out.push(bar(symbol, day(i - 29), close));
        }
    }
    out
}

fn reviews<T: EventBody>(platform: &Platform, prefix: &str) -> Vec<T> {
    platform
        .event_log()
        .by_topic(Topic::WorldModelUpdated)
        .into_iter()
        .filter(|e| {
            e.idempotency_key
                .as_deref()
                .is_some_and(|k| k.starts_with(prefix))
        })
        // `Platform::journal_record` appends a sealed `StreamEnvelope`'s
        // frame, so the log record's payload is the envelope wire and the
        // review is inside it. Decoding the frame as the review directly
        // read the wire's fields as the review's and failed on `cycle`,
        // which is how all seven tests here failed at their first read.
        .map(|e| {
            StreamEnvelope::from_frame(e)
                .and_then(|envelope| envelope.decode::<T>())
                .unwrap()
                .body
        })
        .collect()
}

fn last<T: EventBody>(platform: &Platform, prefix: &str) -> T {
    reviews::<T>(platform, prefix)
        .pop()
        .unwrap_or_else(|| panic!("no `{prefix}` review in the log"))
}

/// WORLD-7196, WORLD-3077, DATA-3080: every cycle steps the NOW Brain at the
/// scopes the platform observes and journals compact deltas, each citing a
/// manifest. Mutation: remove the `understand_now` call from
/// `stage_understand`, and nothing is promoted and nothing is journaled.
#[test]
fn every_cycle_steps_the_now_brain_and_journals_compact_deltas_citing_manifests() {
    let mut platform = platform();
    platform.observe(calm_history());
    assert!(
        platform.now_brain().brain().store().is_empty(),
        "premise: an empty NOW state"
    );
    let report = platform.run_cycle(start());
    let understood = report
        .stage(Stage::Understand)
        .expect("UNDERSTAND always runs");
    assert!(
        understood.detail.contains("NOW state"),
        "{}",
        understood.detail
    );
    let review: NowReview = last(&platform, "now:");
    assert!(!review.deltas.is_empty());
    assert!(review.deltas.iter().all(|d| !d.manifests.is_empty()));
    // Five since the causal estimator joined (R12-WORLD-3078).
    assert_eq!(review.ensemble.len(), 5, "five estimators registered");
    let scopes: BTreeSet<&str> = review
        .variables_by_scope
        .keys()
        .map(String::as_str)
        .collect();
    for kind in [
        ScopeKind::Global,
        ScopeKind::Region,
        ScopeKind::Sector,
        ScopeKind::Instrument,
        ScopeKind::Portfolio,
        ScopeKind::Infrastructure,
    ] {
        assert!(
            scopes.contains(kind.as_str()),
            "{} missing from {scopes:?}",
            kind.as_str()
        );
    }
}

/// DATA-3080: the persisted NOW record holds deltas only, and the schema,
/// not just the writer, says so. A record the platform journaled decodes;
/// the same record with a raw payload field beside its deltas is refused.
/// Mutation: drop `#[serde(deny_unknown_fields)]` from `NowReview` and the
/// smuggled record decodes.
#[test]
fn a_journaled_now_record_with_a_raw_payload_field_is_refused_by_the_schema() {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    let review: NowReview = last(&platform, "now:");
    assert!(!review.deltas.is_empty(), "premise: a pass with deltas");

    let journaled = serde_json::to_value(&review).unwrap();
    let decoded: NowReview = serde_json::from_value(journaled.clone()).unwrap();
    assert_eq!(decoded, review, "premise: a journaled record round-trips");

    let mut smuggled = journaled;
    smuggled["payload"] = serde_json::json!({"close": [100.2, 99.8, 100.2]});
    let refused = serde_json::from_value::<NowReview>(smuggled);
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains("unknown field `payload`")),
        "{refused:?}"
    );
}

/// AMBIENT-3079 and the forecast specialist's wake: a break in one
/// instrument's price is a surprise naming all four specialists, and the
/// lattice re-forecasts that instrument at a horizon that was not yet due,
/// while the quiet instrument is left alone. Mutation: ignore
/// `surprised` in `LatticeStage::run`, and the weeks-months forecast for the
/// broken instrument is not reissued.
#[test]
fn a_price_break_is_a_surprise_and_the_lattice_reforecasts_that_instrument_out_of_turn() {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    let first: LatticeReview = last(&platform, "lattice:");
    let weeks = |r: &LatticeReview, target: &str| {
        r.issued.iter().any(|f| {
            f.target == target && LatticeHorizon::of(f.horizon) == LatticeHorizon::WeeksMonths
        })
    };
    assert!(
        weeks(&first, "obj-AAA/close") && weeks(&first, "obj-BBB/close"),
        "premise"
    );

    platform.observe(vec![bar("AAA", day(1), 130.0)]);
    platform.run_cycle(day(1));
    let now: NowReview = last(&platform, "now:");
    let surprise = now
        .surprises
        .iter()
        .find(|s| s.key.scope.kind == ScopeKind::Instrument && s.key.scope.id == "obj-AAA")
        .expect("a 30% break is a surprise");
    assert_eq!(surprise.wakes, Specialist::ALL.to_vec());
    let second: LatticeReview = last(&platform, "lattice:");
    assert!(
        weeks(&second, "obj-AAA/close"),
        "the surprised instrument is re-forecast"
    );
    assert!(
        !weeks(&second, "obj-BBB/close"),
        "the quiet one waits for its refresh"
    );
    assert!(second.woken.iter().any(|id| id.contains("obj-AAA")));
}

/// WORLD-7197 and WORLD-3084: the cycle issues a calibrated forecast at every
/// central row and never at the microsecond row, whose outputs belong to the
/// reflex alone. Mutation: issue from `Lane::LocalReflex` in the stage.
#[test]
fn the_cycle_issues_at_every_central_horizon_and_never_the_microsecond_row() {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    let review: LatticeReview = last(&platform, "lattice:");
    let rows: BTreeSet<LatticeHorizon> = review
        .issued
        .iter()
        .filter(|f| f.target == "obj-AAA/close")
        .map(|f| LatticeHorizon::of(f.horizon))
        .collect();
    assert_eq!(
        rows,
        BTreeSet::from([
            LatticeHorizon::SecondsMinutes,
            LatticeHorizon::HoursDays,
            LatticeHorizon::WeeksMonths,
            LatticeHorizon::QuartersYears,
        ])
    );
    assert!(review.issued.iter().all(|f| f.expires_at > f.issued_at));
}

/// WORLD-9026: a forecast is scored against the close that answers it, per
/// horizon, and one nothing has answered yet is not. Mutation: drop the
/// resolution pass from `LatticeStage::run`.
#[test]
fn a_day_ahead_forecast_is_scored_against_the_next_close_and_a_five_minute_one_is_not() {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    assert!(
        platform
            .lattice()
            .calibrator()
            .scorecard(LatticeHorizon::HoursDays)
            .is_none(),
        "premise: nothing scored before reality answers"
    );
    platform.observe(SYMBOLS.iter().map(|s| bar(s, day(1), 100.1)).collect());
    platform.run_cycle(day(1));
    let card = platform
        .lattice()
        .calibrator()
        .scorecard(LatticeHorizon::HoursDays)
        .copied()
        .expect("the day-ahead forecasts were scored");
    assert_eq!(card.resolved, 2);
    assert!(
        platform
            .lattice()
            .calibrator()
            .scorecard(LatticeHorizon::SecondsMinutes)
            .is_none()
    );
    let review: LatticeReview = last(&platform, "lattice:");
    assert!(
        review
            .scored
            .iter()
            .all(|s| s.horizon == LatticeHorizon::HoursDays)
    );
    assert!(review.scorecards.contains_key("hours_days"));
}

/// WORLD-9025: with the strategic service down, strategic questions wait in
/// a backlog and every stage of the cycle still runs; back up, they are
/// answered. Mutation: return early from `LatticeStage::run` (or halt the
/// cycle) when the strategic service is down.
#[test]
fn with_the_strategic_service_down_its_questions_wait_and_every_stage_still_runs() {
    let mut baseline = platform();
    baseline.observe(calm_history());
    let healthy: Vec<(Stage, bool)> = baseline
        .run_cycle(start())
        .stages
        .iter()
        .map(|s| (s.stage, s.ran))
        .collect();

    let mut platform = platform();
    platform.observe(calm_history());
    platform.lattice_mut().set_strategic_available(false);
    let report = platform.run_cycle(start());
    let degraded: Vec<(Stage, bool)> = report.stages.iter().map(|s| (s.stage, s.ran)).collect();
    assert_eq!(degraded, healthy, "a strategic outage changes no stage");
    let review: LatticeReview = last(&platform, "lattice:");
    assert_eq!(
        review.backlog, 4,
        "two instruments times two strategic rows"
    );
    assert!(
        review
            .issued
            .iter()
            .all(|f| !LatticeHorizon::of(f.horizon).is_strategic())
    );

    platform.lattice_mut().set_strategic_available(true);
    platform.run_cycle(start().saturating_add(Duration::from_mins(1)));
    let review: LatticeReview = last(&platform, "lattice:");
    assert_eq!(review.backlog, 0);
    assert!(
        review
            .issued
            .iter()
            .any(|f| LatticeHorizon::of(f.horizon).is_strategic())
    );
}

/// WORLD-9027 in the cycle: a pass whose scores did not reach durable
/// storage freezes the weights on the next. Mutation: ignore
/// `calibrator_available` in `LatticeStage::run`.
#[test]
fn a_calibrator_whose_last_output_was_not_kept_freezes_the_weights_next_pass() {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    let healthy: LatticeReview = last(&platform, "lattice:");
    assert_eq!(healthy.weights, WeightUpdate::Updated, "premise");
    platform.lattice_mut().set_calibrator_available(false);
    platform.run_cycle(start().saturating_add(Duration::from_mins(1)));
    let frozen: LatticeReview = last(&platform, "lattice:");
    assert_eq!(frozen.weights, WeightUpdate::Frozen);
}

/// WORLD-9019 in the cycle: an estimator outage shrinks the ensemble the
/// review reports and the NOW state still updates. Mutation: make
/// `NowStage::run` refuse the step when a worker is missing.
#[test]
fn an_estimator_outage_shrinks_the_ensemble_and_the_now_state_still_updates() {
    let mut platform = platform();
    platform.observe(calm_history());
    assert!(platform.now_brain_mut().remove_worker("particle-bootstrap"));
    platform.run_cycle(start());
    let review: NowReview = last(&platform, "now:");
    // Five registered (the causal estimator among them, R12-WORLD-3078), one
    // removed.
    assert_eq!(review.ensemble.len(), 4);
    assert!(!review.deltas.is_empty());
}

/// R12-CONTRACT-6109: the lattice pass folds every forecast it issues into
/// the §24.9 `ForecastState` a control loop reads, and journals the two side
/// by side. Nothing in the workspace built the contract outside its own
/// tests, so a drift between `Forecast` and `ForecastState` (a horizon or an
/// expiry dropped in the projection) would have been invisible. A journaled
/// state with a field removed must not decode, because the contract refuses a
/// missing field. Mutations: skip the projection in `LatticeStage::run`
/// (`states` is empty, so the length premise fails); insert each state at
/// the front (the per-forecast comparison fails on the target).
#[test]
fn every_issued_forecast_is_journaled_with_its_control_plane_state_and_a_dropped_field_is_refused()
{
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    let review: LatticeReview = last(&platform, "lattice:");
    assert!(
        !review.issued.is_empty(),
        "premise: the pass issued forecasts"
    );
    assert_eq!(
        review.states.len(),
        review.issued.len(),
        "a forecast was issued without its control-plane state"
    );
    let horizons: BTreeSet<_> = review.issued.iter().map(|f| f.horizon).collect();
    assert!(
        horizons.len() > 1,
        "premise: more than one horizon, so a mismatched pair shows"
    );
    for (forecast, state) in review.issued.iter().zip(&review.states) {
        assert_eq!(state.target, forecast.target);
        assert_eq!(state.horizon, forecast.horizon);
        assert_eq!(state.expires_at, forecast.expires_at);
        assert_eq!(
            qip_contracts::forecast::Stated::Value(state.confidence_bp),
            forecast.regime_confidence_bp
        );
    }

    let journaled = serde_json::to_value(&review).unwrap();
    let decoded: LatticeReview = serde_json::from_value(journaled.clone()).unwrap();
    assert_eq!(decoded, review, "premise: a journaled record round-trips");

    let mut broken = journaled;
    broken["states"][0]
        .as_object_mut()
        .expect("premise: a state is an object")
        .remove("confidence_bp");
    let refused = serde_json::from_value::<LatticeReview>(broken);
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains("missing field `confidence_bp`")),
        "{refused:?}"
    );
}

/// R12-WORLD-3083: a forecast inconsistent with its adjacent horizon is
/// flagged by the cycle, on the record and on the counter, and never repaired.
/// The first cycle over calm history issues hours-days medians that sit outside
/// the weeks-months 80% interval for both instruments, so the premise is a real
/// finding rather than a manufactured one. The flagged list must be exactly
/// what `constrain` finds over the forecasts the same record carries, and the
/// forecasts must be unchanged by being flagged: a silently widened interval is
/// a forecast nobody issued. Mutations: skip the `constrain` call in
/// `LatticeStage::run` (the premise fails); drop the
/// counter increment in `Platform` (the counter assertion fails).
#[test]
fn a_forecast_outside_its_longer_neighbours_interval_is_flagged_in_the_review_and_on_the_counter() {
    use qip_observability::metrics::labels;
    use qip_world_model::lattice::{InconsistencyKind, constrain};

    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    let review: LatticeReview = last(&platform, "lattice:");

    let flagged: Vec<_> = review
        .inconsistencies
        .iter()
        .filter(|i| i.kind == InconsistencyKind::MedianOutside)
        .collect();
    assert!(
        !flagged.is_empty(),
        "premise: calm history leaves a short-horizon median outside the longer interval"
    );
    for finding in &flagged {
        assert_eq!(
            (finding.shorter.as_str(), finding.longer.as_str()),
            ("hours_days", "weeks_months"),
            "{finding:?}"
        );
    }
    let targets: BTreeSet<&str> = flagged.iter().map(|f| f.target.as_str()).collect();
    assert_eq!(targets, BTreeSet::from(["obj-AAA/close", "obj-BBB/close"]));

    // Flagged, not repaired: the journaled forecasts reproduce the flags.
    let mut recomputed = Vec::new();
    for target in &targets {
        let chain: Vec<_> = review
            .issued
            .iter()
            .filter(|f| f.target == *target)
            .map(|f| f.fields())
            .collect();
        recomputed.extend(constrain(&chain));
    }
    assert_eq!(recomputed, review.inconsistencies);

    let count = platform.telemetry().metrics.snapshot().counter(
        qip_kernel::lattice_stage::LATTICE_INCONSISTENCIES_TOTAL,
        &labels([("kind", "median_outside")]),
    );
    assert_eq!(count, flagged.len() as u64);
}

/// R12-WORLD-9026: forecasts resolved under two different regime labels are
/// scored under each, and the per-regime score history is durable in the log:
/// the `ScoreRecord`s the cycle journals whole rebuild exactly the per-horizon, per-regime
/// Brier the calibrator holds. Driven at the stage, which is what each cycle
/// calls, because the platform's own regime label reads a surprise series that
/// bars never feed, so a cycle over bars alone labels every instrument
/// `quiet/normal` and could not show two regimes. The premise (two labels
/// resolved) is asserted, not assumed. Mutations in
/// `Calibrator::score` (qip-world-model): blank the regime on the
/// `ScoreRecord` (the distinct-regimes premise fails); file every resolution
/// under the `other` label (no card exists for a scored label).
#[test]
fn forecasts_resolved_in_two_regimes_are_scored_under_each_and_the_journal_rebuilds_the_regime_cards()
 {
    use qip_kernel::lattice_stage::{LatticeInputs, LatticeStage, LatticeTarget};

    let target = |subject: &str, regime: &str, answer: Option<f64>| LatticeTarget {
        subject: subject.to_string(),
        closes: (0..30_i64)
            .map(|i| (day(i - 29), if i % 2 == 0 { 100.2 } else { 99.8 }))
            .chain(answer.map(|close| (day(1), close)))
            .collect(),
        regime: regime.to_string(),
        held_quantity: 0.0,
        now_return: None,
    };
    let mut stage = LatticeStage::new().expect("the stage builds");
    stage.run(
        LatticeInputs {
            cycle: 1,
            targets: vec![
                target("obj-AAA", "trending/elevated", None),
                target("obj-BBB", "quiet/normal", None),
            ],
            surprised: BTreeSet::new(),
        },
        start(),
    );
    // A day later each close answers the day-ahead forecast; the climbing
    // instrument closes far from its forecast and the quiet one near it.
    let review = stage.run(
        LatticeInputs {
            cycle: 2,
            targets: vec![
                target("obj-AAA", "trending/elevated", Some(112.0)),
                target("obj-BBB", "quiet/normal", Some(100.05)),
            ],
            surprised: BTreeSet::new(),
        },
        day(1),
    );

    let day_ahead: Vec<_> = review
        .scored
        .iter()
        .filter(|s| s.horizon == LatticeHorizon::HoursDays)
        .collect();
    assert_eq!(
        day_ahead.len(),
        2,
        "premise: both day-ahead forecasts resolved"
    );
    let regimes: BTreeSet<&str> = day_ahead.iter().map(|s| s.regime.as_str()).collect();
    assert_eq!(
        regimes,
        BTreeSet::from(["quiet/normal", "trending/elevated"]),
        "premise: two distinct regimes"
    );

    let cards = stage.calibrator().regime_scorecards();
    for regime in &regimes {
        let rebuilt: Vec<f64> = day_ahead
            .iter()
            .filter(|s| s.regime == *regime)
            .map(|s| s.brier)
            .collect();
        let card = cards
            .get(&(LatticeHorizon::HoursDays, (*regime).to_string()))
            .unwrap_or_else(|| panic!("no card for {regime}: {:?}", cards.keys()));
        assert_eq!(card.resolved, rebuilt.len() as u64, "{regime}");
        let mean = rebuilt.iter().sum::<f64>() / rebuilt.len() as f64;
        assert!(
            (card.brier_sum / card.resolved as f64 - mean).abs() < 1e-12,
            "{regime}: the journal rebuilds {mean}, the card holds {}",
            card.brier_sum / card.resolved as f64
        );
        let pinball: f64 = day_ahead
            .iter()
            .filter(|s| s.regime == *regime)
            .map(|s| s.pinball)
            .sum();
        assert!(
            (card.pinball_sum - pinball).abs() < 1e-12,
            "{regime}: the journal rebuilds a pinball sum of {pinball}, the card holds {}",
            card.pinball_sum
        );
    }
    assert!(
        (day_ahead[0].pinball - day_ahead[1].pinball).abs() > 1e-6,
        "premise: the regimes differ in score, so a merged card would be wrong"
    );
}

/// R12-WORLD-10170: the calibration objective is judged from the card, and only
/// once the card has enough resolutions to judge. Mutations: compare with `<`
/// (the breached case reads met); ignore the minimum (the short card is judged).
#[test]
fn a_horizon_is_judged_against_the_brier_bound_only_once_it_has_enough_resolutions() {
    use qip_kernel::lattice_stage::{
        CALIBRATION_BRIER_BOUND, CALIBRATION_MIN_RESOLVED, calibration_slo_breached,
    };
    use qip_world_model::lattice::Scorecard;

    let card = |resolved: u64, brier: f64| Scorecard {
        resolved,
        brier_sum: brier * resolved as f64,
        pinball_sum: 0.0,
        covered: 0,
    };
    let bound = CALIBRATION_BRIER_BOUND;
    let enough = CALIBRATION_MIN_RESOLVED;
    assert_eq!(
        calibration_slo_breached(&card(enough, bound + 0.1), bound, enough),
        Some(true)
    );
    assert_eq!(
        calibration_slo_breached(&card(enough, bound - 0.1), bound, enough),
        Some(false)
    );
    assert_eq!(
        calibration_slo_breached(&card(enough - 1, bound + 0.4), bound, enough),
        None,
        "a card one resolution short was judged"
    );
    assert!(
        bound > 0.25,
        "premise: the bound must sit above the 0.25 a forecast of one half always scores"
    );
}

/// R12-WORLD-10170's exit test: one run records the NOW state, the forecast
/// contracts and the calibration objective's gauge. The objective is silent
/// while a horizon has too few resolutions and then written every cycle with the
/// verdict; a calm tape forecast at one half scores 0.25, inside the bound, so
/// the gauge reads 0. Mutation: never write the gauge (the final assertion
/// fails).
#[test]
fn a_run_records_the_now_state_the_forecasts_and_the_calibration_objective_gauge() {
    use qip_kernel::lattice_stage::{CALIBRATION_MIN_RESOLVED, LATTICE_CALIBRATION_SLO_BREACHED};
    use qip_observability::metrics::labels;

    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    let gauge = |platform: &Platform| {
        platform.telemetry().metrics.snapshot().gauge(
            LATTICE_CALIBRATION_SLO_BREACHED,
            &labels([("horizon", "hours_days")]),
        )
    };
    assert_eq!(
        gauge(&platform),
        None,
        "premise: nothing is resolved after one cycle"
    );

    let mut days = 0_i64;
    while platform
        .lattice()
        .calibrator()
        .scorecard(LatticeHorizon::HoursDays)
        .is_none_or(|card| card.resolved < CALIBRATION_MIN_RESOLVED)
    {
        days += 1;
        assert!(days < 60, "premise: the card never reached the minimum");
        platform.observe(SYMBOLS.iter().map(|s| bar(s, day(days), 100.1)).collect());
        platform.run_cycle(day(days));
    }

    let now: NowReview = last(&platform, "now:");
    assert!(!now.deltas.is_empty(), "the NOW state was not recorded");
    let lattice: LatticeReview = last(&platform, "lattice:");
    assert!(
        !lattice.issued.is_empty(),
        "no forecast contract was issued"
    );
    assert_eq!(
        gauge(&platform),
        Some(0.0),
        "the objective is met and the gauge says so"
    );
}
