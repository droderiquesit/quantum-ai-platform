//! Clock-qualified ordering of a fused reaction (R12-TICK-1177).
//!
//! [`qip_kernel::fusion::fuse`] reads a reaction as "the market moved
//! `first_move_after` the platform learned of the event". That subtraction
//! treats the tick's stamp as exact. A capture clock that was only known to
//! within a second cannot support a claim that a move 400 ms after the event
//! came *after* it, and a reaction credited on that order trains the belief on
//! a sequence nobody measured.
//!
//! This module relates each reaction's two instants through
//! [`qip_sequencing::align`]: the event's, which the platform stamped on its
//! own clock and so carries no uncertainty of its own, and the market move's,
//! which carries the capture-clock uncertainty of the tick it was read from.
//! A move within the combined uncertainty is [`Order::Unordered`], reported as
//! such, never rounded into "after".

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_kernel::fusion::FusedState;
use qip_sequencing::align::{Order, Placed, align};
use serde::{Deserialize, Serialize};

/// What the clocks support about one reaction's order against its event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionTiming {
    pub event: String,
    pub instrument: String,
    /// Whether the move demonstrably followed the event
    /// ([`Order::Before`] is "the event came first"), or the clocks cannot say.
    pub order: Order,
    /// Event to move, as stamped.
    pub lag: Duration,
    /// The true lag lies within `lag ± lag_uncertainty`.
    pub lag_uncertainty: Duration,
}

/// Relate every fused reaction that moved to its event, using the
/// capture-clock uncertainty of the latest tick knowable at or before the
/// move (`market_clock` is each tick's `(knowable_at, uncertainty)`, in any
/// order). A reaction with no tick at or before its move has no stated
/// uncertainty, and is left out rather than ordered as if it were exact.
pub fn order_reactions(
    fused: &FusedState,
    market_clock: &[(Timestamp, Duration)],
) -> Result<Vec<ReactionTiming>> {
    let mut out = Vec::new();
    for event in &fused.events {
        for reaction in &event.reactions {
            let Some(after) = reaction.first_move_after else {
                continue;
            };
            let moved_at = event.knowable_at.saturating_add(after);
            let Some((_, uncertainty)) = market_clock
                .iter()
                .filter(|(at, _)| *at <= moved_at)
                .max_by_key(|(at, _)| *at)
            else {
                continue;
            };
            let learned = Placed::new(event.knowable_at, Duration::ZERO, event.knowable_at)?;
            let moved = Placed::new(moved_at, *uncertainty, moved_at)?;
            let relation = align(&learned, &moved);
            out.push(ReactionTiming {
                event: event.event.clone(),
                instrument: reaction.instrument.clone(),
                order: relation.order,
                lag: relation.lag,
                lag_uncertainty: relation.lag_uncertainty,
            });
        }
    }
    Ok(out)
}
