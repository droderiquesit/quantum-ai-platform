//! Quotes, trades and ticks.

use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::quality::DataQuality;
use serde::{Deserialize, Serialize};

/// Which side of the book an order or trade sits on.
pub use crate::book::Side;

/// Top-of-book quote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub object_id: ObjectId,
    pub venue: String,
    pub at: Timestamp,
    pub bid: Decimal,
    pub ask: Decimal,
    pub bid_size: Decimal,
    pub ask_size: Decimal,
    /// No `#[serde(default)]`: [`DataQuality::default`] is a *perfect*
    /// measurement — completeness and confidence 1.0, not imputed — which
    /// clears [`qip_financial::quality::DECISION_QUALITY_FLOOR`]. A record
    /// that says nothing about its quality would therefore decode as one that
    /// measured everything perfectly, so it does not decode at all.
    pub quality: DataQuality,
}

impl Quote {
    /// Arithmetic mid. `None` when either side is missing.
    pub fn mid(&self) -> Option<Decimal> {
        if self.bid.is_positive() && self.ask.is_positive() {
            (self.bid + self.ask).checked_div(Decimal::from_int(2))
        } else {
            None
        }
    }

    /// Size-weighted mid, which leans toward the side with less depth.
    ///
    /// A better short-horizon predictor of the next trade price than the plain
    /// mid, and what the microstructure agent reasons about.
    pub fn microprice(&self) -> Option<Decimal> {
        let total = self.bid_size + self.ask_size;
        if !total.is_positive() || !self.bid.is_positive() || !self.ask.is_positive() {
            return self.mid();
        }
        let weighted = self.bid * self.ask_size + self.ask * self.bid_size;
        weighted.checked_div(total)
    }

    pub fn spread(&self) -> Decimal {
        self.ask - self.bid
    }

    /// Spread in basis points of the mid.
    pub fn spread_bps(&self) -> Option<f64> {
        let mid = self.mid()?;
        if !mid.is_positive() {
            return None;
        }
        Some(self.spread().to_f64() / mid.to_f64() * 10_000.0)
    }

    /// True when the ask is below the bid — a data error, not an opportunity.
    ///
    /// Crossed books arrive routinely from consolidated feeds during outages;
    /// treating one as free money is how a naive system loses real capital.
    pub fn is_crossed(&self) -> bool {
        self.bid.is_positive() && self.ask.is_positive() && self.ask < self.bid
    }

    /// Whether both sides are quoted.
    pub fn is_two_sided(&self) -> bool {
        self.bid.is_positive() && self.ask.is_positive()
    }

    /// Depth imbalance in `[-1, 1]`; positive means more size bid than offered.
    pub fn imbalance(&self) -> f64 {
        let total = (self.bid_size + self.ask_size).to_f64();
        if total <= 0.0 {
            return 0.0;
        }
        (self.bid_size - self.ask_size).to_f64() / total
    }

    /// Structural problems with the quote.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.is_crossed() {
            issues.push(format!(
                "crossed quote: bid {} above ask {}",
                self.bid, self.ask
            ));
        }
        if self.bid.is_negative() || self.ask.is_negative() {
            issues.push("negative price".into());
        }
        if self.bid_size.is_negative() || self.ask_size.is_negative() {
            issues.push("negative size".into());
        }
        issues
    }
}

impl EventBody for Quote {
    const TOPIC: Topic = Topic::MarketQuote;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        // A venue may resend the same quote on reconnect; the instant plus the
        // two sides identifies it.
        Some(format!(
            "{}:{}:{}:{}:{}",
            self.object_id,
            self.venue,
            self.at.as_nanos(),
            self.bid,
            self.ask
        ))
    }
}

/// How a trade was executed, which determines whether it is usable for
/// price discovery.
///
/// Deliberately has no `Default`, and [`Trade::condition`] is deliberately not
/// `#[serde(default)]`. The only value a default could pick is `Regular`, and
/// `Regular` is the one condition that is *price-forming*: a late report or an
/// off-exchange cross whose condition a venue omitted would enter the platform
/// as an ordinary continuous-session print and drag the mark it is not
/// entitled to move. Absence of a condition is not a condition, so it is
/// refused where it arrives instead. [`qip_contracts::TradeCondition`] — the
/// edge's own copy of this idea — already has no `Default` for the same
/// reason, and this is the library half catching up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeCondition {
    /// Ordinary continuous-session trade.
    Regular,
    /// Opening or closing auction print.
    Auction,
    /// Reported late; the print does not reflect the current market.
    LateReport,
    /// Executed away from the quoted market, e.g. a block or a cross.
    OffExchange,
    /// Corrected or cancelled after the fact.
    Corrected,
}

impl TradeCondition {
    /// Whether the print should contribute to price discovery.
    ///
    /// Late and corrected prints carry stale or superseded prices; including
    /// them in a VWAP or a volatility estimate corrupts both.
    pub fn is_price_forming(&self) -> bool {
        matches!(self, Self::Regular | Self::Auction)
    }
}

/// An executed trade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trade {
    pub object_id: ObjectId,
    pub venue: String,
    pub at: Timestamp,
    pub price: Decimal,
    pub size: Decimal,
    /// Side the aggressor was on, where the venue reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggressor: Option<Side>,
    /// No `#[serde(default)]`: see [`TradeCondition`]. A record that does not
    /// state how it printed does not decode.
    pub condition: TradeCondition,
    /// Venue-assigned trade identifier, for deduplication and reconciliation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade_id: Option<String>,
    /// No `#[serde(default)]`: [`DataQuality::default`] is a *perfect*
    /// measurement — completeness and confidence 1.0, not imputed — which
    /// clears [`qip_financial::quality::DECISION_QUALITY_FLOOR`]. A record
    /// that says nothing about its quality would therefore decode as one that
    /// measured everything perfectly, so it does not decode at all.
    pub quality: DataQuality,
}

impl Trade {
    pub fn notional(&self) -> Decimal {
        self.price * self.size
    }

    /// Classify the aggressor against the prevailing quote when the venue does
    /// not report it (the Lee-Ready rule).
    pub fn infer_aggressor(&self, quote: &Quote) -> Option<Side> {
        if let Some(side) = self.aggressor {
            return Some(side);
        }
        let mid = quote.mid()?;
        if self.price > mid {
            Some(Side::Buy)
        } else if self.price < mid {
            Some(Side::Sell)
        } else {
            // At the mid the rule is undefined; reporting nothing is better
            // than guessing, since the caller aggregates signed volume.
            None
        }
    }
}

impl EventBody for Trade {
    const TOPIC: Topic = Topic::MarketTrade;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        self.trade_id
            .as_ref()
            .map(|id| format!("{}:{}:{id}", self.object_id, self.venue))
    }
}

/// A last-price update, the lightest market event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    pub object_id: ObjectId,
    pub venue: String,
    pub at: Timestamp,
    pub price: Decimal,
    #[serde(default)]
    pub volume: Decimal,
    /// No `#[serde(default)]`: [`DataQuality::default`] is a *perfect*
    /// measurement — completeness and confidence 1.0, not imputed — which
    /// clears [`qip_financial::quality::DECISION_QUALITY_FLOOR`]. A record
    /// that says nothing about its quality would therefore decode as one that
    /// measured everything perfectly, so it does not decode at all.
    pub quality: DataQuality,
}

impl EventBody for Tick {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}:{}",
            self.object_id,
            self.venue,
            self.at.as_nanos(),
            self.price
        ))
    }
}
