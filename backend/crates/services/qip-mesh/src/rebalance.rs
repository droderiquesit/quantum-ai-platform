//! Inventory drift left by a distributed cycle, raised as work (MESH-033).
//!
//! The arithmetic moved to `qip_peer::rebalance` (ADR 0154 decision 6) so the
//! peer link's saga, which cannot depend on a service, can call it when an
//! epoch closes. This module re-exports it for `qip-mesh`'s callers.
//!
//! Legs land wherever their venues are. If nothing converts the resulting
//! drift into a task, the next cycle starts from a position nobody chose and
//! pre-positioning (MESH-020) decays unseen. The task cites the epoch that
//! caused it, so the treasury can attribute the movement to a cycle.

pub use qip_peer::rebalance::{RebalanceTask, rebalancing_tasks};
