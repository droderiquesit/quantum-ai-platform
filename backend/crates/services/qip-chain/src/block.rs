//! Blocks, transactions and traces.
//!
//! The chain's unit of publication is the block, and the properties that
//! matter to a trader are not the ones a block explorer leads with: the base
//! fee that sets the floor on execution cost, the position of a transaction
//! within the block, the gas it actually burned, and — above all — whether it
//! succeeded.
//!
//! A reverted transaction is the case this module exists to get right. It sits
//! in the block, it consumed gas, it emitted call frames, and it moved
//! nothing. Counting one as a trade inflates volume, corrupts VWAP and invents
//! liquidity that was never there, so a revert is modelled as a state of the
//! transaction rather than a flag on it, and the effective traces of a
//! reverted transaction are empty by construction.

use crate::amm::{FeeBps, PoolCurve, PoolId};
use crate::units::TokenAmount;
use qip_contracts::{BookSide, MarketMessage, MessageBody, Origin, TradeCondition, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A 32-byte digest, rendered as lower-case hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hash32([u8; 32]);

impl Hash32 {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Digest of the concatenated parts, using the in-tree SHA-256.
    pub fn of(parts: &[&[u8]]) -> Self {
        let mut hasher = qip_core::Hasher256::new();
        for part in parts {
            hasher.update(part);
        }
        Self(hasher.finish())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Eight bytes is enough to identify a block in a log line and short
        // enough that the line stays readable.
        for byte in &self.0[..8] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash32({self})")
    }
}

/// A block's identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockHash(Hash32);

impl BlockHash {
    pub const fn new(hash: Hash32) -> Self {
        Self(hash)
    }

    pub const fn hash(&self) -> Hash32 {
        self.0
    }
}

impl fmt::Display for BlockHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A transaction's identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TxHash(Hash32);

impl TxHash {
    pub const fn new(hash: Hash32) -> Self {
        Self(hash)
    }

    pub const fn hash(&self) -> Hash32 {
        self.0
    }
}

impl fmt::Display for TxHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An account or contract address, as the chain writes it.
///
/// Opaque and case-preserving for the same reason [`VenueId`] is: normalising
/// would merge two identifiers that the chain considers distinct.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Address(String);

impl Address {
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which chain a block belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChainId(String);

impl ChainId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Height of a block in its chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockNumber(u64);

impl BlockNumber {
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    pub const fn get(&self) -> u64 {
        self.0
    }

    pub const fn next(&self) -> Self {
        Self(self.0 + 1)
    }

    /// How many blocks `self` is behind `head`. Zero when it is the head.
    pub const fn depth_below(&self, head: Self) -> u64 {
        head.0.saturating_sub(self.0)
    }
}

impl fmt::Display for BlockNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Whether a transaction did anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxStatus {
    /// The transaction executed and its state changes are in the block.
    Succeeded,
    /// The transaction executed, reverted, and changed nothing but the gas
    /// balance of its sender.
    Reverted {
        /// The revert reason the node reported, where it decoded one.
        reason: String,
    },
}

impl TxStatus {
    pub const fn succeeded(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Reverted { .. } => "reverted",
        }
    }
}

/// What one call frame inside a transaction did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TraceKind {
    /// A pool came into existence with its opening reserves.
    PoolCreated {
        pool: PoolId,
        /// The protocol the pool belongs to, which is the venue a fill on it
        /// is attributed to.
        venue: VenueId,
        base: ObjectId,
        quote: ObjectId,
        curve: PoolCurve,
        fee: FeeBps,
        reserve_base: Decimal,
        reserve_quote: Decimal,
    },
    /// A swap against a pool.
    ///
    /// `taker` is the side the taker lifted: [`BookSide::Ask`] means the taker
    /// bought base and the pool's base reserve fell. Both amounts are positive
    /// and the direction is carried by the side, so no caller has to remember
    /// a sign convention.
    Swap {
        pool: PoolId,
        /// The platform instrument this pool prices.
        object_id: ObjectId,
        taker: BookSide,
        base_amount: Decimal,
        quote_amount: Decimal,
    },
    /// Liquidity was added to or removed from a pool, signed by direction.
    LiquidityChanged {
        pool: PoolId,
        base_delta: Decimal,
        quote_delta: Decimal,
    },
    /// A token moved between accounts.
    Transfer {
        object_id: ObjectId,
        from: Address,
        to: Address,
        amount: TokenAmount,
    },
    /// Value was locked on the source chain of a bridge route.
    BridgeDeposit {
        transfer: crate::bridge::TransferId,
        object_id: ObjectId,
        amount: Decimal,
        destination: ChainId,
    },
    /// Value was released on the destination chain of a bridge route.
    BridgeCredit {
        transfer: crate::bridge::TransferId,
        object_id: ObjectId,
        amount: Decimal,
    },
}

impl TraceKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::PoolCreated { .. } => "pool_created",
            Self::Swap { .. } => "swap",
            Self::LiquidityChanged { .. } => "liquidity_changed",
            Self::Transfer { .. } => "transfer",
            Self::BridgeDeposit { .. } => "bridge_deposit",
            Self::BridgeCredit { .. } => "bridge_credit",
        }
    }
}

/// One decoded call frame, in execution order within its transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    /// Position within the transaction. Two traces of the same transaction are
    /// ordered by this and never by anything else.
    pub index: u32,
    pub kind: TraceKind,
}

impl Trace {
    pub fn new(index: u32, kind: TraceKind) -> Self {
        Self { index, kind }
    }
}

/// A transaction as the chain executed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub hash: TxHash,
    /// Position within the block. This is the only ordering that exists: the
    /// order a transaction was broadcast in has no bearing on it.
    pub index: u32,
    pub from: Address,
    pub to: Option<Address>,
    pub status: TxStatus,
    pub gas_used: u64,
    /// Native currency paid per unit of gas, in whole native units. Nine
    /// fractional digits is exactly gwei for an eighteen-decimal native token,
    /// so gas prices are represented without rounding.
    pub effective_gas_price: Decimal,
    pub traces: Vec<Trace>,
}

impl Transaction {
    /// Traces that actually changed the chain.
    ///
    /// Empty for a reverted transaction. This is the accessor every consumer
    /// should use; reaching for [`Transaction::traces`] directly is asking to
    /// count a revert as a fill.
    pub fn effective_traces(&self) -> &[Trace] {
        if self.status.succeeded() {
            &self.traces
        } else {
            &[]
        }
    }

    /// Whether this transaction traded. False for every reverted transaction,
    /// however much it looks like a swap.
    pub fn is_trade(&self) -> bool {
        self.effective_traces()
            .iter()
            .any(|trace| matches!(trace.kind, TraceKind::Swap { .. }))
    }

    /// Native currency burned. Charged on a revert exactly as on a success,
    /// which is why gas is a cost of trying rather than a cost of trading.
    pub fn gas_cost(&self) -> Decimal {
        Decimal::from_int(self.gas_used as i64) * self.effective_gas_price
    }
}

/// A block and everything in it that the platform decodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub chain: ChainId,
    pub number: BlockNumber,
    pub hash: BlockHash,
    pub parent_hash: BlockHash,
    /// The timestamp the proposer stamped. Not this cell's clock, and not
    /// necessarily monotone across a reorg.
    pub timestamp: Timestamp,
    /// EIP-1559 base fee per gas in whole native units.
    pub base_fee: Decimal,
    pub gas_used: u64,
    pub gas_limit: u64,
    pub transactions: Vec<Transaction>,
}

impl Block {
    /// Structural checks that must hold before a block may drive state.
    pub fn validate(&self) -> Result<()> {
        if self.base_fee.is_negative() {
            return Err(Error::invalid(format!(
                "block {} has a negative base fee",
                self.number
            )));
        }
        if self.gas_used > self.gas_limit {
            return Err(Error::invalid(format!(
                "block {} used {} gas against a limit of {}",
                self.number, self.gas_used, self.gas_limit
            )));
        }
        for (position, transaction) in self.transactions.iter().enumerate() {
            if transaction.index as usize != position {
                return Err(Error::invalid(format!(
                    "block {} has transaction {} at position {position}",
                    self.number, transaction.index
                )));
            }
        }
        Ok(())
    }

    /// Transactions that succeeded, in block order.
    pub fn effective_transactions(&self) -> impl Iterator<Item = &Transaction> {
        self.transactions.iter().filter(|tx| tx.status.succeeded())
    }

    /// Swaps that actually happened, in block order.
    pub fn trades(&self) -> Vec<&Trace> {
        self.transactions
            .iter()
            .flat_map(|tx| tx.effective_traces())
            .filter(|trace| matches!(trace.kind, TraceKind::Swap { .. }))
            .collect()
    }

    /// Native currency burned across every transaction, reverts included.
    pub fn gas_spent(&self) -> Decimal {
        self.transactions
            .iter()
            .fold(Decimal::ZERO, |total, tx| total + tx.gas_cost())
    }

    /// Fraction of the gas limit used, as a statistic for congestion models.
    pub fn utilisation(&self) -> f64 {
        if self.gas_limit == 0 {
            return 0.0;
        }
        self.gas_used as f64 / self.gas_limit as f64
    }

    /// Render the block's effective swaps as market messages.
    ///
    /// The chain becomes a feed like any other here: downstream nothing knows
    /// whether a trade came from an exchange's binary protocol or from a log
    /// in a block. Reverted transactions contribute nothing, so a consumer
    /// cannot accidentally price off one.
    ///
    /// `first_sequence` is supplied by the adapter that owns the stream, since
    /// sequence numbers are only meaningful within one.
    pub fn market_messages(
        &self,
        venue: &VenueId,
        feed: &str,
        first_sequence: u64,
        captured_at: Timestamp,
    ) -> Vec<MarketMessage> {
        let mut sequence = first_sequence;
        let mut messages = Vec::new();
        for transaction in self.transactions.iter() {
            for trace in transaction.effective_traces() {
                let TraceKind::Swap {
                    object_id,
                    taker,
                    base_amount,
                    quote_amount,
                    ..
                } = &trace.kind
                else {
                    continue;
                };
                let Some(price) = quote_amount.checked_div(*base_amount) else {
                    continue;
                };
                let origin = Origin::new(venue.clone(), feed, 0, sequence);
                sequence += 1;
                messages.push(MarketMessage::new(
                    object_id.clone(),
                    origin,
                    MessageBody::Trade {
                        price,
                        quantity: *base_amount,
                        condition: TradeCondition::Regular,
                        aggressor: Some(*taker),
                    },
                    self.timestamp,
                    captured_at,
                ));
            }
        }
        messages
    }
}
