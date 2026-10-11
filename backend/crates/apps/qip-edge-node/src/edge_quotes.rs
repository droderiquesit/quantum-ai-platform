//! What this cell offers its peers: the touch of each book it holds
//! (MESH-5060), as plain data for the peer link's port (ADR 0116).
//!
//! Built on the decision thread, because only that thread holds the cell,
//! and handed over with `ReflexPort::try_publish`, which never waits. It is
//! plain data on purpose: this module, like every node module but
//! `main.rs`, may name the peer link only through `qip_peer_link::boundary`,
//! and may not name a protocol type at all — `mesh_lane.rs` (MESH-013)
//! refuses it — so the decision loop holds nothing it could wait on a peer
//! through. The session thread validates and frames on its own side.

use qip_core::Timestamp;
use qip_edge::cell::Cell;
use qip_peer_link::boundary::{EdgeQuote, MAX_QUOTES};

/// The touch of every book the cell holds a two-sided price for, in the
/// cell's own order, at most [`MAX_QUOTES`] of them. Never depth: a peer
/// pricing a cycle needs what it could trade against, and a book it cannot
/// act on is bytes across a region boundary for nothing.
pub fn executable_quotes(cell: &Cell) -> Vec<EdgeQuote> {
    cell.liquidity()
        .iter()
        .filter_map(|state| {
            let bid = state.best_bid()?;
            let ask = state.best_ask()?;
            Some(EdgeQuote {
                venue: state.venue().as_str().to_string(),
                instrument: state.object_id().as_str().to_string(),
                bid: bid.price,
                bid_size: bid.size,
                ask: ask.price,
                ask_size: ask.size,
                as_of_ns: state.last_update().map_or(0, Timestamp::as_nanos),
            })
        })
        .take(MAX_QUOTES)
        .collect()
}
