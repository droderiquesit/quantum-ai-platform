//! A soak of the fast brain's in-memory log under its committed byte ceiling
//! (ADR 0748), from start to past the first material release on the
//! committed seed — every cycle stepped, none fast-forwarded.
//!
//! `node::run` reads the wall clock, so on a deployed node the first material
//! company release on the committed seed arrives about thirteen hours after
//! start, in wall time. Thirteen hours of wall time is not something a
//! session can run. This drives the same `node::step_with` the run loop
//! calls, on `FastBrainConfig::default()`'s seed and cadence and
//! `PlatformConfig::default()`, with the committed universe and the ceiling
//! `main.rs` installs on the `memory` store, but on a `ManualClock` moved one
//! cycle interval per step — the clock a tape replay owns — so the thirteen
//! hours of feed time pass as fast as the cycles can be computed. What it
//! leaves out is what `node::run` adds around the step: the sleep that holds
//! the cadence, the health server, the desk shipper and the survival policy.
//!
//! Run it in release, which takes about an hour:
//!
//! ```text
//! cargo run --release -p qip-fastbrain --example memory_log_soak
//! ```
//!
//! It prints the log's estimated bytes and the process's resident memory at
//! intervals, every release the ceiling made, and — once the release has
//! settled — the reaction records the loop wrote for it. It exits non-zero
//! if the ceiling is ever exceeded after a cycle, if either log stops
//! verifying from its checkpoint, if a federation review stops replaying, or
//! if the release yields no reaction at 1, 5 and 20 bars.

#![allow(clippy::print_stdout, clippy::expect_used, clippy::unwrap_used)] // a measurement binary: a panic is the failure report

use std::sync::Arc;

use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_fastbrain::config::{FastBrainConfig, MEMORY_LOG_CEILING_BYTES};
use qip_fastbrain::feed::Feed;
use qip_fastbrain::node;
use qip_fastbrain::roster::MAXIMUM_BUDGET;
use qip_kernel::bus_bridge::SensePath;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::synthetic::ScriptedEventKind;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

/// How long after the release the soak keeps stepping: the 20th second bar
/// after it closes about twenty seconds later.
const SETTLE: Duration = Duration::from_secs(25);

/// How often a line of measurement is printed, in cycles.
const REPORT_EVERY: u64 = 20_000;

/// A measurement knob read from the environment: `SOAK_UNBOUNDED` (any
/// value) runs without the ceiling, to measure what the logs would hold;
/// `SOAK_STOP=<cycles>` stops early and prints what the last hundred cycles
/// wrote, by topic and producer; `SOAK_REPORT=<cycles>` sets the interval of
/// the progress lines. Unset, the soak runs to the release with the
/// committed ceiling, which is what `main.rs` installs.
fn knob(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn resident_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    let config = FastBrainConfig::default();
    let interval = config.cycle_interval;
    let start = Timestamp::from_civil(2026, 10, 1);
    let mut feed = Feed::synthetic(config.seed, interval, start);
    let clock = Arc::new(ManualClock::new(start));
    let platform_config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), platform_config.seed);
    let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
    let universe = qip_financial::catalogue::load(&text, start)
        .expect("the committed catalogue loads")
        .universe;
    let mut platform = Platform::new(
        platform_config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
    .expect("the platform assembles");
    if knob("SOAK_UNBOUNDED").is_none() {
        platform
            .bound_memory_log(MEMORY_LOG_CEILING_BYTES)
            .expect("the committed ceiling is admitted");
    }
    let mut sensing = SensePath::new(&platform).expect("the sense path opens");
    println!(
        "ceiling {} bytes; cadence {} ms; seed {}",
        MEMORY_LOG_CEILING_BYTES,
        interval.as_millis(),
        config.seed
    );

    let began = std::time::Instant::now();
    let mut cycles = 0u64;
    let mut peak_bytes = 0u64;
    let mut peak_rss = 0u64;
    let mut releases = 0u64;
    let mut last_through = 0u64;
    let mut release: Option<(String, Timestamp)> = None;
    loop {
        clock.advance(interval);
        let now = clock.now();
        node::step_with(&mut sensing, &mut platform, &mut feed, now, MAXIMUM_BUDGET)
            .expect("the step runs");
        cycles += 1;
        let bytes = platform.memory_log_bytes();
        assert!(
            knob("SOAK_UNBOUNDED").is_some() || bytes <= MEMORY_LOG_CEILING_BYTES,
            "cycle {cycles}: the in-memory logs hold {bytes} bytes, over the {MEMORY_LOG_CEILING_BYTES} ceiling"
        );
        peak_bytes = peak_bytes.max(bytes);
        if let Some(range) = platform.event_log().checkpoint()
            && range.through != last_through
        {
            last_through = range.through;
            releases += 1;
            println!(
                "cycle {cycles} ({} h feed): release {releases} took {} record(s), {} bytes, through sequence {}",
                now.since(start).as_secs_f64() / 3600.0,
                range.records,
                range.bytes,
                range.through
            );
        }
        if knob("SOAK_STOP").is_some_and(|stop| cycles >= stop) {
            println!(
                "stopped at {cycles}: logs {bytes} bytes, rss {} KiB, {:.1} ms/cycle",
                resident_kib(),
                began.elapsed().as_secs_f64() * 1000.0 / cycles as f64
            );
            let recent = now.saturating_sub(Duration::from_secs(10));
            let mut by: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
            for record in platform.event_log().records() {
                if record.event.recorded_at >= recent {
                    let entry = by
                        .entry(format!(
                            "{} {}",
                            record.event.topic.name(),
                            record.event.lineage.producer
                        ))
                        .or_default();
                    entry.0 += 1;
                    entry.1 += qip_events::log::record_footprint(record);
                }
            }
            for (key, (count, bytes)) in by {
                println!("  last 100 cycles: {key}: {count} record(s), {bytes} bytes");
            }
            return;
        }
        if cycles.is_multiple_of(knob("SOAK_REPORT").unwrap_or(REPORT_EVERY).max(1)) {
            let rss = resident_kib();
            peak_rss = peak_rss.max(rss);
            println!(
                "cycle {cycles} ({:.2} h feed, {:.0} s wall): logs {bytes} bytes, rss {rss} KiB, {releases} release(s)",
                now.since(start).as_secs_f64() / 3600.0,
                began.elapsed().as_secs_f64()
            );
        }
        if release.is_none()
            && let Feed::Synthetic(environment) = &feed
            && let Some(found) =
                environment
                    .fired_events()
                    .iter()
                    .find_map(|event| match &event.kind {
                        ScriptedEventKind::EarningsSurprise { entity_id, .. } => {
                            Some((entity_id.clone(), event.at))
                        }
                        _ => None,
                    })
        {
            println!(
                "cycle {cycles}: the first material release, {} at {:.2} h feed",
                found.0,
                found.1.since(start).as_secs_f64() / 3600.0
            );
            release = Some(found);
        }
        if let Some((_, at)) = &release
            && now >= at.saturating_add(SETTLE)
        {
            break;
        }
    }

    let rss = resident_kib();
    peak_rss = peak_rss.max(rss);
    let (entity, at) = release.expect("the loop ends only after the release");
    let ticker = match &feed {
        Feed::Synthetic(environment) => environment
            .companies()
            .iter()
            .find(|company| company.entity_id == entity)
            .map(|company| company.ticker.clone())
            .expect("the entity is a company"),
        _ => unreachable!("the soak runs the synthetic feed"),
    };
    let event_id = format!("earnings-{ticker}-{}:{entity}", at.as_nanos());
    let episodes: Vec<_> = platform
        .reaction_episodes()
        .iter()
        .filter(|episode| {
            episode
                .episode_id
                .rsplit_once('@')
                .is_some_and(|(event, _)| event == event_id)
        })
        .collect();
    let horizons: Vec<usize> = episodes.iter().map(|e| e.lag_bars).collect();
    println!("reaction records for the release: horizons {horizons:?}");
    for episode in &episodes {
        println!(
            "  {} at {} bars: {} instrument(s) measured",
            episode.episode_id,
            episode.lag_bars,
            episode.affected_instruments.len()
        );
    }
    let class = episodes.first().map(|e| e.event_name.clone());
    let edges = class.as_ref().map_or(0, |class| {
        platform
            .world()
            .causal()
            .edges()
            .iter()
            .filter(|edge| &edge.cause == class)
            .count()
    });
    println!("causal edges from {class:?}: {edges}");
    let event_log = platform.event_log().verify_from_checkpoint();
    let journal = platform.journal().log().verify_from_checkpoint();
    let reviews = qip_kernel::federation_stage::replay_each(&platform).expect("the journal reads");
    let unreadable = reviews.iter().filter(|review| review.is_err()).count();
    println!(
        "{cycles} cycles in {:.0} s; peak logs {peak_bytes} bytes of {MEMORY_LOG_CEILING_BYTES}; \
         peak rss {peak_rss} KiB; {releases} release(s); event log from checkpoint {event_log:?}; \
         journal from checkpoint {journal:?}; {} federation review(s) retained, {unreadable} unreadable",
        began.elapsed().as_secs_f64(),
        reviews.len()
    );
    assert_eq!(event_log, Ok(()));
    assert_eq!(journal, Ok(()));
    assert_eq!(
        unreadable, 0,
        "a retained federation review does not replay"
    );
    assert!(releases > 0, "the ceiling never released anything");
    assert_eq!(
        horizons,
        vec![1, 5, 20],
        "the release did not react at 1, 5 and 20 bars"
    );
    assert!(
        edges > 0,
        "the reaction loop taught the graph no edge from the release"
    );
}
