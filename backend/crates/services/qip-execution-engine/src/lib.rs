//! `qip-execution-engine` — ACT.
//!
//! Orders, venues, and the controls between a decision and a market.
//!
//! **Live trading is off by default and cannot be turned on from inside the
//! platform.** [`oms::OrderManager::submit`] refuses a live venue below a live
//! autonomy level, the autonomy level cannot be raised without an
//! authenticated operator and a second approver, and the deployment ceiling
//! starts at paper trading so a platform that was never configured for live
//! trading cannot reach it at all.
//!
//! Every fill carries [`order::Fill::simulated`], set by the OMS from the
//! broker rather than taken on the broker's word. A reconciliation can
//! therefore tell paper from real without consulting configuration, which is
//! exactly what gets confused between a test and a deployment.
//!
//! [`broker::LiveBroker`] is a complete interface with no transport. It reports
//! precisely what a deployment is missing — credential, transport, and an
//! explicit operator enablement, which are three separate things — so a
//! misconfigured live deployment fails at start-up rather than at the first
//! order.
//!
//! [`quoting`] and [`origination`] are the newest members and the ones to read
//! the boundary argument for. A quote loop is the most dangerous thing in this
//! workspace to build, because in every real venue *quoting is order
//! submission*. So [`quoting`] produces a [`quoting::QuotePair`] — two prices
//! and a size, carrying no venue, no side, no client id and no time in force —
//! and there is no function in this crate that turns one into an
//! [`order::Order`]. [`origination`] gates market creation behind five checks
//! and a bound that is a constant in its own file rather than a configurable.
//! Neither module names a venue or reaches a [`broker::Broker`], and
//! `qip-acceptance`'s `quote_loop` suite asserts that over their source text.

pub mod broker;
pub mod feasibility;
pub mod multileg;
pub mod observation;
pub mod oms;
pub mod order;
pub mod origination;
pub mod quoting;
pub mod session;

pub use broker::{
    Broker, LiveBroker, LiveVenueConfig, SimulatedBroker, SimulationSettings, VenueCapabilities,
};
pub use feasibility::{Infeasible, VenueFeasibility};
pub use multileg::{GroupState, Leg, LegGroup, Verdict};
pub use observation::{DeclaredVenueProfile, ObservedVenueFacts, VenueObservation};
pub use oms::{OrderManager, RefusalReason, SubmissionResult, order_type_for};
pub use order::{Fill, Order, OrderState, OrderType, Side};
pub use origination::{
    AbsenceCause, AbsenceExplanation, AdverseSelectionModel, ClassApproval, OriginationMandate,
    OriginationRequest, Valuation,
};
pub use quoting::{
    QueuePosition, QuoteDecision, QuoteInputs, QuotePair, QuotePolicy, QuoteReference, SpreadTerms,
    Withheld, quote,
};
pub use session::{
    RecordedInstruction, RecordedSession, SESSION_ENTRY_LIMIT, SESSION_WINDOW, SealOutcome,
    SessionRecorder,
};
