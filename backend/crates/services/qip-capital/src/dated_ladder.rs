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

/// The id prefix a [`RedemptionNotice`] is filed under on the ladder.
pub const NOTICE_ENTRY_PREFIX: &str = "notice:";

/// An operator's declaration that a claim on the book is expected to fall due.
///
/// The platform never processes a withdrawal (ADR 0021: `WithdrawalEntitlement`
/// has one variant), so there is no event to observe and no forecast to build
/// from facts it holds. A notice is the record of an expected claim, an intent
/// and not a payment, and it moves nothing. Its single effect is on the ladder
/// as an [`EntryKind::Outflow`], where the idle-cash router reads it
/// (ADR 0159 decisions 9 and 10).
///
/// Who declared it and when are part of the record because the declaration is
/// the only evidence the claim exists: a notice nobody can attribute is a
/// number nobody checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedemptionNotice {
    /// Stable identity. A later declaration under the same id supersedes an
    /// earlier one; nothing is inferred or aged out silently.
    pub id: String,
    /// The amount expected to fall due. Always positive.
    pub amount: Decimal,
    /// When it falls due.
    pub effective: Timestamp,
    /// The authenticated operator who declared it.
    pub declarer: String,
    /// When it was declared.
    pub declared_at: Timestamp,
}

impl RedemptionNotice {
    /// Build a notice, refusing one that could not mean anything: an empty id
    /// or declarer, a non-positive amount, or an effective instant before the
    /// declaration (a claim that fell due before anyone said so is a
    /// back-dated record, and the ladder is built as of an instant).
    pub fn new(
        id: impl Into<String>,
        amount: Decimal,
        effective: Timestamp,
        declarer: impl Into<String>,
        declared_at: Timestamp,
    ) -> Result<Self> {
        Self {
            id: id.into(),
            amount,
            effective,
            declarer: declarer.into(),
            declared_at,
        }
        .validated()
    }

    /// The same checks as [`Self::new`] on a record that arrived by decoding,
    /// which bypasses the constructor.
    pub fn validated(self) -> Result<Self> {
        if self.id.trim().is_empty() || self.id.contains(char::is_whitespace) {
            return Err(Error::invalid(
                "a redemption notice needs an id with no whitespace, or a later declaration \
                 cannot supersede it",
            ));
        }
        if self.declarer.trim().is_empty() {
            return Err(Error::invalid(format!(
                "redemption notice {} names no declarer; it is evidence only if someone \
                 authenticated stands behind it",
                self.id
            )));
        }
        if !self.amount.is_positive() {
            return Err(Error::invalid(format!(
                "redemption notice {} is for {}; a notice records an amount expected to fall \
                 due, which is positive or it is nothing",
                self.id, self.amount
            )));
        }
        if self.effective < self.declared_at {
            return Err(Error::invalid(format!(
                "redemption notice {} falls due at {} but was declared at {}; a claim cannot \
                 fall due before it was declared, so declare the date it is expected",
                self.id, self.effective, self.declared_at
            )));
        }
        Ok(self)
    }

    /// The id this notice is filed under on a ladder.
    pub fn entry_id(&self) -> String {
        format!("{NOTICE_ENTRY_PREFIX}{}", self.id)
    }

    /// The notice as the outflow it puts on the ladder.
    pub fn as_outflow(&self) -> LadderEntry {
        LadderEntry {
            id: self.entry_id(),
            kind: EntryKind::Outflow,
            amount: self.amount,
            at: self.effective,
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
        self.check(&entry)?;
        Ok(self.insert(entry))
    }

    /// File a validated entry and say which rung it landed in. Private so the
    /// checks in [`Self::place`] cannot be skipped by a second front door.
    fn insert(&mut self, entry: LadderEntry) -> usize {
        let rung = self.bucket_of(entry.at);
        self.entries.insert(entry.id.clone(), entry);
        rung
    }

    /// Open a ladder as of `as_of` and file every entry in `entries`, in the
    /// order given. This is how the ladder is a view (ADR 0159 decision 6): it
    /// is rebuilt from the facts that date it each time it is read and is
    /// never stored, so a replay builds the same ladder from the same facts.
    ///
    /// Refused whole when any entry would be refused by [`Self::place`], which
    /// is the same checks run once more.
    pub fn rebuilt(
        as_of: Timestamp,
        horizons: &[Duration],
        entries: impl IntoIterator<Item = LadderEntry>,
    ) -> Result<Self> {
        let mut ladder = Self::new(as_of, horizons)?;
        for entry in entries {
            ladder.check(&entry)?;
            ladder.insert(entry);
        }
        Ok(ladder)
    }

    fn check(&self, entry: &LadderEntry) -> Result<()> {
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
        Ok(())
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

    /// The outflow that falls due first, ties broken by id so the answer does
    /// not depend on insertion order. `None` when nothing is owed.
    ///
    /// This is the one question the idle-cash router asks of the ladder
    /// (ADR 0159 decision 10): cash may be parked only where it can be had
    /// back before it is owed.
    pub fn earliest_outflow(&self) -> Option<&LadderEntry> {
        self.entries
            .values()
            .filter(|entry| entry.kind == EntryKind::Outflow)
            .min_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)))
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
