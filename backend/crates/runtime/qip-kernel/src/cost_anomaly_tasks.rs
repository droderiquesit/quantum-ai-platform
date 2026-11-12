//! The task adapter for cost anomalies (R12-FINOPS-10141, FINOPS-018): each
//! [`CostAnomaly`] that `qip_observability::billing::detect` finds in a
//! billing export becomes one [`CostAnomalyTask`] in the hash-chained event
//! log, keyed on the anomaly's fingerprint, and a second report of the same
//! breach is refused rather than journaled twice.
//!
//! Here rather than beside the detector because the detector's crate is
//! linked by the edge cell as an I/O-free library; the log is not.

use qip_core::error::Result;
use qip_core::ids::EventId;
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::time::Timestamp;
use qip_events::envelope::{Envelope, EventBody};
use qip_events::log::EventLog;
use qip_events::topic::Topic;
use qip_observability::billing::CostAnomaly;
use serde::{Deserialize, Serialize};

/// The producer every journaled task names.
pub const PRODUCER: &str = "qip-kernel/cost-anomaly-tasks";

/// The idempotency prefix of a journaled task.
pub const IDEMPOTENCY_PREFIX: &str = "cost-anomaly:";

/// The task one anomaly raises, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostAnomalyTask {
    pub anomaly: CostAnomaly,
    /// What the task asks for, in a sentence a person can act on.
    pub ask: String,
}

impl EventBody for CostAnomalyTask {
    const TOPIC: Topic = Topic::SystemAlert;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.anomaly.fingerprint))
    }
}

/// What [`journal`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journaled {
    /// Sequence numbers of the tasks written, in anomaly order.
    pub written: Vec<u64>,
    /// Fingerprints refused because the log already holds their task.
    pub duplicates: Vec<String>,
}

/// Journal one task per anomaly, skipping any whose fingerprint the log
/// already holds. Each task is a root event under its own correlation, the
/// fingerprint, so every later event about the breach can name it.
pub fn journal(log: &mut EventLog, anomalies: &[CostAnomaly], now: Timestamp) -> Result<Journaled> {
    let mut outcome = Journaled::default();
    for anomaly in anomalies {
        let task = CostAnomalyTask {
            ask: format!(
                "spend on project '{}' was {} {} against a threshold of {} in the window from \
                 {} to {}; find what drove it and either stop it or raise the threshold through \
                 review",
                anomaly.project,
                anomaly.amount,
                anomaly.currency,
                anomaly.threshold,
                anomaly.window_start.to_rfc3339(),
                anomaly.window_end.to_rfc3339()
            ),
            anomaly: anomaly.clone(),
        };
        let envelope = Envelope::new(
            EventId::from_string(format!("{IDEMPOTENCY_PREFIX}{}", anomaly.fingerprint)),
            anomaly.window_end,
            now,
            Lineage::root(
                CorrelationId::from_string(anomaly.fingerprint.clone()),
                PRODUCER,
            ),
            task,
        );
        let event = envelope.erase()?;
        if log.holds_idempotent(&event.dedup_key()) {
            outcome.duplicates.push(anomaly.fingerprint.clone());
            continue;
        }
        outcome.written.push(log.append(&event)?);
    }
    Ok(outcome)
}
