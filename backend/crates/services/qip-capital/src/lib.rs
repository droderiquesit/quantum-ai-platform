//! `qip-capital` — global capital and risk.
//!
//! The central-plane counterpart to the edge cells. A cell trades without
//! asking permission, against a pre-signed [`qip_contracts::CapitalEnvelope`];
//! this crate decides what envelopes to issue, watches what the cells have
//! collectively done with them, and takes capital back.
//!
//! Seven things live here.
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
//! * [`reservation`] holds capital between the check and the trade. A
//!   proposal that passes a capital check without holding the capital is one
//!   half of a double-spend — the second proposal against the same free
//!   balance also passes — so [`reservation::ReservationLedger`] makes
//!   passing and holding the same operation, and the second is refused.
//! * [`recall`] withdraws capital mid-flight, and is explicit that a recall is
//!   a request. The reliable bound on a cell nobody can reach is the envelope
//!   expiry, which the cell enforces locally against its own clock.
//!
//! Nothing here reads a wall clock or draws a random number: every entry point
//! takes the [`qip_core::Timestamp`] it is reasoning about, so a replay
//! reproduces the same plans and the same signatures.
//!
//! # Turning a budget into grants
//!
//! ```
//! use qip_capital::{
//!     AllocationLimits, CapacityModel, CapitalAllocator, DrawdownSchedule, EnvelopeIssuer,
//!     EnvelopeTerms, StrategyProposal,
//! };
//! use qip_contracts::governance::Approval;
//! use qip_contracts::signal::StrategyId;
//! use qip_contracts::venue::VenueId;
//! use qip_core::{Decimal, Duration, Timestamp, dec};
//! use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
//!
//! # fn main() -> qip_core::error::Result<()> {
//! let now = Timestamp::from_secs(1_700_000_000);
//! let allocator = CapitalAllocator::new(
//!     AllocationLimits::new(dec!("10000000"), dec!("4000000"), dec!("6000000"), dec!("8000000"))?,
//!     DrawdownSchedule::default(),
//! );
//! let proposal = StrategyProposal {
//!     strategy: StrategyId::new("momentum-v3"),
//!     cell: "cell-lon-1".to_string(),
//!     venue: VenueId::new("XNYS"),
//!     expected_sharpe: 1.8,
//!     sharpe_standard_error: 0.3,
//!     capacity: CapacityModel::new(
//!         LiquidityProfile::listed(Decimal::from_int(5_000_000), 4.0),
//!         TransactionCostModel::listed(4.0),
//!         45.0,
//!         dec!("100"),
//!         0.5,
//!     )?,
//!     capacity_uncertainty: 0.2,
//! };
//!
//! // A 10% drawdown halves the book, per the shipped schedule.
//! let plan = allocator.allocate(&[proposal], 0.10, now)?;
//! assert_eq!(plan.drawdown_multiplier, dec!("0.5"));
//! assert!(plan.is_within_budget());
//!
//! let issuer = EnvelopeIssuer::new(vec![7u8; 32], "capital-key-1")?;
//! let approval = Approval::new("grant", "alice", now, "the committee approved this")?
//!     .countersigned_by("bram")?;
//! for allocation in &plan.allocations {
//!     let envelope = issuer.issue(
//!         &EnvelopeTerms::from_allocation(allocation, Duration::from_hours(8)),
//!         &approval,
//!         now,
//!     )?;
//!     issuer.verify(&envelope, now)?;
//!     // Every grant expires; that is the backstop for a cell nobody can reach.
//!     assert!(!envelope.is_live(envelope.expires_at()));
//! }
//! # Ok(())
//! # }
//! ```

pub mod allocation;
pub mod capacity;
pub mod envelope;
pub mod exposure;
pub mod margin;
pub mod recall;
pub mod reservation;

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
pub use reservation::{Reservation, ReservationLedger};
