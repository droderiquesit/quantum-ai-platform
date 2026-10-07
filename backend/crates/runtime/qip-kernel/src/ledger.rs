//! Ledger sink for P1 Financial Outcomes.
//!
//! This sink handles deduplication of P1 (durable, high-priority) financial
//! outcomes using the deterministic combination of event ID, sequence, and
//! producer epoch to enable idempotent replay and deduplication.

use qip_core::error::Result;

/// A ledger entry for a financial outcome event.
#[derive(Clone, Debug, PartialEq)]
pub struct LedgerEntry {
    /// Deterministic event ID from the event fabric.
    pub event_id: String,
    /// Sequence number within the event log.
    pub sequence: u64,
    /// Producer epoch, part of the dedup key.
    pub producer_epoch: u64,
}

/// Ledger sink that records P1 Financial Outcome events with deduplication.
pub struct LedgerSink {
    entries: std::collections::BTreeMap<String, LedgerEntry>,
}

impl LedgerSink {
    /// Create a new ledger sink.
    pub fn new() -> Self {
        Self {
            entries: std::collections::BTreeMap::new(),
        }
    }

    /// Record a financial outcome, deduplicating on (event_id, sequence, producer_epoch).
    pub fn record(&mut self, entry: LedgerEntry) -> Result<bool> {
        let dedup_key = format!(
            "{}_{}_{}",
            entry.event_id, entry.sequence, entry.producer_epoch
        );

        let is_new = !self.entries.contains_key(&dedup_key);
        self.entries.insert(dedup_key, entry);

        Ok(is_new)
    }

    /// Retrieve the count of recorded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for LedgerSink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_deduplicates_on_event_id_sequence_epoch() {
        let mut ledger = LedgerSink::new();
        let entry1 = LedgerEntry {
            event_id: "evt1".to_string(),
            sequence: 1,
            producer_epoch: 1,
        };
        let entry2 = LedgerEntry {
            event_id: "evt1".to_string(),
            sequence: 1,
            producer_epoch: 1,
        };

        assert!(ledger.record(entry1).unwrap()); // First is new
        assert!(!ledger.record(entry2).unwrap()); // Duplicate is not new
        assert_eq!(ledger.len(), 1);
    }
}
