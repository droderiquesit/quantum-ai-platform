//! Canonical chain state, and what a reorganisation does to it.
//!
//! This is the module that separates a chain from an exchange. An exchange's
//! fill is final when it prints. A chain's block is a proposal that the
//! network may withdraw: the block that was the head five seconds ago can stop
//! existing, taking with it every reserve, every fill and every position that
//! was derived from it.
//!
//! So derived state here is never simply overwritten. Each applied block
//! records what it displaced, a rollback restores exactly that, and a reorg is
//! a rollback to the common ancestor followed by a replay of the winning
//! branch. The undo path runs on every read that asks for confirmations, which
//! is deliberate: an unwind that is only exercised during an incident is an
//! unwind that does not work.

use qip_contracts::{BookSide, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::amm::{FeeBps, Pool, PoolCurve, PoolId};
use crate::block::{Block, BlockHash, BlockNumber, ChainId, TraceKind};
use crate::finality::{Confirmations, Finality};

/// A pool as the chain's history says it currently stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PoolState {
    pub venue: VenueId,
    pub base: ObjectId,
    pub quote: ObjectId,
    pub curve: PoolCurve,
    pub fee: FeeBps,
    pub reserve_base: Decimal,
    pub reserve_quote: Decimal,
    /// Base traded through the pool since it was first seen. Rolled back with
    /// everything else when a block is reorganised away.
    pub cumulative_base_volume: Decimal,
    pub trades: u64,
    pub last_block: BlockNumber,
}

impl PoolState {
    /// A priceable pool at these reserves.
    pub fn to_pool(&self, id: &PoolId) -> Result<Pool> {
        Pool::new(
            id.clone(),
            self.venue.clone(),
            self.base.clone(),
            self.quote.clone(),
            self.curve,
            self.fee,
            self.reserve_base,
            self.reserve_quote,
            self.last_block,
        )
    }
}

/// Everything the platform computes from blocks rather than reads from them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DerivedState {
    pools: BTreeMap<PoolId, PoolState>,
    trades: u64,
    gas_spent: Decimal,
}

impl DerivedState {
    /// A pool's reserves and counters as of this state.
    pub fn pool(&self, id: &PoolId) -> Option<&PoolState> {
        self.pools.get(id)
    }

    /// Every pool the chain has created, in identifier order.
    pub fn pools(&self) -> impl Iterator<Item = (&PoolId, &PoolState)> {
        self.pools.iter()
    }

    /// Swaps counted. Reverted transactions are not among them.
    pub const fn trades(&self) -> u64 {
        self.trades
    }

    /// Native currency burned, including by transactions that reverted.
    pub const fn gas_spent(&self) -> Decimal {
        self.gas_spent
    }
}

/// What one block displaced, so it can be put back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct BlockUndo {
    hash: BlockHash,
    number: BlockNumber,
    /// Previous value of every pool the block touched. `None` means the block
    /// created it and the rollback must remove it.
    pools: Vec<(PoolId, Option<PoolState>)>,
    trades: u64,
    gas_spent: Decimal,
}

impl BlockUndo {
    fn touched(&self) -> Vec<PoolId> {
        self.pools.iter().map(|(id, _)| id.clone()).collect()
    }
}

/// Derived state plus the history needed to unwind it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Ledger {
    derived: DerivedState,
    undo: Vec<BlockUndo>,
}

impl Ledger {
    /// Fold one block into the derived state, recording its undo entry.
    fn apply(&mut self, block: &Block) -> Result<()> {
        let mut undo = BlockUndo {
            hash: block.hash,
            number: block.number,
            pools: Vec::new(),
            trades: self.derived.trades,
            gas_spent: self.derived.gas_spent,
        };
        // Gas is charged on reverted transactions too: it is the cost of
        // trying, and a strategy that only counts the gas of its fills
        // understates what it spends by every attempt that lost the race.
        self.derived.gas_spent += block.gas_spent();

        for transaction in &block.transactions {
            for trace in transaction.effective_traces() {
                self.apply_trace(&trace.kind, block, &mut undo)?;
            }
        }
        self.undo.push(undo);
        Ok(())
    }

    fn apply_trace(&mut self, kind: &TraceKind, block: &Block, undo: &mut BlockUndo) -> Result<()> {
        match kind {
            TraceKind::PoolCreated {
                pool,
                venue,
                base,
                quote,
                curve,
                fee,
                reserve_base,
                reserve_quote,
            } => {
                self.record(undo, pool);
                self.derived.pools.insert(
                    pool.clone(),
                    PoolState {
                        venue: venue.clone(),
                        base: base.clone(),
                        quote: quote.clone(),
                        curve: *curve,
                        fee: *fee,
                        reserve_base: *reserve_base,
                        reserve_quote: *reserve_quote,
                        cumulative_base_volume: Decimal::ZERO,
                        trades: 0,
                        last_block: block.number,
                    },
                );
            }
            TraceKind::Swap {
                pool,
                taker,
                base_amount,
                quote_amount,
                ..
            } => {
                self.record(undo, pool);
                let state =
                    self.derived.pools.get_mut(pool).ok_or_else(|| {
                        Error::not_found(format!("swap against unknown pool {pool}"))
                    })?;
                // The taker's side decides which reserve grows: `Ask` means
                // the taker bought base out of the pool.
                let (base_after, quote_after) = match taker {
                    BookSide::Ask => (
                        state.reserve_base - *base_amount,
                        state.reserve_quote + *quote_amount,
                    ),
                    BookSide::Bid => (
                        state.reserve_base + *base_amount,
                        state.reserve_quote - *quote_amount,
                    ),
                };
                if !base_after.is_positive() || !quote_after.is_positive() {
                    return Err(Error::invalid(format!(
                        "swap in block {} would drain pool {pool} to {base_after}/{quote_after}",
                        block.number
                    )));
                }
                state.reserve_base = base_after;
                state.reserve_quote = quote_after;
                state.cumulative_base_volume += *base_amount;
                state.trades += 1;
                state.last_block = block.number;
                self.derived.trades += 1;
            }
            TraceKind::LiquidityChanged {
                pool,
                base_delta,
                quote_delta,
            } => {
                self.record(undo, pool);
                let state = self.derived.pools.get_mut(pool).ok_or_else(|| {
                    Error::not_found(format!("liquidity change on unknown pool {pool}"))
                })?;
                let base_after = state.reserve_base + *base_delta;
                let quote_after = state.reserve_quote + *quote_delta;
                if base_after.is_negative() || quote_after.is_negative() {
                    return Err(Error::invalid(format!(
                        "liquidity change in block {} would leave pool {pool} negative",
                        block.number
                    )));
                }
                state.reserve_base = base_after;
                state.reserve_quote = quote_after;
                state.last_block = block.number;
            }
            TraceKind::Transfer { .. }
            | TraceKind::BridgeDeposit { .. }
            | TraceKind::BridgeCredit { .. } => {}
        }
        Ok(())
    }

    /// Remember a pool's value before this block first touches it.
    fn record(&mut self, undo: &mut BlockUndo, pool: &PoolId) {
        if undo.pools.iter().any(|(id, _)| id == pool) {
            return;
        }
        undo.pools
            .push((pool.clone(), self.derived.pools.get(pool).cloned()));
    }

    /// Undo the most recently applied block.
    fn rollback(&mut self) -> Result<BlockUndo> {
        let undo = self
            .undo
            .pop()
            .ok_or_else(|| Error::invalid("no retained history left to roll back"))?;
        for (pool, previous) in &undo.pools {
            match previous {
                Some(state) => {
                    self.derived.pools.insert(pool.clone(), state.clone());
                }
                None => {
                    self.derived.pools.remove(pool);
                }
            }
        }
        self.derived.trades = undo.trades;
        self.derived.gas_spent = undo.gas_spent;
        Ok(undo)
    }

    /// Drop undo history older than the retention window.
    ///
    /// The pruned blocks stay canonical; only the ability to unwind them goes,
    /// which is why a reorg deeper than the window is refused rather than
    /// silently mis-applied.
    fn prune(&mut self, retention: usize) {
        if self.undo.len() > retention {
            let excess = self.undo.len() - retention;
            self.undo.drain(..excess);
        }
    }
}

/// What applying a block did to the canonical chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Applied {
    /// The block extended the head.
    Extended { number: BlockNumber },
    /// The block belonged to a branch that has now displaced part of the
    /// canonical chain.
    Reorganised(Reorg),
    /// The block was recorded but the canonical chain did not change: a fork
    /// that is not longer, or one that does not yet connect to anything.
    SideBranch { number: BlockNumber },
    /// Already seen. Nodes repeat themselves; that is not an error.
    Duplicate,
}

/// A reorganisation, and exactly what it invalidated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reorg {
    pub common_ancestor: BlockNumber,
    /// Blocks that were canonical and are not any more, deepest first.
    pub reverted: Vec<BlockHash>,
    /// Blocks that are canonical now, in application order.
    pub applied: Vec<BlockHash>,
    /// Pools whose derived state was rolled back. Everything not in this list
    /// was untouched by the reorg and does not need recomputing.
    pub invalidated_pools: Vec<PoolId>,
    /// Swaps that stopped having happened.
    pub invalidated_trades: u64,
}

impl Reorg {
    /// How many blocks were withdrawn.
    pub fn depth(&self) -> u32 {
        self.reverted.len() as u32
    }
}

/// The canonical chain and the state derived from it.
#[derive(Clone, Debug)]
pub struct ChainState {
    chain: ChainId,
    canonical: Vec<Block>,
    /// Every block seen, canonical or not. A fork only becomes reconstructible
    /// because its blocks were kept when they looked irrelevant.
    seen: BTreeMap<BlockHash, Block>,
    ledger: Ledger,
    retention: u32,
}

impl ChainState {
    /// `retention` is how many blocks of undo history to keep, and therefore
    /// the deepest reorg this state can survive.
    pub fn new(chain: ChainId, retention: u32) -> Self {
        Self {
            chain,
            canonical: Vec::new(),
            seen: BTreeMap::new(),
            ledger: Ledger::default(),
            retention,
        }
    }

    pub const fn chain(&self) -> &ChainId {
        &self.chain
    }

    pub const fn retention(&self) -> u32 {
        self.retention
    }

    pub fn head(&self) -> Option<&Block> {
        self.canonical.last()
    }

    pub fn head_number(&self) -> Option<BlockNumber> {
        self.head().map(|block| block.number)
    }

    /// The canonical chain, oldest retained block first.
    pub fn canonical(&self) -> &[Block] {
        &self.canonical
    }

    /// Whether a block is on the canonical chain right now.
    pub fn is_canonical(&self, hash: &BlockHash) -> bool {
        self.canonical.iter().any(|block| &block.hash == hash)
    }

    /// Apply a block, extending, forking or reorganising as its parent dictates.
    pub fn apply(&mut self, block: Block) -> Result<Applied> {
        if block.chain != self.chain {
            return Err(Error::invalid(format!(
                "block from chain {} applied to state for {}",
                block.chain, self.chain
            )));
        }
        block.validate()?;
        if let Some(existing) = self.seen.get(&block.hash) {
            // A hash identifies a block's contents, not just its slot in the
            // chain. Treating any second arrival under the same hash as a
            // harmless duplicate — without checking it actually matches what
            // is already recorded — would let a corrupted or malicious
            // resend silently replace sealed history with different
            // transactions while every caller still sees the same hash and
            // believes nothing changed. Two different blocks are never
            // allowed to share a hash; a genuine duplicate must be
            // bit-for-bit identical to what this state already retained.
            if existing == &block {
                return Ok(Applied::Duplicate);
            }
            return Err(Error::invalid(format!(
                "block {} was already recorded with different contents; a hash may not be reused for different data",
                block.hash
            )));
        }
        let hash = block.hash;
        let number = block.number;
        self.seen.insert(hash, block);

        let Some(head) = self.head() else {
            self.adopt(&[hash])?;
            return Ok(Applied::Extended { number });
        };

        let follows_head = self.seen.get(&hash).is_some_and(|block| {
            block.parent_hash == head.hash && block.number == head.number.next()
        });
        if follows_head {
            self.adopt(&[hash])?;
            return Ok(Applied::Extended { number });
        }

        // Walk back through blocks we kept until the branch meets the chain.
        let Some((ancestor, branch)) = self.branch_to_canonical(&hash) else {
            return Ok(Applied::SideBranch { number });
        };
        let ancestor_number = self
            .seen
            .get(&ancestor)
            .map(|block| block.number)
            .unwrap_or(number);
        if number <= self.head_number().unwrap_or(number) {
            // Shorter or equal: the fork is kept in case it grows, but the
            // canonical chain does not move for a tie.
            return Ok(Applied::SideBranch { number });
        }
        self.reorganise(ancestor_number, &branch)
            .map(Applied::Reorganised)
    }

    /// Reconstruct the branch ending at `tip` back to its canonical ancestor.
    fn branch_to_canonical(&self, tip: &BlockHash) -> Option<(BlockHash, Vec<BlockHash>)> {
        let mut branch = Vec::new();
        let mut cursor = *tip;
        for _ in 0..=self.retention {
            let block = self.seen.get(&cursor)?;
            if self.is_canonical(&cursor) {
                branch.reverse();
                return Some((cursor, branch));
            }
            branch.push(cursor);
            cursor = block.parent_hash;
        }
        None
    }

    fn reorganise(&mut self, ancestor: BlockNumber, branch: &[BlockHash]) -> Result<Reorg> {
        // Work on a copy so a branch that fails half way through leaves the
        // canonical state exactly as it was.
        let mut ledger = self.ledger.clone();
        let mut reverted = Vec::new();
        let mut invalidated_pools: Vec<PoolId> = Vec::new();
        let trades_before = ledger.derived.trades;

        let mut kept = self.canonical.len();
        while kept > 0 && self.canonical[kept - 1].number > ancestor {
            let undo = ledger.rollback().map_err(|_| {
                Error::invalid(format!(
                    "a reorg back to {ancestor} is deeper than the {} blocks of history retained",
                    self.retention
                ))
            })?;
            reverted.push(undo.hash);
            for pool in undo.touched() {
                if !invalidated_pools.contains(&pool) {
                    invalidated_pools.push(pool);
                }
            }
            kept -= 1;
        }

        let mut applied = Vec::new();
        for hash in branch {
            let block = self
                .seen
                .get(hash)
                .ok_or_else(|| Error::not_found(format!("branch block {hash} was not retained")))?;
            ledger.apply(block)?;
            applied.push(*hash);
        }
        let trades_after = ledger.derived.trades;

        self.canonical.truncate(kept);
        for hash in branch {
            if let Some(block) = self.seen.get(hash) {
                self.canonical.push(block.clone());
            }
        }
        ledger.prune(self.retention as usize);
        self.ledger = ledger;
        self.prune_canonical();

        Ok(Reorg {
            common_ancestor: ancestor,
            reverted,
            applied,
            invalidated_pools,
            invalidated_trades: trades_before.saturating_sub(trades_after),
        })
    }

    fn adopt(&mut self, hashes: &[BlockHash]) -> Result<()> {
        let mut ledger = self.ledger.clone();
        let mut blocks = Vec::new();
        for hash in hashes {
            let block = self
                .seen
                .get(hash)
                .ok_or_else(|| Error::not_found(format!("block {hash} was not retained")))?;
            ledger.apply(block)?;
            blocks.push(block.clone());
        }
        ledger.prune(self.retention as usize);
        self.ledger = ledger;
        self.canonical.extend(blocks);
        self.prune_canonical();
        Ok(())
    }

    fn prune_canonical(&mut self) {
        let retention = self.retention as usize;
        if self.canonical.len() > retention {
            let excess = self.canonical.len() - retention;
            let dropped: Vec<BlockHash> = self.canonical.drain(..excess).map(|b| b.hash).collect();
            for hash in dropped {
                self.seen.remove(&hash);
            }
        }
    }

    /// How settled a block is, against the depth the caller requires.
    pub fn finality_of(&self, hash: &BlockHash, required: Confirmations) -> Finality {
        let Some(block) = self.seen.get(hash) else {
            return Finality::Pending;
        };
        let Some(head) = self.head_number() else {
            return Finality::Pending;
        };
        if !self.is_canonical(hash) {
            return Finality::Orphaned {
                was_at: block.number,
            };
        }
        Finality::of_block(block.number, head, required)
    }

    /// Derived state as of the requested confirmation depth.
    ///
    /// There is no accessor that skips this. Reading chain state without
    /// saying how settled it has to be is the mistake this signature exists to
    /// make impossible; a caller that genuinely wants the head asks for
    /// [`Confirmations::AT_RISK`] and says so in its own code.
    pub fn view(&self, required: Confirmations) -> Result<ConfirmedView> {
        let depth = required.depth() as usize;
        if depth > self.ledger.undo.len() {
            return Err(Error::invalid(format!(
                "a view {depth} blocks deep needs {depth} blocks of undo history; {} are retained",
                self.ledger.undo.len()
            )));
        }
        let mut ledger = self.ledger.clone();
        for _ in 0..depth {
            ledger.rollback()?;
        }
        Ok(ConfirmedView {
            required,
            head: self.head_number(),
            as_of: ledger.undo.last().map(|undo| undo.number),
            derived: ledger.derived,
        })
    }
}

/// Derived state at a stated confirmation depth.
///
/// Carries the depth it was taken at, so a value that travels downstream keeps
/// the assumption that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmedView {
    required: Confirmations,
    head: Option<BlockNumber>,
    as_of: Option<BlockNumber>,
    derived: DerivedState,
}

impl ConfirmedView {
    pub const fn required(&self) -> Confirmations {
        self.required
    }

    /// The deepest block included in this view.
    pub const fn as_of(&self) -> Option<BlockNumber> {
        self.as_of
    }

    pub const fn head(&self) -> Option<BlockNumber> {
        self.head
    }

    pub const fn state(&self) -> &DerivedState {
        &self.derived
    }

    /// A priceable pool at the confirmation depth this view was taken at.
    pub fn pool(&self, id: &PoolId) -> Result<Pool> {
        self.derived
            .pool(id)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "pool {id} is not present at {} of confirmation",
                    self.required
                ))
            })?
            .to_pool(id)
    }
}
