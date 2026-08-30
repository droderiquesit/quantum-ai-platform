//! `qip-events` — the strongly-typed event backbone.
//!
//! Everything the platform does is an event, and every investment decision must
//! be reconstructable from the event history (charter sections 8 and 24). Three
//! properties make that true rather than aspirational:
//!
//! * **Typed contracts.** A payload type declares its [`Topic`] and schema
//!   version through [`EventBody`]; publishing to the wrong topic will not
//!   compile, and a version mismatch is caught on read.
//! * **Deterministic dispatch.** The [`EventBus`] is a breadth-first queue, not
//!   a thread pool. The same inputs produce the same handler order, so a replay
//!   reproduces the original run exactly.
//! * **Tamper-evident history.** [`EventLog`] chains each record to its
//!   predecessor by hash, so a truncated or edited audit trail is detectable.

pub mod bus;
pub mod envelope;
pub mod log;
pub mod registry;
pub mod topic;

pub use bus::{EventBus, HandlerOutcome, Subscription};
pub use envelope::{AnyEvent, Envelope, EventBody};
pub use log::{EventFilter, EventLog, LogStats};
pub use registry::{SchemaDescriptor, SchemaRegistry};
pub use topic::{Topic, TopicGroup};
