//! `qip cost-anomalies`: read a billing export and the declared spend
//! thresholds, journal one task per breach (R12-FINOPS-10141).
//!
//! ```text
//! qip cost-anomalies --export <rows.json> --thresholds <thresholds.json> --log <event log>
//! ```
//!
//! The export is the file the scheduled billing-export read lands; this is the
//! composition root that hands it to `qip_kernel::cost_anomaly_job::run`, so
//! the detector has a caller that reaches the hash-chained log. A breach the
//! log already holds is reported as a duplicate and journaled no more.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::log::EventLog;
use qip_kernel::cost_anomaly_job::run as run_job;

fn flag(arguments: &[String], name: &str) -> Result<String> {
    arguments
        .iter()
        .position(|a| a == name)
        .and_then(|i| arguments.get(i + 1))
        .cloned()
        .ok_or_else(|| Error::invalid(format!("{name} <path> is required")))
}

fn read(path: &str, what: &str) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| Error::io(format!("cannot read the {what} at {path}: {e}")))
}

/// Run the command; the returned text is what `main` prints.
pub fn run(arguments: &[String], now: Timestamp) -> Result<String> {
    let export = read(&flag(arguments, "--export")?, "billing export")?;
    let thresholds = read(&flag(arguments, "--thresholds")?, "spend thresholds")?;
    let mut log = EventLog::open(flag(arguments, "--log")?)?;
    let report = run_job(&export, &thresholds, &mut log, now)?;
    Ok(format!(
        "cost-anomalies: {} row(s), {} threshold(s), {} breach(es), {} task(s) journaled, {} duplicate(s)",
        report.rows_read,
        report.thresholds,
        report.anomalies,
        report.journaled.written.len(),
        report.journaled.duplicates.len()
    ))
}
