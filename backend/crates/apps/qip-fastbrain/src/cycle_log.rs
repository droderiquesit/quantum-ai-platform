//! A bounded stdout line for the decision loop.
//!
//! The node used to print every cycle's report and every rejected record. On
//! Cloud Run stdout is Cloud Logging ingestion, so an always-on fast brain at
//! the default 100 ms cycle wrote about ten per-decision records a second into
//! general-purpose logs, and a tape replay wrote one per knowable instant.
//! Decision history belongs in the event log and the metrics; stdout carries
//! at most one summary line per interval, whatever the cycle count.

use qip_core::{Duration, Timestamp};

/// Accumulates cycles and releases one summary line per `interval`.
#[derive(Debug)]
pub struct CycleLog {
    interval: Duration,
    last_emit: Option<Timestamp>,
    cycles: u64,
    rejected: u64,
    breaches: u64,
    worst: Duration,
}

impl CycleLog {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit: None,
            cycles: 0,
            rejected: 0,
            breaches: 0,
            worst: Duration::ZERO,
        }
    }

    /// Record one cycle; returns a line only when `interval` has passed since
    /// the last one (the first cycle always reports, so a start is visible).
    pub fn observe(
        &mut self,
        now: Timestamp,
        cycle: u64,
        rejected: usize,
        over_budget: bool,
        elapsed: Duration,
    ) -> Option<String> {
        self.cycles += 1;
        self.rejected += rejected as u64;
        self.breaches += u64::from(over_budget);
        if elapsed > self.worst {
            self.worst = elapsed;
        }
        let due = self
            .last_emit
            .is_none_or(|last| now.since(last) >= self.interval);
        if !due {
            return None;
        }
        let line = format!(
            "cycle {cycle}: {} cycle(s) since last line, {} record(s) rejected, {} budget breach(es), worst {}us",
            self.cycles,
            self.rejected,
            self.breaches,
            self.worst.as_nanos() / 1_000
        );
        self.last_emit = Some(now);
        self.cycles = 0;
        self.rejected = 0;
        self.breaches = 0;
        self.worst = Duration::ZERO;
        Some(line)
    }
}
