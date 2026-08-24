//! The edge cell node's composable parts.
//!
//! `main.rs` is the process: it reads the environment, refuses to start
//! without what a cell needs, and serves the health surface. This library is
//! what that process is assembled *from*, exposed so the pieces can be tested
//! against the same types the binary uses rather than against a copy of them.
//!
//! Four modules, and all of them are seams: the venue seam, where the cell's
//! orders meet a matching engine; the *selection* of that venue, which is the
//! one decision in this binary whose wrong answer is not recoverable; the
//! durability seam, where the cell's decision record leaves the process; and
//! the mesh seam, where the cell's state reaches the central plane and the
//! central plane's signed capital reaches the cell. Each is somewhere the node
//! could look healthy while doing nothing — or, in the venue's case, while
//! doing something nobody asked for — so each is exercised against the types
//! the binary actually uses.

pub mod gateway;
pub mod mesh;
pub mod mirror;
pub mod venue;
