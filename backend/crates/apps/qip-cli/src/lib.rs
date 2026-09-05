//! The operator command line's composable parts.
//!
//! `main.rs` is the process: it reads the arguments, builds a platform and
//! prints. This library is what one of those commands is assembled *from*.
//!
//! [`demo`] is here for a specific reason: the live demonstration decides
//! things — which peer an adapter is pointed at, whether an order was
//! accepted, what a layer actually produced — and a decision buried in a
//! command handler is a decision no test can reach. Every function in
//! [`demo`] takes what it needs and returns what it did, so the tests assert
//! on the same values the operator reads.
//!
//! [`registrations`] and [`replay`] are here for the same reason and one
//! more: each ends in an exit code a deployment script gates on, and an exit
//! code decided inside `main` is a decision that can only be tested by
//! running a process. Each returns the lines *and* the code it would exit
//! with, so a test can assert both, and `main` is left with nothing to do
//! but print and exit.

pub mod demo;
pub mod registrations;
pub mod replay;
