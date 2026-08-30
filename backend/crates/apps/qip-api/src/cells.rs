//! What this process has heard from the edge cells, and when.
//!
//! A [`qip_kernel::CellReport`] is absorbed into the central plane's aggregate
//! and the report itself is not kept: the aggregate is the whole of every
//! cell's book, which is what the risk arithmetic needs. What it loses is the
//! one thing a console needs more than the numbers — *when* each cell last
//! spoke.
//!
//! Without that, a cell that stopped reporting an hour ago still contributes
//! its last book to the aggregate, and a page rendering the aggregate shows an
//! hour-old position as current. That is the specific failure this registry
//! exists to prevent, so it records the arrival time of every report and the
//! console reads staleness off it.
//!
//! It is a display concern and lives here rather than in the platform, for the
//! same reason the last cycle's stages do: the platform should not carry a
//! record it does not use.

use qip_core::time::{Duration, Timestamp};
use qip_kernel::CellReport;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// How old a cell report may be before the console stops presenting it as
/// current.
///
/// A minute. Cells report on their own cadence and the centre is explicitly
/// allowed to be slow, so this is not a liveness bound on the cell; it is the
/// point past which an operator should be told the number is old rather than
/// left to assume it is not. Chosen short because the cost of being told
/// wrongly that data is stale is a second glance, and the cost of the opposite
/// is trading on an hour-old book.
pub const CELL_REPORT_FRESHNESS: Duration = Duration::from_secs(60);

/// One cell, as this process last heard from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellObservation {
    pub cell: String,
    /// When the report was made, taken from the report rather than from a
    /// clock read on arrival: the cell's own as-of time is the one that says
    /// how old the book is.
    pub at: Timestamp,
    pub positions: usize,
    pub strategies: usize,
    pub reconciliation_breaks: usize,
}

impl CellObservation {
    /// How old this observation is at `now`.
    pub fn age(&self, now: Timestamp) -> Duration {
        now.since(self.at)
    }

    /// Whether the observation is too old to present as current.
    pub fn is_stale(&self, now: Timestamp, bound: Duration) -> bool {
        self.age(now) > bound
    }
}

/// Every cell this process has heard from.
#[derive(Debug)]
pub struct CellRegistry {
    observations: Mutex<BTreeMap<String, CellObservation>>,
    freshness: Duration,
}

impl Default for CellRegistry {
    fn default() -> Self {
        Self::new(CELL_REPORT_FRESHNESS)
    }
}

impl CellRegistry {
    pub fn new(freshness: Duration) -> Self {
        Self {
            observations: Mutex::new(BTreeMap::new()),
            freshness,
        }
    }

    /// The bound past which an observation is presented as stale.
    pub fn freshness_bound(&self) -> Duration {
        self.freshness
    }

    /// Record a report's arrival.
    ///
    /// Replaces rather than merges, matching the central plane: a report is
    /// the whole of that cell's book, and a leftover from a previous one would
    /// show up as risk nobody holds.
    pub fn record(&self, report: &CellReport) {
        if let Ok(mut observations) = self.observations.lock() {
            observations.insert(
                report.cell.clone(),
                CellObservation {
                    cell: report.cell.clone(),
                    at: report.at,
                    positions: report.positions.len(),
                    strategies: report.utilisation.len(),
                    reconciliation_breaks: report.reconciliation_breaks.len(),
                },
            );
        }
    }

    /// Every observation, ordered by cell.
    ///
    /// Returns owned values so the lock is released before anything renders. A
    /// rendering path holding this lock could stall report ingestion behind an
    /// HTML page.
    pub fn observations(&self) -> Vec<CellObservation> {
        self.observations
            .lock()
            .map(|observations| observations.values().cloned().collect())
            // A poisoned lock means a thread panicked while holding it.
            // Reporting nothing is the honest direction: the console will say
            // no cell has reported, which is better than showing a book whose
            // consistency is unknown.
            .unwrap_or_default()
    }

    /// Whether any cell has reported.
    pub fn is_empty(&self) -> bool {
        self.observations
            .lock()
            .map(|observations| observations.is_empty())
            .unwrap_or(true)
    }
}

/// Format a duration the way an operator reads one.
///
/// Coarse on purpose: the question a staleness label answers is "should I
/// believe this", and `1h 4m` answers it where `3847.221s` does not.
pub fn describe_age(age: Duration) -> String {
    let seconds = age.as_nanos() / 1_000_000_000;
    if seconds < 0 {
        // A report stamped in the future. Worth saying out loud rather than
        // rendering as a negative age: it means a cell's clock disagrees with
        // this process's.
        return format!("{}s in the future", -seconds);
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{}m {}s", minutes, seconds % 60);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{}h {}m", hours, minutes % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}
