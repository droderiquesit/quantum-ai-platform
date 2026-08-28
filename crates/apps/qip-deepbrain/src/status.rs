//! What the node knows about itself, and what a probe reads.
//!
//! Held separately from the platform and updated by the run loop rather than
//! read out of it, for one reason: the health surface must be able to answer
//! *while a cycle is running*. On this node that is not a nicety. A deep-brain
//! cycle may run for minutes and may block on a language model, so "a cycle is
//! in flight" is the ordinary state rather than an instant, and a probe that
//! had to take the same lock the loop holds would time out against a perfectly
//! healthy node and report it dead.
//!
//! # What readiness means here, and what it deliberately does not
//!
//! The fast brain reports itself unready when it is too slow, because being
//! fast is the whole of what it promises. Applying that rule here would be a
//! category error: a deep brain that took ten minutes over a causal analysis
//! did the job. **Slowness is never a reason to be unready and never a reason
//! to fail liveness on this node.**
//!
//! What is left are the five states in [`Unready`], and the one that has no
//! counterpart on the fast path is [`Unready::Warming`]. A fast brain is useful
//! within a hundred milliseconds of starting; this node has produced nothing
//! anybody can consult until its first cycle completes, which may be minutes
//! after the process began answering probes. Reporting ready before then would
//! point traffic at a node whose world model is empty.

use qip_core::{Duration, Timestamp};
use qip_observability::Metrics;
use serde::Serialize;
use std::sync::Arc;

use crate::config::DeepBrainConfig;
use crate::roster::ClearedRoster;

/// How many cycle intervals may pass with nothing finishing before the node is
/// considered stalled.
///
/// Four rather than the fast brain's twenty, because the interval it multiplies
/// is three hundred times longer: twenty intervals at this cadence is most of a
/// working day, by which point a wedged node has been wedged for hours with a
/// green probe.
const STALL_INTERVALS: i64 = 4;

/// The floor under the stall ceiling.
///
/// Half an hour, and it is the number that actually applies at the default
/// cadence. It has to clear a *legitimate* worst case, not a typical one: one
/// full cycle interval of waiting plus a cycle that is itself slow because an
/// agent is blocked on a language model. Anything tighter would restart-loop a
/// node that is thinking hard, which is the failure this node is most likely to
/// suffer and least able to recover from.
const MINIMUM_STALL_CEILING: Duration = Duration::from_mins(30);

/// Everything the node reports about itself.
#[derive(Clone, Debug)]
pub struct NodeStatus {
    /// Whether the roster check passed. It cannot be true unless it did: the
    /// only constructor takes a [`ClearedRoster`], which only the check makes.
    roster_validated: bool,
    roster_agents: Vec<String>,
    excluded_agents: Vec<String>,
    model_callers: usize,
    started_at: Timestamp,
    cycle_interval: Duration,
    failure_tolerance: u32,
    stall_ceiling: Duration,
    event_log: String,

    cycles: u64,
    failed_cycles: u64,
    consecutive_failures: u32,
    last_cycle_started_at: Option<Timestamp>,
    last_cycle_finished_at: Option<Timestamp>,
    last_cycle_elapsed: Duration,
    longest_cycle_elapsed: Duration,
    /// Cycles that took longer than the interval between cycles. Counted and
    /// reported, and deliberately not a reason to be unready — see [`Unready`].
    overruns: u64,
    last_cycle_problems: Vec<String>,
    archived: u64,
    halted: bool,
    stopping: bool,

    /// The metric registry the cycle records into.
    ///
    /// Held here because this struct is what the health thread and the run loop
    /// already share, and the scrape surface has to read the registry the loop
    /// writes to rather than one of its own. A second registry made for the
    /// health thread would answer every scrape with an empty surface forever
    /// while the platform recorded diligently into one nothing could reach —
    /// which is the defect this whole surface exists to close, rebuilt one
    /// level up.
    ///
    /// Defaulted to an empty registry rather than made a constructor argument
    /// so that a status can be built before the platform exists, which is the
    /// order both nodes start in: the surface answers `warming` before there is
    /// anything to record. [`Self::with_metrics`] installs the real one.
    metrics: Arc<Metrics>,
}

/// Why the node is not ready to be consulted.
///
/// An enum rather than a boolean because "not ready" is the answer a probe
/// gets and "why" is the next question; a probe that can only say no sends
/// whoever is paged to read the logs.
///
/// Note what is *not* here. There is no `PersistentlyOverBudget`: on this node
/// a long cycle is the work, not a symptom, and the only thing a readiness
/// signal tied to duration would achieve is taking the deepest analyses out of
/// rotation first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unready {
    /// Shutting down. Correct, not a fault: a node on its way out should stop
    /// being consulted before it stops answering.
    Stopping,
    /// The kill switch is tripped.
    Halted,
    /// Nothing has finished in far longer than a cycle takes. The one state
    /// that distinguishes a node thinking from a node wedged, and the reason
    /// the ceiling above is generous rather than tight.
    Stalled,
    /// Cycles keep failing to traverse the loop. The node is alive, is not
    /// stuck, and is not producing research — which is the case neither a
    /// liveness probe nor a stall check can see.
    PersistentlyFailing,
    /// No cycle has completed yet. Has no counterpart on the fast path: this
    /// node's first cycle may take minutes, and until it lands there is no
    /// world model, no thesis and nothing to consult.
    Warming,
}

impl Unready {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopping => "stopping",
            Self::Halted => "halted",
            Self::Stalled => "stalled",
            Self::PersistentlyFailing => "persistently_failing",
            Self::Warming => "warming",
        }
    }
}

/// One cycle's result, as the status records it.
#[derive(Clone, Debug)]
pub struct CycleRecord {
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub elapsed: Duration,
    /// Whether every stage of the loop ran.
    ///
    /// The failure signal, rather than the presence of problems: the kernel
    /// records a stage's problem and continues on purpose, so a cycle with
    /// problems is a cycle that worked and noticed something. A cycle that
    /// skipped a stage did not run the loop.
    pub traversed_every_stage: bool,
    pub problems: Vec<String>,
    pub archived: usize,
    pub halted: bool,
}

impl NodeStatus {
    /// Open a status for a node whose roster has cleared.
    ///
    /// Takes the cleared roster by reference rather than a boolean, so
    /// `"roster_validated": true` in a health response is not something a
    /// caller can assert on its own behalf.
    pub fn opening(
        roster: &ClearedRoster,
        config: &DeepBrainConfig,
        started_at: Timestamp,
    ) -> Self {
        let ceiling = config.cycle_interval * STALL_INTERVALS;
        Self {
            roster_validated: true,
            roster_agents: roster.ids().into_iter().map(str::to_string).collect(),
            excluded_agents: roster.excluded.clone(),
            model_callers: roster.model_callers(),
            started_at,
            cycle_interval: config.cycle_interval,
            failure_tolerance: config.failure_tolerance,
            stall_ceiling: ceiling.max(MINIMUM_STALL_CEILING),
            event_log: config.event_log.describe(),
            cycles: 0,
            failed_cycles: 0,
            consecutive_failures: 0,
            last_cycle_started_at: None,
            last_cycle_finished_at: None,
            last_cycle_elapsed: Duration::ZERO,
            longest_cycle_elapsed: Duration::ZERO,
            overruns: 0,
            last_cycle_problems: Vec::new(),
            archived: 0,
            halted: false,
            stopping: false,
            metrics: Arc::new(Metrics::new("qip-deepbrain")),
        }
    }

    /// Note that a cycle has begun.
    ///
    /// Recorded before the work rather than after it, which matters more here
    /// than anywhere: a start with no matching finish is the only way a node
    /// blocked on a model call is visible from outside, and a status written
    /// only on completion could never show one.
    pub fn cycle_started(&mut self, at: Timestamp) {
        self.last_cycle_started_at = Some(at);
    }

    /// Fold a completed cycle in.
    pub fn cycle_finished(&mut self, record: &CycleRecord) {
        self.cycles += 1;
        self.last_cycle_started_at = Some(record.started_at);
        self.last_cycle_finished_at = Some(record.finished_at);
        self.last_cycle_elapsed = record.elapsed;
        if record.elapsed > self.longest_cycle_elapsed {
            self.longest_cycle_elapsed = record.elapsed;
        }
        if record.elapsed > self.cycle_interval {
            self.overruns += 1;
        }
        if record.traversed_every_stage {
            self.consecutive_failures = 0;
        } else {
            self.failed_cycles += 1;
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        }
        self.last_cycle_problems.clone_from(&record.problems);
        self.archived += record.archived as u64;
        self.halted = record.halted;
    }

    /// Mark the node as on its way out.
    pub fn stopping(&mut self) {
        self.stopping = true;
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping
    }

    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    pub fn failed_cycles(&self) -> u64 {
        self.failed_cycles
    }

    pub fn overruns(&self) -> u64 {
        self.overruns
    }

    pub fn longest_cycle_elapsed(&self) -> Duration {
        self.longest_cycle_elapsed
    }

    pub fn archived(&self) -> u64 {
        self.archived
    }

    /// Whether a cycle has begun and not yet finished.
    pub fn cycle_in_flight(&self) -> bool {
        match (self.last_cycle_started_at, self.last_cycle_finished_at) {
            (Some(started), Some(finished)) => started > finished,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// How long since anything last finished.
    ///
    /// Measured from start-up when no cycle has finished yet, so a node that
    /// wedged on its very first cycle is eventually stalled rather than
    /// permanently warming.
    pub fn quiet_for(&self, now: Timestamp) -> Duration {
        let last = self.last_cycle_finished_at.unwrap_or(self.started_at);
        now.since(last).max(Duration::ZERO)
    }

    /// The stall ceiling this node is being held to.
    pub fn stall_ceiling(&self) -> Duration {
        self.stall_ceiling
    }

    /// Why this node should not be consulted, if it should not be.
    ///
    /// Ordered by what an operator most needs to know. In particular the stall
    /// check comes *before* the warming one, so a node whose first cycle never
    /// returns reports `stalled` rather than claiming to still be starting up
    /// an hour later.
    pub fn unready(&self, now: Timestamp) -> Option<Unready> {
        if self.stopping {
            return Some(Unready::Stopping);
        }
        if self.halted {
            return Some(Unready::Halted);
        }
        if self.quiet_for(now) > self.stall_ceiling {
            return Some(Unready::Stalled);
        }
        if self.consecutive_failures > self.failure_tolerance {
            return Some(Unready::PersistentlyFailing);
        }
        if self.cycles == 0 {
            return Some(Unready::Warming);
        }
        None
    }

    pub fn is_ready(&self, now: Timestamp) -> bool {
        self.unready(now).is_none()
    }

    /// Use the platform's own metric registry rather than the empty one this
    /// status was built with.
    ///
    /// Called once, in the composition root, with the handle taken from the
    /// telemetry before it moves into the platform. Taking it any later is not
    /// possible in one of the two nodes — deepbrain serves its health surface
    /// before it assembles a platform — and taking it from a different
    /// telemetry would produce a scrape surface that is empty while the loop
    /// records into another registry entirely.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// The registry the scrape surface serves from.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// The serialisable snapshot the health surface renders.
    pub fn view(&self, now: Timestamp) -> StatusView {
        let unready = self.unready(now);
        StatusView {
            node: "qip-deepbrain",
            roster_validated: self.roster_validated,
            roster_agents: self.roster_agents.clone(),
            excluded_agents: self.excluded_agents.clone(),
            agents_that_may_call_a_model: self.model_callers,
            reaches_a_venue: false,
            ready: unready.is_none(),
            unready_because: unready.map(Unready::as_str),
            started_at: self.started_at.as_secs(),
            uptime_secs: now.since(self.started_at).max(Duration::ZERO).as_secs_f64(),
            cycles: self.cycles,
            failed_cycles: self.failed_cycles,
            consecutive_failures: self.consecutive_failures,
            failure_tolerance: self.failure_tolerance,
            cycle_interval_secs: self.cycle_interval.as_secs_f64(),
            cycle_in_flight: self.cycle_in_flight(),
            last_cycle_finished_at: self.last_cycle_finished_at.map(Timestamp::as_secs),
            last_cycle_secs: self.last_cycle_elapsed.as_secs_f64(),
            longest_cycle_secs: self.longest_cycle_elapsed.as_secs_f64(),
            cycle_overruns: self.overruns,
            quiet_for_secs: self.quiet_for(now).as_secs_f64(),
            stall_ceiling_secs: self.stall_ceiling.as_secs_f64(),
            event_log: self.event_log.clone(),
            records_archived: self.archived,
            last_cycle_problems: self.last_cycle_problems.clone(),
            halted: self.halted,
            stopping: self.stopping,
        }
    }
}

/// The health response's body.
///
/// Durations are in seconds rather than the fast brain's microseconds, because
/// that is the scale this node works at and a reader comparing two numbers
/// should not have to divide first.
#[derive(Clone, Debug, Serialize)]
pub struct StatusView {
    pub node: &'static str,
    pub roster_validated: bool,
    pub roster_agents: Vec<String>,
    pub excluded_agents: Vec<String>,
    pub agents_that_may_call_a_model: usize,
    /// Always false, and reported rather than omitted: this is the node's
    /// central claim about itself, and a claim nobody can read is one nobody
    /// can check.
    pub reaches_a_venue: bool,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unready_because: Option<&'static str>,
    pub started_at: i64,
    pub uptime_secs: f64,
    pub cycles: u64,
    pub failed_cycles: u64,
    pub consecutive_failures: u32,
    pub failure_tolerance: u32,
    pub cycle_interval_secs: f64,
    pub cycle_in_flight: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle_finished_at: Option<i64>,
    pub last_cycle_secs: f64,
    pub longest_cycle_secs: f64,
    pub cycle_overruns: u64,
    pub quiet_for_secs: f64,
    pub stall_ceiling_secs: f64,
    pub event_log: String,
    pub records_archived: u64,
    pub last_cycle_problems: Vec<String>,
    pub halted: bool,
    pub stopping: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn roster() -> ClearedRoster {
        crate::roster::clear(start()).expect("the deployed roster clears")
    }

    fn status() -> NodeStatus {
        NodeStatus::opening(&roster(), &DeepBrainConfig::default(), start())
    }

    fn cycle(elapsed: Duration, traversed: bool) -> CycleRecord {
        CycleRecord {
            started_at: start(),
            finished_at: start(),
            elapsed,
            traversed_every_stage: traversed,
            problems: Vec::new(),
            archived: 3,
            halted: false,
        }
    }

    #[test]
    fn a_node_that_has_not_finished_a_cycle_yet_is_warming_rather_than_ready() {
        // The state the fast brain has no equivalent of: a process that is
        // answering probes and has produced nothing anybody can consult.
        let status = status();
        assert_eq!(status.cycles(), 0, "the premise: no cycle has completed");
        assert_eq!(status.unready(start()), Some(Unready::Warming));
        assert!(!status.is_ready(start()));
    }

    #[test]
    fn a_node_becomes_ready_the_moment_its_first_cycle_lands() {
        let mut status = status();
        status.cycle_started(start());
        status.cycle_finished(&cycle(Duration::from_secs(90), true));
        assert!(status.is_ready(start()));
        assert_eq!(status.cycles(), 1);
        assert_eq!(status.archived(), 3);
    }

    #[test]
    fn a_cycle_far_slower_than_the_interval_is_counted_and_is_never_a_reason_to_be_unready() {
        // The rule this node exists under. A ten-minute cycle against a
        // five-minute cadence is a deep analysis, and taking it out of rotation
        // would select against exactly the work this node is for.
        let mut status = status();
        for _ in 0..20 {
            status.cycle_finished(&cycle(Duration::from_mins(10), true));
        }
        assert_eq!(
            status.overruns(),
            20,
            "the overruns are not being counted, so nobody can see the node is behind its cadence"
        );
        assert!(
            status.is_ready(start()),
            "a slow deep brain reported itself unready; that is the fast brain's rule"
        );
        assert_eq!(status.unready(start()), None);
    }

    #[test]
    fn a_cycle_that_started_and_has_not_finished_is_visible_as_in_flight() {
        let mut status = status();
        status.cycle_started(start());
        assert!(
            status.cycle_in_flight(),
            "a started cycle with no finish must be visible; on this node it is the only way a \
             cycle blocked on a model call can be seen from outside"
        );
        status.cycle_finished(&cycle(Duration::from_secs(30), true));
        assert!(!status.cycle_in_flight());
    }

    #[test]
    fn a_node_whose_first_cycle_never_returns_reports_stalled_rather_than_warming_forever() {
        // The ordering in `unready`, asserted: `warming` an hour after start-up
        // would be a reassuring word for a wedged process.
        let status = status();
        let ceiling = status.stall_ceiling();
        let much_later = start().saturating_add(ceiling + Duration::from_mins(1));
        assert_eq!(status.unready(much_later), Some(Unready::Stalled));
    }

    #[test]
    fn the_stall_ceiling_clears_a_full_interval_plus_a_slow_cycle_at_the_default_cadence() {
        // The premise of the number: it has to be longer than a legitimate
        // worst case, or a thinking node gets restarted for thinking.
        let status = status();
        let interval = DeepBrainConfig::default().cycle_interval;
        assert!(
            status.stall_ceiling() > interval * 2,
            "the stall ceiling {:?} is not clear of two intervals at {interval:?}",
            status.stall_ceiling()
        );
        assert_eq!(status.stall_ceiling(), MINIMUM_STALL_CEILING);
    }

    #[test]
    fn one_failed_cycle_does_not_take_the_node_out_of_rotation_but_a_run_of_them_does() {
        let mut status = status();
        let tolerance = DeepBrainConfig::default().failure_tolerance;

        status.cycle_finished(&cycle(Duration::from_secs(30), false));
        assert!(
            status.is_ready(start()),
            "a single failed cycle is a source that timed out, not a broken node"
        );
        assert_eq!(status.failed_cycles(), 1);

        for _ in 0..tolerance {
            status.cycle_finished(&cycle(Duration::from_secs(30), false));
        }
        assert_eq!(
            status.unready(start()),
            Some(Unready::PersistentlyFailing),
            "after {} consecutive failures the node still called itself ready",
            tolerance + 1
        );
    }

    #[test]
    fn a_cycle_that_completes_clears_the_run_of_failures_but_not_the_total() {
        let mut status = status();
        for _ in 0..5 {
            status.cycle_finished(&cycle(Duration::from_secs(30), false));
        }
        assert!(!status.is_ready(start()));

        status.cycle_finished(&cycle(Duration::from_secs(30), true));
        assert!(
            status.is_ready(start()),
            "a node that recovered still reports itself unready"
        );
        assert_eq!(
            status.failed_cycles(),
            5,
            "the running total must survive recovery; it is the record that the loop was broken"
        );
    }

    #[test]
    fn a_stopping_node_reports_unready_before_it_stops_answering() {
        let mut status = status();
        status.cycle_finished(&cycle(Duration::from_secs(30), true));
        status.stopping();
        assert_eq!(status.unready(start()), Some(Unready::Stopping));
    }

    #[test]
    fn a_halted_node_is_unready_even_though_its_cycles_are_completing() {
        let mut status = status();
        let mut halted = cycle(Duration::from_secs(30), true);
        halted.halted = true;
        status.cycle_finished(&halted);
        assert_eq!(status.unready(start()), Some(Unready::Halted));
    }

    #[test]
    fn the_view_says_this_node_cannot_reach_a_venue_and_names_what_it_does_not_host() {
        let status = status();
        let view = status.view(start());
        assert!(!view.reaches_a_venue);
        assert!(view.roster_validated);
        assert!(
            view.excluded_agents
                .contains(&qip_investment_agents::manifests::ids::EXECUTION.to_string()),
            "the status does not say which agent this node refuses to host: {:?}",
            view.excluded_agents
        );
        assert!(
            view.agents_that_may_call_a_model > 0,
            "the status hides the property that makes a cycle's duration unpredictable"
        );
    }

    #[test]
    fn the_view_reports_the_longest_cycle_seen_and_not_only_the_last_one() {
        let mut status = status();
        status.cycle_finished(&cycle(Duration::from_mins(9), true));
        status.cycle_finished(&cycle(Duration::from_secs(30), true));
        assert_eq!(status.longest_cycle_elapsed(), Duration::from_mins(9));

        // Rendered in seconds rather than the fast brain's microseconds,
        // because that is the scale this node works at. Compared with a
        // tolerance because these are floats: an exact bit-for-bit match would
        // be asserting a coincidence rather than the property.
        let view = status.view(start());
        assert!((view.last_cycle_secs - 30.0).abs() < 1e-9);
        assert!((view.longest_cycle_secs - 540.0).abs() < 1e-9);
    }
}
