//! Load, resilience and chaos.
//!
//! The acceptance scenario shows the loop works once. These tests ask whether
//! it still works on the ten-thousandth pass, on a feed that has gone wrong,
//! and while an operator is pulling things out from under it.
//!
//! Three properties are worth more here than any throughput number:
//!
//! * **Bounded state.** A process that runs for months cannot accumulate a
//!   list per cycle. Growth that is invisible in a unit test is the failure
//!   mode of a long-lived system.
//! * **Progress.** A queue that grows is not the same as a queue that is
//!   worked. A platform that reasons about the same opportunity forever is
//!   idle in a way that looks busy.
//! * **Legibility under failure.** A degraded feed, an empty universe or a
//!   halt must produce a report that says what happened, not a shorter one.
//!
//! There is deliberately no assertion on wall-clock time. A timing threshold
//! on shared CI hardware fails for reasons that have nothing to do with the
//! code, and a threshold loose enough not to would not catch anything. The
//! load tests instead assert the shape of the work — that it stays bounded —
//! which is the property a timing test is usually a proxy for.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::cycle::Stage;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::OperatorIdentity;
use std::sync::Arc;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// `count` distinct synthetic instruments, named so they sort stably.
fn symbols(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("SYN{i:04}")).collect()
}

fn universe(symbols: &[String]) -> Universe {
    let mut universe = Universe::new();
    for symbol in symbols {
        universe
            .insert(
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                    .venue("XNYS")
                    .sector(Sector::InformationTechnology)
                    .price(dec!("100"))
                    .provenance(Provenance::synthetic("resilience", start()))
                    .build(start())
                    .expect("valid instrument"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("resilience")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        )
}

fn platform_over(symbols: &[String]) -> Result<Platform> {
    let clock = Arc::new(ManualClock::new(start()));
    let config = PlatformConfig::default();
    let context = Context::new(clock, config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(symbols),
        limits(),
    )
}

/// A bar built from explicit numbers, so a test can make one wrong on purpose.
#[allow(clippy::too_many_arguments)]
fn bar(symbol: &str, at: Timestamp, open: f64, close: f64, volume: Decimal) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("representable open"),
        high: Decimal::from_f64(open.max(close) * 1.003).expect("representable high"),
        low: Decimal::from_f64(open.min(close) * 0.997).expect("representable low"),
        close: Decimal::from_f64(close).expect("representable close"),
        volume,
        trade_count: 8_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

/// A price series with deterministic noise and an optional jump.
///
/// The rotation constant gives different instruments different paths without
/// an RNG, so a run over two hundred instruments still reproduces exactly.
fn history(symbol: &str, days: usize, seed: usize, jump_at: Option<usize>) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..days)
        .map(|i| {
            let phase = (i + seed * 7) as f64;
            let noise = ((phase * 0.7548776662) % 1.0 - 0.5) * 0.009;
            let jump = if jump_at == Some(i) { 0.085 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((days - i) as i64));
            bar(symbol, at, open, price, dec!("2500000"))
        })
        .collect()
}

// --- load -------------------------------------------------------------------

#[test]
fn a_long_run_does_not_accumulate_unbounded_state() -> Result<()> {
    // The failure this catches is a process that is fine for an afternoon and
    // exhausts a node after a fortnight. Every cycle is a chance to push one
    // more item onto a list nothing ever drains.
    let names = symbols(8);
    let mut platform = platform_over(&names)?;
    for (i, symbol) in names.iter().enumerate() {
        platform.observe(history(
            symbol,
            120,
            i,
            if i == 0 { Some(80) } else { None },
        ));
    }

    // Two run lengths, because the property is not "the queue is small" — it
    // is "the queue does not grow with uptime". A cap chosen from one run is
    // a number that will be wrong the next time a detector is added.
    const SHORT_RUN: usize = 40;
    let cycles = 400;
    let mut now = start();
    let mut peak_queue = 0;
    let mut peak_over_short_run = 0;
    for cycle in 0..cycles {
        let report = platform.run_cycle(now);
        assert!(
            report.traversed_every_stage(),
            "a stage stopped running under load:\n{}",
            report.summarise()
        );
        peak_queue = peak_queue.max(platform.queue().len());
        if cycle + 1 == SHORT_RUN {
            peak_over_short_run = peak_queue;
        }
        now = now.saturating_add(Duration::from_hours(1));
    }

    // The queue is capped by the opportunity engine's per-cycle limit and
    // drained by the stages that work it. Ten times the cycles must not mean a
    // deeper queue, or nothing is ever finished.
    assert_eq!(
        peak_queue, peak_over_short_run,
        "the queue peaked at {peak_queue} over {cycles} cycles against \
         {peak_over_short_run} over {SHORT_RUN}: it grows with uptime"
    );
    assert!(
        platform.queue().len() <= peak_queue,
        "the queue ended above its own peak, which cannot happen"
    );

    // Same for proposals: a fixed working window, not one per cycle for the
    // life of the process. The full history lives in the event log, so the
    // count must still be reported rather than quietly forgotten.
    assert!(
        platform.proposals().len() < cycles,
        "one proposal per cycle is retained forever: {} after {cycles}",
        platform.proposals().len()
    );
    assert_eq!(
        platform.proposals_made(),
        cycles as u64,
        "proposals aged out of memory were also dropped from the count"
    );

    assert_eq!(platform.cycle_count(), cycles as u64);
    Ok(())
}

#[test]
fn the_queue_is_worked_rather_than_merely_grown() -> Result<()> {
    // A platform that reasons about the same opportunity on every cycle looks
    // busy in every metric and makes no progress. The detectors fire on three
    // different instruments; over enough cycles all three must be reached.
    let names = symbols(3);
    let mut platform = platform_over(&names)?;
    for (i, symbol) in names.iter().enumerate() {
        platform.observe(history(symbol, 120, i, Some(70 + i * 10)));
    }

    let mut reasoned_about = std::collections::BTreeSet::new();
    let mut now = start();
    for _ in 0..40 {
        let report = platform.run_cycle(now);
        let reason = report.stage(Stage::Reason).expect("reason ran");
        reasoned_about.insert(reason.detail.clone());
        now = now.saturating_add(Duration::from_hours(6));
    }

    assert!(
        reasoned_about.len() > 1,
        "forty cycles produced one distinct reasoning outcome: {reasoned_about:?}"
    );
    Ok(())
}

#[test]
fn a_wide_universe_completes_a_cycle_with_every_stage_intact() -> Result<()> {
    // Two hundred instruments with a year of history each: enough that a
    // quadratic pass through the history would be obvious, and enough to catch
    // a stage that only holds together on a fixture of three.
    let names = symbols(200);
    let mut platform = platform_over(&names)?;
    let mut absorbed = 0;
    for (i, symbol) in names.iter().enumerate() {
        absorbed += platform.observe(history(
            symbol,
            250,
            i,
            if i % 25 == 0 { Some(200) } else { None },
        ));
    }
    assert_eq!(absorbed, 200 * 250);

    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a stage failed at scale:\n{}",
        report.summarise()
    );

    let sense = report.stage(Stage::Sense).expect("sense ran");
    assert_eq!(
        sense.produced,
        200 * 250,
        "sense lost observations at scale: {}",
        sense.detail
    );

    // The per-cycle cap holds however many anomalies fire. Without it, one bad
    // morning across the whole universe becomes a queue nobody can work.
    assert!(
        platform.queue().len() <= 12,
        "the per-cycle cap did not hold at scale: {} queued",
        platform.queue().len()
    );
    Ok(())
}

// --- resilience -------------------------------------------------------------

#[test]
fn a_degraded_feed_does_not_stop_the_loop() -> Result<()> {
    // Everything a real feed does on a bad day, at once: a halted instrument
    // with no volume, a stuck price, and a series far too short to model.
    let names = symbols(3);
    let mut platform = platform_over(&names)?;

    let mut degraded = Vec::new();
    for day in 0..90 {
        let at = start().saturating_sub(Duration::from_days(90 - day));
        // Halted: zero volume, unchanged price.
        degraded.push(bar(&names[0], at, 100.0, 100.0, Decimal::from_int(0)));
        // Stuck: the same print repeated, which is not the same as calm.
        degraded.push(bar(&names[1], at, 42.5, 42.5, Decimal::from_int(1)));
    }
    // And an instrument with three days of history against a stage that wants
    // sixty.
    for day in 0..3 {
        let at = start().saturating_sub(Duration::from_days(3 - day));
        degraded.push(bar(&names[2], at, 100.0, 101.0, dec!("1000")));
    }
    platform.observe(degraded);

    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a degraded feed stopped a stage:\n{}",
        report.summarise()
    );

    // Legibility is the point: the simulate stage must say it has too little
    // history rather than produce a distribution from three observations.
    let simulate = report.stage(Stage::Simulate).expect("simulate ran");
    assert!(
        simulate.detail.contains("too little history") || simulate.produced >= 60,
        "simulate did not say what it had: {}",
        simulate.detail
    );

    // A stuck series is not an opportunity. Detectors that fire on zero
    // variance would flood the queue every quiet day.
    assert!(
        platform.queue().len() <= 12,
        "a flat, halted feed produced {} opportunities",
        platform.queue().len()
    );
    Ok(())
}

#[test]
fn an_out_of_order_and_duplicated_feed_is_absorbed_without_loss_of_legibility() -> Result<()> {
    // Replays, redeliveries and late arrivals are normal on a market feed.
    // The platform may not silently drop them and may not fall over.
    let names = symbols(2);
    let mut platform = platform_over(&names)?;

    let mut records = history(&names[0], 120, 0, Some(80));
    records.reverse();
    let duplicates = records.clone();
    records.extend(duplicates);
    records.extend(history(&names[1], 120, 1, None));

    let absorbed = platform.observe(records);
    assert_eq!(absorbed, 120 * 3, "records were dropped without a word");

    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a reordered feed stopped a stage:\n{}",
        report.summarise()
    );
    Ok(())
}

#[test]
fn a_platform_with_an_empty_universe_says_so_every_cycle() -> Result<()> {
    // The quiet failure: a misconfigured deployment that runs cleanly forever
    // and never mentions that it is looking at nothing.
    let mut platform = platform_over(&[])?;
    for i in 0..5 {
        let report = platform.run_cycle(start().saturating_add(Duration::from_hours(i)));
        assert!(report.traversed_every_stage());
        let sense = report.stage(Stage::Sense).expect("sense ran");
        assert_eq!(sense.produced, 0);
        assert!(
            sense.detail.contains("blind") || sense.detail.contains("no observations"),
            "an empty platform did not say it was empty: {}",
            sense.detail
        );
    }
    Ok(())
}

// --- chaos ------------------------------------------------------------------

#[test]
fn a_halt_and_a_recovery_mid_run_leave_the_platform_consistent() -> Result<()> {
    // An operator halting and restoring the platform repeatedly, in the middle
    // of a run, with data still arriving. Nothing may reach a venue while it
    // is halted, and everything must resume where it was afterwards.
    use qip_risk_engine::autonomy::AutonomyLevel;

    let names = symbols(4);
    let mut platform = platform_over(&names)?;
    for (i, symbol) in names.iter().enumerate() {
        platform.observe(history(
            symbol,
            120,
            i,
            if i == 1 { Some(85) } else { None },
        ));
    }

    let mut now = start();
    let mut halted_cycles = 0;
    let mut clearances = 0;

    // A credential from the start of the run is stale four hours later, and
    // lifting a halt with it is refused. An operator restarting a platform
    // that stopped itself re-authenticates each time.
    let stale = OperatorIdentity::verified("ops@example.com", "hardware-token", start());

    for cycle in 0..24 {
        // Halt on every third cycle, clear on the one after.
        if cycle % 3 == 0 {
            platform.autonomy_mut().kill_switch_mut().trip_global(
                now,
                "chaos",
                "an operator halted the platform mid-run",
            );
        } else if cycle % 3 == 1 {
            if now > start() {
                assert!(
                    platform
                        .autonomy_mut()
                        .kill_switch_mut()
                        .clear_global(&stale, now)
                        .is_err(),
                    "a halt was lifted with a credential hours old"
                );
            }
            let operator = OperatorIdentity::verified("ops@example.com", "hardware-token", now);
            platform
                .autonomy_mut()
                .kill_switch_mut()
                .clear_global(&operator, now)?;
            clearances += 1;
        }

        let tripped = platform.autonomy().kill_switch().is_globally_tripped();
        let report = platform.run_cycle(now);
        assert_eq!(
            report.halted, tripped,
            "the report disagreed about the halt"
        );
        assert!(
            report.traversed_every_stage(),
            "a halt broke the loop on cycle {cycle}:\n{}",
            report.summarise()
        );

        if tripped {
            halted_cycles += 1;
            assert_eq!(
                platform.autonomy().level(),
                AutonomyLevel::Observation,
                "a halted platform reported an acting autonomy level"
            );
            let order = platform.order_from(
                object(&names[0]),
                Side::Buy,
                dec!("100"),
                dec!("100"),
                "chaos",
                vec!["hyp-chaos".to_string()],
                now,
            );
            assert!(
                platform.submit_order(order, now).is_err(),
                "an order was accepted while halted"
            );
        } else {
            // Clearing restores exactly what was configured, not a default.
            assert_eq!(
                platform.autonomy().level(),
                AutonomyLevel::PaperTrading,
                "clearing the halt did not restore the configured level"
            );
        }

        now = now.saturating_add(Duration::from_hours(4));
    }

    assert!(
        halted_cycles > 0,
        "the chaos never actually halted anything"
    );
    assert!(!platform.orders().has_live_fills());

    // Every lift is named. An incident review that can see what stopped the
    // platform but not who restarted it is missing the more consequential half.
    let recorded = platform.autonomy().kill_switch().clearances();
    assert_eq!(
        recorded.len(),
        clearances,
        "halts were lifted without a record of who lifted them"
    );
    for clearance in recorded {
        assert_eq!(clearance.operator, "ops@example.com");
        assert!(
            !clearance.cleared.reason.is_empty(),
            "a clearance did not carry the halt it lifted"
        );
    }

    // And nothing diverged from the venue along the way.
    assert!(
        platform.orders().reconciliation_breaks().is_empty(),
        "the book and the venue disagree: {:?}",
        platform.orders().reconciliation_breaks()
    );
    Ok(())
}

#[test]
fn the_event_log_chain_verifies_after_a_long_chaotic_run() -> Result<()> {
    // Every decision has to be reconstructable from the log afterwards. A
    // chain that only verifies on a clean run verifies nothing worth having.
    let names = symbols(6);
    let mut platform = platform_over(&names)?;
    for (i, symbol) in names.iter().enumerate() {
        platform.observe(history(
            symbol,
            150,
            i,
            if i % 2 == 0 { Some(100) } else { None },
        ));
    }

    let mut now = start();
    for cycle in 0..60 {
        if cycle % 7 == 0 {
            platform.autonomy_mut().kill_switch_mut().trip_global(
                now,
                "chaos",
                "a periodic halt during a long run",
            );
        }
        if cycle % 7 == 2 {
            let operator = OperatorIdentity::verified("ops@example.com", "hardware-token", now);
            platform
                .autonomy_mut()
                .kill_switch_mut()
                .clear_global(&operator, now)?;
        }
        platform.run_cycle(now);
        now = now.saturating_add(Duration::from_hours(3));
    }

    platform.event_log().verify_chain().map_err(|sequence| {
        qip_core::error::Error::invalid(format!(
            "the event log chain broke at sequence {sequence} after a chaotic run"
        ))
    })?;
    Ok(())
}
