//! Venue identity and provenance.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A trading venue, exchange, chain or market operator.
///
/// Opaque and case-preserving. Two venues are the same venue when their
/// identifiers match exactly — normalising case here would silently merge
/// `XNYS` and a hypothetical `xnys` from a different feed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VenueId(String);

impl VenueId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VenueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of market a venue is.
///
/// This is not decoration: it decides which settlement, custody and failure
/// assumptions apply. A centralised exchange can be relied on to hold an order
/// book; a mempool cannot be relied on to hold anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum VenueClass {
    /// A lit exchange with a central limit order book.
    Exchange,
    /// An alternative trading system or dark pool.
    DarkPool,
    /// An electronic communication network.
    Ecn,
    /// A derivatives exchange with margin and clearing.
    Derivatives,
    /// A dealer or bank quoting a bilateral price.
    OverTheCounter,
    /// A centralised crypto exchange.
    CryptoExchange,
    /// An on-chain automated market maker or order book.
    DecentralisedExchange,
    /// A prediction or event market.
    PredictionMarket,
    /// A broker or custodian reporting positions rather than quoting prices.
    Custodian,
}

impl VenueClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Exchange => "exchange",
            Self::DarkPool => "dark_pool",
            Self::Ecn => "ecn",
            Self::Derivatives => "derivatives",
            Self::OverTheCounter => "otc",
            Self::CryptoExchange => "crypto_exchange",
            Self::DecentralisedExchange => "dex",
            Self::PredictionMarket => "prediction_market",
            Self::Custodian => "custodian",
        }
    }

    /// Whether a resting order at this venue can be relied upon to still be
    /// there when an order reaches it.
    ///
    /// False for anything settled on a public chain, where the interval
    /// between deciding and landing is contested by every other participant.
    pub const fn quotes_are_firm(&self) -> bool {
        matches!(
            self,
            Self::Exchange | Self::DarkPool | Self::Ecn | Self::Derivatives | Self::CryptoExchange
        )
    }

    /// Whether execution settles atomically or can partially fail.
    ///
    /// A multi-leg trade on a chain can land one leg and revert another; the
    /// leg planner has to hold inventory against that possibility.
    pub const fn settles_atomically(&self) -> bool {
        !matches!(self, Self::DecentralisedExchange)
    }
}

/// Whether a venue is currently accepting business.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VenueStatus {
    /// Continuous trading.
    Open,
    /// Pre-open, opening or closing auction.
    Auction,
    /// Reachable but not trading.
    Halted,
    /// Outside session hours.
    Closed,
    /// Cannot be reached. Distinct from `Halted`: the venue may be trading and
    /// this cell simply cannot see it, which is the more dangerous case.
    Unreachable,
}

impl VenueStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Auction => "auction",
            Self::Halted => "halted",
            Self::Closed => "closed",
            Self::Unreachable => "unreachable",
        }
    }

    /// Whether an order may be sent.
    ///
    /// Auctions accept orders but not the same order types, which is the
    /// router's problem rather than this flag's.
    pub const fn accepts_orders(&self) -> bool {
        matches!(self, Self::Open | Self::Auction)
    }

    /// Whether prices from this venue may be used as a mark.
    pub const fn prices_are_usable(&self) -> bool {
        matches!(self, Self::Open | Self::Auction)
    }
}

/// Where a fact came from, precisely enough to find it again.
///
/// Carried through every transformation. A feature computed from three book
/// updates records all three origins, because "which feed said this" is the
/// first question asked after a bad trade and the most expensive one to
/// reconstruct afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub venue: VenueId,
    /// The feed or channel within the venue, e.g. `itch-a`, `sbe-2`.
    pub feed: String,
    /// The venue's own sequence number for this message.
    pub sequence: u64,
    /// The partition or channel the sequence is scoped to. Sequence numbers
    /// are only comparable within a partition.
    pub partition: u32,
}

impl Origin {
    pub fn new(venue: VenueId, feed: impl Into<String>, partition: u32, sequence: u64) -> Self {
        Self {
            venue,
            feed: feed.into(),
            sequence,
            partition,
        }
    }

    /// A stable key for the ordered stream this message belongs to.
    ///
    /// Sequence gaps are only meaningful within one of these.
    pub fn stream_key(&self) -> String {
        format!("{}/{}/{}", self.venue.as_str(), self.feed, self.partition)
    }

    /// Whether `self` is the message that should directly follow `previous`.
    pub fn directly_follows(&self, previous: &Self) -> bool {
        self.stream_key() == previous.stream_key() && self.sequence == previous.sequence + 1
    }
}
