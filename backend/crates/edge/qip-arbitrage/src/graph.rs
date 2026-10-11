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
//!
//! # A node is more than an instrument at a venue (R12-MESH-5053)
//!
//! A node was `(object, venue)` until 2026-10-11, and three distinctions an
//! arbitrageur pays for were invisible to it: a wrapped or tokenized
//! representation of an asset is not the asset (redeeming it costs and takes
//! time), a position still settling is not a settled one (it cannot be sold
//! again until it lands, and it can fail), and the same venue's book reached
//! from two regions is two different latencies and two different legal
//! regimes. A graph that collapsed any of them could join the two sides with
//! nothing between them, and the search would read the gap as free money.
//! [`Node`] now carries [`Representation`], [`SettlementStage`] and a region,
//! all three are part of its identity and its ordering, and
//! [`ArbitrageGraph::add_edge`] refuses an edge that joins two nodes
//! differing only in settlement stage or region at no cost — moving between
//! them is never free, and an edge that says it is would be a cycle with a
//! hole in it.
//!
//! # An edge carries more than its fee (R12-MESH-5055 to -5057)
//!
//! [`EdgeCosts`] puts on every conversion the costs that are a property of
//! *that* conversion rather than of the whole path: a tax or duty estimate, a
//! bridge, transfer or logistics charge, a settlement-risk expected loss, a
//! probability of fill, and a measured borrow or funding rate. Each one
//! reaches the search through [`ConversionEdge::effective_rate`] and the net
//! edge through its own named deduction in [`crate::netedge`], so a cycle
//! that is only profitable when a duty is ignored or a fill is assumed is
//! refused rather than ranked.

use crate::arith::mul;
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How an asset is held: itself, or a claim on it.
///
/// A wrapped coin, a tokenized share or a depositary receipt tracks its
/// underlying and is not it: converting back costs a fee and a wait, and the
/// wrapper can fail when the underlying does not. Ordered, because it is part
/// of a node's ordering and a replay that reorders nodes is not a replay.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
    /// The asset itself, as the venue holds it.
    #[default]
    Native,
    /// A wrapper redeemable for the asset by its issuer or contract.
    Wrapped,
    /// An on-chain token representing an off-chain asset.
    Tokenized,
    /// A receipt or depositary claim held through a custodian.
    Receipt,
}

impl Representation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Wrapped => "wrapped",
            Self::Tokenized => "tokenized",
            Self::Receipt => "receipt",
        }
    }
}

/// How far a holding has got through settlement.
///
/// A holding that is still settling cannot be sold again, can fail, and is
/// exposed to its counterparty or chain until it lands; a graph that read it
/// as settled would let a cycle re-use inventory that does not yet exist.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SettlementStage {
    /// Delivered and final: free to trade again.
    #[default]
    Settled,
    /// Traded, awaiting the venue's or the chain's settlement.
    Pending,
    /// Moving between venues, regions or representations.
    InTransit,
}

impl SettlementStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::Pending => "pending",
            Self::InTransit => "in_transit",
        }
    }
}

/// One instrument, in one representation and settlement stage, held at one
/// venue in one region.
///
/// Every field is part of the identity. The same instrument at two venues is
/// two different things to an arbitrageur, and so is the same instrument
/// wrapped and unwrapped, settled and settling, or reached in two regions:
/// collapsing any pair is how a cost becomes invisible and a cycle with a
/// hole in it reads as profit (R12-MESH-5053).
///
/// `region` is `None` when the graph's builder states none — the node is the
/// venue's own book wherever that venue is — and `Some` when the same venue
/// is reached from more than one region. `None` and `Some` are different
/// nodes, on purpose: a builder that names regions for some nodes and not
/// others has two graphs, and joining them is an edge somebody must state.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Node {
    pub object: ObjectId,
    pub venue: VenueId,
    #[serde(default)]
    pub representation: Representation,
    #[serde(default)]
    pub settlement: SettlementStage,
    #[serde(default)]
    pub region: Option<String>,
}

impl Node {
    /// A native, settled holding at a venue, with no region stated.
    pub fn new(object: ObjectId, venue: VenueId) -> Self {
        Self {
            object,
            venue,
            representation: Representation::Native,
            settlement: SettlementStage::Settled,
            region: None,
        }
    }

    /// The same holding in another representation.
    pub fn with_representation(mut self, representation: Representation) -> Self {
        self.representation = representation;
        self
    }

    /// The same holding at another settlement stage.
    pub fn with_settlement(mut self, settlement: SettlementStage) -> Self {
        self.settlement = settlement;
        self
    }

    /// The same holding reached in a named region.
    pub fn in_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Whether `self` and `other` are the same asset, venue and
    /// representation and differ only in settlement stage or region — the
    /// pair no edge may join for nothing.
    pub fn differs_only_in_settlement_or_region(&self, other: &Self) -> bool {
        self.object == other.object
            && self.venue == other.venue
            && self.representation == other.representation
            && (self.settlement != other.settlement || self.region != other.region)
    }

    /// A stable label for logs and rejection messages.
    ///
    /// The default representation and stage are left off so the common case
    /// reads as it always did; anything else is spelled out, because a
    /// rejection naming `ETH@CX` when the node was a pending wrapped holding
    /// in another region would send the reader to the wrong book.
    pub fn label(&self) -> String {
        let mut qualifiers: Vec<&str> = Vec::new();
        if self.representation != Representation::Native {
            qualifiers.push(self.representation.as_str());
        }
        if self.settlement != SettlementStage::Settled {
            qualifiers.push(self.settlement.as_str());
        }
        if let Some(region) = &self.region {
            qualifiers.push(region.as_str());
        }
        if qualifiers.is_empty() {
            format!("{}@{}", self.object.as_str(), self.venue.as_str())
        } else {
            format!(
                "{}@{}[{}]",
                self.object.as_str(),
                self.venue.as_str(),
                qualifiers.join(",")
            )
        }
    }
}

/// The costs that belong to one conversion rather than to the whole path
/// (R12-MESH-5055 to -5057).
///
/// Every fraction is of the conversion's notional, exact because it is
/// charged against money, and refused outside its range by
/// [`EdgeCosts::validate`] rather than clamped into it. The default is the
/// conversion that carries none of these — no duty, no logistics, no
/// settlement risk, a certain fill, no borrow — which is what every edge was
/// before the field existed; a builder that measures any of them states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeCosts {
    /// Tax or duty levied on the conversion, as a fraction in `[0, 1)`: a
    /// stamp duty, a transaction tax, a customs duty on a physical leg.
    pub tax_duty_fraction: Decimal,
    /// Transfer, bridge or logistics charge beyond the venue fee, in `[0, 1)`:
    /// a bridge's toll, a withdrawal fee, freight and insurance.
    pub logistics_fraction: Decimal,
    /// Expected loss from the conversion failing to settle, in `[0, 1)`:
    /// probability of failure times loss given failure.
    pub settlement_risk_fraction: Decimal,
    /// Probability the conversion fills at all, in `(0, 1]`.
    pub fill_probability: Decimal,
    /// Measured annualised borrow or funding rate for holding this leg, as a
    /// fraction, non-negative. Charged over the holding period on top of the
    /// path-wide funding assumption, because a borrow is a property of the
    /// instrument borrowed and not of the path.
    pub borrow_rate_annual: Decimal,
}

impl Default for EdgeCosts {
    fn default() -> Self {
        Self {
            tax_duty_fraction: Decimal::ZERO,
            logistics_fraction: Decimal::ZERO,
            settlement_risk_fraction: Decimal::ZERO,
            fill_probability: Decimal::ONE,
            borrow_rate_annual: Decimal::ZERO,
        }
    }
}

impl EdgeCosts {
    /// Whether every cost is absent and the fill is certain.
    pub fn is_free(&self) -> bool {
        self.tax_duty_fraction.is_zero()
            && self.logistics_fraction.is_zero()
            && self.settlement_risk_fraction.is_zero()
            && self.fill_probability == Decimal::ONE
            && self.borrow_rate_annual.is_zero()
    }

    /// The multiplier the proportional costs and the fill probability apply
    /// to a conversion's rate: `(1 - tax)(1 - logistics)(1 - settlement
    /// risk) × p(fill)`. Borrow is time-based and is charged on the net edge
    /// instead, where the holding period is known.
    pub fn rate_multiplier(&self) -> Result<Decimal> {
        let mut multiplier = self.fill_probability;
        for (fraction, what) in [
            (self.tax_duty_fraction, "tax and duty"),
            (self.logistics_fraction, "logistics"),
            (self.settlement_risk_fraction, "settlement risk"),
        ] {
            multiplier = mul(multiplier, Decimal::ONE - fraction, what)?;
        }
        Ok(multiplier)
    }

    /// Refuse a cost outside its range, naming it.
    pub fn validate(&self, label: &str) -> Result<()> {
        for (fraction, what) in [
            (self.tax_duty_fraction, "tax and duty"),
            (self.logistics_fraction, "logistics"),
            (self.settlement_risk_fraction, "settlement risk"),
        ] {
            if fraction.is_negative() || fraction >= Decimal::ONE {
                return Err(Error::invalid(format!(
                    "conversion {label} has a {what} fraction of {fraction}, outside [0, 1); a \
                     cost is a non-negative share of notional less than the whole of it"
                )));
            }
        }
        if !self.fill_probability.is_positive() || self.fill_probability > Decimal::ONE {
            return Err(Error::invalid(format!(
                "conversion {label} has a fill probability of {}, outside (0, 1]; a conversion \
                 that can never fill is removed from the graph, not priced at zero",
                self.fill_probability
            )));
        }
        if self.borrow_rate_annual.is_negative() {
            return Err(Error::invalid(format!(
                "conversion {label} has a borrow rate of {}; a rate earned is gross edge, not a \
                 negative cost",
                self.borrow_rate_annual
            )));
        }
        Ok(())
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
    /// Tax and duty, logistics, settlement risk, fill probability and
    /// borrow for this conversion. Defaulted on deserialisation so a graph
    /// written before the field existed reads as one that stated none.
    #[serde(default)]
    pub costs: EdgeCosts,
}

impl ConversionEdge {
    /// The rate after the conversion's own costs. What the search searches on.
    ///
    /// The venue fee and every proportional cost in [`EdgeCosts`], with the
    /// fill probability as an expectation: a path that only clears when
    /// every leg fills for certain and no duty is charged is not found.
    pub fn effective_rate(&self) -> Result<Decimal> {
        let after_fee = mul(
            self.indicative_rate,
            Decimal::ONE - self.cost_fraction,
            "effective rate",
        )?;
        mul(after_fee, self.costs.rate_multiplier()?, "effective rate")
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

    /// Whether orders may be sent to every venue an edge would touch.
    ///
    /// For a [`EdgeKind::Trade`] or [`EdgeKind::Transfer`] that is both ends
    /// of the edge. A [`EdgeKind::Synthetic`] sends one order per component,
    /// and a component's venue need not match either end of the edge — a
    /// basket held at one venue can be built from names that trade elsewhere.
    /// Checking only `from`/`to` would let a component sitting on a halted or
    /// unreachable venue through the search's tradability filter, which is
    /// exactly the "path through a halted venue" this method exists to keep
    /// out; the component leg would then be the one that fails at the venue,
    /// discovered by an order rather than by this check.
    pub fn edge_is_tradable(&self, edge: &ConversionEdge) -> bool {
        let mut venues: Vec<&VenueId> = vec![&edge.from.venue, &edge.to.venue];
        if let EdgeKind::Synthetic { components, .. } = &edge.kind {
            venues.extend(components.iter().map(|component| &component.venue));
        }
        venues.into_iter().all(|venue| {
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

    /// Re-quote a trade edge from what a book says now.
    ///
    /// The cell holds the books and the graph holds the cycles they might
    /// close, and the two change at different rates: the whitelist on policy
    /// change, the quotes on every message. Rebuilding the graph per pass
    /// would allocate a graph's worth of nodes and edges on the hot path to
    /// change three numbers per edge, so the three numbers are changed in
    /// place. A rate of zero marks the edge unusable — the search already
    /// refuses a non-positive rate — which is how a book that has gone stale
    /// or empty takes its edge out of every cycle without removing it.
    ///
    /// Refused for an index that names no edge, an edge that is not a trade —
    /// a transfer's rate is one by definition and a synthetic has no single
    /// book to quote from — or a negative rate, which no book produces.
    pub fn refresh_trade(
        &mut self,
        index: usize,
        indicative_rate: Decimal,
        observed_at: Timestamp,
        observations: u32,
    ) -> Result<()> {
        if indicative_rate.is_negative() {
            return Err(Error::invalid(format!(
                "a rate of {indicative_rate} is negative; no book quotes one, so the refresh is \
                 refused rather than applied"
            )));
        }
        let Some(edge) = self.edges.get_mut(index) else {
            return Err(Error::not_found(format!(
                "no conversion at index {index}; the graph holds {}",
                self.edges.len()
            )));
        };
        if !matches!(edge.kind, EdgeKind::Trade { .. }) {
            return Err(Error::invalid(format!(
                "conversion {} is a {} and has no book to re-quote from",
                edge.label(),
                edge.kind.as_str()
            )));
        }
        edge.indicative_rate = indicative_rate;
        edge.observed_at = observed_at;
        edge.observations = observations;
        Ok(())
    }

    /// Replace a conversion's own costs with what was measured.
    ///
    /// The measured half of R12-MESH-5055 to -5057: a cell that knows a
    /// venue's borrow rate, a bridge's toll or a leg's historical fill rate
    /// states it on the edge it belongs to, between scans, the way
    /// [`Self::refresh_trade`] restates a rate. Refused for an index naming
    /// no edge or for a cost outside its range — and for a cost set that
    /// would leave a settlement-stage or region edge free, which
    /// [`Self::add_edge`] refuses for the same reason.
    pub fn set_costs(&mut self, index: usize, costs: EdgeCosts) -> Result<()> {
        let Some(edge) = self.edges.get(index) else {
            return Err(Error::not_found(format!(
                "no conversion at index {index}; the graph holds {}",
                self.edges.len()
            )));
        };
        let mut candidate = edge.clone();
        candidate.costs = costs;
        Self::validate(&candidate)?;
        if let Some(slot) = self.edges.get_mut(index) {
            *slot = candidate;
        }
        Ok(())
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
            costs: EdgeCosts::default(),
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
            costs: EdgeCosts::default(),
        })
    }

    /// Add a movement of one holding between any two locations: venues,
    /// regions, representations or settlement stages (R12-MESH-5053).
    ///
    /// The general form of [`Self::add_transfer`], which moves only between
    /// venues. A wrap or unwrap, a bridge between regions, or settlement
    /// completing (`Pending` to `Settled`) is a transfer of the same asset
    /// that changes where or how it is held. Refused, by [`Self::add_edge`],
    /// when it changes the asset or moves nothing, and when it joins two
    /// nodes differing only in settlement stage or region at no cost.
    pub fn add_movement(
        &mut self,
        from: Node,
        to: Node,
        cost_fraction: Decimal,
        costs: EdgeCosts,
        observed_at: Timestamp,
        observations: u32,
    ) -> Result<usize> {
        self.add_edge(ConversionEdge {
            from,
            to,
            indicative_rate: Decimal::ONE,
            cost_fraction,
            kind: EdgeKind::Transfer,
            observed_at,
            observations,
            costs,
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
            costs: EdgeCosts::default(),
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
        edge.costs.validate(&edge.label())?;
        // R12-MESH-5053: two nodes that differ only in settlement stage or
        // region are distinct, and what joins them is never free — settling
        // takes time and can fail, and a region is a different latency and a
        // different jurisdiction. An edge stating no cost at all between
        // them is a cycle with a hole in it.
        if edge.from.differs_only_in_settlement_or_region(&edge.to)
            && edge.cost_fraction.is_zero()
            && edge.costs.is_free()
        {
            return Err(Error::invalid(format!(
                "conversion {} joins two holdings differing only in settlement stage or region \
                 at no cost; state the transfer fee, settlement risk, fill probability or \
                 borrow it carries — moving between them is never free",
                edge.label()
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
                // A transfer moves the asset somewhere: another venue, region,
                // representation or settlement stage. One that changes none
                // of them moves nothing.
                if edge.from.venue == edge.to.venue
                    && edge.from.region == edge.to.region
                    && edge.from.representation == edge.to.representation
                    && edge.from.settlement == edge.to.settlement
                {
                    return Err(Error::invalid(format!(
                        "transfer {} does not leave the venue, region, representation or \
                         settlement stage it started in",
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
