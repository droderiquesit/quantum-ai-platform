//! The universe as it was, not as it is.
//!
//! [`crate::universe::Universe`] is current membership: an instrument that was
//! delisted is gone from it, a renamed one answers only to its new ticker, and
//! nothing records that a venue was dark for an afternoon. A backtest built from
//! it trains on the survivors, and a replay that "skips" an outage reads as a
//! quiet market. [`UniverseHistory`] keeps what the present universe forgets:
//!
//! * a listing span per instrument, ending in a delisting rather than ending in
//!   the instrument's removal, so [`UniverseHistory::members_as_of`] names a
//!   since-failed product on a date it was still trading;
//! * symbol eras, so a rename is the same [`ObjectId`] on both sides of it and
//!   [`UniverseHistory::resolve`] answers by the date asked, which also keeps a
//!   recycled ticker from resolving to the wrong company;
//! * venue outages as recorded facts that [`UniverseHistory::outages_overlapping`]
//!   returns for any window a replay spans.
//!
//! Nothing reads a clock; every instant is an argument.

use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A ticker, and the instant from which it was the instrument's symbol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEra {
    pub symbol: String,
    pub from: Timestamp,
}

/// One instrument's life in the universe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub object_id: ObjectId,
    pub listed_from: Timestamp,
    /// `None` while it still trades. Set by a delisting; the record is kept.
    pub delisted_at: Option<Timestamp>,
    /// In ascending `from` order; the first starts at `listed_from`.
    pub symbols: Vec<SymbolEra>,
}

impl Listing {
    fn is_member_at(&self, at: Timestamp) -> bool {
        self.listed_from <= at && self.delisted_at.is_none_or(|end| at < end)
    }

    /// The symbol in force at `at`, if the instrument was listed then.
    pub fn symbol_at(&self, at: Timestamp) -> Option<&str> {
        if !self.is_member_at(at) {
            return None;
        }
        self.symbols
            .iter()
            .rev()
            .find(|era| era.from <= at)
            .map(|era| era.symbol.as_str())
    }
}

/// A period in which a venue published nothing because it could not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueOutage {
    pub venue: String,
    pub from: Timestamp,
    /// Exclusive.
    pub until: Timestamp,
    pub reason: String,
}

/// Listing spans, symbol eras and venue outages, as they were.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniverseHistory {
    listings: BTreeMap<String, Listing>,
    outages: Vec<VenueOutage>,
}

impl UniverseHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an instrument listing under its first symbol.
    pub fn list(
        &mut self,
        object_id: ObjectId,
        symbol: impl Into<String>,
        at: Timestamp,
    ) -> Result<()> {
        let symbol = symbol.into();
        if symbol.trim().is_empty() {
            return Err(Error::invalid(
                "a listing needs a symbol; state the instrument's first ticker",
            ));
        }
        let key = object_id.as_str().to_string();
        if self.listings.contains_key(&key) {
            return Err(Error::invalid(format!(
                "{key} is already in the history; a relisting is a new object id, or a rename"
            )));
        }
        self.listings.insert(
            key,
            Listing {
                object_id,
                listed_from: at,
                delisted_at: None,
                symbols: vec![SymbolEra { symbol, from: at }],
            },
        );
        Ok(())
    }

    /// Record a symbol change. The instrument keeps its object id.
    pub fn rename(
        &mut self,
        object_id: &ObjectId,
        new_symbol: impl Into<String>,
        at: Timestamp,
    ) -> Result<()> {
        let new_symbol = new_symbol.into();
        let listing = self.listing_mut(object_id)?;
        let last_from = listing.symbols.last().map(|era| era.from);
        if last_from.is_some_and(|from| at <= from) || at < listing.listed_from {
            return Err(Error::invalid(format!(
                "{} was renamed at {at}, which is not after its previous symbol began; \
                 record changes in order",
                object_id
            )));
        }
        if listing.delisted_at.is_some_and(|end| at >= end) {
            return Err(Error::invalid(format!(
                "{object_id} was renamed after it was delisted"
            )));
        }
        listing.symbols.push(SymbolEra {
            symbol: new_symbol,
            from: at,
        });
        Ok(())
    }

    /// Record a delisting. The listing stays, ending at `at`.
    pub fn delist(&mut self, object_id: &ObjectId, at: Timestamp) -> Result<()> {
        let listing = self.listing_mut(object_id)?;
        if listing.delisted_at.is_some() {
            return Err(Error::invalid(format!("{object_id} is already delisted")));
        }
        let latest = listing
            .symbols
            .last()
            .map_or(listing.listed_from, |era| era.from);
        if at <= latest {
            return Err(Error::invalid(format!(
                "{object_id} was delisted at {at}, not after its latest symbol began"
            )));
        }
        listing.delisted_at = Some(at);
        Ok(())
    }

    /// Record a venue outage. An empty or inverted window is refused.
    pub fn record_outage(&mut self, outage: VenueOutage) -> Result<()> {
        if outage.until <= outage.from {
            return Err(Error::invalid(format!(
                "outage at {} ends at {}, not after it began at {}",
                outage.venue, outage.until, outage.from
            )));
        }
        self.outages.push(outage);
        self.outages
            .sort_by(|a, b| (a.from, &a.venue).cmp(&(b.from, &b.venue)));
        Ok(())
    }

    /// Every instrument that was listed at `at`, including any delisted later.
    pub fn members_as_of(&self, at: Timestamp) -> Vec<&Listing> {
        self.listings
            .values()
            .filter(|listing| listing.is_member_at(at))
            .collect()
    }

    /// The instrument a ticker named at `at`.
    ///
    /// Answers by date, so a ticker reused by a later company does not resolve
    /// to the earlier one, and a retired ticker still resolves for dates it was
    /// in force.
    pub fn resolve(&self, symbol: &str, at: Timestamp) -> Option<&ObjectId> {
        self.listings
            .values()
            .find(|listing| {
                listing
                    .symbol_at(at)
                    .is_some_and(|s| s.eq_ignore_ascii_case(symbol))
            })
            .map(|listing| &listing.object_id)
    }

    /// Outages at `venue` that overlap `[from, until)`; nothing is skipped
    /// because nothing arrived.
    pub fn outages_overlapping(
        &self,
        venue: &str,
        from: Timestamp,
        until: Timestamp,
    ) -> Vec<&VenueOutage> {
        self.outages
            .iter()
            .filter(|o| o.venue == venue && o.from < until && from < o.until)
            .collect()
    }

    fn listing_mut(&mut self, object_id: &ObjectId) -> Result<&mut Listing> {
        self.listings
            .get_mut(object_id.as_str())
            .ok_or_else(|| Error::not_found(format!("{object_id} is not in the universe history")))
    }
}
