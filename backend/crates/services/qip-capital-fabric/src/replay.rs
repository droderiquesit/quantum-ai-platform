//! Rebuild the fabric's state from the event log, and nothing else.
//!
//! [`replay`] takes the log's records oldest-first and returns the
//! [`FabricState`] they build. It refuses — for the whole replay, naming the
//! position — any record that is out of sequence, that does not chain to its
//! predecessor, whose content no longer hashes to what the chain recorded,
//! that cannot be decoded, or whose recorded outcome disagrees with what the
//! control produces when the command is run again. It never skips: a replay
//! that stepped over a bad record and carried on would produce a state that
//! looks rebuilt from the log and is not, which is worse than no state at
//! all because it reads as evidence.
//!
//! # Why the outcome is recomputed rather than copied
//!
//! A hash chain proves that a record has not changed since it was written.
//! It does not prove the record was true when written. A gate record whose
//! chain verifies and whose outcome says "admitted" on inputs the seven
//! checks veto is exactly the record a compromised writer would produce, and
//! copying its outcome into the rebuilt state would launder it. So each
//! command is executed again, by the same code, against the state the
//! previous records built, and the recorded outcome is checked against the
//! recomputed one. That the two agree on every record is the property that
//! makes the log a source of truth rather than a diary.
//!
//! # The chain rule is restated here, and one test pins it to the log's
//!
//! `qip-events` computes a record's hash in a private function. This module
//! recomputes it from the same public parts ([`canonical_json`] and
//! [`sha256_hex`]) in [`chain_hash`], which is a second statement of one
//! rule. `the_journals_chain_rule_agrees_with_the_event_logs_own` in this
//! crate's tests holds the two together: it verifies one set of records under
//! both, so a change to the log's formula fails here before a replay could
//! silently accept or refuse the wrong thing.

use crate::journal::{FabricRecord, FabricState, PRODUCER};
use qip_core::error::{Error, Result};
use qip_core::sha256_hex;
use qip_events::envelope::canonical_json;
use qip_events::log::{GENESIS_HASH, LogRecord};
use qip_events::{AnyEvent, EventBody};

/// What a replay produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replayed {
    /// The state the fabric records built.
    pub state: FabricState,
    /// How many records were fabric decisions and were applied.
    pub applied: usize,
    /// How many records were on other topics or from other producers. They
    /// were chain-verified — a foreign record with a broken hash breaks the
    /// chain for every fabric record after it — and not decoded.
    pub passed_over: usize,
}

/// The hash the log commits a record under: its sequence, its predecessor's
/// hash and its canonical event.
pub fn chain_hash(sequence: u64, previous_hash: &str, event: &AnyEvent) -> Result<String> {
    let value = serde_json::to_value(event)?;
    let material = format!("{sequence}|{previous_hash}|{}", canonical_json(&value));
    Ok(sha256_hex(material.as_bytes()))
}

/// Rebuild the fabric's state from `records`, oldest first.
///
/// Refuses on the first record that fails any check, naming its position
/// (one-based, in the slice) and its sequence, and returns no state: a
/// partial state from a refused log is the thing this function exists not to
/// produce. The records must start at genesis — a slice taken from the
/// middle of a log cannot prove what came before it.
pub fn replay(records: &[LogRecord]) -> Result<Replayed> {
    let mut state = FabricState::new();
    let mut applied = 0usize;
    let mut passed_over = 0usize;
    let mut expected_previous = GENESIS_HASH.to_string();
    let mut last_sequence = 0u64;

    for (index, record) in records.iter().enumerate() {
        let position = index + 1;
        let expected_sequence = last_sequence + 1;
        if record.sequence != expected_sequence {
            return Err(Error::invalid(format!(
                "record at position {position} carries sequence {} but {expected_sequence} was \
                 expected after sequence {last_sequence}; the records are out of order or one \
                 is missing, and a replay does not reorder or skip",
                record.sequence
            )));
        }
        if record.event.sequence != record.sequence {
            return Err(Error::invalid(format!(
                "record at position {position} (sequence {}) carries an event stamped with \
                 sequence {}; the envelope and the chain disagree about where this record sits",
                record.sequence, record.event.sequence
            )));
        }
        if record.previous_hash != expected_previous {
            return Err(Error::invalid(format!(
                "record at position {position} (sequence {}) does not chain to its \
                 predecessor: it names previous hash {} where the chain has {expected_previous}; \
                 a record before it was altered or removed",
                record.sequence, record.previous_hash
            )));
        }
        let recomputed = chain_hash(record.sequence, &record.previous_hash, &record.event)?;
        if recomputed != record.record_hash {
            return Err(Error::invalid(format!(
                "record at position {position} (sequence {}) has been altered: its content \
                 hashes to {recomputed} and the chain recorded {}",
                record.sequence, record.record_hash
            )));
        }
        expected_previous = record.record_hash.clone();
        last_sequence = record.sequence;

        let is_fabric =
            record.event.topic == FabricRecord::TOPIC && record.event.lineage.producer == PRODUCER;
        if !is_fabric {
            passed_over += 1;
            continue;
        }
        let envelope = record.event.decode::<FabricRecord>().map_err(|err| {
            Error::invalid(format!(
                "record at position {position} (sequence {}) is a fabric record that cannot be \
                 decoded: {}",
                record.sequence,
                err.message()
            ))
        })?;
        let recorded = envelope.body;
        let recomputed = state.execute(recorded.command.clone());
        if recomputed.outcome != recorded.outcome {
            return Err(Error::denied(format!(
                "record at position {position} (sequence {}) records an outcome the control \
                 does not produce for its command: recorded {:?}, recomputed {:?}; the record \
                 was written by something other than the control, or against a state the log \
                 does not hold",
                record.sequence, recorded.outcome, recomputed.outcome
            )));
        }
        applied += 1;
    }

    Ok(Replayed {
        state,
        applied,
        passed_over,
    })
}
