//! Cost anomalies over the billing export, and the task each one raises
//! (R12-FINOPS-10141, FINOPS-018).
//!
//! A billing budget can e-mail somebody when a project crosses a figure. It
//! cannot say *which* labelled slice crossed it, and an e-mail is not a
//! record: nothing in the event log said that spend ran away, when, by how
//! much, or that anyone was asked to look. This module is the decision half
//! of that path and the adapter that journals it.
//!
//! * [`detect`] reads rows in the billing export's shape (project, labels,
//!   cost, currency, usage window) against declared [`SpendThreshold`]s and
//!   returns one [`CostAnomaly`] per threshold per window whose summed cost
//!   exceeds it. Pure: no clock, no socket, no environment.
//! * The adapter that journals each anomaly as one task into the hash-chained
//!   event log is `qip_kernel::cost_anomaly_tasks`: this crate is linked by
//!   the edge cell as an I/O-free library, so it does not link the log.
//!
//! **The fingerprint names the breach, not its size.** Billing exports are
//! revised: late rows arrive for a window already reported. A key that
//! included the amount would raise a second task for one runaway each time
//! the export caught up, and the desk would triage the same breach twice. The
//! fingerprint is the project, the threshold's selector and figure, the
//! currency and the window — what makes two reports the same breach.
//!
//! **Refuse rather than sum across a unit.** Rows in two currencies under one
//! threshold are refused by name: adding them is a number nobody can read.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, sha256_hex};
use serde::{Deserialize, Serialize};

/// One row of the billing export, in the columns detection reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingRow {
    pub project: String,
    pub service: String,
    pub labels: BTreeMap<String, String>,
    pub cost: Decimal,
    pub currency: String,
    pub usage_start: Timestamp,
    pub usage_end: Timestamp,
}

/// A declared ceiling on one project's spend, over rows whose labels carry
/// every pair in `selector`, per `window`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendThreshold {
    pub project: String,
    pub selector: BTreeMap<String, String>,
    pub threshold: Decimal,
    pub window: Duration,
}

/// Spend over a threshold in one window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostAnomaly {
    pub project: String,
    pub labels: BTreeMap<String, String>,
    pub amount: Decimal,
    pub threshold: Decimal,
    pub currency: String,
    pub window_start: Timestamp,
    pub window_end: Timestamp,
    /// Rows summed into `amount`.
    pub rows: usize,
    pub fingerprint: String,
}

impl SpendThreshold {
    fn validate(&self) -> Result<()> {
        if self.project.trim().is_empty() {
            return Err(Error::invalid(
                "a spend threshold names no project; name the billing project it bounds",
            ));
        }
        if !self.threshold.is_positive() {
            return Err(Error::invalid(format!(
                "the spend threshold for '{}' is {}, which no spend can be judged against; \
                 declare a positive figure",
                self.project, self.threshold
            )));
        }
        if self.window.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "the spend threshold for '{}' has no window; declare the period its figure \
                 bounds, such as one day",
                self.project
            )));
        }
        Ok(())
    }

    fn selects(&self, row: &BillingRow) -> bool {
        row.project == self.project
            && self
                .selector
                .iter()
                .all(|(key, value)| row.labels.get(key) == Some(value))
    }
}

/// The stable key of one breach: everything that makes two reports the same
/// breach, and not the amount.
pub fn fingerprint(
    project: &str,
    selector: &BTreeMap<String, String>,
    threshold: Decimal,
    currency: &str,
    window_start: Timestamp,
    window_end: Timestamp,
) -> String {
    let labels: Vec<String> = selector.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let canonical = format!(
        "project={project}\nlabels={}\nthreshold={threshold}\ncurrency={currency}\nwindow={}..{}",
        labels.join(","),
        window_start.as_nanos(),
        window_end.as_nanos()
    );
    sha256_hex(canonical.as_bytes())
}

/// Every window, under every threshold, whose summed cost exceeds it.
///
/// A row is assigned to the window its `usage_start` falls in, the windows
/// being aligned to the epoch so two runs agree on them. Output is ordered by
/// threshold, then window, so a rerun journals in the same order.
pub fn detect(rows: &[BillingRow], thresholds: &[SpendThreshold]) -> Result<Vec<CostAnomaly>> {
    for row in rows {
        if row.usage_end < row.usage_start {
            return Err(Error::invalid(format!(
                "a billing row for '{}' ends before it starts; the export is malformed, so \
                 correct or drop the row rather than guess its window",
                row.project
            )));
        }
    }
    let mut found = Vec::new();
    for threshold in thresholds {
        threshold.validate()?;
        let width = threshold.window.as_nanos();
        let mut windows: BTreeMap<i64, (Decimal, String, usize)> = BTreeMap::new();
        for row in rows.iter().filter(|row| threshold.selects(row)) {
            let start = row.usage_start.as_nanos().div_euclid(width) * width;
            let entry = windows
                .entry(start)
                .or_insert_with(|| (Decimal::ZERO, row.currency.clone(), 0));
            if entry.1 != row.currency {
                return Err(Error::invalid(format!(
                    "rows for '{}' in one window are billed in both {} and {}; spend in two \
                     currencies cannot be summed against one threshold, so declare a threshold \
                     per currency",
                    threshold.project, entry.1, row.currency
                )));
            }
            entry.0 += row.cost;
            entry.2 += 1;
        }
        for (start, (amount, currency, count)) in windows {
            if amount > threshold.threshold {
                let window_start = Timestamp::from_nanos(start);
                let window_end = Timestamp::from_nanos(start.saturating_add(width));
                found.push(CostAnomaly {
                    fingerprint: fingerprint(
                        &threshold.project,
                        &threshold.selector,
                        threshold.threshold,
                        &currency,
                        window_start,
                        window_end,
                    ),
                    project: threshold.project.clone(),
                    labels: threshold.selector.clone(),
                    amount,
                    threshold: threshold.threshold,
                    currency,
                    window_start,
                    window_end,
                    rows: count,
                });
            }
        }
    }
    Ok(found)
}
