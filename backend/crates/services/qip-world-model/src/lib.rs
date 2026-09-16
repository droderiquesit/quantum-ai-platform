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
pub mod confounder;
pub mod exposure;
pub mod falsification;
pub mod features;
pub mod granger;
pub mod graph;
pub mod liquidity;
pub mod relationship;
pub mod resolution_source;
pub mod state;
pub mod vocabulary;
pub mod world;

pub use causal::{
    CausalEdge, CausalGraph, ConditionStanding, EdgeStanding, Effect, Mechanism, PropagationResult,
};
pub use confounder::{Confounder, ConfounderSet, ConfounderStanding};
pub use exposure::{
    ConcentrationReport, Exposure, ExposureSet, SecondOrderReview, SharedDriver, UnheldDependency,
    hidden_concentration, instruments_exposed_to, second_order_exposure, unheld_dependencies,
};
pub use falsification::{
    Breach, FalsificationPass, Falsifier, HeldOut, HypothesisSource, Inadmissible, LeakageTally,
    SourceCensus, SourceStanding, TrialLedger, Verdict, rolling_statistic,
};
pub use features::{FEATURE_HISTORY, Feature, FeatureLookup, FeatureStore, FeatureValue};
pub use graph::{Fact, KnowledgeGraph, Node, NodeKind};
pub use liquidity::{
    Concentration, DepthObservation, LiquidityDrift, LiquidityMap, LiquidityTopology, VenueDepth,
    VenueShift,
};
pub use relationship::{Relationship, RelationshipKind};
pub use resolution_source::{RESOLUTION_SOURCE_PREFIX, ResolutionSourceClaim};
pub use state::{Change, ChangeKind, WorldDiff, WorldState};
pub use vocabulary::{AltMetric, FeatureRead, MacroSeries, SubjectKind, UNWRITTEN, Unwritten};
pub use world::WorldModel;
