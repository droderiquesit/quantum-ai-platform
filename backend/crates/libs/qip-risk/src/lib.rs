//! `qip-risk` — risk measurement, the limit engine, and the hedge engine.
//!
//! Three parts. [`metrics`] and [`factor`] measure risk; [`limits`] decides
//! what is allowed; [`hedge`] proposes — never submits — orders that reduce a
//! named exposure toward a declared target. [`hedge_search`] is the Hedge
//! Brain's search: the cheapest executable plan, across instrument classes and
//! forecast worlds, that keeps the book above its survival floor — a plan, never
//! an order and never a veto.
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
//! [`operational`] and [`systemic`] are the last two of the capability map's
//! risk types (R12-RISK-1162): the gross stranded at a venue the platform
//! has withdrawn for process failure, and the gross one causal driver reaches
//! across regions. Both charge an axis the same `MaxAxisWeight` veto reads.
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
pub mod hedge_search;
pub mod limits;
pub mod market_factor;
pub mod metrics;
pub mod operational;
pub mod shared_cause;
pub mod systemic;
pub mod tolerance;

pub use aggregate::{AggregateFigures, RiskAggregates};
pub use factor::{FactorRisk, RiskDecomposition};
pub use hedge::{
    HedgeAxis, HedgeEngine, HedgeExposures, HedgeInstrument, HedgeOutcome, HedgePolicy,
    HedgeProposal, HedgeRefusal, HedgeSide, propose_hedge,
};
pub use limits::{
    Limit, LimitBreach, LimitCheck, LimitKind, LimitSet, Narrowing, RiskState, Severity,
};
pub use metrics::{DrawdownProfile, RiskMetrics, TailRisk};
pub use operational::{OPERATIONAL_FAILURE_AXIS, with_operational_failure};
pub use shared_cause::{
    CAUSAL_DRIVER_AXIS, FACTOR_AXIS, FAMILY_AXIS, SHARED_CAUSE_AXES, SharedCauseExposure,
};
pub use systemic::{SYSTEMIC_CONTAGION_AXIS, SYSTEMIC_REGION_SPAN, with_systemic_contagion};
