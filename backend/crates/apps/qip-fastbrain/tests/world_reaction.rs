//! The world-market reaction loop at `qip-fastbrain`'s own cadence
//! (R12-WORLD-6015).
//!
//! `qip-kernel/tests/world_reaction.rs` proves the loop on the synthetic
//! exchange's default configuration — a 60 s step, minute bars and a cycle
//! every 30 minutes of feed time. No shipped binary runs that. The fast
//! brain's node opens its synthetic feed through `Feed::synthetic` at its
//! committed `DEFAULT_CYCLE_INTERVAL` of 100 ms, which picks second bars, and
//! every step goes through `node::step_with`: the feed's poll,
//! `Platform::admit_world_records`, the bus bridge's `SensePath`, then
//! `run_cycle`. A reader of the kernel suite could not tell whether a
//! material event yields reactions at 1, 5 and 20 bars there, or whether the
//! 20th bar is even reachable at that resolution. This test answers it.
//!
//! What it shares with the node, and what it does not:
//!
//! - **The same**: `FastBrainConfig::default()`'s seed and cycle interval,
//!   `Feed::synthetic` (the arm `Feed::open` returns when no live, connector,
//!   replay or tape source is configured, as on every committed deployment),
//!   `node::step_with` over one `SensePath` held for the session as
//!   `node::run` holds it, `PlatformConfig::default()`, the committed
//!   universe catalogue and the shipped `LimitSet::conservative_default()`.
//! - **Different**: the clock is a `ManualClock` moved exactly one cycle
//!   interval per step, where `node::run` reads the wall clock and so drifts
//!   by however long a cycle took. The survival policy and hedge candidates
//!   `main.rs` installs are left out; neither is read by the reaction loop.
//! - **Fast-forwarded**: on the committed seed the feed's first material
//!   company release fires 468,715 steps in, about thirteen hours of node
//!   time. A debug-build cycle here costs on the order of a hundred
//!   milliseconds, so cycling all of them is not a test. The feed is polled
//!   alone (its records discarded) to [`LEAD`] before that release and the
//!   platform assembled there, as a node started at that instant would be;
//!   every step from then on is the node's. A deployed node on the committed
//!   seed therefore meets its first material release about thirteen hours
//!   after start, and holds all three horizons about twenty seconds later.
//!
//! That first release is Kestrel's, a synthetic supplier that lists no
//! instrument. Until this test ran, the loop paired only macro releases and
//! listed companies with the feed's instruments, so the first material event
//! the node met was skipped.

#![allow(clippy::panic_in_result_fn, clippy::expect_used, clippy::unwrap_used)] // integration tests may expect: a panic is the failure report

use std::collections::BTreeSet;
use std::sync::Arc;

use qip_core::error::Result;
use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_fastbrain::config::FastBrainConfig;
use qip_fastbrain::feed::Feed;
use qip_fastbrain::node;
use qip_fastbrain::roster::MAXIMUM_BUDGET;
use qip_kernel::bus_bridge::SensePath;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::synthetic::{ScriptedEventKind, SyntheticEnvironment};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

/// The catalogue the node reads through `QIP_UNIVERSE_PATH`, at its
/// committed path rather than a copy.
const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

/// How far before the release the platform is assembled: long enough for
/// every instrument to have ticks and a closed bar before the event.
const LEAD: Duration = Duration::from_secs(15);

/// How long after the release the node is stepped: the 20th second bar
/// opening at or after the release closes about 20 s after it, and five more
/// seconds let every instrument's own 20th bar close too.
const SETTLE: Duration = Duration::from_secs(25);

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 10, 1)
}

fn environment(feed: &Feed) -> &SyntheticEnvironment {
    match feed {
        Feed::Synthetic(environment) => environment,
        _ => panic!("the default feed is not the synthetic exchange"),
    }
}

/// A material release on the fast brain's committed synthetic feed, stepped
/// at its committed 100 ms cadence through `node::step_with`, yields
/// reaction records at 1, 5 and 20 bars, each on every instrument the feed
/// carries; the 1-horizon moves are the published ticks', the 5- and
/// 20-horizon moves are the published second bars', and the 20-bar record
/// is dated within about twenty seconds of the release.
///
/// Mutations (each fired): `bar_move` reads the first bar after the event
/// for every horizon (the 5- and 20-bar moves disagree with the published
/// bars); the landing fallback in `react_to_world_events` restricted to
/// macro releases again (Kestrel's release yields no record).
#[test]
fn a_material_release_at_the_fast_brains_cadence_yields_reaction_records_at_three_horizons()
-> Result<()> {
    let config = FastBrainConfig::default();
    let interval = config.cycle_interval;
    // Premise: the committed cadence, which is what makes this test differ
    // from the kernel suite's thirty-minute cycles.
    assert_eq!(
        interval,
        Duration::from_millis(100),
        "premise: the fast brain's committed cycle interval moved"
    );

    // A scout feed, identical to the node's, finds the first material
    // company release on the committed seed.
    let mut scout = Feed::synthetic(config.seed, interval, start());
    let mut now = start();
    let (entity, at) = loop {
        now = now.saturating_add(interval);
        assert!(
            now < start().saturating_add(Duration::from_days(2)),
            "premise: no material company release in two days of the committed feed"
        );
        scout.poll(now, &mut |_| Ok(()))?;
        if let Some(found) = environment(&scout)
            .fired_events()
            .iter()
            .find_map(|e| match &e.kind {
                ScriptedEventKind::EarningsSurprise { entity_id, .. } => {
                    Some((entity_id.clone(), e.at))
                }
                _ => None,
            })
        {
            break found;
        }
    };
    drop(scout);

    // The node's feed and a twin of it, polled in lockstep, so the test reads
    // what the feed published without reading it back through the platform.
    let mut feed = Feed::synthetic(config.seed, interval, start());
    let mut twin = Feed::synthetic(config.seed, interval, start());
    let assemble_at = at.saturating_sub(LEAD);
    let mut now = start();
    while now.saturating_add(interval) <= assemble_at {
        now = now.saturating_add(interval);
        feed.poll(now, &mut |_| Ok(()))?;
        twin.poll(now, &mut |_| Ok(()))?;
    }
    // Premise: the fast-forward passed no company release. It does pass
    // the synthetic macro releases before it, which a node started here
    // would not have seen either.
    assert!(
        environment(&feed)
            .fired_events()
            .iter()
            .all(|e| !matches!(e.kind, ScriptedEventKind::EarningsSurprise { .. })),
        "premise: the fast-forward passed a company release the platform would then never see"
    );

    let clock = Arc::new(ManualClock::new(now));
    let platform_config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), platform_config.seed);
    let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
    let universe = qip_financial::catalogue::load(&text, now)
        .expect("the committed catalogue loads")
        .universe;
    let mut platform = Platform::new(
        platform_config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;
    let mut sensing = SensePath::new(&mut platform)?;

    let ticker = environment(&feed)
        .companies()
        .iter()
        .find(|c| c.entity_id == entity)
        .map(|c| c.ticker.clone())
        .expect("the entity is a company");
    let event_id = format!("earnings-{ticker}-{}:{entity}", at.as_nanos());
    let carried: BTreeSet<String> = environment(&feed)
        .instruments()
        .iter()
        .map(|i| i.object_id.as_str().to_string())
        .collect();
    assert!(
        carried.len() > 1,
        "premise: the feed carries one instrument"
    );
    // The instrument the episode lands on and lists first: the issuer's own
    // where the company lists one, else the first in key order. On the
    // committed seed the first release is Kestrel's, which lists none, so
    // the second arm is the one this run exercises.
    let issuer = environment(&feed)
        .instruments()
        .iter()
        .find(|i| i.contract.is_none() && i.entity_id.as_deref() == Some(entity.as_str()))
        .map(|i| i.object_id.as_str().to_string())
        .unwrap_or_else(|| carried.first().cloned().expect("an instrument"));
    let of_event = |id: &str| {
        id.rsplit_once('@')
            .is_some_and(|(event, _)| event == event_id)
    };
    let mut ticks: Vec<(String, Timestamp, f64)> = Vec::new();
    let mut bars: Vec<(String, Timestamp, Timestamp, f64)> = Vec::new();
    let mut cycles = 0usize;
    // A fixed window rather than "until three episodes": a loop that stopped
    // on the third episode stops wherever a broken loop first produces one,
    // before the published bars the oracle reads have closed.
    let settled = at.saturating_add(SETTLE);
    while now < settled {
        clock.advance(interval);
        now = clock.now();
        for record in twin.poll(now, &mut |_| Ok(()))?.accepted {
            match record {
                SensedRecord::Tick(tick) => {
                    ticks.push((
                        tick.object_id.as_str().to_string(),
                        tick.at,
                        tick.price.to_f64(),
                    ));
                }
                SensedRecord::Bar(bar) => bars.push((
                    bar.object_id.as_str().to_string(),
                    bar.open_time,
                    bar.close_time(),
                    bar.close.to_f64(),
                )),
                _ => {}
            }
        }
        node::step_with(&mut sensing, &mut platform, &mut feed, now, MAXIMUM_BUDGET)?;
        cycles += 1;
    }

    // Premise: the bars the node's feed published are second bars.
    assert!(!bars.is_empty(), "premise: the feed published no bar");
    assert!(
        bars.iter()
            .all(|(_, open, close, _)| close.since(*open) == Duration::from_secs(1)),
        "premise: the fast brain's feed did not publish one-second bars"
    );

    let episodes: Vec<_> = platform
        .reaction_episodes()
        .iter()
        .filter(|e| of_event(&e.episode_id))
        .collect();
    assert_eq!(
        episodes.iter().map(|e| e.lag_bars).collect::<Vec<_>>(),
        vec![1, 5, 20],
        "the release {event_id} did not react at exactly 1, 5 and 20 bars within {cycles} \
         cycles of the node's cadence"
    );
    let mut later_differs_from_first = false;
    for episode in episodes.iter().copied() {
        assert_eq!(episode.event_time, at, "{}", episode.episode_id);
        let measured: Vec<&str> = episode
            .affected_instruments
            .iter()
            .map(|(id, _, _)| id.as_str())
            .collect();
        assert_eq!(measured.first().copied(), Some(issuer.as_str()));
        let set: BTreeSet<String> = measured.iter().map(|s| s.to_string()).collect();
        assert_eq!(set, carried, "{} missed an instrument", episode.episode_id);
        assert_eq!(set.len(), measured.len(), "an instrument measured twice");
        let h = episode.lag_bars;
        for (instrument, moved, _) in &episode.affected_instruments {
            if h == 1 {
                // The published ticks: the first after the release against
                // the last before it.
                let mine: Vec<(Timestamp, f64)> = ticks
                    .iter()
                    .filter(|(i, _, _)| i == instrument)
                    .map(|(_, t, p)| (*t, *p))
                    .collect();
                let before = mine
                    .iter()
                    .rev()
                    .find(|(t, _)| *t < at)
                    .expect("a tick before");
                let after = mine.iter().find(|(t, _)| *t > at).expect("a tick after");
                let expected = (after.1 / before.1 - 1.0) * 10_000.0;
                assert!(
                    (moved - expected).abs() < 1e-9,
                    "{instrument} moved {moved} bps at 1 tick, its ticks say {expected}"
                );
                continue;
            }
            // The published second bars: the close of the h-th bar opening
            // at or after the release, against the close of the bar before.
            let series: Vec<&(String, Timestamp, Timestamp, f64)> =
                bars.iter().filter(|(i, ..)| i == instrument).collect();
            let first = series
                .iter()
                .position(|(_, open, _, _)| *open >= at)
                .expect("a bar after the release");
            assert!(
                first > 0,
                "premise: {instrument} has no bar before the release"
            );
            let base = series[first - 1].3;
            let horizon_bar = series[first + h - 1];
            let expected = (horizon_bar.3 / base - 1.0) * 10_000.0;
            assert!(
                (moved - expected).abs() < 1e-9,
                "{instrument} moved {moved} bps at {h} bars, its bars say {expected}"
            );
            if instrument == &issuer {
                assert_eq!(
                    episode.reaction_time, horizon_bar.2,
                    "{}",
                    episode.episode_id
                );
            }
            let at_first_bar = (series[first].3 / base - 1.0) * 10_000.0;
            if (expected - at_first_bar).abs() > 1e-9 {
                later_differs_from_first = true;
            }
        }
    }
    assert!(
        later_differs_from_first,
        "premise: the 5- and 20-bar moves equal the first bar's everywhere, so this test cannot \
         tell the horizons apart"
    );
    let twentieth = episodes.last().expect("three episodes");
    assert!(
        twentieth.reaction_time.since(at) <= Duration::from_secs(21),
        "the 20-bar reaction is dated {:?} after the release, not on second bars",
        twentieth.reaction_time.since(at)
    );
    Ok(())
}
