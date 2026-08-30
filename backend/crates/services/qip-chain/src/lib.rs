//! `qip-chain` — on-chain state as market data.
//!
//! A chain is a venue with three properties no exchange has, and this crate
//! exists to stop the rest of the platform from forgetting them.
//!
//! 1. **The past is revisable.** A block that was the head can stop existing,
//!    and every reserve, fill and position derived from it goes with it.
//!    [`ChainState`] models that directly: applied blocks record what they
//!    displaced, a reorg rolls back to the common ancestor and replays the
//!    winning branch, and no derived state is readable without the caller
//!    stating the [`Confirmations`] it needs.
//! 2. **The price is computed, not quoted.** An [`amm::Pool`] holds reserves
//!    and a curve; the price is what those imply for the size being traded,
//!    fee and impact included, in exact integer arithmetic at the width the
//!    contract uses.
//! 3. **Nothing is promised.** The [`Mempool`] ordering is a prediction that
//!    a builder owes no one, gas is charged whether or not the transaction
//!    achieves anything, and a bridged position exists on neither chain for
//!    the duration of the transfer.
//!
//! The chain is a first-class feed rather than an appendix to one:
//! [`Block::market_messages`] renders its effective swaps as
//! [`qip_contracts::MarketMessage`]s, so downstream nothing has to know which
//! wire a trade arrived on. Reverted transactions produce nothing there — they
//! consumed gas, moved no value, and counting one as a trade invents liquidity
//! that never existed.

pub mod adapter;
pub mod amm;
pub mod block;
pub mod bridge;
pub mod finality;
pub mod gas;
pub mod math;
pub mod mempool;
pub mod rpc;
pub mod state;
pub mod units;

pub use adapter::{
    ChainAdapter, ChainDescriptor, ChainUpdate, NodeChainAdapter, NodeConfig, SyntheticChain,
    SyntheticChainConfig,
};
pub use amm::{FeeBps, FeeSide, Pool, PoolCurve, PoolId, PoolInvariant, SwapQuote};
pub use block::{
    Address, Block, BlockHash, BlockNumber, ChainId, Hash32, Trace, TraceKind, Transaction, TxHash,
    TxStatus,
};
pub use bridge::{
    BridgeExposure, BridgeFailure, BridgeLedger, BridgeRoute, BridgeTransfer, TransferId,
    TransferStatus,
};
pub use finality::{Confirmations, Finality};
pub use gas::{GasCost, GasProfile, effective_gas_price};
pub use mempool::{LikelyOrdering, Mempool, OrderedPending, PendingTransaction, ReorderingRisk};
pub use rpc::{JsonRpcChainAdapter, RpcChainConfig, RpcStats, TokenBinding};
pub use state::{Applied, ChainState, ConfirmedView, DerivedState, PoolState, Reorg};
pub use units::{Conversion, Quantised, TokenAmount};
