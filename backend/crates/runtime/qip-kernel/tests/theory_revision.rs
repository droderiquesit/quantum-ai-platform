//! R12-EXPAND-6044: a thesis the symbolic rule pack refused that LEARN then
//! credits leaves a theory delta on the log, and nothing moves a rule.
//!
//! The failure this prevents: the pack vetoed a review-approved hypothesis
//! on a hard stop, the hypothesis still became a claim, the claim came true,
//! and the platform held both facts and recorded no contradiction between
//! them — its theory of what may be acted on was never held against its own
//! evidence. Both tests drive the real cycle: a veto in REASON, the horizon
//! closing, the grade in LEARN. The control run differs in one number, the
//! lot size, so a legless, delta-less book is the veto's doing and not the
//! tape's.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_kernel::symbolic_stage;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// `valuation_seam.rs`'s review floor, for the same reason it states: the
/// seam under test is reached only through a thesis review approved, and
/// the shipped `ReviewPolicy::default()` requires 0.50 on a tape no synthetic
/// series here clears. Not the shipped value; the test is a statement about
/// the seam's arithmetic, and the row stays graded accordingly.
const REVIEW_FLOOR: f64 = 0.10;

/// One listed name with one knob: its lot size. A lot of one is sizeable; a
/// lot of a million shares at 100 is a hundred million against a million of
/// cash, which is the pack's `cash_short` stop (and its position-budget stop
/// beside it).
fn universe(lot: Decimal) -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            object("AAA"),
            "AAA",
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .geography("US")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .lot_size(lot)
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

fn platform(lot: Decimal) -> Result<Platform> {
    let config = PlatformConfig {
        initial_equity: dec!("1000000"),
        review: ReviewPolicy {
            minimum_surviving_confidence: REVIEW_FLOOR,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    };
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let limits = LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    );
    Platform::new(config, context, Telemetry::silent(), universe(lot)?, limits)
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

const TAPE: usize = 120;
const DROP: f64 = -0.09;

/// A quiet series that drops nine percent on its final bar, all of it closing
/// at or before `start()`: the return detector reads an `Undervalued` claim
/// off the drop, which is a long the mandate permits.
fn tape(symbol: &str) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let drop = if i == TAPE - 1 { DROP } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// Daily closes back at the pre-drop level from the day after `start()`, so
/// the undervalued claim's reversion comes true at about the magnitude it
/// predicted and the grader credits it.
fn recovered(symbol: &str, days: i64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            bar(
                symbol,
                start().saturating_add(Duration::from_days(day)),
                100.0,
                100.0,
            )
        })
        .collect()
}

/// Form a claim at `start()`, then settle it on a recovered tape. Returns the
/// claim's hypothesis id, the REASON problems of the forming cycle and the
/// LEARN detail of the settling cycle.
fn form_and_settle(platform: &mut Platform) -> (String, Vec<String>, String) {
    platform.observe(tape("AAA"));
    let first = platform.run_cycle(start());
    let reason = first.stage(Stage::Reason).expect("reason ran");
    assert!(
        reason.detail.contains("hypothesis approved at confidence ")
            || reason.detail.contains("hypothesis rejected at confidence "),
        "premise: a hypothesis was formed and reviewed: {}",
        reason.detail
    );
    assert!(
        reason.detail.contains("falsifiable ("),
        "premise: the hypothesis was written as a claim: {}",
        reason.detail
    );
    let prediction = platform
        .predictions()
        .first()
        .expect("premise: a claim was recorded");
    let hypothesis_id = prediction
        .claim
        .as_ref()
        .expect("a graded claim")
        .hypothesis_id
        .clone();
    let resolves_at = prediction.proposition.resolves_at;
    let days = resolves_at.since(start()).as_nanos() / Duration::from_days(1).as_nanos() + 2;
    platform.observe(recovered("AAA", days));
    let settle = resolves_at.saturating_add(Duration::from_hours(1));
    let report = platform.run_cycle(settle);
    let learn = report.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("1 thesis(es) resolved, 1 graded"),
        "premise: the horizon passed and the thesis was graded: {}",
        learn.detail
    );
    (hypothesis_id, reason.problems.clone(), learn.detail.clone())
}

/// Mutation: delete the `revise_theory` call in `calibrate_resolved` — the
/// pending note is never read, `theory_deltas()` stays empty and the
/// `len() == 1` assertion fails; or make `revise_theory` skip the
/// `counts_as_correct` test — the control run below produces no delta
/// either way, but the credited verdict assertion names what the delta must
/// rest on.
#[test]
fn a_veto_the_outcome_contradicts_leaves_a_theory_delta_and_nothing_moves_a_rule() -> Result<()> {
    // Control: a sizeable lot. Review approves, the pack finds it eligible,
    // the claim comes true, and there is no contradiction to record.
    let mut controlled = platform(dec!("1"))?;
    assert!(
        controlled.proposals().is_empty(),
        "premise: no proposal before the first cycle"
    );
    let (_, problems, detail) = form_and_settle(&mut controlled);
    assert!(
        problems.iter().all(|p| !p.contains("vetoed")),
        "premise: nothing is vetoed when the pack has no hard stop: {problems:?}"
    );
    assert!(
        controlled.theory_deltas()?.is_empty(),
        "a thesis the pack never refused left a theory delta: {detail}"
    );
    assert!(
        !detail.contains("theory delta(s) proposed"),
        "the control's LEARN line counts a delta: {detail}"
    );

    // Vetoed: the same tape, a million-share lot.
    let mut vetoed = platform(dec!("1000000"))?;
    let (hypothesis_id, problems, detail) = form_and_settle(&mut vetoed);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("vetoed by the symbolic rule pack")
                && p.contains("cash is short of one lot")),
        "premise: the pack vetoed the thesis on its cash stop: {problems:?}"
    );
    let evaluation = vetoed
        .evaluations()
        .iter()
        .find(|e| e.hypothesis_id == hypothesis_id)
        .expect("premise: the vetoed thesis was graded");
    assert!(
        evaluation.verdict.counts_as_correct(),
        "premise: the grader credited the thesis the pack refused: {:?} — {}",
        evaluation.verdict,
        evaluation.rationale
    );

    let deltas = vetoed.theory_deltas()?;
    assert_eq!(
        deltas.len(),
        1,
        "one theory delta for the one contradicted veto: {deltas:?}; LEARN said: {detail}"
    );
    let delta = &deltas[0];
    delta.validate()?;
    assert_eq!(delta.hypothesis_id, hypothesis_id);
    let pack = symbolic_stage::rule_pack();
    assert_eq!(
        delta.pack, pack.name,
        "the delta names the pack that refused"
    );
    assert_eq!(delta.version, pack.version);
    assert!(
        delta.rules.iter().any(|r| r == "r61-cash-short-of-one-lot"),
        "the delta names the rule whose firing made the stop: {:?}",
        delta.rules
    );
    assert!(
        delta
            .rules
            .iter()
            .all(|r| pack.rules.iter().any(|rule| &rule.id == r)
                || pack.invariants.iter().any(|i| &i.id == r)),
        "every rule the delta names is in the pack: {:?}",
        delta.rules
    );
    assert!(
        delta
            .contradicting_evidence
            .iter()
            .any(|e| e == &format!("hypothesis:{hypothesis_id}")),
        "{:?}",
        delta.contradicting_evidence
    );
    assert!(
        delta
            .contradicting_evidence
            .iter()
            .any(|e| e.starts_with("verdict:")),
        "the delta cites the grade it rests on: {:?}",
        delta.contradicting_evidence
    );
    assert!(
        detail.contains("1 theory delta(s) proposed, 0 veto(es) upheld"),
        "LEARN did not report the delta: {detail}"
    );
    assert_eq!(
        vetoed.symbolic_vetoes_pending().count(),
        0,
        "the note was read and closed"
    );
    // The proposal moved nothing: the pack a fresh platform derives under is
    // the build's own, byte for byte, and a second platform built now
    // derives under the same one.
    assert_eq!(
        symbolic_stage::rule_pack(),
        pack,
        "the rule pack is fixed at build time and a delta is a proposal only"
    );
    assert!(
        vetoed.proposals().iter().all(|p| p.legs.is_empty()),
        "a vetoed thesis still produced a proposed leg"
    );
    Ok(())
}
