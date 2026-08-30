//! The Deep Brain node's parts, separated from the process that runs them.
//!
//! `main.rs` is a composition root and nothing else, for the same reason it is
//! one in `qip-fastbrain`: a rule buried in `fn main` can only be checked by
//! starting a process and watching it. The cadence arithmetic, the readiness
//! decision an operator reads at three in the morning, and the environment
//! parsing that decides what this node will do are each reachable from a test
//! that asserts the property directly.
//!
//! # Why this node is not the fast brain with different numbers
//!
//! The two nodes share a shape — bounded run loop, blocking health surface,
//! loopback quiesce, documented flush — and share almost none of their
//! reasoning, because they are answerable for opposite things.
//!
//! The fast brain's whole guarantee is a *ceiling*: a cycle that takes too long
//! has failed at the thing it exists to do, so its budget may be tightened by
//! configuration and never raised, and a run of breaches takes it out of
//! rotation.
//!
//! The deep brain has no such ceiling and must not pretend to. Research, causal
//! reasoning, simulation and optimisation take as long as they take, and a
//! cycle here may legitimately call a language model and block for minutes. So
//! the one-directional guard runs the *other way*: [`config`] enforces a floor
//! under the cycle interval, because the failure mode worth refusing at
//! start-up is a deep brain configured with a fast brain's cadence — a loop
//! that spins, bills for model calls and finishes nothing.
//!
//! That difference propagates into readiness. In [`status`] a slow cycle is
//! never a reason to report unready and never a reason to fail liveness; the
//! reasons are that the node is stopping, halted, has not finished a first
//! cycle yet, has stopped finishing cycles at all, or keeps failing them. See
//! [`status::Unready`], where each is written down next to why.
//!
//! The rule this node exists to enforce — that nothing it hosts can reach a
//! venue — is in [`roster`], and it is checked before anything in [`node`] is
//! allowed to run.

pub mod config;
pub mod evolution;
pub mod health;
pub mod learning;
pub mod node;
pub mod roster;
pub mod status;
pub mod succession;
pub mod trust;
