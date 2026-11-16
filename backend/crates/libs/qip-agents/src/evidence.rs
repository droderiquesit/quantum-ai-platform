//! The evidence an agent reads (R12-AMBIENT-9001, ADR 0638).
//!
//! The failure this prevents: research that reads from wherever it can reach
//! cannot be replayed or attributed. An agent reads evidence through one
//! port, [`EvidenceStore`], behind the [`Capability::ReadEvidence`] gate, with
//! a query that is validated rather than clamped and a row count that is
//! bounded. The port is the seam a BigQuery REST adapter implements under
//! ADR 0113 section 2; no adapter lives here, and no write method exists on
//! the port, so the warehouse is read-only from an agent by construction.

use crate::capability::Capability;
use crate::runtime::{AgentContext, Gated};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use std::sync::Arc;

/// Most rows one read returns. A query asking for more is refused, not cut:
/// a silently truncated read is a finding built on a sample nobody chose.
pub const MAX_EVIDENCE_ROWS: usize = 500;

/// One bounded read of one dataset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceQuery {
    dataset: String,
    subject: String,
    limit: usize,
}

impl EvidenceQuery {
    /// Refuses a blank dataset or subject, and a limit of zero or above
    /// [`MAX_EVIDENCE_ROWS`]. The dataset is a token, never SQL: whitespace
    /// or punctuation outside `[A-Za-z0-9_.-]` is refused so a query built
    /// from fetched text cannot carry a statement.
    pub fn new(dataset: &str, subject: &str, limit: usize) -> Result<Self> {
        let token_ok = |t: &str| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        };
        if !token_ok(dataset) {
            return Err(Error::invalid(
                "an evidence dataset is a non-empty token of letters, digits, `_`, `.` and `-`",
            ));
        }
        if subject.trim().is_empty() {
            return Err(Error::invalid("an evidence read names its subject"));
        }
        if limit == 0 || limit > MAX_EVIDENCE_ROWS {
            return Err(Error::invalid(format!(
                "an evidence read returns between 1 and {MAX_EVIDENCE_ROWS} rows; asked for {limit}"
            )));
        }
        Ok(Self {
            dataset: dataset.to_string(),
            subject: subject.trim().to_string(),
            limit,
        })
    }
    pub fn dataset(&self) -> &str {
        &self.dataset
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

/// One row of evidence and when it became knowable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceRow {
    pub id: String,
    pub knowable_at: Timestamp,
    pub body: String,
}

/// A read-only source of evidence. Implemented by the warehouse adapter.
pub trait EvidenceStore: Send + Sync + std::fmt::Debug {
    /// Rows for `query`, at most `query.limit()`, none knowable after `as_of`
    /// (a read that returns a row before its knowable instant leaks the
    /// future into a backtest).
    fn read(&self, query: &EvidenceQuery, as_of: Timestamp) -> Result<Vec<EvidenceRow>>;
}

/// The facility an agent host hands out: the store behind its capability.
pub type EvidenceDesk = Gated<Arc<dyn EvidenceStore>>;

/// Wrap `store` so it is reachable only with [`Capability::ReadEvidence`].
pub fn evidence_desk(store: Arc<dyn EvidenceStore>) -> EvidenceDesk {
    Gated::new(store, Capability::ReadEvidence, "evidence warehouse")
}

/// Read through the desk as the run at `now`. The access is recorded by the
/// gate; the rows returned are checked against the port's own contract, so a
/// store that over-returns or returns the future is refused rather than
/// trusted.
pub fn read_evidence(
    desk: &EvidenceDesk,
    ctx: &mut AgentContext,
    query: &EvidenceQuery,
) -> Result<Vec<EvidenceRow>> {
    let now = ctx.now();
    let store = desk.get(ctx)?;
    let rows = store.read(query, now)?;
    if rows.len() > query.limit() {
        return Err(Error::invalid(format!(
            "the evidence store returned {} rows for a limit of {}",
            rows.len(),
            query.limit()
        )));
    }
    if rows.iter().any(|row| row.knowable_at > now) {
        return Err(Error::invalid(
            "the evidence store returned a row knowable after the run's clock",
        ));
    }
    Ok(rows)
}
