//! The bitemporal knowledge graph.
//!
//! Every fact records two time ranges: when it was true in the world, and when
//! the platform knew it. A query supplies both, and the graph answers with what
//! was believed at that moment — not with what is believed now.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::relationship::{Relationship, RelationshipKind};

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
        }
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
    pub fn new(relationship: Relationship, valid_from: Timestamp, recorded_at: Timestamp) -> Self {
        Self {
            relationship,
            valid_from,
            valid_to: None,
            recorded_at,
            retracted_at: None,
            confidence: 1.0,
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
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

    pub fn add_node(&mut self, node: Node) {
        self.nodes.insert(node.id.clone(), node);
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

    /// Record a fact, and its inverse where the relationship implies one.
    pub fn assert_fact(&mut self, fact: Fact) {
        if let Some(inverse) = fact.relationship.inverted() {
            let inverse_fact = Fact {
                relationship: inverse,
                ..fact.clone()
            };
            self.insert_fact(inverse_fact);
        }
        self.insert_fact(fact);
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
