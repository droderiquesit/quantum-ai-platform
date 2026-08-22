//! `qip-api` — the HTTP surface.
//!
//! An in-tree HTTP/1.1 server, a versioned REST API, and the authentication in
//! front of both.
//!
//! Three things are worth reading before the code:
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
//!
//! Every response carries a content-security policy of `default-src 'none'`
//! with no script source at all. The web UI renders no JavaScript, so nothing
//! legitimate is broken by forbidding it entirely.

pub mod auth;
pub mod http;
pub mod json;
pub mod routes;
pub mod web;

pub use auth::{Authenticator, Credential, Principal, RateLimiter, Role};
pub use http::{Handler, Method, Request, Response, Server, ServerLimits};
pub use routes::{Api, ROUTES, Route};
pub use web::{Router, Web};
