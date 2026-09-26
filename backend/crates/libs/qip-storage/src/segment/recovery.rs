//! Segment recovery: recover and retain segments after restart.
//! See ADR 0100 §3. Filled by SLICE-16.
//!
//! A [`super::log::SegmentLog`] is a directory of segment files named
//! `segment.<start offset, 20 digits>`, each holding the dense range of
//! offsets `[start offset, start offset + batch count)`. Every segment but
//! the newest is expected to be **sealed**: fully written and `fsync`ed
//! before the next segment was ever created, exactly as the WAL engine's
//! checkpoint is complete before it is published (`engine/mod.rs`'s
//! `checkpoint_locked`). That is what makes the two torn-tail rules below
//! different rules rather than one applied twice:
//!
//! * A torn tail in the **newest** segment is the ordinary shape of a crash
//!   mid-append. It is truncated away, and recovery resumes appending from
//!   the offset just past the last verified batch.
//! * A torn tail in **any older** segment cannot have been produced by an
//!   interrupted append — that segment was never being appended to when the
//!   process stopped. It is data loss, and recovery refuses to open rather
//!   than guess how much of it is trustworthy.
//!
//! Corruption — a complete-but-CRC-mismatched batch — is refused the same
//! way regardless of which segment holds it, by propagating
//! [`super::file::scan`]'s `Err` with `?`: this function never converts a
//! corrupt batch into either kind of torn-tail handling, which is the
//! mistake this packet's brief calls out by name — treating corruption in
//! the middle of a segment as a tail to truncate would discard every batch
//! recorded after it along with the corrupt one, silently.
//!
//! # Dense offsets, never reused
//!
//! The offset a batch reads back with is derived **only** from what this
//! module finds durable on disk — a segment's declared start offset plus how
//! many batches actually verify inside it. Nothing here trusts an in-memory
//! counter or a cached "next offset" value carried across a restart: if it
//! did, a batch whose `fsync` never completed before a crash could still be
//! counted as assigned, and the next real append would be given a *different*
//! offset than the one already implied to a caller — the exact reuse this
//! module exists to prevent (red-team finding M3, named in this packet's own
//! `why`).

use qip_core::error::{Error, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::file::{self, BatchExtent, SegmentFile};

/// Every `INDEX_STRIDE`-th offset in a segment gets a sparse index entry.
/// Bounded memory (ADR 0100 §1: "a sparse index and no in-memory payloads")
/// means the index does not grow one entry per batch as a segment fills; a
/// [`super::log::SegmentLog::read`] instead seeks to the nearest indexed
/// anchor at or before the target offset and decodes forward from there.
pub(crate) const INDEX_STRIDE: u64 = 8;

/// One sealed segment's metadata: its offset range, its byte length, and its
/// sparse index. No batch payload is held here or anywhere else in this
/// module — a read always goes back to disk.
#[derive(Clone, Debug)]
pub(crate) struct SegmentMeta {
    pub(crate) start_offset: u64,
    /// One past the last offset this segment holds.
    pub(crate) end_offset: u64,
    pub(crate) byte_length: u64,
    /// `(offset, byte start)` pairs, ascending, one every [`INDEX_STRIDE`].
    pub(crate) index: Vec<(u64, u64)>,
}

/// What opening a [`super::log::SegmentLog`] found.
pub(crate) struct Recovered {
    /// Every sealed segment, ascending by start offset.
    pub(crate) sealed: Vec<SegmentMeta>,
    pub(crate) active_start_offset: u64,
    pub(crate) active_file: SegmentFile,
    pub(crate) active_index: Vec<(u64, u64)>,
    /// The next offset an append will be assigned. Always the count of
    /// batches this function actually verified on disk — see the module
    /// documentation.
    pub(crate) next_offset: u64,
    pub(crate) torn_tail_at: Option<u64>,
    pub(crate) bytes_discarded: u64,
    pub(crate) batches_recovered: u64,
}

/// The on-disk path for the segment starting at `start_offset`.
pub(crate) fn segment_path(directory: &Path, start_offset: u64) -> PathBuf {
    directory.join(format!("segment.{start_offset:020}"))
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Segment files present in `directory`, ascending by start offset. Scratch
/// files (`fsio`'s `.qip-partial` suffix) are never mistaken for data, the
/// same rule the WAL engine applies to its own generation files.
fn discover_segment_files(directory: &Path) -> Result<Vec<(u64, PathBuf)>> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(e) => {
            return Err(Error::io(format!(
                "cannot list {}: {e}",
                directory.display()
            )));
        }
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if crate::fsio::is_temporary(&name) {
            continue;
        }
        let Some(digits) = name.strip_prefix("segment.") else {
            continue;
        };
        let start_offset: u64 = digits.parse().map_err(|_| {
            Error::io(format!(
                "{} does not name a segment by its start offset; the directory holds a \
                 file this log did not write",
                entry.path().display()
            ))
        })?;
        found.push((start_offset, entry.path()));
    }
    found.sort_by_key(|(start, _)| *start);
    Ok(found)
}

fn build_sparse_index(start_offset: u64, extents: &[BatchExtent]) -> Vec<(u64, u64)> {
    extents
        .iter()
        .enumerate()
        .filter_map(|(i, extent)| {
            let offset = start_offset + i as u64;
            offset
                .is_multiple_of(INDEX_STRIDE)
                .then_some((offset, extent.start))
        })
        .collect()
}

fn create_first_segment(directory: &Path) -> Result<Recovered> {
    let path = segment_path(directory, 0);
    let file = SegmentFile::create(&path)?;
    crate::fsio::sync_directory(directory);
    Ok(Recovered {
        sealed: Vec::new(),
        active_start_offset: 0,
        active_file: file,
        active_index: Vec::new(),
        next_offset: 0,
        torn_tail_at: None,
        bytes_discarded: 0,
        batches_recovered: 0,
    })
}

/// Recover a [`super::log::SegmentLog`] rooted at `directory`, creating an
/// empty first segment if nothing is there yet.
pub(crate) fn recover(directory: &Path) -> Result<Recovered> {
    std::fs::create_dir_all(directory)?;
    let files = discover_segment_files(directory)?;
    if files.is_empty() {
        return create_first_segment(directory);
    }

    let mut sealed = Vec::new();
    let mut batches_recovered: u64 = 0;
    let last = files.len() - 1;

    for i in 0..last {
        let (start_offset, path) = &files[i];
        let label = display_name(path);
        // `?` here is the whole discipline: a corrupt batch propagates as an
        // `Err` and this loop never continues past it, whether it is the
        // first batch of this segment or the last.
        let scan = file::scan(&label, path)?;
        if let Some(offset) = scan.torn_at {
            return Err(Error::io(format!(
                "segment {label} ends inside a batch at byte offset {offset}, but a newer \
                 segment already exists; a sealed segment is fully written and fsynced \
                 before the next one is created, so a torn tail here is data loss, not an \
                 interrupted write"
            )));
        }
        let end_offset = start_offset + scan.extents.len() as u64;
        let (next_start, next_path) = &files[i + 1];
        if *next_start != end_offset {
            return Err(Error::io(format!(
                "segment {label} holds offsets [{start_offset}, {end_offset}) but the next \
                 segment on disk, {}, starts at {next_start}; a segment is missing between \
                 them",
                display_name(next_path)
            )));
        }
        let index = build_sparse_index(*start_offset, &scan.extents);
        batches_recovered += scan.extents.len() as u64;
        sealed.push(SegmentMeta {
            start_offset: *start_offset,
            end_offset,
            byte_length: scan.valid_end,
            index,
        });
    }

    let (start_offset, path) = &files[last];
    let label = display_name(path);
    let scan = file::scan(&label, path)?;
    let mut active_file = SegmentFile::open_at(path, scan.valid_end)?;
    if scan.torn_at.is_some() {
        // The one place a torn tail is expected and removed rather than
        // raised: the newest segment is the only one that could have been
        // mid-append when the process stopped.
        active_file.truncate_to(scan.valid_end)?;
    }
    let active_index = build_sparse_index(*start_offset, &scan.extents);
    batches_recovered += scan.extents.len() as u64;
    let next_offset = start_offset + scan.extents.len() as u64;

    Ok(Recovered {
        sealed,
        active_start_offset: *start_offset,
        active_file,
        active_index,
        next_offset,
        torn_tail_at: scan.torn_at,
        bytes_discarded: scan.discarded,
        batches_recovered,
    })
}

/// Delete the sealed segment file starting at `start_offset`. The caller
/// (`SegmentLog::retain_before`) has already decided this segment is eligible
/// — archived if its stream requires it, always sealed and never the active
/// segment — so this is pure mechanism, not policy.
pub(crate) fn delete_segment_file(directory: &Path, start_offset: u64) -> Result<()> {
    let path = segment_path(directory, start_offset);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!(
            "cannot delete retained segment {}: {e}",
            path.display()
        ))),
    }
}

/// Segment start offsets eligible for retention, given which ones already
/// carry an archive mark. Kept free of file I/O so the policy — "a sealed
/// segment may be deleted; an archive-required one only once marked" — is
/// exactly the same rule whether or not this build's storage even supports
/// deletion, and so the ordering of which offsets are considered is a
/// [`BTreeSet`], never an iteration order a caller could not reproduce.
pub(crate) fn eligible_for_retention(
    sealed: &[SegmentMeta],
    boundary_offset: u64,
    archive_required: bool,
    archived: &BTreeSet<u64>,
) -> (BTreeSet<u64>, BTreeSet<u64>) {
    let mut deleted = BTreeSet::new();
    let mut withheld = BTreeSet::new();
    for segment in sealed {
        if segment.end_offset > boundary_offset {
            continue;
        }
        if archive_required && !archived.contains(&segment.start_offset) {
            withheld.insert(segment.start_offset);
            continue;
        }
        deleted.insert(segment.start_offset);
    }
    (deleted, withheld)
}
