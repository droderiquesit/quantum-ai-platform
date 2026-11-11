//! Per-domain world-model snapshots on `world_model.snapshot` (ADR 0191
//! decision 3, R12-WORLD-7023).
//!
//! The world model lives in the central brains and is rebuilt from the log
//! after a restart. Nothing could be cited about it: "the causal graph the
//! cycle decided on" named no version a reader could check. A
//! [`WorldModelSnapshot`] gives each domain a monotone version, its counts
//! and a digest of its content as known at the cycle's instant. A reader can
//! then cite "the causal graph at version N", and a replay can be checked
//! against the digest.
//!
//! # A citation, never state
//!
//! A snapshot is written after the cycle and never read back as the domain's
//! content. The only thing read back is each domain's last version and
//! digest, so that a restarted process continues the version sequence
//! rather than starting it again. The topic's retention class is Episodic,
//! which the log keeps indefinitely, so that sequence survives.
//!
//! What a version does not promise: that the content behind it survives a
//! restart. A restarted platform starts from an empty world model and does
//! not rebuild the graphs from the log, so the first snapshot after a
//! restart describes what the new process has learned since, at the next
//! version. The digest makes that visible; it does not prevent it.
//!
//! # Bounded by change
//!
//! A snapshot is journaled only when a domain's digest differs from the
//! last one journaled for it. An idle world adds nothing to the log, and the
//! number of snapshots is the number of changes, not of cycles.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_streaming::envelope::StreamEnvelope;
use qip_world_model::world::WorldModel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The lineage origin snapshots are journaled under, so a reader of the
/// topic can tell them from anything else written to it.
pub const WORLD_SNAPSHOT_ORIGIN: &str = "kernel/world-snapshot";

/// The knowledge graph: the relationship facts true and known at the
/// snapshot's instant.
pub const KNOWLEDGE_GRAPH: &str = "knowledge-graph";

/// The causal graph: the edges recorded by, and not retired at, the
/// snapshot's instant.
pub const CAUSAL_GRAPH: &str = "causal-graph";

/// One domain of the world model at one version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldModelSnapshot {
    pub domain: String,
    /// Strictly increasing per domain, across restarts.
    pub version: u64,
    /// The instant the domain was read at, both as true and as known.
    pub at: Timestamp,
    /// What the domain holds, by name, e.g. `facts`, `edges`.
    pub counts: BTreeMap<String, u64>,
    /// sha256 of the canonical JSON of the domain's content at `at`.
    pub digest: String,
}

impl EventBody for WorldModelSnapshot {
    const TOPIC: Topic = Topic::WorldModelSnapshot;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("world-snapshot:{}:{}", self.domain, self.version))
    }
}

/// One domain's name, counts and content digest.
pub type DomainReading = (String, BTreeMap<String, u64>, String);

/// Each domain's content at `at`, as counts and a digest, in domain order.
/// The content is read in the order the graphs iterate, which is a
/// `BTreeMap`'s order, so the same world gives the same digest.
pub fn domains_of(world: &WorldModel, at: Timestamp) -> Result<Vec<DomainReading>> {
    let facts = world.graph().facts_at(at, at);
    let edges: Vec<_> = world
        .causal()
        .edges()
        .iter()
        .filter(|edge| edge.recorded_at <= at && !edge.retired_by(at))
        .collect();
    Ok(vec![
        (
            CAUSAL_GRAPH.to_string(),
            BTreeMap::from([("edges".to_string(), edges.len() as u64)]),
            sha256_hex(&serde_json::to_vec(&edges)?),
        ),
        (
            KNOWLEDGE_GRAPH.to_string(),
            BTreeMap::from([("facts".to_string(), facts.len() as u64)]),
            sha256_hex(&serde_json::to_vec(&facts)?),
        ),
    ])
}

/// Each domain's last journaled version and digest, read back from the log
/// so a restarted process continues the sequence.
pub fn resume(log: &EventLog) -> Result<BTreeMap<String, (u64, String)>> {
    let mut last = BTreeMap::new();
    for event in log.events() {
        if event.topic != WorldModelSnapshot::TOPIC
            || event.lineage.producer != WORLD_SNAPSHOT_ORIGIN
        {
            continue;
        }
        let snapshot = StreamEnvelope::from_frame(event)
            .and_then(|envelope| envelope.decode::<WorldModelSnapshot>())?
            .body;
        let newer = last
            .get(&snapshot.domain)
            .is_none_or(|(version, _)| snapshot.version > *version);
        if newer {
            last.insert(snapshot.domain.clone(), (snapshot.version, snapshot.digest));
        }
    }
    Ok(last)
}

/// Record what journaling `version` of `domain` did. A snapshot that reached
/// the log takes its digest as the domain's last. One that failed still
/// takes its version, keeping the previous digest, because the record may
/// already be in the log (the log is appended before the journal is
/// published) and journaling the same version twice would let two records
/// claim it. The cost of a failed publish is therefore a skipped version,
/// never a repeated one, and the next cycle retries at the next number.
pub fn settle(
    last: &mut BTreeMap<String, (u64, String)>,
    domain: String,
    version: u64,
    digest: String,
    journaled: bool,
) {
    let kept = if journaled {
        digest
    } else {
        last.get(&domain)
            .map(|(_, previous)| previous.clone())
            .unwrap_or_default()
    };
    last.insert(domain, (version, kept));
}

/// The snapshots one cycle owes: each domain whose digest differs from the
/// last journaled for it, at the next version.
pub fn due(
    domains: Vec<DomainReading>,
    last: &BTreeMap<String, (u64, String)>,
    at: Timestamp,
) -> Vec<WorldModelSnapshot> {
    domains
        .into_iter()
        .filter_map(|(domain, counts, digest)| {
            let previous = last.get(&domain);
            if previous.is_some_and(|(_, d)| *d == digest) {
                return None;
            }
            Some(WorldModelSnapshot {
                version: previous.map_or(1, |(version, _)| version + 1),
                domain,
                at,
                counts,
                digest,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed journal may still have left its record in the log, so the
    /// version it used is taken and never reused; its digest is not, so the
    /// next cycle snapshots the domain again, at the next version.
    ///
    /// Mutation: leave the version unadvanced on a failed journal.
    #[test]
    fn a_failed_journal_takes_its_version_so_no_two_snapshots_claim_one() {
        let mut last = BTreeMap::from([(CAUSAL_GRAPH.to_string(), (3, "old".to_string()))]);
        settle(
            &mut last,
            CAUSAL_GRAPH.to_string(),
            4,
            "new".to_string(),
            false,
        );
        assert_eq!(last[CAUSAL_GRAPH], (4, "old".to_string()));
        let at = Timestamp::from_secs(1_760_000_000);
        let again = due(
            vec![(CAUSAL_GRAPH.to_string(), BTreeMap::new(), "new".to_string())],
            &last,
            at,
        );
        assert_eq!(again.len(), 1, "the unjournaled change was not retried");
        assert_eq!(again[0].version, 5, "a failed journal's version was reused");
        settle(
            &mut last,
            CAUSAL_GRAPH.to_string(),
            5,
            "new".to_string(),
            true,
        );
        assert_eq!(last[CAUSAL_GRAPH], (5, "new".to_string()));
    }
}
