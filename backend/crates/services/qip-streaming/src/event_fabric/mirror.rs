//! The cross-region mirror's copy-and-checkpoint logic, as a function over
//! committed source batches and a checkpoint (ADR 0119 §1, "Dependency
//! direction").
//!
//! No socket and no clock is here. The mTLS client to the source fabric, the
//! producer into the target and the durable checkpoint are composed in the
//! binary that runs the mirror; this module is the part whose mistakes lose or
//! invent records, so it is the part that is tested without a network.
//!
//! # What [`plan_copy`] promises, and why each promise is the mirror's
//!
//! * **Only committed records are copied.** A row at or above the source's
//!   committed bound is refused, not skipped: it is a fetch that read past
//!   what the source acknowledged, and a copy of an unacknowledged record is
//!   a fact the source region does not hold (ADR 0119 "what would make this
//!   wrong": a mirror writing what the source did not commit).
//! * **Idempotence is by source position.** A row below the checkpoint is a
//!   duplicate and is dropped and counted, so a restart that re-fetches from
//!   an earlier offset copies nothing twice.
//! * **A missing source offset is a gap, never a skip.** The plan carries a
//!   [`MirrorGap`] naming the offsets, for the caller to journal as an
//!   `EventFabricGap` in the target stream. The row after a gap cannot be
//!   chained to its predecessor, which the mirror does not hold, so that one
//!   link is taken on the row's own hash; every later link is verified.
//! * **The batch chain is verified before anything is produced** (ADR 0104):
//!   the bytes must hash to the hash the row carries, decode complete at the
//!   offset the row claims, and name the previous batch's hash the mirror
//!   last copied. A plan with a broken chain is an error and copies nothing,
//!   so the target never holds a prefix of a poisoned run.
//! * **Rows arrive in strictly ascending offset order.** Anything else is a
//!   fetch the mirror cannot reason about, and is refused.

use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, ContentHash, DecodeOutcome};

use super::managed::CommittedBatch;

/// The source partition a mirror reads: where its records are a fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourcePartition {
    pub region: String,
    pub stream: String,
    pub partition: u32,
}

impl SourcePartition {
    /// The stream the copies are produced into in the target region:
    /// `<source-region>.<stream>`. Nothing but the mirror may write to it
    /// (ADR 0119 §1), which is what keeps a copy from becoming a new fact.
    pub fn target_stream(&self) -> Result<String> {
        region_qualified(&self.region, &self.stream)
    }
}

/// `<region>.<stream>`, refusing a region that is not a lowercase DNS-style
/// label and a stream that is empty. A region containing a dot would make
/// `a.b` + `c` and `a` + `b.c` the same target stream.
pub fn region_qualified(region: &str, stream: &str) -> Result<String> {
    let label = !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !label {
        return Err(Error::invalid(format!(
            "mirror source region `{region}` is not a lowercase label of letters, digits and \
             hyphens; a dot in it would make two different sources one target stream"
        )));
    }
    if stream.trim().is_empty() {
        return Err(Error::invalid(
            "a mirror needs the source stream's name to qualify it",
        ));
    }
    Ok(format!("{region}.{stream}"))
}

/// How far the mirror has had copies acknowledged by the target: the next
/// source offset it needs and the hash of the last batch it copied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorCheckpoint {
    pub next_offset: u64,
    pub last_batch_hash: Option<String>,
}

impl MirrorCheckpoint {
    fn validate(&self) -> Result<()> {
        if self.next_offset == 0 && self.last_batch_hash.is_some() {
            return Err(Error::invalid(
                "a mirror checkpoint at offset zero cannot name a last batch hash",
            ));
        }
        Ok(())
    }
}

/// One source batch to produce into the target stream, with the position it
/// came from so the copy can be told apart from a new fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorCopy {
    pub region: String,
    pub stream: String,
    pub partition: u32,
    pub offset: u64,
    pub batch_hash: String,
    pub bytes: Vec<u8>,
}

/// Source offsets `[from, to)` the mirror was offered a row after but never a
/// row for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorGap {
    pub from_offset: u64,
    pub to_offset: u64,
}

/// What one fetch asks the mirror to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorPlan {
    pub copies: Vec<MirrorCopy>,
    pub gaps: Vec<MirrorGap>,
    /// Rows at or below the checkpoint, dropped.
    pub duplicates: u64,
    /// The checkpoint once the target has acknowledged every copy.
    pub checkpoint: MirrorCheckpoint,
}

fn hex_of(hash: &ContentHash) -> Option<String> {
    match hash {
        ContentHash::Sha256(digest) => Some(qip_core::hash::to_hex(digest)),
        ContentHash::Blake3(_) => None,
    }
}

/// Plan the copies for `rows`, fetched from `source` up to its committed
/// bound `committed_through` (one past the last committed offset).
pub fn plan_copy(
    source: &SourcePartition,
    checkpoint: &MirrorCheckpoint,
    committed_through: u64,
    rows: Vec<CommittedBatch>,
) -> Result<MirrorPlan> {
    checkpoint.validate()?;
    let label = format!("{}/{}:{}", source.region, source.stream, source.partition);
    let mut plan = MirrorPlan {
        copies: Vec::new(),
        gaps: Vec::new(),
        duplicates: 0,
        checkpoint: checkpoint.clone(),
    };
    let mut previous_offset: Option<u64> = None;
    // The link a row must name: the hash of the batch the mirror copied last,
    // or `None` when the previous offset was never held (a gap) or this is
    // offset zero, in which case the chain starts afresh at this row.
    let mut chain_known = true;
    for row in rows {
        if let Some(previous) = previous_offset
            && row.offset <= previous
        {
            return Err(Error::invalid(format!(
                "{label}: rows arrived at offset {} after {previous}; the mirror reads strictly \
                 ascending offsets",
                row.offset
            )));
        }
        previous_offset = Some(row.offset);
        if row.offset >= committed_through {
            return Err(Error::invalid(format!(
                "{label}: offset {} is not committed (committed through {committed_through}); \
                 refusing to copy a record the source region has not acknowledged",
                row.offset
            )));
        }
        if row.offset < plan.checkpoint.next_offset {
            plan.duplicates += 1;
            continue;
        }
        row.verify()?;
        let batch = match Batch::decode(&row.bytes)? {
            DecodeOutcome::Complete(batch) => batch,
            DecodeOutcome::Torn => {
                return Err(Error::io(format!(
                    "{label}: offset {} does not decode as a complete batch",
                    row.offset
                )));
            }
        };
        if batch.base_offset != row.offset {
            return Err(Error::invalid(format!(
                "{label}: the row at offset {} holds a batch stamped offset {}",
                row.offset, batch.base_offset
            )));
        }
        let embedded = batch.previous_batch_hash.as_ref().and_then(hex_of);
        if embedded != row.previous_batch_hash {
            return Err(Error::invalid(format!(
                "{label}: offset {} carries a chain link that its own bytes do not name",
                row.offset
            )));
        }
        if row.offset > plan.checkpoint.next_offset {
            plan.gaps.push(MirrorGap {
                from_offset: plan.checkpoint.next_offset,
                to_offset: row.offset,
            });
            chain_known = false;
        }
        if chain_known && embedded != plan.checkpoint.last_batch_hash {
            return Err(Error::invalid(format!(
                "{label}: offset {} does not chain to the batch the mirror copied before it \
                 (ADR 0104); refusing to produce a broken chain",
                row.offset
            )));
        }
        plan.checkpoint = MirrorCheckpoint {
            next_offset: row.offset + 1,
            last_batch_hash: Some(row.batch_hash.clone()),
        };
        chain_known = true;
        plan.copies.push(MirrorCopy {
            region: source.region.clone(),
            stream: source.stream.clone(),
            partition: source.partition,
            offset: row.offset,
            batch_hash: row.batch_hash,
            bytes: row.bytes,
        });
    }
    Ok(plan)
}
