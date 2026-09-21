//! Rebuild the fabric's state from the event log, and nothing else.
//!
//! [`replay`] takes the log's records oldest-first and returns the
//! [`FabricState`] they build. It refuses — for the whole replay, naming the
//! position — any record that is out of sequence, that does not chain to its
//! predecessor, whose content no longer hashes to what the chain recorded,
//! that was written under a schema version other than this build's, that
//! cannot be decoded, or whose recorded outcome disagrees with what the
//! control produces when the command is run again.
//!
//! The schema check is stated here rather than left to serde. `AnyEvent::decode`
//! refuses only a *newer* version, so a version 1 fabric record — written
//! before a gate record carried the Intelligence layer's funding ruling — was
//! refused only because `GateCommand::funding` has no `#[serde(default)]` and
//! `CorridorFunding` derives no `Default`. That is a real refusal resting on
//! an accident: either of those two lines arriving later would silently
//! re-admit version 1 records and assess them as though the corridor had been
//! ruled on. An explicit check names the reason in the error instead, and
//! `a_fabric_record_written_under_the_old_schema_is_refused_by_name` pins both
//! halves.
//!
//! Version 2 has no such accident to lean on and is the reason the check must
//! stay. Every field of a version 2 record still deserialises under this
//! build; what changed with ADR 0051 is the *meaning* of two attestation
//! references, which version 2 filled with filing notes because nothing read
//! them. Re-running the control over one produces a veto — the references do
//! not match the assessment identity and the policy fingerprint — and a veto
//! in the log reads as a movement the gate declined, not as a record this
//! build cannot judge. Refusing by version is the difference between those
//! two findings.
//!
//! It never skips: a replay
//! that stepped over a bad record and carried on would produce a state that
//! looks rebuilt from the log and is not, which is worse than no state at
//! all because it reads as evidence.
//!
//! # A rolled log is replayed; a shortened one is refused
//!
//! The one thing a replay does cross is a gap the log itself made. The event
//! log bounds its index by age as well as by count, and the roll lifts
//! replaceable records out of the interior without re-chaining what is left
//! — so a log that has rolled holds a retained span whose sequences are not
//! contiguous, and every record in it is still exactly the record that was
//! written. [`replay_from`] crosses such a gap by re-anchoring on the
//! predecessor the next record names, the same move
//! [`qip_events::log::EventLog::verify_retained_chain`] makes and at the same
//! trust level.
//!
//! It crosses it **only** against the log's own account of what it spent
//! ([`qip_events::log::EventLog::retained_anchor`]), never against the
//! records' own word, and that distinction is the whole safety of the thing.
//! Nothing in a surviving record says whether the sequence before it was
//! rolled or removed, and the two are the difference between a bounded
//! working set and a ledger silently missing its history. The log knows: it
//! counts every eviction and every roll, and no path it counts can take a
//! permanent record — a fabric record's retention class is irreplaceable, so
//! the fabric's own history is never what a gap holds. When more sequences
//! are missing than the log ever dropped, the replay refuses.
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
use qip_events::log::{LogRecord, RetainedAnchor};
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
    /// How many sequences were missing from the span and re-anchored across
    /// rather than read as a break: the records the log rolled or evicted
    /// from the interior. Reported rather than swallowed because a replay
    /// that crossed a gap and a replay that crossed none are different
    /// findings about the same log, and only one of them is standing on the
    /// log's own account of what it spent.
    pub re_anchored: u64,
}

/// The hash the log commits a record under: its sequence, its predecessor's
/// hash and its canonical event.
pub fn chain_hash(sequence: u64, previous_hash: &str, event: &AnyEvent) -> Result<String> {
    let value = serde_json::to_value(event)?;
    let material = format!("{sequence}|{previous_hash}|{}", canonical_json(&value));
    Ok(sha256_hex(material.as_bytes()))
}

/// Rebuild the fabric's state from `records`, oldest first, holding the first
/// of them to genesis.
///
/// What a caller holding a bare slice must ask, because a slice carries no
/// account of what is missing from it. A caller holding the log itself asks
/// [`replay_from`] with [`qip_events::log::EventLog::retained_anchor`], and
/// gets the same answer over a span the log has rolled.
pub fn replay(records: &[LogRecord]) -> Result<Replayed> {
    replay_from(records, &RetainedAnchor::genesis())
}

/// Rebuild the fabric's state from `records`, oldest first, against the
/// anchor the log they came from gives for its retained span.
///
/// Refuses on the first record that fails any check, naming its position
/// (one-based, in the slice) and its sequence, and returns no state: a
/// partial state from a refused log is the thing this function exists not to
/// produce.
///
/// # Why an anchor rather than genesis
///
/// The log bounds its index by age as well as by size — `qip-events`'
/// snapshot window — and a roll takes replaceable records out of the
/// interior without re-chaining what is left. Until 2026-09-21 this replay
/// demanded sequences contiguous from one, so a log that had rolled a
/// ninety-day-old book snapshot could not be resumed at all. That is why the
/// window shipped with no production caller, and why turning it on without
/// this change would have lost a ledger rather than bounded one: three
/// `qip-cli replay` runs refused a one-cycle journal on 2026-09-19 for
/// exactly this reason.
///
/// A gap is now crossed by re-anchoring the record after it on the
/// predecessor *it* names, exactly as
/// [`qip_events::log::EventLog::verify_retained_chain`] does and at the same
/// trust level — no new assumption, because the record's own hash is
/// recomputed either way.
///
/// What is **not** taken on the records' word is whether the gap belongs
/// there. A span beginning at sequence 412 reads identically whether the log
/// rolled 411 records or somebody removed them, and the second is a ledger
/// missing history it reports as complete — the worst outcome available
/// here, worse than refusing to start. So the anchor comes from the log,
/// which counts what it spent and whose every spending path skips a
/// permanent record, and the sequences missing from the span may never
/// exceed that count. Past it this refuses and names the arithmetic, because
/// retention cannot account for the difference and nothing else here can.
pub fn replay_from(records: &[LogRecord], anchor: &RetainedAnchor) -> Result<Replayed> {
    let mut state = FabricState::new();
    let mut applied = 0usize;
    let mut passed_over = 0usize;
    let mut expected_previous = anchor.previous_hash().to_string();
    let mut last_sequence = anchor.first_sequence().saturating_sub(1);
    // Sequences the span does not hold, counted from the anchor's own start
    // so that a head the log rolled past is charged against the same budget
    // an interior gap is. A genesis anchor starts this at zero and has a
    // budget of zero, which is what makes `replay` the strict question.
    let mut missing = last_sequence;

    for (index, record) in records.iter().enumerate() {
        let position = index + 1;
        let expected_sequence = last_sequence + 1;
        if record.sequence > expected_sequence {
            missing = missing.saturating_add(record.sequence - expected_sequence);
            if missing > anchor.dropped() {
                return Err(Error::invalid(format!(
                    "record at position {position} carries sequence {} where {expected_sequence} \
                     was expected, and the log these records came from accounts for dropping \
                     only {} record(s) against the {missing} sequence(s) missing from the span; \
                     retention did not take the difference, so a record was reordered or \
                     removed rather than rolled or evicted, and a replay does not reorder or \
                     skip. A ledger rebuilt across that gap would be missing history it reports \
                     as complete, which is why this refuses instead. Replay the log itself \
                     rather than a slice of it, or restore the file the chain was written over",
                    record.sequence,
                    anchor.dropped(),
                )));
            }
            // The gap is inside what the log says it spent, and no retention
            // path in that log can spend a permanent record — a fabric
            // record's class is irreplaceable — so nothing the fabric needs
            // was in it. Re-anchor on what this record claims, as the head is
            // anchored.
            expected_previous = record.previous_hash.clone();
        } else if record.sequence != expected_sequence {
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
        if record.event.schema_version != FabricRecord::SCHEMA_VERSION {
            return Err(Error::invalid(format!(
                "record at position {position} (sequence {}) is a fabric record written under \
                 schema version {}, and this build reads version {}; a record from another \
                 version is refused by name rather than re-interpreted, because what it lacks is \
                 what a control ruled on — a version 1 gate record carries no funding ruling, and \
                 a version 2 gate record's transfer-gate and custody-policy attestations name \
                 nothing checkable, because nothing checked them when it was written, and a \
                 version 3 wallet reconciliation was judged against one tolerance constant per \
                 asset rather than against §38.3's formula per venue-asset. Re-running the \
                 control over any of them would produce a verdict about a question the record \
                 was never made to answer — or, for version 3, the same question answered by a \
                 different rule, which can turn a recorded halt into a clean book and back \
                 again — and neither is the same finding as a record this build cannot judge",
                record.sequence,
                record.event.schema_version,
                FabricRecord::SCHEMA_VERSION,
            )));
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
        re_anchored: missing,
    })
}
