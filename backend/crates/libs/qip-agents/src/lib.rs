//! `qip-agents` — the agent runtime.
//!
//! An agent in this platform is not a prompt. It is a governed component with
//! a declared purpose, an explicit capability grant, a resource budget, an
//! owner and an expiry date, and it produces a record whether it succeeds or
//! fails.
//!
//! Four ideas carry the weight:
//!
//! * [`capability`] and [`manifest`] make separation of duties structural.
//!   A research agent cannot submit an order because
//!   [`manifest::AgentManifest::validate`] refuses to authorise the
//!   combination, not because the code that would do it was never written.
//! * [`runtime::Gated`] puts the capability check between the agent and every
//!   facility it can reach, so an agent that neglects to check its own
//!   permissions is still contained.
//! * [`budget`] charges before work happens, so the call that would exceed the
//!   allowance never runs.
//! * [`finding::NumericFact`] gives every number an agent reports a
//!   provenance, and the only provenances that exist are *observed* and
//!   *computed*. A language model has nowhere to put a number.
//!
//! [`governance`] then checks the properties that only exist across the whole
//! roster: that somebody holds a veto, that the adversarial function does not
//! report to the people it reviews, that escalation chains terminate.

pub mod budget;
pub mod capability;
pub mod finding;
pub mod governance;
pub mod manifest;
pub mod memory;
pub mod runtime;

pub use budget::{Budget, BudgetLedger, BudgetLine, Spend};
pub use capability::{Capability, CapabilitySet};
pub use finding::{
    AgentBrief, AgentFinding, BriefPrecedent, Direction, FindingStatus, NumericFact,
    NumericProvenance,
};
pub use governance::{AuditTrail, GovernanceFinding, Roster, Severity};
pub use manifest::{AgentManifest, AgentRole, EscalationPolicy};
pub use memory::{Episode, EpisodeOutcome, Lesson, PromotionPolicy, ResearchMemory};
pub use runtime::{
    Agent, AgentContext, AgentHost, AgentRunRecord, Gated, Reading, RunStatus, Upstream,
};
