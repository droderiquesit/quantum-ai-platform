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
use qip_kernel::series;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{Snapshot, labels};
use qip_risk::limits::{Limit, LimitKind, LimitSet};

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
        snapshot.counter_total(series::THESES_EVALUATED),
        0,
        "a claim was graded before its horizon passed"
    );
    assert!(
        snapshot
            .gauge(series::BELIEF_BRIER_SCORE, &labels([]))
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
        snapshot.counter_total(series::THESES_EVALUATED) >= 1,
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
        snapshot.gauge(series::BELIEF_BRIER_SCORE, &labels([])),
        Some(calibration.brier_score),
        "the gauge must be the calibration's own Brier score"
    );
    assert_eq!(
        snapshot.gauge(series::BELIEF_CONFIDENCE_ADJUSTMENT, &labels([])),
        Some(calibration.confidence_adjustment)
    );
    assert_eq!(
        snapshot.gauge(series::BELIEF_EVALUATIONS, &labels([])),
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
        recorded(&platform).counter_total(series::COUNTERFACTUALS_SCORED),
        0
    );

    // Before the horizon has passed, nothing is priced: the twin marks the
    // alternative at the horizon, and a path scored on the part of the tape
    // that had happened to print would be a track record manufactured from
    // whatever suited it.
    let early = platform.run_cycle(start());
    assert_eq!(
        recorded(&platform).counter_total(series::COUNTERFACTUALS_SCORED),
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
            series::COUNTERFACTUALS_SCORED,
            &labels([("gate", "order-validation")])
        ),
        1,
        "the score is not charged to the gate that refused: {:?}",
        snapshot
            .series
            .iter()
            .filter(|s| s.name == series::COUNTERFACTUALS_SCORED)
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
        series::COUNTERFACTUAL_REGRETS,
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
    assert_eq!(snapshot.counter_total(series::COUNTERFACTUALS_SCORED), 8);
    assert_eq!(
        snapshot.counter_total(series::COUNTERFACTUALS_DEFERRED),
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
    assert_eq!(snapshot.counter_total(series::COUNTERFACTUALS_SCORED), 9);
    assert_eq!(snapshot.counter_total(series::COUNTERFACTUALS_DEFERRED), 1);
    assert_eq!(platform.declined_awaiting_score(), 0);
    assert_eq!(
        snapshot.counter_total(series::COUNTERFACTUALS_UNSCORED),
        0,
        "nothing was refused by the twin, so nothing may be counted as unscorable"
    );
    Ok(())
}
