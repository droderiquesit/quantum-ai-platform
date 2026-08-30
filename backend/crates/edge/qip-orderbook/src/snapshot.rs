//! Comparable pictures of venue state.
//!
//! A snapshot exists so replay can prove itself: apply a message stream to a
//! fresh book, apply it again, and the two snapshots must be identical. Value
//! equality is the primitive; [`BookSnapshot::digest`] exists for the case
//! where the comparison happens across a log or a process boundary and only a
//! fingerprint travels.
//!
//! Snapshots are plain data with no back-reference to the book they came from.
//! Taking one costs a walk of the levels and nothing else, so a consumer can
//! keep the last known-good picture without pinning the live structure.

use crate::auction::AuctionState;
use crate::venue::LastTrade;
use crate::view::Level;
use qip_contracts::VenueStatus;
use qip_core::{Decimal, sha256_hex};
use serde::{Deserialize, Serialize};

/// Which resolution a book was built from.
///
/// Carried on the snapshot because two books with identical levels are not the
/// same book: only one of them can answer a queue-position question, and a
/// consumer comparing across a replay boundary needs to know it is comparing
/// like with like.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BookKind {
    /// Every resting order tracked by reference.
    OrderByOrder,
    /// Aggregated price levels.
    Aggregated,
}

impl BookKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::OrderByOrder => "order_by_order",
            Self::Aggregated => "aggregated",
        }
    }

    /// Whether books of this kind can answer a queue-position question.
    pub const fn tracks_orders(&self) -> bool {
        matches!(self, Self::OrderByOrder)
    }
}

/// The book at a moment, as levels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookSnapshot {
    pub kind: BookKind,
    /// Bids, dearest first.
    pub bids: Vec<Level>,
    /// Asks, cheapest first.
    pub asks: Vec<Level>,
    /// Orders tracked individually; zero for an aggregated book.
    pub resting_orders: usize,
}

impl BookSnapshot {
    /// A fingerprint over every level, in book order.
    ///
    /// Over the raw fixed-point integers rather than a formatted rendering, so
    /// the digest cannot change because a display convention did.
    pub fn digest(&self) -> String {
        let mut bytes = Vec::with_capacity(64 + (self.bids.len() + self.asks.len()) * 36);
        bytes.push(match self.kind {
            BookKind::OrderByOrder => 3u8,
            BookKind::Aggregated => 2u8,
        });
        bytes.extend_from_slice(&(self.resting_orders as u64).to_le_bytes());
        for (tag, side) in [(0u8, &self.bids), (1u8, &self.asks)] {
            bytes.push(tag);
            bytes.extend_from_slice(&(side.len() as u64).to_le_bytes());
            for level in side {
                bytes.extend_from_slice(&level.price.raw().to_le_bytes());
                bytes.extend_from_slice(&level.size.raw().to_le_bytes());
                bytes.extend_from_slice(&level.order_count.to_le_bytes());
            }
        }
        sha256_hex(&bytes)
    }

    /// Total size on both sides.
    pub fn total_size(&self) -> Decimal {
        self.bids.iter().map(|l| l.size).sum::<Decimal>()
            + self.asks.iter().map(|l| l.size).sum::<Decimal>()
    }

    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }
}

/// Everything a venue's state holds at a moment.
///
/// The book alone is not the state: a book that is empty because the venue is
/// closed, and a book that is empty because it was thrown away after a gap,
/// have the same levels and completely different meanings. `awaiting_snapshot`
/// is what separates them, and it travels with the levels for that reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueSnapshot {
    pub status: VenueStatus,
    /// The book is known to be wrong and has not been rebuilt.
    pub awaiting_snapshot: bool,
    pub book: BookSnapshot,
    pub session_volume: Decimal,
    pub session_notional: Decimal,
    pub trade_count: u64,
    pub last_trade: Option<LastTrade>,
    pub auction: Option<AuctionState>,
    /// Messages applied since the state was created.
    pub applied: u64,
}

impl VenueSnapshot {
    /// A fingerprint over the whole state.
    ///
    /// Folds in the fields the book digest cannot see, so a replay divergence
    /// in session volume or venue status is caught by the same comparison that
    /// catches a divergence in the levels.
    pub fn digest(&self) -> String {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(self.book.digest().as_bytes());
        bytes.extend_from_slice(self.status.as_str().as_bytes());
        bytes.push(u8::from(self.awaiting_snapshot));
        bytes.extend_from_slice(&self.session_volume.raw().to_le_bytes());
        bytes.extend_from_slice(&self.session_notional.raw().to_le_bytes());
        bytes.extend_from_slice(&self.trade_count.to_le_bytes());
        bytes.extend_from_slice(&self.applied.to_le_bytes());
        if let Some(trade) = &self.last_trade {
            bytes.extend_from_slice(&trade.price.raw().to_le_bytes());
            bytes.extend_from_slice(&trade.quantity.raw().to_le_bytes());
            bytes.extend_from_slice(&trade.at.as_nanos().to_le_bytes());
        }
        if let Some(auction) = &self.auction {
            bytes.extend_from_slice(
                &auction
                    .indicative_price
                    .unwrap_or(Decimal::ZERO)
                    .raw()
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(&auction.paired.raw().to_le_bytes());
            bytes.extend_from_slice(&auction.imbalance.raw().to_le_bytes());
        }
        sha256_hex(&bytes)
    }
}
