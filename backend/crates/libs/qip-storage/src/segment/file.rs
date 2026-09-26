//! Segment file format: frames and torn-tail rules, shared with the WAL
//! engine and the edge spool. See ADR 0100 §1. Filled by SLICE-16.
//!
//! A segment file is a small fixed header (mirroring
//! [`crate::engine::frame`]'s own header, but with its own magic so the two
//! are never confused) followed by any number of event-fabric batches, each
//! written once by [`qip_events::event_fabric::codec::Batch::encode`] and
//! appended byte-for-byte as the drain or broker produced it (ADR 0100 §1:
//! "a batch is written once… and appended as-is"). This module does not
//! re-implement that framing: every batch is already self-delimiting (its
//! own magic, declared length, prefix CRC32C and body CRC32C — see the codec
//! module's doc for the exact wire table), so scanning a segment is a loop
//! over [`Batch::decode`], not a second, competing frame format.
//!
//! ## Corruption rules, restated for a segment
//!
//! Reading forward through a segment produces exactly the codec's own three
//! outcomes, and the discipline recovery depends on is what a caller does
//! with each:
//!
//! * **Complete** — the batch is durable data; its byte extent is recorded
//!   and the scan advances past it.
//! * **Torn** — the file ends before the declared length. This is the
//!   ordinary shape of a crash mid-append and is *never* an error: the scan
//!   stops here and reports the offset, and the caller (recovery, in
//!   `segment/recovery.rs`) decides whether a torn tail at this position is
//!   expected (the newest segment) or is itself data loss (an older,
//!   supposedly-sealed one).
//! * **Corrupt** — [`Batch::decode`] returns `Err`, and this module never
//!   converts that into a `Torn` result or swallows it to keep scanning.
//!   Doing so would be exactly the gap SLICE-06's review found in the codec
//!   itself before the prefix CRC existed: a corrupted length field made a
//!   corrupt batch indistinguishable from an ordinary torn tail, and a
//!   recovery path that truncates a torn tail would then silently discard
//!   every batch appended after the corruption too. [`scan`] propagates the
//!   error with `?`, so a caller that also uses `?` cannot make that mistake
//!   by construction — there is no intermediate value to misclassify.

use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

/// Identifies a file written by this module. Distinct from the WAL engine's
/// `QIPSTOR` magic (`engine/frame.rs`) and from the event-fabric batch magic
/// `QEVB` (`qip_events::event_fabric::codec::BATCH_MAGIC`) so a reader handed
/// the wrong file, or the wrong offset within the right one, refuses
/// immediately rather than misparsing it.
pub(crate) const SEGMENT_MAGIC: [u8; 8] = *b"QIPSEGL\x01";

/// Bumped only for an incompatible layout change.
pub(crate) const FORMAT_VERSION: u32 = 1;

/// magic (8) + format version (4) + reserved (4).
pub(crate) const HEADER_LEN: usize = 16;

/// The event-fabric batch prefix that precedes every batch's body: magic,
/// format version, declared body length and the prefix's own CRC32C, four
/// bytes each (`4 + 2 + 4 + 4`, since format version is a `u16`). Restated
/// here, rather than imported, because `qip_events::event_fabric::codec`
/// keeps this width private — it is the codec's own implementation detail,
/// not part of its public contract — and this module only ever needs it to
/// locate where one encoded batch ends and the next begins, which the module
/// documentation there fixes as a stable wire fact: "the prefix — magic,
/// format version and the declared length — carries its own CRC32C".
/// `qip_events::event_fabric::codec::{BATCH_MAGIC, FORMAT_VERSION,
/// MAX_BATCH_LEN}` are exactly the public halves of that same contract, and
/// the length below is the arithmetic of the three private fields those
/// public constants describe, plus the trailing CRC.
const BATCH_PREFIX_LEN: usize = 4 + 2 + 4 + 4;

/// The bytes of a segment file header.
pub(crate) fn file_header() -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(&SEGMENT_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// Validate a segment file's header, naming what is wrong if it is not ours.
pub(crate) fn check_file_header(label: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::io(format!(
            "segment {label} is {} bytes, shorter than the {HEADER_LEN}-byte header",
            bytes.len()
        )));
    }
    if bytes[..8] != SEGMENT_MAGIC {
        return Err(Error::io(format!(
            "segment {label} does not begin with the segment-log magic; \
             it was not written by this log"
        )));
    }
    let mut version = [0u8; 4];
    version.copy_from_slice(&bytes[8..12]);
    let version = u32::from_le_bytes(version);
    if version != FORMAT_VERSION {
        return Err(Error::schema(format!(
            "segment {label} is format version {version}, this build reads version {FORMAT_VERSION}"
        )));
    }
    Ok(())
}

/// Restate a codec error's message with an absolute file position, keeping
/// its error class. The codec's own message names an offset relative to the
/// slice it was handed, which is always 0 for the prefix checks it can fail
/// on; a caller here always passes a slice starting partway through a file,
/// so the raw message alone would point nowhere an operator could find with
/// a hex dump.
fn at_absolute_offset(error: Error, label: &str, offset: u64) -> Error {
    let message = format!(
        "segment {label} at file byte offset {offset}: {}",
        error.message()
    );
    error.relabelled(message)
}

/// The declared body length of the batch beginning at the start of `prefix`.
///
/// `prefix` must be at least [`BATCH_PREFIX_LEN`] bytes — callers only reach
/// this after [`Batch::decode`] has already validated the same bytes and
/// returned [`DecodeOutcome::Complete`], which cannot happen unless the
/// buffer held at least that many bytes.
fn declared_body_len(prefix: &[u8]) -> usize {
    u32::from_le_bytes([prefix[6], prefix[7], prefix[8], prefix[9]]) as usize
}

/// One batch's byte extent within a segment file: `[start, end)`, header
/// included in neither. No payload is retained — a batch is re-decoded from
/// disk on demand, never cached, so a segment's dense extent list is a
/// scan-time scratch value and never the shape of what stays resident.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BatchExtent {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

/// The result of scanning one segment file from its header to its end.
#[derive(Clone, Debug)]
pub(crate) struct SegmentFileScan {
    /// Every complete, digest-verified batch, in file order.
    pub(crate) extents: Vec<BatchExtent>,
    /// Offset just past the last verified batch — where appends resume.
    pub(crate) valid_end: u64,
    /// Offset at which an incomplete batch began, if the tail was torn.
    pub(crate) torn_at: Option<u64>,
    /// Bytes after `valid_end`, all of which are discarded.
    pub(crate) discarded: u64,
}

/// Read every batch in the segment file at `path`.
///
/// A torn tail is reported, not raised — see the module documentation.
/// Corruption inside a complete batch is raised via `?` and never converted
/// to a torn-tail result, which is the one rule this function exists to
/// enforce: a caller (recovery) that also propagates this `Err` cannot
/// mistake a corrupt batch, and everything durable after it, for an ordinary
/// crash tail and truncate them away together.
pub(crate) fn scan(label: &str, path: &Path) -> Result<SegmentFileScan> {
    let bytes = std::fs::read(path).map_err(|e| {
        Error::io(format!(
            "cannot read segment {}, which the log expects to find: {e}",
            path.display()
        ))
    })?;
    check_file_header(label, &bytes)?;
    scan_body(label, &bytes, HEADER_LEN)
}

/// Scan forward from `start_byte`, which must already be the start of a
/// batch (never the file header) — used by [`super::log::locate_extent`] to
/// walk forward from the nearest sparse-index anchor to a target offset
/// instead of rescanning a whole segment for one read.
pub(crate) fn scan_from(label: &str, path: &Path, start_byte: u64) -> Result<SegmentFileScan> {
    let bytes = std::fs::read(path).map_err(|e| {
        Error::io(format!(
            "cannot read segment {}, which a read expects to find: {e}",
            path.display()
        ))
    })?;
    scan_body(label, &bytes, start_byte as usize)
}

/// The one decode loop both [`scan`] and [`scan_from`] drive, so a segment's
/// torn-tail and corruption rules are expressed exactly once.
fn scan_body(label: &str, bytes: &[u8], start: usize) -> Result<SegmentFileScan> {
    let mut extents = Vec::new();
    let mut offset = start;
    let mut torn_at = None;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        match Batch::decode(remaining).map_err(|e| at_absolute_offset(e, label, offset as u64))? {
            DecodeOutcome::Complete(_) => {
                let end = offset + BATCH_PREFIX_LEN + declared_body_len(remaining);
                extents.push(BatchExtent {
                    start: offset as u64,
                    end: end as u64,
                });
                offset = end;
            }
            DecodeOutcome::Torn => {
                torn_at = Some(offset as u64);
                break;
            }
        }
    }

    Ok(SegmentFileScan {
        extents,
        valid_end: offset as u64,
        torn_at,
        discarded: bytes.len() as u64 - offset as u64,
    })
}

/// Read exactly the bytes of one already-located batch and decode it.
///
/// Used by [`super::log::SegmentLog::read`] once the sparse index has named
/// an extent: a read costs a seek and the size of the one batch it fetches,
/// never a rescan of the whole segment and never a cached copy held between
/// calls (ADR 0100 §1's bounded-memory constraint: "no payload cache").
pub(crate) fn read_extent(label: &str, path: &Path, extent: BatchExtent) -> Result<Batch> {
    use std::io::Read;
    let mut file = File::open(path).map_err(|e| {
        Error::io(format!(
            "cannot open segment {} to read a batch: {e}",
            path.display()
        ))
    })?;
    let len = (extent.end - extent.start) as usize;
    let mut buffer = vec![0u8; len];
    file.seek(SeekFrom::Start(extent.start))?;
    file.read_exact(&mut buffer)?;
    match Batch::decode(&buffer).map_err(|e| at_absolute_offset(e, label, extent.start))? {
        DecodeOutcome::Complete(batch) => Ok(batch),
        DecodeOutcome::Torn => Err(Error::io(format!(
            "segment {label} at file byte offset {}: the sparse index named a batch \
             that no longer decodes complete; the index and the file have diverged",
            extent.start
        ))),
    }
}

/// An open segment file positioned for appends.
#[derive(Debug)]
pub(crate) struct SegmentFile {
    file: File,
    /// Bytes currently in the file, header included.
    length: u64,
}

impl SegmentFile {
    /// Create a new segment file, replacing any file already at `path`.
    ///
    /// The header is written and flushed before the call returns, so a
    /// segment named by the log's own bookkeeping always has a readable
    /// header, even if nothing has been appended to it yet.
    pub(crate) fn create(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        let header = file_header();
        file.write_all(&header)?;
        file.sync_all()?;
        Ok(Self {
            file,
            length: header.len() as u64,
        })
    }

    /// Open an existing segment file for appending, positioned at `length`.
    ///
    /// `length` comes from recovery and is the offset just past the last
    /// batch that verified; anything after it was a torn tail and has
    /// already been removed by the caller.
    pub(crate) fn open_at(path: &Path, length: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(path)
            .map_err(|e| Error::io(format!("cannot open {} for appending: {e}", path.display())))?;
        Ok(Self { file, length })
    }

    /// Bytes of batches, excluding the fixed header.
    pub(crate) fn batch_bytes(&self) -> u64 {
        self.length.saturating_sub(HEADER_LEN as u64)
    }

    pub(crate) fn byte_len(&self) -> u64 {
        self.length
    }

    /// Append already-encoded batch bytes as-is (ADR 0100 §1). This does
    /// **not** fsync; the caller decides when to pay for the barrier, which
    /// for [`super::log::SegmentLog::append`] is always before it returns —
    /// see that function's own documentation for why every append pays its
    /// own sync rather than sharing one across a batch of appends.
    pub(crate) fn append(&mut self, encoded: &[u8]) -> Result<u64> {
        self.file.write_all(encoded)?;
        self.length += encoded.len() as u64;
        Ok(encoded.len() as u64)
    }

    /// Flush the file's data and metadata to the storage device.
    pub(crate) fn sync(&self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

    /// Cut the file back to `length`, discarding a torn tail, and flush.
    pub(crate) fn truncate_to(&mut self, length: u64) -> Result<()> {
        self.file.set_len(length)?;
        self.file.sync_all()?;
        self.length = length;
        Ok(())
    }
}
