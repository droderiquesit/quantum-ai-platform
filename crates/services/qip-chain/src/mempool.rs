//! Pending transactions, and the ordering nobody owes you.
//!
//! A mempool looks like a book and is not one. The entries are visible,
//! priced, and completely reorderable: a builder may include them in any order
//! it likes, insert its own transactions between them, or drop them. Reading a
//! predicted ordering as though it were a queue position is how a strategy
//! discovers sandwiching from the losing side.
//!
//! [`VenueClass::DecentralisedExchange`] already reports
//! `quotes_are_firm() == false` and `settles_atomically() == false`. The types
//! here make a caller act on that rather than merely be able to read it: the
//! predicted sequence is unreachable except through a [`ReorderingRisk`]
//! derived from that exact ordering, so acknowledging the risk is a step that
//! has to appear in the caller's own code.

use qip_contracts::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::block::{Address, Block, TraceKind, TxHash};
use crate::gas::effective_gas_price;

/// A transaction seen in the mempool and not yet in a block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingTransaction {
    pub hash: TxHash,
    pub from: Address,
    /// Sender sequence number. A builder cannot include a nonce before the one
    /// below it, which is the only hard constraint on ordering there is.
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee_per_gas: Decimal,
    pub max_priority_fee_per_gas: Decimal,
    pub first_seen: Timestamp,
    /// What the transaction appears to intend, where the adapter decoded it.
    /// Absent is the honest answer for an opaque calldata blob.
    pub intent: Option<TraceKind>,
}

impl PendingTransaction {
    /// What this transaction would pay per gas at the given base fee.
    pub fn effective_gas_price(&self, base_fee: Decimal) -> Result<Decimal> {
        effective_gas_price(base_fee, self.max_fee_per_gas, self.max_priority_fee_per_gas)
    }

    /// The part of the gas price that a builder keeps, which is the only part
    /// it has any reason to order on.
    pub fn tip(&self, base_fee: Decimal) -> Result<Decimal> {
        Ok(self.effective_gas_price(base_fee)? - base_fee)
    }

    /// Whether the transaction can be included at this base fee at all.
    pub fn is_includable(&self, base_fee: Decimal) -> bool {
        self.max_fee_per_gas >= base_fee
    }
}

/// Pending transactions for one chain venue.
#[derive(Clone, Debug)]
pub struct Mempool {
    venue: VenueId,
    class: VenueClass,
    pending: BTreeMap<TxHash, PendingTransaction>,
}

impl Mempool {
    /// Refuses a venue class whose quotes are firm: a mempool models the
    /// absence of a guarantee, and attaching one to a venue that does give
    /// guarantees would misrepresent both.
    pub fn new(venue: VenueId, class: VenueClass) -> Result<Self> {
        if class.quotes_are_firm() {
            return Err(Error::invalid(format!(
                "venue {venue} is a {} and quotes firmly; it has no mempool",
                class.as_str()
            )));
        }
        Ok(Self {
            venue,
            class,
            pending: BTreeMap::new(),
        })
    }

    pub const fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub const fn class(&self) -> VenueClass {
        self.class
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn get(&self, hash: &TxHash) -> Option<&PendingTransaction> {
        self.pending.get(hash)
    }

    /// Record a pending transaction, replacing an earlier view of the same one.
    pub fn insert(&mut self, transaction: PendingTransaction) -> Option<PendingTransaction> {
        self.pending.insert(transaction.hash, transaction)
    }

    pub fn remove(&mut self, hash: &TxHash) -> Option<PendingTransaction> {
        self.pending.remove(hash)
    }

    /// Drop everything the block included. Returns how many were pending.
    pub fn absorb(&mut self, block: &Block) -> usize {
        let mut removed = 0;
        for transaction in &block.transactions {
            if self.pending.remove(&transaction.hash).is_some() {
                removed += 1;
            }
        }
        removed
    }

    /// The ordering a fee-maximising builder would probably apply.
    ///
    /// Probably. The result is a prediction with no standing whatsoever, and
    /// the type it comes back in says so.
    pub fn likely_ordering(
        &self,
        base_fee: Decimal,
        block_gas_limit: u64,
        at: Timestamp,
    ) -> Result<LikelyOrdering> {
        let mut candidates: Vec<(Decimal, &PendingTransaction)> = Vec::new();
        let mut excluded: Vec<TxHash> = Vec::new();
        for transaction in self.pending.values() {
            if !transaction.is_includable(base_fee) {
                excluded.push(transaction.hash);
                continue;
            }
            candidates.push((transaction.tip(base_fee)?, transaction));
        }
        // Highest tip first; ties broken by arrival and then by hash so the
        // prediction is the same on every machine that computes it.
        candidates.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then(left.1.first_seen.cmp(&right.1.first_seen))
                .then(left.1.hash.cmp(&right.1.hash))
        });

        let mut entries: Vec<OrderedPending> = Vec::new();
        let mut cumulative_gas: u64 = 0;
        let mut next_nonce: BTreeMap<Address, u64> = BTreeMap::new();
        for (_, transaction) in &candidates {
            let lowest = next_nonce
                .entry(transaction.from.clone())
                .or_insert(u64::MAX);
            *lowest = (*lowest).min(transaction.nonce);
        }
        let mut remaining: Vec<(Decimal, &PendingTransaction)> = candidates;
        while !remaining.is_empty() {
            let position = remaining.iter().position(|(_, transaction)| {
                next_nonce
                    .get(&transaction.from)
                    .is_some_and(|expected| *expected == transaction.nonce)
            });
            // A nonce gap makes the rest of that sender's transactions
            // unreachable; they stay pending rather than being invented into
            // an order that no builder could execute.
            let Some(position) = position else { break };
            let (price, transaction) = remaining.remove(position);
            let projected = cumulative_gas.saturating_add(transaction.gas_limit);
            if projected > block_gas_limit {
                excluded.push(transaction.hash);
                continue;
            }
            cumulative_gas = projected;
            if let Some(expected) = next_nonce.get_mut(&transaction.from) {
                *expected = transaction.nonce.saturating_add(1);
            }
            entries.push(OrderedPending {
                transaction: transaction.clone(),
                effective_gas_price: base_fee + price,
                tip: price,
                cumulative_gas,
            });
        }
        for (_, transaction) in remaining {
            excluded.push(transaction.hash);
        }

        let digest = ordering_digest(&self.venue, base_fee, at, &entries);
        Ok(LikelyOrdering {
            venue: self.venue.clone(),
            class: self.class,
            base_fee,
            at,
            entries,
            excluded,
            digest,
        })
    }
}

/// One transaction in a predicted ordering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderedPending {
    pub transaction: PendingTransaction,
    pub effective_gas_price: Decimal,
    pub tip: Decimal,
    /// Gas consumed by this transaction and everything predicted before it.
    pub cumulative_gas: u64,
}

/// A predicted block ordering, and the risk of believing it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LikelyOrdering {
    venue: VenueId,
    class: VenueClass,
    base_fee: Decimal,
    at: Timestamp,
    entries: Vec<OrderedPending>,
    excluded: Vec<TxHash>,
    digest: String,
}

impl LikelyOrdering {
    pub const fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub const fn base_fee(&self) -> Decimal {
        self.base_fee
    }

    pub const fn at(&self) -> Timestamp {
        self.at
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Transactions the prediction leaves out, priced out or unreachable.
    pub fn excluded(&self) -> &[TxHash] {
        &self.excluded
    }

    /// Where a transaction is predicted to land.
    pub fn predicted_position(&self, hash: &TxHash) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| &entry.transaction.hash == hash)
    }

    /// State what believing this ordering exposes the caller to.
    ///
    /// Produces the token that [`LikelyOrdering::sequence`] demands. It is
    /// cheap to call and impossible to skip, which is the point.
    pub fn assess(&self) -> ReorderingRisk {
        ReorderingRisk {
            digest: self.digest.clone(),
            venue: self.venue.clone(),
            competing: self.entries.len(),
            settles_atomically: self.class.settles_atomically(),
            quotes_are_firm: self.class.quotes_are_firm(),
        }
    }

    /// The predicted sequence, once the caller holds the matching risk.
    pub fn sequence(&self, risk: &ReorderingRisk) -> Result<&[OrderedPending]> {
        if risk.digest != self.digest {
            return Err(Error::invalid(
                "the reordering risk was assessed against a different mempool state",
            ));
        }
        Ok(&self.entries)
    }
}

/// What a caller is accepting by acting on a predicted ordering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReorderingRisk {
    digest: String,
    pub venue: VenueId,
    /// Transactions that could be reordered ahead of any given one.
    pub competing: usize,
    /// From [`VenueClass::settles_atomically`]: false means one leg can land
    /// and another revert.
    pub settles_atomically: bool,
    /// From [`VenueClass::quotes_are_firm`]: false means the state read here
    /// need not survive to execution.
    pub quotes_are_firm: bool,
}

impl ReorderingRisk {
    /// The position a transaction must survive if every other pending
    /// transaction outbids it — the only position worth sizing against.
    pub const fn worst_case_position(&self) -> usize {
        self.competing.saturating_sub(1)
    }

    /// Whether a transaction here can be preceded by one that did not exist
    /// when the ordering was predicted. Always true on a public mempool.
    pub const fn can_be_front_run(&self) -> bool {
        !self.quotes_are_firm
    }
}

fn ordering_digest(
    venue: &VenueId,
    base_fee: Decimal,
    at: Timestamp,
    entries: &[OrderedPending],
) -> String {
    let mut hasher = qip_core::Hasher256::new();
    hasher.update(venue.as_str().as_bytes());
    hasher.update(&base_fee.raw().to_le_bytes());
    hasher.update(&at.as_nanos().to_le_bytes());
    for entry in entries {
        hasher.update(entry.transaction.hash.hash().as_bytes());
    }
    let digest = hasher.finish();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
