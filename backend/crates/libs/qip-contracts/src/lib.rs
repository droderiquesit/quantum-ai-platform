//! `qip-contracts` — the vocabulary shared between the edge cells and the
//! central plane.
//!
//! Every subsystem in the hot path and the strategy factory is written against
//! the types here rather than against each other. That is what keeps the
//! dependency graph a fan-out from this crate instead of a mesh: an order book
//! does not know what a strategy is, a strategy does not know what a venue
//! protocol looks like, and neither knows how capital is allocated.
//!
//! The types are deliberately thin. A contract that carries behaviour becomes
//! a place where two subsystems disagree about semantics, and the disagreement
//! only shows up in production.
//!
//! Four ideas recur:
//!
//! * **Every quantity that reaches a decision is exact.** Prices, sizes and
//!   money are [`qip_core::Decimal`]; `f64` appears only where the value is a
//!   statistic and is named so.
//! * **Every fact carries both times.** [`Stamped`] pairs valid-time with
//!   known-time, so a subsystem physically cannot read a value that was not yet
//!   known at the moment it claims to be reasoning about.
//! * **Nothing crosses a boundary unattributed.** [`Origin`] says which venue,
//!   feed and sequence a fact came from, and survives every transformation.
//! * **A cost is never a single number.** [`NetEdge`] decomposes gross edge
//!   into the nine deductions that decide whether an opportunity is real —
//!   seven the market charges, plus the compute and data cost of having
//!   reached the decision at all — and refuses to report a net figure that its
//!   parts do not sum to.

pub mod action;
pub mod ambient;
pub mod capital;
pub mod captured_event;
pub mod degradation;
pub mod edge;
pub mod expansion;
pub mod feasibility;
pub mod feature;
pub mod federation;
pub mod gate;
pub mod governance;
pub mod intent;
pub mod knowledge_pack;
pub mod ledger;
pub mod ledger_operation;
pub mod market_event;
pub mod market_state;
pub mod message;
/// The peer half of the Reflex Mesh: what regional cells say to each other
/// (moved from `qip-mesh` so the edge node, which may not link that crate,
/// can speak it).
pub mod peer;
pub mod policy;
pub mod quantum;
pub mod reflex;
pub mod reflex_journal;
pub mod reflex_package;
pub mod regional_episode;
pub mod replay;
pub mod routing;
pub mod schema;
pub mod signal;
pub mod time;
pub mod trust;
pub mod venue;
pub mod wire;

// Blueprint v12.0 §27 and §24.9 contracts between brains, each a
// `validated::Validated` record. `action_intent::ActionIntent` is deliberately
// not re-exported here: `action::ActionIntent` is a different concept with the
// same name (see the module doc).
pub mod action_intent;
pub mod control;
pub mod control_pack;
pub mod episode;
pub mod evidence_packet;
pub mod forecast;
pub mod knowledge_delta;
pub mod microstructure_delta;
pub mod mirror_checkpoint;
pub mod opportunity_token;
pub mod risk_envelope;
pub mod strategy_pack;
pub mod symbolic;
pub mod validated;
pub mod world_model;

pub use capital::{CapitalAdmission, CapitalEnvelope, CapitalGrant, Utilisation};
pub use control::{ComputePlan, HedgePlan, InformationRequest, ModelVote};
pub use control_pack::ControlPackAnnouncement;
pub use degradation::{AllocationMode, Capability, DegradationState, Freshness, StrategyClass};
pub use edge::{Deduction, DeductionKind, LegPlan, LegStep, NetEdge};
pub use episode::Episode;
pub use evidence_packet::EvidencePacket;
pub use expansion::{
    BrainSpec, CurriculumItem, GapClass, GapSignal, MemoryEntry, MemoryKind, RetentionPolicy,
    ValuePolicy,
};
pub use feature::{
    Distribution, FeatureKey, FeatureSnapshot, FeatureValue, FeatureVector, ForecastLattice,
    KnowableAt, Revision,
};
pub use forecast::{Forecast, ForecastState};
pub use gate::{GateOutcome, GateStage, Promotion};
pub use governance::{Approval, Control, Entitlement, Provenance, Severity, Usage};
pub use intent::{
    Contributor, Intent, NetIntent, NettingPolicy, Representation, net, netting_ratio,
};
pub use knowledge_delta::KnowledgeDelta;
pub use message::{BookSide, MarketMessage, MessageBody, TradeCondition};
pub use microstructure_delta::MicrostructureDelta;
pub use mirror_checkpoint::MirrorCheckpoint;
pub use opportunity_token::OpportunityToken;
pub use policy::{
    BeliefState, DistilledModelContract, ModelPack, PolicyFrame, PolicyItem, PolicyPayload,
    RegimeChange, RiskGate, Slot, UncertaintyType,
};
pub use quantum::{ChosenPath, DecisionRequest, RoutingDecision, SolverKind, SolverResult};
pub use reflex_journal::{DecisionOutcome, DecisionReason, ReflexDecision, ReflexJournalSummary};
pub use regional_episode::{EpisodeKind, RegionalEpisode};
pub use risk_envelope::RiskEnvelope;
pub use routing::{QuantumRouter, RoutingConfig};
pub use schema::{CompatibilityPolicy, SchemaDefinition, SchemaRegistry};
pub use signal::{Conviction, OpportunityId, Signal, SignalKind, StrategyId};
pub use strategy_pack::StrategyPack;
pub use symbolic::SymbolicDerivation;
pub use time::{Stamped, Watermark};
pub use validated::{Contract, Validated};
pub use venue::{Origin, VenueClass, VenueId, VenueStatus};
pub use world_model::{ModelDisagreement, WorldBranch, WorldModelState};
