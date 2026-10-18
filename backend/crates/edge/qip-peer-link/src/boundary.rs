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
//!
//! # How a cross-region cycle starts without the loop waiting for it
//!
//! A cross-region opportunity needs two facts the loop cannot hold together:
//! this cell's touch, which only the decision thread can read, and a peer's,
//! which only the session thread receives. The loop does not receive — the
//! port has no method that could — so the decision is split where the facts
//! are. The decision thread runs the cell's own gate and publishes, with its
//! touch, a [`LegAuthority`] for each book: the most this cell will buy or
//! sell there as one leg of a peer cycle, at what limit, until when. That is
//! the cell's intent, decided and bounded locally. The session thread, which
//! holds the peers' edge states, pairs an authority with a peer's touch that
//! crosses it, and runs the protocol — token, reservation, FIRE, fill or
//! unwind — reserving and sending only inside the authority the cell last
//! published. What happened comes back as counters in [`PeerStatus`], which
//! the loop reads without waiting and nothing on the decision thread decides
//! on. Execution is paper: a leg fills against the touch its authority was
//! priced from, on the session thread, and never reaches a venue.

use qip_core::Decimal;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

/// The most quotes one publish carries before the session thread truncates
/// it: the protocol's bound on one edge state, so the node can trim before
/// it offers rather than ship a vector the far side will cut.
pub const MAX_QUOTES: usize = qip_peer::MAX_EDGES_PER_STATE;

/// What the decision loop hands the session each turn: the touch it offers
/// its peers, and the legs its gate authorises.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    pub quotes: Vec<EdgeQuote>,
    /// Empty from a halted cell, a cell with no capital free, or a cell that
    /// publishes its touch and coordinates nothing.
    pub legs: Vec<LegAuthority>,
    /// Books the cell holds a two-sided price for that this offer left out
    /// because [`MAX_QUOTES`] were already chosen (MESH-5060). Counted into
    /// [`PeerStatus::quotes_withheld`] so a partial view is never silent.
    pub withheld: u64,
}

impl From<Vec<EdgeQuote>> for Offer {
    fn from(quotes: Vec<EdgeQuote>) -> Self {
        Self {
            quotes,
            legs: Vec::new(),
            withheld: 0,
        }
    }
}

/// Which way an authorised leg trades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Buy,
    Sell,
}

/// The most this cell will trade as one leg of a peer-coordinated cycle at
/// one book, as its own gate decided on the decision thread this turn.
///
/// The session thread may reserve and send a leg only inside the newest
/// authority for that venue, instrument and side, and only before
/// `valid_until_ns`: an authority is the cell's decision, and the session is
/// never the one to widen it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegAuthority {
    pub venue: String,
    pub instrument: String,
    pub side: Side,
    /// The touch the leg would trade at: the ask for a buy, the bid for a
    /// sell. A leg priced worse than this is refused.
    pub limit_price: Decimal,
    /// The most one leg may be: the region capital the cell has free, at
    /// this price. The gate refuses a larger leg at reservation and again at
    /// sending.
    pub max_size: Decimal,
    /// The size the touch shows. A leg sent in paper fills up to this and
    /// no further, so a touch that shrank after the cycle was priced is a
    /// partial fill, and the cycle has to resize or unwind (MESH-5065).
    pub touch_size: Decimal,
    /// What crossing back out costs per unit: the spread at this book. A
    /// cycle whose declared unwind loss does not cover it is declined.
    pub unwind_per_unit: Decimal,
    /// After this, on the cell's clock, the authority authorises nothing.
    pub valid_until_ns: i64,
}

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
    /// Books left out of an offer because the bound was reached, summed
    /// over every offer (MESH-5060).
    pub quotes_withheld: u64,
    /// Peers whose clock this node has a fresh measurement of (MESH-007).
    pub clocks_measured: u64,
    /// The shortest round trip, in nanoseconds, among the fresh clock
    /// samples across every peer: how far the nearest peer is, net of its
    /// own holding time. Meaningful only while `clocks_measured` is
    /// non-zero; zero otherwise, and a measured round trip is never zero
    /// because the endpoint refuses a negative one and a wire cannot answer
    /// in no time.
    pub clock_round_trip_ns: u64,
    /// Cycles this node opened as coordinator.
    pub cycles_originated: u64,
    /// Times this node reserved its own legs of an epoch.
    pub reservations: u64,
    /// Times this node's gate or book declined its legs of an epoch.
    pub declines: u64,
    /// FIREs this node declared as coordinator.
    pub fires: u64,
    /// This node's legs sent after FIRE, through its gate.
    pub legs_sent: u64,
    /// This node's legs filled, in paper.
    pub fills: u64,
    /// This node's filled legs unwound because a counterparty filled less.
    pub unwinds: u64,
    /// Cycles rejected at the end of their TTL for want of a guarantee.
    pub cycles_rejected: u64,
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
    pub(crate) quotes_withheld: AtomicU64,
    pub(crate) clocks_measured: AtomicU64,
    pub(crate) clock_round_trip_ns: AtomicU64,
    pub(crate) cycles_originated: AtomicU64,
    pub(crate) reservations: AtomicU64,
    pub(crate) declines: AtomicU64,
    pub(crate) fires: AtomicU64,
    pub(crate) legs_sent: AtomicU64,
    pub(crate) fills: AtomicU64,
    pub(crate) unwinds: AtomicU64,
    pub(crate) cycles_rejected: AtomicU64,
}

/// The decision loop's whole view of the peer link.
#[derive(Debug)]
pub struct ReflexPort {
    offers: SyncSender<Offer>,
    cells: Arc<StatusCells>,
}

impl ReflexPort {
    pub(crate) fn new(offers: SyncSender<Offer>, cells: Arc<StatusCells>) -> Self {
        Self { offers, cells }
    }

    /// Offer this cell's touch, and the legs its gate authorises, to the
    /// session. Never waits: `false` is a full queue or a session that has
    /// stopped, and is counted in [`PeerStatus::dropped_offers`]. A dropped
    /// offer costs one edge state and one turn's authorities; the next
    /// turn's supersede them anyway, and an authority the session already
    /// holds lapses at its own expiry rather than outliving the cell's view.
    pub fn try_publish(&self, offer: impl Into<Offer>) -> bool {
        match self.offers.try_send(offer.into()) {
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
            quotes_withheld: cells.quotes_withheld.load(Ordering::Relaxed),
            clocks_measured: cells.clocks_measured.load(Ordering::Relaxed),
            clock_round_trip_ns: cells.clock_round_trip_ns.load(Ordering::Relaxed),
            cycles_originated: cells.cycles_originated.load(Ordering::Relaxed),
            reservations: cells.reservations.load(Ordering::Relaxed),
            declines: cells.declines.load(Ordering::Relaxed),
            fires: cells.fires.load(Ordering::Relaxed),
            legs_sent: cells.legs_sent.load(Ordering::Relaxed),
            fills: cells.fills.load(Ordering::Relaxed),
            unwinds: cells.unwinds.load(Ordering::Relaxed),
            cycles_rejected: cells.cycles_rejected.load(Ordering::Relaxed),
        }
    }
}
