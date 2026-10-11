//! The tick-lake segment codec and the measurement that decides whether to
//! use it (R12-TICK-2057, ADR 0113 §3).
//!
//! ADR 0113 admits `zstd` *on measurement*: a committed figure of bytes per
//! partition, uncompressed against compressed, decides whether the archive's
//! cost warrants the codec. So this module ships both halves and keeps them
//! apart:
//!
//! * [`measure_partitions`] reads every sealed market segment under a prefix,
//!   compresses it in memory, and reports the raw size, the compressed size
//!   and the ratio **per partition** — the per-venue, per-day, per-instrument,
//!   per-entitlement figure the ADR asks for. It writes nothing, so running it
//!   against a production lake is safe.
//! * [`SegmentCodec`] is the codec a [`crate::lake::Lake`] writes with. The
//!   default is [`SegmentCodec::Identity`], and every writer in the tree uses
//!   it until a composition root is told otherwise: the flag is off by
//!   default because the decision the ADR defers has not been taken.
//!
//! # What a codec may not change
//!
//! A segment's `canonical_sha256` is the hash of its **uncompressed**
//! canonical bytes whichever codec stored it. Lineage names content, not an
//! encoding: a manifest pinned before compression was switched on must still
//! verify after, and two segments holding the same ticks must not read as
//! different history because one was written on a different day's setting.
//! The stored object's key carries the codec (`.canonical` or
//! `.canonical.zst`) and so does the [`crate::lake::SegmentRecord`], so
//! compressed and uncompressed segments coexist and a reader decodes both.
//!
//! Frames carry zstd's own content checksum as well. That is not redundant
//! with the SHA-256: the frame checksum tells a decoder the bytes it inflated
//! are the bytes that were deflated, and is checked by BigQuery or Dataflow
//! reading the object directly, which never see the SHA-256.

use crate::blob::BlobStore;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The largest canonical segment a decoder will inflate. A frame that claims
/// more is refused rather than allocated: a corrupted or hostile header must
/// not be able to ask this process for an unbounded buffer.
pub const MAX_DECODED_SEGMENT_BYTES: usize = 512 * 1024 * 1024;

/// The zstd levels a writer may be configured with. Above 19 is zstd's
/// "ultra" range, whose memory cost belongs in a measured decision of its
/// own; below 1 is a negative "fast" level the archive has no use for.
pub const ZSTD_LEVELS: std::ops::RangeInclusive<i32> = 1..=19;

/// How a lake segment's canonical bytes are stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "codec")]
pub enum SegmentCodec {
    /// Stored as written. The default, and what every segment sealed before
    /// this codec existed is.
    #[default]
    Identity,
    /// A zstd frame with the content checksum on, at `level`.
    Zstd { level: i32 },
}

impl SegmentCodec {
    /// Whether this is the identity codec — for `skip_serializing_if`, so a
    /// record written uncompressed serialises exactly as it did before the
    /// field existed and every pinned manifest still parses.
    pub fn is_identity(&self) -> bool {
        matches!(self, Self::Identity)
    }

    /// The object-key suffix the codec stores a segment under.
    pub const fn suffix(&self) -> &'static str {
        match self {
            Self::Identity => ".canonical",
            Self::Zstd { .. } => ".canonical.zst",
        }
    }

    /// Parse an operator's setting: `identity` (or `none`, or empty), `zstd`
    /// (level 3, zstd's own default), or `zstd:<level>`. Anything else is
    /// refused by name rather than read as identity — a typo in a codec flag
    /// that silently stored uncompressed would be a measurement nobody took.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        match text {
            "" | "identity" | "none" => return Ok(Self::Identity),
            "zstd" => return Self::zstd(3),
            _ => {}
        }
        if let Some(level) = text.strip_prefix("zstd:") {
            let level: i32 = level.parse().map_err(|_| {
                Error::invalid(format!(
                    "tick-lake codec level {level:?} is not an integer; write zstd:<1..=19>"
                ))
            })?;
            return Self::zstd(level);
        }
        Err(Error::invalid(format!(
            "tick-lake codec {text:?} is not one this lake writes; use identity, zstd or \
             zstd:<1..=19>"
        )))
    }

    /// A zstd codec, refusing a level outside [`ZSTD_LEVELS`].
    pub fn zstd(level: i32) -> Result<Self> {
        if !ZSTD_LEVELS.contains(&level) {
            return Err(Error::invalid(format!(
                "zstd level {level} is outside {}..={}; the ultra and negative ranges are a \
                 decision of their own, not a setting",
                ZSTD_LEVELS.start(),
                ZSTD_LEVELS.end()
            )));
        }
        Ok(Self::Zstd { level })
    }

    /// Encode canonical bytes for storage.
    pub fn encode(&self, canonical: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::Identity => Ok(canonical.to_vec()),
            Self::Zstd { level } => {
                let mut compressor = zstd::bulk::Compressor::new(*level)
                    .map_err(|error| Error::io(format!("zstd compressor: {error}")))?;
                compressor
                    .set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))
                    .map_err(|error| Error::io(format!("zstd checksum flag: {error}")))?;
                compressor
                    .compress(canonical)
                    .map_err(|error| Error::io(format!("zstd compression failed: {error}")))
            }
        }
    }

    /// Decode stored bytes back to canonical bytes, bounded by
    /// [`MAX_DECODED_SEGMENT_BYTES`]. A frame that fails its own checksum is
    /// refused here, before the SHA-256 is even computed.
    pub fn decode(&self, stored: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::Identity => Ok(stored.to_vec()),
            Self::Zstd { .. } => {
                zstd::bulk::decompress(stored, MAX_DECODED_SEGMENT_BYTES).map_err(|error| {
                    Error::invalid(format!(
                        "a zstd lake segment does not decode ({error}); it is corrupt, \
                         truncated, or larger than {MAX_DECODED_SEGMENT_BYTES} bytes"
                    ))
                })
            }
        }
    }

    /// The codec a stored key was written with, from its suffix.
    pub fn of_key(key: &str) -> Option<Self> {
        if key.ends_with(".canonical.zst") {
            // The level is not recoverable from the key and not needed to
            // decode; 3 is recorded so a re-encode is deterministic.
            Some(Self::Zstd { level: 3 })
        } else if key.ends_with(".canonical") {
            Some(Self::Identity)
        } else {
            None
        }
    }
}

/// The measured compression of one partition.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionMeasurement {
    /// Segments measured.
    pub segments: u64,
    /// Canonical bytes, uncompressed.
    pub raw_bytes: u64,
    /// The same bytes as the candidate codec would store them.
    pub compressed_bytes: u64,
}

impl PartitionMeasurement {
    /// `compressed / raw`. `None` for a partition with no bytes, rather than
    /// a 0 or a 1 that would read as a measurement.
    pub fn ratio(&self) -> Option<f64> {
        // Statistics: a byte count crosses to f64 here, for a ratio only.
        (self.raw_bytes > 0).then(|| self.compressed_bytes as f64 / self.raw_bytes as f64)
    }
}

/// What [`measure_partitions`] found, keyed by partition prefix so the report
/// reads in path order and two runs over one lake print identically.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompressionReport {
    /// The codec measured.
    pub codec: SegmentCodec,
    pub partitions: BTreeMap<String, PartitionMeasurement>,
}

impl CompressionReport {
    /// The whole lake under the measured prefix, summed.
    pub fn total(&self) -> PartitionMeasurement {
        let mut total = PartitionMeasurement::default();
        for m in self.partitions.values() {
            total.segments += m.segments;
            total.raw_bytes += m.raw_bytes;
            total.compressed_bytes += m.compressed_bytes;
        }
        total
    }

    /// Raw and compressed bytes per partition date, summed across venues,
    /// instruments and entitlements — the "bytes per day" figure ADR 0113's
    /// decision is stated in.
    pub fn by_date(&self) -> BTreeMap<String, PartitionMeasurement> {
        let mut out: BTreeMap<String, PartitionMeasurement> = BTreeMap::new();
        for (prefix, m) in &self.partitions {
            let date = prefix
                .split('/')
                .find_map(|part| part.strip_prefix("date="))
                .unwrap_or("unknown")
                .to_string();
            let entry = out.entry(date).or_default();
            entry.segments += m.segments;
            entry.raw_bytes += m.raw_bytes;
            entry.compressed_bytes += m.compressed_bytes;
        }
        out
    }

    /// One line an operator can paste into the measurement record.
    pub fn describe(&self) -> String {
        let total = self.total();
        match total.ratio() {
            Some(ratio) => format!(
                "{} partition(s), {} segment(s): {} raw byte(s) -> {} under {:?} (ratio {ratio:.3})",
                self.partitions.len(),
                total.segments,
                total.raw_bytes,
                total.compressed_bytes,
                self.codec
            ),
            None => "no sealed segment under the prefix; nothing was measured".to_string(),
        }
    }
}

/// Measure what `candidate` would store for every sealed segment under
/// `prefix` (typically `lake/class=market/`), per partition. Writes nothing.
///
/// A segment already stored compressed is decoded first, so the raw figure is
/// always the canonical size and the measurement is of the candidate, not of
/// whatever happens to be on disk. Refuses an identity candidate: measuring
/// the identity codec against itself is a ratio of one that nobody needed.
pub fn measure_partitions(
    store: &dyn BlobStore,
    prefix: &str,
    candidate: SegmentCodec,
) -> Result<CompressionReport> {
    if candidate.is_identity() {
        return Err(Error::invalid(
            "measuring the identity codec reports a ratio of one by construction; name the \
             candidate codec to measure",
        ));
    }
    let mut report = CompressionReport {
        codec: candidate,
        partitions: BTreeMap::new(),
    };
    for key in store.list(prefix)? {
        let Some(stored_codec) = SegmentCodec::of_key(&key) else {
            continue;
        };
        let Some(stored) = store.get(&key)? else {
            continue;
        };
        let canonical = stored_codec.decode(&stored)?;
        let compressed = candidate.encode(&canonical)?;
        let partition = key
            .rsplit_once('/')
            .map_or_else(|| key.clone(), |(dir, _)| dir.to_string());
        let entry = report.partitions.entry(partition).or_default();
        entry.segments += 1;
        entry.raw_bytes += canonical.len() as u64;
        entry.compressed_bytes += compressed.len() as u64;
    }
    Ok(report)
}
