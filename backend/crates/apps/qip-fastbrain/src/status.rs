//! What the node knows about itself, and what a probe reads.
//!
//! Held separately from the platform and updated by the run loop rather than
//! read out of it, for one reason: the health surface must be able to answer
//! *while a cycle is running*. A probe that has to take the same lock the loop
//! holds cannot distinguish a node that is working from a node that is wedged —
//! it blocks on both, and the timeout it eventually hits says the same thing
//! either way. This structure is small, cheap to clone and locked only for the
//! moment it takes to update, so the answer is always available and the
//! interesting case — a cycle that started and has not finished — is visible in
//! it rather than inferred from a hang.

use qip_core::{Duration, Timestamp};
use qip_observability::Metrics;
use serde::Serialize;
use std::sync::Arc;

use crate::config::FastBrainConfig;
use crate::roster::ClearedRoster;

/// The floor under the stall ceiling.
///
/// A node configured with a one-millisecond interval would otherwise be called
/// wedged for a twenty-millisecond pause, which is a scheduler, not a fault.
const MINIMUM_STALL_CEILING: Duration = Duration::from_secs(1);

/// How many cycle intervals may pass with nothing finishing before the node is
/// considered stalled.
const STALL_INTERVALS: i64 = 20;

/// Everything the node reports about itself.
#[derive(Clone, Debug)]
pub struct NodeStatus {
    /// Whether the roster check passed. It cannot be true unless it did: the
    /// only constructor takes a [`ClearedRoster`], which only the check makes.
    roster_validated: bool,
    roster_agents: Vec<String>,
    feed: String,
    feed_is_production_grade: bool,
    started_at: Timestamp,
    cycle_interval: Duration,
    cycle_budget: Duration,
    breach_tolerance: u32,
    stall_ceiling: Duration,

    cycles: u64,
    records_observed: u64,
    records_rejected: u64,
    last_cycle_started_at: Option<Timestamp>,
    last_cycle_finished_at: Option<Timestamp>,
    last_cycle_elapsed: Duration,
    worst_cycle_elapsed: Duration,
    breaches: u64,
    consecutive_breaches: u32,
    last_cycle_problems: Vec<String>,
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

/// Why the node is not ready to be sent work.
///
/// An enum rather than a boolean because "not ready" is the answer an operator
/// gets from a probe and "why" is the next question; a probe that can only say
/// no sends whoever is paged to read the logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unready {
    /// Shutting down. Correct, not a fault: a node on its way out should stop
    /// being sent work before it stops answering.
    Stopping,
    /// The kill switch is tripped.
    Halted,
    /// Cycles keep missing the fast-path ceiling. The node is alive and is not
    /// fast, which is the case a liveness probe cannot see.
    PersistentlyOverBudget,
    /// Nothing has finished in far longer than a cycle takes.
    Stalled,
}

impl Unready {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopping => "stopping",
            Self::Halted => "halted",
            Self::PersistentlyOverBudget => "persistently_over_budget",
            Self::Stalled => "stalled",
        }
    }
}

/// One cycle's result, as the status records it.
#[derive(Clone, Debug)]
pub struct CycleRecord {
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub elapsed: Duration,
    pub observed: usize,
    pub rejected: usize,
    pub problems: Vec<String>,
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
        config: &FastBrainConfig,
        feed: impl Into<String>,
        feed_is_production_grade: bool,
        started_at: Timestamp,
    ) -> Self {
        let ceiling = config.cycle_interval * STALL_INTERVALS;
        Self {
            roster_validated: true,
            roster_agents: roster.agents.iter().map(|a| a.id.clone()).collect(),
            feed: feed.into(),
            feed_is_production_grade,
            started_at,
            cycle_interval: config.cycle_interval,
            cycle_budget: config.cycle_budget,
            breach_tolerance: config.breach_tolerance,
            stall_ceiling: ceiling.max(MINIMUM_STALL_CEILING),
            cycles: 0,
            records_observed: 0,
            records_rejected: 0,
            last_cycle_started_at: None,
            last_cycle_finished_at: None,
            last_cycle_elapsed: Duration::ZERO,
            worst_cycle_elapsed: Duration::ZERO,
            breaches: 0,
            consecutive_breaches: 0,
            last_cycle_problems: Vec::new(),
            halted: false,
            stopping: false,
            metrics: Arc::new(Metrics::new("qip-fastbrain")),
        }
    }

    /// Note that a cycle has begun.
    ///
    /// Recorded before the work rather than after it, which is the whole point:
    /// a start with no matching finish is what a wedged node looks like from
    /// outside, and a status only written on completion can never show one.
    pub fn cycle_started(&mut self, at: Timestamp) {
        self.last_cycle_started_at = Some(at);
    }

    /// Fold a completed cycle in.
    pub fn cycle_finished(&mut self, record: &CycleRecord) {
        self.cycles += 1;
        self.records_observed += record.observed as u64;
        self.records_rejected += record.rejected as u64;
        self.last_cycle_started_at = Some(record.started_at);
        self.last_cycle_finished_at = Some(record.finished_at);
        self.last_cycle_elapsed = record.elapsed;
        if record.elapsed > self.worst_cycle_elapsed {
            self.worst_cycle_elapsed = record.elapsed;
        }
        if record.elapsed > self.cycle_budget {
            self.breaches += 1;
            self.consecutive_breaches = self.consecutive_breaches.saturating_add(1);
        } else {
            self.consecutive_breaches = 0;
        }
        self.last_cycle_problems.clone_from(&record.problems);
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

    pub fn breaches(&self) -> u64 {
        self.breaches
    }

    pub fn worst_cycle_elapsed(&self) -> Duration {
        self.worst_cycle_elapsed
    }

    pub fn records_observed(&self) -> u64 {
        self.records_observed
    }

    pub fn records_rejected(&self) -> u64 {
        self.records_rejected
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
    /// wedged on its very first cycle is stalled rather than merely new.
    pub fn quiet_for(&self, now: Timestamp) -> Duration {
        let last = self.last_cycle_finished_at.unwrap_or(self.started_at);
        now.since(last).max(Duration::ZERO)
    }

    /// Why this node should not be sent work, if it should not be.
    pub fn unready(&self, now: Timestamp) -> Option<Unready> {
        if self.stopping {
            return Some(Unready::Stopping);
        }
        if self.halted {
            return Some(Unready::Halted);
        }
        if self.consecutive_breaches > self.breach_tolerance {
            return Some(Unready::PersistentlyOverBudget);
        }
        if self.quiet_for(now) > self.stall_ceiling {
            return Some(Unready::Stalled);
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
            node: "qip-fastbrain",
            roster_validated: self.roster_validated,
            roster_agents: self.roster_agents.clone(),
            ready: unready.is_none(),
            unready_because: unready.map(Unready::as_str),
            feed: self.feed.clone(),
            feed_is_production_grade: self.feed_is_production_grade,
            started_at: self.started_at.as_secs(),
            uptime_secs: now.since(self.started_at).max(Duration::ZERO).as_secs_f64(),
            cycles: self.cycles,
            cycle_interval_ms: self.cycle_interval.as_millis(),
            cycle_budget_ms: self.cycle_budget.as_millis(),
            cycle_in_flight: self.cycle_in_flight(),
            last_cycle_finished_at: self.last_cycle_finished_at.map(Timestamp::as_secs),
            last_cycle_micros: self.last_cycle_elapsed.as_nanos() / 1_000,
            worst_cycle_micros: self.worst_cycle_elapsed.as_nanos() / 1_000,
            quiet_for_ms: self.quiet_for(now).as_millis(),
            stall_ceiling_ms: self.stall_ceiling.as_millis(),
            records_observed: self.records_observed,
            records_rejected: self.records_rejected,
            budget_breaches: self.breaches,
            consecutive_budget_breaches: self.consecutive_breaches,
            breach_tolerance: self.breach_tolerance,
            last_cycle_problems: self.last_cycle_problems.clone(),
            halted: self.halted,
            stopping: self.stopping,
        }
    }
}

/// The health response's body.
///
/// Durations are reported in whole units an operator can compare without
/// arithmetic: a cycle in microseconds because that is the scale this node
/// claims to work at, and everything else in milliseconds or seconds.
#[derive(Clone, Debug, Serialize)]
pub struct StatusView {
    pub node: &'static str,
    pub roster_validated: bool,
    pub roster_agents: Vec<String>,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unready_because: Option<&'static str>,
    pub feed: String,
    pub feed_is_production_grade: bool,
    pub started_at: i64,
    pub uptime_secs: f64,
    pub cycles: u64,
    pub cycle_interval_ms: i64,
    pub cycle_budget_ms: i64,
    pub cycle_in_flight: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle_finished_at: Option<i64>,
    pub last_cycle_micros: i64,
    pub worst_cycle_micros: i64,
    pub quiet_for_ms: i64,
    pub stall_ceiling_ms: i64,
    pub records_observed: u64,
    pub records_rejected: u64,
    pub budget_breaches: u64,
    pub consecutive_budget_breaches: u32,
    pub breach_tolerance: u32,
    pub last_cycle_problems: Vec<String>,
    pub halted: bool,
    pub stopping: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::ClearedAgent;

    fn roster() -> ClearedRoster {
        ClearedRoster {
            agents: vec![ClearedAgent {
                id: "microstructure-analyst".to_string(),
                wall_time: Duration::from_millis(5),
                tool_calls: 4,
            }],
            ceiling: Duration::from_millis(50),
        }
    }

    fn status() -> NodeStatus {
        NodeStatus::opening(
            &roster(),
            &FastBrainConfig::default(),
            "synthetic-exchange",
            false,
            Timestamp::from_secs(1_000),
        )
    }

    fn cycle(started: i64, elapsed_ms: i64) -> CycleRecord {
        CycleRecord {
            started_at: Timestamp::from_secs(started),
            finished_at: Timestamp::from_secs(started),
            elapsed: Duration::from_millis(elapsed_ms),
            observed: 7,
            rejected: 0,
            problems: Vec::new(),
            halted: false,
        }
    }

    #[test]
    fn a_node_that_has_just_started_and_run_a_cycle_is_ready() {
        let mut status = status();
        status.cycle_started(Timestamp::from_secs(1_000));
        status.cycle_finished(&cycle(1_000, 1));
        assert!(status.is_ready(Timestamp::from_secs(1_000)));
        assert_eq!(status.cycles(), 1);
        assert_eq!(status.records_observed(), 7);
    }

    #[test]
    fn a_cycle_that_started_and_has_not_finished_is_visible_as_in_flight() {
        let mut status = status();
        status.cycle_started(Timestamp::from_secs(1_000));
        assert!(
            status.cycle_in_flight(),
            "a started cycle with no finish must be visible; that is what a wedged node looks like"
        );
        status.cycle_finished(&cycle(1_000, 1));
        assert!(!status.cycle_in_flight());
    }

    #[test]
    fn a_node_whose_cycles_stopped_finishing_reports_itself_stalled_rather_than_ready() {
        let mut status = status();
        status.cycle_started(Timestamp::from_secs(1_000));
        status.cycle_finished(&cycle(1_000, 1));

        // The default interval is 100 ms, so the stall ceiling is the 1 s floor.
        let much_later = Timestamp::from_secs(1_060);
        assert_eq!(status.unready(much_later), Some(Unready::Stalled));
        assert!(!status.is_ready(much_later));
    }

    #[test]
    fn one_slow_cycle_does_not_take_the_node_out_of_rotation_but_a_run_of_them_does() {
        let mut status = status();
        let budget_ms = FastBrainConfig::default().cycle_budget.as_millis();
        let tolerance = FastBrainConfig::default().breach_tolerance;

        status.cycle_finished(&cycle(1_000, budget_ms + 1));
        assert!(
            status.is_ready(Timestamp::from_secs(1_000)),
            "a single slow cycle is a noisy neighbour, not a fault"
        );
        assert_eq!(status.breaches(), 1);

        for _ in 0..tolerance {
            status.cycle_finished(&cycle(1_000, budget_ms + 1));
        }
        assert_eq!(
            status.unready(Timestamp::from_secs(1_000)),
            Some(Unready::PersistentlyOverBudget),
            "after {} consecutive breaches the node still called itself ready",
            tolerance + 1
        );
    }

    #[test]
    fn a_cycle_inside_the_budget_clears_the_run_of_breaches_but_not_the_total() {
        let mut status = status();
        let budget_ms = FastBrainConfig::default().cycle_budget.as_millis();
        for _ in 0..5 {
            status.cycle_finished(&cycle(1_000, budget_ms + 1));
        }
        assert!(!status.is_ready(Timestamp::from_secs(1_000)));

        status.cycle_finished(&cycle(1_000, 1));
        assert!(
            status.is_ready(Timestamp::from_secs(1_000)),
            "a node that recovered still reports itself unready"
        );
        assert_eq!(
            status.breaches(),
            5,
            "the running total must survive recovery; it is the record that the ceiling was missed"
        );
    }

    #[test]
    fn a_stopping_node_reports_unready_before_it_stops_answering() {
        let mut status = status();
        status.cycle_finished(&cycle(1_000, 1));
        status.stopping();
        assert_eq!(
            status.unready(Timestamp::from_secs(1_000)),
            Some(Unready::Stopping)
        );
    }

    #[test]
    fn the_view_reports_the_worst_cycle_seen_and_not_only_the_last_one() {
        let mut status = status();
        status.cycle_finished(&cycle(1_000, 40));
        status.cycle_finished(&cycle(1_000, 1));
        let view = status.view(Timestamp::from_secs(1_000));
        assert_eq!(view.last_cycle_micros, 1_000);
        assert_eq!(view.worst_cycle_micros, 40_000);
        assert!(view.roster_validated);
    }
}
