//! `MirrorCheckpoint` (blueprint v12.0 §27, R12-CONTRACT-7092): how far one
//! stream partition has been mirrored from one region to another.
//!
//! The fabric has no mirror yet (`StreamPolicy::new` refuses mirroring until
//! one exists). The contract comes first so the mirror, when it lands, has a
//! record that cannot claim more than it did: a checkpoint names the source
//! epoch and offset it read up to, the destination offset it wrote, the lag
//! that leaves, and an integrity hash over the mirrored range. A checkpoint
//! from a region to itself, or one whose lag does not match its own offsets,
//! is refused.

use crate::validated::{Contract, Validated, digest, named, non_negative_duration, refuse};
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorCheckpointFields {
    pub schema_version: u32,
    pub source_region: String,
    pub destination_region: String,
    pub stream: String,
    pub partition: u32,
    /// The source leader epoch the offsets below belong to.
    pub source_epoch: u64,
    /// The next source offset not yet mirrored.
    pub source_offset: u64,
    /// The source's high-water mark when the checkpoint was taken.
    pub source_high_water: u64,
    /// The next destination offset to be written.
    pub destination_offset: u64,
    /// Messages behind: exactly `source_high_water - source_offset`.
    pub lag_messages: u64,
    /// Time behind: age of the oldest unmirrored message.
    pub lag_time: Duration,
    /// SHA-256 over the mirrored range, for a reader to verify.
    pub integrity_hash: String,
    pub last_success_at: Timestamp,
}

/// The §27 mirror checkpoint. Built only through [`Validated::new`].
pub type MirrorCheckpoint = Validated<MirrorCheckpointFields>;

impl Contract for MirrorCheckpointFields {
    const NAME: &'static str = "MirrorCheckpoint";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank regions or stream, a mirror from a region to itself, a
    /// source offset beyond the high-water mark, a lag that disagrees with
    /// the offsets (a stated lag nobody can recompute is a dashboard number),
    /// a negative lag time, and a malformed integrity hash.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "source_region", &self.source_region)?;
        named(c, "destination_region", &self.destination_region)?;
        named(c, "stream", &self.stream)?;
        if self.source_region == self.destination_region {
            return refuse(
                c,
                "mirrors a region to itself; that is replication, not a mirror",
            );
        }
        let Some(lag) = self.source_high_water.checked_sub(self.source_offset) else {
            return refuse(
                c,
                "has mirrored past the source high-water mark; a mirror cannot read what was \
                 never written",
            );
        };
        if lag != self.lag_messages {
            return refuse(
                c,
                format!(
                    "states a lag of {} messages where its offsets give {lag}",
                    self.lag_messages
                ),
            );
        }
        if lag == 0 && !self.lag_time.is_zero() {
            return refuse(c, "states a lag time with no messages behind");
        }
        non_negative_duration(c, "lag_time", self.lag_time)?;
        digest(c, "integrity_hash", &self.integrity_hash)
    }
}
