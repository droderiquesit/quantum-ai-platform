//! The evidence fabric: where an [`EvidenceRecord`] is checked, linked and
//! weighed before anything believes it (R12-EVID-2031..2038, R12-DATA-2012).
//!
//! Before this module the record type had every §7 field and nothing
//! constructed it; the checks it carried (`check_temporal_consistency`) had
//! no production caller, the corroborate/contradict vectors were never
//! filled, authenticity signals were stored and never read, and a
//! contradiction was something a reasoning engine could represent and
//! nothing on the ingestion path ever created. [`EvidenceFabric::admit`] is
//! the one seam that does all of it, in a fixed order, for every claim:
//!
//! 1. refuse a claim with incomplete provenance;
//! 2. the temporal check against what is held;
//! 3. the geographic check against what is held;
//! 4. authenticity, from the signals the claim carries *and that the
//!    fabric can verify* — a hash is credited only if it matches the
//!    document the claim was derived from, a TLS domain only if it is the
//!    source's own host, a signature never (this platform verifies none);
//! 5. linking: a claim agreeing with a held claim about the same subject,
//!    attribute and instant from an *independent origin* corroborates it,
//!    one disagreeing contradicts it, and both are kept as competing
//!    hypotheses rather than either being deleted;
//! 6. the provenance graph: claim → document → source, plus the
//!    corroborate/contradict edges.
//!
//! The outcome is an [`EvidenceVerdict`] whose `belief_weight` is what a
//! downstream belief should be moved by — confidence × verified authenticity
//! × the source's track record — and a claim on a physically impossible
//! timeline is marked inadmissible so the ingestion stage can quarantine it.
//!
//! # Bounded
//!
//! Every collection is capped. Claims are evicted oldest first and take
//! their graph edges and hypothesis entries with them; source histories are
//! capped by count and keep the most recently active. A fabric fed for a
//! year holds what it held after its first `capacity` claims.

use crate::evidence::{
    AuthenticitySignal, EvidenceRecord, GeographicConsistency, META_ATTRIBUTE, META_VALUE,
    TemporalConsistency,
};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Claims a fabric holds by default before evicting the oldest.
pub const DEFAULT_EVIDENCE_CAPACITY: usize = 4_096;
/// Sources whose track record is kept by default.
pub const DEFAULT_SOURCE_CAPACITY: usize = 512;
/// The authenticity a claim earns with no verifiable signal at all. Not zero:
/// a claim from an admitted source with nothing else is still a claim; it is
/// simply not one to lean on.
pub const UNSIGNED_AUTHENTICITY: f64 = 0.2;
/// Authenticity at or above which a later claim may resolve a contest.
pub const RESOLVING_AUTHENTICITY: f64 = 0.6;

/// A node of the provenance graph.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ProvenanceNode {
    Claim(String),
    /// A fetched document, by content hash.
    Document(String),
    Source(String),
}

/// An edge of the provenance graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceEdge {
    /// Claim → the document it was extracted from.
    DerivedFrom,
    /// Document → the source that served it.
    PublishedBy,
    /// Claim → the claim it agrees with.
    Corroborates,
    /// Claim → the claim it disagrees with.
    Contradicts,
}

/// Claims linked to documents and sources and to each other (R12-EVID-2033).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ProvenanceGraph {
    edges: BTreeSet<(ProvenanceNode, ProvenanceEdge, ProvenanceNode)>,
}

impl ProvenanceGraph {
    fn link(&mut self, from: ProvenanceNode, edge: ProvenanceEdge, to: ProvenanceNode) {
        self.edges.insert((from, edge, to));
    }

    /// Remove every edge touching `claim`, and any document left with no
    /// claim deriving from it.
    fn forget_claim(&mut self, claim: &str) {
        let node = ProvenanceNode::Claim(claim.to_string());
        let documents: Vec<ProvenanceNode> = self
            .edges
            .iter()
            .filter(|(from, edge, _)| *from == node && *edge == ProvenanceEdge::DerivedFrom)
            .map(|(_, _, to)| to.clone())
            .collect();
        self.edges
            .retain(|(from, _, to)| *from != node && *to != node);
        for document in documents {
            let still_used = self
                .edges
                .iter()
                .any(|(_, edge, to)| *edge == ProvenanceEdge::DerivedFrom && *to == document);
            if !still_used {
                self.edges.retain(|(from, _, _)| *from != document);
            }
        }
    }

    /// Edges leaving `node` of kind `edge`.
    pub fn targets(&self, node: &ProvenanceNode, edge: ProvenanceEdge) -> Vec<&ProvenanceNode> {
        self.edges
            .iter()
            .filter(|(from, kind, _)| from == node && *kind == edge)
            .map(|(_, _, to)| to)
            .collect()
    }

    /// The sources a claim traces back to through its documents.
    pub fn sources_of(&self, claim: &str) -> BTreeSet<String> {
        let mut sources = BTreeSet::new();
        for document in self.targets(
            &ProvenanceNode::Claim(claim.to_string()),
            ProvenanceEdge::DerivedFrom,
        ) {
            for source in self.targets(document, ProvenanceEdge::PublishedBy) {
                if let ProvenanceNode::Source(id) = source {
                    sources.insert(id.clone());
                }
            }
        }
        sources
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Every edge, in a stable order, for the record.
    pub fn edges(&self) -> impl Iterator<Item = &(ProvenanceNode, ProvenanceEdge, ProvenanceNode)> {
        self.edges.iter()
    }
}

/// What the fabric could and could not verify about a claim's authenticity
/// (R12-EVID-2034).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Authenticity {
    /// In `[0, 1]`: [`UNSIGNED_AUTHENTICITY`] plus what was verified.
    pub score: f64,
    /// Signals the fabric checked and credited.
    pub credited: Vec<String>,
    /// Signals carried and not credited, each with the reason.
    pub refused: Vec<String>,
    /// A signal contradicted the claim's own identity — a TLS domain that is
    /// not the source's host, a hash that is not the document's. Read by the
    /// poisoning detector as spoofing.
    pub spoofed: bool,
}

/// The host part of a source identity, if it is a URL.
fn host_of(identity: &str) -> Option<String> {
    let rest = identity
        .split_once("://")
        .map_or(identity, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next()?.split(':').next()?;
    (!host.is_empty() && host.contains('.')).then(|| host.to_ascii_lowercase())
}

/// Assess the signals a claim carries against what the fabric can verify.
///
/// `document_hash` is the SHA-256 the ingestion path took over the fetched
/// body; `registries` the identity registries this deployment trusts.
pub fn assess_authenticity(
    record: &EvidenceRecord,
    document_hash: Option<&str>,
    registries: &BTreeSet<String>,
) -> Authenticity {
    let mut score = UNSIGNED_AUTHENTICITY;
    let mut credited = Vec::new();
    let mut refused = Vec::new();
    let mut spoofed = false;
    let mut identity_registered = false;
    for signal in &record.authenticity_signals {
        if let AuthenticitySignal::IdentityRegistered { registry } = signal
            && registries.contains(registry)
        {
            identity_registered = true;
        }
    }
    for signal in &record.authenticity_signals {
        match signal {
            AuthenticitySignal::HashVerified { algorithm, hash } => {
                match document_hash {
                    Some(document) if algorithm.eq_ignore_ascii_case("sha256") && hash == document => {
                        score += 0.3;
                        credited.push(format!("sha256 {} matches the fetched document", short(hash)));
                    }
                    Some(_) => {
                        spoofed = true;
                        refused.push(format!(
                            "{algorithm} {} does not match the fetched document",
                            short(hash)
                        ));
                    }
                    None => refused.push(format!(
                        "{algorithm} {} names no document this fabric holds",
                        short(hash)
                    )),
                }
            }
            AuthenticitySignal::TlsVerified { domain } => {
                match host_of(&record.source_identity) {
                    Some(host) if host == domain.to_ascii_lowercase() || host.ends_with(&format!(".{}", domain.to_ascii_lowercase())) => {
                        score += 0.2;
                        credited.push(format!("tls domain {domain} is the source's host"));
                    }
                    Some(host) => {
                        spoofed = true;
                        refused.push(format!(
                            "tls domain {domain} is not the source's host {host}"
                        ));
                    }
                    None => refused.push(format!(
                        "tls domain {domain} cannot be compared: the source identity names no host"
                    )),
                }
            }
            AuthenticitySignal::IdentityRegistered { registry } => {
                if registries.contains(registry) {
                    score += 0.25;
                    credited.push(format!("identity registered with {registry}"));
                } else {
                    refused.push(format!("registry {registry} is not one this deployment trusts"));
                }
            }
            AuthenticitySignal::OfficialRelease { channel } => {
                if identity_registered {
                    score += 0.1;
                    credited.push(format!("official release on {channel} from a registered identity"));
                } else {
                    refused.push(format!(
                        "official-release channel {channel} claimed by an unregistered identity"
                    ));
                }
            }
            // This platform verifies no asymmetric signature (no in-tree
            // signing, ADR 0043), so a signature claim is recorded and never
            // credited: crediting it would be believing the claim's own word.
            AuthenticitySignal::SignatureVerified { signer } => refused.push(format!(
                "signature by {signer} carried and not verified: this platform verifies no signature"
            )),
        }
    }
    Authenticity {
        score: score.min(1.0),
        credited,
        refused,
        spoofed,
    }
}

fn short(hash: &str) -> &str {
    &hash[..12.min(hash.len())]
}

/// A source's track record: how often its claims were later corroborated
/// or contradicted by the resolution of a contest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SourceHistory {
    pub claims: u64,
    pub corroborated: u64,
    pub contradicted: u64,
    pub impossible: u64,
    pub spoofed: u64,
    pub last_seen: Option<Timestamp>,
}

impl SourceHistory {
    /// Laplace-smoothed share of resolved claims that held up, in `(0, 1)`.
    /// A source with no resolved claims reads 0.5: unknown, not trusted.
    pub fn reliability(&self) -> f64 {
        (self.corroborated as f64 + 1.0) / ((self.corroborated + self.contradicted) as f64 + 2.0)
    }
}

/// What one claim asserts, as a key and a value. Two claims with the same
/// key and different values cannot both be true.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ClaimKey {
    pub subject: String,
    pub attribute: String,
    pub instant: Timestamp,
}

impl ClaimKey {
    /// The key of `record`, if its metadata names a subject, an attribute and
    /// a value — a claim that says nothing checkable has no key.
    pub fn of(record: &EvidenceRecord) -> Option<(Self, String)> {
        let subject = record.subject()?.to_string();
        let attribute = record.metadata.get(META_ATTRIBUTE)?.clone();
        let value = record.metadata.get(META_VALUE)?.clone();
        Some((
            Self {
                subject,
                attribute,
                instant: record.claim_instant(),
            },
            value,
        ))
    }
}

/// Competing values for one claim key, kept until resolved
/// (R12-EVID-2038). Nothing is deleted when two values disagree.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Contest {
    /// Each value and the claims asserting it.
    pub hypotheses: BTreeMap<String, BTreeSet<String>>,
    /// The value that won and the claim that resolved it.
    pub resolution: Option<Resolution>,
}

impl Contest {
    pub fn is_contested(&self) -> bool {
        self.hypotheses.len() > 1 && self.resolution.is_none()
    }
}

/// How a contest was resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Resolution {
    pub value: String,
    /// The later claim whose authenticity resolved it.
    pub resolved_by: String,
    pub at: Timestamp,
}

/// The fabric's outcome for one admitted claim.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EvidenceVerdict {
    pub id: String,
    pub temporal: TemporalConsistency,
    pub geographic: GeographicConsistency,
    pub authenticity: Authenticity,
    /// The source's reliability when the claim arrived.
    pub source_reliability: f64,
    /// confidence × authenticity × reliability, in `[0, 1]`. What a belief
    /// should be moved by; independent of how many copies of the claim exist.
    pub belief_weight: f64,
    pub corroborates: Vec<String>,
    pub contradicts: Vec<String>,
    /// A claim this verdict resolved a contest in favour of, if any.
    pub resolved: Option<ClaimKey>,
    /// False for a claim on a physically impossible timeline: ingested
    /// before it was published, or describing an event after its own
    /// publication. The ingestion stage quarantines it.
    pub admissible: bool,
}

/// The knowledge delta one batch of claims produced — what changed in what
/// the platform believes, as distinct from the claims themselves
/// (R12-DATA-2012's "knowledge delta").
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct KnowledgeDelta {
    /// Claim keys asserted for the first time.
    pub new_keys: Vec<ClaimKey>,
    /// Keys that became contested in this batch.
    pub contested: Vec<ClaimKey>,
    /// Keys resolved in this batch.
    pub resolved: Vec<ClaimKey>,
    /// Claims admitted and claims refused as inadmissible.
    pub admitted: u64,
    pub inadmissible: u64,
}

impl KnowledgeDelta {
    pub fn is_empty(&self) -> bool {
        self.new_keys.is_empty()
            && self.contested.is_empty()
            && self.resolved.is_empty()
            && self.admitted == 0
            && self.inadmissible == 0
    }

    /// This delta as a knowledge pack (R12-DATA-1036, 1041): the compact,
    /// schema-checked artifact derived knowledge is stored in, carrying
    /// which claim keys were asserted, contested and resolved between two
    /// fabric versions — never a claim's text or the document it came from.
    pub fn to_pack(
        &self,
        id: impl Into<String>,
        created: Timestamp,
        from_version: u64,
        to_version: u64,
    ) -> qip_contracts::knowledge_pack::KnowledgePack {
        let key = |prefix: &str, key: &ClaimKey| {
            format!(
                "{prefix}:{}/{}@{}",
                key.subject,
                key.attribute,
                key.instant.as_nanos()
            )
        };
        let mut added: Vec<String> = self.new_keys.iter().map(|k| key("asserted", k)).collect();
        added.extend(self.contested.iter().map(|k| key("contested", k)));
        added.extend(self.resolved.iter().map(|k| key("resolved", k)));
        let created_ms = u64::try_from(created.as_nanos() / 1_000_000).unwrap_or(0);
        qip_contracts::knowledge_pack::KnowledgePack::new(
            id,
            created_ms,
            qip_contracts::knowledge_pack::PackBody::WorldModelDelta {
                from_version,
                to_version,
                added,
                removed: Vec::new(),
            },
        )
    }

    /// Fold one verdict in.
    pub fn absorb(
        &mut self,
        verdict: &EvidenceVerdict,
        outcome: KeyOutcome,
        key: Option<ClaimKey>,
    ) {
        if verdict.admissible {
            self.admitted += 1;
        } else {
            self.inadmissible += 1;
        }
        if let Some(key) = key {
            match outcome {
                KeyOutcome::New => self.new_keys.push(key),
                KeyOutcome::BecameContested => self.contested.push(key),
                KeyOutcome::Unchanged => {}
            }
        }
        if let Some(resolved) = &verdict.resolved {
            self.resolved.push(resolved.clone());
        }
    }
}

/// What admitting a claim did to its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    New,
    BecameContested,
    Unchanged,
}

/// One admitted claim held by the fabric.
#[derive(Clone, Debug, Serialize)]
pub struct HeldEvidence {
    #[serde(skip)]
    pub record: EvidenceRecord,
    pub document_hash: Option<String>,
    pub verdict: EvidenceVerdict,
    /// A fingerprint of the claim's normalised statement, so the same text
    /// re-bylined by another source is recognised as one origin.
    pub statement_fingerprint: String,
}

/// The bounded evidence store with its graph, contests and source records.
#[derive(Clone, Debug)]
pub struct EvidenceFabric {
    capacity: usize,
    source_capacity: usize,
    registries: BTreeSet<String>,
    held: VecDeque<HeldEvidence>,
    graph: ProvenanceGraph,
    contests: BTreeMap<ClaimKey, Contest>,
    sources: BTreeMap<String, SourceHistory>,
    /// Claims admitted since the fabric was made; the version a knowledge
    /// pack's delta runs between.
    version: u64,
}

impl Default for EvidenceFabric {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_EVIDENCE_CAPACITY,
            source_capacity: DEFAULT_SOURCE_CAPACITY,
            registries: BTreeSet::new(),
            held: VecDeque::new(),
            graph: ProvenanceGraph::default(),
            contests: BTreeMap::new(),
            sources: BTreeMap::new(),
            version: 0,
        }
    }
}

/// Normalise a statement for origin comparison: lowercase, alphanumerics
/// only, whitespace collapsed — so punctuation and a changed byline do not
/// make one press release into two.
pub fn statement_fingerprint(statement: &str) -> String {
    let normalised: String = statement
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    qip_core::sha256_hex(normalised.as_bytes())
}

impl EvidenceFabric {
    /// A fabric holding at most `capacity` claims and `source_capacity`
    /// source histories. Refuses a zero capacity, which would hold nothing
    /// and check every claim against nothing.
    pub fn with_capacity(capacity: usize, source_capacity: usize) -> Result<Self> {
        if capacity == 0 || source_capacity == 0 {
            return Err(Error::invalid(
                "an evidence fabric with zero capacity checks every claim against nothing; give \
                 it room for at least one claim and one source",
            ));
        }
        Ok(Self {
            capacity,
            source_capacity,
            ..Self::default()
        })
    }

    /// Trust `registry` as an identity registry for authenticity.
    pub fn trusting_registry(mut self, registry: impl Into<String>) -> Self {
        self.registries.insert(registry.into());
        self
    }

    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Claims admitted so far, evicted ones included.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn held(&self) -> impl Iterator<Item = &HeldEvidence> {
        self.held.iter()
    }

    pub fn get(&self, id: &str) -> Option<&HeldEvidence> {
        self.held.iter().find(|held| held.record.id == id)
    }

    pub fn graph(&self) -> &ProvenanceGraph {
        &self.graph
    }

    pub fn contests(&self) -> &BTreeMap<ClaimKey, Contest> {
        &self.contests
    }

    pub fn source_history(&self, source: &str) -> Option<&SourceHistory> {
        self.sources.get(source)
    }

    pub fn sources(&self) -> &BTreeMap<String, SourceHistory> {
        &self.sources
    }

    /// How many independent origins stand behind `claims`: claims derived
    /// from one document, or carrying one normalised statement, count once
    /// however many sources repeat them (R12-EVID-2035). Ten copies of one
    /// press release are one origin.
    pub fn independent_origins<'a>(&self, claims: impl IntoIterator<Item = &'a str>) -> usize {
        // Union-find over claims, joined when they share a document or a
        // statement fingerprint.
        let held: Vec<&HeldEvidence> = claims.into_iter().filter_map(|id| self.get(id)).collect();
        let mut parent: Vec<usize> = (0..held.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for i in 0..held.len() {
            for j in (i + 1)..held.len() {
                let shared_document = held[i].document_hash.is_some()
                    && held[i].document_hash == held[j].document_hash;
                let shared_text = held[i].statement_fingerprint == held[j].statement_fingerprint;
                if shared_document || shared_text {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a] = b;
                }
            }
        }
        (0..held.len())
            .map(|i| find(&mut parent, i))
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Check, link, weigh and hold one claim. See the module doc for the
    /// order. Refuses a claim with incomplete provenance or a duplicate id;
    /// returns an inadmissible verdict (still held, so the poisoning
    /// detector can see the pattern) for an impossible timeline.
    pub fn admit(
        &mut self,
        record: EvidenceRecord,
        document_hash: Option<&str>,
        now: Timestamp,
    ) -> Result<(EvidenceVerdict, KeyOutcome)> {
        if !record.is_complete() || record.id.trim().is_empty() {
            return Err(Error::invalid(format!(
                "evidence `{}` from `{}` has incomplete provenance and is refused before it can \
                 be weighed",
                record.id, record.source_identity
            )));
        }
        if self.get(&record.id).is_some() {
            return Err(Error::invalid(format!(
                "evidence `{}` is already held; a claim admitted twice would corroborate itself",
                record.id
            )));
        }
        let temporal = record.check_temporal_consistency(self.held.iter().map(|h| &h.record));
        let geographic = record.check_geographic_consistency(self.held.iter().map(|h| &h.record));
        let authenticity = assess_authenticity(&record, document_hash, &self.registries);
        let admissible = !matches!(
            temporal,
            TemporalConsistency::ViolatesPhysicalTimeline { .. }
                | TemporalConsistency::PostdatesSource { .. }
        );
        let fingerprint = statement_fingerprint(&record.statement);

        // Linking, through the contest for this claim's key.
        let mut corroborates = Vec::new();
        let mut contradicts = Vec::new();
        let mut outcome = KeyOutcome::Unchanged;
        let mut resolved = None;
        let keyed = ClaimKey::of(&record);
        if admissible && let Some((key, value)) = &keyed {
            let contest = self.contests.entry(key.clone()).or_default();
            let was_contested = contest.hypotheses.len() > 1;
            if contest.hypotheses.is_empty() {
                outcome = KeyOutcome::New;
            }
            for (held_value, ids) in &contest.hypotheses {
                for id in ids {
                    let Some(other) = self.held.iter().find(|h| &h.record.id == id) else {
                        continue;
                    };
                    let same_origin = other.statement_fingerprint == fingerprint
                        || (document_hash.is_some()
                            && other.document_hash.as_deref() == document_hash);
                    if held_value == value {
                        if !same_origin {
                            corroborates.push(id.clone());
                        }
                    } else {
                        contradicts.push(id.clone());
                    }
                }
            }
            contest
                .hypotheses
                .entry(value.clone())
                .or_default()
                .insert(record.id.clone());
            if !was_contested && contest.hypotheses.len() > 1 {
                outcome = KeyOutcome::BecameContested;
            }
            // A later, authentic claim resolves a live contest in favour of
            // its value; the losing hypotheses stay on the record.
            if contest.hypotheses.len() > 1
                && contest.resolution.is_none()
                && authenticity.score >= RESOLVING_AUTHENTICITY
            {
                contest.resolution = Some(Resolution {
                    value: value.clone(),
                    resolved_by: record.id.clone(),
                    at: now,
                });
                resolved = Some(key.clone());
            }
        }

        let history = self
            .sources
            .entry(record.source_identity.clone())
            .or_default();
        history.claims += 1;
        history.last_seen = Some(now);
        if !admissible {
            history.impossible += 1;
        }
        if authenticity.spoofed {
            history.spoofed += 1;
        }
        let source_reliability = history.reliability();
        let belief_weight = if admissible {
            (record.confidence * authenticity.score * source_reliability).clamp(0.0, 1.0)
        } else {
            0.0
        };

        if let (Some(key), Some(_)) = (&resolved, &keyed) {
            self.score_resolution(key);
        }

        // The graph.
        let claim = ProvenanceNode::Claim(record.id.clone());
        let document = ProvenanceNode::Document(
            document_hash.map_or_else(|| format!("unhashed:{}", record.id), str::to_string),
        );
        let source = ProvenanceNode::Source(record.source_identity.clone());
        self.graph
            .link(claim.clone(), ProvenanceEdge::DerivedFrom, document.clone());
        self.graph
            .link(document, ProvenanceEdge::PublishedBy, source);
        for id in &corroborates {
            self.graph.link(
                claim.clone(),
                ProvenanceEdge::Corroborates,
                ProvenanceNode::Claim(id.clone()),
            );
        }
        for id in &contradicts {
            self.graph.link(
                claim.clone(),
                ProvenanceEdge::Contradicts,
                ProvenanceNode::Claim(id.clone()),
            );
        }

        let mut record = record;
        record.temporal_consistency = Some(temporal.clone());
        record.geographic_consistency = Some(geographic.clone());
        for id in &corroborates {
            record.corroborates.push(id.clone());
        }
        for id in &contradicts {
            record.contradicts.push(id.clone());
        }
        let verdict = EvidenceVerdict {
            id: record.id.clone(),
            temporal,
            geographic,
            authenticity,
            source_reliability,
            belief_weight,
            corroborates,
            contradicts,
            resolved,
            admissible,
        };
        self.held.push_back(HeldEvidence {
            record,
            document_hash: document_hash.map(str::to_string),
            verdict: verdict.clone(),
            statement_fingerprint: fingerprint,
        });
        self.version = self.version.saturating_add(1);
        self.evict();
        Ok((verdict, outcome))
    }

    /// Credit the sources behind the winning value and debit those behind
    /// the losing ones — the only place a track record moves, because only
    /// a resolution says who was right.
    fn score_resolution(&mut self, key: &ClaimKey) {
        let Some(contest) = self.contests.get(key) else {
            return;
        };
        let Some(resolution) = &contest.resolution else {
            return;
        };
        let mut credit: Vec<(String, bool)> = Vec::new();
        for (value, ids) in &contest.hypotheses {
            for id in ids {
                if *id == resolution.resolved_by {
                    continue;
                }
                if let Some(held) = self.held.iter().find(|h| &h.record.id == id) {
                    credit.push((
                        held.record.source_identity.clone(),
                        *value == resolution.value,
                    ));
                }
            }
        }
        for (source, won) in credit {
            let history = self.sources.entry(source).or_default();
            if won {
                history.corroborated += 1;
            } else {
                history.contradicted += 1;
            }
        }
    }

    fn evict(&mut self) {
        while self.held.len() > self.capacity {
            let Some(oldest) = self.held.pop_front() else {
                break;
            };
            let id = oldest.record.id;
            self.graph.forget_claim(&id);
            self.contests.retain(|_, contest| {
                for ids in contest.hypotheses.values_mut() {
                    ids.remove(&id);
                }
                contest.hypotheses.retain(|_, ids| !ids.is_empty());
                !contest.hypotheses.is_empty()
            });
        }
        while self.sources.len() > self.source_capacity {
            // Drop the source seen least recently; ties broken by name so
            // the choice replays.
            let Some(stalest) = self
                .sources
                .iter()
                .min_by(|a, b| a.1.last_seen.cmp(&b.1.last_seen).then(a.0.cmp(b.0)))
                .map(|(name, _)| name.clone())
            else {
                break;
            };
            self.sources.remove(&stalest);
        }
    }
}
