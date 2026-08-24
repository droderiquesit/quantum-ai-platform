//! The run loop: cycle, archive, wait, repeat, and stop when told.
//!
//! Four things matter here and each is a function rather than a paragraph
//! inside the loop:
//!
//! * [`step`] is one pass — run a cycle and time it. It takes the clock reading
//!   as an argument, so a test can drive a cycle without a process and without
//!   a sleep.
//! * [`Stop`] is every way the loop may end. All of them are reached by
//!   finishing a cycle and then deciding, never by abandoning one, which is
//!   what makes shutdown clean: there is no partial cycle to reconcile because
//!   the node never stops inside one. That is worth more here than on the fast
//!   path, where an abandoned cycle costs a millisecond of market view; here it
//!   costs a research run.
//! * [`wait`] is the cadence, and it is interruptible. A five-minute sleep that
//!   ignored a quiesce would make "please stop" take five minutes to land,
//!   against a 120-second termination grace period — the node would be killed
//!   asleep, holding everything it had not archived.
//! * [`appended_since`] and [`flush`] are what survives the process, and
//!   between them they carry the one subtlety in this file. See below.
//!
//! # Why the archive is still here now that the event log has a file
//!
//! [`qip_kernel::EventLogDestination`] lets this node point the kernel's event
//! log at a JSONL file, and [`qip_events::log::EventLog::open`] reads that file
//! back, so the log's own hash chain continues across a restart instead of
//! beginning again at sequence one. That removes the *reason* the app-level
//! archive was originally written down in `qip_storage::chain` — and it does
//! not remove the archive, for two reasons.
//!
//! The first is substrate. The log's file is a path on this container's
//! filesystem; in this workload that is an `emptyDir`, which does not outlive
//! the pod. The archive is a [`qip_storage::ChainArchive`] over whatever
//! `QIP_STORAGE_TARGET` resolves to, which is where the evidence is meant to
//! end up and is what `qip-api` and `qip-cli` read back and verify. A file log
//! makes the chain span *restarts of this process*; the archive makes it span
//! *replacements of this pod*. Those are different guarantees and the node
//! needs both.
//!
//! The second is that the two mechanisms are not automatically composable, and
//! this is the trap [`appended_since`] exists to avoid.
//! [`qip_storage::ChainArchive::open`] deliberately resets its
//! `absorbed_through` watermark to zero, documented there as being because "a
//! restarted process has a fresh source log whose sequence 1 is a genuinely new
//! record". With a file-backed log that assumption is *false*: the log comes
//! back holding sequences 1..=N that a previous run already archived. Handing
//! the whole slice over would append every one of them a second time, at fresh
//! archive positions, and the result would verify perfectly — a chain in which
//! the platform's history appears twice, once per restart, with nothing to say
//! which entries are the duplicates. So this node hands over only what *it*
//! appended, and the watermark it filters on is read from the log at assembly.

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_events::log::LogRecord;
use qip_kernel::{CycleReport, Platform};
use qip_storage::ChainArchive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::DeepBrainConfig;
use crate::status::{CycleRecord, NodeStatus};

/// How often an interruptible wait looks at the stop flag.
///
/// A quarter second: short enough that a quiesce lands promptly against a
/// cadence measured in minutes, long enough that a node idling between cycles
/// is not measurably awake.
const QUIESCE_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Why the loop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The configured cycle count was reached.
    CycleLimit,
    /// The configured runtime was reached.
    TimeLimit,
    /// Somebody asked, through the quiesce endpoint.
    Requested,
}

impl Stop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CycleLimit => "the configured cycle count was reached",
            Self::TimeLimit => "the configured runtime was reached",
            Self::Requested => "a quiesce was requested",
        }
    }
}

/// What one pass produced.
#[derive(Debug)]
pub struct StepOutcome {
    pub report: CycleReport,
    /// Records the evolution engine's adapter fed the platform this cycle.
    /// Zero when no engine is attached — the node then runs blind, exactly as
    /// it did before evolution existed, and the cycle line shows it.
    pub observed: usize,
    /// What the evolution round did, on the cycles where one ran.
    pub evolution: Option<crate::evolution::RoundSummary>,
    /// Measured on a monotonic clock, so a wall-clock adjustment mid-cycle
    /// cannot invent or erase an overrun.
    pub elapsed: Duration,
    /// Whether the cycle took longer than the interval between cycles.
    ///
    /// Reported, never fatal, and never a reason to be unready: on this node it
    /// means the analysis was deeper than the schedule assumed, which is a
    /// capacity signal for an operator and not a fault in the node.
    pub overran_the_interval: bool,
}

impl StepOutcome {
    /// The problems the cycle recorded, as an operator would read them.
    ///
    /// The overrun is deliberately *not* folded in here, which is where this
    /// differs from the fast brain: there, exceeding the budget is the failure
    /// and belongs among the problems. Here it is a fact about the schedule.
    pub fn problems(&self) -> Vec<String> {
        self.report
            .problems()
            .into_iter()
            .map(|(stage, problem)| format!("{}: {problem}", stage.as_str()))
            .collect()
    }
}

/// One pass: run a cycle and time it.
///
/// `now` is passed in rather than read here because everything downstream takes
/// a timestamp as a parameter, and that is what makes a session replayable. The
/// *duration* is measured separately, on [`std::time::Instant`], because what
/// is being reported is how long the machine actually took.
pub fn step(platform: &mut Platform, now: Timestamp, interval: Duration) -> StepOutcome {
    let began = std::time::Instant::now();
    let report = platform.run_cycle(now);
    let elapsed = monotonic(began);

    StepOutcome {
        overran_the_interval: elapsed > interval,
        report,
        elapsed,
        observed: 0,
        evolution: None,
    }
}

/// A monotonic elapsed time as the platform's own [`Duration`].
///
/// Saturating rather than wrapping: a measurement that overflowed would report
/// a fast cycle, and a number that can be improved by taking longer is not a
/// measurement.
fn monotonic(began: std::time::Instant) -> Duration {
    Duration::from_nanos(i64::try_from(began.elapsed().as_nanos()).unwrap_or(i64::MAX))
}

/// The highest sequence a log already held, before this process appended to it.
///
/// Read once at assembly. Zero for a log that started empty — including every
/// in-memory log, which is why a node that keeps nothing behaves exactly as it
/// did before the destination was configurable.
pub fn restored_through(records: &[LogRecord]) -> u64 {
    records.last().map_or(0, |record| record.sequence)
}

/// The records this process appended, as opposed to the ones it inherited.
///
/// The guard described in this module's header. Without it, a node with both a
/// file-backed log and a durable archive re-archives its entire history on
/// every restart.
///
/// Uses a partition rather than an index because the log may evict its oldest
/// records under a capacity bound: eviction removes from the front, so a
/// remembered *count* would drift while a remembered *sequence* stays correct.
pub fn appended_since(records: &[LogRecord], through: u64) -> &[LogRecord] {
    let start = records.partition_point(|record| record.sequence <= through);
    &records[start..]
}

/// Wait out the rest of the cadence, returning early if asked to stop.
///
/// Returns how long it actually waited, so a test can assert the early return
/// happened rather than inferring it from a stopwatch.
///
/// The promise this makes is the one an operator relies on: a quiesce takes
/// effect within the cycle in flight plus [`QUIESCE_POLL`], and *not* within
/// the cycle interval — which at this node's cadence would be longer than the
/// termination grace period the Deployment grants.
pub fn wait(remaining: Duration, stop: &AtomicBool, poll: std::time::Duration) -> Duration {
    let began = std::time::Instant::now();
    if remaining <= Duration::ZERO {
        return Duration::ZERO;
    }
    let total = std::time::Duration::from_nanos(u64::try_from(remaining.as_nanos()).unwrap_or(0));

    while began.elapsed() < total {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let left = total.saturating_sub(began.elapsed());
        std::thread::sleep(poll.min(left));
    }
    monotonic(began)
}

/// What the run did, for the closing banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSummary {
    pub stopped_because: Stop,
    pub cycles: u64,
    /// Cycles that did not traverse every stage of the loop.
    pub failed_cycles: u64,
    /// Cycles that took longer than the interval between cycles.
    pub overruns: u64,
    pub longest_cycle: Duration,
    /// Event records handed to the chain archive between cycles, before the
    /// shutdown flush ran at all.
    pub archived_while_running: usize,
}

/// Whether the loop should end, and why.
///
/// A function rather than three `if`s inside the loop because it is asked twice
/// per iteration — once before starting a cycle and once before waiting out the
/// cadence — and two copies of a stop condition is how a node ends up honouring
/// a bound in one place and ignoring it in the other.
///
/// The quiesce is checked first: when a node has been asked to stop and has
/// also hit a bound, the reason an operator wants to read is the one they
/// caused.
pub fn should_stop(
    config: &DeepBrainConfig,
    cycles: u64,
    elapsed: Duration,
    stop: &AtomicBool,
) -> Option<Stop> {
    if stop.load(Ordering::Relaxed) {
        return Some(Stop::Requested);
    }
    if config.max_cycles.is_some_and(|limit| cycles >= limit) {
        return Some(Stop::CycleLimit);
    }
    if config.max_runtime.is_some_and(|limit| elapsed >= limit) {
        return Some(Stop::TimeLimit);
    }
    None
}

/// Run until something says to stop.
///
/// The stop flag is read between cycles and again inside [`wait`], so a quiesce
/// takes effect within one cycle plus a poll interval and never mid-cycle.
pub fn run(
    platform: &mut Platform,
    archive: &ChainArchive,
    config: &DeepBrainConfig,
    status: &Arc<Mutex<NodeStatus>>,
    stop: &Arc<AtomicBool>,
    clock: &Arc<dyn qip_core::Clock>,
    restored_through: u64,
    mut evolution: Option<&mut crate::evolution::EvolutionEngine>,
    mut on_cycle: impl FnMut(&StepOutcome),
) -> Result<RunSummary> {
    let started = clock.now();
    let mut cycles = 0u64;
    let mut failed = 0u64;
    let mut overruns = 0u64;
    let mut longest = Duration::ZERO;
    let mut archived = 0usize;
    let mut since_archive = 0u64;

    let reason = loop {
        if let Some(reason) = should_stop(config, cycles, clock.now().since(started), stop) {
            break reason;
        }

        let now = clock.now();
        set_status(status, |status| status.cycle_started(now));

        // Sense before thinking: the engine's adapter feeds the platform the
        // records this cycle will reason over, and tees bars for the search.
        let observed = match evolution.as_deref_mut() {
            Some(engine) => engine.sense(platform, now)?,
            None => 0,
        };

        let mut outcome = step(platform, now, config.cycle_interval);
        outcome.observed = observed;

        cycles += 1;

        // Search after thinking, on the engine's own cadence, in the gap
        // between cycles for the same reason the archive writes there: a
        // round's backtests are work the cycle's budget never promised.
        if let Some(engine) = evolution.as_deref_mut() {
            outcome.evolution = engine.maybe_turn(platform, cycles, now)?;
        }
        if !outcome.report.traversed_every_stage() {
            failed += 1;
        }
        if outcome.overran_the_interval {
            overruns += 1;
        }
        if outcome.elapsed > longest {
            longest = outcome.elapsed;
        }

        // Archived in the gap between cycles rather than inside one, and by
        // default after every cycle. The reasoning is in
        // `config::DEFAULT_ARCHIVE_EVERY`: a store write is invisible against a
        // cycle measured in minutes, and what batching would risk losing is the
        // most expensive thing this node produces.
        since_archive += 1;
        let mut archived_now = 0usize;
        if config.archive_every > 0 && since_archive >= config.archive_every {
            since_archive = 0;
            archived_now = archive.absorb(appended_since(
                platform.event_log().records(),
                restored_through,
            ))?;
            archived += archived_now;
        }

        let record = CycleRecord {
            started_at: now,
            finished_at: clock.now(),
            elapsed: outcome.elapsed,
            traversed_every_stage: outcome.report.traversed_every_stage(),
            problems: outcome.problems(),
            archived: archived_now,
            halted: outcome.report.halted,
        };
        set_status(status, |status| status.cycle_finished(&record));

        on_cycle(&outcome);

        // The stop decision is taken *before* the wait rather than at the top
        // of the next iteration. At this node's cadence the difference is not
        // cosmetic: a run bounded at one cycle would otherwise sit through five
        // minutes of sleep it had already decided not to use, and an operator
        // watching a bounded run would conclude it had hung.
        if let Some(reason) = should_stop(config, cycles, clock.now().since(started), stop) {
            break reason;
        }

        // Hold the cadence rather than the gap: a cycle that took two minutes
        // out of a five-minute interval waits three, so the schedule does not
        // drift by the cost of the work. A cycle that outran its interval waits
        // not at all and the next one starts immediately.
        wait(config.cycle_interval - outcome.elapsed, stop, QUIESCE_POLL);
    };

    set_status(status, NodeStatus::stopping);

    Ok(RunSummary {
        stopped_because: reason,
        cycles,
        failed_cycles: failed,
        overruns,
        longest_cycle: longest,
        archived_while_running: archived,
    })
}

/// Update the shared status, tolerating a poisoned lock.
///
/// A panic in a health handler must not take the run loop with it. The status
/// is a report, not a decision: losing an update makes the node look one cycle
/// behind, and abandoning a research run because a reporting lock was poisoned
/// would be the worse of the two outcomes.
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
    /// Records this process appended that the flush did not reach.
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
/// What is at stake on the way out is the event log: every conclusion the
/// cycles reached, sealed onto the archive's hash chain. It goes over in the
/// gaps between cycles and once more here, so a stop loses at most the cycle in
/// flight and a crash loses at most the archive interval.
///
/// Nothing else is at stake, and that is a decision rather than an omission.
/// The world model, the opportunity queue and every agent's working state are
/// *derived* — from the chain, from the universe and from the market — and a
/// half-restored world model is a description of a world that stopped being
/// true when the process died. Rebuilding is both cheaper to reason about and
/// the only version anybody can trust. There is never a partial cycle to
/// reconcile either, because [`run`] stops between cycles and never inside one.
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
    restored_through: u64,
) -> Result<FlushReport> {
    /// Records per chunk. Small enough that the budget is checked often, large
    /// enough that the check is not most of the cost.
    const CHUNK: usize = 256;

    let began = std::time::Instant::now();
    // Only what this process appended. See this module's header: the archive
    // resets its watermark on open, so handing it an inherited prefix would
    // seal the previous run's records a second time.
    let records = appended_since(platform.event_log().records(), restored_through);
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

    fn platform_with(config: PlatformConfig, clock: Arc<dyn Clock>) -> Platform {
        let context = qip_core::Context::new(clock, config.seed);
        let seed = config.seed;
        let _ = seed;
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    fn platform(clock: Arc<dyn Clock>) -> Platform {
        platform_with(PlatformConfig::default(), clock)
    }

    fn archive() -> ChainArchive {
        ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
            .expect("an empty archive opens")
    }

    fn shared(config: &DeepBrainConfig) -> Arc<Mutex<NodeStatus>> {
        let roster = crate::roster::clear(start()).expect("the roster clears");
        Arc::new(Mutex::new(NodeStatus::opening(&roster, config, start())))
    }

    /// A configuration a test can run in well under a second.
    ///
    /// Built directly rather than parsed, because `parse` refuses a sub-second
    /// interval on purpose and this is the one context where a fast cadence is
    /// not a misconfiguration.
    fn brisk() -> DeepBrainConfig {
        DeepBrainConfig {
            cycle_interval: Duration::from_millis(1),
            ..DeepBrainConfig::default()
        }
    }

    fn log_directory(label: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "qip-deepbrain-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn one_step_runs_every_stage_of_a_cycle() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let outcome = step(&mut platform, start(), Duration::from_secs(300));

        assert!(
            outcome.report.traversed_every_stage(),
            "a cycle that skipped a stage is not a cycle"
        );
        assert_eq!(outcome.report.cycle, 1);
        assert!(
            !outcome.overran_the_interval,
            "a cycle with no data outran a five-minute interval"
        );
    }

    #[test]
    fn a_cycle_slower_than_the_interval_is_recorded_as_an_overrun_and_not_as_a_problem() {
        // The difference from the fast brain, asserted: there, exceeding the
        // budget belongs among the cycle's problems because it *is* the
        // failure. Here it is a note about the schedule.
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);

        // An interval of one nanosecond no cycle can meet, so what is being
        // tested is the reporting and not the speed of the test machine.
        let outcome = step(&mut platform, start(), Duration::from_nanos(1));
        assert!(
            outcome.overran_the_interval,
            "a cycle beat a one-nanosecond interval"
        );
        assert!(
            !outcome
                .problems()
                .iter()
                .any(|problem| problem.contains("interval")),
            "the overrun was filed as a problem with the cycle: {:?}",
            outcome.problems()
        );
    }

    #[test]
    fn the_loop_stops_itself_at_the_configured_cycle_count() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let config = DeepBrainConfig {
            max_cycles: Some(3),
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let summary = run(
            &mut platform,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            0,
            None,
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
        let config = brisk();
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        // Asked to stop from inside the first cycle's callback, which is the
        // worst case: the request lands while a cycle is already running.
        let requester = stop.clone();
        let summary = run(
            &mut platform,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            0,
            None,
            move |_| requester.store(true, Ordering::Relaxed),
        )
        .expect("the loop runs");

        assert_eq!(summary.stopped_because, Stop::Requested);
        assert_eq!(summary.cycles, 1);
    }

    #[test]
    fn a_quiesce_during_the_wait_between_cycles_does_not_wait_out_the_cadence() {
        // The reason `wait` exists. At the deployed cadence an uninterruptible
        // sleep would make a stop request take five minutes to land, against a
        // 120-second termination grace period: the node would be killed asleep,
        // holding whatever it had not archived.
        let stop = AtomicBool::new(true);
        let waited = wait(
            Duration::from_secs(300),
            &stop,
            std::time::Duration::from_millis(1),
        );
        assert!(
            waited < Duration::from_secs(1),
            "a quiesced node waited {waited:?} of a five-minute cadence before noticing"
        );
    }

    #[test]
    fn a_wait_nobody_interrupts_lasts_as_long_as_it_was_asked_to() {
        // The premise of the test above: `wait` really does wait when it is not
        // asked to stop, so the early return there is the flag and not a
        // function that never sleeps.
        let stop = AtomicBool::new(false);
        let waited = wait(
            Duration::from_millis(40),
            &stop,
            std::time::Duration::from_millis(5),
        );
        assert!(
            waited >= Duration::from_millis(35),
            "an uninterrupted wait returned after {waited:?} of a 40ms cadence"
        );
    }

    #[test]
    fn a_cycle_that_outran_its_interval_is_followed_immediately_by_the_next_one() {
        let stop = AtomicBool::new(false);
        assert_eq!(
            wait(
                Duration::from_secs(-10),
                &stop,
                std::time::Duration::from_millis(1)
            ),
            Duration::ZERO,
            "a node already behind its cadence still slept"
        );
    }

    #[test]
    fn the_status_a_probe_reads_tracks_the_loop_rather_than_being_written_once() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let config = DeepBrainConfig {
            max_cycles: Some(2),
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        run(
            &mut platform,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            0,
            None,
            |_| {},
        )
        .expect("the loop runs");

        let guard = status.lock().expect("the status is readable");
        assert_eq!(guard.cycles(), 2);
        assert!(
            guard.is_stopping(),
            "a loop that has ended must say so, or a probe keeps reporting it ready"
        );
    }

    #[test]
    fn the_run_hands_records_to_the_archive_between_cycles_rather_than_only_at_the_end() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let archive = archive();
        let config = DeepBrainConfig {
            max_cycles: Some(2),
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let summary = run(
            &mut platform,
            &archive,
            &config,
            &status,
            &stop,
            &clock,
            0,
            None,
            |_| {},
        )
        .expect("the loop runs");

        assert!(
            summary.archived_while_running > 0,
            "nothing reached the archive during the run, so a crash would take every cycle"
        );
        assert!(archive.len().expect("the archive counts itself") > 0);
    }

    #[test]
    fn archiving_can_be_deferred_entirely_to_the_shutdown_flush() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let archive = archive();
        let config = DeepBrainConfig {
            max_cycles: Some(2),
            archive_every: 0,
            ..brisk()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let summary = run(
            &mut platform,
            &archive,
            &config,
            &status,
            &stop,
            &clock,
            0,
            None,
            |_| {},
        )
        .expect("the loop runs");
        assert_eq!(summary.archived_while_running, 0);

        let report =
            flush(&platform, &archive, true, Duration::from_secs(5), 0).expect("the flush runs");
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
        let _ = step(&mut platform, start(), Duration::from_secs(300));

        let report =
            flush(&platform, &archive(), false, Duration::from_secs(5), 0).expect("the flush runs");
        assert!(
            report.describe().contains("NOTHING HERE SURVIVES"),
            "the report reads as a success against a store that keeps nothing: {}",
            report.describe()
        );
    }

    #[test]
    fn flushing_twice_seals_each_record_once() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let archive = archive();
        for _ in 0..2 {
            let _ = step(&mut platform, start(), Duration::from_secs(300));
        }

        let first = flush(&platform, &archive, true, Duration::from_secs(5), 0).expect("flushes");
        assert!(first.archived > 0);
        let after = archive.len().expect("the archive counts itself");

        let second = flush(&platform, &archive, true, Duration::from_secs(5), 0).expect("flushes");
        assert_eq!(second.archived, 0, "the second flush re-sealed records");
        assert_eq!(archive.len().expect("counts"), after);
    }

    #[test]
    fn a_flush_that_runs_out_of_its_budget_says_how_much_it_left_behind() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        let _ = step(&mut platform, start(), Duration::from_secs(300));
        assert!(
            !platform.event_log().records().is_empty(),
            "the premise: the log holds something to leave behind"
        );

        // A budget of nothing, so the first chunk is never offered.
        let report = flush(&platform, &archive(), true, Duration::ZERO, 0).expect("the flush runs");
        assert_eq!(report.archived, 0);
        assert!(
            report.left_behind > 0 && report.describe().contains("left behind"),
            "an exhausted flush reported as though it had finished: {}",
            report.describe()
        );
    }

    #[test]
    fn the_watermark_of_a_log_that_started_empty_is_zero_so_everything_is_new() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock);
        assert_eq!(restored_through(platform.event_log().records()), 0);

        let _ = step(&mut platform, start(), Duration::from_secs(300));
        let records = platform.event_log().records();
        assert_eq!(
            appended_since(records, 0).len(),
            records.len(),
            "a node that inherited nothing must hand over everything it appended"
        );
    }

    #[test]
    fn a_restarted_node_over_a_file_backed_log_does_not_archive_the_previous_run_a_second_time() {
        // The composition trap this module's header describes, exercised end to
        // end: the archive resets its watermark on open, the log does not reset
        // its sequences, and handing over the whole slice would seal the first
        // run's records again at fresh positions.
        let directory = log_directory("rearchive");
        let path = directory.join("events.jsonl");
        let store = Arc::new(MemoryKeyValueStore::default());

        let first_run_records = {
            let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
            let mut platform =
                platform_with(PlatformConfig::default().with_event_log_file(&path), clock);
            let inherited = restored_through(platform.event_log().records());
            assert_eq!(inherited, 0, "the premise: the first run inherits nothing");

            let _ = step(&mut platform, start(), Duration::from_secs(300));
            let archive = ChainArchive::open(store.clone()).expect("the archive opens");
            let sealed = archive
                .absorb(appended_since(platform.event_log().records(), inherited))
                .expect("the first run archives");
            assert!(sealed > 0, "the premise: the first run sealed something");
            sealed
        };

        // A second process over the same file and the same store.
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform =
            platform_with(PlatformConfig::default().with_event_log_file(&path), clock);
        let inherited = restored_through(platform.event_log().records());
        assert_eq!(
            inherited as usize, first_run_records,
            "the premise: the second process read the first run's log back"
        );

        let archive = ChainArchive::open(store.clone()).expect("the archive reopens");
        assert_eq!(
            archive.absorbed_through(),
            0,
            "the premise: a reopened archive has forgotten what it absorbed, which is why the \
             filtering below is needed at all"
        );

        let _ = step(&mut platform, start(), Duration::from_secs(300));
        let sealed = archive
            .absorb(appended_since(platform.event_log().records(), inherited))
            .expect("the second run archives");

        let total = archive.len().expect("the archive counts itself");
        assert_eq!(
            total,
            first_run_records + sealed,
            "the archive holds more entries than the two runs produced, so the first run's \
             records were sealed twice"
        );

        // And the guard is load-bearing: without it the whole history goes over
        // again.
        let unfiltered = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
            .expect("a fresh archive opens");
        let would_seal = unfiltered
            .absorb(platform.event_log().records())
            .expect("an unfiltered hand-over");
        assert!(
            would_seal > sealed,
            "handing over the whole slice sealed no more than the filtered one, so this test \
             is not exercising the trap it describes"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_bounded_run_exits_when_its_last_cycle_lands_rather_than_sleeping_out_the_cadence() {
        // At the deployed cadence the difference between deciding before the
        // wait and deciding after it is five minutes of a run that had already
        // finished. Asserted with a wall clock because the defect is a sleep,
        // and a sleep is invisible to a manual clock.
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
        let mut platform = platform(clock.clone());
        let config = DeepBrainConfig {
            max_cycles: Some(1),
            // The deployed cadence, so this is the real number and not a
            // shortened stand-in for it.
            cycle_interval: Duration::from_secs(300),
            ..DeepBrainConfig::default()
        };
        let status = shared(&config);
        let stop = Arc::new(AtomicBool::new(false));

        let began = std::time::Instant::now();
        let summary = run(
            &mut platform,
            &archive(),
            &config,
            &status,
            &stop,
            &clock,
            0,
            None,
            |_| {},
        )
        .expect("the loop runs");
        let took = began.elapsed();

        assert_eq!(summary.cycles, 1);
        assert_eq!(summary.stopped_because, Stop::CycleLimit);
        assert!(
            took < std::time::Duration::from_secs(30),
            "a one-cycle run took {took:?}; it waited out a cadence it had already decided \
             not to use"
        );
    }

    #[test]
    fn a_quiesce_outranks_a_bound_so_the_reason_reported_is_the_one_somebody_caused() {
        let config = DeepBrainConfig {
            max_cycles: Some(1),
            ..DeepBrainConfig::default()
        };
        let quiesced = AtomicBool::new(true);
        assert_eq!(
            should_stop(&config, 5, Duration::ZERO, &quiesced),
            Some(Stop::Requested),
            "a node that was asked to stop reported a bound instead"
        );

        let running = AtomicBool::new(false);
        assert_eq!(
            should_stop(&config, 1, Duration::ZERO, &running),
            Some(Stop::CycleLimit)
        );
        assert_eq!(
            should_stop(&config, 0, Duration::ZERO, &running),
            None,
            "a node inside every bound was told to stop"
        );
    }

    #[test]
    fn a_run_with_no_bounds_at_all_only_ends_when_it_is_asked_to() {
        // The deployed case: this node stays up until something stops it, so
        // nothing but a quiesce may end the loop.
        let unbounded = DeepBrainConfig::default();
        let running = AtomicBool::new(false);
        assert_eq!(
            should_stop(&unbounded, 10_000, Duration::from_days(30), &running),
            None,
            "an unbounded node stopped itself after enough cycles or enough time"
        );
    }

    #[test]
    fn every_way_the_loop_can_end_says_why_in_words_an_operator_can_read() {
        for stop in [Stop::CycleLimit, Stop::TimeLimit, Stop::Requested] {
            assert!(
                stop.as_str().len() > 10,
                "{stop:?} has no readable reason attached"
            );
        }
    }
}
