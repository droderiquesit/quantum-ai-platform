//! `qip-world-model` — the UNDERSTAND stage.
//!
//! A bitemporal knowledge graph of what the platform believes about the world,
//! plus a causal layer over it and a point-in-time feature store.
//!
//! Bitemporality is the load-bearing idea. Every fact carries *when it was true*
//! and *when the platform learned it*, which are different questions with
//! different answers. A backtest asking "was this true in March?" gets a
//! different result from "did we know it in March?", and only the second is a
//! legitimate basis for a decision made in March. Storing one timestamp makes
//! that distinction unrepresentable, and the resulting look-ahead is invisible
//! in the backtest results.
//!
//! The causal layer is deliberately separate from the relationship graph. That a
//! company supplies another is a fact; that a disruption at the supplier moves
//! the customer's price is a claim, with a mechanism, a lag, a strength and
//! evidence behind it. Conflating the two is how a correlation becomes a thesis.

pub mod causal;
pub mod features;
pub mod graph;
pub mod liquidity;
pub mod relationship;
pub mod state;
pub mod world;

pub use causal::{CausalEdge, CausalGraph, Effect, Mechanism, PropagationResult};
pub use features::{Feature, FeatureStore, FeatureValue};
pub use graph::{Fact, KnowledgeGraph, Node, NodeKind};
pub use liquidity::{
    Concentration, DepthObservation, LiquidityDrift, LiquidityMap, LiquidityTopology, VenueDepth,
    VenueShift,
};
pub use relationship::{Relationship, RelationshipKind};
pub use state::{Change, ChangeKind, WorldDiff, WorldState};
pub use world::WorldModel;
