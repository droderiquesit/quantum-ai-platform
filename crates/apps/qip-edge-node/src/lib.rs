//! The edge cell node's composable parts.
//!
//! `main.rs` is the process: it reads the environment, refuses to start
//! without what a cell needs, and serves the health surface. This library is
//! what that process is assembled *from*, exposed so the pieces can be tested
//! against the same types the binary uses rather than against a copy of them.
//!
//! Today that is one module, and it is the one worth testing separately: the
//! venue seam, where the cell's orders meet a matching engine.

pub mod gateway;
