//! The event log's hash chain, kept across restarts.
//!
//! [`qip_events::log::EventLog`] is hash-chained and can mirror itself to a
//! JSONL file, but the platform builds its log in memory and the chain
//! therefore begins again at every start. The audit trail that matters is the
//! one spanning every run of the process, and this is where it lives.
//!
//! # Why the archive chains again rather than reusing the log's chain
//!
//! A restarted process starts a fresh log whose sequences begin at 1 and whose
//! first record chains onto the genesis hash. Keying the archive by the source
//! sequence would make the second run overwrite the first, one record at a
//! time, and the result would still verify — every record present would be
//! internally consistent, and an entire run of the platform would be gone with
//! nothing to show it had ever been there. That is the worst shape a data loss
//! can take, so the archive keeps its own dense position and its own linkage
//! over the source records' hashes: run two appends after run one, and a
//! missing entry anywhere breaks the chain at that point.
//!
//! # What it does not do
//!
//! * **It does not make the source log durable.** A record is archived when a
//!   caller hands it over, so a crash between the append and the hand-over
//!   loses it. Closing that gap means writing through the archive on every
//!   append, which puts a disk on the path of every event the platform emits.
//!   Callers hand over at a cycle boundary instead, and what is lost is at
//!   most the events of the cycle that was interrupted.
//! * **It does not trim.** An archive that expired its own entries would be
//!   deleting the evidence it exists to hold. Retention is an operator's
//!   decision about the underlying store, not a default in this code.

use crate::kv::{KeyValueStore, KeyValueStoreExt};
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_events::envelope::canonical_json;
use qip_events::log::{GENESIS_HASH, LogRecord};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// The key prefix every archived record sits under.
const KEY_PREFIX: &str = "chain/";

/// Width of the zero-padded position in a key.
///
/// Twenty digits holds `u64::MAX`. The padding is not cosmetic: the key-value
/// port promises keys back in *lexicographic* order, so an unpadded `10` sorts
/// before `9` and the chain would be replayed out of order — and, because each
/// entry carries its own predecessor's digest, it would then fail verification
/// for a reason that has nothing to do with what happened to the data.
const POSITION_WIDTH: usize = 20;

/// One source record, at its position in the archive's own chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArchivedRecord {
    /// Dense from zero, across every run of the process.
    pub position: u64,
    /// The digest of the entry before this one, or [`GENESIS_HASH`] for the
    /// first entry the archive ever held.
    pub previous_digest: String,
    /// `sha256(previous_digest | position | the whole canonicalised record)`.
    ///
    /// Over the whole record, not over the record's own `record_hash`. Hashing
    /// the hash looks equivalent and is not: the source hash is stored inside
    /// the same entry, so an edit that changes the event and leaves the hash
    /// alone produces a record that disagrees with itself and an archive chain
    /// that still verifies. Committing to the serialized record closes that,
    /// at the cost of one canonicalisation per entry on a path that is already
    /// writing to a disk.
    pub digest: String,
    /// The record as the source log wrote it, its own chain linkage included,
    /// so an archived entry is still checkable against the log it came from.
    pub record: LogRecord,
}

/// An append-only, hash-chained archive of an event log, on any
/// [`KeyValueStore`].
///
/// Shared behind an `Arc` by request handlers, so the interior state is behind
/// a lock rather than requiring `&mut`. The lock is held across the store
/// write on purpose: two threads appending concurrently would otherwise both
/// read the same tail digest and write two entries claiming the same
/// predecessor, which is a fork rather than a chain.
#[derive(Debug)]
pub struct ChainArchive {
    store: Arc<dyn KeyValueStore>,
    state: Mutex<ArchiveState>,
}

#[derive(Debug)]
struct ArchiveState {
    next_position: u64,
    tail_digest: String,
    /// The highest source sequence this process has already handed over.
    ///
    /// Reset to zero on open rather than read from the archive, because a
    /// restarted process has a fresh source log whose sequence 1 is a genuinely
    /// new record and not the one already archived under the same number.
    absorbed_through: u64,
}

impl ChainArchive {
    /// Open an archive over `store`, continuing whatever chain it already holds.
    pub fn open(store: Arc<dyn KeyValueStore>) -> Result<Self> {
        let keys = store.keys_with_prefix(KEY_PREFIX)?;
        let state = match keys.last() {
            None => ArchiveState {
                next_position: 0,
                tail_digest: GENESIS_HASH.to_string(),
                absorbed_through: 0,
            },
            Some(key) => {
                let tail: ArchivedRecord = store.get_as(key)?.ok_or_else(|| {
                    Error::io(format!(
                        "the archive lists key {key} but does not hold it; the store is \
                         inconsistent and continuing would append onto a chain whose tail is \
                         unknown"
                    ))
                })?;
                ArchiveState {
                    next_position: tail.position.saturating_add(1),
                    tail_digest: tail.digest,
                    absorbed_through: 0,
                }
            }
        };
        Ok(Self {
            store,
            state: Mutex::new(state),
        })
    }

    /// How many records the archive holds, without reading the store.
    ///
    /// Exact rather than approximate: positions are dense from zero, so the
    /// next position *is* the count. It exists because [`ChainArchive::len`]
    /// scans every key, and a status endpoint that a monitor polls every few
    /// seconds must not do that — a health check whose cost grows with the
    /// audit trail is a health check that fails first on the busiest system.
    pub fn records_archived(&self) -> u64 {
        self.locked().next_position
    }

    /// How many records the archive holds, read back from the store.
    ///
    /// The ground truth, and the one to use when what is being checked is the
    /// store itself rather than this process's view of it.
    pub fn len(&self) -> Result<usize> {
        Ok(self.store.keys_with_prefix(KEY_PREFIX)?.len())
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// The digest the next appended entry will chain onto.
    pub fn tail_digest(&self) -> String {
        self.locked().tail_digest.clone()
    }

    /// The highest source sequence handed over since this archive was opened.
    pub fn absorbed_through(&self) -> u64 {
        self.locked().absorbed_through
    }

    /// Archive every record the caller has not handed over yet.
    ///
    /// Takes the log's whole record slice rather than a delta, because that is
    /// what [`qip_events::log::EventLog::records`] hands out and asking each
    /// caller to compute the delta would put the same off-by-one in every
    /// binary. Records at or below [`ChainArchive::absorbed_through`] are
    /// skipped, so calling this twice with the same slice writes nothing the
    /// second time.
    ///
    /// Filtering by sequence rather than by position also survives the source
    /// log evicting its oldest records under a capacity bound: eviction only
    /// removes from the front, so everything above the watermark is still
    /// exactly the set not yet archived.
    pub fn absorb(&self, records: &[LogRecord]) -> Result<usize> {
        let mut state = self.locked();
        let mut written = 0;
        for record in records {
            if record.sequence <= state.absorbed_through {
                continue;
            }
            let position = state.next_position;
            let digest = entry_digest(&state.tail_digest, position, record)?;
            let entry = ArchivedRecord {
                position,
                previous_digest: state.tail_digest.clone(),
                digest: digest.clone(),
                record: record.clone(),
            };
            // The store write happens before the in-memory tail moves, so a
            // failed write leaves the archive and this state agreeing about
            // where the chain ends. The other order would advance past an
            // entry that was never written and silently orphan the next one.
            self.store.put_as(&key_for_position(position), &entry)?;
            state.next_position = position.saturating_add(1);
            state.tail_digest = digest;
            state.absorbed_through = record.sequence;
            written += 1;
        }
        Ok(written)
    }

    /// Every archived record, in chain order.
    pub fn records(&self) -> Result<Vec<ArchivedRecord>> {
        let mut out = Vec::new();
        for key in self.store.keys_with_prefix(KEY_PREFIX)? {
            let entry: ArchivedRecord = self.store.get_as(&key)?.ok_or_else(|| {
                Error::io(format!("the archive lists key {key} but does not hold it"))
            })?;
            out.push(entry);
        }
        Ok(out)
    }

    /// The archive position where the chain first breaks, or `None` when intact.
    ///
    /// The read failure is kept separate from the verdict on purpose: `Err`
    /// means the archive could not be checked at all, and collapsing that into
    /// the broken case would report an unreadable disk as evidence of
    /// tampering — which is the sort of wrong answer that sends an incident
    /// review in the wrong direction for a day.
    pub fn first_broken_position(&self) -> Result<Option<u64>> {
        let mut previous = GENESIS_HASH.to_string();
        let mut expected_position = 0u64;
        for entry in self.records()? {
            if entry.position != expected_position
                || entry.previous_digest != previous
                || entry.digest != entry_digest(&previous, entry.position, &entry.record)?
            {
                return Ok(Some(entry.position));
            }
            previous = entry.digest;
            expected_position = expected_position.saturating_add(1);
        }
        Ok(None)
    }

    /// [`ChainArchive::first_broken_position`] as a pass or fail.
    pub fn verify(&self) -> Result<()> {
        match self.first_broken_position()? {
            None => Ok(()),
            Some(position) => Err(Error::invalid(format!(
                "the archived event chain breaks at position {position}; every entry from there \
                 on is unaccounted for"
            ))),
        }
    }

    /// A one-line summary for a start-up banner.
    ///
    /// Verifies the whole chain, which costs a read of every record. That is
    /// deliberate at start-up and nowhere else: an archive that was damaged
    /// while the process was down should be discovered *before* this run
    /// appends to it, because a break found later cannot be told apart from
    /// one this run caused. Anything polled repeatedly should read
    /// [`ChainArchive::records_archived`] instead.
    pub fn describe(&self) -> String {
        match (self.len(), self.first_broken_position()) {
            (Ok(0), _) => "empty; this is the first run against this store".to_string(),
            (Ok(count), Ok(None)) => format!("{count} record(s) retained, chain intact"),
            (Ok(count), Ok(Some(position))) => {
                format!("{count} record(s) retained, CHAIN BROKEN at position {position}")
            }
            (Ok(count), Err(error)) => {
                format!(
                    "{count} record(s) retained, UNVERIFIABLE: {}",
                    error.message()
                )
            }
            (Err(error), _) => format!("UNREADABLE: {}", error.message()),
        }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, ArchiveState> {
        // A poisoned lock here means a thread panicked mid-append. The state it
        // left is still the truth about what reached the store, because the
        // store write precedes every field update, so recovering the guard is
        // sounder than refusing to archive anything for the rest of the run.
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

fn key_for_position(position: u64) -> String {
    format!("{KEY_PREFIX}{position:0width$}", width = POSITION_WIDTH)
}

/// The linkage over one entry.
///
/// Canonicalised rather than serialized directly, so a `serde_json` release
/// that changed key ordering could not silently invalidate every archive
/// already written.
fn entry_digest(previous: &str, position: u64, record: &LogRecord) -> Result<String> {
    let body = canonical_json(&serde_json::to_value(record)?);
    Ok(sha256_hex(
        format!("{previous}|{position}|{body}").as_bytes(),
    ))
}
