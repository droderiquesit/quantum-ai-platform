//! The peer Reflex Mesh's link: the wire, the session, and the one port the
//! reflex node's decision loop meets them through (ADR 0116).
//!
//! Registered in Lane 1, and hosted by `qip-edge-node`, which must host it
//! because blueprint §16 requires the peer hop to be node to node. Before
//! ADR 0116 all of this was `qip_edge_node::peer`, and the session ran on
//! the decision thread taking the cell itself; that made coordination part
//! of the reflex budget, and with the protocol then in Lane 0
//! `qip-contracts`, MESH-013 failed on it, correctly.
//!
//! The crate sits under `edge/` and not `libs/` because it owns a
//! `TcpListener` and `TcpStream`, under mutual TLS off-host (ADR 0112): a library that opens a socket is a
//! service in the wrong directory (`00-boundaries.md`, enforced by
//! `only_the_named_libraries_open_sockets`). Its dependencies are `qip-peer`
//! and `qip-core`; it links no cell, so nothing here can reach `Cell::work`.
//!
//! * [`wire`] — settings, the wire thread ([`PeerLink`]) and [`PeerSession`],
//!   public for Lane 1 callers and tests.
//! * [`spawn`] — moves a session onto a thread of its own and returns the
//!   port. Called by the node's composition root and nothing else there.
//! * [`boundary`] — [`boundary::ReflexPort`], the decision loop's whole view:
//!   `try_publish` and `status`, neither of which can wait.
//! * [`saga`] — the peer cycle on the session thread: origination from the
//!   cell's leg authorities and peers' edge states, reservation, FIRE,
//!   paper execution, fill and unwind.

pub mod boundary;
mod host;
pub mod saga;
pub mod wire;

pub use host::{PROBE_INTERVAL, TURN, spawn};
pub use wire::{
    CLOCK_BOUND_VARIABLE, DEFAULT_CLOCK_BOUND, InboundFrame, LISTEN_VARIABLE, MAX_CLOCK_BOUND,
    MAX_FRAME, PEERS_VARIABLE, PeerLink, PeerSession, PeerSettings, PeerTurn, PeerWireStats,
    limits_with, node_limits,
};
