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

use crate::boundary::{EdgeQuote, MAX_QUOTES, Offer, ReflexPort, StatusCells};
use crate::saga::{Saga, SagaStep};
use crate::wire::{PeerSession, PeerSettings, PeerTurn};
use qip_core::error::{Error, Result};
use qip_core::{Clock, Decimal, Timestamp};
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

/// How often every peer's clock is probed (MESH-007). Often enough that a
/// sample is always far younger than `qip_peer::CLOCK_SAMPLE_TTL`, and that
/// the tightest of the recent samples is a good one.
pub const PROBE_INTERVAL: qip_core::Duration = qip_core::Duration::from_millis(250);

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
    let session = PeerSession::with_clock(settings, Arc::clone(&clock))?;
    let local = session.link().local_addr();
    let (sender, offers) = sync_channel::<Offer>(PORT_QUEUE);
    let cells = Arc::new(StatusCells::default());
    let started = clock.now();
    let host = Host {
        session,
        offers,
        cells: Arc::clone(&cells),
        clock,
        saga: Saga::new(started),
        last_probe: None,
    };
    std::thread::Builder::new()
        .name("qip-peer-session".to_string())
        .spawn(move || host.run())
        .map_err(|error| Error::io(format!("cannot start the peer session: {error}")))?;
    Ok((ReflexPort::new(sender, cells), local))
}

/// The value of a quote for choosing which to publish when there are more
/// than the bound: the smaller side's notional at the touch, which is what a
/// peer could actually trade against it.
pub(crate) fn quote_value(
    bid: Decimal,
    bid_size: Decimal,
    ask: Decimal,
    ask_size: Decimal,
) -> Decimal {
    let bid_side = bid.checked_mul(bid_size).unwrap_or(Decimal::MAX);
    let ask_side = ask.checked_mul(ask_size).unwrap_or(Decimal::MAX);
    bid_side.min(ask_side)
}

/// The quotes the cell offered, as protocol edges: validated here, on the
/// session's side, and bounded at the protocol's limit. Returns the edges,
/// how many quotes were refused, and how many valid ones were withheld by
/// the bound — chosen by value, largest first, then by venue and
/// instrument, so the same books always make the cut and the cut is
/// counted rather than silent (MESH-5060).
fn edges(quotes: Vec<EdgeQuote>) -> (Vec<ExecutableEdge>, u64, u64) {
    let mut refused = 0u64;
    let mut edges: Vec<ExecutableEdge> = quotes
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
        .collect();
    let mut withheld = 0u64;
    if edges.len() > MAX_QUOTES {
        edges.sort_by(|a, b| {
            quote_value(b.bid, b.bid_size, b.ask, b.ask_size)
                .cmp(&quote_value(a.bid, a.bid_size, a.ask, a.ask_size))
                .then_with(|| a.venue.cmp(&b.venue))
                .then_with(|| a.instrument.cmp(&b.instrument))
        });
        withheld = (edges.len() - MAX_QUOTES) as u64;
        edges.truncate(MAX_QUOTES);
    }
    (edges, refused, withheld)
}

struct Host {
    session: PeerSession,
    offers: Receiver<Offer>,
    cells: Arc<StatusCells>,
    clock: Arc<dyn Clock>,
    saga: Saga,
    last_probe: Option<Timestamp>,
}

impl Host {
    fn run(mut self) {
        loop {
            let offered = match self.offers.recv_timeout(TURN) {
                // Only the newest offer matters: an edge state is a snapshot,
                // and a peer holding an older one is replaced by the next.
                // What every dropped offer withheld is still counted.
                Ok(first) => {
                    let mut newest = first;
                    for later in self.offers.try_iter() {
                        self.cells
                            .quotes_withheld
                            .fetch_add(newest.withheld, Ordering::Relaxed);
                        newest = later;
                    }
                    Some(newest)
                }
                Err(RecvTimeoutError::Timeout) => None,
                // The decision loop dropped its port: nothing more to carry.
                // Dropping the session stops and joins the wire's thread.
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let now: Timestamp = self.clock.now();
            let turn = match offered {
                Some(offer) => {
                    self.saga.authorise(offer.legs);
                    let (edges, refused, withheld) = edges(offer.quotes);
                    self.cells
                        .quotes_refused
                        .fetch_add(refused, Ordering::Relaxed);
                    self.cells
                        .quotes_withheld
                        .fetch_add(offer.withheld.saturating_add(withheld), Ordering::Relaxed);
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
            self.session.answer_probes(now);
            if self
                .last_probe
                .is_none_or(|at| now.since(at).as_nanos() >= PROBE_INTERVAL.as_nanos())
            {
                // A probe the endpoint cannot frame is not a probe; the next
                // interval tries again and the samples age out meanwhile.
                let _ = self.session.probe(now);
                self.last_probe = Some(now);
            }
            let step = self.saga.step(&mut self.session, now);
            self.record(&turn, &step, now);
        }
    }

    /// Store this turn into the cells the port copies. Counters the wire
    /// already keeps cumulatively are stored, not added, so the port reads
    /// the wire's own figure rather than a second count of the same fact.
    fn record(&self, turn: &PeerTurn, step: &SagaStep, now: Timestamp) {
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
            .clocks_measured
            .store(endpoint.clocked_peers(now).len() as u64, Ordering::Relaxed);
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
        for (cell, add) in [
            (&cells.cycles_originated, step.originated),
            (&cells.reservations, step.reservations),
            (&cells.declines, step.declines),
            (&cells.fires, step.fires),
            (&cells.legs_sent, step.legs_sent),
            (&cells.fills, step.fills),
            (&cells.unwinds, step.unwinds),
            (&cells.cycles_rejected, step.rejected),
        ] {
            cell.fetch_add(add, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(venue: &str, instrument: &str, size: i64) -> EdgeQuote {
        EdgeQuote {
            venue: venue.into(),
            instrument: instrument.into(),
            bid: Decimal::from_int(99),
            bid_size: Decimal::from_int(size),
            ask: Decimal::from_int(101),
            ask_size: Decimal::from_int(size),
            as_of_ns: 1,
        }
    }

    /// MESH-5060: past the bound the session keeps the most valuable books,
    /// in a fixed order, and counts the rest — it used to `take` the first
    /// sixty-four in whatever order arrived and say nothing.
    ///
    /// Mutation: restore `.take(MAX_QUOTES)` without the sort and count and
    /// the withheld figure and the kept set both fail.
    #[test]
    fn past_the_bound_the_most_valuable_books_are_kept_and_the_rest_are_counted() {
        let quotes: Vec<EdgeQuote> = (0..MAX_QUOTES as i64 + 3)
            .map(|i| quote("XLON", &format!("obj-{i:03}"), i + 1))
            .collect();
        assert!(
            quotes.len() > MAX_QUOTES,
            "premise: more quotes than the bound"
        );
        let (kept, refused, withheld) = edges(quotes);
        assert_eq!(refused, 0);
        assert_eq!(kept.len(), MAX_QUOTES);
        assert_eq!(withheld, 3);
        // The three smallest are the ones left out, wherever they arrived.
        for dropped in ["obj-000", "obj-001", "obj-002"] {
            assert!(
                kept.iter().all(|e| e.instrument != dropped),
                "{dropped} was kept over a more valuable book"
            );
        }
    }
}
