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
pub mod metrics;

pub use aggregate::{AggregateFigures, RiskAggregates};
pub use factor::{FactorRisk, RiskDecomposition};
pub use hedge::{
    HedgeAxis, HedgeEngine, HedgeExposures, HedgeInstrument, HedgeOutcome, HedgePolicy,
    HedgeProposal, HedgeRefusal, HedgeSide, propose_hedge,
};
pub use limits::{Limit, LimitBreach, LimitCheck, LimitKind, LimitSet, RiskState, Severity};
pub use metrics::{DrawdownProfile, RiskMetrics, TailRisk};
