//! The bitemporal knowledge graph.
//!
//! Every fact records two time ranges: when it was true in the world, and when
//! the platform knew it. A query supplies both, and the graph answers with what
//! was believed at that moment — not with what is believed now.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
pub use qip_entity_resolution::entity::EntityKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::relationship::{Relationship, RelationshipKind};

/// The longest run of text any one field of a graph record may hold, in
/// characters (WORLD-057).
///
/// The graph holds distilled knowledge and the reference a source can be
/// fetched from again; it never holds the source. Until this limit existed
/// that rested entirely on the upstream types happening to have no body
/// field, and on headlines happening to be short: `add_node` took any string
/// as a label or an attribute, so the first caller to put an article where a
/// headline goes would have made the graph a copy of a document nobody
/// licensed it to keep. 280 characters holds any name, identifier, locator
/// or headline and is far too short for a body, so a verbatim span of a
/// source longer than this cannot be written at all.
pub const EXCERPT_LIMIT: usize = 280;

/// Refuse text too long to be a name, a reference or a headline.
///
/// One function for every string a graph record carries, called at the two
/// write seams ([`KnowledgeGraph::add_node`] and [`Fact::new`]) and by the
/// ingestion path before it writes anything, because a limit stated in two
/// places is two limits. Refused, never truncated: a truncated body is still
/// a copied excerpt nobody chose, and the caller's bug survives it.
pub fn refuse_a_body(record: &str, field: &str, text: &str) -> Result<()> {
    let length = text.chars().count();
    if length > EXCERPT_LIMIT {
        return Err(Error::invalid(format!(
            "{record} carries {length} characters in its {field}, over the \
             {EXCERPT_LIMIT}-character excerpt limit -- the knowledge graph holds distilled \
             knowledge and the reference to fetch a source again, never a copy of it; write \
             the locator and a content hash and leave the text where it was fetched from"
        )));
    }
    Ok(())
}

/// The first few characters of `text`, for naming a refused record in its own
/// refusal without quoting the body the refusal is about.
fn excerpt_of(text: &str) -> String {
    text.chars().take(40).collect()
}

/// What a node represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Entity,
    FinancialObject,
    Event,
    Factor,
    Portfolio,
    Thesis,
    Evidence,
    /// The authority a thesis is settled against. Held here rather than only
    /// on the proposition that names it, so "who said so" is an edge to
    /// follow rather than a struct to know about.
    ResolutionSource,
}

/// A vertex.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    /// When the platform first recorded the node.
    pub recorded_at: Timestamp,
    /// What an entity node is, as a type (WORLD-003). Set by
    /// [`Node::entity`] and required of every [`NodeKind::Entity`] node at
    /// the write seam; `None` on every other kind. This was a string
    /// attribute called `kind` that nothing read, so "which nodes are
    /// supply chains" was a string comparison somebody had to spell right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_kind: Option<EntityKind>,
    /// The instant an event node applies to: when the thing happened, as
    /// distinct from `recorded_at`, when the platform learned of it
    /// (WORLD-003). Set by [`Node::event`] and required of every
    /// [`NodeKind::Event`] node at the write seam; `None` on every other
    /// kind. Without it an event's own time lived only on whichever fact
    /// happened to point at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<Timestamp>,
}

impl Node {
    pub fn new(
        id: impl Into<String>,
        kind: NodeKind,
        label: impl Into<String>,
        recorded_at: Timestamp,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            label: label.into(),
            attributes: BTreeMap::new(),
            recorded_at,
            entity_kind: None,
            occurred_at: None,
        }
    }

    /// An entity, with the kind of thing it is.
    pub fn entity(
        id: impl Into<String>,
        entity_kind: EntityKind,
        label: impl Into<String>,
        recorded_at: Timestamp,
    ) -> Self {
        let mut node = Self::new(id, NodeKind::Entity, label, recorded_at);
        node.entity_kind = Some(entity_kind);
        node
    }

    /// An event, with the instant it applies to and the instant the platform
    /// learned of it. Two parameters of one type in a fixed order, because
    /// they are the two time dimensions and an event that happened on Monday
    /// and was learned on Tuesday is not the reverse.
    pub fn event(
        id: impl Into<String>,
        label: impl Into<String>,
        occurred_at: Timestamp,
        recorded_at: Timestamp,
    ) -> Self {
        let mut node = Self::new(id, NodeKind::Event, label, recorded_at);
        node.occurred_at = Some(occurred_at);
        node
    }

    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }
}

/// A relationship with its two time dimensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub relationship: Relationship,
    /// When the relationship began to hold in the world.
    pub valid_from: Timestamp,
    /// When it stopped holding. `None` means it still holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<Timestamp>,
    /// When the platform learned it.
    pub recorded_at: Timestamp,
    /// When the platform learned it was wrong or superseded. A retracted fact
    /// is never deleted: a decision made while it was believed must remain
    /// reconstructable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted_at: Option<Timestamp>,
    /// Confidence in the claim, in `[0, 1]`.
    pub confidence: f64,
}

impl Fact {
    /// A belief about the world, stated with the confidence it is held at.
    ///
    /// Refused rather than defaulted or clamped (WORLD-008, WORLD-056). This
    /// constructor once took no confidence and stamped `1.0`, and its
    /// `with_confidence` clamped, so a news fact with no stated confidence was
    /// stored as certain and a broken estimator's `2.0` read as certainty;
    /// `NAN.clamp(0.0, 1.0)` is `NAN`, so the one value a clamp most needed
    /// to stop passed straight through. A confidence or relationship weight
    /// outside `[0, 1]` (NaN included) and an empty source are each a caller
    /// bug the caller must fix at its origin.
    pub fn new(
        relationship: Relationship,
        valid_from: Timestamp,
        recorded_at: Timestamp,
        confidence: f64,
    ) -> Result<Self> {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(Error::invalid(format!(
                "fact `{}` was stated with confidence {confidence}, which is not a finite \
                 value in [0, 1] -- state the confidence the claim is held at; a missing \
                 one is not 1.0",
                relationship.key()
            )));
        }
        if !(0.0..=1.0).contains(&relationship.weight) {
            return Err(Error::invalid(format!(
                "relationship `{}` has weight {}, which is not a finite value in [0, 1] -- \
                 fix the weight at its source rather than relying on it being clamped",
                relationship.key(),
                relationship.weight
            )));
        }
        if relationship.source.trim().is_empty() {
            return Err(Error::invalid(format!(
                "relationship `{}` names no source -- cite the reference the claim can be \
                 fetched from again",
                relationship.key()
            )));
        }
        // WORLD-057: a source is a reference and the two ends are node ids;
        // a body in any of them is a document stored under another name.
        for (field, text) in [
            ("source", &relationship.source),
            ("from end", &relationship.from),
            ("to end", &relationship.to),
        ] {
            refuse_a_body("a fact", field, text)?;
        }
        Ok(Self {
            relationship,
            valid_from,
            valid_to: None,
            recorded_at,
            retracted_at: None,
            confidence,
        })
    }

    pub fn valid_until(mut self, until: Timestamp) -> Self {
        self.valid_to = Some(until);
        self
    }

    /// Whether the fact held at `valid_at` and was known by `known_at`.
    ///
    /// Both conditions, always. Checking only one is precisely the mistake that
    /// lets a backtest use a relationship discovered after the fact.
    pub fn holds(&self, valid_at: Timestamp, known_at: Timestamp) -> bool {
        if self.recorded_at > known_at {
            return false;
        }
        if self.retracted_at.is_some_and(|r| r <= known_at) {
            return false;
        }
        if self.valid_from > valid_at {
            return false;
        }
        if self.valid_to.is_some_and(|v| v <= valid_at) {
            return false;
        }
        true
    }

    /// Whether the fact holds now, given everything known.
    pub fn holds_now(&self, now: Timestamp) -> bool {
        self.holds(now, now)
    }
}

/// Weight gap at or over which two sources' statements of one relationship
/// cannot both be right.
///
/// A relationship's weight is a fraction of something (a revenue share, a
/// supply share, how strongly an item concerns an entity). Sources differ in
/// the second decimal all the time and that is noise; a quarter of the whole
/// scale apart, they are describing different worlds.
pub const CONTRADICTION_GAP: f64 = 0.25;

/// How many contradiction records the graph holds before the oldest leaves.
/// The facts themselves are never evicted, so a record that aged out can be
/// re-derived from them; this bounds the working set, not the knowledge.
pub const CONTRADICTION_HISTORY: usize = 1_024;

/// One side of a contradiction: enough to find the belief again among the
/// versions held under the relationship's key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Belief {
    pub source: String,
    pub weight: f64,
    pub confidence: f64,
    pub recorded_at: Timestamp,
}

impl Belief {
    fn of(fact: &Fact) -> Self {
        Self {
            source: fact.relationship.source.clone(),
            weight: fact.relationship.weight,
            confidence: fact.confidence,
            recorded_at: fact.recorded_at,
        }
    }
}

/// Two beliefs about one relationship that cannot both be right, linked
/// (WORLD-009).
///
/// The graph already kept both: a second statement under a key is another
/// version, never an overwrite. What it did not keep was the fact *that they
/// disagree*. A reader walking `facts_at` saw two edges and multiplied
/// through whichever came first, and "the sources conflict about this" was
/// knowledge the platform held and could not state. A retraction is not this
/// record: it says one side stopped being believed, and names no other side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contradiction {
    /// The relationship both beliefs are about.
    pub key: String,
    /// The belief the graph already held.
    pub held: Belief,
    /// The belief whose arrival exposed the conflict.
    pub arrived: Belief,
    /// How far apart the two weights are.
    pub gap: f64,
    /// The first instant the platform held both, which is when the conflict
    /// became knowable. Reads are filtered on this.
    pub detected_at: Timestamp,
}

/// A path through the graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub nodes: Vec<String>,
    pub edges: Vec<RelationshipKind>,
    /// Product of the edge weights along the path.
    pub strength: f64,
}

impl Path {
    pub fn length(&self) -> usize {
        self.edges.len()
    }
}

/// A bitemporal directed multigraph.
#[derive(Debug, Default)]
pub struct KnowledgeGraph {
    nodes: BTreeMap<String, Node>,
    /// Facts keyed by their relationship key; several versions may share a key
    /// over time, so each key holds a history.
    facts: BTreeMap<String, Vec<Fact>>,
    /// Outgoing adjacency: node to fact keys.
    outgoing: BTreeMap<String, BTreeSet<String>>,
    /// Incoming adjacency.
    incoming: BTreeMap<String, BTreeSet<String>>,
    /// Conflicts between sources, oldest first, bounded by
    /// [`CONTRADICTION_HISTORY`].
    contradictions: VecDeque<Contradiction>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of distinct relationships, counting all historical versions.
    pub fn fact_count(&self) -> usize {
        self.facts.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Write a node, refusing one that carries a body (WORLD-057).
    ///
    /// The id, the label and every attribute are held to [`EXCERPT_LIMIT`],
    /// and a refused node leaves the graph exactly as it was. This returned
    /// nothing and accepted any string until the limit existed, so "the
    /// graph stores no documents" was a property of today's callers and not
    /// of the graph.
    ///
    /// Also refused: an entity node that does not say what kind of thing it
    /// is, and an event node that does not say when it happened (WORLD-003).
    /// Both were writable until the typed fields existed, and an untyped
    /// entity or an undated event is a node every typed or as-of query then
    /// silently leaves out.
    pub fn add_node(&mut self, node: Node) -> Result<()> {
        let record = format!("node `{}`", excerpt_of(&node.id));
        if node.kind == NodeKind::Entity && node.entity_kind.is_none() {
            return Err(Error::invalid(format!(
                "{record} is an entity with no entity kind -- build it with `Node::entity`, \
                 naming what kind of thing it is"
            )));
        }
        if node.kind == NodeKind::Event && node.occurred_at.is_none() {
            return Err(Error::invalid(format!(
                "{record} is an event with no instant it applies to -- build it with \
                 `Node::event`, giving when it happened as well as when it was learned"
            )));
        }
        refuse_a_body(&record, "id", &node.id)?;
        refuse_a_body(&record, "label", &node.label)?;
        for (key, value) in &node.attributes {
            refuse_a_body(&record, "attribute name", key)?;
            refuse_a_body(&record, &format!("`{key}` attribute"), value)?;
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn nodes_of_kind(&self, kind: NodeKind) -> Vec<&Node> {
        self.nodes.values().filter(|n| n.kind == kind).collect()
    }

    /// Every entity of one kind, by the type and not by a label.
    pub fn entities_of_kind(&self, kind: EntityKind) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.entity_kind == Some(kind))
            .collect()
    }

    /// The events that had happened by `valid_at` and were known by
    /// `known_at`: the node-side counterpart of [`Self::facts_at`], with the
    /// same two questions asked and for the same reason.
    pub fn events_at(&self, valid_at: Timestamp, known_at: Timestamp) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.kind == NodeKind::Event && n.recorded_at <= known_at)
            .filter(|n| n.occurred_at.is_some_and(|at| at <= valid_at))
            .collect()
    }

    /// Record a fact, and its inverse where the relationship implies one.
    ///
    /// Where another source already holds a materially different statement
    /// of the same relationship, the conflict is recorded as a
    /// [`Contradiction`] linking the two, and both stay in the graph
    /// (WORLD-009). Checked against the stated direction only: the inverse
    /// is the same claim read backwards, and recording it twice would count
    /// one disagreement as two.
    pub fn assert_fact(&mut self, fact: Fact) {
        for contradiction in self.contradictions_with(&fact) {
            if self.contradictions.len() == CONTRADICTION_HISTORY {
                self.contradictions.pop_front();
            }
            self.contradictions.push_back(contradiction);
        }
        if let Some(inverse) = fact.relationship.inverted() {
            let inverse_fact = Fact {
                relationship: inverse,
                ..fact.clone()
            };
            self.insert_fact(inverse_fact);
        }
        self.insert_fact(fact);
    }

    /// The beliefs already held that `fact` cannot be true alongside.
    ///
    /// A different source, the same relationship, validity that overlaps,
    /// not retracted by the time `fact` was learned, and weights at least
    /// [`CONTRADICTION_GAP`] apart. The same source restating its own figure
    /// is a revision, not a contradiction: nobody is disagreeing with it.
    fn contradictions_with(&self, fact: &Fact) -> Vec<Contradiction> {
        let key = fact.relationship.key();
        let Some(versions) = self.facts.get(&key) else {
            return Vec::new();
        };
        versions
            .iter()
            .filter(|held| held.relationship.source != fact.relationship.source)
            .filter(|held| held.retracted_at.is_none_or(|at| at > fact.recorded_at))
            .filter(|held| {
                let both_from = held.valid_from.max(fact.valid_from);
                held.valid_to.is_none_or(|end| end > both_from)
                    && fact.valid_to.is_none_or(|end| end > both_from)
            })
            .filter_map(|held| {
                let gap = (held.relationship.weight - fact.relationship.weight).abs();
                (gap >= CONTRADICTION_GAP).then(|| Contradiction {
                    key: key.clone(),
                    held: Belief::of(held),
                    arrived: Belief::of(fact),
                    gap,
                    detected_at: held.recorded_at.max(fact.recorded_at),
                })
            })
            .collect()
    }

    /// Every contradiction knowable by `known_at`, oldest first.
    ///
    /// Filtered on the instant the platform first held both sides, so a
    /// replay of Monday does not see a conflict a source only created on
    /// Tuesday.
    pub fn contradictions_at(&self, known_at: Timestamp) -> Vec<&Contradiction> {
        self.contradictions
            .iter()
            .filter(|c| c.detected_at <= known_at)
            .collect()
    }

    fn insert_fact(&mut self, fact: Fact) {
        let key = fact.relationship.key();
        self.outgoing
            .entry(fact.relationship.from.clone())
            .or_default()
            .insert(key.clone());
        self.incoming
            .entry(fact.relationship.to.clone())
            .or_default()
            .insert(key.clone());
        self.facts.entry(key).or_default().push(fact);
    }

    /// Mark a fact as no longer believed, from `at`.
    ///
    /// The record is kept. A decision made while the fact was believed has to
    /// remain explicable, and deleting the fact would make it look arbitrary.
    pub fn retract(&mut self, key: &str, at: Timestamp) -> bool {
        let Some(versions) = self.facts.get_mut(key) else {
            return false;
        };
        let mut retracted = false;
        for fact in versions.iter_mut() {
            if fact.retracted_at.is_none() {
                fact.retracted_at = Some(at);
                retracted = true;
            }
        }
        retracted
    }

    /// Every fact believed at the given point in both time dimensions.
    pub fn facts_at(&self, valid_at: Timestamp, known_at: Timestamp) -> Vec<&Fact> {
        self.facts
            .values()
            .flat_map(|versions| versions.iter())
            .filter(|f| f.holds(valid_at, known_at))
            .collect()
    }

    /// Outgoing neighbours of `from`, believed at the given point.
    pub fn neighbours(
        &self,
        from: &str,
        kind: Option<RelationshipKind>,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Vec<&Fact> {
        let Some(keys) = self.outgoing.get(from) else {
            return Vec::new();
        };
        keys.iter()
            .filter_map(|key| self.facts.get(key))
            .flat_map(|versions| versions.iter())
            .filter(|f| f.holds(valid_at, known_at))
            .filter(|f| kind.is_none_or(|k| f.relationship.kind == k))
            .collect()
    }

    /// Incoming neighbours of `to`.
    pub fn predecessors(
        &self,
        to: &str,
        kind: Option<RelationshipKind>,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Vec<&Fact> {
        let Some(keys) = self.incoming.get(to) else {
            return Vec::new();
        };
        keys.iter()
            .filter_map(|key| self.facts.get(key))
            .flat_map(|versions| versions.iter())
            .filter(|f| f.holds(valid_at, known_at))
            .filter(|f| kind.is_none_or(|k| f.relationship.kind == k))
            .collect()
    }

    /// Shortest paths from `from` to `to`, up to `max_depth` edges.
    ///
    /// Breadth-first, so the first path found is the shortest. Bounded depth is
    /// not an optimisation: a six-hop explanation of why one stock moved is not
    /// an explanation.
    pub fn paths_between(
        &self,
        from: &str,
        to: &str,
        max_depth: usize,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Vec<Path> {
        if from == to || max_depth == 0 {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut queue: VecDeque<Path> = VecDeque::new();
        queue.push_back(Path {
            nodes: vec![from.to_string()],
            edges: Vec::new(),
            strength: 1.0,
        });

        while let Some(path) = queue.pop_front() {
            if path.edges.len() >= max_depth {
                continue;
            }
            let Some(current) = path.nodes.last().cloned() else {
                continue;
            };
            for fact in self.neighbours(&current, None, valid_at, known_at) {
                let next = &fact.relationship.to;
                if path.nodes.contains(next) {
                    continue; // no cycles
                }
                let mut extended = path.clone();
                extended.nodes.push(next.clone());
                extended.edges.push(fact.relationship.kind);
                extended.strength *= fact.relationship.weight * fact.confidence;

                if next == to {
                    found.push(extended);
                } else {
                    queue.push_back(extended);
                }
            }
        }

        found.sort_by(|a, b| {
            a.length()
                .cmp(&b.length())
                .then_with(|| {
                    b.strength
                        .partial_cmp(&a.strength)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.nodes.cmp(&b.nodes))
        });
        found
    }

    /// Nodes reachable from `from` within `max_depth`, with the hop count.
    pub fn reachable(
        &self,
        from: &str,
        max_depth: usize,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> BTreeMap<String, usize> {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((from.to_string(), 0));
        seen.insert(from.to_string(), 0);

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for fact in self.neighbours(&current, None, valid_at, known_at) {
                let next = fact.relationship.to.clone();
                if seen.contains_key(&next) {
                    continue;
                }
                seen.insert(next.clone(), depth + 1);
                queue.push_back((next, depth + 1));
            }
        }
        seen.remove(from);
        seen
    }

    /// Degree of a node at a point in time, counting both directions.
    pub fn degree(&self, id: &str, valid_at: Timestamp, known_at: Timestamp) -> usize {
        self.neighbours(id, None, valid_at, known_at).len()
            + self.predecessors(id, None, valid_at, known_at).len()
    }

    /// The most connected nodes, which are where a shock spreads furthest.
    pub fn most_connected(
        &self,
        limit: usize,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Vec<(&Node, usize)> {
        let mut ranked: Vec<(&Node, usize)> = self
            .nodes
            .values()
            .map(|n| (n, self.degree(&n.id, valid_at, known_at)))
            .filter(|(_, degree)| *degree > 0)
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
        ranked.truncate(limit);
        ranked
    }
}
