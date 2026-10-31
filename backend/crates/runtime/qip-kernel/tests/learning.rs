//! What the LEARN stage now does on its own that nothing used to do for it.
//!
//! Two functions on the platform existed with no production caller:
//! `Platform::learn_from`, which computes the belief calibration the
//! blueprint calls its single most important metric, and
//! `Platform::evaluate_alternatives`, which prices the paths the platform
//! declined. Each test here drives a cycle through the event that should
//! reach one of them and asserts the series moved — so deleting the call
//! site fails a test rather than leaving a dashboard that is merely blank.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::ids::OrderId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind};
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{Snapshot, labels, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};

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

// --- fixtures ---------------------------------------------------------------

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

// --- failure clusters become research prompts (MODEL-046) -------------------

#[test]
fn a_cluster_of_failed_theses_raises_one_research_prompt_that_closes_onto_a_named_task()
-> Result<()> {
    // The failure this guards: the same hypothesis class was wrong again and
    // again and every miss was charged to a track record and forgotten, so
    // the research side was never asked why. `learn_from` is the call the
    // LEARN stage makes; the prompt must come out of it, not out of a helper
    // the stage never calls.
    use qip_learning_engine::{Outcome, ThesisClaim};
    let mut platform = platform()?;
    let claim = |id: &str, class: &str| ThesisClaim {
        hypothesis_id: id.to_string(),
        class: class.to_string(),
        subject: "AAA".to_string(),
        formed_at: start(),
        resolves_at: Timestamp::from_secs(1_760_000_000 + 60),
        direction: 1.0,
        expected_move_bps: 100.0,
        confidence: 0.8,
        falsifiers: vec![],
        contributors: vec![],
    };
    let outcome = |id: &str| Outcome {
        hypothesis_id: id.to_string(),
        observed_at: Timestamp::from_secs(1_760_000_000 + 61),
        realised_move_bps: -100.0,
        realised_pnl: -1.0,
        falsifiers_triggered: vec![],
        mechanism_confirmed: None,
    };
    let now = Timestamp::from_secs(1_760_000_000 + 120);

    // Premise: nothing has been prompted yet.
    assert_eq!(platform.research().prompts().count(), 0);

    // Two failures of one class, and one of another, are not a cluster.
    let claims = [
        claim("h1", "momentum"),
        claim("h2", "momentum"),
        claim("h9", "carry"),
    ];
    let outcomes = [outcome("h1"), outcome("h2"), outcome("h9")];
    platform.learn_from(&claims, &outcomes, now)?;
    assert_eq!(platform.research().prompts().count(), 0);

    // The third failure of the class completes the cluster.
    platform.learn_from(&[claim("h3", "momentum")], &[outcome("h3")], now)?;
    let prompts: Vec<_> = platform.research().prompts().cloned().collect();
    assert_eq!(prompts.len(), 1, "exactly one prompt for one cluster");
    assert_eq!(prompts[0].class, "momentum");
    assert_eq!(prompts[0].episodes, vec!["h1", "h2", "h3"]);
    assert!(prompts[0].is_open());

    platform.close_research_prompt(&prompts[0].id, "task-42", now)?;
    let closed = platform.research().prompt(&prompts[0].id).cloned().unwrap();
    assert_eq!(closed.answered_by.as_deref(), Some("task-42"));
    Ok(())
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

/// A price series with a jump partway through, so the detectors have something
/// real to find — the same shape the kernel's founding test feeds.
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

fn recorded(platform: &Platform) -> Snapshot {
    platform.telemetry().metrics.snapshot()
}

// --- belief calibration -----------------------------------------------------

#[test]
fn a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series() -> Result<()> {
    // The failure this guards: `learn_from` computed the calibration and
    // nothing called it, so the platform wrote a confidence beside every
    // hypothesis and never asked whether its seventy percents happened
    // seventy percent of the time. The LEARN stage now settles each claim
    // whose horizon has passed against the platform's own series and grades
    // it; this is the cycle that proves the seam is wired.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());

    // Premise: the first cycle made a claim, and nothing has been graded.
    // Without this the assertions below could pass against a registry that
    // was never empty.
    assert!(
        !platform.predictions().is_empty(),
        "no claim was written, so there is nothing to resolve:\n{}",
        first.summarise()
    );
    let prediction = platform.predictions()[0].clone();
    assert!(prediction.is_open(), "a fresh claim must be open");
    let claim = prediction
        .claim
        .clone()
        .expect("a claim records the confidence it was made at, or it cannot be graded");
    assert!(
        (0.0..=1.0).contains(&claim.confidence),
        "confidence {} is not a probability",
        claim.confidence
    );
    let snapshot = recorded(&platform);
    assert_eq!(
        snapshot.counter_total(names::THESES_EVALUATED),
        0,
        "a claim was graded before its horizon passed"
    );
    assert!(
        snapshot
            .gauge(names::BELIEF_BRIER_SCORE, &labels([]))
            .is_none(),
        "a Brier score exists before anything resolved"
    );
    assert!(
        platform.calibration().is_none(),
        "the platform reports a calibration it has not computed"
    );

    // The world moves far enough past the reference that the verdict is
    // informative either way — a move inside the noise floor would grade as
    // inconclusive and calibrate nothing, which is the honest result for a
    // quiet tape and not the one under test. Twenty swinging bars move every
    // observable a claim here can name at once: the last close ends fifty
    // percent above where the series stood, and the realised volatility over
    // the detector's window is an order of magnitude larger than anything the
    // fixture's tape showed.
    let horizon = prediction.proposition.resolves_at;
    assert!(
        horizon > start(),
        "a claim resolving in the past is not a claim"
    );
    let metric = prediction
        .proposition
        .criteria
        .metrics()
        .first()
        .cloned()
        .expect("a threshold names its metric");
    assert!(
        metric.starts_with("close:") || metric.starts_with("volatility:"),
        "the claim is about {metric}; this test moves the close and the volatility"
    );
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

    let snapshot = recorded(&platform);
    assert!(
        snapshot.counter_total(names::THESES_EVALUATED) >= 1,
        "the resolved claim was not counted as evaluated"
    );
    assert!(
        platform.predictions()[0].verdict.is_some(),
        "the claim was graded but not settled"
    );
    let evaluations = platform.evaluations();
    assert!(!evaluations.is_empty());
    assert!(
        evaluations.iter().any(|e| e.verdict.is_informative()),
        "an 80% move graded as inconclusive: {:?}",
        evaluations
            .iter()
            .map(|e| e.rationale.clone())
            .collect::<Vec<_>>()
    );
    let calibration = platform
        .calibration()
        .expect("an informative verdict produces a calibration");
    assert_eq!(
        snapshot.gauge(names::BELIEF_BRIER_SCORE, &labels([])),
        Some(calibration.brier_score),
        "the gauge must be the calibration's own Brier score"
    );
    assert_eq!(
        snapshot.gauge(names::BELIEF_CONFIDENCE_ADJUSTMENT, &labels([])),
        Some(calibration.confidence_adjustment)
    );
    assert_eq!(
        snapshot.gauge(names::BELIEF_EVALUATIONS, &labels([])),
        Some(calibration.evaluated as f64)
    );

    // And it is journaled on the cycle that computed it, and only on that
    // cycle: a replay of the durable log must find the figure where LEARN
    // produced it rather than restated on every entry after.
    let entries = platform.journal_entries()?;
    assert_eq!(entries.len(), 2);
    assert!(
        entries[0].calibration.is_none(),
        "the first cycle journaled a calibration it did not compute"
    );
    let journaled = entries[1]
        .calibration
        .as_ref()
        .expect("the cycle that graded a thesis journals the calibration");
    assert!(journaled.evaluated_this_cycle >= 1);
    assert!(
        (journaled.brier_score - calibration.brier_score).abs() < f64::EPSILON,
        "the journal carries {} and the platform {}",
        journaled.brier_score,
        calibration.brier_score
    );
    Ok(())
}

// --- counterfactual scoring of declined paths -------------------------------

/// Offer an order the controls refuse — it traces to no hypothesis — and hand
/// back its id. The refusal is the premise of every test below.
fn refuse_one(platform: &mut Platform, proposal: &str, at: Timestamp) -> Result<OrderId> {
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        proposal,
        Vec::new(),
        at,
    );
    let order_id = order.order_id.clone();
    assert!(
        platform.submit_order(order, at).is_err(),
        "an untraceable order was accepted; the fixture is not a refusal"
    );
    Ok(order_id)
}

/// Daily bars after the refusal, so the twin has somewhere to enter and a
/// close to mark the horizon at.
fn bars_after(symbol: &str, from: Timestamp, days: i64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            let at = from.saturating_add(Duration::from_days(day));
            let open = 100.0 + day as f64;
            bar(symbol, at, open, open + 0.5)
        })
        .collect()
}

/// A flat, near-silent tape at 100, for a test that needs to know its own
/// entry price rather than read one off a random walk. `bars`'s jump-and-noise
/// fixture is right for the tests above, which do not care what the price
/// is, only that something moved; a test that has to place the twin's
/// simulated entry on a known side of a known level cannot afford that
/// uncertainty.
fn quiet_bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    (0..count)
        .map(|i| {
            let wiggle = if i % 2 == 0 { 0.3 } else { -0.3 };
            let price = 100.0 + wiggle;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, price, price)
        })
        .collect()
}

/// A flat tape at `level`, every day after `from`.
///
/// [`qip_simulation_engine::clock::SimulationClock::fill_price`] marks the
/// twin's alternative at a bar's *open*, not its close, so what this needs to
/// fix is one number the twin will actually read — not a trajectory, which a
/// single day's move would already settle. Flat rather than a one-day spike
/// so the exact bar the horizon lands on cannot matter either.
fn flat_bars_after(symbol: &str, from: Timestamp, days: i64, level: f64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            let at = from.saturating_add(Duration::from_days(day));
            bar(symbol, at, level, level)
        })
        .collect()
}

#[test]
fn a_refused_order_is_priced_once_its_horizon_has_passed_and_charged_to_its_gate() -> Result<()> {
    // The failure this guards: `evaluate_alternatives` priced the paths not
    // taken and was called only by tests, so every veto the gates recorded
    // was a data point nothing scored — blueprint §12's "enormous signal
    // being discarded daily". LEARN now prices each refusal once the world
    // has said what would have happened, and charges the score to the rule
    // that refused.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 90));
    let order_id = refuse_one(&mut platform, "prop-refused", start())?;

    // Premise: the refusal is on the chain, waiting, and nothing is priced.
    assert!(
        !platform.outcomes().refusals().is_empty(),
        "no refusal was captured; there is nothing to price"
    );
    assert_eq!(platform.declined_awaiting_score(), 1);
    assert_eq!(
        recorded(&platform).counter_total(names::COUNTERFACTUALS_SCORED),
        0
    );

    // Before the horizon has passed, nothing is priced: the twin marks the
    // alternative at the horizon, and a path scored on the part of the tape
    // that had happened to print would be a track record manufactured from
    // whatever suited it.
    let early = platform.run_cycle(start());
    assert_eq!(
        recorded(&platform).counter_total(names::COUNTERFACTUALS_SCORED),
        0,
        "a path was priced before its horizon passed:\n{}",
        early.summarise()
    );
    assert_eq!(platform.declined_awaiting_score(), 1);

    platform.observe(bars_after("AAA", start(), 5));
    let later = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let learn = later.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("declined path(s) priced"),
        "LEARN did not report pricing anything: {}",
        learn.detail
    );

    let snapshot = recorded(&platform);
    // Charged to the gate, in the same vocabulary `qip_orders_refused_total`
    // uses, so the ratio §12.3 wants — vetoes that were profitable over
    // vetoes — can be read off two series with one label.
    assert_eq!(
        snapshot.counter(
            names::COUNTERFACTUALS_SCORED,
            &labels([("gate", "order-validation")])
        ),
        1,
        "the score is not charged to the gate that refused: {:?}",
        snapshot
            .series
            .iter()
            .filter(|s| s.name == names::COUNTERFACTUALS_SCORED)
            .map(|s| s.labels.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        platform.declined_awaiting_score(),
        0,
        "a priced path stays queued"
    );
    let scores = platform.declined_scores();
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].order_id, order_id);
    assert_eq!(scores[0].gate, "order-validation");
    assert!(
        scores[0].alternatives > 1,
        "the twin priced only {} alternative(s)",
        scores[0].alternatives
    );
    // The regret bit and the counter agree, whichever way the tape went.
    let regrets = snapshot.counter(
        names::COUNTERFACTUAL_REGRETS,
        &labels([("gate", "order-validation")]),
    );
    assert_eq!(regrets, u64::from(scores[0].regret));
    // And nothing simulated reached the realised line.
    assert_eq!(platform.outcomes().realised_pnl(), Decimal::ZERO);

    let entries = platform.journal_entries()?;
    assert_eq!(entries.len(), 2);
    assert!(
        entries[0].counterfactuals.is_none(),
        "the first cycle journaled a pricing it did not do"
    );
    let journaled = entries[1]
        .counterfactuals
        .as_ref()
        .expect("the cycle that priced a path journals it");
    assert_eq!(journaled.scored, 1);
    assert_eq!(journaled.deferred, 0);
    Ok(())
}

#[test]
fn a_declined_actions_counterfactual_record_names_the_method_that_estimated_it_and_never_joins_the_observed_outcomes()
-> Result<()> {
    // WORLD-039. The failure this guards: a declined path's score said it was
    // simulated (the `Simulated` type) and did not say by what. The pricer
    // was implicit in the code path, so a record read back later could not
    // be told from one a different pricer produced, and a regret series
    // mixing two methods would have looked like one.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 90));
    let order_id = refuse_one(&mut platform, "prop-refused", start())?;
    platform.observe(bars_after("AAA", start(), 5));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));

    // Premise: the cycle priced exactly the action that was declined.
    let scores = platform.declined_scores();
    assert_eq!(scores.len(), 1, "no declined path was priced");
    assert_eq!(scores[0].order_id, order_id);
    assert!(
        scores[0].alternatives > 1,
        "the twin priced nothing, so there is no estimate to attribute"
    );

    // The record names its method, on the value and in the journalled form.
    assert_eq!(
        scores[0].method.as_str(),
        "bar_open_replay",
        "the counterfactual record does not name the method that priced it"
    );
    let journalled = serde_json::to_value(&scores[0]).expect("a score serialises");
    assert_eq!(journalled["method"], "bar_open_replay");

    // And it is an estimate that never joins what was observed: the declined
    // order has no taken outcome, and nothing reached the realised line.
    assert!(
        platform.outcomes().taken().is_empty(),
        "a declined action was counted among the observed outcomes"
    );
    assert_eq!(platform.outcomes().realised_pnl(), Decimal::ZERO);
    Ok(())
}

#[test]
fn declined_paths_past_the_per_cycle_cap_are_counted_as_deferred_and_priced_next_cycle()
-> Result<()> {
    // The cap is eight per cycle. Nine refusals due at once must produce
    // eight scores and one *counted* deferral — not nine scores, which would
    // mean the cap is decorative, and not eight with the ninth silently
    // gone, which is the truncation this test exists to refuse.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 90));
    for n in 0..9 {
        refuse_one(&mut platform, &format!("prop-{n}"), start())?;
    }
    assert_eq!(
        platform.declined_awaiting_score(),
        9,
        "the premise is nine waiting"
    );

    platform.observe(bars_after("AAA", start(), 5));
    let first = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let snapshot = recorded(&platform);
    assert_eq!(snapshot.counter_total(names::COUNTERFACTUALS_SCORED), 8);
    assert_eq!(
        snapshot.counter_total(names::COUNTERFACTUALS_DEFERRED),
        1,
        "the ninth path was not counted as deferred:\n{}",
        first.summarise()
    );
    assert_eq!(
        platform.declined_awaiting_score(),
        1,
        "the deferred path must still be waiting"
    );
    let journaled = platform.journal_entries()?[0]
        .counterfactuals
        .clone()
        .expect("journaled");
    assert_eq!((journaled.scored, journaled.deferred), (8, 1));

    // The next cycle prices what the cap left.
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let snapshot = recorded(&platform);
    assert_eq!(snapshot.counter_total(names::COUNTERFACTUALS_SCORED), 9);
    assert_eq!(snapshot.counter_total(names::COUNTERFACTUALS_DEFERRED), 1);
    assert_eq!(platform.declined_awaiting_score(), 0);
    assert_eq!(
        snapshot.counter_total(names::COUNTERFACTUALS_UNSCORED),
        0,
        "nothing was refused by the twin, so nothing may be counted as unscorable"
    );
    Ok(())
}

// --- counterfactual scoring of filled orders ---------------------------------

/// Offer a traceable buy the controls accept and the simulated venue fills,
/// and hand back its id. The fill is the premise of every test below.
///
/// 700 rather than the 1000 this was until round 3. The default simulated
/// venue fills 60% on acceptance and leaves the rest working, and the
/// pre-trade gate now charges a working remainder as exposure the book has
/// committed to. Ten 1000-lot buys of AAA at 100 commit just over the 10%
/// weight `limits()` sets, so the tenth was refused — the gate seeing what
/// the fixture had always built and never meant to. The tests below are
/// about how fills are learned from, not about sizing; thirteen of these
/// stay inside the limit, which is the most any of them sends.
fn fill_one(platform: &mut Platform, proposal: &str, at: Timestamp) -> Result<OrderId> {
    let fills_before = platform.orders().fills().len();
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("700"),
        dec!("100"),
        proposal,
        vec![format!("hyp-{proposal}")],
        at,
    );
    let order_id = order.order_id.clone();
    platform.submit_order(order, at)?;
    assert!(
        platform.orders().fills().len() > fills_before,
        "the accepted order did not fill; the fixture is not a fill"
    );
    Ok(order_id)
}

#[test]
fn a_filled_order_whose_twin_refuses_to_price_it_is_counted_unscored_and_leaves_the_queue()
-> Result<()> {
    // The refusal case first. A fill on an instrument with one closed bar
    // of history is one the twin cannot estimate liquidity for, and it says
    // so. What must not happen is the path staying in the queue to be
    // refused again every cycle for ever: the twin's answer will not change,
    // and a queue that fills with permanently unpriceable fills is one
    // that, at `DECLINED_HISTORY`, stops taking the fills it could price.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 1));
    fill_one(&mut platform, "prop-thin", start())?;
    assert_eq!(
        platform.filled_awaiting_score(),
        1,
        "the premise failed: the fill was not captured for the twin"
    );
    assert_eq!(
        recorded(&platform).counter(
            names::COUNTERFACTUALS_UNSCORED,
            &labels([("reason", "refused")])
        ),
        0
    );

    platform.observe(bars_after("AAA", start(), 5));
    let cycle = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let learn = cycle.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn
            .problems
            .iter()
            .any(|problem| problem.contains("could not be priced for its size")),
        "the refusal was not reported on the cycle: {:?}",
        learn.problems
    );
    assert_eq!(
        recorded(&platform).counter(
            names::COUNTERFACTUALS_UNSCORED,
            &labels([("reason", "refused")])
        ),
        1,
        "the twin's refusal was not counted"
    );
    assert_eq!(
        platform.filled_awaiting_score(),
        0,
        "a fill the twin refused to price stayed queued"
    );
    assert!(platform.fill_scores().is_empty());
    Ok(())
}

#[test]
fn a_filled_order_is_priced_once_its_horizon_has_passed_and_its_size_regrets_are_recorded()
-> Result<()> {
    // The failure this guards: blueprint §12.3's last row has two halves
    // and the platform read only one. `evaluate_alternatives` could price a
    // *placed* order — the twin's `smaller_size` and `larger_size` arms exist
    // for exactly that — and nothing in production called it for one, so
    // every fill was attributed and never asked whether its size was right.
    let mut platform = platform()?;
    // A tape flat at the same level before and after the fill, rather than
    // a jump to a different level. This is deliberate, not merely quiet: a
    // jump would make the `trade` arm's loss purely directional (a bigger
    // fill loses proportionally more simply because it was more exposed to
    // an adverse move that had nothing to do with its size), and that shape
    // is exactly the code-review finding this test now guards against —
    // `smaller_favoured` used to fire on *any* such loss, however small the
    // transaction cost, because a smaller loss is trivially closer to zero.
    // With the entry and exit price identical, the twin's gross
    // `(entry − exit) × quantity` is exactly zero for every size arm, so the
    // only thing separating `smaller_size`, `trade` and `larger_size` is the
    // commission, spread and tax each pays on its own notional — a genuine,
    // isolated cost-driven regret, which is what `smaller_favoured` must
    // still catch after the fix.
    platform.observe(flat_bars_after(
        "AAA",
        start().saturating_sub(Duration::from_days(91)),
        90,
        100.0,
    ));
    let order_id = fill_one(&mut platform, "prop-filled", start())?;

    // Premise: one fill waiting, nothing declined, nothing priced.
    assert_eq!(platform.filled_awaiting_score(), 1);
    assert_eq!(
        platform.declined_awaiting_score(),
        0,
        "the premise failed: an accepted order was captured as declined"
    );
    assert!(platform.fill_scores().is_empty());

    // Before the horizon, nothing: the twin marks at the horizon.
    platform.run_cycle(start());
    assert_eq!(platform.filled_awaiting_score(), 1);
    assert!(platform.fill_scores().is_empty());

    // The tape after the fill stays at the same level, so the fill's only
    // regret is the transaction cost paid on the size actually taken.
    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    let later = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let learn = later.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("filled path(s) priced for size"),
        "LEARN did not report pricing the fill: {}",
        learn.detail
    );
    assert_eq!(platform.filled_awaiting_score(), 0);
    let scores = platform.fill_scores();
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].order_id, order_id);
    assert_eq!(scores[0].venue, "simulated-venue");
    assert!(
        scores[0].smaller_favoured,
        "a losing fill did not favour the smaller size"
    );
    assert!(
        !scores[0].larger_favoured,
        "a losing fill favoured the larger size"
    );
    // The declined record is untouched: the two halves are scored apart.
    assert!(platform.declined_scores().is_empty());
    // And nothing simulated reached the realised line.
    assert_eq!(platform.outcomes().realised_pnl(), Decimal::ZERO);

    let entries = platform.journal_entries()?;
    assert_eq!(entries.len(), 2);
    let journaled = entries[1]
        .counterfactuals
        .as_ref()
        .expect("the cycle that priced a fill journals it");
    assert_eq!((journaled.fills_scored, journaled.fills_deferred), (1, 0));
    assert_eq!(journaled.scored, 0, "no declined path existed to price");
    Ok(())
}

#[test]
fn a_partly_filled_order_is_scored_as_partial_with_the_quantity_the_venue_left() -> Result<()> {
    // The failure this guards (R12-MODEL-6004): the simulated venue fills 60%
    // of an order on acceptance and leaves the rest working, and the fill
    // score was the same record whether the order had filled whole or in
    // part, so a size finding priced on the quantity that traded read as
    // though the order had been the size that was asked for.
    let mut platform = platform()?;
    platform.observe(flat_bars_after(
        "AAA",
        start().saturating_sub(Duration::from_days(91)),
        90,
        100.0,
    ));
    let order_id = fill_one(&mut platform, "prop-partial", start())?;
    let left = platform
        .orders()
        .order(&order_id)
        .expect("the order the manager booked")
        .remaining_quantity();
    assert!(
        left.is_positive(),
        "premise: the venue left part of the order unfilled ({left})"
    );

    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let scores = platform.fill_scores();
    assert_eq!(scores.len(), 1, "premise: the fill was priced");
    assert_eq!(scores[0].order_id, order_id);
    assert_eq!(
        scores[0].unfilled, left,
        "the score does not carry what the venue left unfilled"
    );
    assert!(scores[0].is_partial());

    // A score written before the field existed reads as whole, the reading
    // those records were made under.
    let mut old = serde_json::to_value(&scores[0]).expect("a score serialises");
    old.as_object_mut()
        .expect("a score is an object")
        .remove("unfilled");
    let old: qip_kernel::platform::FillScore =
        serde_json::from_value(old).expect("an old score decodes");
    assert!(!old.is_partial());
    Ok(())
}

#[test]
fn a_fill_that_lost_purely_to_an_adverse_price_move_does_not_favour_the_smaller_size() -> Result<()>
{
    // The code-review finding this guards: comparing the smaller-size
    // alternative's simulated P&L against the trade's used to mark
    // `smaller_favoured` on *every* fill that lost money before costs at
    // all, regardless of whether the size itself was the problem — a
    // smaller loss is trivially closer to zero than a larger one of the
    // same shape. This is the exact fixture the test above used before the
    // fix: every order here is a buy, and (as that test's own comment
    // states) the twin's book-side convention charges a buy a loss when the
    // tape *rises* after it, so a jump from ~100 to a flat 300 is a large
    // *directional* loss with a transaction cost that is a rounding error
    // beside it — the desk's default cost model charges a few basis points
    // of round-trip commission, spread and tax against a two-hundred-percent
    // adverse move. Before the fix this fixture is exactly what armed
    // `smaller_favoured` on pure direction; after it, this fixture is the
    // mutation the fix is for, made a premise instead of an accident. If the
    // fix regresses to comparing raw simulated P&L, this fill will (wrongly)
    // favour the smaller size, exactly like every other loss.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 90));
    let order_id = fill_one(&mut platform, "prop-adverse", start())?;

    platform.observe(flat_bars_after("AAA", start(), 5, 300.0));
    let later = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let learn = later.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("filled path(s) priced for size"),
        "LEARN did not report pricing the fill: {}",
        learn.detail
    );
    let scores = platform.fill_scores();
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].order_id, order_id);
    assert!(
        !scores[0].smaller_favoured,
        "an adverse price move — not a mis-sized fill — armed the smaller-favoured bit, which \
         is the exact bug this fix closes"
    );
    assert!(
        !scores[0].larger_favoured,
        "an adverse price move favoured the larger size"
    );
    Ok(())
}

#[test]
fn declined_and_filled_paths_share_one_per_cycle_counterfactual_cap() -> Result<()> {
    // Eight declines and one fill due together. The cap is eight
    // evaluations per cycle for both passes — one number, not one each —
    // and the declined pass goes first, so the first cycle prices the eight
    // declines and counts the fill as deferred, and the second cycle prices
    // the fill. A second cap of eight for fills would price nine in cycle
    // one, and the stage's latency budget would be a claim about half of
    // what runs in it.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 90));
    for n in 0..8 {
        refuse_one(&mut platform, &format!("prop-refused-{n}"), start())?;
    }
    fill_one(&mut platform, "prop-filled", start())?;
    assert_eq!(platform.declined_awaiting_score(), 8);
    assert_eq!(platform.filled_awaiting_score(), 1);

    platform.observe(flat_bars_after("AAA", start(), 5, 300.0));
    let first = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let snapshot = recorded(&platform);
    assert_eq!(snapshot.counter_total(names::COUNTERFACTUALS_SCORED), 8);
    assert_eq!(
        platform.fill_scores().len(),
        0,
        "the fill was priced in the same cycle as eight declines:\n{}",
        first.summarise()
    );
    assert_eq!(
        snapshot.counter_total(names::COUNTERFACTUALS_DEFERRED),
        1,
        "the fill was not counted as deferred"
    );
    assert_eq!(platform.filled_awaiting_score(), 1);
    let journaled = platform.journal_entries()?[0]
        .counterfactuals
        .clone()
        .expect("journaled");
    assert_eq!(
        (
            journaled.scored,
            journaled.deferred,
            journaled.fills_scored,
            journaled.fills_deferred
        ),
        (8, 0, 0, 1)
    );

    platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    assert_eq!(platform.fill_scores().len(), 1);
    assert_eq!(platform.filled_awaiting_score(), 0);
    assert_eq!(
        recorded(&platform).counter_total(names::COUNTERFACTUALS_DEFERRED),
        1
    );
    Ok(())
}

#[test]
fn the_twins_entry_price_error_against_the_actual_fill_is_recorded_per_venue() -> Result<()> {
    // Blueprint §12.4's "fill error", which `docs/DELIVERY-STATUS.md`
    // scored as untracked. The twin marks its entry at the open of the bar
    // covering the decision instant; the venue fills a buy at the arrival
    // price plus its modelled cost. The tape here opens its last bar before
    // the fill at 90 against an arrival of 100, so the twin enters ten
    // percent below where the venue actually filled — the flattering
    // direction, which the series must chart as negative.
    let mut platform = platform()?;
    let mut tape = quiet_bars("AAA", 90);
    let last = tape.pop().expect("ninety bars");
    let last_at = match &last {
        SensedRecord::Bar(bar) => bar.open_time,
        _ => unreachable!("quiet_bars produces bars"),
    };
    tape.push(bar("AAA", last_at, 90.0, 90.0));
    platform.observe(tape);
    fill_one(&mut platform, "prop-filled", start())?;
    let venue = labels([("venue", "simulated-venue")]);
    assert!(
        recorded(&platform)
            .histogram(names::VENUE_FILL_ERROR_BPS, &venue)
            .is_none(),
        "the premise failed: a fill error was recorded before anything was priced"
    );

    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let scores = platform.fill_scores();
    assert_eq!(scores.len(), 1, "the fill was not priced");
    let error = scores[0]
        .trade_error_bps
        .expect("a fill the twin priced carries its entry error");
    assert!(
        error < -500.0,
        "the twin entered ten percent below the venue's fill and the error reads {error} bps"
    );
    let snapshot = recorded(&platform);
    let histogram = snapshot
        .histogram(names::VENUE_FILL_ERROR_BPS, &venue)
        .expect("the fill error is charted by venue");
    assert_eq!(histogram.count, 1);
    assert!(
        (histogram.sum - error).abs() < 1e-9,
        "the histogram holds {} where the score says {error}",
        histogram.sum
    );
    Ok(())
}

/// R12-TICK-10103: the LEARN score compares the twin's predicted fill,
/// slippage and outcome with what happened. The tape opens its last bar
/// before the fill at 90 against an arrival of 100, so the twin predicts an
/// entry ten percent below the arrival while the venue fills a buy at the
/// arrival plus its cost; the bars after the fill print 100, which is the
/// price at the twin's exit instant.
///
/// The outcome is per unit and net of what each side paid to get in, both
/// marked at that printed exit: the twin's from its entry less its cost
/// model's entry charge, the venue's from its fill less the costs the venue
/// actually charged. An independent review found the first version of this
/// residual marked two gross returns at one close, which is the entry error
/// rescaled: it read zero whenever the twin's entry matched the fill,
/// whatever the costs did. So the realised figure is checked against the
/// order manager's own fills and costs, worked here by hand, and the
/// predicted figure must sit strictly under the twin's gross return.
///
/// Mutations, each fails here: record `outcome_error_bps` as `None`; drop
/// `paid / filled` from the realised outcome (the hand-worked figure
/// disagrees); drop the twin's entry costs from the predicted outcome (it
/// equals the gross return).
#[test]
fn a_filled_orders_learn_score_records_predicted_and_realised_slippage_and_the_outcome_residual()
-> Result<()> {
    let mut platform = platform()?;
    let mut tape = quiet_bars("AAA", 90);
    let last = tape.pop().expect("ninety bars");
    let last_at = match &last {
        SensedRecord::Bar(bar) => bar.open_time,
        _ => unreachable!("quiet_bars produces bars"),
    };
    tape.push(bar("AAA", last_at, 90.0, 90.0));
    platform.observe(tape);
    fill_one(&mut platform, "prop-residuals", start())?;
    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));

    let scores = platform.fill_scores();
    assert_eq!(scores.len(), 1, "premise: the fill was priced");
    let score = &scores[0];
    let realised = score
        .realised_slippage_bps
        .expect("the desk's fill has an arrival price to slip against");
    assert!(
        realised >= 0.0,
        "a buy filled at the arrival plus cost slipped {realised} bps"
    );
    let predicted = score
        .predicted_slippage_bps
        .expect("the twin's entry has a slippage too");
    assert!(
        predicted < -500.0,
        "the twin predicted an entry ten percent under the arrival: {predicted} bps"
    );
    let fills: Vec<_> = platform
        .orders()
        .fills()
        .into_iter()
        .filter(|fill| fill.order_id == score.order_id)
        .collect();
    assert!(
        !fills.is_empty(),
        "premise: the order manager holds the fill"
    );
    let filled: f64 = fills.iter().map(|fill| fill.quantity.to_f64()).sum();
    let paid: f64 = fills.iter().map(|fill| fill.costs.to_f64()).sum();
    let fill_price = fills[0].price.to_f64();
    assert!(
        paid > 0.0,
        "premise: the venue charged costs, so leaving them out would move the figure"
    );
    let exit = 100.0;
    let realised_outcome = score
        .realised_outcome_bps
        .expect("the venue's fill has an outcome at the twin's exit");
    let worked = ((exit - fill_price) - paid / filled) / fill_price * 10_000.0;
    assert!(
        (realised_outcome - worked).abs() < 1e-6,
        "the realised outcome is the fill marked at {exit} net of the venue's costs: \
         {realised_outcome} against {worked} bps"
    );
    let arrival = platform
        .orders()
        .order(&score.order_id)
        .expect("the order manager holds the order")
        .arrival_price
        .to_f64();
    let twin_entry = arrival * (1.0 + predicted / 10_000.0);
    let twin_gross = (exit - twin_entry) / fill_price * 10_000.0;
    let predicted_outcome = score
        .predicted_outcome_bps
        .expect("the twin traded, so it predicted an outcome");
    assert!(
        predicted_outcome > 0.0 && predicted_outcome < twin_gross - 1e-6,
        "the twin's outcome from {twin_entry} to {exit} is net of its entry costs: \
         {predicted_outcome} against a gross {twin_gross} bps"
    );
    let residual = score
        .outcome_error_bps
        .expect("the horizon closed, so the outcome residual exists");
    assert!(
        (residual - (predicted_outcome - realised_outcome)).abs() < 1e-9,
        "the residual is predicted less realised: {residual}"
    );
    assert!(
        residual > 500.0,
        "the twin's outcome from 90 beats the realised one from the fill: {residual} bps"
    );
    Ok(())
}

/// R12-TICK-2080: the twin is recalibrated from the fills it mispriced, and
/// the calibration is what its next pricing on that venue uses.
///
/// Mutation: delete the `fill_calibration.observe` call in `score_filled` —
/// the venue holds no calibration. Mutation: price with
/// `CostModel::liquid_equity()` again instead of the calibrated model — the
/// spread assertion still passes here, so the observation count and bias
/// are the guard on the first half and `qip-twin`'s
/// `after_fills_with_a_systematic_error_the_twins_spread_moves_toward_the_observed_fills`
/// on the second.
#[test]
fn a_fill_the_twin_mispriced_moves_that_venues_calibrated_spread_toward_the_fill() -> Result<()> {
    let mut platform = platform()?;
    let mut tape = quiet_bars("AAA", 90);
    let last = tape.pop().expect("ninety bars");
    let last_at = match &last {
        SensedRecord::Bar(bar) => bar.open_time,
        _ => unreachable!("quiet_bars produces bars"),
    };
    tape.push(bar("AAA", last_at, 90.0, 90.0));
    platform.observe(tape);
    fill_one(&mut platform, "prop-filled", start())?;
    assert!(
        platform
            .fill_calibration()
            .venue("simulated-venue")
            .is_none(),
        "the premise failed: the venue was calibrated before any fill was priced"
    );
    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let error = platform.fill_scores()[0]
        .trade_error_bps
        .expect("the fill was priced");
    let calibration = platform
        .fill_calibration()
        .venue("simulated-venue")
        .expect("the priced fill calibrated its venue");
    assert_eq!(calibration.observations, 1);
    assert!(
        (calibration.bias_bps - 0.2 * error).abs() < 1e-9,
        "one fifth of the way to the observed error {error}: {}",
        calibration.bias_bps
    );
    let base = qip_simulation_engine::costs::CostModel::liquid_equity();
    let calibrated = platform
        .fill_calibration()
        .cost_model(&base, "simulated-venue");
    assert!(
        calibrated.half_spread_bps > base.half_spread_bps,
        "a twin that filled better than the venue now pays a wider spread there"
    );
    Ok(())
}

#[test]
fn a_twin_that_is_wildly_wrong_about_fills_never_withdraws_a_venue() -> Result<()> {
    // The "diagnostic only" guarantee of ADR 0062. `qip_venue_fill_error_bps`
    // measures the twin against the venue; a venue is withdrawn on
    // feasibility evidence and on nothing else. Twelve fills the twin
    // misprices by a thousand basis points each — the tape opens its last
    // bar before the fills at 90 against an arrival of 100 — and after LEARN
    // has priced every one of them, no venue is withdrawn, no withdrawal is
    // on the record, and the feasibility window is empty because nothing
    // was infeasible. A review that read the fill error would fail here.
    let mut platform = platform()?;
    let mut tape = quiet_bars("AAA", 90);
    let last = tape.pop().expect("ninety bars");
    let last_at = match &last {
        SensedRecord::Bar(bar) => bar.open_time,
        _ => unreachable!("quiet_bars produces bars"),
    };
    tape.push(bar("AAA", last_at, 90.0, 90.0));
    platform.observe(tape);
    const FILLS: usize = 12;
    for n in 0..FILLS {
        fill_one(&mut platform, &format!("prop-filled-{n}"), start())?;
    }
    assert_eq!(platform.filled_awaiting_score(), FILLS);

    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    let scoring_time = start().saturating_add(Duration::from_days(3));
    platform.run_cycle(scoring_time);
    platform.run_cycle(scoring_time);
    let scores = platform.fill_scores();
    assert_eq!(scores.len(), FILLS, "not every fill was priced");
    assert!(
        scores
            .iter()
            .all(|score| score.trade_error_bps.is_some_and(|bps| bps < -500.0)),
        "the premise failed: the twin was not wildly wrong on every fill: {:?}",
        scores
            .iter()
            .map(|score| score.trade_error_bps)
            .collect::<Vec<_>>()
    );

    assert!(
        platform.withdrawn_venues().is_empty(),
        "a venue was withdrawn on fill-error evidence: {:?}",
        platform.withdrawn_venues()
    );
    assert!(
        platform.feasibility_refusals().is_empty(),
        "a fill error reached the feasibility window"
    );
    let withdrawn = platform
        .replay_journal(&qip_events::EventFilter::new().topic(qip_events::Topic::VenueWithdrawn))?;
    assert!(withdrawn.is_empty(), "a withdrawal was journaled");
    // And the venue still fills.
    fill_one(&mut platform, "prop-filled-after", scoring_time)?;
    Ok(())
}

/// Withdraw the desk's venue the way LEARN does: ten off-lot buys on `AAA`
/// (a one-lot listing), every one refused by the lot gate at the desk's
/// one broker, then a cycle.
fn withdraw_the_desk_venue(platform: &mut Platform) -> Result<()> {
    for n in 0..10 {
        let order = platform.order_from(
            object("AAA"),
            Side::Buy,
            dec!("10.5"),
            dec!("100"),
            &format!("prop-off-lot-{n}"),
            vec![format!("hyp-off-lot-{n}")],
            start(),
        );
        let error = platform
            .submit_order(order, start())
            .expect_err("ten and a half shares of a one-lot listing reached the venue");
        assert!(
            error.message().contains("infeasible (feasibility_lot):"),
            "the premise failed: refused for another reason than the lot grid: {}",
            error.message()
        );
    }
    platform.run_cycle(start());
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec!["simulated-venue"],
        "the premise failed: ten lot refusals did not withdraw the desk venue"
    );
    Ok(())
}

fn operator(name: &str, at: Timestamp) -> qip_risk_engine::autonomy::OperatorIdentity {
    qip_risk_engine::autonomy::OperatorIdentity::verified(name, "hardware-token", at)
}

fn reinstatements(
    platform: &Platform,
) -> Result<Vec<qip_kernel::venue_review::VenueReinstatementEntry>> {
    platform
        .replay_journal(&qip_events::EventFilter::new().topic(qip_events::Topic::VenueReinstated))?
        .iter()
        .map(|envelope| {
            Ok(envelope
                .decode::<qip_kernel::venue_review::VenueReinstatementEntry>()?
                .body)
        })
        .collect()
}

#[test]
fn reinstatement_needs_two_different_fresh_operators_and_is_journaled_at_each_signature()
-> Result<()> {
    // Putting a venue the platform stopped using back into use is a person
    // widening what the platform may do, so it is held to the promotion's
    // discipline: a fresh credential, two distinct people, every signature
    // on the record before anything changes. The refusals come first — a
    // venue nothing withdrew, a stale credential, the same person twice —
    // and each leaves the venue withdrawn, which the next order proves.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 30));
    let now = start();
    // Refusal first: nothing is withdrawn, so nothing can be reinstated.
    let not_withdrawn = platform
        .reinstate_venue(
            "simulated-venue",
            &operator("alice", now),
            "it is fine",
            now,
        )
        .expect_err("a venue nothing withdrew accepted a reinstatement signature");
    assert!(
        not_withdrawn.message().contains("is not withdrawn"),
        "{}",
        not_withdrawn.message()
    );

    withdraw_the_desk_venue(&mut platform)?;
    let refused_order = |platform: &mut Platform, n: usize| -> bool {
        let order = platform.order_from(
            object("AAA"),
            Side::Buy,
            dec!("1000"),
            dec!("100"),
            &format!("prop-probe-{n}"),
            vec![format!("hyp-probe-{n}")],
            now,
        );
        platform.submit_order(order, now).is_err_and(|error| {
            error
                .message()
                .contains("withdrawn on feasibility evidence")
        })
    };
    assert!(
        refused_order(&mut platform, 0),
        "the premise failed: the venue admits orders"
    );

    // A stale credential signs nothing.
    let stale = operator("alice", now.saturating_sub(Duration::from_mins(20)));
    let refused = platform
        .reinstate_venue("simulated-venue", &stale, "it is fine", now)
        .expect_err("a stale credential signed a reinstatement");
    assert!(
        refused.message().contains("re-authenticate"),
        "{}",
        refused.message()
    );
    assert!(
        reinstatements(&platform)?.is_empty(),
        "a refused signature was journaled"
    );
    assert!(refused_order(&mut platform, 1));

    // The first signature is journaled and changes nothing.
    let first = platform.reinstate_venue(
        "simulated-venue",
        &operator("alice", now),
        "the venue's grid was corrected in the catalogue",
        now,
    )?;
    assert_eq!(first.outcome, "awaiting_countersignature");
    assert_eq!(first.approver, "alice");
    assert_eq!(first.second_approver, None);
    assert!(
        refused_order(&mut platform, 2),
        "one signature reinstated the venue"
    );
    assert_eq!(reinstatements(&platform)?.len(), 1);

    // The same person again is not a second person.
    let same = platform
        .reinstate_venue(
            "simulated-venue",
            &operator("alice", now),
            "still fine",
            now,
        )
        .expect_err("one operator countersigned their own signature");
    assert!(
        same.message()
            .contains("a second session is not a second person"),
        "{}",
        same.message()
    );
    assert!(refused_order(&mut platform, 3));
    assert_eq!(
        reinstatements(&platform)?.len(),
        1,
        "a refused countersignature was journaled"
    );

    // A second, distinct, fresh person: the record first, then the venue.
    let second = platform.reinstate_venue(
        "simulated-venue",
        &operator("bram", now),
        "confirmed against the venue's own specification",
        now,
    )?;
    assert_eq!(second.outcome, "reinstated");
    assert_eq!(second.approver, "alice");
    assert_eq!(second.second_approver.as_deref(), Some("bram"));
    assert!(platform.withdrawn_venues().is_empty());
    let records = reinstatements(&platform)?;
    assert_eq!(records.len(), 2);
    assert_eq!(
        records
            .iter()
            .map(|record| record.outcome.as_str())
            .collect::<Vec<_>>(),
        vec!["awaiting_countersignature", "reinstated"]
    );
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-after-reinstatement",
        vec!["hyp-after".to_string()],
        now,
    );
    platform.submit_order(order, now)?;

    // And the reinstatement survives a LEARN pass.
    //
    // **This assertion was the opposite until 2026-09-14, and the old one
    // was wrong.** It read "a reinstated venue can be withdrawn again on
    // fresh evidence … the window still holds the ten, and nothing has
    // reinstated the *evidence*, so the next review withdraws it once more",
    // and asserted the venue was withdrawn after this cycle. Those ten
    // refusals are not fresh evidence — they are the *same* evidence the
    // withdrawal was made on, still sitting in the window the signatures did
    // not reach. So a reinstatement had never survived a single cycle in
    // this platform's history: two people signed, and the machine reverted
    // them before anybody could place a second order. A control the system
    // undoes by itself is the `MaxExpectedShortfall` shape this repository
    // names, and a test asserting the revert pinned the defect in place.
    //
    // What two signatures mean is that the accumulated evidence no longer
    // binds this venue (`venue_review::pardon`), so the venue must earn a
    // fresh cluster before it goes again — which the half below proves it
    // still can.
    platform.run_cycle(now);
    assert!(
        platform.withdrawn_venues().is_empty(),
        "the platform withdrew a venue two operators had just reinstated, on the very evidence \
         they signed away: {:?}",
        platform.withdrawn_venues()
    );
    assert_eq!(
        withdrawals_recorded(&platform)?,
        1,
        "a second withdrawal record was written for a venue that has refused nothing since"
    );

    // The admitting half, without which everything above passes against a
    // reinstatement that disables the control for ever. Ten *new* off-lot
    // refusals at the reinstated venue are evidence nobody signed away, and
    // they withdraw it again, with its own record on its own cluster.
    withdraw_the_desk_venue(&mut platform)?;
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec!["simulated-venue"],
        "a venue that was once reinstated could never be withdrawn again, whatever it refused"
    );
    assert_eq!(
        withdrawals_recorded(&platform)?,
        2,
        "the second withdrawal is not on the record as its own finding"
    );
    Ok(())
}

/// How many `venue.withdrawn` records the log holds.
fn withdrawals_recorded(platform: &Platform) -> Result<usize> {
    Ok(platform
        .replay_journal(&qip_events::EventFilter::new().topic(qip_events::Topic::VenueWithdrawn))?
        .len())
}

#[test]
fn the_two_reinstatement_signatures_are_held_to_one_rationale_floor() -> Result<()> {
    // The record of why a venue the platform stopped using was put back is
    // the *second* signer's text — `reinstate_venue` writes `rationale` into
    // the entry on the countersignature — and until 2026-09-14 only the first
    // signature was held to a floor. `Approval::new` refuses a rationale
    // under ten trimmed characters; `Approval::countersigned_by` checks
    // nothing, and the countersigning arm wrote the string straight onto the
    // entry. So the first signer had to state a reviewable reason and the
    // second could write "x", which is the one that survives on the record.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 30));
    let now = start();
    withdraw_the_desk_venue(&mut platform)?;

    // The premise: the floor is real on the first signature, and it is ten
    // trimmed characters. Asserted against `Approval` itself rather than
    // assumed, because `COUNTERSIGNATURE_RATIONALE_FLOOR` is a constant
    // mirroring a number the contracts crate does not export — a mirror that
    // drifts would hold the two signers to different floors and nobody would
    // know.
    //
    // **The two strings below are the whole of that proof, and until
    // 2026-09-14 they were not.** This premise passed `"  fine  "` — four
    // trimmed characters, described in its own message as nine — and the
    // countersignature `"   x   "`, one. Both sit so far below either floor
    // that the kernel's constant was free to move: a review dropped it from
    // ten to five and all three tests that claim to hold it stayed green, so
    // the second signer could have been held to half the first signer's bar
    // with nothing catching it. Nine trimmed characters must be refused and
    // exactly ten admitted at *both* signatures, which pins each floor from
    // both sides and so pins them equal.
    const BELOW_FLOOR: &str = "  lot fixed  ";
    const AT_FLOOR: &str = "  grid fixed  ";
    assert_eq!(BELOW_FLOOR.trim().len(), 9);
    assert_eq!(AT_FLOOR.trim().len(), 10);

    let too_short = platform
        .reinstate_venue("simulated-venue", &operator("alice", now), BELOW_FLOOR, now)
        .expect_err("a rationale one character below the floor signed the first half");
    assert!(
        too_short
            .message()
            .contains("state a rationale somebody can review later"),
        "{}",
        too_short.message()
    );
    assert!(
        reinstatements(&platform)?.is_empty(),
        "a refused first signature was journaled"
    );

    // And admitted at exactly the floor, which pins `Approval::new`'s number
    // from above: a contracts-side floor of eleven fails here rather than
    // silently holding the first signer to a bar the second is not.
    let first =
        platform.reinstate_venue("simulated-venue", &operator("alice", now), AT_FLOOR, now)?;
    assert_eq!(first.outcome, "awaiting_countersignature");

    // The countersignature, one character short of the same floor. Trimmed,
    // so padding it with spaces does not buy a signer past the bar either.
    let thin = platform
        .reinstate_venue("simulated-venue", &operator("bram", now), BELOW_FLOOR, now)
        .expect_err("a countersignature below the floor reinstated a withdrawn venue");
    assert!(
        thin.message()
            .contains("must state a rationale somebody can review later"),
        "the countersignature was refused for some other reason: {}",
        thin.message()
    );
    assert!(
        thin.message().contains("reinstatement"),
        "the refusal does not name which signature is short: {}",
        thin.message()
    );
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec!["simulated-venue"],
        "a refused countersignature reinstated the venue anyway"
    );
    assert_eq!(
        reinstatements(&platform)?.len(),
        1,
        "a refused countersignature was journaled"
    );
    // And the first signature is still standing: a refusal on the floor is
    // not a refusal of the pair, so the second signer can restate their
    // reason rather than sending the first back to sign again.
    assert!(
        platform
            .pending_venue_reinstatement("simulated-venue")
            .is_some(),
        "a rationale too short discarded the first signature"
    );

    // The admitting half. Without it this test passes against a
    // `reinstate_venue` that refuses every countersignature, which is a floor
    // nobody can clear rather than a floor.
    let second =
        platform.reinstate_venue("simulated-venue", &operator("bram", now), AT_FLOOR, now)?;
    assert_eq!(second.outcome, "reinstated");
    assert_eq!(second.second_approver.as_deref(), Some("bram"));
    assert_eq!(
        second.rationale.trim().len(),
        AT_FLOOR.trim().len(),
        "the record does not carry the second signer's own text"
    );
    assert!(platform.withdrawn_venues().is_empty());
    Ok(())
}

#[test]
fn a_larger_size_pattern_journals_a_sizing_proposal_and_leaves_every_bound_where_it_was()
-> Result<()> {
    // ADR 0063's loosening direction. Ten filled buys before a tape that
    // falls by half: on the twin's tape every one profits on the `trade` arm
    // and profits more at twice the size, so `larger_size` wins on every
    // fill. §12.4 forbids acting on that automatically, and the only
    // consequence is a record: the proposal is journaled once, the cap on
    // the instrument stays at one, ADR 0055's number stays at one, and no
    // arming is on the record. A review that read the larger finding into
    // a bound — in either direction — fails here.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 90));
    const SAMPLE: usize = 10;
    for n in 0..SAMPLE {
        fill_one(&mut platform, &format!("prop-filled-{n}"), start())?;
    }
    platform.observe(flat_bars_after("AAA", start(), 5, 50.0));
    let scoring_time = start().saturating_add(Duration::from_days(3));
    platform.run_cycle(scoring_time);
    platform.run_cycle(scoring_time);
    let fills = platform.fill_scores();
    assert_eq!(fills.len(), SAMPLE, "not every fill was priced");
    assert_eq!(
        fills.iter().filter(|score| score.larger_favoured).count(),
        SAMPLE,
        "the premise failed: a winning fill did not favour the larger size: {:?}",
        fills
            .iter()
            .map(|score| (score.smaller_favoured, score.larger_favoured))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fills.iter().filter(|score| score.smaller_favoured).count(),
        0,
        "the premise failed: a winning fill favoured the smaller size too"
    );

    // Every bound where it was.
    assert_eq!(
        platform.sizing_cap_multiplier(object("AAA").as_str()),
        Decimal::ONE,
        "a larger-size pattern moved the weight-bound cap"
    );
    assert_eq!(
        platform.sizing_confidence(object("AAA").as_str(), scoring_time)?,
        Decimal::ONE,
        "a larger-size pattern moved sizing confidence"
    );

    // And the record: one proposal, no arming, journaled once.
    let reviewed = platform
        .replay_journal(&qip_events::EventFilter::new().topic(qip_events::Topic::SizingReviewed))?;
    assert_eq!(
        reviewed.len(),
        1,
        "the sizing review wrote {} record(s)",
        reviewed.len()
    );
    let proposal = reviewed[0]
        .decode::<qip_kernel::sizing_review::SizingProposal>()?
        .body;
    assert_eq!(proposal.object_id, "obj-AAA");
    assert_eq!(proposal.direction, "larger");
    assert_eq!(proposal.outcome, "proposed");
    assert_eq!(proposal.sample, SAMPLE);
    assert!((proposal.fraction - 1.0).abs() < f64::EPSILON);
    platform.run_cycle(scoring_time);
    let reviewed = platform
        .replay_journal(&qip_events::EventFilter::new().topic(qip_events::Topic::SizingReviewed))?;
    assert_eq!(reviewed.len(), 1, "a standing proposal was journaled again");
    Ok(())
}

// --- ADR 0055: the counterfactual record narrows sizing, never widens it ---

#[test]
fn a_persistent_pattern_of_unfavourable_declines_on_one_instrument_narrows_only_that_instruments_sizing()
-> Result<()> {
    // The failure this guards: blueprint §12.3 named it exactly —
    // "nothing consumes the findings… no sizing function is adjusted from a
    // counterfactual result." `score_declined` already prices every refused
    // path (the test above); this proves the LEARN stage's own findings now
    // reach the DECIDE stage's sizing at the seam
    // `docs/DELIVERY-STATUS.md` §11.2 already scores as production —
    // `Platform::sizing_confidence` — and reach only the instrument the
    // pattern is actually about.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 90));

    // ADR 0055's `COUNTERFACTUAL_SIZING_MIN_SAMPLE`. Ten refusals on the same
    // instrument, all declined for lack of a hypothesis (`refuse_one`), which
    // says nothing yet about whether declining them was right.
    const SAMPLE: usize = 10;
    for n in 0..SAMPLE {
        refuse_one(&mut platform, &format!("prop-refused-{n}"), start())?;
    }
    assert_eq!(
        platform.declined_awaiting_score(),
        SAMPLE,
        "the premise failed: not every refusal was captured for the twin"
    );
    // Premise: before anything is scored, a fresh platform's counterfactual
    // record narrows nothing — the mechanism under test has not run yet.
    assert_eq!(
        platform.sizing_confidence(object("AAA").as_str(), start())?,
        Decimal::ONE,
        "the premise failed: sizing is already narrowed before any path is scored"
    );

    // Every refusal here is a buy, and `Platform::capture_submission` records
    // a buy against the ask (`book_side`'s own doc comment: "a buy lifts the
    // offer"), so `ActualTrade::direction` for it is `-1`
    // (`qip_twin::counterfactual`'s `Bid => 1, Ask => -1`) and the twin's
    // gross for the "trade" alternative is `(entry_price - exit_price) *
    // quantity`: a *rise* after the refusal is what the twin charges the
    // declined buy a loss for. The tape jumps from the quiet ~100 entry to a
    // flat 300 immediately after the refusal, so every one of these ten
    // declined buys should mark as correctly declined — unambiguously,
    // because the jump is two hundred, not a trajectory nobody computed by
    // hand.
    platform.observe(flat_bars_after("AAA", start(), 5, 300.0));
    let scoring_time = start().saturating_add(Duration::from_days(3));
    // Two cycles: `COUNTERFACTUALS_PER_CYCLE` caps one LEARN pass at eight,
    // and ten declines need a second pass to drain — the same two-cycle
    // shape `declined_paths_past_the_per_cycle_cap_are_counted_as_deferred_and_priced_next_cycle`
    // above proves the cap by, reused here to reach a full sample.
    platform.run_cycle(scoring_time);
    platform.run_cycle(scoring_time);
    assert_eq!(
        platform.declined_awaiting_score(),
        0,
        "not every declined path was priced across two LEARN passes"
    );

    let scores = platform.declined_scores();
    assert_eq!(scores.len(), SAMPLE, "an unrelated path was captured too");
    let unfavourable = scores.iter().filter(|score| !score.regret).count();
    assert_eq!(
        unfavourable,
        SAMPLE,
        "the premise failed: a falling price after a declined buy did not score every path as \
         unfavourable, so the pattern below proves nothing about the discipline — regrets: {:?}",
        scores.iter().map(|s| s.regret).collect::<Vec<_>>()
    );

    // The consequence: the instrument this pattern is about is narrowed to
    // exactly ADR 0055's stated discount, never further and never less.
    let narrowed = platform.sizing_confidence(object("AAA").as_str(), scoring_time)?;
    assert_eq!(
        narrowed,
        dec!("0.5"),
        "a clear, sufficient pattern of unfavourable declines did not narrow sizing confidence \
         to the ADR 0055 discount"
    );
    assert!(
        narrowed < Decimal::ONE,
        "the discount did not narrow sizing confidence at all"
    );

    // And the instrument beside it, with no counterfactual record of its
    // own, is untouched — the finding is about this name's own proposals,
    // not a global brake on every instrument's sizing.
    assert_eq!(
        platform.sizing_confidence(object("BBB").as_str(), scoring_time)?,
        Decimal::ONE,
        "an unrelated instrument's sizing was narrowed by another instrument's record"
    );

    // --- ADR 0063: the executed-order half, on disjoint evidence ---------
    // Premise: with no fill priced, the fill-record cap is one on both names,
    // so what moves below is the fills and only the fills.
    assert_eq!(
        platform.sizing_cap_multiplier(object("AAA").as_str()),
        Decimal::ONE
    );
    assert!(platform.fill_scores().is_empty());

    // Ten filled buys on `AAA`, submitted two days in — once the tape has
    // already settled onto the flat 300 plateau, not during the jump that
    // produced it. This is deliberate: a fill whose entry and exit both sit
    // on the plateau has an *identical* twin entry and exit price, so its
    // `trade` arm's gross is exactly zero and its only regret is the
    // transaction cost the size actually taken paid — a genuine, isolated
    // cost-driven case, not the pure-adverse-price-move shape that used to
    // make `smaller_favoured` fire on any loss regardless of size (the
    // code-review finding the fix above addresses). Scored on its own,
    // later cycle so its horizon — anchored to its own later decision
    // instant — has genuinely passed.
    let fill_time = start().saturating_add(Duration::from_days(2));
    let fill_scoring_time = start().saturating_add(Duration::from_days(4));
    for n in 0..SAMPLE {
        fill_one(&mut platform, &format!("prop-filled-{n}"), fill_time)?;
    }
    assert_eq!(platform.filled_awaiting_score(), SAMPLE);
    platform.run_cycle(fill_scoring_time);
    platform.run_cycle(fill_scoring_time);
    assert_eq!(
        platform.filled_awaiting_score(),
        0,
        "not every fill was priced"
    );
    let fills = platform.fill_scores();
    assert_eq!(fills.len(), SAMPLE);
    assert_eq!(
        fills.iter().filter(|score| score.smaller_favoured).count(),
        SAMPLE,
        "the premise failed: a cost-driven fill did not favour the smaller size: {:?}",
        fills
            .iter()
            .map(|score| (score.smaller_favoured, score.larger_favoured))
            .collect::<Vec<_>>()
    );

    // The consequence: the weight-bound cap on `AAA` is ADR 0063's stated
    // multiplier, while ADR 0055's number is exactly where it was — the two
    // read disjoint evidence, and a fill record folded into
    // `counterfactual_sizing_multiplier` would move this figure.
    assert_eq!(
        platform.sizing_cap_multiplier(object("AAA").as_str()),
        dec!("0.5"),
        "a clear, sufficient pattern of smaller-favoured fills did not arm the sizing cap"
    );
    assert_eq!(
        platform.sizing_confidence(object("AAA").as_str(), scoring_time)?,
        dec!("0.5"),
        "the fill record moved the declined-path narrowing, which reads disjoint evidence"
    );
    assert_eq!(
        platform.sizing_cap_multiplier(object("BBB").as_str()),
        Decimal::ONE
    );
    assert_eq!(
        platform.sizing_confidence(object("BBB").as_str(), scoring_time)?,
        Decimal::ONE
    );
    // And the arming is on the record, once.
    let armed = platform
        .replay_journal(&qip_events::EventFilter::new().topic(qip_events::Topic::SizingReviewed))?;
    assert_eq!(
        armed.len(),
        1,
        "the cap's arming is not on the record exactly once"
    );
    platform.run_cycle(scoring_time);
    let armed = platform
        .replay_journal(&qip_events::EventFilter::new().topic(qip_events::Topic::SizingReviewed))?;
    assert_eq!(armed.len(), 1, "a standing cap was journaled again");
    Ok(())
}

// --- meta-learning: which class of claim worked, and where -------------------

#[test]
fn a_resolved_claim_is_scored_against_the_regime_it_resolved_in() -> Result<()> {
    // `qip_evolution::scoring::Scoreboard` had no caller outside its own crate
    // — the platform graded every thesis and never asked which *kinds* of
    // thesis work in which regime, which is the whole of blueprint §15.1's
    // first capability. The board is written at the one instant that fact
    // becomes known: where a claim resolves.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());

    // Premise, twice over. A claim must have been made, and the board must be
    // empty: asserting a non-empty board at the end proves nothing if it was
    // never empty.
    assert!(
        !platform.predictions().is_empty(),
        "no claim was written, so there is nothing to resolve:\n{}",
        first.summarise()
    );
    assert!(
        platform.claim_scores().is_empty(),
        "the board holds a score before anything resolved"
    );
    let prediction = platform.predictions()[0].clone();
    let claim = prediction
        .claim
        .clone()
        .expect("a claim records what it claimed, or it cannot be graded");
    assert!(
        claim.direction != 0.0,
        "a claim naming no direction cannot be right or wrong, and is \
         deliberately not scored — this fixture must produce a directional one"
    );

    // The same swinging tape the calibration test uses: the move has to be
    // large enough that the verdict is informative either way.
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
        learn.detail.contains("scored by regime"),
        "LEARN graded a claim and did not score it by regime: {}",
        learn.detail
    );

    // Keyed on the claim's *class*, not its hypothesis id. A hypothesis
    // resolves once and never recurs, so a board keyed on it would hold one
    // observation per cell for ever and could never leave its prior — a
    // meta-learner that learns nothing. The class is what recurs.
    let scores = platform.claim_scores().scores_of(&claim.class);
    assert_eq!(
        scores.len(),
        1,
        "the resolved claim's class carries no score: subjects are {:?}",
        platform.claim_scores().subjects()
    );
    let score = &scores[0];
    // The context is the regime the claim resolved in, both axes together: a
    // claim that works in a calm trend and fails in a volatile one is exactly
    // the conditional fact the board exists to keep.
    assert!(
        score.context().contains('/'),
        "the context {:?} does not carry both regime axes",
        score.context()
    );
    // And one observation is not evidence. The band says so in words rather
    // than leaving a reader to notice the count.
    assert_eq!(
        score.band(),
        qip_evolution::scoring::ScoreBand::Unproven,
        "a single resolution produced a score above `Unproven`: {}",
        score.summarise()
    );
    Ok(())
}

#[test]
fn counterfactual_outcomes_are_journaled_for_reproducibility() -> Result<()> {
    // AMBIENT-012: Individual counterfactual outcomes (DeclinedScore and
    // FillScore records) are journaled so they can be reproduced from the
    // event log. The journal captures the outcomes known at each cycle,
    // bounded by a 256-entry history.
    //
    // Mutation verified: removing the line that populates fill_outcomes in the
    // journal causes this test to fail with the assertion that fill_outcomes is
    // not empty, confirming the implementation is necessary.
    let mut platform = platform()?;
    let tape = quiet_bars("AAA", 100);
    platform.observe(tape);

    // Set up a filled order so there are fill scores to journal
    const FILLS: usize = 3;
    for n in 0..FILLS {
        fill_one(&mut platform, &format!("prop-filled-{n}"), start())?;
    }
    assert_eq!(platform.filled_awaiting_score(), FILLS);

    // Run a cycle to score the fills and journal the outcomes
    let scoring_time = start().saturating_add(Duration::from_days(3));
    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    platform.run_cycle(scoring_time);

    // Access the journal and verify that fill_outcomes are present
    let entries = platform.journal_entries()?;
    assert!(
        !entries.is_empty(),
        "at least one cycle should be journaled"
    );

    // The journal entry should contain fill_outcomes
    let journal = &entries.last().expect("last entry exists").counterfactuals;
    assert!(
        journal.is_some(),
        "the cycle with fill scores should journal counterfactual outcomes"
    );

    let counterfactual = journal.as_ref().expect("journal exists");
    assert!(
        !counterfactual.fill_outcomes.is_empty(),
        "fill_outcomes should be populated when fills are scored"
    );
    assert_eq!(
        counterfactual.fill_outcomes.len(),
        FILLS,
        "all fill scores should be journaled"
    );

    // Verify each fill outcome record has the expected structure
    for outcome in &counterfactual.fill_outcomes {
        assert!(
            !outcome.order_id.as_str().is_empty(),
            "each fill outcome must have an order_id"
        );
        assert!(
            !outcome.venue.is_empty(),
            "each fill outcome must have a venue"
        );
    }

    Ok(())
}

/// Per-horizon calibration metrics (Brier score and evaluation count) are
/// recorded with the horizon label for each resolved thesis.
///
/// This test prevents regression where per-horizon metrics are silently dropped
/// or recorded without the horizon discriminator, making per-horizon calibration
/// tracking invisible to operators.
#[test]
fn per_horizon_brier_scores_are_recorded_with_horizon_label() -> Result<()> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let _first = platform.run_cycle(start());

    // Premise: the first cycle produces claims; collect them for verification.
    assert!(
        !platform.predictions().is_empty(),
        "no claims were generated"
    );

    // Get the claim that will be resolved, and track its horizon for verification.
    let prediction = platform.predictions()[0].clone();
    let claim = prediction
        .claim
        .clone()
        .expect("a claim records its resolution time");
    let horizon = prediction.proposition.resolves_at.since(claim.formed_at);

    // Move time past the horizon with significant price movement so the verdict is
    // informative (not inconclusive due to quiet tape).
    let horizon_resolution = prediction
        .proposition
        .resolves_at
        .saturating_add(Duration::from_mins(1));
    let swings: Vec<SensedRecord> = (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            let at = horizon_resolution.saturating_sub(Duration::from_mins((20 - i) * 60));
            bar("AAA", at, open, close)
        })
        .collect();
    platform.observe(swings);

    // Run the cycle after the horizon passes, which should trigger grading.
    let _ = platform.run_cycle(horizon_resolution);

    // Verify per-horizon metrics are recorded with the horizon label.
    let snapshot = recorded(&platform);

    // The label is the horizon's fixed bucket, never the duration itself: a
    // label built from a runtime value minted one series per distinct
    // horizon (W0.8). The old form, `format!("{}s", secs)`, must no longer
    // be recorded at all.
    let horizon_label = qip_kernel::confidence::HorizonBucket::of(horizon)
        .as_str()
        .to_string();
    let unbounded = format!("{}s", horizon.as_secs_f64() as i64);
    assert!(
        snapshot
            .gauge(
                names::BELIEF_BRIER_SCORE_BY_HORIZON,
                &labels([("horizon", &unbounded)]),
            )
            .is_none(),
        "the per-horizon series is still labelled by a runtime duration ({unbounded})"
    );

    // Check that per-horizon Brier score is recorded.
    let brier = snapshot.gauge(
        names::BELIEF_BRIER_SCORE_BY_HORIZON,
        &labels([("horizon", &horizon_label)]),
    );
    assert!(
        brier.is_some(),
        "per-horizon Brier score not recorded for horizon label '{}'",
        horizon_label
    );

    // Check that per-horizon evaluation count is recorded.
    let eval_count = snapshot.gauge(
        names::BELIEF_EVALUATIONS_BY_HORIZON,
        &labels([("horizon", &horizon_label)]),
    );
    assert!(
        eval_count.is_some(),
        "per-horizon evaluation count not recorded for horizon label '{}'",
        horizon_label
    );

    Ok(())
}

// --- CAPITAL-036: shadow portfolios run counterfactual allocations -----------

#[test]
fn shadow_portfolios_hold_alternate_sizes_and_rejected_opportunities() -> Result<()> {
    use qip_contracts::action::{ShadowPortfolio, ShadowRecord};

    let mut portfolio = ShadowPortfolio::new("alloc-001".to_string());

    assert!(
        !portfolio.is_complete(),
        "a new portfolio with no shadows should not be complete"
    );
    assert_eq!(portfolio.size_alternatives.len(), 0);
    assert_eq!(portfolio.rejected_opportunities.len(), 0);

    portfolio.add_size_alternative(ShadowRecord::new(
        "opp-smaller".to_string(),
        "AAA".to_string(),
    ));

    assert!(
        !portfolio.is_complete(),
        "portfolio with only size alternatives should not be complete"
    );
    assert_eq!(portfolio.size_alternatives.len(), 1);

    portfolio.add_rejected_opportunity(ShadowRecord::new(
        "opp-declined".to_string(),
        "BBB".to_string(),
    ));

    assert!(
        portfolio.is_complete(),
        "portfolio with at least one of each shadow type should be complete"
    );
    assert_eq!(portfolio.size_alternatives.len(), 1);
    assert_eq!(portfolio.rejected_opportunities.len(), 1);

    Ok(())
}

#[test]
fn one_allocation_yields_at_least_one_alternate_size_and_one_rejected_opportunity_shadow()
-> Result<()> {
    use qip_contracts::action::{ShadowPortfolio, ShadowRecord};

    let mut platform = platform()?;
    platform.observe(bars("AAA", 90));

    let order_id = refuse_one(&mut platform, "prop-test", start())?;

    platform.observe(bars_after("AAA", start(), 5));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));

    let declined_scores = platform.declined_scores();
    assert!(
        !declined_scores.is_empty(),
        "at least one order should have been declined and scored"
    );

    for score in declined_scores {
        assert!(
            score.alternatives > 0,
            "each scored path should have generated alternatives"
        );
    }

    let mut portfolio = ShadowPortfolio::new(order_id.to_string());

    for score in declined_scores {
        if score.regret {
            portfolio.add_rejected_opportunity(ShadowRecord::new(
                score.order_id.to_string(),
                score.object_id.to_string(),
            ));
        }
        if score.alternatives > 1 {
            portfolio.add_size_alternative(ShadowRecord::new(
                score.order_id.to_string(),
                format!("alt-{}", score.object_id),
            ));
        }
    }

    assert!(
        portfolio.is_complete(),
        "a declined allocation should generate both size and opportunity shadows"
    );

    Ok(())
}

// --- admission: received before it happened (ADR 0132, R12-TICK-2053) -----

/// The centre holds every sensed record to the one admission rule by the
/// times the record carries: a tick captured before the instant it says it
/// happened was knowable before it was true, so it reaches no series and is
/// reported as a capture problem rather than absorbed. The same tick with
/// its capture at or after its instant is absorbed. Mutation: drop the
/// `Admission::check_order` arm in `Platform::absorb_records` and the
/// contaminated tick is absorbed, moving the close feature.
#[test]
fn a_tick_captured_before_it_happened_is_refused_at_admission_and_reported() -> Result<()> {
    let mut platform = platform()?;
    let at = start().saturating_add(Duration::from_secs(60));

    let absorbed = platform.observe(vec![late_tick("AAA", at, Duration::from_secs(-5), "123")]);
    assert_eq!(
        absorbed, 0,
        "a tick received before it happened was absorbed"
    );
    let report = platform.run_cycle(at);
    let problems: Vec<&String> = report
        .stages
        .iter()
        .flat_map(|stage| stage.problems.iter())
        .collect();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("received before it happened")),
        "the refusal was not reported as a capture problem: {problems:?}"
    );

    // The same tick, captured five seconds after it happened: absorbed.
    let later = at.saturating_add(Duration::from_secs(120));
    let absorbed = platform.observe(vec![late_tick("AAA", later, Duration::from_secs(5), "123")]);
    assert_eq!(absorbed, 1, "an ordinary late tick was refused");
    let report = platform.run_cycle(later.saturating_add(Duration::from_secs(10)));
    assert!(
        !report
            .stages
            .iter()
            .flat_map(|stage| stage.problems.iter())
            .any(|problem| problem.contains("received before it happened")),
        "an ordinary tick was reported as received before it happened"
    );
    Ok(())
}

// --- publication delay and the decision-time read (R12-TICK-2087) ----------

fn late_tick(symbol: &str, at: Timestamp, late_by: Duration, price: &str) -> SensedRecord {
    SensedRecord::Tick(Tick {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        at,
        capture_time: at.saturating_add(late_by),
        sequence: None,
        price: Decimal::parse(price).expect("a price"),
        volume: dec!("100"),
        quality: DataQuality::clean(),
    })
}

/// R12-TICK-2087, first half. A tick is knowable here only once the feed
/// delivered it, and a read at a decision instant between the two must not
/// see it: that is the feed-arrival delay, and reading through it is
/// point-in-time leakage of exactly that length. The failure this guards
/// happened: `capture_time` was optional until 2026-10-11 and the kernel
/// filed a record with none as knowable the moment it happened.
///
/// Mutation: in `Platform::observe`'s `SensedRecord::Tick` arm use
/// `tick.at` as `known_at` — the read at the decision instant then returns
/// the tick and the second assertion fails.
#[test]
fn a_read_at_decision_time_returns_only_records_with_known_at_at_or_before_it() -> Result<()> {
    let mut platform = platform()?;
    let happened = start();
    let late_by = Duration::from_mins(2);
    let decided = happened.saturating_add(Duration::from_mins(1));
    let arrived = happened.saturating_add(late_by);
    assert_eq!(
        platform.observe(vec![late_tick("AAA", happened, late_by, "101.5")]),
        1,
        "the premise: the tick was absorbed"
    );
    let world = platform.world();
    let features = world.features();
    let once_arrived = features
        .value_as_of("close", object("AAA").as_str(), arrived, arrived)
        .expect("the premise: once the tick has arrived, a read sees it");
    assert!((once_arrived.value - 101.5).abs() < f64::EPSILON);

    // Decided after the tick happened and before it arrived: not visible.
    assert!(
        features
            .value_as_of("close", object("AAA").as_str(), decided, decided)
            .is_none(),
        "a read at the decision instant saw a tick the feed had not yet delivered"
    );
    Ok(())
}

/// R12-TICK-2087, second half. The delay each source shows is calibrated
/// from what it delivered, and the SIMULATE stage's capacity probe runs
/// behind the slowest one — reported off the simulator's own applied
/// conditions, not off the schedule that was asked for.
///
/// Mutation: drop the `with_conditions` call in `capacity_probe` — the probe
/// reports no `delayed_feed` condition and the last assertion fails.
/// Mutation: drop `observe_publication_delay` from the `Tick` arm — the
/// calibration premise fails.
#[test]
fn a_sources_publication_delay_is_calibrated_from_its_ticks_and_the_capacity_probe_runs_behind_it()
-> Result<()> {
    let mut platform = platform()?;
    assert!(platform.publication_delays().is_empty());
    assert_eq!(platform.calibrated_feed_delay(), Duration::ZERO);
    platform.observe(bars("AAA", 120));
    for (minutes_ago, late_by) in [(30, 1), (20, 3), (10, 2)] {
        let at = start().saturating_sub(Duration::from_mins(minutes_ago));
        platform.observe(vec![late_tick(
            "AAA",
            at,
            Duration::from_mins(late_by),
            "100",
        )]);
    }
    let delay = platform
        .publication_delays()
        .get("XNYS")
        .expect("the premise: the source was calibrated");
    assert_eq!(delay.observations(), 3);
    assert_eq!(delay.mean(), Duration::from_mins(2));
    assert_eq!(delay.max(), Duration::from_mins(3));
    assert_eq!(platform.calibrated_feed_delay(), Duration::from_mins(2));

    let report = platform.run_cycle(start());
    let simulate = report.stage(Stage::Simulate).expect("simulate ran");
    assert!(
        simulate.detail.contains("capacity probe"),
        "the premise: the probe ran: {}",
        simulate.detail
    );
    assert!(
        simulate
            .detail
            .contains("behind a 120s calibrated feed delay"),
        "the probe did not run behind the calibrated delay: {}",
        simulate.detail
    );
    assert!(
        simulate.detail.contains("delayed_feed"),
        "the simulator did not apply the feed delay to the probe's arrival: {}",
        simulate.detail
    );
    Ok(())
}

// --- R12-RES-7104: a persistently wrong twin narrows the venue's capital ---

/// A platform whose only limit is the shipped misfit-venue cap, on a book
/// small enough that three 70,000 fills sit under it (21%) and a fourth
/// would not (28%), whatever else the shipped set says.
fn platform_under_the_misfit_cap() -> Result<Platform> {
    let shipped = LimitSet::conservative_default()
        .limits
        .into_iter()
        .find(|limit| limit.name == "twin-misfit-venue-gross")
        .expect("the shipped set carries the misfit-venue cap");
    let config = PlatformConfig::default().with_initial_equity(dec!("1000000"));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        LimitSet::new("misfit-only").with(shipped),
    )
}

const MISFIT_VENUE: &str = "simulated-venue";

fn teach_the_twin_it_is_wrong(platform: &mut Platform, observations: usize) {
    for _ in 0..observations {
        platform
            .fill_calibration_mut()
            .observe(MISFIT_VENUE, -300.0)
            .expect("a finite error is observed");
    }
}

fn fourth_buy(platform: &mut Platform) -> Result<()> {
    for n in 0..3 {
        fill_one(platform, &format!("prop-misfit-{n}"), start())?;
    }
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("700"),
        dec!("100"),
        "prop-misfit-3",
        vec!["hyp-prop-misfit-3".to_string()],
        start(),
    );
    platform.submit_order(order, start()).map(|_| ())
}

/// Mutation: make `twin_misfit` always false, delete the axis insertion in
/// the submit path, or delete `with_twin_misfit` from `risk_state_from`.
#[test]
fn a_venue_whose_twin_is_persistently_wrong_is_narrowed_to_a_quarter_of_equity() -> Result<()> {
    let mut platform = platform_under_the_misfit_cap()?;
    teach_the_twin_it_is_wrong(&mut platform, 10);
    assert!(
        platform.twin_misfit(MISFIT_VENUE),
        "premise: the twin is wrong"
    );
    let error = fourth_buy(&mut platform)
        .expect_err("a fourth fill took a misfit venue past a quarter of equity");
    assert!(
        error.message().contains("twin-misfit-venue-gross"),
        "refused for another reason: {}",
        error.message()
    );
    // Narrowing is not withdrawal: the venue stays in service.
    assert!(platform.withdrawn_venues().is_empty());
    Ok(())
}

#[test]
fn the_same_book_at_a_venue_the_twin_has_not_mispriced_is_admitted() -> Result<()> {
    let mut platform = platform_under_the_misfit_cap()?;
    assert!(
        !platform.twin_misfit(MISFIT_VENUE),
        "premise: no error observed"
    );
    fourth_buy(&mut platform)?;
    // And too few observations are not a trend, however large the error.
    teach_the_twin_it_is_wrong(&mut platform, 7);
    assert!(!platform.twin_misfit(MISFIT_VENUE));
    fill_one(&mut platform, "prop-misfit-5", start())?;
    Ok(())
}

// --- R12-RISK-4096: a short with no known borrow is refused ----------------

fn sell(platform: &mut Platform, quantity: Decimal, tag: &str, at: Timestamp) -> Result<()> {
    let order = platform.order_from(
        object("AAA"),
        Side::Sell,
        quantity,
        dec!("100"),
        tag,
        vec![format!("hyp-{tag}")],
        at,
    );
    platform.submit_order(order, at).map(|_| ())
}

/// Mutation: make `short_without_borrow` return `None` when no reading
/// exists, or compare `available` with `>` instead of `<`.
#[test]
fn a_short_nobody_has_said_is_borrowable_is_refused_and_a_covered_one_is_admitted() -> Result<()> {
    let mut platform = platform()?;
    // Premise: selling what the book holds is not a short and needs no
    // borrow, so the refusals below are the borrow rule's.
    fill_one(&mut platform, "prop-borrow-long", start())?;
    sell(&mut platform, dec!("100"), "prop-borrow-close", start())?;

    let error = sell(&mut platform, dec!("2000"), "prop-borrow-none", start())
        .expect_err("a short with no borrow reading was admitted");
    assert!(
        error.message().contains("no borrow availability"),
        "{}",
        error.message()
    );

    // A reading smaller than the shorted quantity is not enough.
    platform.observe_borrow("obj-AAA", dec!("99"), start(), start())?;
    let error = sell(&mut platform, dec!("2000"), "prop-borrow-small", start())
        .expect_err("a short beyond the available borrow was admitted");
    assert!(error.message().contains("only 99"), "{}", error.message());

    // A stale reading is no reading.
    let later = start().saturating_add(Duration::from_days(3));
    let error = sell(&mut platform, dec!("2000"), "prop-borrow-stale", later)
        .expect_err("a short against a stale reading was admitted");
    assert!(
        error.message().contains("older than"),
        "{}",
        error.message()
    );

    // A fresh reading that covers it admits.
    platform.observe_borrow("obj-AAA", dec!("10000"), later, later)?;
    sell(&mut platform, dec!("2000"), "prop-borrow-covered", later)?;
    Ok(())
}

#[test]
fn borrow_availability_that_is_negative_or_from_the_future_is_refused() -> Result<()> {
    let mut platform = platform()?;
    assert!(
        platform
            .observe_borrow("obj-AAA", dec!("-1"), start(), start())
            .is_err()
    );
    assert!(
        platform
            .observe_borrow(
                "obj-AAA",
                dec!("1"),
                start().saturating_add(Duration::from_days(1)),
                start()
            )
            .is_err()
    );
    Ok(())
}

/// R12-WORLD-1156, "hypothesis generation": on the default composition
/// (`Platform::new` with `PlatformConfig::default()`), a sensed tape whose
/// last bars jump leaves REASON with a hypothesis recorded as a falsifiable
/// claim. That is a hypothesis id, a proposition that resolves after the
/// cycle, and the confidence it was made at. A cycle with nothing sensed forms
/// none, which is the premise that the tape and not the composition made it.
///
/// Mutation: in `Platform::keep_prediction`, drop the push -- the sensed cycle
/// then records no hypothesis and the assertion fails.
#[test]
fn reason_forms_a_hypothesis_from_a_sensed_anomaly_on_the_default_composition() -> Result<()> {
    let mut quiet = platform()?;
    quiet.run_cycle(start());
    assert!(
        quiet.predictions().is_empty(),
        "premise: a cycle with nothing sensed formed a hypothesis"
    );

    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());
    assert!(report.stage(Stage::Reason).is_some(), "REASON ran");
    let formed = platform.predictions();
    assert!(
        !formed.is_empty(),
        "REASON formed no hypothesis from a sensed anomaly:\n{}",
        report.summarise()
    );
    for prediction in formed {
        assert!(
            !prediction.hypothesis.trim().is_empty(),
            "an unnamed hypothesis"
        );
        assert!(
            prediction.proposition.resolves_at > start(),
            "a claim already resolved"
        );
        let claim = prediction
            .claim
            .as_ref()
            .expect("a hypothesis is recorded with the confidence it was made at");
        assert!((0.0..=1.0).contains(&claim.confidence));
    }
    Ok(())
}

/// R12-WORLD-1156, "uncertainty": a resolved prediction updates the
/// calibration report the platform measures its own confidence by. Before
/// anything resolves there is no report; once the claim's horizon passes on a
/// move large enough to grade it, the report exists, counts the evaluation,
/// and carries a Brier score in `[0, 1]`.
///
/// Mutation: in `Platform::learn_from`, which `calibrate_resolved` reaches,
/// never store the report (`last_calibration`) -- the platform then reports
/// none after the resolution.
#[test]
fn a_resolved_prediction_updates_the_calibration_report() -> Result<()> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());
    let horizon = platform
        .predictions()
        .first()
        .map(|p| p.proposition.resolves_at)
        .expect("premise: the first cycle made a claim");
    assert!(
        platform.calibration().is_none(),
        "premise: nothing calibrated yet"
    );

    let swings: Vec<SensedRecord> = (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            bar(
                "AAA",
                horizon.saturating_sub(Duration::from_mins((20 - i) * 60)),
                open,
                close,
            )
        })
        .collect();
    platform.observe(swings);
    platform.run_cycle(horizon.saturating_add(Duration::from_mins(1)));
    let calibration = platform
        .calibration()
        .expect("a resolved prediction updates the calibration report");
    assert!(calibration.evaluated >= 1, "{calibration:?}");
    assert!(
        (0.0..=1.0).contains(&calibration.brier_score),
        "{calibration:?}"
    );
    Ok(())
}

// --- the historical universe (R12-TICK-2090) --------------------------------

fn members_as_of(platform: &Platform, at: Timestamp) -> Vec<String> {
    platform
        .universe_history()
        .members_as_of(at)
        .iter()
        .map(|listing| listing.object_id.as_str().to_string())
        .collect()
}

/// R12-TICK-2090: the platform keeps the universe that existed. A delisting
/// announced on the tape ends the name's membership at its ex-date and not
/// before, so a backtest as of a date before the ex-date still holds it; a
/// renaming opens a new symbol era at its ex-date; and a venue the platform
/// withdraws on feasibility evidence is an outage in the history from the
/// withdrawal on, which is what a backtest across that window reports.
///
/// Mutations: drop the `delist` call in `queue_corporate_action` and the
/// membership after the ex-date still names BBB; drop the `open_outage` call
/// in `withdraw_venue` and the outage query comes back empty.
#[test]
fn a_delisting_on_the_tape_ends_membership_at_its_ex_date_and_a_withdrawn_venue_is_an_outage()
-> Result<()> {
    let mut platform = platform()?;
    let ex = start().saturating_add(Duration::from_days(3));
    assert_eq!(
        members_as_of(&platform, start()),
        vec!["obj-AAA".to_string(), "obj-BBB".to_string()],
        "the premise: every assembled instrument is listed from the assembly instant"
    );

    let absorbed = platform.observe(vec![
        SensedRecord::CorporateAction(Box::new(CorporateAction {
            object_id: object("BBB"),
            ex_date: ex,
            record_date: None,
            payment_date: None,
            kind: CorporateActionKind::Delisting {
                reason: "the product failed".to_string(),
            },
            announced_at: start(),
        })),
        SensedRecord::CorporateAction(Box::new(CorporateAction {
            object_id: object("AAA"),
            ex_date: ex,
            record_date: None,
            payment_date: None,
            kind: CorporateActionKind::Renamed {
                new_symbol: "AAX".to_string(),
            },
            announced_at: start(),
        })),
    ]);
    assert_eq!(absorbed, 2, "the premise: both actions were absorbed");
    assert_eq!(
        members_as_of(&platform, ex.saturating_sub(Duration::from_secs(1))),
        vec!["obj-AAA".to_string(), "obj-BBB".to_string()],
        "a delisting announced early must not end membership before its ex-date"
    );
    assert_eq!(
        members_as_of(&platform, ex),
        vec!["obj-AAA".to_string()],
        "the delisted name is still a member at its ex-date"
    );
    assert_eq!(
        platform
            .universe_history()
            .resolve("AAX", ex)
            .map(|id| id.as_str().to_string()),
        Some("obj-AAA".to_string()),
        "the new symbol does not resolve from its ex-date"
    );
    assert!(
        platform
            .universe_history()
            .resolve("AAX", start())
            .is_none(),
        "the new symbol resolved before it existed"
    );

    // The venue outage: nothing is out until the desk venue is withdrawn.
    assert!(
        platform
            .universe_history()
            .outages_overlapping("simulated-venue", start(), Timestamp::MAX)
            .is_empty(),
        "the premise: no outage before the withdrawal"
    );
    withdraw_the_desk_venue(&mut platform)?;
    let outages = platform.universe_history().outages_overlapping(
        "simulated-venue",
        start(),
        start().saturating_add(Duration::from_secs(1)),
    );
    assert_eq!(
        outages.len(),
        1,
        "the withdrawal did not open an outage: {outages:?}"
    );
    assert_eq!(outages[0].from, start());
    assert_eq!(
        outages[0].until,
        Timestamp::MAX,
        "an outage nobody has reinstated is open"
    );
    assert!(
        outages[0].reason.contains("feasibility evidence"),
        "{}",
        outages[0].reason
    );
    Ok(())
}

// --- the fill's participation reaches the twin's calibration (R12-TICK-2080)

/// R12-TICK-2080: the LEARN stage hands the twin's calibration each scored
/// fill's participation in its bar's volume beside the fill error, so the
/// twin can tell an impact misfit from a spread misfit; without it every
/// error was charged to the spread. Mutation: pass `None` for the
/// participation in `score_filled` — the calibration counts no fill with a
/// size and the assertion fails.
#[test]
fn a_scored_fill_reaches_the_twins_calibration_with_its_participation() -> Result<()> {
    let mut platform = platform()?;
    let mut tape = quiet_bars("AAA", 90);
    let last = tape.pop().expect("ninety bars");
    let last_at = match &last {
        SensedRecord::Bar(bar) => bar.open_time,
        _ => unreachable!("quiet_bars produces bars"),
    };
    tape.push(bar("AAA", last_at, 90.0, 90.0));
    platform.observe(tape);
    fill_one(&mut platform, "prop-participation", start())?;
    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));

    let scores = platform.fill_scores();
    assert_eq!(scores.len(), 1, "premise: the fill was priced");
    assert!(
        scores[0].trade_error_bps.is_some(),
        "premise: the twin priced an entry to compare the fill with"
    );
    let venue = platform
        .fill_calibration()
        .venue(&scores[0].venue)
        .expect("the venue was calibrated from the fill");
    assert_eq!(venue.observations, 1);
    assert_eq!(
        venue.participation_observations, 1,
        "the fill reached the calibration without its participation"
    );
    Ok(())
}

// --- measured decision latency reaches the capacity probe (R12-TICK-2074) --

/// The shortfall against the decision-time mark the SIMULATE stage's
/// capacity probe reported, in basis points, and the latency it arrived
/// after, in seconds, read off the stage detail.
fn probe_shortfall(detail: &str) -> (f64, f64) {
    let after = detail
        .split(", arriving ")
        .nth(1)
        .expect("the probe reported its arrival");
    let seconds = after
        .split("s after the decision at ")
        .next()
        .and_then(|figure| figure.trim().parse().ok())
        .expect("the latency is a number");
    let bps = after
        .split("s after the decision at ")
        .nth(1)
        .and_then(|rest| rest.split("bps").next())
        .and_then(|figure| figure.trim().parse().ok())
        .expect("the shortfall is a number");
    (seconds, bps)
}

/// R12-TICK-2074. The platform's own decision latency — SENSE through ACT,
/// measured every cycle — is a condition the SIMULATE stage's capacity
/// probe runs under, so a simulated execution arrives as late as this
/// platform's decisions do. Two platforms on the same tape and seed differ
/// only in the latency they have measured: the slow one's probe reports the
/// simulator applied a latency, and its execution costs more.
///
/// Mutation: in `capacity_probe`, calibrate from an empty sample set — no
/// latency condition is applied, the detail does not name one and the two
/// probes cost the same.
///
/// On a tape that moves against the order. A measured latency on a random
/// walk can as easily land on a better price as a worse one — the first
/// version of this test, on the noisy tape the other tests use, saw the
/// slow probe fill 22bps better — so the property is asserted where the
/// requirement states it: a later execution pays the move it waited through.
#[test]
fn a_measured_decision_latency_pushes_the_simulated_execution_later_and_worsens_its_fill()
-> Result<()> {
    let run = |latencies: &[Duration]| -> Result<String> {
        let mut platform = platform()?;
        // A tape rising one percent a bar and nothing else: the probe's
        // drift is the trend and its volatility the floor, so a buy that
        // arrives later pays the move it waited through and no draw of
        // noise can hand it a better price.
        let rising: Vec<SensedRecord> = (0..120)
            .map(|i| {
                let open = 100.0 * 1.01_f64.powi(i);
                let at = start().saturating_sub(Duration::from_days(120 - i64::from(i)));
                bar("AAA", at, open, open * 1.01)
            })
            .collect();
        platform.observe(rising);
        for latency in latencies {
            platform.observe_decision_latency(*latency);
        }
        let report = platform.run_cycle(start());
        let simulate = report.stage(Stage::Simulate).expect("simulate ran");
        assert!(
            simulate.detail.contains("capacity probe"),
            "the premise: the probe ran: {}",
            simulate.detail
        );
        Ok(simulate.detail.clone())
    };
    let prompt = run(&[])?;
    let slow = run(&[
        Duration::from_secs(60),
        Duration::from_secs(90),
        Duration::from_secs(120),
    ])?;
    assert!(
        !prompt.contains("latency"),
        "premise: a platform with no measured latency probes without one: {prompt}"
    );
    assert!(
        slow.contains("latency"),
        "the simulator did not apply the measured decision latency: {slow}"
    );
    let ((prompt_secs, prompt_bps), (slow_secs, slow_bps)) =
        (probe_shortfall(&prompt), probe_shortfall(&slow));
    assert!(
        prompt_secs < 1.0 && slow_secs >= 60.0,
        "the measured latency pushed the execution later: {prompt_secs}s against {slow_secs}s"
    );
    assert!(
        slow_bps > prompt_bps,
        "a slower decision did not worsen the fill: {slow_bps} bps against {prompt_bps} bps"
    );
    Ok(())
}
