//! `qip-capital` — global capital and risk.
//!
//! The central-plane counterpart to the edge cells. A cell trades without
//! asking permission, against a pre-signed [`qip_contracts::CapitalEnvelope`];
//! this crate decides what envelopes to issue, watches what the cells have
//! collectively done with them, and takes capital back.
//!
//! Five things live here.
//!
//! * [`allocation`] splits a risk budget across strategies and cells, subject
//!   to per-strategy, per-cell, per-venue and total limits at once. Strategies
//!   are sized on a lower confidence bound — the point estimate less a stated
//!   number of standard errors — so a great number with a wide error bar gets
//!   less than the number alone would suggest. Sums are exact: every
//!   allocation is capped by a running [`qip_core::Decimal`] remainder, so a
//!   plan cannot exceed its budget by a rounding step.
//! * [`capacity`] models the size at which a strategy's own impact eats its
//!   edge, using the square-root law that
//!   [`qip_financial::costs::TransactionCostModel`] already implements. Past
//!   capacity extra capital carries negative expected edge, and the allocator
//!   reduces rather than extrapolating.
//! * [`envelope`] signs and verifies grants. Every envelope expires, and the
//!   ceiling on that is hours — the module documents plainly that HMAC is
//!   symmetric and names the asymmetric signing, key custody, rotation,
//!   revocation and replay scoping a production deployment still needs.
//! * [`exposure`] aggregates positions across cells along the axes limits are
//!   written against, and answers the question no cell can:
//!   [`exposure::AggregateExposure::crowded`] names the instruments several
//!   cells have independently accumulated.
//! * [`margin`] computes required margin against posted collateral, and how
//!   long the book would take to exit at a stated participation rate. A
//!   position that takes three weeks to leave is a different animal from one
//!   that takes an hour at the same notional.
//! * [`recall`] withdraws capital mid-flight, and is explicit that a recall is
//!   a request. The reliable bound on a cell nobody can reach is the envelope
//!   expiry, which the cell enforces locally against its own clock.
//!
//! Nothing here reads a wall clock or draws a random number: every entry point
//! takes the [`qip_core::Timestamp`] it is reasoning about, so a replay
//! reproduces the same plans and the same signatures.

pub mod allocation;
pub mod capacity;
pub mod envelope;
pub mod exposure;
pub mod margin;
pub mod recall;

pub use allocation::{
    Allocation, AllocationLimits, AllocationPlan, CapitalAllocator, DrawdownSchedule,
    StrategyProposal,
};
pub use capacity::{Capacity, CapacityBound, CapacityModel};
pub use envelope::{EnvelopeIssuer, EnvelopeTerms, MAXIMUM_ENVELOPE_VALIDITY};
pub use exposure::{
    AggregateExposure, CellPosition, ConcentrationFinding, ConcentrationLimits, CrowdedPosition,
};
pub use margin::{
    LiquidationHorizon, LiquidityAssessment, MarginModel, MarginRequirement, assess_liquidity,
};
pub use recall::{RecallOrder, RecallReason, RecallRegister, RecallState};
