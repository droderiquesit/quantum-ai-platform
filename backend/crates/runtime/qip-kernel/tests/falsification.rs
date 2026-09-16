//! Blueprint §14.3's `Testing` gate, as the LEARN stage actually runs it.
//!
//! The gap these tests close is a caller, not a capability: `record_prediction`
//! wrote a thesis's falsifiers down at formation and every construction of
//! `ThesisOutcome::falsifiers_triggered` in the kernel built it as
//! `Vec::new()`, so no falsifier this platform has ever stated has been
//! evaluated against anything. Each test here drives a real cycle through the
//! seam, so deleting the call site in `stage_learn` fails a test rather than
//! leaving a gate that reads as passed because it never ran.
//!
//! The property that matters most is the middle test: a claim is tested only
//! against closes that became **knowable** after it was formed. The platform
//! ingests a hundred and twenty bars before the claim exists, and not one of
//! them may reach the falsifier.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

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
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_prediction::resolution::{Comparison, ResolutionCriteria};
use qip_risk::limits::{Limit, LimitKind, LimitSet};

// --- fixtures ---------------------------------------------------------------

/// The liquidity every fixture states, because nothing states it for them:
/// `LiquidityProfile` has no `Default`, and a fixture may state its own
/// premise but not inherit one nobody wrote down.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

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
/// real to find. Every bar closes at or before `start()`, which is what makes
/// all of it in-sample for a claim formed at `start()`.
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

/// The stage detail the LEARN stage produced for a cycle at `now`.
fn learn_detail(platform: &mut Platform, now: Timestamp) -> String {
    learn_outcome(platform, now).0
}

/// The LEARN stage's detail and its problems.
fn learn_outcome(platform: &mut Platform, now: Timestamp) -> (String, Vec<String>) {
    let report = platform.run_cycle(now);
    report
        .stage(Stage::Learn)
        .map(|outcome| (outcome.detail.clone(), outcome.problems.clone()))
        .unwrap_or_default()
}

/// What the platform's first recorded claim actually claims.
struct FirstClaim {
    hypothesis_id: String,
    metric: String,
    comparison: Comparison,
    reference: Decimal,
    expected_move_bps: f64,
    class: String,
    /// The prose the hypothesis's author stated as what would refute it. The
    /// mirror falsifier carries it into the refutation register, so it is
    /// what a settlement's rationale must name.
    stated_falsifiers: Vec<String>,
    resolves_at: Timestamp,
}

fn first_claim(platform: &Platform) -> Option<FirstClaim> {
    let prediction = platform.predictions().first()?;
    let claim = prediction.claim.as_ref()?;
    match &prediction.proposition.criteria {
        ResolutionCriteria::Threshold {
            metric,
            comparison,
            value,
        } => Some(FirstClaim {
            hypothesis_id: claim.hypothesis_id.clone(),
            metric: metric.clone(),
            comparison: *comparison,
            reference: *value,
            expected_move_bps: claim.expected_move_bps,
            class: claim.class.clone(),
            stated_falsifiers: claim.falsifiers.clone(),
            resolves_at: prediction.proposition.resolves_at,
        }),
        _ => None,
    }
}

/// Hourly bars, so a test can put twenty-five held-out closes inside a
/// twenty-day horizon. A claim whose horizon passes is settled by
/// `calibrate_resolved` before the falsification pass sees it, and a claim
/// that is no longer open is not a claim under test.
fn hourly(symbol: &str, count: i64, level: impl Fn(i64) -> f64) -> Vec<SensedRecord> {
    (1..=count)
        .map(|i| {
            let at = start().saturating_add(Duration::from_hours(i));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Hour,
                open_time: at,
                open: Decimal::from_f64(level(i)).expect("a price"),
                high: Decimal::from_f64(level(i) * 1.002).expect("a price"),
                low: Decimal::from_f64(level(i) * 0.998).expect("a price"),
                close: Decimal::from_f64(level(i)).expect("a price"),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64(level(i)),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

// --- the seam ---------------------------------------------------------------

#[test]
fn the_learn_stage_reports_a_falsification_pass_on_every_cycle_including_an_idle_one() -> Result<()>
{
    // The silent-when-idle failure, refused. A platform whose claims have not
    // reached a second cycle is the only state a fresh deployment is ever in,
    // and a gate that said nothing there would be indistinguishable from one
    // nobody wired — which is the exact defect this whole seam exists to end.
    let mut platform = platform()?;

    // Premise: the very first cycle has no claim at all, so this is genuinely
    // the idle case and not a pass that happened to find nothing.
    assert!(
        platform.predictions().is_empty(),
        "the platform began with a claim on its books"
    );
    let idle = learn_detail(&mut platform, start());
    assert!(
        idle.contains("falsification: 0 open claim(s)"),
        "a cycle with nothing to test said nothing about it: {idle}"
    );
    assert!(
        idle.contains("sources:"),
        "the §14.2 source register was not reported: {idle}"
    );
    let pass = platform
        .falsification()
        .last_pass()
        .expect("an idle cycle still records a pass");
    assert_eq!(pass.open_claims, 0, "the idle pass invented a claim");
    assert_eq!(pass.tested, 0, "the idle pass tested something");
    Ok(())
}

#[test]
fn a_claim_is_never_tested_against_closes_that_were_knowable_before_it_was_formed() -> Result<()> {
    // The point-in-time property, at the seam where it would actually be
    // violated. The platform absorbs a hundred and twenty bars, every one of
    // which closed at or before `start()`, and then forms a claim at
    // `start()`. Days later, with no new data, the falsifier must have
    // nothing to read: a sample assembled by valid time, or by no time at
    // all, would hand it the whole series the claim was built on.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());

    // Premise: a claim exists, and the store holds the history it was built
    // from. Without both, "nothing was tested" proves nothing.
    assert!(
        !platform.predictions().is_empty(),
        "no claim was written, so there is nothing whose boundary could leak:\n{}",
        first.summarise()
    );
    let claim = first_claim(&platform).expect("the claim was recorded against a threshold");
    let subject = claim
        .metric
        .split_once(':')
        .map(|(_, subject)| subject.to_string())
        .expect("the metric names observable and subject");
    let world = platform.world();
    let retained = world.features().history("close", &subject, start()).len();
    drop(world);
    assert!(
        retained > 100,
        "the world model holds {retained} close(s) for {subject}; the test needs a real \
         in-sample history for the boundary to have anything to withhold"
    );

    let later = start().saturating_add(Duration::from_days(5));
    assert!(
        later < claim.resolves_at,
        "the claim would already have been settled by the calibration pass, so the \
         falsification pass would not see it as open"
    );
    let (detail, problems) = learn_outcome(&mut platform, later);

    // The ledger refuses a sample carrying a record the claim could already
    // see, rather than filtering it — so a pass that assembled its sample
    // from the unpartitioned history reports a problem here instead of a
    // verdict. This is the assertion that fires when the in-sample history
    // reaches the evaluation by any route, including one that derives a
    // statistic from it first.
    let leaked: Vec<&String> = problems
        .iter()
        .filter(|problem| problem.contains("falsifier") || problem.contains("held-out"))
        .collect();
    assert!(
        leaked.is_empty(),
        "the falsification pass refused its own sample, which means data the claim could \
         already see reached the ledger: {leaked:?}"
    );
    let pass = platform
        .falsification()
        .last_pass()
        .expect("the LEARN stage recorded a pass");
    assert!(
        pass.tested >= 1,
        "no falsifier was evaluated at all, so the call site in stage_learn is not reached: \
         {detail}"
    );
    assert_eq!(
        pass.refuted, 0,
        "a claim was refuted on records it could already see when it was formed: {detail}"
    );
    assert_eq!(
        pass.survived, 0,
        "a claim cleared the Testing gate on in-sample records: {detail}"
    );
    assert!(
        pass.leakage.in_sample > 100,
        "only {} record(s) were withheld as in-sample out of {retained} held; the boundary is \
         reading something other than the knowable instant",
        pass.leakage.in_sample
    );
    assert!(
        platform.falsification().ledger().spent(&claim.class) >= 1,
        "the falsifier for family {} was never charged a trial",
        claim.class
    );
    Ok(())
}

#[test]
fn a_statistic_derived_only_from_closes_knowable_after_a_claim_was_formed_can_refute_it()
-> Result<()> {
    // The other half of the same property: the boundary must admit genuinely
    // new data, or it is a gate that refuses everything and proves nothing.
    //
    // This is also the test that proves the whole path is live. The platform's
    // detectors raise a volatility-shift claim, so the falsifier is evaluated
    // against realised volatility derived from held-out closes — every window
    // built entirely from records that became knowable after the claim was
    // written, which is the only construction under which a rolling statistic
    // is held out at all.
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());
    assert!(
        !platform.predictions().is_empty(),
        "no claim was written:\n{}",
        first.summarise()
    );
    let claim = first_claim(&platform).expect("the claim was recorded against a threshold");
    let observable = claim
        .metric
        .split_once(':')
        .map(|(observable, _)| observable.to_string())
        .expect("the metric names observable and subject");

    // Premise: this fixture's claim is the volatility one, and its mirror
    // points upward — the claim expects volatility to fall, so it is
    // contradicted by volatility rising. A fixture whose claim pointed the
    // other way would need the opposite tape and this test would be asserting
    // nothing.
    assert_eq!(
        observable, "volatility",
        "this fixture's claim is about {observable}; the tape below moves realised volatility"
    );
    assert!(
        matches!(claim.comparison, Comparison::LessThan | Comparison::AtMost),
        "the claim compares {:?}, so its mirror does not point upward and the tape below would \
         not contradict it",
        claim.comparison
    );
    assert!(
        claim.reference.is_positive(),
        "a claim against a non-positive reference has no magnitude to mirror"
    );
    assert!(
        claim.expected_move_bps.abs() > 0.0,
        "the claim states no magnitude, so its mirror has no level and nothing can contradict it"
    );

    // Thirty hourly closes alternating between 100 and 110, every one of them
    // knowable after the claim was formed. Twenty-one closes make one
    // volatility observation, so thirty make ten — more than the five the
    // gate asks for, and each window's standard deviation of log returns is
    // about 0.095 against a reference under 0.01.
    platform.observe(hourly(
        "AAA",
        30,
        |i| if i % 2 == 0 { 100.0 } else { 110.0 },
    ));

    let later = start().saturating_add(Duration::from_days(5));
    assert!(
        later < claim.resolves_at,
        "the claim would already have been settled before the falsification pass saw it"
    );
    let detail = learn_detail(&mut platform, later);

    let pass = platform
        .falsification()
        .last_pass()
        .expect("the LEARN stage recorded a pass");
    assert_eq!(
        pass.refuted, 1,
        "a claim contradicted by ten held-out volatility observations was not refuted: {detail}"
    );
    let ledger = platform.falsification().ledger();
    assert_eq!(
        ledger.refutations(),
        1,
        "the refutation was not recorded, so the same idea can be re-proposed unchallenged"
    );
    assert!(
        ledger
            .refuted_statements(&claim.class)
            .is_some_and(|set| !set.is_empty()),
        "the refutation was recorded under a family other than {}",
        claim.class
    );
    assert!(
        detail.contains("1 refuted"),
        "the LEARN stage did not report the refutation: {detail}"
    );
    Ok(())
}

/// A claim, refuted mid-horizon or not, driven all the way to its settlement.
///
/// `spike` decides the only difference between the two tests below: whether
/// the tape contradicts the falsifier before the horizon closes. Everything
/// after it is identical — a flat tape to settlement, so realised volatility
/// falls to zero and the claim's own prediction comes true — which is what
/// makes the pair a controlled comparison rather than two anecdotes.
fn settle_a_volatility_claim(spike: bool) -> Result<(FirstClaim, Platform, usize)> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());
    assert!(
        !platform.predictions().is_empty(),
        "no claim was written, so there is nothing to settle:\n{}",
        first.summarise()
    );
    let claim = first_claim(&platform).expect("the claim was recorded against a threshold");
    assert!(
        matches!(claim.comparison, Comparison::LessThan | Comparison::AtMost),
        "the claim compares {:?}, so a rising tape does not contradict it and neither test \
         below asserts anything",
        claim.comparison
    );
    assert!(
        !claim.stated_falsifiers.is_empty(),
        "the claim states no falsifier in prose, so the refutation register would hold \
         arithmetic with no sentence and this test could not tell provenance from coincidence"
    );

    if spike {
        // Thirty alternating held-out closes: realised volatility of about
        // 0.095 against a reference under 0.01, which is the claim's own
        // mirror breached many times over.
        platform.observe(hourly(
            "AAA",
            30,
            |i| if i % 2 == 0 { 100.0 } else { 110.0 },
        ));
    }
    let mid = start().saturating_add(Duration::from_days(5));
    assert!(
        mid < claim.resolves_at,
        "the claim would already have been settled, so the falsification pass would never \
         see it open and the spike could not be recorded"
    );
    let detail = learn_detail(&mut platform, mid);
    let refuted = platform
        .falsification()
        .last_pass()
        .expect("the LEARN stage recorded a pass")
        .refuted;

    // A flat tape from here to the horizon. Realised volatility over the
    // settlement window is zero, which is below the reference the claim named,
    // so the claim is about to come true on the arithmetic the grader reads.
    platform.observe(hourly("AAA", 900, |_| 100.0));
    let settle = claim.resolves_at.saturating_add(Duration::from_hours(1));
    let settled = learn_detail(&mut platform, settle);
    assert!(
        settled.contains("1 thesis(es) resolved, 1 graded"),
        "the horizon passed and nothing was graded, so the verdict this test is about was \
         never reached. mid-horizon: {detail}\nsettlement: {settled}"
    );
    Ok((claim, platform, refuted))
}

#[test]
fn a_thesis_whose_own_falsifier_fired_is_graded_falsified_rather_than_credited_as_a_correct_call()
-> Result<()> {
    // The defect this closes, stated as the failure it caused: every
    // production construction of `ThesisOutcome::falsifiers_triggered` in the
    // kernel was `Vec::new()`, so `Verdict::Falsified` — the grader's
    // strongest refutation — could not be awarded however plainly a thesis
    // had been contradicted. A thesis refuted mid-horizon whose observable
    // then did what it predicted was graded as a call that held, and
    // `counts_as_correct()` fed that into the Brier score. The platform was
    // learning that its reasoning worked from the one case proving it did not.
    let (claim, platform, refuted) = settle_a_volatility_claim(true)?;

    // Premise one: the falsifier actually fired while the claim was open.
    // Without this the verdict below would say nothing about the wiring.
    assert_eq!(
        refuted, 1,
        "the mid-horizon pass refuted nothing, so no refutation was on the register when the \
         thesis settled and this test would pass on an unwired platform"
    );

    let evaluation = platform
        .evaluations()
        .iter()
        .find(|evaluation| evaluation.hypothesis_id == claim.hypothesis_id)
        .expect("the settled thesis was graded");

    // Premise two: the direction held and the move cleared the noise floor.
    // Those two facts rule out `Wrong` and `Inconclusive`, and the platform
    // confirms no mechanism, so the only verdicts this path could otherwise
    // reach are `Vindicated` and `DirectionallyRight` — and
    // `counts_as_correct()` is true of both. That is what makes the assertion
    // below a statement about the falsifier rather than about the arithmetic.
    let policy = qip_learning_engine::evaluation::EvaluationPolicy::default();
    assert!(
        evaluation.magnitude_ratio > 0.0,
        "the realised move went against the claim, so this thesis would have been graded \
         Wrong anyway and the falsifier is not what decided it (ratio {})",
        evaluation.magnitude_ratio
    );
    assert!(
        evaluation.realised_move_bps.abs() >= policy.noise_floor_bps,
        "the realised move of {}bp is inside the {}bp noise floor, so this thesis would have \
         been graded Inconclusive anyway",
        evaluation.realised_move_bps,
        policy.noise_floor_bps
    );

    assert_eq!(
        evaluation.verdict,
        qip_learning_engine::evaluation::Verdict::Falsified,
        "a thesis its own stated falsifier contradicted was graded {:?}: {}",
        evaluation.verdict,
        evaluation.rationale
    );
    assert!(
        !evaluation.verdict.counts_as_correct(),
        "a refuted thesis still counts as a correct call, so the calibration is being fed the \
         opposite of what happened"
    );
    // Provenance, not merely a non-empty list: the rationale has to name the
    // sentence the hypothesis's author wrote, followed by the mirror's own
    // breach word. A `falsifiers_triggered` filled with anything else would
    // reach `Falsified` and prove nothing about where it came from, and the
    // prose alone would match a rationale that merely echoed the claim.
    let stated = &claim.stated_falsifiers[0];
    assert!(
        evaluation
            .rationale
            .contains(&format!("{stated} (rises_to ")),
        "the grade names no falsifier the platform actually stated; it should carry {stated:?} \
         and the level its mirror was breached at: {}",
        evaluation.rationale
    );
    Ok(())
}

#[test]
fn a_thesis_no_held_out_observation_ever_contradicted_is_not_graded_falsified() -> Result<()> {
    // The other half, and the one that stops the wiring above from being a
    // gate that refuses everything. The tape is identical to the test above
    // except that the spike never happens, so the falsifier is never
    // breached — and the settlement arithmetic, which is the same in both,
    // must then produce a verdict that credits the claim. A `Falsified` here
    // would mean the register is answering for a refutation that never
    // occurred, which reads downstream exactly like a thesis that failed.
    let (claim, platform, refuted) = settle_a_volatility_claim(false)?;

    assert_eq!(
        refuted, 0,
        "the tape with no spike refuted the claim anyway, so the two tests are not the \
         controlled comparison they are written as"
    );
    assert_eq!(
        platform.falsification().ledger().refutations(),
        0,
        "the register holds a refutation nothing observed"
    );

    let evaluation = platform
        .evaluations()
        .iter()
        .find(|evaluation| evaluation.hypothesis_id == claim.hypothesis_id)
        .expect("the settled thesis was graded");
    assert_ne!(
        evaluation.verdict,
        qip_learning_engine::evaluation::Verdict::Falsified,
        "an unrefuted thesis was graded as refuted: {}",
        evaluation.rationale
    );
    assert!(
        evaluation.verdict.counts_as_correct(),
        "the claim came true on the settlement arithmetic and was still not credited ({:?}), \
         so the pair of tests is comparing two different settlements rather than one \
         settlement under two histories: {}",
        evaluation.verdict,
        evaluation.rationale
    );
    Ok(())
}
