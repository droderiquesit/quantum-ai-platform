//! The HTTP/1.1 server, re-exported from `qip_transport::server`.
//!
//! ADR 0100 §1 moved the server itself to `qip-transport`, because
//! `qip-fabricd` and `qip-ledgerd` have to serve without depending on this
//! application crate, and a second hand-written HTTP parser living here would
//! be new attack surface duplicating the one already reviewed. This module
//! stays so that every `qip_api::http::…` path already in this crate's routes,
//! tests and doc comments keeps resolving without an edit — the move is
//! mechanical, not a rename callers have to chase.
pub use qip_transport::server::*;
