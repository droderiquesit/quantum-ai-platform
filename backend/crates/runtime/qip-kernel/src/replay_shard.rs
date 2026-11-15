//! Disjoint replay shards and the merge of their live-versus-counterfactual
//! totals (R12-TICK-8110; ADR 0611).
//!
//! A replay over archived segments is a fold: each segment yields the
//! comparison of what the live path did against what a counterfactual would
//! have, and the total is the sum. That makes it shardable, but only if two
//! things hold: the shards are **disjoint and cover every segment** (a segment
//! in two shards is counted twice, one in none is silently lost), and the
//! merge of the shards' totals equals the single-process total. This module
//! holds both, as pure functions, so a batch job that launches shards can be
//! proven to reproduce the in-process replay.
//!
//! It does not read the archive and it does not score: which crate reads
//! `qip-storage`'s `ArchiveReader` into the kernel's `score_declined` and
//! `score_filled` inputs is decided in ADR 0611, and that seam is not built.

use qip_core::error::{Error, Result};

/// The live-versus-counterfactual comparison over some segments. Sums only, so
/// a merge is order-independent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ComparisonTotals {
    pub segments: u64,
    /// Live path's result, summed, in basis points of the replayed notional.
    /// A statistic, so `f64`.
    pub live_bps: f64,
    pub counterfactual_bps: f64,
}

impl ComparisonTotals {
    /// One segment's contribution.
    pub const fn one(live_bps: f64, counterfactual_bps: f64) -> Self {
        Self {
            segments: 1,
            live_bps,
            counterfactual_bps,
        }
    }

    /// The counterfactual's edge over live; positive when it would have done
    /// better. `None` over no segments, because nothing was compared.
    pub fn edge_bps(&self) -> Option<f64> {
        (self.segments > 0).then(|| self.counterfactual_bps - self.live_bps)
    }

    /// Merge two shards' totals.
    pub fn merge(self, other: Self) -> Self {
        Self {
            segments: self.segments + other.segments,
            live_bps: self.live_bps + other.live_bps,
            counterfactual_bps: self.counterfactual_bps + other.counterfactual_bps,
        }
    }
}

/// The segment indices shard `shard` of `shards` replays out of `segments`.
/// Round-robin by index, so every index belongs to exactly one shard whatever
/// the counts. Refuses zero shards and a shard index outside them, rather than
/// returning an empty shard that would read as "nothing to do".
pub fn shard_indices(segments: usize, shard: usize, shards: usize) -> Result<Vec<usize>> {
    if shards == 0 {
        return Err(Error::invalid(
            "a replay has at least one shard; zero shards would replay nothing and report success",
        ));
    }
    if shard >= shards {
        return Err(Error::invalid(format!(
            "shard {shard} is outside the {shards} shard(s); shards are numbered from zero"
        )));
    }
    Ok((0..segments).filter(|i| i % shards == shard).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn per_segment(i: usize) -> ComparisonTotals {
        ComparisonTotals::one(i as f64 * 1.5, i as f64 * 2.0 + 1.0)
    }

    fn replay(indices: &[usize]) -> ComparisonTotals {
        indices
            .iter()
            .fold(ComparisonTotals::default(), |t, i| t.merge(per_segment(*i)))
    }

    /// Mutation: not run (nothing was compiled). Intended: drop one shard's
    /// segments (`shard_indices` returning an empty list for shard 1) and the
    /// equality fails.
    #[test]
    fn two_shards_over_disjoint_segments_reproduce_the_single_process_comparison() {
        let n = 11;
        let all: Vec<usize> = (0..n).collect();
        let single = replay(&all);
        assert_eq!(single.segments, 11, "premise: the single run saw every segment");
        let a = shard_indices(n, 0, 2).unwrap();
        let b = shard_indices(n, 1, 2).unwrap();
        assert!(a.iter().all(|i| !b.contains(i)), "a segment is in both shards");
        assert_eq!(a.len() + b.len(), n, "a segment is in no shard");
        let merged = replay(&a).merge(replay(&b));
        assert_eq!(merged, single);
        assert_eq!(merged.edge_bps(), single.edge_bps());
        assert_ne!(replay(&a), single, "premise: one shard alone differs");
    }

    #[test]
    fn a_bad_shard_request_is_refused_and_no_segments_compare_nothing() {
        assert!(shard_indices(5, 0, 0).is_err());
        assert!(shard_indices(5, 3, 3).is_err());
        assert_eq!(ComparisonTotals::default().edge_bps(), None);
    }
}
