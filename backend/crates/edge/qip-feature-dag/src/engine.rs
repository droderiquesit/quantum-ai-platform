//! Incremental evaluation: dirty marking on ingest, computation on demand.

use crate::definition::{FeatureContext, FeatureDefinition, ValueKind};
use crate::graph::{FeatureGraph, FeatureId};
use crate::state::{MarketReads, MarketState};
use qip_contracts::{FeatureKey, FeatureValue, FeatureVector, MarketMessage, Revision};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use std::collections::BTreeMap;

/// What the engine remembers about one node between passes.
#[derive(Clone, Copy, Debug)]
struct Slot {
    value: FeatureValue,
    revision: Revision,
    /// The instant this value was computed for. Compared against the
    /// evaluation instant so a feature *of* time recomputes when time moves
    /// and everything else does not.
    computed_at: Option<Timestamp>,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            value: FeatureValue::Undefined,
            revision: Revision::default(),
            computed_at: None,
        }
    }
}

/// The feature DAG in operation.
///
/// One computation per changed feature, shared by every consumer that asked
/// for it. A message marks the nodes it can actually affect dirty, dirtiness
/// travels to their dependents, and an evaluation runs exactly the dirty ones
/// in topological order. Everything else is served from the value it already
/// had, at the revision it already had, which is what lets a consumer tell a
/// reused value from a fresh one without recomputing it to find out.
#[derive(Debug)]
pub struct FeatureEngine {
    graph: FeatureGraph,
    state: MarketState,
    slots: Vec<Slot>,
    dirty: Vec<bool>,
    /// Nodes that read each instrument, so a message is matched against the
    /// features that care about it rather than against all of them.
    by_subject: BTreeMap<String, Vec<usize>>,
    /// Whether each instrument was stale at the last evaluation.
    stale: BTreeMap<String, bool>,
    max_staleness: Duration,
    computations: u64,
    evaluations: u64,
    /// Reused dependency buffer: the hot path allocates nothing per node.
    scratch: Vec<FeatureValue>,
}

impl Default for FeatureEngine {
    fn default() -> Self {
        Self::new(MarketState::default(), crate::state::DEFAULT_MAX_STALENESS)
    }
}

impl FeatureEngine {
    /// An engine over a given market state and staleness tolerance.
    pub fn new(state: MarketState, max_staleness: Duration) -> Self {
        Self {
            graph: FeatureGraph::new(),
            state,
            slots: Vec::new(),
            dirty: Vec::new(),
            by_subject: BTreeMap::new(),
            stale: BTreeMap::new(),
            max_staleness,
            computations: 0,
            evaluations: 0,
            scratch: Vec::new(),
        }
    }

    /// Register a feature, or record another consumer of one already there.
    ///
    /// Two registrations of the same [`FeatureKey`] are one node. That is the
    /// deduplication: twenty strategies wanting a twenty-period realised
    /// volatility on the same instrument produce one node and one computation
    /// per change, not twenty.
    pub fn register(&mut self, definition: Box<dyn FeatureDefinition>) -> Result<FeatureId> {
        let id = self.graph.register(definition)?;
        self.slots.resize_with(self.graph.len(), Slot::default);
        self.dirty.resize(self.graph.len(), true);

        for subject in self.graph.node(id.index()).subjects.clone() {
            let nodes = self.by_subject.entry(subject).or_default();
            if !nodes.contains(&id.index()) {
                nodes.push(id.index());
            }
        }
        Ok(id)
    }

    /// Fold a message into the market state and mark what it can affect.
    ///
    /// A message dirties a node only when it touches an instrument the node
    /// reads *and* changes an aspect the node reads. A print cannot move the
    /// touch, so it cannot dirty a spread; a quote on one instrument cannot
    /// dirty a feature of another.
    pub fn ingest(&mut self, message: &MarketMessage) -> Result<()> {
        self.state.apply(message)?;
        let reads = MarketReads::of_message(&message.body);
        if reads.is_empty() {
            return Ok(());
        }
        let Some(candidates) = self.by_subject.get(message.object_id.as_str()) else {
            return Ok(());
        };
        let affected: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&id| self.graph.node(id).reads.intersects(reads))
            .collect();
        for id in affected {
            self.graph.mark_transitively(id, &mut self.dirty);
        }
        Ok(())
    }

    /// Compute every dirty feature and return one consistent view.
    ///
    /// Every value in the returned vector comes from this pass: a node is
    /// either recomputed here or served from a value that nothing since its
    /// last computation could have changed. A strategy reading the vector is
    /// therefore reasoning about one instant, not a mixture of two.
    pub fn evaluate(&mut self, as_of: Timestamp) -> Result<FeatureVector> {
        self.evaluations += 1;
        self.mark_staleness_changes(as_of);

        for position in 0..self.graph.order().len() {
            let id = self.graph.order()[position];
            let node = self.graph.node(id);
            let stale_clock = node.time_sensitive && self.slots[id].computed_at != Some(as_of);
            if !self.dirty[id] && !stale_clock && self.slots[id].computed_at.is_some() {
                continue;
            }

            self.scratch.clear();
            for &dependency in &node.dependencies {
                self.scratch.push(self.slots[dependency].value);
            }

            let declared = node.kind;
            let value = match &node.definition {
                Some(definition) => {
                    let ctx =
                        FeatureContext::new(as_of, &self.state, &self.scratch, self.max_staleness);
                    definition.compute(&ctx)?
                }
                // A feature nobody defined has no value, and saying so is the
                // only honest answer. `require_complete` is how a cell refuses
                // to run in this state.
                None => FeatureValue::Undefined,
            };
            let computed = node.definition.is_some();

            if let (Some(declared), Some(actual)) = (declared, ValueKind::of(&value))
                && declared != actual
            {
                return Err(Error::schema(format!(
                    "feature {} declared {} but computed {}",
                    self.graph.node(id).key.canonical(),
                    declared.as_str(),
                    actual.as_str()
                )));
            }

            let slot = &mut self.slots[id];
            slot.value = value;
            slot.revision = slot.revision.next();
            slot.computed_at = Some(as_of);
            self.dirty[id] = false;
            if computed {
                self.computations += 1;
            }
        }

        let mut vector = FeatureVector::new(as_of);
        for &id in self.graph.order() {
            let slot = self.slots[id];
            vector.insert(self.graph.node(id).key.clone(), slot.value, slot.revision);
        }
        Ok(vector)
    }

    /// Dirty everything that reads an instrument which has just crossed the
    /// staleness boundary.
    ///
    /// Staleness is the one input that changes without a message: a feed that
    /// stops delivering makes its features undefined by saying nothing at all.
    /// Without this, a cached value would outlive the market it came from, and
    /// an incremental evaluation would stop agreeing with a full one.
    fn mark_staleness_changes(&mut self, as_of: Timestamp) {
        for (subject, nodes) in &self.by_subject {
            let stale = self
                .state
                .instrument_named(subject)
                .is_none_or(|state| state.is_stale(as_of, self.max_staleness));
            if self.stale.get(subject).copied() == Some(stale) {
                continue;
            }
            self.stale.insert(subject.clone(), stale);
            for &id in nodes {
                self.graph.mark_transitively(id, &mut self.dirty);
            }
        }
    }

    /// How many definitions have been run since the counter was last reset.
    ///
    /// The measurement the whole design exists to keep small. It is exposed
    /// rather than logged because the sharing claim is only worth making if a
    /// test can hold it to account.
    pub const fn computations(&self) -> u64 {
        self.computations
    }

    /// How many evaluation passes have run.
    pub const fn evaluations(&self) -> u64 {
        self.evaluations
    }

    /// Reset the counters, leaving values and dirtiness alone.
    pub fn reset_counters(&mut self) {
        self.computations = 0;
        self.evaluations = 0;
    }

    /// How many nodes are waiting to be recomputed.
    pub fn dirty_count(&self) -> usize {
        self.dirty.iter().filter(|d| **d).count()
    }

    /// Whether a feature will be recomputed on the next pass.
    pub fn is_dirty(&self, key: &FeatureKey) -> bool {
        self.graph
            .id_of(key)
            .is_some_and(|id| self.dirty[id.index()])
    }

    /// The value held for a feature, without evaluating.
    pub fn value(&self, key: &FeatureKey) -> Option<FeatureValue> {
        self.graph.id_of(key).map(|id| self.slots[id.index()].value)
    }

    /// The revision held for a feature, without evaluating.
    pub fn revision(&self, key: &FeatureKey) -> Option<Revision> {
        self.graph
            .id_of(key)
            .map(|id| self.slots[id.index()].revision)
    }

    pub const fn graph(&self) -> &FeatureGraph {
        &self.graph
    }

    pub const fn state(&self) -> &MarketState {
        &self.state
    }

    /// How long an instrument may go unrefreshed before its features read as
    /// undefined.
    pub const fn max_staleness(&self) -> Duration {
        self.max_staleness
    }
}
