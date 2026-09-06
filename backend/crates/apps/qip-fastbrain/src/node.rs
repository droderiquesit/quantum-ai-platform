//! The run loop: ingest, cycle, repeat, and stop when told.
//!
//! Three properties matter here and each is a function rather than a paragraph
//! inside the loop:
//!
//! * [`step`] is one pass — poll the feed, hand the platform what passed
//!   validation, run a cycle, and time it. It takes the clock reading as an
//!   argument, so a test can drive a cycle without a process and without a
//!   sleep.
//! * [`Stop`] is every way the loop may end. All of them are reached by
//!   finishing a cycle and then deciding, never by abandoning one, which is
//!   what makes shutdown clean: there is no partial cycle to reconcile because
//!   the node never stops inside one.
//! * [`flush`] is what survives the process. It is the only thing between a
//!   session and a session nobody can reconstruct.
//!
//! The fast-path ceiling is checked here, on every cycle, and not only at
//! start-up. A guarantee checked once is a guarantee that drifts: the roster
//! check says the agents *may not* take longer than the ceiling, and this says
//! whether the cycles *did*. A breach is reported and counted, never fatal — a
//! fast path that killed itself for being slow would turn a latency problem
//! into an outage — but a run of them takes the node out of rotation through
//! [`crate::status::NodeStatus::unready`].

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_kernel::{CycleReport, Platform};
use qip_storage::ChainArchive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::FastBrainConfig;
use crate::feed::Feed;
use crate::status::{CycleRecord, NodeStatus};

/// Why the loop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The configured cycle count was reached.
    CycleLimit,
    /// The configured runtime was reached.
    TimeLimit,
    /// A replay ran out of records. The session is over, not broken.
    FeedExhausted,
    /// Somebody asked, through the quiesce endpoint.
    Requested,
}

impl Stop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CycleLimit => "the configured cycle count was reached",
            Self::TimeLimit => "the configured runtime was reached",
            Self::FeedExhausted => "the feed has no records left",
            Self::Requested => "a quiesce was requested",
        }
    }
}

/// What one pass produced.
#[derive(Debug)]
pub struct StepOutcome {
    pub report: CycleReport,
    pub observed: usize,
    pub rejections: Vec<String>,
    /// Measured on a monotonic clock, so a wall-clock adjustment mid-cycle
    /// cannot invent or erase a breach.
    pub elapsed: Duration,
    pub over_budget: bool,
}

impl StepOutcome {
    /// The problems the cycle recorded, plus the budget breach if there was
    /// one, as an operator would read them.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self
            .report
            .problems()
            .into_iter()
            .map(|(stage, problem)| format!("{}: {problem}", stage.as_str()))
            .collect();
        problems.extend(self.rejections.iter().cloned());
        if self.over_budget {
            problems.push(format!(
                "the cycle took {}us, beyond the fast-path ceiling",
                self.elapsed.as_nanos() / 1_000
            ));
        }
        problems
    }
}

/// One pass: poll, observe, cycle, time it.
///
/// `now` is passed in rather than read here because everything downstream takes
/// a timestamp as a parameter, and that is what makes a session replayable. The
/// *duration* is measured separately, on [`std::time::Instant`], because what
/// the budget is about is how long the machine actually took.
pub fn step(
    platform: &mut Platform,
    feed: &mut Feed,
    now: Timestamp,
    budget: Duration,
) -> Result<StepOutcome> {
    let began = std::time::Instant::now();
    let batch = feed.poll(now)?;
    let observed = platform.observe(batch.accepted);
    let report = platform.run_cycle(now);
    let elapsed = monotonic(began);

    Ok(StepOutcome {
        report,
        observed,
        rejections: batch.rejections,
        elapsed,
        over_budget: elapsed > budget,
    })
}

/// A monotonic elapsed time as the platform's own [`Duration`].
///
/// Saturating rather than wrapping: a measurement that overflowed would report
/// a fast cycle, and a ceiling that can be beaten by taking too long is not one.
fn monotonic(began: std::time::Instant) -> Duration {
    Duration::from_nanos(i64::try_from(began.elapsed().as_nanos()).unwrap_or(i64::MAX))
}

/// What the run did, for the closing banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSummary {
    pub stopped_because: Stop,
    pub cycles: u64,
    pub observed: u64,
    pub rejected: u64,
    pub breaches: u64,
    pub worst_cycle: Duration,
    /// Event records handed to the chain archive between cycles, before the
    /// shutdown flush ran at all.
    pub archived_while_running: usize,
}

/// Run until something says to stop.
///
/// The stop flag is read once per iteration, between cycles, so a quiesce takes
/// effect within one cycle interval and never mid-cycle. That bound is the
/// promise: an operator asking a node to stop is told how long it may take, and
/// the answer is one interval plus the flush.
pub fn run(
    platform: &mut Platform,
    feed: &mut Feed,
    archive: &ChainArchive,
    config: &FastBrainConfig,
    status: &Arc<Mutex<NodeStatus>>,
    stop: &Arc<AtomicBool>,
    clock: &Arc<dyn qip_core::Clock>,
    mut on_cycle: impl FnMut(&StepOutcome),
) -> Result<RunSummary> {
    let started = clock.now();
    let mut cycles = 0u64;
    let mut observed = 0u64;
    let mut rejected = 0u64;
    let mut breaches = 0u64;
    let mut worst = Duration::ZERO;
    let mut archived = 0usize;
    let mut since_archive = 0u64;

    let reason = loop {
        if stop.load(Ordering::Relaxed) {
            break Stop::Requested;
        }
        if config.max_cycles.is_some_and(|limit| cycles >= limit) {
            break Stop::CycleLimit;
        }
        if config
            .max_runtime
            .is_some_and(|limit| clock.now().since(started) >= limit)
        {
            break Stop::TimeLimit;
        }
        if feed.is_exhausted() {
            break Stop::FeedExhausted;
        }

        // The feed says when the cycle is: the wall clock for a venue or the
        // synthetic exchange, the next knowable instant for a tape. A tape
        // read on the wall clock is swallowed whole in one poll, and a claim
        // with a five-day horizon recorded in that cycle is never scored.
        let Some(now) = feed.cycle_instant(clock.as_ref()) else {
            break Stop::FeedExhausted;
        };
        set_status(status, |status| status.cycle_started(now));

        let outcome = step(platform, feed, now, config.cycle_budget)?;

        cycles += 1;
        observed += outcome.observed as u64;
        rejected += outcome.rejections.len() as u64;
        if outcome.over_budget {
            breaches += 1;
        }
        if outcome.elapsed > worst {
            worst = outcome.elapsed;
        }

        let record = CycleRecord {
            started_at: now,
            // On the same clock the cycle started on, so a tape's status
            // does not report a cycle that began last year and ended today.
            finished_at: feed.now(clock.as_ref()),
            elapsed: outcome.elapsed,
            observed: outcome.observed,
            rejected: outcome.rejections.len(),
            problems: outcome.problems(),
            halted: outcome.report.halted,
        };
        set_status(status, |status| status.cycle_finished(&record));

        on_cycle(&outcome);

        // Archived in the gap between cycles rather than inside one. A store's
        // latency on the path of every event would be latency this node exists
        // not to have, and the sleep below is dead time that costs nothing to
        // spend here. What the interval trades is how many cycles a crash takes
        // with it.
        since_archive += 1;
        if config.archive_every > 0 && since_archive >= config.archive_every {
            since_archive = 0;
            archived += archive.absorb(platform.event_log().records())?;
        }

        // Hold the cadence rather than the gap: a cycle that took 30 ms out of
        // a 100 ms interval sleeps 70, so the clock does not drift by the cost
        // of the work. A cycle that overran its interval sleeps not at all.
        let remaining = config.cycle_interval - outcome.elapsed;
        if remaining > Duration::ZERO {
            std::thread::sleep(std::time::Duration::from_nanos(
                u64::try_from(remaining.as_nanos()).unwrap_or(0),
            ));
        }
    };

    set_status(status, NodeStatus::stopping);

    Ok(RunSummary {
        stopped_because: reason,
        cycles,
        observed,
        rejected,
        breaches,
        worst_cycle: worst,
        archived_while_running: archived,
    })
}

/// Update the shared status, tolerating a poisoned lock.
///
/// A panic in a health handler must not take the run loop with it. The status
/// is a report, not a decision: losing an update makes the node look one cycle
/// behind, and refusing to trade because a reporting lock was poisoned would be
/// the wrong trade of the two.
fn set_status(status: &Arc<Mutex<NodeStatus>>, update: impl FnOnce(&mut NodeStatus)) {
    match status.lock() {
        Ok(mut guard) => update(&mut guard),
        Err(poisoned) => update(&mut poisoned.into_inner()),
    }
}

/// What the shutdown flush managed to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlushReport {
    /// Event records handed to the chain archive by this flush.
    pub archived: usize,
    /// Records the log held that the flush did not reach.
    pub left_behind: usize,
    /// Whether an acknowledged write here survives this process.
    pub durable: bool,
    pub elapsed: Duration,
    /// The chain's own account of itself afterwards.
    pub chain: String,
}

impl FlushReport {
    pub fn describe(&self) -> String {
        let mut line = format!(
            "flushed {} event record(s) in {}ms; the chain is {}",
            self.archived,
            self.elapsed.as_millis(),
            self.chain
        );
        if self.left_behind > 0 {
            line.push_str(&format!(
                ". {} record(s) were left behind: the flush ran out of its budget",
                self.left_behind
            ));
        }
        if !self.durable {
            line.push_str(
                ". NOTHING HERE SURVIVES THIS PROCESS: the store is memory, so this flush \
                 rearranged what is about to be discarded",
            );
        }
        line
    }
}

/// Write what the node holds, bounded.
///
/// What is at stake on the way out is the event log: every observation the node
/// absorbed and every decision the cycles reached, sealed onto the archive's
/// hash chain. It is *not* written during a cycle, because a store's latency on
/// the path of every event would be latency this node exists not to have — it
/// goes in the gaps between cycles and once more here, so a stop loses at most
/// the cycles since the last gap and a crash loses at most the archive interval.
///
/// Nothing else is at stake, and that is a decision rather than an omission:
/// the platform's market view, price history and feature state are derived from
/// the feed and are rebuilt by running, and a half-restored view is a picture of
/// a market that stopped being true when the process died. There is never a
/// partial cycle to reconcile either, because [`run`] stops between cycles and
/// never inside one.
///
/// Bounded by the work offered rather than by a timer: there is no thread here
/// to interrupt a blocking write, so the records are handed over in chunks and
/// the budget is checked between them. A flush that runs out says how much it
/// left rather than exiting as though it had finished.
pub fn flush(
    platform: &Platform,
    archive: &ChainArchive,
    durable: bool,
    budget: Duration,
) -> Result<FlushReport> {
    /// Records per chunk. Small enough that the budget is checked often,
    /// large enough that the check is not most of the cost.
    const CHUNK: usize = 256;

    let began = std::time::Instant::now();
    let records = platform.event_log().records();
    let mut archived = 0usize;
    let mut reached = 0usize;

    for chunk in records.chunks(CHUNK) {
        if monotonic(began) >= budget {
            break;
        }
        // `absorb` takes the whole slice and skips what it has already sealed,
        // so handing it a growing prefix is the same work split into pieces.
        reached += chunk.len();
        archived += archive.absorb(&records[..reached])?;
    }

    Ok(FlushReport {
        archived,
        left_behind: records.len().saturating_sub(reached),
        durable,
        elapsed: monotonic(began),
        chain: archive.describe(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::{Clock, ManualClock};
    use qip_financial::universe::Universe;
    use qip_kernel::PlatformConfig;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;
    use qip_storage::MemoryKeyValueStore;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform(clock: Arc<dyn Clock>) -> Platform {
        let config = PlatformConfig::default();
        let context = qip_core::Context::new(clock.clone(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    fn archive() -> ChainArchive {
        ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
            .expect("an empty archive opens")
    }

    fn shared(config: &FastBrainConfig) -> Arc<Mutex<NodeStatus>> {
        let roster = crate::roster::clear(start()).expect("the roster clears");
        Arc::new(Mutex::new(NodeStatus::opening(
            &roster,
            config,
            "synthetic-exchange",
            false,
            start(),
        )))
    }

    /// A configuration a test can run in under a second.
    fn brisk() -> FastBrainConfig {
        FastBrainConfig {
            cycle_interval: Duration::from_millis(1),
            ..FastBrainConfig::default()
        }
    }

    /// A feed with half an hour of market already behind it.
    ///
    /// The loop polls up to the clock reading it is given, and these tests hold
    /// the clock still so a cycle is deterministic. A feed that began at the
    /// same instant would have nothing to give, and the tests would assert
    /// against an empty stream while looking like they had ingested one.
    fn backfilled(seed: u64) -> Feed {
        Feed::synthetic(
            seed,
            Duration::from_secs(60),
            start().saturating_sub(Duration::from_mins(30)),
        )
    }

    #[test]
    fn one_step_ingests_records_and_runs_every_stage_of_a_cycle() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let mut feed = Feed::synthetic(11, Duration::from_secs(60), start());

        let now = start().saturating_add(Duration::from_mins(30));
        let outcome = step(&mut platform, &mut feed, now, crate::roster::MAXIMUM_BUDGET)
            .expect("a step runs");

        assert!(
            outcome.observed > 0,
            "the cycle observed nothing; the feed is not reaching the platform"
        );
        assert!(
            outcome.report.traversed_every_stage(),
            "a cycle that skipped a stage is not a cycle"
        );
        assert_eq!(outcome.report.cycle, 1);
    }

    #[test]
    fn a_cycle_slower_than_the_ceiling_is_reported_as_a_breach_rather_than_refused() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let mut feed = Feed::synthetic(12, Duration::from_secs(60), start());

        // A ceiling of one nanosecond no cycle can meet, so what is being tested
        // is the reporting and not the speed of the machine running the test.
        let outcome = step(
            &mut platform,
            &mut feed,
            start().saturating_add(Duration::from_mins(5)),
            Duration::from_nanos(1),
        )
        .expect("an over-budget cycle still returns a report");

        assert!(outcome.over_budget, "a cycle beat a one-nanosecond ceiling");
        assert!(
            outcome
                .problems()
                .iter()
                .any(|problem| problem.contains("beyond the fast-path ceiling")),
            "the breach is not in the problems an operator reads: {:?}",
            outcome.problems()
        );
    }

    #[test]
    fn a_cycle_inside_the_ceiling_is_not_reported_as_a_breach() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let mut feed = Feed::synthetic(13, Duration::from_secs(60), start());

        let outcome = step(
            &mut platform,
            &mut feed,
            start().saturating_add(Duration::from_mins(5)),
            Duration::from_hours(1),
        )
        .expect("a step runs");
        assert!(
            !outcome.over_budget,
            "a cycle was called over budget against an hour-long ceiling"
        );
    }

    #[test]
    fn the_loop_stops_itself_at_the_configured_cycle_count() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let mut feed = backfilled(14);
        let config = FastBrainConfig {
            max_cycles: Some(3),
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let summary = run(
            &mut platform,
            &mut feed,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            |_| {},
        )
        .expect("the loop runs");

        assert_eq!(summary.stopped_because, Stop::CycleLimit);
        assert_eq!(summary.cycles, 3);
        assert_eq!(platform.cycle_count(), 3);
    }

    #[test]
    fn a_stop_request_ends_the_loop_after_the_cycle_in_flight_and_no_later() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let mut feed = backfilled(15);
        let config = brisk();
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        // Asked to stop from inside the first cycle's callback, which is the
        // worst case: the request lands while a cycle is already running.
        let requester = stop.clone();
        let summary = run(
            &mut platform,
            &mut feed,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            move |_| requester.store(true, Ordering::Relaxed),
        )
        .expect("the loop runs");

        assert_eq!(summary.stopped_because, Stop::Requested);
        assert_eq!(summary.cycles, 1);
    }

    #[test]
    fn the_loop_stops_when_a_replay_runs_out_rather_than_cycling_on_an_empty_feed() {
        // Recorded entirely in the past, so the replay drains on the first poll
        // rather than waiting for a clock these tests hold still.
        let mut source = backfilled(16);
        let recorded = source.poll(start()).expect("polls").accepted;
        let directory =
            std::env::temp_dir().join(format!("qip-fastbrain-loop-{}", std::process::id()));
        let path = directory.join("records.jsonl");
        qip_market_ingestion::replay::ReplayAdapter::write(&path, &recorded)
            .expect("the replay file is written");

        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let mut feed = Feed::replay(&path.display().to_string()).expect("the replay file opens");
        let config = brisk();
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let summary = run(
            &mut platform,
            &mut feed,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            |_| {},
        )
        .expect("the loop runs");

        assert_eq!(summary.stopped_because, Stop::FeedExhausted);
        assert!(summary.cycles >= 1);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_status_a_probe_reads_tracks_the_loop_rather_than_being_written_once() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let mut feed = backfilled(17);
        let config = FastBrainConfig {
            max_cycles: Some(2),
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        run(
            &mut platform,
            &mut feed,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            |_| {},
        )
        .expect("the loop runs");

        let guard = status.lock().expect("the status is readable");
        assert_eq!(guard.cycles(), 2);
        assert!(guard.records_observed() > 0);
        assert!(
            guard.is_stopping(),
            "a loop that has ended must say so, or a probe keeps reporting it ready"
        );
    }

    #[test]
    fn the_run_hands_records_to_the_archive_between_cycles_rather_than_only_at_the_end() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let mut feed = backfilled(20);
        let archive = archive();
        let config = FastBrainConfig {
            max_cycles: Some(4),
            archive_every: 1,
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let summary = run(
            &mut platform,
            &mut feed,
            &archive,
            &config,
            &status,
            &stop,
            &clock,
            |_| {},
        )
        .expect("the loop runs");

        assert!(
            summary.archived_while_running > 0,
            "nothing reached the archive during the run, so a crash would take every cycle"
        );
        assert!(
            archive.len().expect("the archive counts itself") > 0,
            "the archive is empty after four archived cycles"
        );
    }

    #[test]
    fn archiving_can_be_deferred_entirely_to_the_shutdown_flush() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let mut feed = backfilled(21);
        let archive = archive();
        let config = FastBrainConfig {
            max_cycles: Some(2),
            archive_every: 0,
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let summary = run(
            &mut platform,
            &mut feed,
            &archive,
            &config,
            &status,
            &stop,
            &clock,
            |_| {},
        )
        .expect("the loop runs");
        assert_eq!(summary.archived_while_running, 0);

        let report =
            flush(&platform, &archive, true, Duration::from_secs(5)).expect("the flush runs");
        assert!(
            report.archived > 0,
            "two cycles produced no event record for the flush to seal"
        );
        assert_eq!(report.left_behind, 0);
    }

    #[test]
    fn a_flush_against_a_store_that_keeps_nothing_says_so_rather_than_reporting_success() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let mut feed = Feed::synthetic(18, Duration::from_secs(60), start());
        let _ = step(
            &mut platform,
            &mut feed,
            start().saturating_add(Duration::from_mins(5)),
            crate::roster::MAXIMUM_BUDGET,
        )
        .expect("a step runs");

        let report =
            flush(&platform, &archive(), false, Duration::from_secs(5)).expect("the flush runs");
        assert!(
            report.describe().contains("NOTHING HERE SURVIVES"),
            "the report reads as a success against a store that keeps nothing: {}",
            report.describe()
        );
    }

    #[test]
    fn flushing_twice_seals_each_record_once() {
        // The chain is the account of what happened. A second flush that
        // appended the same records again would make the count a fiction.
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let mut feed = Feed::synthetic(22, Duration::from_secs(60), start());
        let archive = archive();
        for minute in 1..=2 {
            let _ = step(
                &mut platform,
                &mut feed,
                start().saturating_add(Duration::from_mins(minute)),
                crate::roster::MAXIMUM_BUDGET,
            )
            .expect("a step runs");
        }

        let first = flush(&platform, &archive, true, Duration::from_secs(5)).expect("flushes");
        assert!(first.archived > 0);
        let after = archive.len().expect("the archive counts itself");

        let second = flush(&platform, &archive, true, Duration::from_secs(5)).expect("flushes");
        assert_eq!(second.archived, 0, "the second flush re-sealed records");
        assert_eq!(archive.len().expect("counts"), after);
    }

    #[test]
    fn a_flush_that_runs_out_of_its_budget_says_how_much_it_left_behind() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let mut feed = Feed::synthetic(23, Duration::from_secs(60), start());
        let _ = step(
            &mut platform,
            &mut feed,
            start().saturating_add(Duration::from_mins(20)),
            crate::roster::MAXIMUM_BUDGET,
        )
        .expect("a step runs");
        assert!(
            !platform.event_log().records().is_empty(),
            "the premise: the log holds something to leave behind"
        );

        // A budget of nothing, so the first chunk is never offered.
        let report = flush(&platform, &archive(), true, Duration::ZERO).expect("the flush runs");
        assert_eq!(report.archived, 0);
        assert!(
            report.left_behind > 0 && report.describe().contains("left behind"),
            "an exhausted flush reported as though it had finished: {}",
            report.describe()
        );
    }

    #[test]
    fn every_way_the_loop_can_end_says_why_in_words_an_operator_can_read() {
        for stop in [
            Stop::CycleLimit,
            Stop::TimeLimit,
            Stop::FeedExhausted,
            Stop::Requested,
        ] {
            assert!(
                stop.as_str().len() > 10,
                "{stop:?} has no readable reason attached"
            );
        }
    }
}
