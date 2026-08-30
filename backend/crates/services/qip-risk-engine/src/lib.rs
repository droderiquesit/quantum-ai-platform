//! `qip-risk-engine` — the control function.
//!
//! Three things live here, and all three are deterministic arithmetic against
//! declared limits. A control whose output depends on how a prompt was phrased
//! is not a control, so no model is consulted anywhere in this crate.
//!
//! * [`autonomy`] holds the autonomy level and the kill switch. The shipped
//!   default is paper trading, the deployment ceiling starts there too, and
//!   raising either requires an authenticated operator — with a second
//!   approver to reach any live level. There is no path from a model output,
//!   an agent finding or a configuration value to a higher level.
//! * [`pretrade`] checks each order against the risk state it *would* produce.
//!   Checking the state before the trade is the classic mistake: it passes
//!   every order up to the one that breaches, and then passes that one too.
//! * [`monitor`] watches the book while it sits, because a position that was
//!   6% of equity when it opened is 12% after the rest of the book halves.
//!
//! The kill switch is asymmetric on purpose. Tripping it requires no authority
//! — any component that notices something wrong can stop the platform, because
//! a false stop costs far less than a missed one. Clearing it requires an
//! operator.

pub mod autonomy;
pub mod monitor;
pub mod pretrade;

pub use autonomy::{
    AutonomyChange, AutonomyController, AutonomyLevel, KillSwitch, KillSwitchTrip, OperatorIdentity,
};
pub use monitor::{MonitorAction, MonitorPolicy, Observation, RiskMonitor};
pub use pretrade::{PreTradeChecker, PreTradeDecision, PreTradeResult, ProposedOrder};
