//! One record per reconciliation break that halted a cell
//! (R12-EXPAND-6066, ADR 0193).
//!
//! `CentralPlane::record_halt` only counted the breaks. A count says a cell
//! halted; it does not say which records disagreed, so the deep brain's
//! research had nothing to raise an execution gap on and nobody could ask
//! afterwards which instruments broke. The platform journals each break
//! that halted a cell here, bounded per report, and the deep brain reads
//! them back off the log.

use crate::central::ReconciliationBreak;
use qip_core::Timestamp;
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

/// The lineage origin break records are journaled under.
pub const RECONCILIATION_BREAK_ORIGIN: &str = "kernel/reconciliation-break";

/// What every break record's idempotency key starts with.
pub const RECONCILIATION_BREAK_PREFIX: &str = "reconciliation-break:";

/// Break records journaled per halting report. A report past it journals
/// the first this many and the halt itself still happens; a cell that
/// breaks on hundreds of records at once is one incident, and its first
/// records already name it.
pub const MAX_BREAK_RECORDS_PER_REPORT: usize = 64;

/// One break that halted a cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReconciliationBreakRecord {
    /// `<cell>@<report instant in nanoseconds>#<index>`: unique per break
    /// per report, and stable across a replay of the same report.
    pub record_id: String,
    pub cell: String,
    pub instrument: String,
    /// `BreakDirection::as_str` of the break.
    pub direction: String,
    pub detail: String,
    /// The instant the report was made.
    pub at: Timestamp,
}

impl EventBody for ReconciliationBreakRecord {
    const TOPIC: Topic = Topic::ReconciliationCompleted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{RECONCILIATION_BREAK_PREFIX}{}", self.record_id))
    }
}

/// The records a halting report's breaks make, at most
/// [`MAX_BREAK_RECORDS_PER_REPORT`] of them.
pub fn records_of(
    cell: &str,
    breaks: &[ReconciliationBreak],
    at: Timestamp,
) -> Vec<ReconciliationBreakRecord> {
    breaks
        .iter()
        .take(MAX_BREAK_RECORDS_PER_REPORT)
        .enumerate()
        .map(|(index, found)| ReconciliationBreakRecord {
            record_id: format!("{cell}@{}#{index}", at.as_nanos()),
            cell: cell.to_string(),
            instrument: found.instrument.clone(),
            direction: found.direction().as_str().to_string(),
            detail: found.detail.clone(),
            at,
        })
        .collect()
}
