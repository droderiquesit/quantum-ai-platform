//! `qip-api` — the HTTP surface.
//!
//! An in-tree HTTP/1.1 server, a versioned REST API, and the authentication in
//! front of both.
//!
//! Six things are worth reading before the code:
//!
//! * [`routes::ROUTES`] is the entire API surface with the authority each path
//!   requires, in one table. A security review can read what the API permits
//!   without reading what it does, and anything not in the table is a 404.
//! * [`auth::Authenticator::authenticate`] compares tokens in constant time
//!   and does not return early on a match. An early return would make response
//!   latency depend on how far down the credential list the match sits, which
//!   is a slower but perfectly usable oracle.
//! * [`http::ServerLimits`] bounds everything an unauthenticated caller can
//!   make the server allocate, and the bounds are enforced while reading
//!   rather than after.
//! * [`openapi::document`] is that same table as an OpenAPI 3.1 description,
//!   generated from [`routes::ROUTES`] when it is asked for rather than
//!   checked in. There is nothing to update when a route is added, and no
//!   second list of routes to disagree with the first.
//! * [`console::Console`] serves the nine-view operator console. It reads and
//!   it can trip the kill switch; it has no handler that clears one, because
//!   clearing requires an operator credential a page cannot establish.
//! * [`missing`] is the inventory of what the platform does not expose, with
//!   the reason for each. A console panel with nothing behind it names one of
//!   these rather than rendering a zero it never observed.
//! * [`stream::StreamKind`] is the live surface: five server-sent-event
//!   streams under `/api/v1/stream`, what feeds each of them, and exactly what
//!   a reconnecting client recovers. Nothing there is generated — four of the
//!   five are views over the platform's own event log and the fifth reports
//!   this process's observed health — because a dashboard cannot tell a
//!   plausible number from a measured one.
//!
//! Every response carries a content-security policy of `default-src 'none'`
//! with no script source at all. The web UI renders no JavaScript, so nothing
//! legitimate is broken by forbidding it entirely.

pub mod auth;
pub mod cells;
pub mod console;
pub mod feed;
pub mod http;
pub mod json;
pub mod ledger_views;
pub mod mesh;
pub mod missing;
pub mod openapi;
pub mod openobserve;
pub mod routes;
pub mod self_model_views;
pub mod stream;
pub mod trust;
pub mod web;

pub use auth::{Authenticator, Credential, Principal, RateLimiter, Role};
pub use cells::{CellObservation, CellRegistry};
pub use console::Console;
pub use feed::{ApiFeed, ConnectorSettings, FeedSettings, Sensed};
pub use http::{
    Handler, Method, Request, Response, ResponseStream, Server, ServerLimits, StreamDecision,
    StreamEnd, StreamOutcome, pump,
};
pub use mesh::{MeshBackbone, MeshSettings, MeshStatus, pending_capital};
pub use openapi::document;
pub use openobserve::{
    DrainHandle, ExportPass, OpenObserveConfig, SignalOutcome, export_once, record,
};
pub use routes::{Api, ROUTES, Route};
pub use stream::{EventSource, EventStream, SseEvent, StreamKind, StreamLimits};
pub use trust::{ENVELOPE_KEY_VARIABLE, KeyProvenance, harden_central};
pub use web::{Router, Web};
