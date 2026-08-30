//! An embedded, crash-safe storage engine.
//!
//! [`DurableStore`] is a single-node, single-process key-value engine written
//! in-tree with no third-party dependencies. It implements the platform's
//! [`KeyValueStore`] port, so anything already holding an
//! `Arc<dyn KeyValueStore>` can be pointed at it unchanged, and adds
//! transactions, range scans and integrity verification as inherent methods.
//!
//! # What it is
//!
//! A write-ahead log with periodic checkpoints, and a `BTreeMap` index held in
//! memory.
//!
//! * **Writes append.** A commit is one framed, digest-covered record appended
//!   to `wal.<generation>` and flushed. Nothing else on disk is touched. The
//!   cost of a write is proportional to the write, not to the size of the
//!   dataset.
//! * **Reads are in-memory.** The index maps every live key to its current
//!   value; `get` never touches the disk. Prefix scans are `BTreeMap` range
//!   queries, so they cost the number of matches plus a lookup, not a full
//!   table scan.
//! * **Checkpoints bound the log.** When the log outgrows its trigger the
//!   whole index is written to a new `checkpoint.<generation>`, a fresh empty
//!   log is created beside it, and `MANIFEST` is atomically flipped to name
//!   the new generation. The previous pair is then deleted.
//!
//! # The durability guarantee, exactly
//!
//! > **When `put`, `delete` or `commit` returns `Ok`, the record describing
//! > that change has been written to the write-ahead log and `fsync` has
//! > returned successfully on that log file. The change survives an immediate
//! > `kill -9` of the process and an immediate loss of power to the machine.**
//!
//! Four things that sentence deliberately does not say:
//!
//! * It does not promise anything *before* the call returns. There is no
//!   partial credit and no "probably flushed by now": a `put` still in flight
//!   is not durable.
//! * It holds only for [`Durability::Synchronous`], which is the default.
//!   Under [`Durability::OsBuffered`] a commit returns once the kernel has the
//!   bytes; that survives a process crash but not a power loss, and the
//!   setting exists so a caller who genuinely does not need durability has to
//!   say so in writing.
//! * It is bounded by the honesty of the hardware. `fsync` is a request that
//!   travels to the drive; a device with a volatile write cache that ignores
//!   flush commands, or a network filesystem that acknowledges early, defeats
//!   it. No software can detect that from above.
//! * The tests can kill a process; they cannot cut power. Power loss is
//!   modelled by truncating the log at every byte offset of its final record
//!   and reopening — the failure mode a lost write actually produces. That
//!   substitution is the one place the test suite argues by analogy, and it is
//!   named here rather than glossed over.
//!
//! # Atomicity
//!
//! A [`WriteBatch`] of any number of keys becomes exactly one log record. A
//! record is either complete and digest-verified, in which case every
//! operation in it is applied, or it is not, in which case none are. There is
//! no window in which half a transaction is visible, on disk or in memory:
//! the index is only updated after the record is durable.
//!
//! # Corruption
//!
//! Every record carries a SHA-256 of its payload. Recovery distinguishes two
//! kinds of damage and treats them very differently:
//!
//! * A **torn tail** — the file ends inside the last record, or trails off
//!   into zeroes — is the signature of a crash mid-append. The incomplete
//!   record is discarded, the file is truncated back to the last verified
//!   offset, and the store opens with every complete record intact. The count
//!   and offset are reported in [`RecoveryReport`].
//! * **Corruption inside a complete record** — a digest that does not match
//!   the bytes it covers — cannot be produced by a truncated write. It means
//!   the device returned data nobody wrote. Opening fails with an error naming
//!   the file and the byte offset. A corrupt record is never decoded, never
//!   applied, and never returned to a caller as though it were data.
//!
//! [`DurableStore::verify`] re-reads both files and re-checks every digest on
//! demand, for a periodic integrity audit that does not require a restart.
//!
//! # What this engine does not provide
//!
//! It is an embedded engine, not a database. Every one of the following is a
//! real limitation, not a temporary gap:
//!
//! * **No replication and no failover.** One copy of the data on one node's
//!   filesystem. If that filesystem is lost, the data is lost. Durability
//!   against a crash is not availability, and neither is a backup.
//! * **No multi-process access.** One process may open a directory. A second
//!   [`DurableStore`] on the same directory *in the same process* is refused;
//!   a second *process* is not detected at all, because detecting it portably
//!   would need file locking this crate cannot reach without dependencies.
//!   Two processes writing one directory will corrupt it, and nothing here
//!   will stop them. Deployments must guarantee a single writer.
//! * **No distributed transactions.** Atomicity covers one batch against one
//!   store. Nothing spans two stores or two nodes.
//! * **No concurrent writers.** Commits serialise on one mutex. Readers run
//!   concurrently with each other and with a writer, and always observe a
//!   committed state, but write throughput is one commit at a time and one
//!   `fsync` per commit. There is no group commit.
//! * **No MVCC and no snapshot isolation across calls.** A read sees the state
//!   at the moment it takes the lock. [`DurableStore::snapshot`] copies the
//!   index if a caller needs a stable view, and pays for the copy.
//! * **The dataset must fit in memory.** The index holds every live key and
//!   value. There is no block cache, no paging, no on-disk B-tree. This is
//!   sized for the platform's operational state — portfolios, orders, agent
//!   memory — not for tick history.
//! * **No secondary indexes and no query language.** Lookup by key, scan by
//!   prefix or by range. Anything else is the caller's job.
//! * **No background compaction thread.** Checkpoints happen inline on the
//!   commit that crosses the threshold, so that commit is slow. There is no
//!   thread; there is nothing to tune.
//! * **No encryption at rest and no access control.** File permissions are
//!   whatever the operating system gives the process.
//!
//! # Write amplification
//!
//! A checkpoint is triggered when the live log exceeds
//! `max(configured minimum, size of the last checkpoint)`. Because the log
//! must therefore grow to at least the size of the previous checkpoint before
//! another one is written, the bytes a checkpoint spends are bounded by the
//! bytes the log had already spent to earn it. Total bytes written stay within
//! a small constant multiple of the bytes committed — the engine's own test
//! measures roughly 2.3× — rather than growing with the dataset on every put.
//!
//! So `N` writes cost `O(N)` bytes in total, against the `O(N²)` a store that
//! re-serialises its whole document on every change pays. What the policy
//! varies is how often a checkpoint falls, never what an individual write
//! costs.

mod frame;
mod log;

use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use crate::fsio;
use crate::kv::KeyValueStore;
use log::{FrameLog, Manifest};

/// Name of the pointer file naming the live generation.
const MANIFEST_NAME: &str = "MANIFEST";

/// Entries per checkpoint record. Bounds the memory a single frame needs on
/// the way in and out without paying 40 bytes of framing per key.
const CHECKPOINT_CHUNK: usize = 256;

/// Default minimum log size before a checkpoint is considered.
const DEFAULT_CHECKPOINT_AFTER_BYTES: u64 = 4 * 1024 * 1024;

// --- configuration ----------------------------------------------------------

/// How much a commit is willing to pay for durability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// `fsync` the log on every commit before returning. The default, and the
    /// only setting under which the guarantee documented on this module holds.
    #[default]
    Synchronous,
    /// Return once the operating system has the bytes. Survives `kill -9`;
    /// does not survive power loss. Choose it only where losing the last
    /// commits is acceptable and say why at the call site.
    OsBuffered,
}

impl Durability {
    /// Whether an acknowledged commit survives loss of power.
    pub fn survives_power_loss(self) -> bool {
        matches!(self, Self::Synchronous)
    }
}

/// How a [`DurableStore`] is opened.
///
/// A [`Clock`] is required rather than defaulted: the engine stamps every
/// commit, and the platform forbids reaching for the wall clock ambiently, so
/// the caller states which clock a replay should see.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    clock: Arc<dyn Clock>,
    durability: Durability,
    checkpoint_after_bytes: u64,
}

impl EngineConfig {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            durability: Durability::Synchronous,
            checkpoint_after_bytes: DEFAULT_CHECKPOINT_AFTER_BYTES,
        }
    }

    /// Trade durability for speed, explicitly.
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// Minimum log size before a checkpoint is considered.
    ///
    /// Clamped to at least one frame's worth of bytes: a threshold of zero
    /// would checkpoint on every commit, which is the write amplification this
    /// engine exists to avoid.
    pub fn with_checkpoint_after_bytes(mut self, bytes: u64) -> Self {
        self.checkpoint_after_bytes = bytes.max(frame::FRAME_HEADER_LEN as u64);
        self
    }

    pub fn durability(&self) -> Durability {
        self.durability
    }

    pub fn checkpoint_after_bytes(&self) -> u64 {
        self.checkpoint_after_bytes
    }
}

// --- records ----------------------------------------------------------------

/// One change within a commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Operation {
    Put {
        key: String,
        value: serde_json::Value,
    },
    Delete {
        key: String,
    },
}

/// The payload of one log record: an atomic group of operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Commit {
    sequence: u64,
    committed_at: Timestamp,
    operations: Vec<Operation>,
}

/// A group of changes applied as one atomic unit.
///
/// The batch becomes a single log record, so it is either entirely present
/// after a crash or entirely absent. Building one is free; nothing touches
/// disk until it is handed to [`DurableStore::commit`].
#[derive(Clone, Debug, Default)]
pub struct WriteBatch {
    operations: Vec<Operation>,
}

impl WriteBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage a write. Later operations on the same key win, as they would if
    /// applied in order.
    #[must_use]
    pub fn put(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.operations.push(Operation::Put {
            key: key.into(),
            value,
        });
        self
    }

    /// Stage a serializable write.
    pub fn put_as<T: Serialize>(self, key: impl Into<String>, value: &T) -> Result<Self> {
        Ok(self.put(key, serde_json::to_value(value)?))
    }

    /// Stage a delete. Deleting an absent key is not an error.
    #[must_use]
    pub fn delete(mut self, key: impl Into<String>) -> Self {
        self.operations.push(Operation::Delete { key: key.into() });
        self
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

// --- reports ----------------------------------------------------------------

/// What opening the store found and what it had to discard.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    /// Generation named by the manifest when the store was opened.
    pub generation: u64,
    /// Records read from the checkpoint.
    pub checkpoint_records: u64,
    /// Keys restored from the checkpoint.
    pub checkpoint_keys: u64,
    /// Complete log records replayed on top of the checkpoint.
    pub log_records_applied: u64,
    /// Incomplete records discarded. Zero or one — a log has one tail.
    pub torn_records_discarded: u64,
    /// Byte offset at which the incomplete record began, if there was one.
    pub torn_tail_at: Option<u64>,
    /// Bytes removed from the end of the log.
    pub bytes_discarded: u64,
    /// Commit sequence the store recovered to.
    pub last_sequence: u64,
}

impl RecoveryReport {
    /// Whether recovery had to discard an unfinished write.
    pub fn recovered_from_a_torn_write(&self) -> bool {
        self.torn_tail_at.is_some()
    }
}

/// Counters describing what the engine has written since it was opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStats {
    /// Commits acknowledged since open.
    pub commits: u64,
    /// Individual puts and deletes within those commits.
    pub operations: u64,
    /// Bytes appended to the log, framing included.
    pub log_bytes_appended: u64,
    /// Checkpoints written since open.
    pub checkpoints: u64,
    /// Bytes written by those checkpoints.
    pub checkpoint_bytes_written: u64,
    /// Bytes of records currently in the live log.
    pub live_log_bytes: u64,
    /// Live keys.
    pub keys: u64,
    /// Sequence of the most recent commit.
    pub last_sequence: u64,
    /// Live generation.
    pub generation: u64,
}

impl EngineStats {
    /// Every byte the engine has written since it was opened.
    pub fn total_bytes_written(&self) -> u64 {
        self.log_bytes_appended
            .saturating_add(self.checkpoint_bytes_written)
    }
}

/// The result of re-reading both files and re-checking every digest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityReport {
    pub checkpoint_records: u64,
    pub log_records: u64,
    pub bytes_verified: u64,
}

// --- single-writer guard ----------------------------------------------------

/// Directories open in this process.
///
/// This prevents the easy mistake — two stores over one directory inside one
/// binary. It says nothing about other processes; see the module docs.
static OPEN_DIRECTORIES: LazyLock<Mutex<BTreeSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(BTreeSet::new()));

#[derive(Debug)]
struct DirectoryGuard {
    path: PathBuf,
}

impl DirectoryGuard {
    fn acquire(path: PathBuf) -> Result<Self> {
        let mut open = OPEN_DIRECTORIES.lock().unwrap_or_else(|e| e.into_inner());
        if !open.insert(path.clone()) {
            return Err(Error::denied(format!(
                "{} is already open in this process; the engine permits one writer per directory",
                path.display()
            )));
        }
        Ok(Self { path })
    }
}

impl Drop for DirectoryGuard {
    fn drop(&mut self) {
        OPEN_DIRECTORIES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.path);
    }
}

// --- the engine -------------------------------------------------------------

/// Mutable writer state. Everything here is serialised by one mutex.
#[derive(Debug)]
struct Writer {
    generation: u64,
    sequence: u64,
    wal: FrameLog,
    /// Log size at which the next checkpoint fires.
    checkpoint_trigger: u64,
    stats: EngineStats,
}

/// A crash-safe embedded key-value engine. See the module documentation for
/// the exact durability guarantee and for what it does not provide.
#[derive(Debug)]
pub struct DurableStore {
    directory: PathBuf,
    config: EngineConfig,
    index: RwLock<BTreeMap<String, serde_json::Value>>,
    writer: Mutex<Writer>,
    recovery: RecoveryReport,
    _guard: DirectoryGuard,
}

impl DurableStore {
    /// Open the store rooted at `directory`, recovering any previous state.
    ///
    /// Creates the directory and an empty generation if nothing is there. If a
    /// previous run left a torn record at the end of the log it is discarded
    /// and the log truncated; the details land in [`DurableStore::recovery`].
    pub fn open(directory: impl AsRef<Path>, config: EngineConfig) -> Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        std::fs::create_dir_all(&directory)?;
        let canonical = std::fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone());
        let guard = DirectoryGuard::acquire(canonical)?;

        let manifest_path = directory.join(MANIFEST_NAME);
        let manifest = if manifest_path.exists() {
            Manifest::read(&manifest_path)?
        } else {
            initialise(&directory, &manifest_path)?
        };

        let checkpoint_path = generation_path(&directory, "checkpoint", manifest.generation);
        let wal_path = generation_path(&directory, "wal", manifest.generation);

        let mut index = BTreeMap::new();
        let mut report = RecoveryReport {
            generation: manifest.generation,
            last_sequence: manifest.sequence,
            ..RecoveryReport::default()
        };

        // The checkpoint is published only after it is complete and flushed,
        // so a torn tail in it is damage rather than an interrupted append.
        let checkpoint_label = display(&checkpoint_path);
        let checkpoint = log::scan(&checkpoint_label, &checkpoint_path)?;
        if let Some(offset) = checkpoint.torn_at {
            return Err(Error::io(format!(
                "the checkpoint {checkpoint_label} ends inside a record at byte offset {offset}; \
                 a checkpoint is flushed before it is published, so this is data loss, \
                 not an interrupted write"
            )));
        }
        for payload in &checkpoint.payloads {
            let commit = decode_commit(&checkpoint_label, payload)?;
            report.checkpoint_records += 1;
            apply(&mut index, &commit.operations);
        }
        report.checkpoint_keys = index.len() as u64;

        let wal_label = display(&wal_path);
        let scan = log::scan(&wal_label, &wal_path)?;
        let mut sequence = manifest.sequence;
        for payload in &scan.payloads {
            let commit = decode_commit(&wal_label, payload)?;
            if commit.sequence <= sequence {
                return Err(Error::io(format!(
                    "{wal_label} replays commit {} after commit {sequence}; \
                     the log is out of order and cannot be trusted",
                    commit.sequence
                )));
            }
            sequence = commit.sequence;
            report.log_records_applied += 1;
            apply(&mut index, &commit.operations);
        }
        report.last_sequence = sequence;
        report.torn_tail_at = scan.torn_at;
        report.bytes_discarded = scan.discarded;
        report.torn_records_discarded = u64::from(scan.torn_at.is_some());

        let mut wal = FrameLog::open_at(&wal_path, scan.valid_end)?;
        if scan.torn_at.is_some() {
            // Appending after a partial record would bury it inside the log,
            // where recovery would read it as corruption rather than a tail.
            wal.truncate_to(scan.valid_end)?;
        }

        let checkpoint_trigger = config.checkpoint_after_bytes.max(
            checkpoint
                .valid_end
                .saturating_sub(frame::FILE_HEADER_LEN as u64),
        );
        let stats = EngineStats {
            live_log_bytes: wal.frame_bytes(),
            keys: index.len() as u64,
            last_sequence: sequence,
            generation: manifest.generation,
            ..EngineStats::default()
        };

        Ok(Self {
            directory,
            config,
            index: RwLock::new(index),
            writer: Mutex::new(Writer {
                generation: manifest.generation,
                sequence,
                wal,
                checkpoint_trigger,
                stats,
            }),
            recovery: report,
            _guard: guard,
        })
    }

    /// The directory backing this store.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// What the last open recovered, and what it discarded.
    pub fn recovery(&self) -> &RecoveryReport {
        &self.recovery
    }

    /// The durability setting in force.
    pub fn durability(&self) -> Durability {
        self.config.durability
    }

    /// Counters describing what has been written since the store was opened.
    pub fn stats(&self) -> EngineStats {
        let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut stats = writer.stats;
        stats.live_log_bytes = writer.wal.frame_bytes();
        stats.generation = writer.generation;
        stats.last_sequence = writer.sequence;
        stats.keys = self.read_index().len() as u64;
        stats
    }

    /// Apply a batch atomically and durably. Returns the commit sequence.
    ///
    /// An empty batch writes nothing and returns the current sequence: there
    /// is no reason to pay for a barrier that records no change.
    pub fn commit(&self, batch: WriteBatch) -> Result<u64> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if batch.operations.is_empty() {
            return Ok(writer.sequence);
        }
        self.commit_locked(&mut writer, batch.operations)
    }

    /// Force a checkpoint regardless of the log size. Returns the new
    /// generation. Used by operators and by tests; the engine calls it itself
    /// when the log outgrows its trigger.
    pub fn checkpoint(&self) -> Result<u64> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        self.checkpoint_locked(&mut writer)?;
        Ok(writer.generation)
    }

    /// A copy of the whole keyspace, stable while the caller holds it.
    ///
    /// Costs a full clone of the index; the engine has no MVCC, so this is
    /// what a stable view is made of.
    pub fn snapshot(&self) -> BTreeMap<String, serde_json::Value> {
        self.read_index().clone()
    }

    /// Every key and value under a prefix, in key order.
    pub fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, serde_json::Value)>> {
        let index = self.read_index();
        Ok(index
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }

    /// Every key and value in `[start, end)`, in key order.
    pub fn range(&self, start: &str, end: &str) -> Result<Vec<(String, serde_json::Value)>> {
        if end < start {
            return Err(Error::invalid(format!(
                "range start {start:?} is after range end {end:?}"
            )));
        }
        let index = self.read_index();
        Ok(index
            .range::<str, _>((Bound::Included(start), Bound::Excluded(end)))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }

    /// Re-read both files and re-check every digest.
    ///
    /// Detects bit rot that has appeared since the store was opened, without
    /// requiring a restart. Blocks commits for the duration.
    pub fn verify(&self) -> Result<IntegrityReport> {
        let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let checkpoint_path = generation_path(&self.directory, "checkpoint", writer.generation);
        let wal_path = generation_path(&self.directory, "wal", writer.generation);
        let checkpoint = log::scan(&display(&checkpoint_path), &checkpoint_path)?;
        let wal = log::scan(&display(&wal_path), &wal_path)?;
        Ok(IntegrityReport {
            checkpoint_records: checkpoint.payloads.len() as u64,
            log_records: wal.payloads.len() as u64,
            bytes_verified: checkpoint.valid_end + wal.valid_end,
        })
    }

    fn read_index(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, serde_json::Value>> {
        self.index.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Append one record, make it durable, then make it visible.
    ///
    /// The order matters and is the whole guarantee: the index is updated only
    /// after `fsync` returns, so no reader can observe a value that a crash
    /// would take back.
    fn commit_locked(&self, writer: &mut Writer, operations: Vec<Operation>) -> Result<u64> {
        let sequence = writer.sequence + 1;
        let commit = Commit {
            sequence,
            committed_at: self.config.clock.now(),
            operations,
        };
        let payload = serde_json::to_vec(&commit)?;
        let written = writer.wal.append(&payload)?;
        if self.config.durability == Durability::Synchronous {
            writer.wal.sync()?;
        }

        writer.sequence = sequence;
        writer.stats.commits += 1;
        writer.stats.operations += commit.operations.len() as u64;
        writer.stats.log_bytes_appended += written;

        {
            let mut index = self.index.write().unwrap_or_else(|e| e.into_inner());
            apply(&mut index, &commit.operations);
        }

        if writer.wal.frame_bytes() >= writer.checkpoint_trigger {
            self.checkpoint_locked(writer)?;
        }
        Ok(sequence)
    }

    /// Write a new checkpoint and log pair and publish them atomically.
    ///
    /// Ordering is what makes this crash-safe. The new checkpoint is written
    /// and flushed under a scratch name, renamed, and the new empty log
    /// created — all before `MANIFEST` is flipped. A crash before the flip
    /// leaves the previous generation intact and complete; a crash after it
    /// leaves the new one. The old files are deleted last, and only then.
    fn checkpoint_locked(&self, writer: &mut Writer) -> Result<()> {
        let previous = writer.generation;
        let generation = previous + 1;
        let sequence = writer.sequence;
        let committed_at = self.config.clock.now();

        let entries: Vec<(String, serde_json::Value)> = {
            let index = self.read_index();
            index
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        };

        let checkpoint_path = generation_path(&self.directory, "checkpoint", generation);
        let building = fsio::temporary_path(&checkpoint_path);
        let mut written = frame::FILE_HEADER_LEN as u64;
        {
            let mut file = FrameLog::create(&building)?;
            for chunk in entries.chunks(CHECKPOINT_CHUNK) {
                let commit = Commit {
                    sequence,
                    committed_at,
                    operations: chunk
                        .iter()
                        .map(|(key, value)| Operation::Put {
                            key: key.clone(),
                            value: value.clone(),
                        })
                        .collect(),
                };
                written += file.append(&serde_json::to_vec(&commit)?)?;
            }
            file.sync()?;
        }
        if let Err(e) = std::fs::rename(&building, &checkpoint_path) {
            let _ = std::fs::remove_file(&building);
            return Err(e.into());
        }
        fsio::sync_directory(&self.directory);

        let wal_path = generation_path(&self.directory, "wal", generation);
        let wal = FrameLog::create(&wal_path)?;

        // The commit point: after this line recovery uses the new generation.
        Manifest::new(generation, sequence).write(&self.directory.join(MANIFEST_NAME))?;

        writer.wal = wal;
        writer.generation = generation;
        writer.checkpoint_trigger = self
            .config
            .checkpoint_after_bytes
            .max(written.saturating_sub(frame::FILE_HEADER_LEN as u64));
        writer.stats.checkpoints += 1;
        writer.stats.checkpoint_bytes_written += written;
        writer.stats.generation = generation;

        let _ = std::fs::remove_file(generation_path(&self.directory, "checkpoint", previous));
        let _ = std::fs::remove_file(generation_path(&self.directory, "wal", previous));
        fsio::sync_directory(&self.directory);
        Ok(())
    }
}

impl KeyValueStore for DurableStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self.read_index().get(key).cloned())
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        self.commit_locked(
            &mut writer,
            vec![Operation::Put {
                key: key.to_string(),
                value,
            }],
        )?;
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<bool> {
        // The writer lock is taken before the existence check so that the
        // answer and the record that justifies it cannot disagree.
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let present = self.read_index().contains_key(key);
        if !present {
            return Ok(false);
        }
        self.commit_locked(
            &mut writer,
            vec![Operation::Delete {
                key: key.to_string(),
            }],
        )?;
        Ok(true)
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let index = self.read_index();
        Ok(index
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, _)| key.clone())
            .collect())
    }

    fn len(&self) -> Result<usize> {
        Ok(self.read_index().len())
    }
}

// --- helpers ----------------------------------------------------------------

fn generation_path(directory: &Path, kind: &str, generation: u64) -> PathBuf {
    directory.join(format!("{kind}.{generation:020}"))
}

fn display(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn apply(index: &mut BTreeMap<String, serde_json::Value>, operations: &[Operation]) {
    for operation in operations {
        match operation {
            Operation::Put { key, value } => {
                index.insert(key.clone(), value.clone());
            }
            Operation::Delete { key } => {
                index.remove(key);
            }
        }
    }
}

fn decode_commit(label: &str, payload: &[u8]) -> Result<Commit> {
    serde_json::from_slice(payload).map_err(|e| {
        Error::schema(format!(
            "a digest-valid record in {label} does not parse as a commit: {e}"
        ))
    })
}

/// Create generation zero: an empty checkpoint, an empty log, a manifest.
///
/// The manifest is written last, so a crash part-way leaves a directory with
/// no manifest — which is exactly the state this function is called for.
fn initialise(directory: &Path, manifest_path: &Path) -> Result<Manifest> {
    refuse_to_reinitialise_over_data(directory)?;
    // Every remaining generation file is now known to hold no records, so
    // clearing them loses nothing and keeps an interrupted first open from
    // leaving files behind that no manifest names.
    for path in generation_files(directory) {
        let _ = std::fs::remove_file(path);
    }
    FrameLog::create(&generation_path(directory, "checkpoint", 0))?;
    FrameLog::create(&generation_path(directory, "wal", 0))?;
    let manifest = Manifest::new(0, 0);
    manifest.write(manifest_path)?;
    Ok(manifest)
}

/// Refuse to treat a directory as fresh if it holds records.
///
/// A missing manifest beside a log that contains data means either a crash
/// during the very first open — in which case the leftovers are empty and
/// discarding them costs nothing — or a manifest somebody removed. The second
/// case must not be answered by silently starting over.
fn refuse_to_reinitialise_over_data(directory: &Path) -> Result<()> {
    for path in generation_files(directory) {
        let length = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if length > frame::FILE_HEADER_LEN as u64 {
            return Err(Error::io(format!(
                "{} has no {MANIFEST_NAME} but {} holds {length} bytes of records; \
                 refusing to reinitialise over data. Restore the manifest or move the \
                 directory aside deliberately.",
                directory.display(),
                display(&path)
            )));
        }
    }
    Ok(())
}

/// Checkpoint and log files in the directory, scratch files excluded.
fn generation_files(directory: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            !fsio::is_temporary(&name)
                && (name.starts_with("checkpoint.") || name.starts_with("wal."))
        })
        .map(|entry| entry.path())
        .collect()
}
