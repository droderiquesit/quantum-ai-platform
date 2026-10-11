//! AGENCY-026 / R12-GOV-6123: an action tool is registered with six
//! declarations or not at all, and a revoked, rate-limited or over-budget tool
//! refuses the next call.
//!
//! The registry is `qip_contracts::action::ToolRegistry`, re-exported. This
//! module used to hold a second, unaudited registry that nothing called:
//! `engine::execute` consulted neither, so a tool revoked in one was still
//! "registered" in the other and nothing stood between a plan and its
//! adapter (R12-AGENCY-6091). [`crate::engine::execute`] now authorises every
//! step against this one, and the registry audits every decision it makes.

pub use qip_contracts::action::{ToolAudit, ToolRegistration, ToolRegistry};
