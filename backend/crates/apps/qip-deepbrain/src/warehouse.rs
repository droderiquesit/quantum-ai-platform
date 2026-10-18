//! Resolved outcomes streamed to the research warehouse (R12-DATA-9054).
//!
//! The learning stage grades every thesis the platform published and keeps the
//! grades in a bounded window ([`qip_kernel::Platform::evaluations`]). The
//! window is a ring: a grade older than it is gone from memory, and the event
//! log holds the record but not a table a research query can join. The
//! warehouse (`qip_mesh::bigquery`) is that table, and until this module
//! nothing in a shipped binary constructed a [`BigQueryWarehouse`], so the
//! adapter, its dataset in `modules/data` and its alert wiring existed for
//! outcomes that were never written.
//!
//! # Off unless a deployment names a target
//!
//! [`OutcomeSink::from_lookup`] returns `None` when neither
//! `QIP_BIGQUERY_PROJECT` nor `QIP_BIGQUERY_DATASET` is set, which is every
//! environment today (`enable_bigquery = false`). It refuses when only one is
//! set, and refuses a target with no endpoint or credential, so a half-written
//! deployment stops the process rather than reading as a warehouse that is
//! quietly not there. There is no fallback sink: an outcome the warehouse
//! cannot take is held and offered again, never written somewhere else.
//!
//! # What is sent
//!
//! One row per grade, keyed by its hypothesis id so a retry after a lost
//! response is dropped by BigQuery rather than doubled. Only derived facts:
//! the grade's rationale is free text and stays in the event log. Rows the
//! warehouse rejects, and a batch that fails to send, stay unshipped and go
//! out again next cycle; the shipped set is pruned to the grades the window
//! still holds, so it is bounded by the window and not by the run.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_learning_engine::evaluation::Evaluation;
use qip_mesh::bigquery::{BigQueryConfig, BigQueryWarehouse, InsertRow};
use qip_storage::StorageTarget;
use qip_storage::gcp::{DATASET_VARIABLE, PROJECT_VARIABLE};
use qip_storage::managed::ManagedSettings;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The table the grades are streamed into. Created outside this code, with
/// the columns [`row`] writes.
pub const OUTCOMES_TABLE: &str = "resolved_outcomes";

/// Grades one cycle sends at most, so a backlog after an outage drains over
/// cycles instead of holding one cycle on a single large request.
pub const MAX_ROWS_PER_CYCLE: usize = 500;

/// What one cycle's shipping did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipRound {
    pub sent: usize,
    pub rejected: usize,
    /// Grades still unshipped after this cycle.
    pub held: usize,
}

impl ShipRound {
    pub fn describe(&self) -> String {
        format!(
            "warehouse: {} outcome(s) sent to {OUTCOMES_TABLE}, {} rejected, {} held",
            self.sent, self.rejected, self.held
        )
    }
}

/// The deep brain's sink for resolved outcomes.
#[derive(Debug)]
pub struct OutcomeSink {
    warehouse: BigQueryWarehouse,
    shipped: BTreeSet<String>,
}

fn present(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> bool {
    lookup(name).is_some_and(|value| !value.trim().is_empty())
}

impl OutcomeSink {
    /// A sink over a warehouse the caller built.
    pub fn new(warehouse: BigQueryWarehouse) -> Self {
        Self {
            warehouse,
            shipped: BTreeSet::new(),
        }
    }

    /// The sink a deployment asked for, or `None` when it asked for none.
    pub fn from_lookup(
        lookup: &dyn Fn(&str) -> Option<String>,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Result<Option<Self>> {
        let project = present(lookup, PROJECT_VARIABLE);
        let dataset = present(lookup, DATASET_VARIABLE);
        if !project && !dataset {
            return Ok(None);
        }
        // Either one set is a request for the warehouse, so a missing
        // half, endpoint or credential is a refusal that names it.
        let settings = ManagedSettings::from_env(StorageTarget::BigQuery, lookup)?;
        let config = BigQueryConfig::from_managed(&settings, clock)?;
        let warehouse = BigQueryWarehouse::new(config)?;
        if !warehouse.is_available() {
            return Err(qip_core::error::Error::invalid(format!(
                "configuration: the research warehouse is requested but cannot be reached: {}",
                warehouse.requirement()
            )));
        }
        Ok(Some(Self::new(warehouse)))
    }

    /// Grades handed in and not yet shipped.
    pub fn pending(&self, evaluations: &[Evaluation]) -> usize {
        evaluations
            .iter()
            .filter(|e| !self.shipped.contains(&e.hypothesis_id))
            .count()
    }

    /// Stream the grades in `evaluations` this sink has not shipped.
    ///
    /// A request that fails outright returns the error with nothing marked
    /// shipped; the caller reports it and the next cycle offers the same
    /// grades again.
    pub fn ship(&mut self, evaluations: &[Evaluation], now: Timestamp) -> Result<ShipRound> {
        let window: BTreeSet<&str> = evaluations
            .iter()
            .map(|e| e.hypothesis_id.as_str())
            .collect();
        self.shipped.retain(|id| window.contains(id.as_str()));
        let mut fresh: Vec<&Evaluation> = evaluations
            .iter()
            .filter(|e| !self.shipped.contains(&e.hypothesis_id))
            .collect();
        let backlog = fresh.len();
        fresh.truncate(MAX_ROWS_PER_CYCLE);
        if fresh.is_empty() {
            return Ok(ShipRound::default());
        }
        let rows: Vec<InsertRow> = fresh
            .iter()
            .map(|evaluation| InsertRow::with_id(&evaluation.hypothesis_id, row(evaluation, now)))
            .collect();
        let outcome = self.warehouse.insert(OUTCOMES_TABLE, rows)?;
        let rejected: BTreeSet<usize> = outcome.rejected().iter().map(|r| r.index).collect();
        for (index, evaluation) in fresh.iter().enumerate() {
            if !rejected.contains(&index) {
                self.shipped.insert(evaluation.hypothesis_id.clone());
            }
        }
        let sent = fresh.len() - rejected.len();
        Ok(ShipRound {
            sent,
            rejected: rejected.len(),
            held: backlog - sent,
        })
    }
}

/// One grade as a row. Statistics only; money never crosses here as `f64`
/// except where the grade itself already held it as one.
fn row(evaluation: &Evaluation, shipped_at: Timestamp) -> serde_json::Value {
    serde_json::json!({
        "hypothesis_id": evaluation.hypothesis_id,
        "class": evaluation.class,
        "subject": evaluation.subject,
        "verdict": evaluation.verdict.as_str(),
        "expected_move_bps": evaluation.expected_move_bps,
        "realised_move_bps": evaluation.realised_move_bps,
        "magnitude_ratio": evaluation.magnitude_ratio,
        "confidence": evaluation.confidence,
        "realised_pnl": evaluation.realised_pnl,
        "evaluated_at": evaluation.evaluated_at.to_rfc3339(),
        "shipped_at": shipped_at.to_rfc3339(),
    })
}
