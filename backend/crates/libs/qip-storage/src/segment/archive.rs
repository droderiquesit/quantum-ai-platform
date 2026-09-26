//! Segment archive: content-addressed manifest and hydrate. See ADR 0100 §3.
//!
//! ADR 0100 §3's producer-retained durability rests on one fact: a producer
//! may only forget a record once the broker reports the sealed segment
//! holding it has actually been archived (`archived_through`). Before this
//! module, [`super::log::SegmentLog::mark_archived`] existed but nothing ever
//! called it against a real copy of the bytes — the mark and the archive
//! were two separate claims that had never been made to agree, which is
//! exactly the shape of control `.claude/rules/domains/risk-and-execution.md`
//! calls out by name: one that looks like protection and cannot fire. This
//! module is what makes the mark true: [`Archiver::archive`] copies a sealed
//! segment's exact bytes into a [`super::super::blob::BlobStore`] under the
//! SHA-256 of those bytes, reads the upload back and checks the digest
//! again, and only then calls `mark_archived` — never the other way round.
//!
//! # Content addressing, not offset addressing
//!
//! An archived segment's key is `blob_key`, a pure function of its
//! [`qip_events::event_fabric::codec::ContentHash`] alone. Nothing here ever
//! derives a key from a segment's start offset. That is not a style choice:
//! it is what lets [`Manifest::verify`] and [`ArchiveReader::read`] trust an
//! object by recomputing its name from the hash they already have, rather
//! than trusting a name someone else attached to it. A store that named
//! objects by offset instead would still "work" for a single, honest writer
//! and would silently stop proving anything the moment two different byte
//! strings could end up filed under the same name.
//!
//! # What a hydrate must never do
//!
//! [`ArchiveReader::read`] never returns bytes it fetched from the blob store
//! without checking them against the [`Manifest`]'s own content hash first.
//! A hydrate that skipped this would be a restore that can silently serve
//! corrupted or substituted history as if it were the original run — the
//! exact failure this packet's brief names.
//!
//! # What this module does not do
//!
//! It keeps no mark of its own for "is this segment archived": that
//! question has exactly one owner, [`super::log::SegmentLog::is_archived`],
//! which retention and (per this packet's brief) the broker both already
//! read. A second, independent flag here would be the same mistake ADR
//! 0100 already closed once for `archived_through` itself — two claims
//! about one fact that can drift apart.
//!
//! It also never runs on the append path: [`Archiver::archive`] takes a
//! `&SegmentLog` and does no threading of its own, so whichever thread the
//! caller dedicates to archiving is the only thread archiving ever costs.
//!
//! [`ArchiveReader::read`] resolves a sealed, archived segment from
//! [`super::log::SegmentLog::segments`] and [`super::log::SegmentLog::is_archived`],
//! which forget a segment once [`super::log::SegmentLog::retain_before`] has
//! deleted it. Reading a fully retained-away range therefore depends on the
//! caller keeping its own record of which offsets were ever archived — this
//! module does not invent a second one, for the reason above.

use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, ContentHash, DecodeOutcome, LogicalTimestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::blob::BlobStore;

use super::file::{self, BatchExtent};
use super::log::SegmentLog;
use super::recovery;

/// The manifest key one archived segment's record is stored under, via
/// [`SegmentLog::manifest_put`]. Distinct from `SegmentLog`'s own internal
/// `seal.<offset>` and `archived.<offset>` keys (see `segment/log.rs`), so
/// none of the three can ever be read back as one of the others.
fn archive_manifest_key(start_offset: u64) -> String {
    format!("archive-manifest.{start_offset:020}")
}

/// The on-disk label used only for error messages — restated rather than
/// imported from `segment/log.rs`'s private `segment_label`, matching the
/// convention `segment/log.rs`'s own module doc fixes: `segment.<start
/// offset, 20 digits>`.
fn segment_label(start_offset: u64) -> String {
    format!("segment.{start_offset:020}")
}

/// Where a sealed segment's bytes live in the blob store: the hex SHA-256 of
/// its own bytes, and nothing else. See the module documentation's "Content
/// addressing, not offset addressing".
fn blob_key(hash: &ContentHash) -> Result<String> {
    match hash {
        ContentHash::Sha256(digest) => Ok(format!(
            "segments/sha256/{}",
            qip_core::hash::to_hex(digest)
        )),
        ContentHash::Blake3(_) => Err(Error::invalid(
            "a segment cannot be archived under a Blake3 content hash; this build only ever \
             produces Sha256 (ADR 0100: seals and manifests carry SLICE-06's ContentHash tag, \
             Sha256 only)"
                .to_string(),
        )),
    }
}

fn hash_to_wire(hash: &ContentHash) -> Result<(String, String)> {
    match hash {
        ContentHash::Sha256(digest) => Ok(("sha256".to_string(), qip_core::hash::to_hex(digest))),
        ContentHash::Blake3(_) => Err(Error::invalid(
            "a manifest cannot carry a Blake3 content hash; this build only ever produces \
             Sha256"
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

// --- manifest ----------------------------------------------------------

/// One archived segment's evidence: everything a verifier needs to trust the
/// bytes and to know what they are, without asking the live [`SegmentLog`]
/// anything else. See the module documentation for what "trust" means here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Which stream this segment belongs to. `qip-storage` has no notion of
    /// a stream of its own — [`SegmentLog`] is reused as both a broker
    /// partition and the edge spool (`segment/log.rs`'s module doc) — so the
    /// caller supplies it and it travels with the manifest rather than being
    /// inferred from a directory layout this crate does not own.
    pub stream: String,
    pub partition: u32,
    /// Inclusive offset range this segment holds, `[base_offset, last_offset]`.
    pub base_offset: u64,
    pub last_offset: u64,
    pub record_count: u64,
    /// The first and last batch's broker-assigned logical timestamp, in file
    /// order — not necessarily the numeric minimum and maximum, since a
    /// well-formed partition's timestamps are already non-decreasing on
    /// admission (`codec.rs`'s own field-ownership table).
    pub first_logical_timestamp: LogicalTimestamp,
    pub last_logical_timestamp: LogicalTimestamp,
    /// The segment file's total byte length, header included — the same
    /// quantity [`super::log::Seal::byte_length`] carries.
    pub bytes: u64,
    /// This segment's own content hash: what [`blob_key`] addresses it by.
    pub content_hash: ContentHash,
    /// The content hash of the segment sealed immediately before this one,
    /// or `None` for the first segment this log ever sealed — copied from
    /// [`super::log::Seal::previous_seal_hash`] so a chain of manifests can
    /// be verified on its own, the same property
    /// [`super::log::verify_seal_chain`] gives the live log's seals
    /// (`chain.rs`'s `ChainArchive` is the precedent for chaining an
    /// archive's own records independently of the log they were drawn from).
    pub chain_in: Option<ContentHash>,
    /// This segment's own hash, restated under the name the *next* segment's
    /// `chain_in` will carry. Always equal to `content_hash` in this build —
    /// the two are kept as separate fields because a manifest is read on its
    /// own, without the addressing convention in scope, and "what is this
    /// segment addressed by" and "what does the next segment chain onto" are
    /// two questions even though this build answers them identically.
    pub chain_out: ContentHash,
    /// Data-licensing entitlements this segment's records were produced
    /// under. `qip-storage` has no licensing model of its own (that is
    /// `qip-compliance`'s and `qip-data-finder`'s domain) — a caller that
    /// does supplies the labels, and a `BTreeSet` rather than a `Vec` so two
    /// archivers describing the same segment always write byte-identical
    /// manifests regardless of the order entitlements were collected in.
    pub entitlements: BTreeSet<String>,
    /// `(offset, byte start within the archived object)`, ascending, one
    /// entry per record. Bounded by `record_count`, which a sealed segment
    /// already bounds via `SegmentLogConfig::roll_after_bytes` — the same
    /// bounded-memory constraint ADR 0100 §1 states for the hot log's own
    /// sparse index, restated here for the archived copy.
    pub replay_index: Vec<(u64, u64)>,
}

impl Manifest {
    /// Confirm `bytes` really are the segment this manifest describes.
    ///
    /// Checks the SHA-256 digest, not merely the length: two byte strings of
    /// the same length can differ anywhere in the middle, and a check that
    /// stopped at the length would let a single flipped byte — anywhere in
    /// an archived segment — through undetected. That is the named failure
    /// this method exists to close, not a hypothetical one: it is exactly
    /// the mutation this method's own test applies.
    pub fn verify(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 != self.bytes {
            return Err(Error::invalid(format!(
                "segment [{}, {}]: manifest declares {} bytes, the object holds {}",
                self.base_offset,
                self.last_offset,
                self.bytes,
                bytes.len()
            )));
        }
        let actual = ContentHash::sha256_of(bytes);
        if actual != self.content_hash {
            return Err(Error::invalid(format!(
                "segment [{}, {}]: fails its manifest's content hash; the archived object does \
                 not match what was sealed",
                self.base_offset, self.last_offset
            )));
        }
        Ok(())
    }

    fn to_wire_bytes(&self) -> Result<Vec<u8>> {
        let (content_hash_algorithm, content_hash_hex) = hash_to_wire(&self.content_hash)?;
        let (chain_in_algorithm, chain_in_hex) = match &self.chain_in {
            Some(hash) => {
                let (algorithm, hex) = hash_to_wire(hash)?;
                (Some(algorithm), Some(hex))
            }
            None => (None, None),
        };
        let (chain_out_algorithm, chain_out_hex) = hash_to_wire(&self.chain_out)?;
        let wire = ManifestWire {
            stream: self.stream.clone(),
            partition: self.partition,
            base_offset: self.base_offset,
            last_offset: self.last_offset,
            record_count: self.record_count,
            first_physical_ns: self.first_logical_timestamp.physical_ns,
            first_logical: self.first_logical_timestamp.logical,
            last_physical_ns: self.last_logical_timestamp.physical_ns,
            last_logical: self.last_logical_timestamp.logical,
            bytes: self.bytes,
            content_hash_algorithm,
            content_hash_hex,
            chain_in_algorithm,
            chain_in_hex,
            chain_out_algorithm,
            chain_out_hex,
            entitlements: self.entitlements.clone(),
            replay_index: self.replay_index.clone(),
        };
        Ok(serde_json::to_vec(&wire)?)
    }

    fn from_wire_bytes(bytes: &[u8]) -> Result<Self> {
        let wire: ManifestWire = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("archive manifest does not parse: {e}")))?;
        let content_hash = hash_from_wire(
            &wire.content_hash_algorithm,
            &wire.content_hash_hex,
            "manifest content hash",
        )?;
        let chain_out = hash_from_wire(
            &wire.chain_out_algorithm,
            &wire.chain_out_hex,
            "manifest chain-out hash",
        )?;
        let chain_in = match (wire.chain_in_algorithm, wire.chain_in_hex) {
            (Some(algorithm), Some(hex)) => {
                Some(hash_from_wire(&algorithm, &hex, "manifest chain-in hash")?)
            }
            (None, None) => None,
            _ => {
                return Err(Error::schema(
                    "archive manifest's chain-in hash fields are inconsistent: one is present \
                     and the other is not"
                        .to_string(),
                ));
            }
        };
        Ok(Manifest {
            stream: wire.stream,
            partition: wire.partition,
            base_offset: wire.base_offset,
            last_offset: wire.last_offset,
            record_count: wire.record_count,
            first_logical_timestamp: LogicalTimestamp {
                physical_ns: wire.first_physical_ns,
                logical: wire.first_logical,
            },
            last_logical_timestamp: LogicalTimestamp {
                physical_ns: wire.last_physical_ns,
                logical: wire.last_logical,
            },
            bytes: wire.bytes,
            content_hash,
            chain_in,
            chain_out,
            entitlements: wire.entitlements,
            replay_index: wire.replay_index,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ManifestWire {
    stream: String,
    partition: u32,
    base_offset: u64,
    last_offset: u64,
    record_count: u64,
    first_physical_ns: i64,
    first_logical: u32,
    last_physical_ns: i64,
    last_logical: u32,
    bytes: u64,
    content_hash_algorithm: String,
    content_hash_hex: String,
    chain_in_algorithm: Option<String>,
    chain_in_hex: Option<String>,
    chain_out_algorithm: String,
    chain_out_hex: String,
    entitlements: BTreeSet<String>,
    replay_index: Vec<(u64, u64)>,
}

// --- archiver ------------------------------------------------------------

/// Copies sealed segments into a [`BlobStore`] under their own content hash,
/// verifying the upload before advancing `archived_through` (via
/// [`SegmentLog::mark_archived`]). See the module documentation.
#[derive(Debug)]
pub struct Archiver {
    blob_store: Arc<dyn BlobStore>,
}

impl Archiver {
    pub fn new(blob_store: Arc<dyn BlobStore>) -> Self {
        Self { blob_store }
    }

    /// Archive the sealed segment starting at `start_offset` in `log`.
    ///
    /// A no-op if the segment is already archived: the manifest already on
    /// record is read back and returned rather than the segment's bytes
    /// being uploaded a second time. This is what makes a retried archive
    /// report (the caller's, not this module's own) safe to retry freely.
    ///
    /// Order of operations matters and is the whole point of this method:
    /// the bytes are uploaded, the upload is read back and checked against
    /// the manifest's own content hash, and only *then* is
    /// [`SegmentLog::mark_archived`] called. Reversing the last two steps is
    /// this method's own named failure mode (see the module documentation
    /// and this crate's test suite) — a mark that can be set on an
    /// unverified upload is not evidence of anything.
    pub fn archive(
        &self,
        log: &SegmentLog,
        start_offset: u64,
        stream: &str,
        partition: u32,
        entitlements: &BTreeSet<String>,
    ) -> Result<Manifest> {
        if stream.is_empty() {
            return Err(Error::invalid(
                "a segment cannot be archived without naming the stream it belongs to".to_string(),
            ));
        }
        if log.is_archived(start_offset)? {
            return self.stored_manifest(log, start_offset);
        }

        let seal = log.seal_of(start_offset)?.ok_or_else(|| {
            Error::not_found(format!(
                "no sealed segment starts at offset {start_offset}; only a sealed segment can \
                 be archived"
            ))
        })?;
        if seal.record_count == 0 {
            return Err(Error::invalid(format!(
                "segment {start_offset}'s seal declares zero records; a manifest cannot \
                 describe an empty segment"
            )));
        }
        let last_offset = start_offset
            .checked_add(seal.record_count - 1)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "segment {start_offset} plus its {} records overflows a u64 offset",
                    seal.record_count
                ))
            })?;

        let label = segment_label(start_offset);
        let path = recovery::segment_path(log.directory(), start_offset);
        let bytes = std::fs::read(&path).map_err(|e| {
            Error::io(format!(
                "cannot read sealed segment {start_offset} to archive it: {e}"
            ))
        })?;

        // Defend against a segment whose bytes have already drifted from its
        // own seal (disk corruption, or a bug upstream of this module):
        // archiving it anyway would enshrine that drift as a "verified" copy.
        let content_hash = ContentHash::sha256_of(&bytes);
        if content_hash != seal.content_hash {
            return Err(Error::invalid(format!(
                "segment {start_offset} on disk no longer matches its own seal; refusing to \
                 archive a segment that already disagrees with what was sealed"
            )));
        }

        let scan = file::scan(&label, &path)?;
        if scan.torn_at.is_some() {
            return Err(Error::invalid(format!(
                "segment {start_offset} is sealed but its scan found a torn tail; a sealed \
                 segment is never appended to again, so this is corruption, not an interrupted \
                 write"
            )));
        }
        if scan.extents.len() as u64 != seal.record_count {
            return Err(Error::invalid(format!(
                "segment {start_offset}'s seal declares {} records but {} decode; the seal and \
                 the file have diverged",
                seal.record_count,
                scan.extents.len()
            )));
        }

        let first_logical_timestamp = decode_extent(
            &label,
            &bytes,
            *scan.extents.first().ok_or_else(|| {
                Error::io(format!(
                    "segment {start_offset} has a non-zero record count but no extents"
                ))
            })?,
        )?
        .logical_timestamp;
        let last_logical_timestamp = decode_extent(
            &label,
            &bytes,
            *scan.extents.last().ok_or_else(|| {
                Error::io(format!(
                    "segment {start_offset} has a non-zero record count but no extents"
                ))
            })?,
        )?
        .logical_timestamp;

        let replay_index: Vec<(u64, u64)> = scan
            .extents
            .iter()
            .enumerate()
            .map(|(i, extent)| (start_offset + i as u64, extent.start))
            .collect();

        let manifest = Manifest {
            stream: stream.to_string(),
            partition,
            base_offset: start_offset,
            last_offset,
            record_count: seal.record_count,
            first_logical_timestamp,
            last_logical_timestamp,
            bytes: seal.byte_length,
            content_hash: content_hash.clone(),
            chain_in: seal.previous_seal_hash.clone(),
            chain_out: content_hash.clone(),
            entitlements: entitlements.clone(),
            replay_index,
        };

        let key = blob_key(&content_hash)?;
        self.blob_store.put(&key, bytes)?;

        // The verified-upload check this whole module exists for: read the
        // object back under the same content-addressed key and confirm it
        // against the manifest, before anything durable records this
        // segment as archived.
        let round_tripped = self.blob_store.get(&key)?.ok_or_else(|| {
            Error::io(format!(
                "segment {start_offset} was written to {key} but cannot be read back; refusing \
                 to mark it archived on an unverified upload"
            ))
        })?;
        manifest.verify(&round_tripped)?;

        log.manifest_put(
            &archive_manifest_key(start_offset),
            &manifest.to_wire_bytes()?,
        )?;
        // The one durable mark this packet is allowed to move: SLICE-16's
        // own archived flag, which retention and the broker already read.
        // Nothing above this line is visible to either of them yet.
        log.mark_archived(start_offset)?;

        Ok(manifest)
    }

    fn stored_manifest(&self, log: &SegmentLog, start_offset: u64) -> Result<Manifest> {
        let bytes = log
            .manifest_get(&archive_manifest_key(start_offset))?
            .ok_or_else(|| {
                Error::io(format!(
                    "segment {start_offset} is marked archived but this store holds no manifest \
                     for it; the archived flag and the manifest record have diverged"
                ))
            })?;
        Manifest::from_wire_bytes(&bytes)
    }
}

fn decode_extent(label: &str, bytes: &[u8], extent: BatchExtent) -> Result<Batch> {
    let start = extent.start as usize;
    let end = extent.end as usize;
    if end > bytes.len() || start > end {
        return Err(Error::io(format!(
            "segment {label}: batch extent [{start}, {end}) exceeds the {} bytes just read; a \
             sealed segment must never change between two reads of it",
            bytes.len()
        )));
    }
    match Batch::decode(&bytes[start..end])? {
        DecodeOutcome::Complete(batch) => Ok(batch),
        DecodeOutcome::Torn => Err(Error::io(format!(
            "segment {label} at byte offset {start}: a batch the scan already verified complete \
             no longer decodes complete"
        ))),
    }
}

// --- reader ----------------------------------------------------------------

/// Reads a [`SegmentLog`]'s offsets across both custodians: an archived
/// segment is hydrated (and verified) from the [`BlobStore`], and every
/// other offset is read from the hot log directly. See the module
/// documentation for the one gap this reader has (a fully retained-away
/// range) and why it is not closed here.
#[derive(Debug)]
pub struct ArchiveReader {
    blob_store: Arc<dyn BlobStore>,
}

impl ArchiveReader {
    pub fn new(blob_store: Arc<dyn BlobStore>) -> Self {
        Self { blob_store }
    }

    /// Read the batch at `offset`, from the archive if the segment holding
    /// it is archived, from the hot log otherwise. The two paths yield the
    /// original byte stream identically: this is what "archive-then-hot" in
    /// this packet's objective means.
    pub fn read(&self, log: &SegmentLog, offset: u64) -> Result<Option<Batch>> {
        let containing = log
            .segments()
            .into_iter()
            .find(|s| s.sealed && offset >= s.start_offset && offset < s.end_offset);

        let Some(summary) = containing else {
            return log.read(offset);
        };
        if !log.is_archived(summary.start_offset)? {
            return log.read(offset);
        }
        self.hydrate(log, summary.start_offset, offset).map(Some)
    }

    fn hydrate(&self, log: &SegmentLog, segment_start: u64, offset: u64) -> Result<Batch> {
        let manifest_bytes = log
            .manifest_get(&archive_manifest_key(segment_start))?
            .ok_or_else(|| {
                Error::io(format!(
                    "segment {segment_start} is archived but this store holds no manifest for it"
                ))
            })?;
        let manifest = Manifest::from_wire_bytes(&manifest_bytes)?;

        let key = blob_key(&manifest.content_hash)?;
        let bytes = self.blob_store.get(&key)?.ok_or_else(|| {
            Error::not_found(format!(
                "segment {segment_start}'s manifest names object {key}, which the blob store \
                 does not hold"
            ))
        })?;
        // Never trust a hydrated segment before it re-proves its own seal: a
        // restore that skipped this could silently serve corrupted or
        // substituted history as if it were the original run.
        manifest.verify(&bytes)?;

        let index = manifest
            .replay_index
            .binary_search_by_key(&offset, |(o, _)| *o)
            .map_err(|_| {
                Error::not_found(format!(
                    "segment {segment_start}'s manifest has no replay-index entry for offset \
                     {offset}"
                ))
            })?;
        let start = manifest.replay_index[index].1 as usize;
        let end = manifest
            .replay_index
            .get(index + 1)
            .map(|(_, byte_start)| *byte_start as usize)
            .unwrap_or(bytes.len());
        if end > bytes.len() || start > end {
            return Err(Error::io(format!(
                "segment {segment_start}'s replay index names byte range [{start}, {end}) but \
                 the archived object is only {} bytes",
                bytes.len()
            )));
        }
        match Batch::decode(&bytes[start..end])? {
            DecodeOutcome::Complete(batch) => Ok(batch),
            DecodeOutcome::Torn => Err(Error::io(format!(
                "segment {segment_start}: the replay index named a batch at offset {offset} \
                 that does not decode complete"
            ))),
        }
    }
}
