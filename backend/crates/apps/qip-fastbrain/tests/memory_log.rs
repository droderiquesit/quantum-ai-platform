//! The fast brain's in-memory log under its committed byte ceiling
//! (R12-WORLD-6015, owner decision 3 of 2026-11-28, ADR 0748).
//!
//! On the committed `memory` store the platform's event log and the kernel
//! journal held every record twice and nothing bounded either by size: the
//! node outgrew its 2 GiB container (`infrastructure/terraform/catalogue.tf`)
//! in minutes, about thirteen hours before the first material release on the
//! committed seed. `main.rs` now calls `Platform::bound_memory_log` with
//! [`MEMORY_LOG_CEILING_BYTES`] on a store that does not outlive the process.
//!
//! What is the same as the node, and what is not, as in `world_reaction.rs`:
//! the committed seed, cadence, feed, `PlatformConfig::default()`, universe
//! and limits, every step through `node::step_with`; the clock is a
//! `ManualClock` moved one interval per step. The feed is fast-forwarded to a
//! point before the first material release far enough back that the log
//! must release at least once before the release arrives; every step from
//! there is the node's. The un-fast-forwarded run from start, thirteen hours
//! of feed time, is `examples/memory_log_soak.rs` — about an hour in release,
//! which is not a test.

#![allow(clippy::panic_in_result_fn, clippy::expect_used, clippy::unwrap_used)] // integration tests may expect: a panic is the failure report

use std::sync::Arc;

use qip_core::error::Result;
use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_fastbrain::config::{CONTAINER_MEMORY_BYTES, FastBrainConfig, MEMORY_LOG_CEILING_BYTES};
use qip_fastbrain::feed::Feed;
use qip_fastbrain::node;
use qip_fastbrain::roster::MAXIMUM_BUDGET;
use qip_kernel::bus_bridge::SensePath;
use qip_kernel::{EventLogDestination, Platform, PlatformConfig};
use qip_market_ingestion::synthetic::{ScriptedEventKind, SyntheticEnvironment};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

const CATALOGUE_TF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../infrastructure/terraform/catalogue.tf"
);

/// How far before the release the platform is assembled. On the committed
/// seed the two logs first reach the ceiling a little after a thousand
/// cycles (`examples/memory_log_soak.rs` printed the first release at cycle
/// 1,019); twelve hundred puts at least one release before the event.
const LEAD: Duration = Duration::from_secs(120);

/// As in `world_reaction.rs`: the 20th second bar after the release closes
/// about twenty seconds later.
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

/// The memory the fast brain's `memory` line in `catalogue.tf` commits, in
/// bytes. Reads the `fastbrain = {` block of the catalogue, not the first
/// `memory` in the file, which belongs to another service.
fn committed_container_memory() -> u64 {
    let text = std::fs::read_to_string(CATALOGUE_TF).expect("catalogue.tf is readable");
    let block = text
        .lines()
        .skip_while(|line| line.trim() != "fastbrain = {")
        .take_while(|line| line.trim() != "}")
        .collect::<Vec<_>>();
    assert!(
        block.iter().any(|line| line.contains("\"qip-fastbrain\"")),
        "premise: the block read is not the fast brain's"
    );
    let value = block
        .iter()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "memory").then(|| value.trim().trim_matches('"').to_string())
        })
        .expect("the fast brain's block commits a memory");
    let gibibytes: u64 = value
        .strip_suffix("Gi")
        .unwrap_or_else(|| panic!("the fast brain's memory {value} is not in Gi"))
        .parse()
        .expect("a whole number of GiB");
    gibibytes * 1024 * 1024 * 1024
}

/// The ceiling is a quarter of the memory `catalogue.tf` commits to the fast
/// brain: 512 MiB for 2 GiB. Stated as a constant because the binary cannot
/// read the catalogue, and held to it here so a change to the container's
/// memory cannot leave the ceiling sized for the old one.
///
/// Mutation (fired): `CONTAINER_MEMORY_BYTES` set to 4 GiB (the catalogue
/// says 2 GiB).
#[test]
fn the_ceiling_is_a_quarter_of_the_memory_catalogue_tf_commits_to_the_fast_brain() {
    assert_eq!(CONTAINER_MEMORY_BYTES, committed_container_memory());
    assert_eq!(MEMORY_LOG_CEILING_BYTES, committed_container_memory() / 4);
    assert_eq!(MEMORY_LOG_CEILING_BYTES, 512 * 1024 * 1024);
}

/// A ceiling over a log written to a file is refused, not ignored: the file
/// is the durable record and is never released. So is a ceiling of zero.
///
/// Mutation (fired): the file-destination refusal in `bound_memory_log`
/// removed (the ceiling is admitted).
#[test]
fn a_ceiling_over_a_file_backed_log_and_a_zero_ceiling_are_refused() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("qip-fastbrain-ceiling-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let now = start();
    let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
    let assemble = |event_log: EventLogDestination| -> Result<Platform> {
        let config = PlatformConfig {
            event_log,
            ..PlatformConfig::default()
        };
        let clock = Arc::new(ManualClock::new(now));
        let context = qip_core::Context::new(clock, config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            qip_financial::catalogue::load(&text, now)
                .expect("the committed catalogue loads")
                .universe,
            LimitSet::conservative_default(),
        )
    };
    let mut filed = assemble(EventLogDestination::file(dir.join("events.jsonl")))?;
    assert!(filed.bound_memory_log(MEMORY_LOG_CEILING_BYTES).is_err());
    assert_eq!(filed.memory_log_ceiling(), None);
    let mut held = assemble(EventLogDestination::InMemory)?;
    assert!(held.bound_memory_log(0).is_err());
    assert_eq!(held.memory_log_ceiling(), None);
    held.bound_memory_log(MEMORY_LOG_CEILING_BYTES)?;
    assert_eq!(held.memory_log_ceiling(), Some(MEMORY_LOG_CEILING_BYTES));
    drop(filed);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// At the committed cadence, seed and ceiling, stepped through
/// `node::step_with`: the two in-memory logs never hold more than the
/// ceiling after a cycle; they release at least once before the first
/// material release arrives; the release is checkpointed in the journal with
/// the ranges each log released; both logs verify from that checkpoint and
/// not from genesis; every retained federation review still replays, the
/// ones naming a pass inside the released range included; and the reaction
/// loop then acts on the release in the running node — reaction records at
/// 1, 5 and 20 bars, and causal edges learnt from it through
/// `granger::apply_reaction`.
///
/// Mutations (each fired): the `release_memory_log` call at the end of
/// `run_cycle` removed (the logs pass the ceiling); `replay_each` seeded
/// with empty bases rather than the checkpoint's (retained reviews refuse
/// to replay); the checkpoint record not journaled (no checkpoint in the
/// journal).
#[test]
fn under_the_committed_ceiling_the_node_releases_its_log_and_still_reacts_to_the_first_material_release()
-> Result<()> {
    let config = FastBrainConfig::default();
    let interval = config.cycle_interval;
    assert_eq!(
        interval,
        Duration::from_millis(100),
        "premise: the fast brain's committed cycle interval moved"
    );

    // A scout feed finds the first material company release on the seed.
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

    let mut feed = Feed::synthetic(config.seed, interval, start());
    let assemble_at = at.saturating_sub(LEAD);
    let mut now = start();
    while now.saturating_add(interval) <= assemble_at {
        now = now.saturating_add(interval);
        feed.poll(now, &mut |_| Ok(()))?;
    }

    let clock = Arc::new(ManualClock::new(now));
    let platform_config = PlatformConfig::default();
    assert_eq!(
        platform_config.event_log,
        EventLogDestination::InMemory,
        "premise: the committed platform keeps its log in memory"
    );
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
    // What `main.rs` does on the `memory` store.
    platform.bound_memory_log(MEMORY_LOG_CEILING_BYTES)?;
    let mut sensing = SensePath::new(&platform)?;

    let mut released_before_event = 0usize;
    let mut last_through = 0u64;
    let mut peak = 0u64;
    let settled = at.saturating_add(SETTLE);
    while now < settled {
        clock.advance(interval);
        now = clock.now();
        node::step_with(&mut sensing, &mut platform, &mut feed, now, MAXIMUM_BUDGET)?;
        let held = platform.memory_log_bytes();
        assert!(
            held <= MEMORY_LOG_CEILING_BYTES,
            "at {now} the in-memory logs hold {held} bytes, over the {MEMORY_LOG_CEILING_BYTES} \
             ceiling"
        );
        peak = peak.max(held);
        if let Some(range) = platform.event_log().checkpoint()
            && range.through != last_through
        {
            last_through = range.through;
            if now < at {
                released_before_event += 1;
            }
        }
    }
    assert!(
        released_before_event > 0,
        "premise: the logs released nothing before the release arrived (peak {peak} bytes), so \
         this run says nothing about a node living past its ceiling"
    );

    // The checkpoint is on the record, naming the ranges the logs released.
    let checkpoint = platform
        .memory_log_checkpoint()?
        .expect("a release journals its checkpoint");
    assert_eq!(checkpoint.ceiling_bytes, MEMORY_LOG_CEILING_BYTES);
    assert!(checkpoint.held_bytes > MEMORY_LOG_CEILING_BYTES);
    assert_eq!(
        Some(&checkpoint.journal),
        platform.journal().log().checkpoint(),
        "the journaled checkpoint is not the journal's newest release"
    );
    let journal_head = platform
        .journal()
        .records()
        .first()
        .expect("a retained head");
    assert_eq!(journal_head.sequence, checkpoint.journal.through + 1);
    assert_eq!(journal_head.previous_hash, checkpoint.journal.last_hash);

    // Both logs verify from the checkpoint, and no longer from genesis.
    assert_eq!(platform.event_log().verify_from_checkpoint(), Ok(()));
    assert_eq!(platform.journal().log().verify_from_checkpoint(), Ok(()));
    assert!(platform.journal().verify_chain().is_err());
    assert!(platform.event_log().verify_chain().is_err());

    // Every retained federation review replays from the checkpoint.
    let reviews = qip_kernel::federation_stage::replay_each(&platform)?;
    assert!(
        !reviews.is_empty(),
        "premise: no federation review retained"
    );
    for review in &reviews {
        assert!(
            review.is_ok(),
            "a retained review does not replay: {review:?}"
        );
    }
    // And the cycle journal still decodes from it.
    assert!(!platform.journal_entries()?.is_empty());

    // The reaction loop acted on the release in the running node. Matched
    // on the release's own event id, read exactly up to the horizon suffix.
    let ticker = environment(&feed)
        .companies()
        .iter()
        .find(|c| c.entity_id == entity)
        .map(|c| c.ticker.clone())
        .expect("the entity is a company");
    let event_id = format!("earnings-{ticker}-{}:{entity}", at.as_nanos());
    let episodes: Vec<_> = platform
        .reaction_episodes()
        .iter()
        .filter(|e| {
            e.episode_id
                .rsplit_once('@')
                .is_some_and(|(event, _)| event == event_id)
        })
        .collect();
    assert_eq!(
        episodes.iter().map(|e| e.lag_bars).collect::<Vec<_>>(),
        vec![1, 5, 20],
        "the release {event_id} did not react at exactly 1, 5 and 20 bars under the ceiling: {:?}",
        episodes.iter().map(|e| &e.episode_id).collect::<Vec<_>>()
    );
    let class = episodes[0].event_name.clone();
    assert!(
        platform
            .world()
            .causal()
            .edges()
            .iter()
            .any(|edge| edge.cause == class),
        "the reaction taught the graph no edge from {class}"
    );
    Ok(())
}
