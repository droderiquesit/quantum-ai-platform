//! Phase 12's exit (R12-MODEL-10171): one kernel cycle opens a tournament
//! question, a later LEARN settles it through the forecast market, and the
//! adversary review journals a verdict on the venue — driven through
//! `Platform::run_cycle` and `Platform::submit_order`, the calls the binaries
//! make, and read back from the platform's own log.
//!
//! The settlement and the posture are the two halves a library-only test
//! would miss: deleting the `tournament.learn` call in LEARN leaves the
//! question open and nothing settled; deleting the `adversary_review::review`
//! call leaves the fills scored and no posture ever journaled.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::EventBody;
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::adversary_review::{AdversaryPosture, posture_changes};
use qip_kernel::config::PlatformConfig;
use qip_kernel::model_tournament::{TOURNAMENT_ORIGIN, TournamentReview};
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-AAA")
}

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            object(),
            "AAA",
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("phase12-exit", start()))
        .build(start())?,
    )?;
    let limits = LimitSet::new("phase12-exit")
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
        );
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits)
}

fn bar(at: Timestamp, open: f64, close: f64) -> SensedRecord {
    let price = |value: f64| Decimal::from_f64(value).ok_or_else(|| Error::invalid("a price"));
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: price(open).unwrap_or_default(),
        high: price(open.max(close) * 1.002).unwrap_or_default(),
        low: price(open.min(close) * 0.998).unwrap_or_default(),
        close: price(close).unwrap_or_default(),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

/// Noise with a jump two thirds in, so the detectors find something and
/// REASON forms a hypothesis.
fn jumpy(count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.754_877_666_2) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            bar(
                start().saturating_sub(Duration::from_days((count - i) as i64)),
                open,
                price,
            )
        })
        .collect()
}

fn reviews(platform: &Platform) -> Result<Vec<TournamentReview>> {
    platform
        .event_log()
        .records()
        .iter()
        .filter(|record| {
            record.event.topic == TournamentReview::TOPIC
                && record.event.lineage.producer == TOURNAMENT_ORIGIN
        })
        .map(|record| {
            Ok(StreamEnvelope::from_frame(&record.event)?
                .decode::<TournamentReview>()?
                .body)
        })
        .collect()
}

#[test]
fn a_cycle_opens_a_tournament_question_and_a_later_learn_settles_it_through_the_market()
-> Result<()> {
    let mut platform = platform()?;
    platform.observe(jumpy(120));
    let first = platform.run_cycle(start());
    let question = platform
        .model_tournament()
        .open_questions()
        .next()
        .cloned()
        .ok_or_else(|| Error::not_found(format!("no question opened:\n{}", first.summarise())))?;
    let id = question.spec().id.clone();
    let resolves_at = question.spec().resolves_at;
    assert!(
        reviews(&platform)?
            .iter()
            .all(|review| review.resolved.iter().all(|r| r.question != id)),
        "premise: nothing is settled before the horizon"
    );

    platform.observe(vec![bar(resolves_at, 150.0, 165.0)]);
    platform.run_cycle(resolves_at.saturating_add(Duration::from_days(2)));

    assert!(platform.model_tournament().question(&id).is_none());
    let settled = reviews(&platform)?
        .into_iter()
        .flat_map(|review| review.resolved)
        .find(|resolved| resolved.question == id)
        .ok_or_else(|| Error::not_found("the resolution was not journaled"))?;
    assert!(
        !settled.settlements.is_empty(),
        "the forecast market settled nothing for a resolved question"
    );
    Ok(())
}

#[test]
fn twelve_fills_the_twin_misprices_make_the_adversary_review_journal_a_posture() -> Result<()> {
    let mut platform = platform()?;
    // A quiet tape whose last bar opens at 90 against an arrival of 100, so
    // the twin prices every fill a thousand basis points wrong.
    let mut tape: Vec<SensedRecord> = (0..90)
        .map(|i| {
            let price = if i % 2 == 0 { 100.3 } else { 99.7 };
            bar(
                start().saturating_sub(Duration::from_days(90 - i)),
                price,
                price,
            )
        })
        .collect();
    let last_at = match tape.pop() {
        Some(SensedRecord::Bar(last)) => last.open_time,
        _ => return Err(Error::invalid("the tape was empty")),
    };
    tape.push(bar(last_at, 90.0, 90.0));
    platform.observe(tape);
    for n in 0..12 {
        let fills_before = platform.orders().fills().len();
        let order = platform.order_from(
            object(),
            Side::Buy,
            dec!("700"),
            dec!("100"),
            &format!("prop-filled-{n}"),
            vec![format!("hyp-{n}")],
            start(),
        );
        platform.submit_order(order, start())?;
        assert!(
            platform.orders().fills().len() > fills_before,
            "premise: the order filled"
        );
    }
    assert!(
        posture_changes(&platform)?.is_empty(),
        "premise: no posture is journaled before LEARN has priced a fill"
    );
    let flat: Vec<SensedRecord> = (1..=5)
        .map(|day| {
            bar(
                start().saturating_add(Duration::from_days(day)),
                100.0,
                100.0,
            )
        })
        .collect();
    platform.observe(flat);
    let scoring_time = start().saturating_add(Duration::from_days(3));
    platform.run_cycle(scoring_time);
    platform.run_cycle(scoring_time);

    let changes = posture_changes(&platform)?;
    assert!(!changes.is_empty(), "no adversary posture was journaled");
    assert!(
        changes
            .iter()
            .all(|change| change.posture != AdversaryPosture::Unmeasured),
        "an unmeasured venue was journaled as a verdict"
    );
    Ok(())
}
