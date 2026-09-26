//! Partitions over segment logs: high watermark and archived_through.
//! See ADR 0100 §3. Filled by SLICE-27.
//!
//! [`PartitionLog`] is one `(stream, partition)`'s durable data: a thin
//! wrapper over `qip_storage::segment::log::SegmentLog` that adds nothing to
//! the offset or fsync discipline SLICE-16 already proved, and adds exactly
//! one derived fact SLICE-16 does not compute for itself:
//! [`PartitionLog::archived_through`], read straight off the segment log's
//! own archive marks (`SegmentLog::is_archived`) so there is never a second
//! counter that could disagree with them.
//!
//! This module also owns the one fact ADR 0100 §1 requires to be defined
//! once: **where a `(stream, partition)`'s bytes live on disk.**
//! [`open_read_only`] is the only way anything outside this module — the
//! broker included — may derive that path, so SLICE-37's verifier and
//! SLICE-39's replay never re-derive a layout that could drift from what the
//! broker itself uses.
//!
//! # Why the read-only opener cannot be `SegmentLog::open`
//!
//! `SegmentLog::open`'s recovery pass treats a torn tail as a crash to
//! repair: it truncates the file back to the last batch it can verify. That
//! is exactly right for the one process that owns the directory, and exactly
//! wrong for anything else. A second reader racing an in-flight append could
//! see fewer bytes on disk than the writer's own in-memory state already
//! accounts for, read that gap as a crash, and truncate bytes the writer is
//! still in the middle of completing out from under it — corrupting the
//! very partition it was only trying to read. `SegmentLog::open` also takes
//! this process's one-writer-per-directory guard and a write-capable handle
//! on the active segment, neither of which a read-only view may hold.
//! [`ReadOnlyPartition`] therefore never calls `SegmentLog::open`: it lists
//! segment files, opens each strictly read-only, and stops at the first
//! incomplete batch rather than acting on a belief about why it is
//! incomplete — the same three-outcome discipline `Batch::decode` documents,
//! applied by a reader that holds no lock and so can afford to be wrong about
//! a live tail, because it never touches it.

use std::fs::OpenOptions;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use qip_core::error::{Error, Result};
use qip_core::hash::sha256;
use qip_events::event_fabric::codec::{Batch, DecodeOutcome};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};

/// The fixed byte length of a segment file's own header (magic + format
/// version + reserved bytes), restated independently of
/// `qip_storage::segment::file`'s private constant of the same value.
///
/// This is not a second definition of the batch wire format — every batch's
/// own length is still read from its own self-describing, CRC-protected
/// prefix via the public [`Batch::decode`], never assumed here. It is only
/// the fixed offset at which the *first* batch in a segment file begins,
/// which this reader must know to start decoding at all. If it were ever
/// wrong, `Batch::decode` would refuse the resulting bytes as corrupt rather
/// than silently misread them — the fixed point of that decoder's own
/// documentation — so a drift here fails loudly on the very first read
/// rather than quietly. `qip-storage`'s own test suite
/// (`backend/crates/libs/qip-storage/tests/segment.rs`) restates this same
/// fact for the same reason: a reader built to verify a writer proves more
/// by never sharing code with it.
const SEGMENT_FILE_HEADER_LEN: u64 = 16;

fn validate_stream_component(stream: &str) -> Result<()> {
    if stream.trim().is_empty() {
        return Err(Error::invalid(
            "a stream must be named to locate its partition data",
        ));
    }
    let safe = stream
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if !safe {
        return Err(Error::invalid(format!(
            "stream name {stream:?} must contain only ASCII letters, digits, '.', '-' or '_', \
             so it can never name a path outside the partition directory"
        )));
    }
    Ok(())
}

/// The one place a `(stream, partition)` becomes a directory path. Private:
/// every other function in this module goes through it, and nothing outside
/// this module may derive a path independently — see the module
/// documentation.
fn partition_directory(data_dir: &Path, stream: &str, partition: u32) -> Result<PathBuf> {
    validate_stream_component(stream)?;
    Ok(data_dir
        .join("streams")
        .join(stream)
        .join(format!("partition-{partition:05}")))
}

/// Route `key` to a partition of a stream declared with `partition_count`
/// partitions: `u64(sha256(key)[0..8]) mod partition_count`.
///
/// Pure and total for any non-zero count, so the same key always names the
/// same partition for the lifetime of a stream's declared partition count —
/// which is exactly what "no API merges partitions" (this packet's own
/// constraint) depends on: nothing here ever reads more than one partition's
/// key range at a time, because there is no operation that could.
///
/// Refuses `partition_count == 0` rather than dividing by it: a stream with
/// no partitions names nowhere for a key to route to, which is a caller
/// error to refuse, not a value to clamp up to one and silently accept.
pub fn partition_for(key: &str, partition_count: u32) -> Result<u32> {
    if partition_count == 0 {
        return Err(Error::invalid(
            "a stream's partition count must be at least one; zero names no partition a key \
             could route to",
        ));
    }
    let digest = sha256(key.as_bytes());
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    let hashed = u64::from_be_bytes(prefix);
    let index = hashed % u64::from(partition_count);
    // `index` is strictly less than `partition_count`, itself a `u32`, so
    // this narrowing conversion can never lose a bit.
    Ok(index as u32)
}

/// One `(stream, partition)`'s durable segment data. See the module
/// documentation.
#[derive(Debug)]
pub struct PartitionLog {
    log: SegmentLog,
}

impl PartitionLog {
    /// Open (or create) the segment log for `(stream, partition)` rooted at
    /// `data_dir`, recovering whatever a previous run left.
    pub fn open(
        data_dir: &Path,
        stream: &str,
        partition: u32,
        config: SegmentLogConfig,
    ) -> Result<Self> {
        let directory = partition_directory(data_dir, stream, partition)?;
        let log = SegmentLog::open(directory, config)?;
        Ok(Self { log })
    }

    /// Append one already-stamped batch. Delegates entirely to
    /// [`SegmentLog::append`]: this type adds nothing to that call's
    /// fsync-before-offset discipline.
    pub fn append(&self, batch: &Batch) -> Result<u64> {
        self.log.append(batch)
    }

    /// Read the batch at `offset`, or `None` if it is not — yet, or any
    /// longer — readable. See [`SegmentLog::read`].
    pub fn read(&self, offset: u64) -> Result<Option<Batch>> {
        self.log.read(offset)
    }

    /// The next offset an append will be assigned; every offset below it has
    /// been appended and fsynced. See [`SegmentLog::high_water`].
    pub fn high_water(&self) -> u64 {
        self.log.high_water()
    }

    /// Record that the sealed segment starting at `start_offset` has been
    /// archived. See [`SegmentLog::mark_archived`].
    pub fn mark_archived(&self, start_offset: u64) -> Result<()> {
        self.log.mark_archived(start_offset)
    }

    /// Every sealed segment's start offset, ascending — the candidate list
    /// an archiver (SLICE-21 through SLICE-38) considers for upload before
    /// calling [`Self::mark_archived`] on whichever one it finished. Never
    /// includes the active segment, which is never sealed by definition.
    pub fn sealed_segment_starts(&self) -> Vec<u64> {
        self.log
            .segments()
            .into_iter()
            .filter(|summary| summary.sealed)
            .map(|summary| summary.start_offset)
            .collect()
    }

    /// The highest offset below which every batch has been durably archived.
    ///
    /// Derived by walking this partition's own segments, ascending, and
    /// stopping at the first one that is not sealed or not yet marked
    /// archived — [`SegmentLog::segments`] and [`SegmentLog::is_archived`]
    /// are the **only** facts this reads. There is no second counter this
    /// broker maintains alongside them: an offset this function reports as
    /// archived is, by construction, an offset the segment log's own archive
    /// record already covers, and it can never exceed [`Self::high_water`]
    /// because only a sealed segment (always strictly behind the active one)
    /// is ever counted.
    pub fn archived_through(&self) -> Result<u64> {
        let mut through = 0u64;
        for summary in self.log.segments() {
            if !summary.sealed {
                // `SegmentLog::segments` always yields sealed segments
                // ascending, then the active one last (see that method's own
                // implementation): the active segment is never archived, so
                // reaching it ends the walk.
                break;
            }
            if !self.log.is_archived(summary.start_offset)? {
                break;
            }
            through = summary.end_offset;
        }
        Ok(through)
    }
}

/// A read-only view over one partition's segment data. See the module
/// documentation for why this cannot be, and does not share code with,
/// [`SegmentLog::open`].
#[derive(Debug)]
pub struct ReadOnlyPartition {
    directory: PathBuf,
}

impl ReadOnlyPartition {
    /// Every complete batch on disk at or after `from_offset`, in offset
    /// order.
    ///
    /// Stops at the first torn or absent extent rather than guessing past
    /// it — the same rule a live writer's own recovery applies to its
    /// newest segment, reached here independently. A batch this returns may
    /// lag a concurrently-running broker's true high watermark (this reader
    /// takes no lock and asks for none), but it never fabricates, reorders
    /// or half-returns one.
    pub fn read_from(&self, from_offset: u64) -> Result<Vec<Batch>> {
        let mut out = Vec::new();
        for (start_offset, path) in self.segment_files()? {
            let mut file = OpenOptions::new().read(true).open(&path).map_err(|e| {
                Error::io(format!(
                    "cannot open segment {} read-only: {e}",
                    path.display()
                ))
            })?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|e| Error::io(format!("cannot read segment {}: {e}", path.display())))?;
            if (bytes.len() as u64) < SEGMENT_FILE_HEADER_LEN {
                // A segment file's header is written and flushed before
                // anything else touches it (`SegmentFile::create`), so fewer
                // bytes than that means this file is still being created by
                // a concurrent writer. Stop rather than guess how much of it
                // to trust.
                break;
            }
            let mut offset = start_offset;
            let mut cursor = SEGMENT_FILE_HEADER_LEN as usize;
            let mut torn = false;
            while cursor < bytes.len() {
                match Batch::decode(&bytes[cursor..])? {
                    DecodeOutcome::Complete(batch) => {
                        // Re-encoding the batch this decode just produced
                        // reproduces exactly the bytes it was decoded from —
                        // `Batch::encode` is a pure function of the fields
                        // `Batch::decode` just populated — which is how this
                        // reader locates the next batch's start without
                        // duplicating any private knowledge of the wire
                        // format's internal layout.
                        let width = batch.encode()?.len();
                        if offset >= from_offset {
                            out.push(batch);
                        }
                        offset += 1;
                        cursor += width;
                    }
                    DecodeOutcome::Torn => {
                        torn = true;
                        break;
                    }
                }
            }
            if torn {
                // The ordinary shape of a live tail this reader raced. Stop
                // the whole scan here rather than continuing to (what would
                // have to be) a newer segment file: a torn tail can only be
                // legitimate in the newest segment a writer holds open.
                break;
            }
        }
        Ok(out)
    }

    /// Segment files present in this partition's directory, ascending by
    /// start offset. A missing directory (nothing has ever been produced to
    /// this partition) is reported as no files, never an error: a reader
    /// must never create the directory it was only asked to read.
    fn segment_files(&self) -> Result<Vec<(u64, PathBuf)>> {
        let mut found = Vec::new();
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
            Err(e) => {
                return Err(Error::io(format!(
                    "cannot list {}: {e}",
                    self.directory.display()
                )));
            }
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(start) = segment_start_from_name(&name) {
                found.push((start, entry.path()));
            }
        }
        found.sort_by_key(|(start, _)| *start);
        Ok(found)
    }
}

/// The start offset a segment file's own name encodes, or `None` for
/// anything else in the directory — the manifest subdirectory, a
/// `.qip-partial` scratch file, or any name this reader did not write.
///
/// Pure and I/O-free on purpose: this is the one rule that decides which
/// files in a partition's directory this reader treats as segment data, and
/// keeping it free of any real directory listing is what makes it testable
/// without one — a scratch file mistaken for a segment would be read as
/// truncated, valid-looking batch bytes instead of being skipped.
fn segment_start_from_name(name: &str) -> Option<u64> {
    name.strip_prefix("segment.")?.parse().ok()
}

/// Open a read-only view of `(stream, partition)`'s segment data rooted at
/// `data_dir`. The only way anything outside this module may reach a
/// partition's bytes without becoming its writer — see the module
/// documentation.
pub fn open_read_only(data_dir: &Path, stream: &str, partition: u32) -> Result<ReadOnlyPartition> {
    let directory = partition_directory(data_dir, stream, partition)?;
    Ok(ReadOnlyPartition { directory })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_key_always_names_the_same_partition_of_a_fixed_count() {
        let count = 8;
        for key in ["orders-abc", "orders-def", "cell-eu-1", "cell-us-2"] {
            let first = partition_for(key, count).unwrap();
            let second = partition_for(key, count).unwrap();
            assert_eq!(
                first, second,
                "key {key:?} must route to the same partition on every call"
            );
            assert!(first < count, "a returned partition must be in range");
        }
    }

    #[test]
    fn a_zero_partition_count_is_refused_rather_than_treated_as_one() {
        let error = partition_for("any-key", 0).unwrap_err();
        assert!(
            error.to_string().contains("at least one"),
            "the refusal must name why a zero count cannot route anything: {error}"
        );
    }

    #[test]
    fn a_segment_file_name_parses_to_its_start_offset_and_nothing_else_does() {
        assert_eq!(
            segment_start_from_name("segment.00000000000000000000"),
            Some(0)
        );
        assert_eq!(
            segment_start_from_name("segment.00000000000000000005"),
            Some(5)
        );
        assert_eq!(
            segment_start_from_name("segment.00000000000000000005.qip-partial"),
            None,
            "a scratch file left behind by an in-progress write must never be read as a segment"
        );
        assert_eq!(
            segment_start_from_name("manifest"),
            None,
            "the manifest subdirectory must never be read as a segment"
        );
    }

    #[test]
    fn segment_names_discovered_in_any_order_sort_to_a_dense_ascending_list_with_no_duplicate() {
        // Deliberately out of order and salted with two names that must be
        // ignored, mirroring exactly what `std::fs::read_dir` could hand
        // back in one real directory listing.
        let names = [
            "segment.00000000000000000005",
            "manifest",
            "segment.00000000000000000000",
            "segment.00000000000000000005.qip-partial",
        ];
        let mut starts: Vec<u64> = names
            .iter()
            .filter_map(|name| segment_start_from_name(name))
            .collect();
        assert_eq!(
            starts.len(),
            2,
            "premise: exactly two of the four names are genuine segment files"
        );
        starts.sort_unstable();
        assert_eq!(starts, vec![0, 5]);
    }
}
