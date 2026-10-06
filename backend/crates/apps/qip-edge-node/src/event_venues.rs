//! Event venue adapters on the hot path.
//!
//! Prediction markets and other event-based venues feed into the cell directly,
//! without going through the Fabric or regional services. Each venue holds its
//! own adapter, and the pass loop polls them per cycle.
//!
//! # Message format
//!
//! Event venues are modeled as L2 (level-based) venues, so their order books
//! are published as [`qip_contracts::message::MessageBody::LevelSet`] messages.
//! Each update is encoded and passed to [`Cell::on_bytes`] just like market data
//! from any other venue.

use qip_contracts::message::{BookSide, MessageBody};
use qip_contracts::venue::VenueId;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_prediction::adapter::{PredictionAdapter, PredictionUpdate};
use std::collections::BTreeMap;

/// A decoded event venue update ready to feed to the cell.
#[derive(Clone, Debug)]
pub struct EventVenueUpdate {
    pub venue: VenueId,
    pub messages: Vec<(Timestamp, MessageBody)>,
}

/// One event venue feeding directly into the cell.
pub struct EventVenue {
    adapter: Box<dyn PredictionAdapter>,
    /// Market ID → instrument object ID, for routing book updates to the cell.
    /// Built once at initialization so the decoder path in `feed.rs` can name
    /// the instrument the cell already holds through the same table.
    outcomes: BTreeMap<String, String>,
}

impl std::fmt::Debug for EventVenue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventVenue")
            .field("adapter", &self.adapter.descriptor().venue)
            .field("outcomes", &self.outcomes)
            .finish()
    }
}

impl EventVenue {
    /// Create a new event venue from an adapter.
    ///
    /// Panics if the adapter is not synthetic; only the simulated adapter is
    /// used in this build. A live venue is an architecture decision, not a
    /// configuration value (ADR 0003).
    pub fn new(mut adapter: Box<dyn PredictionAdapter>) -> Result<Self> {
        if !adapter.descriptor().is_synthetic {
            return Err(Error::denied(
                "only synthetic event venues are supported in this build; \
                 a live venue adapter is an architecture decision (ADR 0003)",
            ));
        }
        adapter.start(Timestamp::from_secs(0))?;
        Ok(Self {
            adapter,
            outcomes: BTreeMap::new(),
        })
    }

    /// Poll for updates since the last call, converting them to messages.
    pub fn poll(&mut self, until: Timestamp) -> Result<EventVenueUpdate> {
        let updates = self.adapter.poll(until)?;
        let venue = self.adapter.descriptor().venue.clone();
        let mut messages = Vec::new();

        for update in updates {
            match update {
                PredictionUpdate::MarketListed(market) => {
                    // Record the mapping of outcomes to their object IDs so
                    // we can route book updates to the right instrument.
                    for outcome in market.outcomes() {
                        self.outcomes.insert(
                            outcome.id.as_str().to_string(),
                            outcome.object_id.as_str().to_string(),
                        );
                    }
                }
                PredictionUpdate::Book {
                    outcome: _, book, ..
                } => {
                    // Convert order book levels to LevelSet messages.
                    // The book's bid and ask levels are converted per price level.
                    for bid in &book.bids {
                        messages.push((
                            until,
                            MessageBody::LevelSet {
                                side: BookSide::Bid,
                                price: bid.price,
                                quantity: bid.size,
                                order_count: bid.order_count.into(),
                            },
                        ));
                    }
                    for ask in &book.asks {
                        messages.push((
                            until,
                            MessageBody::LevelSet {
                                side: BookSide::Ask,
                                price: ask.price,
                                quantity: ask.size,
                                order_count: ask.order_count.into(),
                            },
                        ));
                    }
                }
                PredictionUpdate::Report { .. } => {
                    // Oracle reports resolve markets but don't update the order book.
                    // They're handled outside the pass loop via settlement logic.
                }
            }
        }

        Ok(EventVenueUpdate { venue, messages })
    }
}
