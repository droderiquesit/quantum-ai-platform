//! The dated liquidity ladder (CAPITAL-002): every balance, inflow, outflow
//! and encumbrance in the time bucket at which it becomes available or falls
//! due.
//!
//! [`qip_financial::ladder::LiquidityLadder`] answers a different question —
//! what would it cost to sell what the book holds — and it has no clock in it.
//! This one has nothing *but* a clock. The failure it prevents is the oldest
//! one in treasury: a balance that settles on Tuesday counted against a
//! payment due on Monday, because both were rows in the same total.
//!
//! So an entry is placed by one fact, the instant it becomes available or
//! falls due, and [`DatedLadder::available_by`] counts a bucket only once the
//! whole bucket has closed. An entry can therefore be read later than it is
//! true and never earlier. An encumbered balance is a balance whose instant
//! is its unlock time: it is on the ladder, at the rung where it stops being
//! somebody else's.
//!
//! Nothing here reads a wall clock; the ladder is built as of an instant the
//! caller names, so a replay builds the same ladder.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What an entry on the ladder is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntryKind {
    /// Cash or a liquid asset, available at its settlement time.
    Balance,
    /// A receipt expected at a known time.
    Inflow,
    /// A payment falling due. The only kind that subtracts.
    Outflow,
    /// A balance pledged or locked, available again at its unlock time.
    Encumbered,
}

/// One dated amount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderEntry {
    /// Stable identity, unique on the ladder.
    pub id: String,
    /// What it is.
    pub kind: EntryKind,
    /// Always positive; [`EntryKind::Outflow`] carries the sign.
    pub amount: Decimal,
    /// When it becomes available, or falls due.
    pub at: Timestamp,
}

impl LadderEntry {
    /// The amount as it counts toward liquidity: an outflow subtracts.
    pub fn signed(&self) -> Decimal {
        match self.kind {
            EntryKind::Outflow => -self.amount,
            EntryKind::Balance | EntryKind::Inflow | EntryKind::Encumbered => self.amount,
        }
    }
}

/// One rung: everything that becomes available or falls due before it closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    /// The instant the rung closes. `None` is the open rung past the last
    /// horizon, which holds what the ladder was not asked to date more finely.
    pub closes: Option<Timestamp>,
    /// Net liquidity in the rung.
    pub net: Decimal,
    /// The entries in it, by id.
    pub entries: Vec<String>,
}

/// Dated liquidity as of one instant.
#[derive(Debug, Clone)]
pub struct DatedLadder {
    /// Rung closes, ascending. The first is the as-of instant itself.
    closes: Vec<Timestamp>,
    entries: BTreeMap<String, LadderEntry>,
}

impl DatedLadder {
    /// Open a ladder as of `as_of` with rungs closing at each horizon after
    /// it. Horizons must be positive and strictly ascending; an unordered
    /// list is refused rather than sorted, because a ladder whose rungs were
    /// silently reordered no longer matches the one its reader configured.
    pub fn new(as_of: Timestamp, horizons: &[Duration]) -> Result<Self> {
        if horizons.is_empty() {
            return Err(Error::invalid(
                "a liquidity ladder needs at least one horizon; name the rungs it should date",
            ));
        }
        let mut closes = vec![as_of];
        let mut previous = Duration::from_nanos(0);
        for horizon in horizons {
            if horizon.as_nanos() <= previous.as_nanos() {
                return Err(Error::invalid(format!(
                    "ladder horizons must be positive and strictly ascending, and {horizon:?} \
                     follows {previous:?}; list them shortest first"
                )));
            }
            closes.push(as_of.saturating_add(*horizon));
            previous = *horizon;
        }
        Ok(Self {
            closes,
            entries: BTreeMap::new(),
        })
    }

    /// The instant the ladder is built as of.
    pub fn as_of(&self) -> Timestamp {
        self.closes[0]
    }

    /// The rung an instant falls in: the first that closes at or after it.
    /// Anything past the last horizon lands in the open rung.
    pub fn bucket_of(&self, at: Timestamp) -> usize {
        self.closes
            .iter()
            .position(|close| *close >= at)
            .unwrap_or(self.closes.len())
    }

    /// Place an entry, returning the rung it landed in.
    pub fn place(&mut self, entry: LadderEntry) -> Result<usize> {
        if !entry.amount.is_positive() {
            return Err(Error::invalid(format!(
                "ladder entry {} must carry a positive amount, got {}; an outflow is an \
                 Outflow, not a negative balance",
                entry.id, entry.amount
            )));
        }
        if self.entries.contains_key(&entry.id) {
            return Err(Error::invalid(format!(
                "ladder entry {} is already placed; remove it before placing it again, or the \
                 same balance is counted in two rungs",
                entry.id
            )));
        }
        let rung = self.bucket_of(entry.at);
        self.entries.insert(entry.id.clone(), entry);
        Ok(rung)
    }

    /// Take an entry off the ladder.
    pub fn remove(&mut self, id: &str) -> Result<LadderEntry> {
        self.entries
            .remove(id)
            .ok_or_else(|| Error::not_found(format!("no ladder entry {id} to remove")))
    }

    /// One entry, if it is on the ladder.
    pub fn entry(&self, id: &str) -> Option<&LadderEntry> {
        self.entries.get(id)
    }

    /// Net liquidity across every entry.
    pub fn total(&self) -> Decimal {
        self.entries
            .values()
            .fold(Decimal::ZERO, |sum, entry| sum + entry.signed())
    }

    /// Every rung in order, the open rung last.
    pub fn buckets(&self) -> Vec<Bucket> {
        let mut buckets: Vec<Bucket> = self
            .closes
            .iter()
            .map(|close| Some(*close))
            .chain([None])
            .map(|closes| Bucket {
                closes,
                net: Decimal::ZERO,
                entries: Vec::new(),
            })
            .collect();
        for entry in self.entries.values() {
            let rung = &mut buckets[self.bucket_of(entry.at)];
            rung.net += entry.signed();
            rung.entries.push(entry.id.clone());
        }
        buckets
    }

    /// Net liquidity certain to be available by `at`: the rungs that have
    /// closed by then. A rung still open at `at` is left out whole, so the
    /// figure can understate what is available and never overstate it.
    pub fn available_by(&self, at: Timestamp) -> Decimal {
        self.buckets()
            .iter()
            .filter(|bucket| bucket.closes.is_some_and(|close| close <= at))
            .fold(Decimal::ZERO, |sum, bucket| sum + bucket.net)
    }
}
