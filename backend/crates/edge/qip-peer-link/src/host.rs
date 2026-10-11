//! The session thread: where the peer endpoint lives in production
//! (ADR 0116 §2).
//!
//! Until ADR 0116 the reflex node's decision thread drove the session
//! itself, once per turn: it received peer frames into the endpoint, framed
//! the cell's edge state and expired epochs. The queues were non-blocking,
//! but that work was coordination done inside the loop whose budget is the
//! reflex pass's. Here a thread of its own owns the [`PeerSession`] — and so
//! the endpoint and, through the session's [`crate::PeerLink`], the wire —
//! and the decision loop holds only the [`ReflexPort`] [`spawn`] returns.
//!
//! This thread may wait; it is Lane 1. It waits on the quote queue for at
//! most [`TURN`], so a cell that offers nothing still has its peers' frames
//! read and its epochs expired on time.

use crate::boundary::{EdgeQuote, MAX_QUOTES, ReflexPort, StatusCells};
use crate::wire::{PeerSession, PeerSettings, PeerTurn};
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_peer::ExecutableEdge;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};

/// Publishes the port holds before it counts a drop. Small on purpose: an
/// edge state is a snapshot and the newest supersedes the rest, so a deep
/// queue would only carry stale touches to peers.
const PORT_QUEUE: usize = 4;

/// The longest the session thread goes without reading its peers or expiring
/// an epoch when the cell offers nothing.
pub const TURN: std::time::Duration = std::time::Duration::from_millis(10);

/// Bind the peer listener, start the wire, and move the session onto a
/// thread of its own. Returns the port the decision loop holds and the
/// address actually bound.
///
/// The bind happens here, on the caller's thread, so a listener that cannot
/// be bound stops the process at start-up rather than surfacing later as a
/// session that never ran. The thread is not joined by anything the
/// decision loop holds: it ends when the port is dropped, and joins the
/// wire's thread on its own way out.
pub fn spawn(settings: &PeerSettings, clock: Arc<dyn Clock>) -> Result<(ReflexPort, SocketAddr)> {
    let session = PeerSession::new(settings)?;
    let local = session.link().local_addr();
    let (sender, quotes) = sync_channel::<Vec<EdgeQuote>>(PORT_QUEUE);
    let cells = Arc::new(StatusCells::default());
    let host = Host {
        session,
        quotes,
        cells: Arc::clone(&cells),
        clock,
    };
    std::thread::Builder::new()
        .name("qip-peer-session".to_string())
        .spawn(move || host.run())
        .map_err(|error| Error::io(format!("cannot start the peer session: {error}")))?;
    Ok((ReflexPort::new(sender, cells), local))
}

/// The quotes the cell offered, as protocol edges: validated here, on the
/// session's side, and bounded at the protocol's limit. Returns the edges
/// and how many quotes were refused.
fn edges(quotes: Vec<EdgeQuote>) -> (Vec<ExecutableEdge>, u64) {
    let mut refused = 0u64;
    let edges = quotes
        .into_iter()
        .filter_map(|quote| {
            let edge = ExecutableEdge {
                venue: quote.venue,
                instrument: quote.instrument,
                bid: quote.bid,
                bid_size: quote.bid_size,
                ask: quote.ask,
                ask_size: quote.ask_size,
                as_of_ns: quote.as_of_ns,
            };
            if edge.validate().is_ok() {
                Some(edge)
            } else {
                refused += 1;
                None
            }
        })
        .take(MAX_QUOTES)
        .collect();
    (edges, refused)
}

struct Host {
    session: PeerSession,
    quotes: Receiver<Vec<EdgeQuote>>,
    cells: Arc<StatusCells>,
    clock: Arc<dyn Clock>,
}

impl Host {
    fn run(mut self) {
        loop {
            let offered = match self.quotes.recv_timeout(TURN) {
                // Only the newest offer matters: an edge state is a snapshot,
                // and a peer holding an older one is replaced by the next.
                Ok(first) => Some(self.quotes.try_iter().last().unwrap_or(first)),
                Err(RecvTimeoutError::Timeout) => None,
                // The decision loop dropped its port: nothing more to carry.
                // Dropping the session stops and joins the wire's thread.
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let now: Timestamp = self.clock.now();
            let turn = match offered {
                Some(quotes) => {
                    let (edges, refused) = edges(quotes);
                    self.cells
                        .quotes_refused
                        .fetch_add(refused, Ordering::Relaxed);
                    match self.session.turn(edges, now) {
                        Ok(turn) => turn,
                        // Unreachable in practice — the edges were validated
                        // and bounded above — but if the endpoint refuses to
                        // frame them the refusal is counted, not swallowed;
                        // that turn's received-frame count is lost with it.
                        Err(_) => {
                            self.cells.publish_failures.fetch_add(1, Ordering::Relaxed);
                            PeerTurn::default()
                        }
                    }
                }
                None => self.session.listen(now),
            };
            self.record(&turn);
        }
    }

    /// Store this turn into the cells the port copies. Counters the wire
    /// already keeps cumulatively are stored, not added, so the port reads
    /// the wire's own figure rather than a second count of the same fact.
    fn record(&self, turn: &PeerTurn) {
        let cells = &self.cells;
        cells.turns.fetch_add(1, Ordering::Relaxed);
        cells
            .frames_received
            .fetch_add(turn.received as u64, Ordering::Relaxed);
        cells
            .frames_refused
            .fetch_add(turn.refused as u64, Ordering::Relaxed);
        let endpoint = self.session.endpoint();
        cells
            .edge_peers_held
            .store(endpoint.edge_peers().len() as u64, Ordering::Relaxed);
        cells
            .refusals
            .store(self.session.refusals().len() as u64, Ordering::Relaxed);
        let wire = self.session.link().stats();
        cells.sent.store(wire.sent, Ordering::Relaxed);
        cells
            .send_failures
            .store(wire.send_failures, Ordering::Relaxed);
        cells.wire_drops.store(wire.queued_drops, Ordering::Relaxed);
        cells
            .inbound_drops
            .store(wire.inbound_drops, Ordering::Relaxed);
    }
}
