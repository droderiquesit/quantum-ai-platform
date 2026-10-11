//! The one thing the reflex node's decision loop may hold of Lane 1
//! (ADR 0116 §3).
//!
//! The failure this prevents is the one Lane 1 exists to make impossible: a
//! reflex pass that waits for a peer. That pass compiles, passes every test
//! run with the peer present, and stops deciding the moment a region is
//! partitioned, which is when a cell most needs to decide alone (MESH-002).
//! Making that impossible by review is a promise; making it impossible by
//! what the decision loop can *hold* is a property. So the loop holds a
//! [`ReflexPort`] and nothing else of the peer link, and the port has two
//! methods, neither of which can wait:
//!
//! * [`ReflexPort::try_publish`] is a `try_send` into a bounded queue. A full
//!   queue is a counted drop, never a wait.
//! * [`ReflexPort::status`] copies atomics the session thread writes. No lock
//!   is taken.
//!
//! The port has no method that receives, joins, locks, waits or sleeps, and
//! no method that returns a coordination type; `mesh_lane.rs` (MESH-013)
//! reads this file and fails if one appears. A reflex decision that seems to
//! need a peer's answer inside the pass gets that answer on a later pass,
//! from the cell's own state, or argues the need in an ADR against MESH-002.
//! A blocking method here is not the answer.
//!
//! [`EdgeQuote`] is plain data and not `qip_peer::ExecutableEdge`, so the
//! node's decision loop names no protocol type at all. The session thread
//! converts and validates it on its own side.

use qip_core::Decimal;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

/// The most quotes one publish carries before the session thread truncates
/// it: the protocol's bound on one edge state, so the node can trim before
/// it offers rather than ship a vector the far side will cut.
pub const MAX_QUOTES: usize = qip_peer::MAX_EDGES_PER_STATE;

/// The touch of one book: price and size each side, and when the book last
/// showed it. Never depth (MESH-5060).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeQuote {
    pub venue: String,
    pub instrument: String,
    pub bid: Decimal,
    pub bid_size: Decimal,
    pub ask: Decimal,
    pub ask_size: Decimal,
    /// When the node's book last showed this touch, in nanoseconds; zero
    /// when the book has never been updated.
    pub as_of_ns: i64,
}

/// What the peer link has done, as of the session thread's last turn. A copy:
/// it is one turn behind the wire, and nothing decides on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerStatus {
    /// Session turns completed on the session thread.
    pub turns: u64,
    /// Peer frames read off the wire and given to the endpoint.
    pub frames_received: u64,
    /// Of those, frames the endpoint refused.
    pub frames_refused: u64,
    /// Peers whose executable edge state this node holds.
    pub edge_peers_held: u64,
    /// Refusals the endpoint has journaled.
    pub refusals: u64,
    /// Frame copies written to a peer's socket.
    pub sent: u64,
    /// Writes or connects that failed.
    pub send_failures: u64,
    /// Frames the wire dropped because its own outbound queue was full.
    pub wire_drops: u64,
    /// Frames read while the inbound queue was full.
    pub inbound_drops: u64,
    /// Publishes this port refused because its queue was full or the
    /// session had stopped.
    pub dropped_offers: u64,
    /// Quotes the session thread refused as crossed, locked or empty.
    pub quotes_refused: u64,
    /// Edge states the endpoint could not frame.
    pub publish_failures: u64,
}

/// The cells [`PeerStatus`] is copied from. The session thread stores into
/// them; the port only loads.
#[derive(Debug, Default)]
pub(crate) struct StatusCells {
    pub(crate) turns: AtomicU64,
    pub(crate) frames_received: AtomicU64,
    pub(crate) frames_refused: AtomicU64,
    pub(crate) edge_peers_held: AtomicU64,
    pub(crate) refusals: AtomicU64,
    pub(crate) sent: AtomicU64,
    pub(crate) send_failures: AtomicU64,
    pub(crate) wire_drops: AtomicU64,
    pub(crate) inbound_drops: AtomicU64,
    pub(crate) dropped_offers: AtomicU64,
    pub(crate) quotes_refused: AtomicU64,
    pub(crate) publish_failures: AtomicU64,
}

/// The decision loop's whole view of the peer link.
#[derive(Debug)]
pub struct ReflexPort {
    quotes: SyncSender<Vec<EdgeQuote>>,
    cells: Arc<StatusCells>,
}

impl ReflexPort {
    pub(crate) fn new(quotes: SyncSender<Vec<EdgeQuote>>, cells: Arc<StatusCells>) -> Self {
        Self { quotes, cells }
    }

    /// Offer this cell's touch to its peers. Never waits: `false` is a full
    /// queue or a session that has stopped, and is counted in
    /// [`PeerStatus::dropped_offers`]. A dropped offer costs one edge state;
    /// the next turn's supersedes it anyway.
    pub fn try_publish(&self, quotes: Vec<EdgeQuote>) -> bool {
        match self.quotes.try_send(quotes) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.cells.dropped_offers.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// The link's counters as the session thread last stored them.
    pub fn status(&self) -> PeerStatus {
        let cells = &self.cells;
        PeerStatus {
            turns: cells.turns.load(Ordering::Relaxed),
            frames_received: cells.frames_received.load(Ordering::Relaxed),
            frames_refused: cells.frames_refused.load(Ordering::Relaxed),
            edge_peers_held: cells.edge_peers_held.load(Ordering::Relaxed),
            refusals: cells.refusals.load(Ordering::Relaxed),
            sent: cells.sent.load(Ordering::Relaxed),
            send_failures: cells.send_failures.load(Ordering::Relaxed),
            wire_drops: cells.wire_drops.load(Ordering::Relaxed),
            inbound_drops: cells.inbound_drops.load(Ordering::Relaxed),
            dropped_offers: cells.dropped_offers.load(Ordering::Relaxed),
            quotes_refused: cells.quotes_refused.load(Ordering::Relaxed),
            publish_failures: cells.publish_failures.load(Ordering::Relaxed),
        }
    }
}
