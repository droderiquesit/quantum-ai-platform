//! The edge cell node's composable parts.
//!
//! `main.rs` is the process: it reads the environment, refuses to start
//! without what a cell needs, and serves the health surface. This library is
//! what that process is assembled *from*, exposed so the pieces can be tested
//! against the same types the binary uses rather than against a copy of them.
//!
//! Two modules, and both are seams: the venue seam, where the cell's orders
//! meet a matching engine, and the durability seam, where the cell's decision
//! record leaves the process. Each is somewhere the node could look healthy
//! while doing nothing, so each is exercised against the types the binary
//! actually uses.

pub mod gateway;
pub mod mirror;
