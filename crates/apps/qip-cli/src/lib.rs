//! The operator command line's composable parts.
//!
//! `main.rs` is the process: it reads the arguments, builds a platform and
//! prints. This library is what one of those commands is assembled *from*.
//!
//! Only [`demo`] lives here so far, and it is here for a specific reason: the
//! live demonstration decides things — which peer an adapter is pointed at,
//! whether an order was accepted, what a layer actually produced — and a
//! decision buried in a command handler is a decision no test can reach. Every
//! function in [`demo`] takes what it needs and returns what it did, so the
//! tests assert on the same values the operator reads.

pub mod demo;
