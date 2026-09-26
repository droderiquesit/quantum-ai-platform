//! Segment log: append, fsync-before-ack, roll, seal. See ADR 0100 §3.
//! Filled by SLICE-16.
//!
//! [`SegmentLog`] is one append-only log of event-fabric batches
//! (`qip_events::event_fabric::codec::Batch`), rooted at a directory of
//! `segment.<start offset>` files. It is the type ADR 0100 §1 names once and
//! reuses twice: the same struct is a broker partition and the edge spool —
//! nothing here is specific to either custodian.
//!
//! # What "readable" means
//!
//! The high watermark is **the last `fsync`ed offset, never the last
//! appended one** (ADR 0100 §3). [`SegmentLog::append`] does not return an
//! offset — and [`SegmentLog::read`] will not serve one — until the bytes
//! for it have been handed to the storage device and the device has
//! confirmed them, in that order. See [`SegmentLog::append`]'s own
//! documentation for the red-team finding (M3) this ordering exists to
//! close: a batch counted as durable before it actually was, reused for
//! different content after a crash discarded it.
//!
//! # Roll and seal
//!
//! An append that leaves the active segment at or past
//! [`SegmentLogConfig::roll_after_bytes`] seals it: the file's bytes are
//! hashed (its [`ContentHash`]), the hash is chained to the previous sealed
//! segment's own hash, and the pair is written as a [`Seal`] through the
//! manifest before a new, empty segment is created to receive the next
//! append. [`verify_seal_chain`] is what turns that chain into a check: a
//! segment deleted, substituted or corrupted after the fact breaks the link
//! at exactly that point, and retention keeps every segment's seal even
//! after the segment's own bytes are gone, so the gap stays detectable.
//!
//! # Retention
//!
//! [`SegmentLog::retain_before`] deletes only **sealed** segments — the
//! active one is never eligible — and, for a stream whose
//! [`SegmentLogConfig::archive_required`] is set, only ones
//! [`SegmentLog::mark_archived`] has already recorded. A sealed segment that
//! has not yet been archived is withheld, not deleted, however old it is:
//! producer-retained durability (ADR 0100 §3) depends on that segment being
//! the last copy until the archive report says otherwise.

use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_events::event_fabric::codec::{Batch, ContentHash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use super::file::{self, SegmentFile};
use super::recovery::{self, SegmentMeta};

// --- configuration -----------------------------------------------------

const DEFAULT_ROLL_AFTER_BYTES: u64 = 8 * 1024 * 1024;

/// A roll threshold below this would seal a segment after essentially every
/// append, which is write amplification with no compensating benefit — the
/// same judgement `EngineConfig::with_checkpoint_after_bytes` already makes
/// for the WAL engine's checkpoint trigger, restated here for segments.
const MIN_ROLL_AFTER_BYTES: u64 = 4096;

/// How a [`SegmentLog`] is opened.
#[derive(Clone, Debug)]
pub struct SegmentLogConfig {
    clock: Arc<dyn Clock>,
    roll_after_bytes: u64,
    archive_required: bool,
}

impl SegmentLogConfig {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            roll_after_bytes: DEFAULT_ROLL_AFTER_BYTES,
            archive_required: false,
        }
    }

    /// Clamped to [`MIN_ROLL_AFTER_BYTES`]: see that constant's own comment.
    pub fn with_roll_after_bytes(mut self, bytes: u64) -> Self {
        self.roll_after_bytes = bytes.max(MIN_ROLL_AFTER_BYTES);
        self
    }

    /// Marks this log as one whose producer-retained durability
    /// (ADR 0100 §3) depends on the archive report: a sealed segment here is
    /// never deleted by [`SegmentLog::retain_before`] until
    /// [`SegmentLog::mark_archived`] has recorded it.
    pub fn with_archive_required(mut self, required: bool) -> Self {
        self.archive_required = required;
        self
    }

    pub fn roll_after_bytes(&self) -> u64 {
        self.roll_after_bytes
    }

    pub fn archive_required(&self) -> bool {
        self.archive_required
    }
}

// --- manifest: a small, self-verifying keyed record store --------------

/// Marks a file written by [`manifest_write`]. Distinct from every other
/// magic this crate defines, so a manifest record is never mistaken for a
/// segment file or vice versa.
const MANIFEST_MAGIC: [u8; 4] = *b"QMAN";

/// magic (4) + CRC32C (4).
const MANIFEST_HEADER_LEN: usize = 8;

fn validate_manifest_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(Error::invalid(
            "a manifest key must not be empty".to_string(),
        ));
    }
    if key.len() > 200 {
        return Err(Error::invalid(format!(
            "manifest key is {} bytes, over the 200-byte limit",
            key.len()
        )));
    }
    let safe = key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if !safe {
        return Err(Error::invalid(format!(
            "manifest key {key:?} must contain only ASCII letters, digits, '.', '-' or '_', \
             so it can never name a path outside the manifest directory"
        )));
    }
    Ok(())
}

fn manifest_dir(directory: &Path) -> PathBuf {
    directory.join("manifest")
}

fn manifest_file_path(directory: &Path, key: &str) -> PathBuf {
    manifest_dir(directory).join(key)
}

fn wrap_manifest(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MANIFEST_HEADER_LEN + bytes.len());
    out.extend_from_slice(&MANIFEST_MAGIC);
    let crc = qip_events::event_fabric::crc32c::crc32c(bytes);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Refuse a torn or corrupted manifest rather than trust it.
///
/// [`crate::fsio::write_atomic`] already makes a manifest write atomic and
/// durable — either the previous bytes survive a crash or the new ones do,
/// never a mixture — so this self-check exists for the one failure that
/// atomicity cannot rule out from above: a device that reports a flush
/// complete before the bytes are actually safe. Without a checksum of its
/// own, a manifest record that a crash left short would be indistinguishable
/// from a short but valid one, and would be handed to a caller as data.
fn unwrap_manifest(key: &str, wrapped: &[u8]) -> Result<Vec<u8>> {
    if wrapped.len() < MANIFEST_HEADER_LEN {
        return Err(Error::schema(format!(
            "manifest record {key} is {} bytes, shorter than its own {MANIFEST_HEADER_LEN}-byte \
             header; refusing a torn record rather than trusting it",
            wrapped.len()
        )));
    }
    if wrapped[..4] != MANIFEST_MAGIC {
        return Err(Error::schema(format!(
            "manifest record {key} does not begin with the manifest magic; refusing a record \
             this store did not write"
        )));
    }
    let expected = u32::from_le_bytes([wrapped[4], wrapped[5], wrapped[6], wrapped[7]]);
    let payload = &wrapped[MANIFEST_HEADER_LEN..];
    let actual = qip_events::event_fabric::crc32c::crc32c(payload);
    if actual != expected {
        return Err(Error::schema(format!(
            "manifest record {key} fails its own checksum (recorded {expected:#010x}, computed \
             {actual:#010x}); refusing a torn or corrupted record rather than serving it as data"
        )));
    }
    Ok(payload.to_vec())
}

fn manifest_write(directory: &Path, key: &str, bytes: &[u8]) -> Result<()> {
    crate::fsio::write_atomic(&manifest_file_path(directory, key), &wrap_manifest(bytes))
}

fn manifest_read(directory: &Path, key: &str) -> Result<Option<Vec<u8>>> {
    let path = manifest_file_path(directory, key);
    match std::fs::read(&path) {
        Ok(wrapped) => Ok(Some(unwrap_manifest(key, &wrapped)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(format!("cannot read manifest record {key}: {e}"))),
    }
}

fn seal_key(start_offset: u64) -> String {
    format!("seal.{start_offset:020}")
}

fn archived_key(start_offset: u64) -> String {
    format!("archived.{start_offset:020}")
}

// --- seals ---------------------------------------------------------------

/// A sealed segment's evidence: what it held, and the hash of the segment
/// before it. Carries [`ContentHash`] (SLICE-06's tag), and this build only
/// ever produces [`ContentHash::Sha256`] — [`Seal::to_wire_bytes`] refuses to
/// serialise anything else rather than write a hash format no verifier here
/// can check.
#[derive(Clone, Debug, PartialEq)]
pub struct Seal {
    pub segment_start_offset: u64,
    pub record_count: u64,
    pub byte_length: u64,
    pub sealed_at: Timestamp,
    pub content_hash: ContentHash,
    /// The content hash of the segment sealed immediately before this one,
    /// or `None` for the very first segment. A missing or substituted
    /// segment breaks this link at exactly the point it was removed — see
    /// [`verify_seal_chain`].
    pub previous_seal_hash: Option<ContentHash>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SealWire {
    segment_start_offset: u64,
    record_count: u64,
    byte_length: u64,
    sealed_at_nanos: i64,
    content_hash_algorithm: String,
    content_hash_hex: String,
    previous_seal_hash_algorithm: Option<String>,
    previous_seal_hash_hex: Option<String>,
}

fn hash_to_wire(hash: &ContentHash) -> Result<(String, String)> {
    match hash {
        ContentHash::Sha256(digest) => Ok(("sha256".to_string(), qip_core::hash::to_hex(digest))),
        ContentHash::Blake3(_) => Err(Error::invalid(
            "a seal cannot carry a Blake3 content hash; this build only ever produces Sha256 \
             (ADR 0100: seals carry SLICE-06's ContentHash tag, Sha256 only)"
                .to_string(),
        )),
    }
}

fn hash_from_wire(algorithm: &str, hex: &str, context: &str) -> Result<ContentHash> {
    match algorithm {
        "sha256" => {
            let bytes = qip_core::hash::from_hex(hex)
                .ok_or_else(|| Error::schema(format!("{context}: hash is not valid hex")))?;
            let digest: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                Error::schema(format!(
                    "{context}: hash is {} bytes, not the 32 SHA-256 produces",
                    bytes.len()
                ))
            })?;
            Ok(ContentHash::Sha256(digest))
        }
        other => Err(Error::schema(format!(
            "{context}: unknown content-hash algorithm {other:?}"
        ))),
    }
}

impl Seal {
    fn to_wire_bytes(&self) -> Result<Vec<u8>> {
        let (content_hash_algorithm, content_hash_hex) = hash_to_wire(&self.content_hash)?;
        let (previous_seal_hash_algorithm, previous_seal_hash_hex) = match &self.previous_seal_hash
        {
            Some(hash) => {
                let (algorithm, hex) = hash_to_wire(hash)?;
                (Some(algorithm), Some(hex))
            }
            None => (None, None),
        };
        let wire = SealWire {
            segment_start_offset: self.segment_start_offset,
            record_count: self.record_count,
            byte_length: self.byte_length,
            sealed_at_nanos: self.sealed_at.as_nanos(),
            content_hash_algorithm,
            content_hash_hex,
            previous_seal_hash_algorithm,
            previous_seal_hash_hex,
        };
        Ok(serde_json::to_vec(&wire)?)
    }

    fn from_wire_bytes(bytes: &[u8]) -> Result<Self> {
        let wire: SealWire = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("seal record does not parse: {e}")))?;
        let content_hash = hash_from_wire(
            &wire.content_hash_algorithm,
            &wire.content_hash_hex,
            "seal content hash",
        )?;
        let previous_seal_hash = match (
            wire.previous_seal_hash_algorithm,
            wire.previous_seal_hash_hex,
        ) {
            (Some(algorithm), Some(hex)) => {
                Some(hash_from_wire(&algorithm, &hex, "seal previous-seal hash")?)
            }
            (None, None) => None,
            _ => {
                return Err(Error::schema(
                    "seal record's previous-seal hash fields are inconsistent: one is present \
                     and the other is not"
                        .to_string(),
                ));
            }
        };
        Ok(Seal {
            segment_start_offset: wire.segment_start_offset,
            record_count: wire.record_count,
            byte_length: wire.byte_length,
            sealed_at: Timestamp::from_nanos(wire.sealed_at_nanos),
            content_hash,
            previous_seal_hash,
        })
    }
}

/// Confirm a sequence of seals, ascending by segment start offset, chains
/// without a gap: each seal's `previous_seal_hash` must equal the content
/// hash of the seal immediately before it in the slice. This is what makes a
/// deleted or substituted segment detectable without re-reading every other
/// segment's raw bytes — a missing entry, or one whose hash was altered,
/// breaks the chain at exactly that point, and the error names which
/// segment's link failed rather than reporting the whole chain as untrusted.
pub fn verify_seal_chain(seals: &[Seal]) -> Result<()> {
    for (i, seal) in seals.iter().enumerate() {
        let expected_previous = if i == 0 {
            None
        } else {
            Some(seals[i - 1].content_hash.clone())
        };
        if seal.previous_seal_hash != expected_previous {
            return Err(Error::schema(format!(
                "the seal chain breaks at the segment starting at offset {}: its recorded \
                 previous-seal hash does not match the segment immediately before it in this \
                 sequence; a segment is missing, out of order, or was substituted",
                seal.segment_start_offset
            )));
        }
    }
    Ok(())
}

// --- reports ---------------------------------------------------------------

/// What opening a [`SegmentLog`] found and what it had to discard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegmentRecoveryReport {
    pub segments_recovered: u64,
    pub batches_recovered: u64,
    /// Byte offset within the active segment at which an incomplete batch
    /// began, if there was one.
    pub torn_tail_at: Option<u64>,
    pub bytes_discarded: u64,
    pub next_offset: u64,
}

impl SegmentRecoveryReport {
    pub fn recovered_from_a_torn_write(&self) -> bool {
        self.torn_tail_at.is_some()
    }
}

/// A read-only summary of one segment, sealed or active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentSummary {
    pub start_offset: u64,
    pub end_offset: u64,
    pub sealed: bool,
    pub byte_length: u64,
}

/// What [`SegmentLog::retain_before`] did, by segment start offset — a
/// [`BTreeSet`] rather than a `Vec` because the order retention reports in
/// must not depend on how the sealed segments happened to iterate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub deleted: BTreeSet<u64>,
    pub withheld: BTreeSet<u64>,
}

// --- single-writer guard -----------------------------------------------

/// Directories open in this process. Mirrors `engine::DirectoryGuard`: two
/// [`SegmentLog`]s over one directory in one process would each believe
/// they alone decide the next offset, which is the exact reuse this crate's
/// single-writer discipline exists to prevent.
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
                "{} is already open in this process; a segment log permits one writer per \
                 directory",
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

// --- the log ---------------------------------------------------------------

/// Mutable state, serialised by one mutex — this crate's storage engine
/// makes the same "no concurrent writers" trade for the same reason
/// (`engine::DurableStore`'s own module documentation): one disk, one
/// `fsync` at a time, and no group commit to reason about.
#[derive(Debug)]
struct Writer {
    active: SegmentFile,
    active_start_offset: u64,
    active_index: Vec<(u64, u64)>,
    /// The next offset an append will be assigned, and this log's high
    /// watermark: every offset below it has been appended *and* fsynced.
    next_offset: u64,
    /// Sealed segments, ascending by start offset.
    sealed: Vec<SegmentMeta>,
    archived: BTreeSet<u64>,
    /// Rolls a checkpoint's own trigger cannot complete are counted rather
    /// than failing the append that tripped them, for the reason
    /// `DurableStore::commit_locked` gives for `checkpoint_failures`: the
    /// append's own record is already durable by the time a roll is
    /// attempted, and reporting it as failed would invite a caller to retry
    /// a write that already landed.
    roll_failures: u64,
}

/// One append-only log of event-fabric batches. See the module
/// documentation.
#[derive(Debug)]
pub struct SegmentLog {
    directory: PathBuf,
    config: SegmentLogConfig,
    writer: Mutex<Writer>,
    recovery: SegmentRecoveryReport,
    _guard: DirectoryGuard,
}

impl SegmentLog {
    /// Open the log rooted at `directory`, recovering any previous state.
    ///
    /// Creates the directory and an empty first segment if nothing is
    /// there. See the module documentation and `segment/recovery.rs` for
    /// exactly what a previous crash leaves and how it is handled.
    pub fn open(directory: impl AsRef<Path>, config: SegmentLogConfig) -> Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        std::fs::create_dir_all(&directory)?;
        let canonical = std::fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone());
        let guard = DirectoryGuard::acquire(canonical)?;

        let recovered = recovery::recover(&directory)?;
        let report = SegmentRecoveryReport {
            segments_recovered: recovered.sealed.len() as u64,
            batches_recovered: recovered.batches_recovered,
            torn_tail_at: recovered.torn_tail_at,
            bytes_discarded: recovered.bytes_discarded,
            next_offset: recovered.next_offset,
        };

        let mut archived = BTreeSet::new();
        for segment in &recovered.sealed {
            if manifest_read(&directory, &archived_key(segment.start_offset))?.is_some() {
                archived.insert(segment.start_offset);
            }
        }

        let writer = Writer {
            active: recovered.active_file,
            active_start_offset: recovered.active_start_offset,
            active_index: recovered.active_index,
            next_offset: recovered.next_offset,
            sealed: recovered.sealed,
            archived,
            roll_failures: 0,
        };

        Ok(Self {
            directory,
            config,
            writer: Mutex::new(writer),
            recovery: report,
            _guard: guard,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn recovery(&self) -> &SegmentRecoveryReport {
        &self.recovery
    }

    /// The next offset an append will be assigned. Every offset strictly
    /// below this one has been appended *and* fsynced — see the module
    /// documentation's "What 'readable' means".
    pub fn high_water(&self) -> u64 {
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_offset
    }

    /// Rolls this append's own trigger could not complete, since it was
    /// opened. Non-zero means the active segment keeps growing past its
    /// trigger without being sealed — worth an operator's attention — but
    /// never means an append was lost; see [`Writer::roll_failures`].
    pub fn roll_failures(&self) -> u64 {
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .roll_failures
    }

    /// Append one batch, encoding it once and writing it as-is (ADR 0100
    /// §1). Returns the offset it was assigned.
    ///
    /// The offset is not assigned — and is not handed to the caller — until
    /// `fsync` has returned successfully on the bytes just written. This
    /// ordering is the whole of "fsync before an append is acknowledged":
    /// swapping it (advancing the log's notion of what is readable before
    /// the sync call, rather than after) is the red-team finding this
    /// module's own documentation names by number (M3) — a batch counted as
    /// durable before it actually was, so that a crash which truncates the
    /// write `fsync` never finished leaves that offset both "already given
    /// out" and empty, and the very next append reuses it for different
    /// content. A `sync` failure here therefore leaves `next_offset`
    /// untouched: the caller sees the append failed, and no offset was ever
    /// promised to anyone for it.
    pub fn append(&self, batch: &Batch) -> Result<u64> {
        let encoded = batch.encode()?;
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let offset = writer.next_offset;
        let start_byte = writer.active.byte_len();
        writer.active.append(&encoded)?;
        writer.active.sync()?;
        writer.next_offset = offset + 1;
        if offset.is_multiple_of(recovery::INDEX_STRIDE) {
            writer.active_index.push((offset, start_byte));
        }
        if writer.active.batch_bytes() >= self.config.roll_after_bytes
            && self.roll_locked(&mut writer).is_err()
        {
            writer.roll_failures += 1;
        }
        Ok(offset)
    }

    /// Read the batch at `offset`, or `None` if it is not — yet, or any
    /// longer — readable: at or past [`Self::high_water`], or already
    /// retained away by [`Self::retain_before`].
    pub fn read(&self, offset: u64) -> Result<Option<Batch>> {
        let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if offset >= writer.next_offset {
            return Ok(None);
        }
        if offset >= writer.active_start_offset {
            let path = recovery::segment_path(&self.directory, writer.active_start_offset);
            let label = segment_label(writer.active_start_offset);
            let extent = locate_extent(
                &label,
                &path,
                writer.active_start_offset,
                &writer.active_index,
                offset,
            )?;
            return file::read_extent(&label, &path, extent).map(Some);
        }
        for segment in &writer.sealed {
            if offset >= segment.start_offset && offset < segment.end_offset {
                let path = recovery::segment_path(&self.directory, segment.start_offset);
                let label = segment_label(segment.start_offset);
                let extent =
                    locate_extent(&label, &path, segment.start_offset, &segment.index, offset)?;
                return file::read_extent(&label, &path, extent).map(Some);
            }
        }
        Ok(None)
    }

    /// A summary of every segment, sealed and active, ascending by start
    /// offset.
    pub fn segments(&self) -> Vec<SegmentSummary> {
        let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<SegmentSummary> = writer
            .sealed
            .iter()
            .map(|s| SegmentSummary {
                start_offset: s.start_offset,
                end_offset: s.end_offset,
                sealed: true,
                byte_length: s.byte_length,
            })
            .collect();
        out.push(SegmentSummary {
            start_offset: writer.active_start_offset,
            end_offset: writer.next_offset,
            sealed: false,
            byte_length: writer.active.byte_len(),
        });
        out
    }

    /// The seal recorded for the sealed segment starting at `start_offset`,
    /// if one exists. Retention keeps a segment's seal after deleting the
    /// segment's own bytes, so this can still answer after the data is gone
    /// — see the module documentation's "Retention".
    pub fn seal_of(&self, start_offset: u64) -> Result<Option<Seal>> {
        match manifest_read(&self.directory, &seal_key(start_offset))? {
            Some(bytes) => Ok(Some(Seal::from_wire_bytes(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Record that the sealed segment starting at `start_offset` has been
    /// archived — the one fact [`Self::retain_before`] and (per this
    /// packet's brief) the broker's own `archived_through` both read.
    ///
    /// Refuses a segment that is not sealed: archiving is a claim about
    /// immutable bytes, and the active segment is never immutable.
    pub fn mark_archived(&self, start_offset: u64) -> Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if writer.active_start_offset == start_offset {
            return Err(Error::invalid(format!(
                "segment {start_offset} is still the active segment; only a sealed segment can \
                 be archived"
            )));
        }
        if !writer.sealed.iter().any(|s| s.start_offset == start_offset) {
            return Err(Error::not_found(format!(
                "no sealed segment starts at offset {start_offset}"
            )));
        }
        manifest_write(&self.directory, &archived_key(start_offset), b"archived")?;
        writer.archived.insert(start_offset);
        Ok(())
    }

    pub fn is_archived(&self, start_offset: u64) -> Result<bool> {
        Ok(self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .archived
            .contains(&start_offset))
    }

    /// Delete every **sealed** segment whose offsets end at or before
    /// `boundary_offset` — except, for an archive-required stream, one that
    /// [`Self::mark_archived`] has not yet recorded. The active segment is
    /// never eligible: it is not sealed by definition.
    pub fn retain_before(&self, boundary_offset: u64) -> Result<RetentionReport> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let (deleted, withheld) = recovery::eligible_for_retention(
            &writer.sealed,
            boundary_offset,
            self.config.archive_required,
            &writer.archived,
        );
        for start_offset in &deleted {
            recovery::delete_segment_file(&self.directory, *start_offset)?;
        }
        writer.sealed.retain(|s| !deleted.contains(&s.start_offset));
        writer.archived.retain(|s| !deleted.contains(s));
        Ok(RetentionReport { deleted, withheld })
    }

    /// Write a small, keyed record durably: the one manifest primitive both
    /// retention (this module) and a later archive/broker packet read
    /// (this packet's brief). Bounded to one record per caller-supplied
    /// key, written through [`crate::fsio::write_atomic`], which fsyncs the
    /// manifest directory before it returns.
    pub fn manifest_put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        validate_manifest_key(key)?;
        manifest_write(&self.directory, key, bytes)
    }

    pub fn manifest_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        validate_manifest_key(key)?;
        manifest_read(&self.directory, key)
    }

    /// Seal the active segment and open a new, empty one to receive the
    /// next append. Called with the writer lock already held, from
    /// [`Self::append`] only.
    fn roll_locked(&self, writer: &mut Writer) -> Result<()> {
        let start_offset = writer.active_start_offset;
        let record_count = writer.next_offset - start_offset;
        let byte_length = writer.active.byte_len();

        let path = recovery::segment_path(&self.directory, start_offset);
        // The whole file's bytes, header included: a deliberately simple
        // hash of "this file", not a convention about which prefix content
        // starts at that a reader would have to reproduce exactly.
        let bytes = std::fs::read(&path)?;
        let content_hash = ContentHash::sha256_of(&bytes);

        let previous_seal_hash = match writer.sealed.last() {
            Some(previous) => {
                let previous_seal = self.seal_of(previous.start_offset)?.ok_or_else(|| {
                    Error::io(format!(
                        "segment {} was sealed but its own seal record is missing; cannot \
                             chain the new seal to a hash that is not there",
                        previous.start_offset
                    ))
                })?;
                Some(previous_seal.content_hash)
            }
            None => None,
        };

        let seal = Seal {
            segment_start_offset: start_offset,
            record_count,
            byte_length,
            sealed_at: self.config.clock.now(),
            content_hash,
            previous_seal_hash,
        };
        manifest_write(
            &self.directory,
            &seal_key(start_offset),
            &seal.to_wire_bytes()?,
        )?;

        writer.sealed.push(SegmentMeta {
            start_offset,
            end_offset: writer.next_offset,
            byte_length,
            index: std::mem::take(&mut writer.active_index),
        });

        let new_path = recovery::segment_path(&self.directory, writer.next_offset);
        let new_file = SegmentFile::create(&new_path)?;
        crate::fsio::sync_directory(&self.directory);

        writer.active = new_file;
        writer.active_start_offset = writer.next_offset;
        Ok(())
    }
}

fn segment_label(start_offset: u64) -> String {
    format!("segment.{start_offset:020}")
}

/// Find the batch at `target`, seeking to the nearest sparse-index anchor at
/// or before it and decoding forward from there — never rescanning a whole
/// segment for one read.
fn locate_extent(
    label: &str,
    path: &Path,
    segment_start_offset: u64,
    index: &[(u64, u64)],
    target: u64,
) -> Result<file::BatchExtent> {
    let (anchor_offset, anchor_byte) = index
        .iter()
        .rev()
        .find(|(offset, _)| *offset <= target)
        .copied()
        .unwrap_or((segment_start_offset, file::HEADER_LEN as u64));
    let scan = file::scan_from(label, path, anchor_byte)?;
    let want = (target - anchor_offset) as usize;
    scan.extents.get(want).copied().ok_or_else(|| {
        Error::io(format!(
            "segment {label} has no batch at offset {target}; the sparse index and the file \
             have diverged"
        ))
    })
}
