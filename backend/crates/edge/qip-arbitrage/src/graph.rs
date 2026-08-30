//! The arbitrage graph: what can be turned into what, where, and at what cost.
//!
//! A node is one instrument held at one venue. An edge is a conversion — a
//! trade, a transfer, or the assembly of a synthetic — carrying the rate the
//! platform *believes* and the proportional cost of taking it. Believing is the
//! operative word: the rate on an edge is indicative, good enough to decide
//! what is worth examining and never good enough to decide what is worth doing.
//! The book decides that, in [`crate::pricing`].
//!
//! Three shapes of opportunity fall out of one structure rather than needing
//! three code paths:
//!
//! * **Cross-venue** — buy an instrument at one venue, move it, sell it at
//!   another. A four-edge cycle through two [`EdgeKind::Transfer`] edges.
//! * **Triangular** — A to B to C to A at one venue. A three-edge cycle of
//!   [`EdgeKind::Trade`] edges, the classic FX and crypto shape.
//! * **Cross-instrument** — a synthetic against the components that replicate
//!   it. A two-edge cycle through one [`EdgeKind::Synthetic`] edge. A basket
//!   against its constituents is the many-component case; a future against
//!   spot is the same edge with one component and the carry expressed as the
//!   conversion's cost, which is then charged as funding on the net edge.
//!
//! Every one of them is a cycle whose product exceeds one, which is why the
//! search in [`crate::search`] needs to know about only one of them.

use crate::arith::mul;
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One instrument held at one venue.
///
/// The venue is part of the identity. The same instrument at two venues is two
/// different things to an arbitrageur, and collapsing them is how a cross-venue
/// opportunity becomes invisible.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Node {
    pub object: ObjectId,
    pub venue: VenueId,
}

impl Node {
    pub fn new(object: ObjectId, venue: VenueId) -> Self {
        Self { object, venue }
    }

    /// A stable label for logs and rejection messages.
    pub fn label(&self) -> String {
        format!("{}@{}", self.object.as_str(), self.venue.as_str())
    }
}

/// One constituent of a synthetic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyntheticComponent {
    pub object: ObjectId,
    pub venue: VenueId,
    /// Units of this component per unit of the synthetic. Always positive; a
    /// short constituent is expressed by its `side`, not by a negative weight.
    pub units_per_unit: Decimal,
    /// The side of this component's book consumed when the synthetic is
    /// *unwound*. Assembling it consumes the opposite side.
    pub unwind_side: BookSide,
}

/// What kind of conversion an edge is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EdgeKind {
    /// The same instrument moved between venues.
    ///
    /// No book and no price risk; the cost is the transfer and the risk is the
    /// time it takes, which is why the planner turns one of these into held
    /// inventory rather than into an order.
    Transfer,
    /// One instrument traded for another against a book at one venue.
    ///
    /// `market` names the book, not either of the instruments on it. A venue
    /// quoting EUR against both USD and GBP has two books and no single book
    /// for EUR, so keying depth by an instrument would collapse them.
    ///
    /// `side` is the side of that book which gets consumed, which also fixes
    /// which end of the edge is the base: consuming bids sells `from.object`,
    /// consuming offers buys `to.object`. Swept quantities are always in the
    /// market's base units.
    Trade { market: ObjectId, side: BookSide },
    /// A synthetic assembled from, or unwound into, its components.
    Synthetic {
        /// Which endpoint of the edge is the synthetic itself. The other is
        /// what pays for it.
        synthetic_object: ObjectId,
        components: Vec<SyntheticComponent>,
    },
}

impl EdgeKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Trade { .. } => "trade",
            Self::Synthetic { .. } => "synthetic",
        }
    }
}

/// A convertibility: what one unit of `from` becomes at `to`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConversionEdge {
    pub from: Node,
    pub to: Node,
    /// Units of `to` obtained per unit of `from`, before cost.
    ///
    /// Indicative. It is what the log-space search reads and what the exact
    /// confirmation re-checks; it is never what a fill is priced at.
    pub indicative_rate: Decimal,
    /// Proportional cost of taking the conversion, as a fraction in `[0, 1)`.
    ///
    /// Venue fees, transfer fees, amortised gas. Not the spread and not the
    /// slippage: those are properties of the book at a size, and this is the
    /// part that is charged whatever the size.
    pub cost_fraction: Decimal,
    pub kind: EdgeKind,
    /// When the rate was observed. Feeds the uncertainty haircut.
    pub observed_at: Timestamp,
    /// How many observations back the rate.
    pub observations: u32,
}

impl ConversionEdge {
    /// The rate after the conversion's own cost. What the search searches on.
    pub fn effective_rate(&self) -> Result<Decimal> {
        mul(
            self.indicative_rate,
            Decimal::ONE - self.cost_fraction,
            "effective rate",
        )
    }

    pub fn label(&self) -> String {
        format!(
            "{} -> {} ({})",
            self.from.label(),
            self.to.label(),
            self.kind.as_str()
        )
    }
}

/// What is known about a venue, beyond its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueFacts {
    pub class: VenueClass,
    pub status: VenueStatus,
}

impl VenueFacts {
    pub const fn new(class: VenueClass, status: VenueStatus) -> Self {
        Self { class, status }
    }
}

/// The shape of an opportunity, read off the cycle rather than declared.
///
/// Classification is derived so a caller cannot mislabel a path: the structure
/// of the edges is the only evidence, and it is the same evidence the reader of
/// a rejection message needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathKind {
    /// One instrument, two or more venues.
    CrossVenue,
    /// Three or more instruments at a single venue.
    Triangular,
    /// A synthetic against the components that replicate it.
    CrossInstrument,
    /// Instruments and venues both, with no synthetic leg.
    Mixed,
}

impl PathKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::CrossVenue => "cross_venue",
            Self::Triangular => "triangular",
            Self::CrossInstrument => "cross_instrument",
            Self::Mixed => "mixed",
        }
    }
}

/// Nodes, edges, and what is known about the venues they sit on.
#[derive(Clone, Debug, Default)]
pub struct ArbitrageGraph {
    nodes: Vec<Node>,
    index: BTreeMap<Node, usize>,
    edges: Vec<ConversionEdge>,
    outgoing: Vec<Vec<usize>>,
    venues: BTreeMap<VenueId, VenueFacts>,
}

impl ArbitrageGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what a venue is and whether it is open.
    ///
    /// A venue with no facts recorded is treated as unusable rather than as
    /// open: an unknown venue class means the settlement assumptions are
    /// unknown, and guessing them is how a chain leg gets planned as if it
    /// could not revert.
    pub fn register_venue(&mut self, venue: VenueId, facts: VenueFacts) {
        self.venues.insert(venue, facts);
    }

    pub fn venue_facts(&self, venue: &VenueId) -> Option<VenueFacts> {
        self.venues.get(venue).copied()
    }

    /// Whether orders may be sent to both ends of an edge.
    pub fn edge_is_tradable(&self, edge: &ConversionEdge) -> bool {
        [&edge.from.venue, &edge.to.venue].into_iter().all(|venue| {
            self.venue_facts(venue)
                .is_some_and(|facts| facts.status.accepts_orders())
        })
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn edges(&self) -> &[ConversionEdge] {
        &self.edges
    }

    pub fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index)
    }

    pub fn edge(&self, index: usize) -> Option<&ConversionEdge> {
        self.edges.get(index)
    }

    pub fn node_index(&self, node: &Node) -> Option<usize> {
        self.index.get(node).copied()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Edge indices leaving a node.
    pub fn outgoing(&self, node: usize) -> &[usize] {
        self.outgoing.get(node).map_or(&[], Vec::as_slice)
    }

    /// Add a conversion, registering its endpoints.
    ///
    /// Validation is structural, not economic: an edge whose kind contradicts
    /// its endpoints — a trade between two venues, a transfer that changes the
    /// instrument — would be priced against the wrong book and produce a
    /// confident wrong answer, so it is refused here rather than discovered
    /// three stages later.
    pub fn add_edge(&mut self, edge: ConversionEdge) -> Result<usize> {
        Self::validate(&edge)?;
        let from = self.intern(edge.from.clone());
        let to = self.intern(edge.to.clone());
        if from == to {
            return Err(Error::invalid(format!(
                "conversion {} starts and ends at the same node",
                edge.label()
            )));
        }
        let index = self.edges.len();
        self.edges.push(edge);
        self.outgoing[from].push(index);
        Ok(index)
    }

    /// Add a trade against a book at one venue.
    #[allow(clippy::too_many_arguments)]
    pub fn add_trade(
        &mut self,
        from: Node,
        to: Node,
        indicative_rate: Decimal,
        cost_fraction: Decimal,
        market: ObjectId,
        side: BookSide,
        observed_at: Timestamp,
        observations: u32,
    ) -> Result<usize> {
        self.add_edge(ConversionEdge {
            from,
            to,
            indicative_rate,
            cost_fraction,
            kind: EdgeKind::Trade { market, side },
            observed_at,
            observations,
        })
    }

    /// Add a movement of one instrument between two venues.
    pub fn add_transfer(
        &mut self,
        object: ObjectId,
        from_venue: VenueId,
        to_venue: VenueId,
        cost_fraction: Decimal,
        observed_at: Timestamp,
        observations: u32,
    ) -> Result<usize> {
        self.add_edge(ConversionEdge {
            from: Node::new(object.clone(), from_venue),
            to: Node::new(object, to_venue),
            indicative_rate: Decimal::ONE,
            cost_fraction,
            kind: EdgeKind::Transfer,
            observed_at,
            observations,
        })
    }

    /// Add the assembly or unwinding of a synthetic.
    #[allow(clippy::too_many_arguments)]
    pub fn add_synthetic(
        &mut self,
        from: Node,
        to: Node,
        indicative_rate: Decimal,
        cost_fraction: Decimal,
        synthetic_object: ObjectId,
        components: Vec<SyntheticComponent>,
        observed_at: Timestamp,
        observations: u32,
    ) -> Result<usize> {
        self.add_edge(ConversionEdge {
            from,
            to,
            indicative_rate,
            cost_fraction,
            kind: EdgeKind::Synthetic {
                synthetic_object,
                components,
            },
            observed_at,
            observations,
        })
    }

    /// Name the shape of a cycle from the edges it uses.
    pub fn classify(&self, edges: &[usize]) -> PathKind {
        let mut venues: Vec<&VenueId> = Vec::new();
        let mut objects: Vec<&ObjectId> = Vec::new();
        let mut has_synthetic = false;
        for index in edges {
            let Some(edge) = self.edges.get(*index) else {
                continue;
            };
            if matches!(edge.kind, EdgeKind::Synthetic { .. }) {
                has_synthetic = true;
            }
            for node in [&edge.from, &edge.to] {
                if !venues.contains(&&node.venue) {
                    venues.push(&node.venue);
                }
                if !objects.contains(&&node.object) {
                    objects.push(&node.object);
                }
            }
        }
        match (has_synthetic, venues.len(), objects.len()) {
            (true, _, _) => PathKind::CrossInstrument,
            (false, 1, _) => PathKind::Triangular,
            (false, _, 2) => PathKind::CrossVenue,
            _ => PathKind::Mixed,
        }
    }

    fn intern(&mut self, node: Node) -> usize {
        if let Some(existing) = self.index.get(&node) {
            return *existing;
        }
        let index = self.nodes.len();
        self.index.insert(node.clone(), index);
        self.nodes.push(node);
        self.outgoing.push(Vec::new());
        index
    }

    fn validate(edge: &ConversionEdge) -> Result<()> {
        if edge.indicative_rate <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "conversion {} has a non-positive rate {}",
                edge.label(),
                edge.indicative_rate
            )));
        }
        if edge.cost_fraction < Decimal::ZERO || edge.cost_fraction >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "conversion {} has a cost fraction of {}, which is outside [0, 1)",
                edge.label(),
                edge.cost_fraction
            )));
        }
        match &edge.kind {
            EdgeKind::Transfer => {
                if edge.from.object != edge.to.object {
                    return Err(Error::invalid(format!(
                        "transfer {} changes the instrument; that is a trade",
                        edge.label()
                    )));
                }
                if edge.from.venue == edge.to.venue {
                    return Err(Error::invalid(format!(
                        "transfer {} does not leave the venue",
                        edge.label()
                    )));
                }
            }
            EdgeKind::Trade { market, side } => {
                if edge.from.venue != edge.to.venue {
                    return Err(Error::invalid(format!(
                        "trade {} spans two venues; that is a trade and a transfer",
                        edge.label()
                    )));
                }
                if edge.from.object == edge.to.object {
                    return Err(Error::invalid(format!(
                        "trade {} on market {} does not change the instrument held; that is a transfer",
                        edge.label(),
                        market.as_str()
                    )));
                }
                let _ = side;
            }
            EdgeKind::Synthetic {
                synthetic_object,
                components,
            } => {
                if components.is_empty() {
                    return Err(Error::invalid(format!(
                        "synthetic {} has no components to price it from",
                        edge.label()
                    )));
                }
                if synthetic_object != &edge.from.object && synthetic_object != &edge.to.object {
                    return Err(Error::invalid(format!(
                        "synthetic {} names {} as the synthetic, which is neither end of the edge",
                        edge.label(),
                        synthetic_object.as_str()
                    )));
                }
                if let Some(bad) = components
                    .iter()
                    .find(|c| c.units_per_unit <= Decimal::ZERO)
                {
                    return Err(Error::invalid(format!(
                        "synthetic {} gives component {} a weight of {}; a short constituent is expressed by its side",
                        edge.label(),
                        bad.object.as_str(),
                        bad.units_per_unit
                    )));
                }
            }
        }
        Ok(())
    }
}
