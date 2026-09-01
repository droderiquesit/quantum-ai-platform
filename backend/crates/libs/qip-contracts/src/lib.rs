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

pub mod capital;
pub mod degradation;
pub mod edge;
pub mod feature;
pub mod gate;
pub mod governance;
pub mod intent;
pub mod message;
pub mod policy;
pub mod signal;
pub mod time;
pub mod venue;

pub use capital::{CapitalEnvelope, CapitalGrant, Utilisation};
pub use degradation::{AllocationMode, Capability, DegradationState, Freshness, StrategyClass};
pub use edge::{Deduction, DeductionKind, LegPlan, LegStep, NetEdge};
pub use feature::{FeatureKey, FeatureValue, FeatureVector, Revision};
pub use gate::{GateOutcome, GateStage, Promotion};
pub use governance::{Approval, Control, Entitlement, Provenance, Severity, Usage};
pub use intent::{
    Contributor, Intent, NetIntent, NettingPolicy, Representation, net, netting_ratio,
};
pub use message::{BookSide, MarketMessage, MessageBody, TradeCondition};
pub use policy::{PolicyItem, PolicyPayload, Slot};
pub use signal::{Conviction, Signal, SignalKind, StrategyId};
pub use time::{Stamped, Watermark};
pub use venue::{Origin, VenueClass, VenueId, VenueStatus};
