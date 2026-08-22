//! Chain adapter ports.
//!
//! Pull, not push: `poll(until)` hands the caller the clock, which is what
//! lets one adapter drive a live run, a backtest and a replay without any of
//! them knowing which they are. The synthetic implementation is the one that
//! ships, and it is deterministic to the bit given its seed — including its
//! reorganisations, which is the only way to test the code that handles them.
//!
//! No node is reachable from this build. [`NodeChainAdapter`] exists so the
//! shape of the real thing is compiled and reviewed, and it refuses every call
//! by naming the RPC methods and the credential a deployment has to supply. It
//! does not pretend to connect.

use qip_contracts::{BookSide, VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::amm::{FeeBps, Pool, PoolCurve, PoolId};
use crate::block::{
    Address, Block, BlockHash, BlockNumber, ChainId, Hash32, Trace, TraceKind, Transaction, TxHash,
    TxStatus,
};
use crate::mempool::PendingTransaction;

/// What an adapter is, and what a deployment must supply to make it real.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainDescriptor {
    pub name: String,
    pub chain: ChainId,
    pub venue: VenueId,
    pub class: VenueClass,
    /// Nominal time between blocks. Not a guarantee: the interval between two
    /// blocks is a random variable and the tail is what costs money.
    pub block_time: Duration,
    /// Whether the data is generated rather than observed.
    pub is_synthetic: bool,
    /// What a production deployment must supply, when this is a stand-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production_requirement: Option<String>,
}

/// One thing an adapter observed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ChainUpdate {
    /// A block, canonical or not. The adapter does not decide which: it
    /// reports what the node served, and [`crate::state::ChainState`] works
    /// out whether it extends, forks or displaces.
    Block(Box<Block>),
    /// A transaction seen in the mempool.
    Pending(Box<PendingTransaction>),
    /// A transaction that left the mempool without being included.
    Dropped(TxHash),
}

impl ChainUpdate {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Block(_) => "block",
            Self::Pending(_) => "pending",
            Self::Dropped(_) => "dropped",
        }
    }
}

/// The common chain adapter contract.
pub trait ChainAdapter: std::fmt::Debug {
    fn descriptor(&self) -> ChainDescriptor;

    /// Everything observed up to and including `until`.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<ChainUpdate>>;

    fn start(&mut self, _at: Timestamp) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

/// How the synthetic chain behaves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyntheticChainConfig {
    pub chain: ChainId,
    pub venue: VenueId,
    pub pool: PoolId,
    pub base: ObjectId,
    pub quote: ObjectId,
    pub curve: PoolCurve,
    pub fee: FeeBps,
    pub reserve_base: Decimal,
    pub reserve_quote: Decimal,
    pub block_time: Duration,
    pub seed: u64,
    /// Probability that a block arrives on a branch that displaces the head.
    pub reorg_probability: f64,
    pub max_reorg_depth: u32,
    /// Probability that a swap transaction reverts. Reverted swaps are emitted
    /// in full, traces included, because that is what a node serves and what
    /// a consumer has to refuse to count.
    pub revert_probability: f64,
    pub max_swaps_per_block: u32,
    pub base_fee: Decimal,
    pub pending_per_block: u32,
}

impl SyntheticChainConfig {
    /// A two-token pool on a plausible layer-one, for tests and demos.
    pub fn demo(seed: u64) -> Result<Self> {
        Ok(Self {
            chain: ChainId::new("synthetic-1"),
            venue: VenueId::new("SYNTH-DEX"),
            pool: PoolId::new("pool-base-quote"),
            base: ObjectId::from_string("SYNTH-BASE"),
            quote: ObjectId::from_string("SYNTH-QUOTE"),
            curve: PoolCurve::ConstantProduct,
            fee: FeeBps::new(30)?,
            reserve_base: Decimal::from_int(2_000),
            reserve_quote: Decimal::from_int(4_000_000),
            block_time: Duration::from_secs(12),
            seed,
            reorg_probability: 0.05,
            max_reorg_depth: 3,
            revert_probability: 0.1,
            max_swaps_per_block: 3,
            base_fee: Decimal::from_raw(20),
            pending_per_block: 2,
        })
    }
}

/// How many pending transactions the synthetic mempool holds before the
/// oldest is dropped unmined.
const MEMPOOL_WINDOW: usize = 8;

/// One produced block and the pool reserves it displaced.
#[derive(Clone, Debug)]
struct Produced {
    block: Block,
    reserve_base_before: Decimal,
    reserve_quote_before: Decimal,
}

/// A deterministic chain that produces blocks, swaps, reverts and reorgs.
///
/// It prices its own swaps through [`Pool`], so the trades it emits satisfy
/// the same curve arithmetic the rest of the crate computes against. A
/// synthetic feed whose fills do not satisfy its own invariant would let a
/// broken quote function pass its tests.
#[derive(Debug)]
pub struct SyntheticChain {
    config: SyntheticChainConfig,
    rng: Xoshiro256,
    pool: Pool,
    branch: Vec<Produced>,
    next_block_at: Timestamp,
    nonce: u64,
    sender_nonces: BTreeMap<Address, u64>,
    /// Pending transactions still believed to be in the mempool. A public
    /// mempool forgets what it does not include, and a consumer that never
    /// hears so keeps sizing against liquidity that has gone.
    pending_window: Vec<TxHash>,
    started: bool,
}

impl SyntheticChain {
    /// Start a chain at `start`, with the pool the config describes already
    /// funded.
    pub fn new(config: SyntheticChainConfig, start: Timestamp) -> Result<Self> {
        let pool = Pool::new(
            config.pool.clone(),
            config.venue.clone(),
            config.base.clone(),
            config.quote.clone(),
            config.curve,
            config.fee,
            config.reserve_base,
            config.reserve_quote,
            BlockNumber::new(0),
        )?;
        let rng = Xoshiro256::seeded(config.seed);
        Ok(Self {
            config,
            rng,
            pool,
            branch: Vec::new(),
            next_block_at: start,
            nonce: 0,
            sender_nonces: BTreeMap::new(),
            pending_window: Vec::new(),
            started: true,
        })
    }

    /// The pool as the synthetic chain's own canonical branch leaves it.
    ///
    /// A consumer that applies the emitted blocks must arrive here; when it
    /// does not, one of the two is mishandling reverts or reorgs.
    pub const fn pool(&self) -> &Pool {
        &self.pool
    }

    /// The height of the branch this chain is currently building on.
    pub fn head(&self) -> Option<BlockNumber> {
        self.branch.last().map(|produced| produced.block.number)
    }

    fn head_hash(&self) -> BlockHash {
        self.branch
            .last()
            .map(|produced| produced.block.hash)
            .unwrap_or_else(|| BlockHash::new(Hash32::of(&[b"genesis"])))
    }

    /// Rewind the branch, restoring the reserves each dropped block changed.
    fn rewind(&mut self, depth: usize) {
        for _ in 0..depth {
            let Some(produced) = self.branch.pop() else {
                return;
            };
            // Restoring the recorded reserves rather than replaying backwards:
            // a swap is not invertible through the fee, so an "undo" computed
            // from the trade would not land where it started.
            let _ = self.pool.set_reserves(
                produced.reserve_base_before,
                produced.reserve_quote_before,
                produced.block.number,
            );
        }
    }

    fn next_hash(&mut self, number: BlockNumber, parent: BlockHash) -> BlockHash {
        self.nonce += 1;
        BlockHash::new(Hash32::of(&[
            self.config.chain.as_str().as_bytes(),
            &number.get().to_le_bytes(),
            parent.hash().as_bytes(),
            &self.nonce.to_le_bytes(),
        ]))
    }

    fn produce_block(&mut self) -> Result<Block> {
        let number = self.head().map_or(BlockNumber::new(1), |head| head.next());
        let parent = self.head_hash();
        let hash = self.next_hash(number, parent);
        let timestamp = self.next_block_at;
        self.next_block_at = self.next_block_at.saturating_add(self.config.block_time);

        let reserve_base_before = self.pool.reserve_base();
        let reserve_quote_before = self.pool.reserve_quote();

        let mut transactions = Vec::new();
        if number.get() == 1 {
            transactions.push(self.creation_transaction(
                number,
                reserve_base_before,
                reserve_quote_before,
            ));
        }
        let swaps = self
            .rng
            .below(u64::from(self.config.max_swaps_per_block) + 1);
        for _ in 0..swaps {
            if let Some(transaction) = self.swap_transaction(transactions.len() as u32)? {
                transactions.push(transaction);
            }
        }

        let gas_used = transactions.iter().map(|tx| tx.gas_used).sum();
        let block = Block {
            chain: self.config.chain.clone(),
            number,
            hash,
            parent_hash: parent,
            timestamp,
            base_fee: self.config.base_fee,
            gas_used,
            gas_limit: 30_000_000,
            transactions,
        };
        self.branch.push(Produced {
            block: block.clone(),
            reserve_base_before,
            reserve_quote_before,
        });
        Ok(block)
    }

    fn creation_transaction(
        &mut self,
        number: BlockNumber,
        reserve_base: Decimal,
        reserve_quote: Decimal,
    ) -> Transaction {
        let hash = TxHash::new(Hash32::of(&[b"pool-creation", &number.get().to_le_bytes()]));
        Transaction {
            hash,
            index: 0,
            from: Address::new("0xdeployer"),
            to: None,
            status: TxStatus::Succeeded,
            gas_used: 2_500_000,
            effective_gas_price: self.config.base_fee,
            traces: vec![Trace::new(
                0,
                TraceKind::PoolCreated {
                    pool: self.config.pool.clone(),
                    venue: self.config.venue.clone(),
                    base: self.config.base.clone(),
                    quote: self.config.quote.clone(),
                    curve: self.config.curve,
                    fee: self.config.fee,
                    reserve_base,
                    reserve_quote,
                },
            )],
        }
    }

    fn swap_transaction(&mut self, index: u32) -> Result<Option<Transaction>> {
        let taker = if self.rng.bernoulli(0.5) {
            BookSide::Ask
        } else {
            BookSide::Bid
        };
        let reserve_in = match taker {
            BookSide::Ask => self.pool.reserve_quote(),
            BookSide::Bid => self.pool.reserve_base(),
        };
        let fraction = self.rng.uniform(0.0002, 0.01);
        let Some(factor) = Decimal::from_f64(fraction) else {
            return Ok(None);
        };
        let Some(amount_in) = reserve_in.checked_mul(factor) else {
            return Ok(None);
        };
        if !amount_in.is_positive() {
            return Ok(None);
        }
        let Ok(quote) = self.pool.quote_exact_in(taker, amount_in) else {
            return Ok(None);
        };

        let reverted = self.rng.bernoulli(self.config.revert_probability);
        self.nonce += 1;
        let hash = TxHash::new(Hash32::of(&[b"swap", &self.nonce.to_le_bytes()]));
        let sender = Address::new(format!("0xtrader{}", self.rng.below(4)));
        let tip = Decimal::from_raw(1 + self.rng.below(5) as i128);
        let transaction = Transaction {
            hash,
            index,
            from: sender,
            to: Some(Address::new(self.config.pool.as_str())),
            status: if reverted {
                TxStatus::Reverted {
                    reason: "insufficient output amount".to_string(),
                }
            } else {
                TxStatus::Succeeded
            },
            gas_used: 140_000,
            effective_gas_price: self.config.base_fee + tip,
            traces: vec![Trace::new(
                0,
                TraceKind::Swap {
                    pool: self.config.pool.clone(),
                    object_id: self.config.base.clone(),
                    taker,
                    base_amount: match taker {
                        BookSide::Ask => quote.amount_out,
                        BookSide::Bid => quote.amount_in,
                    },
                    quote_amount: match taker {
                        BookSide::Ask => quote.amount_in,
                        BookSide::Bid => quote.amount_out,
                    },
                },
            )],
        };
        // A revert moves nothing but the gas balance, so the pool is left
        // exactly as it was.
        if !reverted {
            self.pool.apply(&quote)?;
        }
        Ok(Some(transaction))
    }

    fn pending_transaction(&mut self, at: Timestamp) -> PendingTransaction {
        self.nonce += 1;
        let sender = Address::new(format!("0xtrader{}", self.rng.below(4)));
        let nonce = self.sender_nonces.entry(sender.clone()).or_insert(0);
        let sender_nonce = *nonce;
        *nonce += 1;
        let tip = Decimal::from_raw(1 + self.rng.below(9) as i128);
        PendingTransaction {
            hash: TxHash::new(Hash32::of(&[b"pending", &self.nonce.to_le_bytes()])),
            from: sender,
            nonce: sender_nonce,
            gas_limit: 200_000,
            max_fee_per_gas: self.config.base_fee + tip + Decimal::from_raw(10),
            max_priority_fee_per_gas: tip,
            first_seen: at,
            intent: None,
        }
    }
}

impl ChainAdapter for SyntheticChain {
    fn descriptor(&self) -> ChainDescriptor {
        ChainDescriptor {
            name: "synthetic-chain".to_string(),
            chain: self.config.chain.clone(),
            venue: self.config.venue.clone(),
            class: VenueClass::DecentralisedExchange,
            block_time: self.config.block_time,
            is_synthetic: true,
            production_requirement: Some(
                "a node RPC endpoint; see NodeChainAdapter for the exact methods".to_string(),
            ),
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<ChainUpdate>> {
        if !self.started {
            return Err(Error::invalid("the synthetic chain was stopped"));
        }
        let mut updates = Vec::new();
        while self.next_block_at <= until {
            let depth = self.rng.below(u64::from(self.config.max_reorg_depth)) as usize + 1;
            let reorg =
                self.rng.bernoulli(self.config.reorg_probability) && self.branch.len() > depth;
            if reorg {
                // Rewind and build one block more than was withdrawn, so the
                // branch is longer and a consumer must reorganise onto it.
                self.rewind(depth);
                for _ in 0..=depth {
                    let block = self.produce_block()?;
                    updates.push(ChainUpdate::Block(Box::new(block)));
                }
            } else {
                let block = self.produce_block()?;
                updates.push(ChainUpdate::Block(Box::new(block)));
            }
            let at = self.next_block_at;
            for _ in 0..self.config.pending_per_block {
                let pending = self.pending_transaction(at);
                self.pending_window.push(pending.hash);
                updates.push(ChainUpdate::Pending(Box::new(pending)));
                if self.pending_window.len() > MEMPOOL_WINDOW {
                    updates.push(ChainUpdate::Dropped(self.pending_window.remove(0)));
                }
            }
        }
        Ok(updates)
    }

    fn stop(&mut self) -> Result<()> {
        self.started = false;
        Ok(())
    }
}

/// What a real node adapter needs before it can do anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeConfig {
    pub chain: ChainId,
    pub venue: VenueId,
    /// Environment variable holding the JSON-RPC endpoint.
    pub endpoint_env: String,
    /// Environment variable holding the provider credential.
    pub credential_env: String,
    /// RPC methods the adapter calls. Listed so an operator can check an
    /// endpoint's method allow-list before deploying rather than after.
    pub required_methods: Vec<String>,
}

impl NodeConfig {
    /// The methods this crate's adapter needs from an archive-capable node.
    pub fn ethereum_like(chain: ChainId, venue: VenueId) -> Self {
        Self {
            chain,
            venue,
            endpoint_env: "QIP_CHAIN_RPC_ENDPOINT".to_string(),
            credential_env: "QIP_CHAIN_RPC_CREDENTIAL".to_string(),
            required_methods: vec![
                "eth_getBlockByNumber".to_string(),
                "eth_getBlockReceipts".to_string(),
                "eth_getLogs".to_string(),
                "debug_traceBlockByHash".to_string(),
                "txpool_content".to_string(),
            ],
        }
    }
}

/// The real-node adapter, declared and unavailable.
///
/// It reports its requirement rather than failing obscurely at the first call,
/// and it never returns data. A synthetic block that a caller believes came
/// from a node is worse than no block at all.
#[derive(Clone, Debug)]
pub struct NodeChainAdapter {
    config: NodeConfig,
    endpoint_present: bool,
    credential_present: bool,
}

impl NodeChainAdapter {
    /// `endpoint_present` and `credential_present` are supplied by the
    /// composition root, which is the only layer allowed to read the
    /// environment.
    pub fn new(config: NodeConfig, endpoint_present: bool, credential_present: bool) -> Self {
        Self {
            config,
            endpoint_present,
            credential_present,
        }
    }

    pub const fn config(&self) -> &NodeConfig {
        &self.config
    }

    pub const fn is_available(&self) -> bool {
        self.endpoint_present && self.credential_present
    }

    /// Exactly what is missing, named so an operator can act on it.
    pub fn requirement(&self) -> String {
        let mut missing = Vec::new();
        if !self.endpoint_present {
            missing.push(format!(
                "a JSON-RPC endpoint in the environment variable {}",
                self.config.endpoint_env
            ));
        }
        if !self.credential_present {
            missing.push(format!(
                "a provider credential in the environment variable {}",
                self.config.credential_env
            ));
        }
        format!(
            "chain {} needs {} exposing the RPC methods {}",
            self.config.chain,
            if missing.is_empty() {
                "a transport implementation".to_string()
            } else {
                missing.join(" and ")
            },
            self.config.required_methods.join(", ")
        )
    }
}

impl ChainAdapter for NodeChainAdapter {
    fn descriptor(&self) -> ChainDescriptor {
        ChainDescriptor {
            name: "node-rpc".to_string(),
            chain: self.config.chain.clone(),
            venue: self.config.venue.clone(),
            class: VenueClass::DecentralisedExchange,
            block_time: Duration::from_secs(12),
            is_synthetic: false,
            production_requirement: Some(self.requirement()),
        }
    }

    fn poll(&mut self, _until: Timestamp) -> Result<Vec<ChainUpdate>> {
        Err(Error::unavailable(self.requirement()))
    }

    fn start(&mut self, _at: Timestamp) -> Result<()> {
        Err(Error::unavailable(self.requirement()))
    }
}
