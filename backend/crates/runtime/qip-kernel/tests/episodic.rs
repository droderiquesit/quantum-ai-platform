//! Episodic memory in the cycle: REASON recalls what LEARN resolved, and
//! records it as precedent without touching the confidence.
//!
//! The failure the suite guards has two halves. A memory that fills from
//! nothing — `EpisodicMemory` with no production writer — would be the
//! blueprint's §10 in name only. And a precedent that quietly moved the
//! confidence would put an unreviewed statistic one governed approval away
//! from a position size, which is exactly what ADR 0005's evidence
//! arithmetic exists to prevent.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
// Exact float comparison is deliberate: the claim under test is that the
// confidence with a precedent is the same number as the confidence without
// one, bit for bit. A tolerance would let a small leak through.
#![allow(clippy::float_cmp)]

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
use qip_risk::limits::{Limit, LimitKind, LimitSet};

// --- fixtures, the same shape `learning.rs` feeds -----------------------------

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

fn fresh() -> Result<Platform> {
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

/// A price series with a jump partway through, so the detectors have
/// something real to find.
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

/// Twenty swinging bars up to `horizon`, which move every observable a
/// claim here can name far enough that the verdict is informative.
fn swings(symbol: &str, horizon: Timestamp) -> Vec<SensedRecord> {
    (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            bar(symbol, at, open, close)
        })
        .collect()
}

/// Drive a platform through the three cycles the suite is about: form a
/// claim, resolve it, reason again in the same name. `second_at` is when
/// the resolving cycle runs and `third_at` when the next REASON asks for
/// precedent; the tape is identical whatever the two instants are.
fn three_cycles(second_at: Timestamp, third_at: Timestamp) -> Result<(Platform, Timestamp)> {
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());
    assert!(
        !platform.predictions().is_empty(),
        "premise: the first cycle made a claim:\n{}",
        first.summarise()
    );
    let horizon = platform.predictions()[0].proposition.resolves_at;
    assert!(
        second_at > horizon,
        "the fixture's resolving cycle must run after the horizon"
    );
    platform.observe(swings("AAA", horizon));
    let second = platform.run_cycle(second_at);
    let learn = second.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("episode(s) remembered"),
        "LEARN did not remember the resolved thesis as an episode: {}",
        learn.detail
    );
    assert!(
        platform.predictions()[0].verdict.is_some(),
        "premise: the first claim was settled"
    );
    let third = platform.run_cycle(third_at);
    let reason = third.stage(Stage::Reason).expect("reason ran");
    assert!(
        reason.detail.contains("hypothesis"),
        "premise: the third cycle formed a hypothesis: {}",
        reason.detail
    );
    Ok((platform, horizon))
}

#[test]
fn the_kernel_records_precedents_on_a_hypothesis_once_prior_episodes_resolved_and_leaves_the_confidence_alone()
-> Result<()> {
    // Platform A: the claim resolves at `t2`, and one second later REASON
    // asks again in the same name. The episode LEARN stamped at `t2` is
    // known before `t2 + 1s`, so it must come back as precedent.
    let probe = {
        let mut platform = fresh()?;
        platform.observe(bars("AAA", 120));
        platform.run_cycle(start());
        platform.predictions()[0].proposition.resolves_at
    };
    let t2 = probe.saturating_add(Duration::from_mins(1));
    let t3 = t2.saturating_add(Duration::from_secs(1));
    let (with_memory, _) = three_cycles(t2, t3)?;

    let precedents = with_memory.precedents();
    assert_eq!(
        precedents.len(),
        3,
        "premise: every hypothesis carries a precedent record"
    );
    // The first two REASONs ran before anything resolved, so they saw an
    // empty memory and say so — "no precedent" is `None`, not zero.
    for earlier in &precedents[..2] {
        assert_eq!(
            earlier.memory_size, 0,
            "{}: memory was not empty",
            earlier.hypothesis_id
        );
        assert!(earlier.nearest.is_empty());
        assert_eq!(earlier.digest.agreement, None);
    }
    let third = &precedents[2];
    assert_eq!(third.cycle, 3);
    assert_eq!(
        third.memory_size, 1,
        "the resolved episode did not enter memory"
    );
    assert!(
        !third.nearest.is_empty(),
        "the third REASON recalled nothing though a resolved episode in the same name was \
         known before it ran"
    );
    let recalled = &third.nearest[0];
    assert_eq!(recalled.instrument, "obj-AAA");
    assert_eq!(
        recalled.episode_id, "ep-hyp-1-obj-AAA",
        "the precedent must be the first cycle's episode"
    );
    assert!(
        recalled.known_at < t3 && recalled.known_at == t2,
        "the precedent's known_at is the resolution instant; got {} against t2 {}",
        recalled.known_at.to_rfc3339(),
        t2.to_rfc3339()
    );
    assert!(
        recalled.realised_move_bps.is_some(),
        "a precedent without an outcome is not a precedent"
    );
    assert!(
        third.examined <= 256 && third.examined >= 1,
        "examined {} candidates",
        third.examined
    );
    assert_eq!(third.digest.nearest, third.nearest.len());
    assert!(
        third.digest.agreement.is_some(),
        "a resolved, signed outcome must yield an agreement share"
    );

    // Platform B: the same tape and the same three REASONs, except that the
    // resolving cycle and the next one share the clock reading `t3`. The
    // episode is then stamped known at `t3`, which is not before `t3`, so
    // recall is empty by the point-in-time rule — and everything else about
    // the third REASON is the same question asked at the same instant on
    // the same history. That makes it the control: whatever the precedent
    // digest says, the confidence review produced must be identical.
    let (without_memory, _) = three_cycles(t3, t3)?;
    let control = &without_memory.precedents()[2];
    assert_eq!(control.cycle, 3);
    assert_eq!(
        control.memory_size, 1,
        "premise: the control also resolved and remembered the episode"
    );
    assert!(
        control.nearest.is_empty(),
        "an episode stamped at the instant of the question was recalled"
    );
    assert_eq!(control.digest.agreement, None);
    assert_ne!(
        third.digest, control.digest,
        "premise: the two platforms saw different precedent"
    );
    assert_eq!(
        third.confidence, control.confidence,
        "precedent moved the confidence: {} with a precedent of {:?} against {} without",
        third.confidence, third.digest, control.confidence
    );
    // And the confidence the record carries is the one the claim was
    // written at — the number calibration grades — not a copy taken
    // somewhere else in the stage.
    let claim = with_memory.predictions()[2]
        .claim
        .as_ref()
        .expect("a claim records its confidence");
    assert_eq!(claim.hypothesis_id, third.hypothesis_id);
    assert_eq!(claim.confidence, third.confidence);
    Ok(())
}
