//! The job that feeds the cost-anomaly detector (R12-FINOPS-10141): a billing
//! export and the declared thresholds in, one journaled task per breach out.
//!
//! [`crate::cost_anomaly_tasks`] journals anomalies it is handed and
//! `qip_observability::billing::detect` finds them in rows it is handed; until
//! this module nothing joined the two or read either input from the place a
//! scheduled export lands. The join is here so a binary only has to name two
//! files and a log, and so the wiring has a test that fails when it is
//! unplugged — a detector no caller reaches reads as protection and is not.
//!
//! **Refuse rather than guess.** An empty threshold list is refused: a job
//! with no thresholds finds nothing and reports success, which is the
//! silent-pass shape. Unparseable input is refused by name, never read as an
//! empty export.

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::log::EventLog;
use qip_observability::billing::{BillingRow, SpendThreshold, detect};

use crate::cost_anomaly_tasks::{Journaled, journal};

/// What one run of the job found and did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobReport {
    pub rows_read: usize,
    pub thresholds: usize,
    pub anomalies: usize,
    pub journaled: Journaled,
}

/// Run the job once over the export and thresholds, both JSON arrays.
///
/// Idempotent across runs: the anomaly fingerprint names the breach, so a
/// scheduled re-read of an export that has since gained late rows journals
/// nothing new for a breach the log already holds.
pub fn run(
    export_json: &str,
    thresholds_json: &str,
    log: &mut EventLog,
    now: Timestamp,
) -> Result<JobReport> {
    let rows: Vec<BillingRow> = serde_json::from_str(export_json).map_err(|e| {
        Error::invalid(format!(
            "the billing export is not a JSON array of rows: {e}; refusing rather than reading \
             it as an export with no spend"
        ))
    })?;
    let thresholds: Vec<SpendThreshold> = serde_json::from_str(thresholds_json).map_err(|e| {
        Error::invalid(format!(
            "the spend thresholds are not a JSON array of thresholds: {e}"
        ))
    })?;
    if thresholds.is_empty() {
        return Err(Error::invalid(
            "no spend thresholds are declared; a job with none finds nothing and would report \
             success, so declare at least one",
        ));
    }
    let anomalies = detect(&rows, &thresholds)?;
    let journaled = journal(log, &anomalies, now)?;
    Ok(JobReport {
        rows_read: rows.len(),
        thresholds: thresholds.len(),
        anomalies: anomalies.len(),
        journaled,
    })
}
