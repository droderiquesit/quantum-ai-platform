//! Adversarial and poisoned source detection over the evidence fabric
//! (R12-EVID-2039).
//!
//! The fabric judges one claim at a time; this judges patterns no single
//! claim reveals. Until 2026-10-11 the platform's only "detector" was an
//! agent whose `analyse` returned no data and whose tests asserted on
//! findings they had built by hand. Every class here is computed from what
//! the fabric actually holds:
//!
//! * [`PoisoningClass::CoordinatedManipulation`] — the same normalised
//!   statement from several distinct sources, from *different* documents,
//!   published within a short spread. Syndication shares a document and is
//!   already collapsed to one origin by the fabric; several independent
//!   fetches carrying identical text at the same minute is a campaign.
//! * [`PoisoningClass::SourcePoisoning`] — a source whose claims lose the
//!   contests they enter: contradicted at least `min_resolved` times, and
//!   on a share of its resolved claims at or above `poisoned_share`.
//! * [`PoisoningClass::Spoofing`] — a source whose own authenticity signals
//!   contradicted its identity (a TLS domain that is not its host, a hash
//!   that is not the document's).
//! * [`PoisoningClass::SyntheticContent`] — a source repeatedly producing
//!   claims on physically impossible timelines: ingested before published,
//!   or describing an event after its own publication. Once is a clock;
//!   repeatedly is fabricated data.
//! * [`PoisoningClass::Spam`] — a source whose claim count in the window is
//!   at least `burst_limit` and at least `burst_ratio` times the median of
//!   the sources active in the window: anomalous volume against its peers.
//!
//! Findings are ordered by class then by source so a replay reads the same.

use crate::fabric::{
    EvidenceFabric, shingle_similarity, statement_fingerprint, statement_shingles,
};
use qip_core::{Duration, Timestamp};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Categories of source poisoning that can be detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoisoningClass {
    /// Coordinated manipulation campaign across multiple outlets.
    CoordinatedManipulation,
    /// Single source publishing claims later shown false.
    SourcePoisoning,
    /// A source whose authenticity signals contradict its identity.
    Spoofing,
    /// A source repeatedly producing physically impossible timelines.
    SyntheticContent,
    /// A source flooding the fabric against its peers.
    Spam,
}

impl PoisoningClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoordinatedManipulation => "coordinated_manipulation",
            Self::SourcePoisoning => "source_poisoning",
            Self::Spoofing => "spoofing",
            Self::SyntheticContent => "synthetic_content",
            Self::Spam => "spam",
        }
    }
}

/// One detection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PoisoningFinding {
    pub class: PoisoningClass,
    /// The sources implicated; one for a per-source class, several for a
    /// campaign.
    pub sources: BTreeSet<String>,
    /// The claims the finding rests on, where it rests on claims.
    pub claims: Vec<String>,
    /// What was measured, in a sentence.
    pub evidence: String,
}

/// The figures a detection is judged against. Named rather than buried so a
/// reviewer can see what "too many" means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoisoningThresholds {
    /// How far back the windowed classes (coordination, spam) look.
    pub window: Duration,
    /// Distinct sources carrying one statement before it is a campaign.
    pub min_coordinated_sources: usize,
    /// The publication spread inside which identical text is coordinated.
    pub coordination_spread: Duration,
    /// Shingle similarity ([`shingle_similarity`]) at or above which two
    /// statements are near-identical for the coordination arm (EVID-2039).
    /// Identical text scores one; a copy with a clause added or a word
    /// swapped every few words still scores above this; two stories on one
    /// event in their own words do not.
    pub near_duplicate_similarity: f64,
    /// The most recent records the near-duplicate pass compares pairwise.
    /// The pass is quadratic in this figure, so it is bounded here rather
    /// than by the fabric's capacity.
    pub near_duplicate_scan: usize,
    /// Contradicted resolutions before a source can be called poisoned.
    pub min_resolved: u64,
    /// Share of resolved claims lost at or above which it is.
    pub poisoned_share: f64,
    /// Impossible timelines before a source is called synthetic.
    pub min_impossible: u64,
    /// Claims in the window before a source can be called spam.
    pub burst_limit: usize,
    /// Multiple of the peer median at or above which it is.
    pub burst_ratio: f64,
}

impl Default for PoisoningThresholds {
    fn default() -> Self {
        Self {
            window: Duration::from_hours(24),
            min_coordinated_sources: 3,
            coordination_spread: Duration::from_mins(10),
            near_duplicate_similarity: 0.6,
            near_duplicate_scan: 512,
            min_resolved: 3,
            poisoned_share: 0.6,
            min_impossible: 2,
            burst_limit: 50,
            burst_ratio: 10.0,
        }
    }
}

/// Scan `fabric` as of `now`.
pub fn detect_poisoning(
    fabric: &EvidenceFabric,
    now: Timestamp,
    thresholds: &PoisoningThresholds,
) -> Vec<PoisoningFinding> {
    let mut findings = Vec::new();
    let since = now.saturating_sub(thresholds.window);
    let recent: Vec<_> = fabric
        .held()
        .filter(|held| held.record.ingestion_timestamp >= since)
        .collect();

    // Coordination: near-identical text, distinct sources, distinct
    // documents. Identical text (one fingerprint) is the similarity-one
    // case of the same clustering; the shingle pass is what catches a copy
    // with a clause added or a word swapped, which an exact fingerprint read
    // as three unrelated stories (EVID-2039). Clusters are the connected
    // components of "similar enough" over the newest records, pairwise, so
    // the scan is bounded by `near_duplicate_scan` rather than the window.
    let mut by_text: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for held in &recent {
        by_text
            .entry(statement_fingerprint(&held.record.statement))
            .or_default()
            .push(*held);
    }
    let mut newest: Vec<_> = recent.clone();
    newest.sort_by(|a, b| {
        b.record
            .ingestion_timestamp
            .cmp(&a.record.ingestion_timestamp)
            .then_with(|| a.record.id.cmp(&b.record.id))
    });
    newest.truncate(thresholds.near_duplicate_scan);
    let shingles: Vec<BTreeSet<String>> = newest
        .iter()
        .map(|held| statement_shingles(&held.record.statement))
        .collect();
    let mut parent: Vec<usize> = (0..newest.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..newest.len() {
        for j in (i + 1)..newest.len() {
            if shingle_similarity(&shingles[i], &shingles[j])
                >= thresholds.near_duplicate_similarity
            {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let mut components: BTreeMap<usize, Vec<_>> = BTreeMap::new();
    for (i, held) in newest.iter().enumerate() {
        let r = root(&mut parent, i);
        components.entry(r).or_default().push(*held);
    }
    // A component of one fingerprint is already the exact cluster above;
    // keep the exact clusters (they cover records past the scan bound) and
    // add every component that joins more than one fingerprint.
    let mut clusters: Vec<Vec<_>> = by_text.into_values().collect();
    for component in components.into_values() {
        let fingerprints: BTreeSet<String> = component
            .iter()
            .map(|held| statement_fingerprint(&held.record.statement))
            .collect();
        if fingerprints.len() > 1 {
            clusters.push(component);
        }
    }
    for cluster in &clusters {
        let sources: BTreeSet<String> = cluster
            .iter()
            .map(|held| held.record.source_identity.clone())
            .collect();
        let documents: BTreeSet<Option<&String>> = cluster
            .iter()
            .map(|held| held.document_hash.as_ref())
            .collect();
        if sources.len() < thresholds.min_coordinated_sources || documents.len() < 2 {
            continue;
        }
        let (Some(first), Some(last)) = (
            cluster.iter().map(|h| h.record.source_timestamp).min(),
            cluster.iter().map(|h| h.record.source_timestamp).max(),
        ) else {
            continue;
        };
        if last.since(first) > thresholds.coordination_spread {
            continue;
        }
        let distinct_texts = cluster
            .iter()
            .map(|held| statement_fingerprint(&held.record.statement))
            .collect::<BTreeSet<_>>()
            .len();
        findings.push(PoisoningFinding {
            class: PoisoningClass::CoordinatedManipulation,
            evidence: format!(
                "{} sources published {} from {} distinct documents within {:.0}s",
                sources.len(),
                if distinct_texts == 1 {
                    "one statement".to_string()
                } else {
                    format!("{distinct_texts} near-identical wordings of one statement")
                },
                documents.len(),
                last.since(first).as_secs_f64()
            ),
            sources,
            claims: cluster.iter().map(|held| held.record.id.clone()).collect(),
        });
    }

    for (source, history) in fabric.sources() {
        let resolved = history.corroborated + history.contradicted;
        if history.contradicted >= thresholds.min_resolved
            && resolved > 0
            && history.contradicted as f64 / resolved as f64 >= thresholds.poisoned_share
        {
            findings.push(PoisoningFinding {
                class: PoisoningClass::SourcePoisoning,
                sources: BTreeSet::from([source.clone()]),
                claims: Vec::new(),
                evidence: format!(
                    "contradicted in {} of {} resolved contests",
                    history.contradicted, resolved
                ),
            });
        }
        if history.spoofed > 0 {
            findings.push(PoisoningFinding {
                class: PoisoningClass::Spoofing,
                sources: BTreeSet::from([source.clone()]),
                claims: Vec::new(),
                evidence: format!(
                    "{} claim(s) carried authenticity signals contradicting the source's identity",
                    history.spoofed
                ),
            });
        }
        if history.impossible >= thresholds.min_impossible {
            findings.push(PoisoningFinding {
                class: PoisoningClass::SyntheticContent,
                sources: BTreeSet::from([source.clone()]),
                claims: Vec::new(),
                evidence: format!(
                    "{} claim(s) on physically impossible timelines",
                    history.impossible
                ),
            });
        }
    }

    // Spam: volume against the peer median.
    let mut counts: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for held in &recent {
        counts
            .entry(held.record.source_identity.as_str())
            .or_default()
            .push(held.record.id.clone());
    }
    let mut sizes: Vec<usize> = counts.values().map(Vec::len).collect();
    sizes.sort_unstable();
    if let Some(median) = median_of(&sizes) {
        for (source, claims) in counts {
            let count = claims.len();
            if count >= thresholds.burst_limit && count as f64 >= thresholds.burst_ratio * median {
                findings.push(PoisoningFinding {
                    class: PoisoningClass::Spam,
                    sources: BTreeSet::from([source.to_string()]),
                    evidence: format!(
                        "{count} claims in the window against a peer median of {median}"
                    ),
                    claims,
                });
            }
        }
    }

    findings.sort_by(|a, b| a.class.cmp(&b.class).then(a.sources.cmp(&b.sources)));
    findings
}

fn median_of(sorted: &[usize]) -> Option<f64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    Some(if n % 2 == 1 {
        sorted[n / 2] as f64
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) as f64 / 2.0
    })
}
