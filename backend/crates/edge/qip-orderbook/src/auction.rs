//! Auction state.
//!
//! During an auction the continuous book is not what trades. Orders accumulate,
//! the venue publishes an indicative price and the imbalance that would remain
//! at it, and everything crosses at once at the uncross. A consumer that reads
//! the continuous touch through an opening auction is reading a book that
//! nobody can hit — which is why [`crate::VenueState::continuous_trading`]
//! exists and why the auction's own numbers are kept separately here rather
//! than folded into the book.

use qip_contracts::BookSide;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// What the venue says an auction would currently uncross at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuctionState {
    /// Where the auction would price. `None` before enough interest has
    /// accumulated for the venue to publish one — which is information, not an
    /// absence of it, and is why this is not defaulted to the last trade.
    pub indicative_price: Option<Decimal>,
    /// Quantity that would trade at the indicative price.
    pub paired: Decimal,
    /// Quantity that would remain unfilled. Always non-negative; the side it
    /// sits on is carried separately.
    pub imbalance: Decimal,
    /// Which side the imbalance is on.
    pub imbalance_side: Option<BookSide>,
    /// Venue time of the update this state came from.
    pub at: Timestamp,
}

impl AuctionState {
    pub fn new(
        indicative_price: Option<Decimal>,
        paired: Decimal,
        imbalance: Decimal,
        imbalance_side: Option<BookSide>,
        at: Timestamp,
    ) -> Self {
        Self {
            indicative_price,
            paired,
            imbalance,
            imbalance_side,
            at,
        }
    }

    /// Total interest in the auction: what would trade plus what would not.
    pub fn total_interest(&self) -> Decimal {
        self.paired + self.imbalance
    }

    /// Imbalance signed by side — positive when buyers are left over.
    ///
    /// An unsided imbalance is reported as zero: a venue that publishes a size
    /// without a side has told us how uncertain the uncross is, not which way
    /// it leans.
    pub fn signed_imbalance(&self) -> Decimal {
        match self.imbalance_side {
            Some(BookSide::Bid) => self.imbalance,
            Some(BookSide::Ask) => -self.imbalance,
            None => Decimal::ZERO,
        }
    }

    /// Imbalance as a fraction of total interest, in `[-1, 1]`.
    ///
    /// A statistic, so `f64`. Zero when nothing is in the auction yet.
    pub fn imbalance_ratio(&self) -> f64 {
        let total = self.total_interest();
        if !total.is_positive() {
            return 0.0;
        }
        self.signed_imbalance().to_f64() / total.to_f64()
    }

    /// Whether the auction has published a price to reason about.
    pub fn is_indicative(&self) -> bool {
        self.indicative_price.is_some()
    }
}
