//! The graph itself: identity, edges, and the refusal to hold a cycle.

use crate::definition::{FeatureDefinition, ValueKind};
use crate::state::MarketReads;
use qip_contracts::FeatureKey;
use qip_core::error::{Error, Result};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// A node's position in the graph.
///
/// Stable for the lifetime of the graph: registering more features never
/// renumbers the ones already there, so a consumer may hold one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeatureId(usize);

impl FeatureId {
    pub const fn index(self) -> usize {
        self.0
    }
}

/// One feature in the graph.
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) key: FeatureKey,
    /// `None` while the feature is only referenced as somebody's dependency
    /// and has not itself been registered.
    pub(crate) definition: Option<Box<dyn FeatureDefinition>>,
    pub(crate) dependencies: Vec<usize>,
    pub(crate) dependents: Vec<usize>,
    /// Subjects as strings, because dirty marking compares them against every
    /// arriving message and an allocation per comparison is not free.
    pub(crate) subjects: Vec<String>,
    pub(crate) reads: MarketReads,
    pub(crate) kind: Option<ValueKind>,
    pub(crate) time_sensitive: bool,
    /// How many registrations asked for this feature. Above one is the whole
    /// point of the graph.
    pub(crate) consumers: u32,
}

/// The registered features and the edges between them.
///
/// Edges run from a dependency to its dependents, which is the direction
/// invalidation travels. Evaluation walks the reverse.
#[derive(Debug, Default)]
pub struct FeatureGraph {
    nodes: Vec<Node>,
    /// Canonical key form to node index. [`FeatureKey::canonical`] is what
    /// makes two independently-built keys the same node, so twenty strategies
    /// asking for the same volatility get one computation between them.
    index: BTreeMap<String, usize>,
    order: Vec<usize>,
}

impl FeatureGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a feature, or record another consumer of one already present.
    ///
    /// The first definition of a key wins. A definition is a pure function of
    /// its key's parameters, so a second registration of the same key computes
    /// the same thing by construction; treating it as another consumer rather
    /// than as a replacement keeps evaluation counts honest.
    ///
    /// Dependencies may be registered in any order — a reference to a feature
    /// that does not exist yet creates a placeholder. That is what makes a
    /// cycle possible to express, and therefore worth refusing here: a cycle
    /// found at evaluation time is an unbounded loop on the hot path.
    pub fn register(&mut self, definition: Box<dyn FeatureDefinition>) -> Result<FeatureId> {
        let key = definition.key();
        let canonical = key.canonical();

        if let Some(&existing) = self.index.get(&canonical)
            && self.nodes[existing].definition.is_some()
        {
            self.nodes[existing].consumers += 1;
            return Ok(FeatureId(existing));
        }

        let id = self.ensure(&key);
        let declared = definition.dependencies();
        let mut dependencies = Vec::with_capacity(declared.len());
        for dependency in &declared {
            dependencies.push(self.ensure(dependency));
        }

        // Refuse before mutating anything: a half-registered cycle would leave
        // the graph in a state no later call could repair.
        for &dependency in &dependencies {
            if dependency == id {
                return Err(Error::invalid(format!(
                    "feature cycle: {canonical} -> {canonical}"
                )));
            }
            if let Some(path) = self.path_to(dependency, id) {
                let mut named = vec![canonical.clone()];
                named.extend(path.into_iter().map(|node| self.nodes[node].key.canonical()));
                return Err(Error::invalid(format!(
                    "feature cycle: {}",
                    named.join(" -> ")
                )));
            }
        }

        for &dependency in &dependencies {
            self.nodes[dependency].dependents.push(id);
            self.nodes[dependency].dependents.sort_unstable();
            self.nodes[dependency].dependents.dedup();
        }

        let node = &mut self.nodes[id];
        node.subjects = definition
            .subjects()
            .iter()
            .map(|subject| subject.as_str().to_string())
            .collect();
        node.reads = definition.reads();
        node.kind = Some(definition.value_kind());
        node.time_sensitive = definition.time_sensitive();
        node.dependencies = dependencies;
        node.consumers += 1;
        node.definition = Some(definition);

        self.reorder()?;
        Ok(FeatureId(id))
    }

    /// The node for a key, creating an unresolved placeholder if needed.
    fn ensure(&mut self, key: &FeatureKey) -> usize {
        let canonical = key.canonical();
        if let Some(&existing) = self.index.get(&canonical) {
            return existing;
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            key: key.clone(),
            definition: None,
            dependencies: Vec::new(),
            dependents: Vec::new(),
            subjects: Vec::new(),
            reads: MarketReads::NONE,
            kind: None,
            time_sensitive: false,
            consumers: 0,
        });
        self.index.insert(canonical, id);
        self.order.push(id);
        id
    }

    /// A dependency path from `from` down to `target`, if one exists.
    ///
    /// Walks the dependency direction over a graph that is acyclic by
    /// induction, so the walk terminates.
    fn path_to(&self, from: usize, target: usize) -> Option<Vec<usize>> {
        let mut parent: BTreeMap<usize, usize> = BTreeMap::new();
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = vec![from];
        seen[from] = true;

        while let Some(current) = stack.pop() {
            if current == target {
                let mut path = vec![current];
                let mut cursor = current;
                while let Some(&previous) = parent.get(&cursor) {
                    path.push(previous);
                    cursor = previous;
                }
                path.reverse();
                return Some(path);
            }
            for &next in &self.nodes[current].dependencies {
                if !seen[next] {
                    seen[next] = true;
                    parent.insert(next, current);
                    stack.push(next);
                }
            }
        }
        None
    }

    /// Recompute the evaluation order.
    ///
    /// Kahn's algorithm, taking the lowest ready index first, so the order is
    /// a function of the graph rather than of hash iteration or insertion
    /// timing. Two cells built the same way evaluate in the same sequence.
    fn reorder(&mut self) -> Result<()> {
        let mut remaining: Vec<usize> = self.nodes.iter().map(|n| n.dependencies.len()).collect();
        let mut ready: BinaryHeap<Reverse<usize>> = remaining
            .iter()
            .enumerate()
            .filter(|&(_, &count)| count == 0)
            .map(|(id, _)| Reverse(id))
            .collect();

        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(Reverse(id)) = ready.pop() {
            order.push(id);
            for &dependent in &self.nodes[id].dependents {
                remaining[dependent] -= 1;
                if remaining[dependent] == 0 {
                    ready.push(Reverse(dependent));
                }
            }
        }

        if order.len() != self.nodes.len() {
            // Registration refuses cycles, so reaching here means the edge set
            // and the refusal disagree — report it rather than evaluate a
            // graph whose order is a lie.
            return Err(Error::invalid(
                "feature graph does not admit a topological order",
            ));
        }
        self.order = order;
        Ok(())
    }

    /// Number of nodes, placeholders included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether the key names a node at all, defined or merely referenced.
    pub fn contains(&self, key: &FeatureKey) -> bool {
        self.index.contains_key(&key.canonical())
    }

    /// Whether the key names a node that has a definition behind it.
    pub fn is_defined(&self, key: &FeatureKey) -> bool {
        self.id_of(key)
            .is_some_and(|id| self.nodes[id.index()].definition.is_some())
    }

    pub fn id_of(&self, key: &FeatureKey) -> Option<FeatureId> {
        self.index.get(&key.canonical()).copied().map(FeatureId)
    }

    /// How many registrations asked for this feature.
    pub fn consumers(&self, key: &FeatureKey) -> u32 {
        self.id_of(key)
            .map_or(0, |id| self.nodes[id.index()].consumers)
    }

    /// The kind of value the feature's definition declares.
    pub fn value_kind(&self, key: &FeatureKey) -> Option<ValueKind> {
        self.id_of(key).and_then(|id| self.nodes[id.index()].kind)
    }

    /// Every key, in evaluation order.
    pub fn keys(&self) -> Vec<&FeatureKey> {
        self.order.iter().map(|&id| &self.nodes[id].key).collect()
    }

    /// Keys referenced as a dependency but never registered.
    ///
    /// These evaluate to undefined forever. A cell should refuse to go live
    /// with any, which is why they are reported rather than tolerated
    /// silently.
    pub fn unresolved(&self) -> Vec<&FeatureKey> {
        self.nodes
            .iter()
            .filter(|node| node.definition.is_none())
            .map(|node| &node.key)
            .collect()
    }

    /// Refuse the graph if anything it references is missing.
    pub fn require_complete(&self) -> Result<()> {
        let missing = self.unresolved();
        if missing.is_empty() {
            return Ok(());
        }
        let named: Vec<String> = missing.iter().map(|key| key.canonical()).collect();
        Err(Error::not_found(format!(
            "feature graph references undefined features: {}",
            named.join(", ")
        )))
    }

    /// The keys a feature is computed from, in declaration order.
    pub fn dependencies_of(&self, key: &FeatureKey) -> Vec<&FeatureKey> {
        self.id_of(key)
            .map(|id| {
                self.nodes[id.index()]
                    .dependencies
                    .iter()
                    .map(|&dependency| &self.nodes[dependency].key)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The keys computed from a feature.
    pub fn dependents_of(&self, key: &FeatureKey) -> Vec<&FeatureKey> {
        self.id_of(key)
            .map(|id| {
                self.nodes[id.index()]
                    .dependents
                    .iter()
                    .map(|&dependent| &self.nodes[dependent].key)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn order(&self) -> &[usize] {
        &self.order
    }

    pub(crate) fn node(&self, id: usize) -> &Node {
        &self.nodes[id]
    }

    /// Mark `id` and everything computed from it, transitively.
    pub(crate) fn mark_transitively(&self, id: usize, dirty: &mut [bool]) {
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            if dirty[current] {
                continue;
            }
            dirty[current] = true;
            stack.extend(self.nodes[current].dependents.iter().copied());
        }
    }
}
