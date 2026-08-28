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

pub mod broker;
pub mod multileg;
pub mod oms;
pub mod order;

pub use broker::{
    Broker, LiveBroker, LiveVenueConfig, SimulatedBroker, SimulationSettings, VenueCapabilities,
};
pub use multileg::{GroupState, Leg, LegGroup, Verdict};
pub use oms::{OrderManager, RefusalReason, SubmissionResult, order_type_for};
pub use order::{Fill, Order, OrderState, OrderType, Side};
