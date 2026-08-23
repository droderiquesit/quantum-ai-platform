//! The Fast Brain node's parts, separated from the process that runs them.
//!
//! `main.rs` is a composition root and nothing else. Everything with a
//! decision in it lives here, because a rule buried in `fn main` is a rule that
//! can only be checked by starting a process and watching it: the budget
//! arithmetic, the health response an operator reads at three in the morning,
//! and the environment parsing that decides what this node will do are each
//! reachable from a test that asserts the property directly.
//!
//! The one rule the whole node exists to keep — nothing on the fast path waits
//! for a language model — is in [`roster`], and it is checked before anything
//! in [`node`] is allowed to run.

pub mod config;
pub mod feed;
pub mod health;
pub mod node;
pub mod roster;
pub mod status;
