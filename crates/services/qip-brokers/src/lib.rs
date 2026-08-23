//! `qip-brokers` — the broker and exchange adapter framework: one interface,
//! with sandbox and simulated venues behind it.
//!
//! Every other execution crate talks to a venue through
//! [`qip_execution_engine::broker::Broker`], which is deliberately narrow: a
//! name, a simulation flag, availability, submit, cancel. That is the right
//! surface for an order management system and the wrong one for a venue, which
//! also has a session to open, a logon to survive, a heartbeat that can stop,
//! market data, amendments, and books of its own to be reconciled against.
//! [`VenueAdapter`] adds exactly those, and extends `Broker` rather than
//! replacing it, so every adapter here drops into the existing OMS unchanged
//! and inherits its controls instead of routing around them.
//!
//! Four decisions in this crate are structural rather than procedural. Each one
//! could have been a runtime check; each one is a type instead, because a
//! runtime check is a thing somebody can forget to call and a type is not.
//!
//! # 1. There is no live venue to construct
//!
//! The platform's autonomy ceiling is paper trading, and this crate makes that
//! a property of the code rather than of the configuration. [`AdapterClass`]
//! has two variants — [`AdapterClass::Simulated`] and
//! [`AdapterClass::Sandbox`] — and no third. There is no live adapter in this
//! crate to instantiate, no feature flag that adds one, and no string that
//! deserialises into one:
//!
//! ```compile_fail
//! # use qip_brokers::AdapterClass;
//! // There is no `Live` variant. A deployment cannot reach live trading by
//! // setting a field, because there is no field to set.
//! let class = AdapterClass::Live;
//! ```
//!
//! The absence survives serialisation too, which is the path a runtime check
//! would most likely miss — a venue descriptor read from a config file:
//!
//! ```
//! # use qip_brokers::AdapterClass;
//! assert!(serde_json::from_str::<AdapterClass>("\"live\"").is_err());
//! assert!(serde_json::from_str::<AdapterClass>("\"simulated\"").is_ok());
//! ```
//!
//! Should a real venue ever be added, [`credential::standard_requirements`]
//! already names what it would take: an endpoint, a session credential, session
//! parameters, an account, entitlements, and a separate, explicitly named
//! operator enablement — because holding a credential is not the same as having
//! decided to trade. None of those environment variables exist in this build,
//! and the refusal when they are absent is a clean [`qip_core::Error`] naming
//! every missing item, not a panic and not a silent fallback to a demo account.
//!
//! # 2. An order cannot be sent to a venue that is not ready
//!
//! [`VenueAdapter::submit_order`] takes a [`ReadyTicket`], whose fields are
//! private and whose only constructor is [`ConnectionState::ready_ticket`] —
//! which refuses unless the venue is ready *at the timestamp given*. The
//! unsafe call is therefore not merely discouraged; it cannot be written,
//! because there is no ticket to pass:
//!
//! ```compile_fail
//! # use qip_brokers::ReadyTicket;
//! # use qip_core::time::Timestamp;
//! // No public constructor, and the fields are private.
//! let forged = ReadyTicket { venue: "x".into(), session: 1, issued_at: Timestamp::EPOCH };
//! ```
//!
//! Every ticket names a session number that increments on each phase change, so
//! a ticket minted before a heartbeat gap is refused after it. Cancels take no
//! ticket at all: cancelling reduces risk, and the safe direction never waits
//! for permission.
//!
//! # 3. A secret cannot reach a log or a serialised form
//!
//! [`Secret`] holds a value and gives it out only through
//! [`Secret::expose_str`], which is named to be conspicuous in review. Its
//! `Debug` and `Display` are written by hand and redact; it implements neither
//! [`serde::Serialize`] nor [`serde::Deserialize`], so a struct that contains
//! one cannot derive them either:
//!
//! ```compile_fail
//! # use qip_brokers::Secret;
//! let secret = Secret::new("value");
//! // `Secret` is not `Serialize`. A secret cannot be written to an event, a
//! // snapshot or a log line by accident.
//! let leaked = serde_json::to_string(&secret).unwrap();
//! ```
//!
//! [`VenueCredential`] may carry resolved secrets, and its `Debug`, `Display`
//! and `Serialize` are all hand-written to emit only the venue, the account and
//! the *names* of what is satisfied. A credential's normal state is to carry
//! references — the environment variable each secret is read from — because a
//! reference in a log is a configuration note and a value in a log is an
//! incident.
//!
//! # 4. The simulation flag belongs to the adapter, not to the message
//!
//! Every fill an adapter reports passes through [`stamp_simulated`], which
//! overwrites whatever the venue said with what the adapter is. The order
//! management system applies the same rule again downstream. The belt and the
//! braces are both deliberate: this bit decides whether a number is real money
//! in every report and reconciliation, and a malformed venue message does not
//! get a vote on it.
//!
//! # What is actually in here
//!
//! [`SimulatedExchange`] is a venue that really matches. Orders queue behind
//! each other at a price, a large order walks several levels and leaves a
//! residual, an amendment that adds size loses its place in the queue, a market
//! order that outruns the book has its remainder cancelled rather than quietly
//! filled, commission is charged exactly, and the acknowledgement arrives after
//! a latency. It refuses unlisted instruments, off-lot quantities, off-tick
//! prices, execution algorithms it does not implement, and a small seeded
//! fraction of orders for no reason at all — because an execution path that has
//! never met a rejection meets its first one in production. Its heartbeat can
//! be stopped without warning, which is the failure the connection state
//! machine exists for.
//!
//! Its books are [`AccountLedger`], a thin wrapper over
//! [`qip_portfolio::portfolio::Portfolio`] rather than a second set of
//! accounting: `equity = cash + position value` holds exactly after any
//! sequence of fills, and [`AccountLedger::verify`] says so on demand.
//!
//! [`RestOrderEntryAdapter`] is the other kind of venue: one that opens a
//! socket. It sends real orders over HTTP to whatever endpoint a deployment
//! configures, which makes it the only thing in this crate whose behaviour is
//! not decided inside this process. Everything it does is arranged around one
//! rule — **a fill is never inferred**. A submit that times out, a body that
//! will not parse, an acknowledgement naming a different order: each of those
//! leaves the order's state *unknown*, which is a third thing that is neither
//! filled nor rejected, and which only a query answered by the venue resolves.
//! Every submit carries a client-generated idempotency key, and where the venue
//! does not promise to honour one the adapter refuses to retry rather than risk
//! a second order. With no endpoint or no credential it refuses and opens no
//! socket; there is no code path from it into [`SimulatedExchange`], because a
//! live path that quietly degraded to a simulator would report fills for orders
//! that never left the process. Its own module documentation is the long form.
//!
//! The simulated venue reads no clock and no ambient random source. `at` is a
//! parameter on every operation and its coin-flips come from a seeded
//! [`qip_core::Xoshiro256`], so the same instructions at the same timestamps
//! with the same seed produce byte-identical fills. That is what makes a
//! simulated venue usable as a test oracle instead of merely as a stub.
//! [`RestOrderEntryAdapter`] inherits the first half and cannot have the
//! second: its timestamps are still the caller's or the venue's, but what a
//! venue answers is not this crate's to reproduce, and the one interval it
//! measures itself — the round trip on an acknowledgement — comes from
//! [`std::time::Instant`], because there is no way to learn how long a socket
//! took from a timestamp taken before it was opened.
//!
//! Money, prices and quantities are [`qip_core::Decimal`] throughout. `f64`
//! appears only for statistics — [`MarginState::leverage`] and
//! [`ExchangeSettings::rejection_probability`] — and is named so.

pub mod adapter;
pub mod connection;
pub mod credential;
pub mod exchange;
pub mod ledger;
pub mod matching;
pub mod rest;

pub use adapter::{
    AdapterClass, CashBalance, Heartbeat, MarginState, MarketData, OrderAck, PositionSnapshot,
    VenueAdapter, VenueOrder, VenueOrderState, stamp_simulated,
};
pub use connection::{ConnectionPhase, ConnectionState, ReadyTicket, SessionHealth};
pub use credential::{
    RequirementKind, Secret, VenueCredential, VenueRequirement, describe_missing,
    requirements_of_kind, standard_requirements,
};
pub use exchange::{BookableFill, ExchangeSettings, SimulatedExchange};
pub use ledger::{AccountLedger, MarginPolicy};
pub use matching::{ExecutionOutcome, MatchingEngine, Participant, Resting, Trade};
pub use rest::{
    IdempotencySupport, OrderOutcome, RestOrderEntryAdapter, RestOrderStats, RestVenueConfig,
    TrackedOrder,
};
