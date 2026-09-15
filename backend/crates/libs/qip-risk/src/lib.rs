//! `qip-risk` — risk measurement, the limit engine, and the hedge engine.
//!
//! Three parts. [`metrics`] and [`factor`] measure risk; [`limits`] decides
//! what is allowed; [`hedge`] proposes — never submits — orders that reduce a
//! named exposure toward a declared target.
//!
//! The measurement half is deliberately explicit about its assumptions. There
//! are two ways to compute value at risk from the same return series and they
//! disagree by a lot in the tail, so both are provided and named
//! ([`metrics::historical_var`], [`metrics::parametric_var`]) rather than one
//! being presented as *the* answer. Expected shortfall is reported alongside,
//! because a quantile says nothing about what lies beyond it.
//!
//! [`shared_cause`] produces the three envelope levels no instrument record
//! can carry — per family, per factor and per causal driver — and charges them
//! to the same axis mechanism the sector and country caps already read, so
//! that they are enforced by a veto already proven to fire rather than by a
//! second one nobody has driven.
//!
//! The decision half reads aggregates, never strategy lists: [`aggregate`]
//! keeps the running counters a check consults, updated per fill, so a check
//! costs the same at ten strategies as at ten thousand.
//!
//! The decision half is deterministic code, never model judgement. A limit
//! either binds or it does not, the check is reproducible, and the answer
//! carries which limit bound and by how much. The risk engine's veto is only
//! meaningful if it cannot be talked out of.

pub mod aggregate;
pub mod factor;
pub mod hedge;
pub mod limits;
pub mod market_factor;
pub mod metrics;
pub mod shared_cause;

pub use aggregate::{AggregateFigures, RiskAggregates};
pub use factor::{FactorRisk, RiskDecomposition};
pub use hedge::{
    HedgeAxis, HedgeEngine, HedgeExposures, HedgeInstrument, HedgeOutcome, HedgePolicy,
    HedgeProposal, HedgeRefusal, HedgeSide, propose_hedge,
};
pub use limits::{Limit, LimitBreach, LimitCheck, LimitKind, LimitSet, RiskState, Severity};
pub use metrics::{DrawdownProfile, RiskMetrics, TailRisk};
pub use shared_cause::{
    CAUSAL_DRIVER_AXIS, FACTOR_AXIS, FAMILY_AXIS, SHARED_CAUSE_AXES, SharedCauseExposure,
};
