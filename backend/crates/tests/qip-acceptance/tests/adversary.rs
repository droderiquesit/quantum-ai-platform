//! Blueprint §15.2, as far as the platform's own evidence honestly reaches.
//!
//! §15.2's `AdversaryProfiles` policy slot has been declared and unproduced
//! since the payload was written: twelve typed slots, and the twelfth carried
//! nothing in any payload the centre has ever built. These tests drive a real
//! [`Platform`] through real fills, let the counterfactual twin price them,
//! and assert three things the arithmetic tests beside the module cannot
//! reach — that the monitor measures what a *deployed* loop produced, that
//! its finding lands in the hash-chained log, and that producing it moves
//! nothing.
//!
//! The third is the load-bearing one. ADR 0062 says a venue is withdrawn on
//! feasibility evidence and on nothing else, and a monitor that could withdraw
//! on the twin's fill error would put a mis-specified cost model in charge of
//! whether the platform trades. Every test here that produces a finding also
//! asserts the venue is still in use.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::degradation::Freshness;
use qip_contracts::policy::PolicyItem;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::adversary_review::{
    ADVERSARY_MIN_SAMPLE, AdversaryPosture, assess, posture_changes, review, slot,
};
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

/// The desk's one broker, and therefore the only venue any of this can name.
const DESK_VENUE: &str = "simulated-venue";

// --- fixtures ---------------------------------------------------------------
//
// Cloned from `qip-kernel/tests/learning.rs`, which is the only harness in the
// tree that drives a fill all the way through to a twin-priced `FillScore`.
// Cloned rather than shared because that file is a crate's own test binary and
// nothing can import it; the shapes are deliberately identical so that a
// reader comparing the two is comparing one recipe with itself.

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// A liquid listed name on five million units a day, quoted at three basis
/// points. Stated rather than defaulted: `MinLiquidity` and
/// `MaxDaysToLiquidate` are controls whose job is to veto, and a fixture may
/// state its own premise but may not inherit one nobody wrote down.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object("AAA"),
                "AAA",
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
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("adversary-test")
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

fn flat_bars_after(symbol: &str, from: Timestamp, days: i64, level: f64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            let at = from.saturating_add(Duration::from_days(day));
            bar(symbol, at, level, level)
        })
        .collect()
}

/// One accepted, filled order at the desk's venue.
fn fill_one(platform: &mut Platform, proposal: &str, at: Timestamp) -> Result<()> {
    let fills_before = platform.orders().fills().len();
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        proposal,
        vec![format!("hyp-{proposal}")],
        at,
    );
    platform.submit_order(order, at)?;
    assert!(
        platform.orders().fills().len() > fills_before,
        "the accepted order did not fill; the fixture is not a fill"
    );
    Ok(())
}

/// A platform whose twin has priced `count` fills at the desk's venue, every
/// one of them materially worse than the twin modelled.
///
/// The recipe is `learning.rs`'s: the tape's last bar before the fills opens
/// at 90 against a fill at 100, so the twin enters ten percent below the
/// venue's price and the fill error reads a thousand basis points in the
/// direction that says the venue charged more than the model.
fn platform_with_priced_fills(count: usize) -> Result<(Platform, Timestamp)> {
    let mut platform = platform()?;
    let mut tape = quiet_bars("AAA", 90);
    let last = tape.pop().expect("ninety bars");
    let last_at = match &last {
        SensedRecord::Bar(bar) => bar.open_time,
        _ => panic!("quiet_bars produces bars"),
    };
    tape.push(bar("AAA", last_at, 90.0, 90.0));
    platform.observe(tape);
    for n in 0..count {
        fill_one(&mut platform, &format!("prop-filled-{n}"), start())?;
    }
    assert_eq!(
        platform.filled_awaiting_score(),
        count,
        "the premise failed: not every fill reached the twin's queue"
    );
    platform.observe(flat_bars_after("AAA", start(), 5, 100.0));
    // Two cycles, because the twin's per-cycle budget will not price twelve
    // paths in one.
    let scoring_time = start().saturating_add(Duration::from_days(3));
    platform.run_cycle(scoring_time);
    platform.run_cycle(scoring_time);
    assert_eq!(
        platform.fill_scores().len(),
        count,
        "the premise failed: the twin did not price every fill"
    );
    Ok((platform, scoring_time))
}

#[test]
fn a_venue_the_twin_says_is_filling_the_desk_badly_is_measured_and_put_on_the_record() -> Result<()>
{
    // §15.2's instrumentation clause: "recording who filled what and what
    // followed must exist from the beginning or the history required to build
    // it will not be there". Until this module the platform recorded both and
    // read neither. Twelve real fills, priced by the real twin on the real
    // tape, and the monitor reaches a posture from them and writes it to the
    // hash-chained log.
    let (mut platform, now) = platform_with_priced_fills(12)?;

    // Premise: the twin measured every fill, and it measured them as the
    // venue charging materially more than the model. A monitor reading an
    // unmeasured sample would conclude nothing here for the wrong reason.
    let profiles = assess(platform.fill_scores());
    let profile = profiles
        .get(DESK_VENUE)
        .expect("the desk's venue is profiled");
    assert_eq!(profile.measured, 12, "the twin priced fewer than it scored");
    assert!(
        profile.measured >= ADVERSARY_MIN_SAMPLE,
        "the premise failed: the fixture is below the monitor's own bar"
    );
    assert_eq!(profile.adverse, 12);

    // The log already holds this venue's first posture change, and that is
    // the wiring rather than a broken premise: `stage_learn` calls
    // `adversary_review::review` on every cycle, and `platform_with_priced_fills`
    // ran cycles to produce the fills above. This asserted the log was *empty*
    // here until 2026-09-14, which was true only while the module had no
    // production caller — so the assertion was pinning the very gap the wiring
    // closed, and it is inverted rather than relaxed.
    let before = posture_changes(&platform)?;
    assert_eq!(
        before.len(),
        1,
        "the premise failed: the LEARN stage did not record this venue's posture, so the review \
         is reached only by this test: {before:?}"
    );
    assert_eq!(before[0].venue, DESK_VENUE);
    let (summary, problems) = review(&mut platform, now);
    assert!(problems.is_empty(), "the review reported {problems:?}");
    let summary = summary.expect("a review that measured a venue says so");
    // A delimited match, not a substring: "deteriorating" is not a substring
    // of any other posture here, but "unmeasured" and "measured" would be,
    // and the next posture added could be.
    assert!(
        summary.contains(&format!("{DESK_VENUE}: deteriorating")),
        "the summary does not name the venue and its posture: {summary}"
    );

    // Still one: the direct call above re-derived the same posture the cycle
    // had already recorded, and a posture that has not changed writes nothing.
    // That idempotence is the property, and it is stronger evidence now that
    // two independent callers reach the same conclusion than it was when only
    // this test called it.
    let changes = posture_changes(&platform)?;
    assert_eq!(
        changes.len(),
        1,
        "the log holds {} posture change(s), not one: {changes:?}",
        changes.len()
    );
    assert_eq!(changes[0].venue, DESK_VENUE);
    assert_eq!(changes[0].posture, AdversaryPosture::Deteriorating);
    assert_eq!(
        changes[0].previous,
        AdversaryPosture::Unmeasured,
        "a venue the log had never spoken about did not read as unmeasured before"
    );
    assert_eq!(changes[0].measured, 12);

    // And nothing moved. This is ADR 0062's guarantee and it is the reason
    // the monitor may be wrong without being dangerous: the cost model it
    // reads is an estimate, and an estimate must not be able to stop the
    // platform trading.
    assert!(
        platform.withdrawn_venues().is_empty(),
        "a venue was withdrawn on adversary evidence: {:?}",
        platform.withdrawn_venues()
    );
    assert!(
        platform.feasibility_refusals().is_empty(),
        "an adversary finding reached the feasibility window"
    );
    fill_one(&mut platform, "prop-after-the-finding", now)?;
    Ok(())
}

#[test]
fn a_repeated_review_writes_no_second_record_for_a_posture_that_has_not_changed() -> Result<()> {
    // Two failure modes at once. A review that re-derived the same posture and
    // journaled it again would fill the log with one fact repeated every
    // cycle — the `RULE_DEFENDED` discipline, and the reason the record is
    // keyed on venue, posture and cycle. And a review that fell silent once it
    // had nothing new to journal would read in a cycle report exactly like a
    // review that never ran, which is the shape of gap this repository has
    // been bitten by: three modules last wave each returned nothing when idle
    // and each reached no surface at all.
    let (mut platform, now) = platform_with_priced_fills(12)?;
    let cycle_before = platform.cycle_count();
    let (first, problems) = review(&mut platform, now);
    assert!(
        problems.is_empty(),
        "the first review reported {problems:?}"
    );
    assert!(
        first.is_some(),
        "the premise failed: the first review said nothing"
    );
    assert_eq!(
        posture_changes(&platform)?.len(),
        1,
        "the premise failed: the first review did not write exactly one record"
    );

    let (second, problems) = review(&mut platform, now);
    assert!(
        problems.is_empty(),
        "the second review reported {problems:?}"
    );
    let second = second.expect("a review with an unchanged posture still reports the posture");
    assert!(
        second.contains(&format!("{DESK_VENUE}: deteriorating")),
        "a quiet review stopped naming the venue it is still measuring: {second}"
    );
    assert_eq!(
        posture_changes(&platform)?.len(),
        1,
        "the second review wrote a second record for a posture that did not change"
    );

    // And across a cycle boundary, which is the case the idempotency key
    // cannot cover and which this test did not cover until a mutation proved
    // it: the key carries the cycle, so a review that journaled every venue
    // every cycle regardless of what the log already said would mint a fresh
    // key each cycle, write a record each cycle, and bury the log in one fact
    // repeated. Only the comparison against the log's last word stops that.
    // Running a cycle bumps the counter.
    let later = now.saturating_add(Duration::from_days(1));
    platform.run_cycle(later);
    assert!(
        platform.cycle_count() > cycle_before,
        "the premise failed: the cycle counter did not move, so this is still the first cycle and \
         the idempotency key alone would explain the assertion below"
    );
    let (third, problems) = review(&mut platform, later);
    assert!(
        problems.is_empty(),
        "the third review reported {problems:?}"
    );
    assert!(
        third.is_some_and(|line| line.contains(&format!("{DESK_VENUE}: deteriorating"))),
        "the venue stopped being measured in a later cycle"
    );
    assert_eq!(
        posture_changes(&platform)?.len(),
        1,
        "a later cycle wrote a second record for a posture that has not changed since"
    );
    Ok(())
}

#[test]
fn the_adversary_slot_ships_produced_once_a_venue_has_been_measured_and_unproduced_before()
-> Result<()> {
    // Slot twelve of blueprint §41.5, which has been declared and unproduced
    // in every payload the centre has ever built. The two halves matter
    // equally: a slot that produced on an empty measurement would read
    // `Fresh` at a cell on the strength of the producer having run, and a slot
    // that never produced would leave §15.2's transport shell empty for ever.
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 90));
    let before = start().saturating_add(Duration::from_days(1));
    assert!(
        platform.fill_scores().is_empty(),
        "the premise failed: a platform that has traded nothing holds fill scores"
    );
    assert_eq!(
        slot(platform.fill_scores(), before).freshness(PolicyItem::AdversaryProfiles, before),
        Freshness::Unavailable,
        "the slot produced before anything was measured"
    );

    let (platform, now) = platform_with_priced_fills(12)?;
    let produced = slot(platform.fill_scores(), now);
    assert_eq!(
        produced.freshness(PolicyItem::AdversaryProfiles, now),
        Freshness::Fresh,
        "the slot did not produce after twelve fills were priced"
    );
    let venues = &produced
        .value()
        .expect("a produced slot carries a value")
        .venues;
    assert_eq!(
        venues.keys().collect::<Vec<_>>(),
        vec![DESK_VENUE],
        "the shipped profile set names venues the desk does not trade at"
    );
    assert_eq!(
        venues[DESK_VENUE]
            .get("posture")
            .and_then(serde_json::Value::as_str),
        Some("deteriorating"),
        "the shipped profile does not carry the posture the monitor found"
    );
    Ok(())
}

#[test]
fn a_platform_that_has_scored_no_fill_reports_nothing_rather_than_a_clean_bill_of_health()
-> Result<()> {
    // The fail-closed half. A monitor that answered "no adversary found" on a
    // platform that has never had a fill scored would be reporting a
    // measurement nobody made, and an operator reading it would believe the
    // question had been asked. `None` is the honest answer and it is what the
    // stage folds as "this review had no subject".
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 90));
    let now = start().saturating_add(Duration::from_days(1));
    assert!(
        platform.fill_scores().is_empty(),
        "the premise failed: the platform has scored a fill"
    );
    let (summary, problems) = review(&mut platform, now);
    assert!(problems.is_empty(), "the review reported {problems:?}");
    assert_eq!(summary, None);
    assert!(
        posture_changes(&platform)?.is_empty(),
        "a review with no subject journaled a posture"
    );
    Ok(())
}
