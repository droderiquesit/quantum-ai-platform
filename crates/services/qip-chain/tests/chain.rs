//! Blocks, reorganisations, finality and the adapters.
//!
//! The properties asserted here are the ones that distinguish a chain from an
//! exchange: state derived from a withdrawn block goes with it, a rollback
//! followed by a replay lands exactly where a direct application would, and a
//! reverted transaction is never a trade however much it looks like one.

use qip_chain::adapter::{
    ChainAdapter, ChainUpdate, NodeChainAdapter, NodeConfig, SyntheticChain, SyntheticChainConfig,
};
use qip_chain::amm::{FeeBps, PoolCurve, PoolId};
use qip_chain::block::{
    Address, Block, BlockHash, BlockNumber, ChainId, Hash32, Trace, TraceKind, Transaction, TxHash,
    TxStatus,
};
use qip_chain::finality::{Confirmations, Finality};
use qip_chain::state::{Applied, ChainState, DerivedState};
use qip_contracts::{BookSide, MessageBody, VenueId};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};

fn chain() -> ChainId {
    ChainId::new("test-chain")
}

fn venue() -> VenueId {
    VenueId::new("TEST-DEX")
}

fn block_hash(label: &str, number: u64) -> BlockHash {
    BlockHash::new(Hash32::of(&[label.as_bytes(), &number.to_le_bytes()]))
}

fn tx_hash(label: &str, index: u64) -> TxHash {
    TxHash::new(Hash32::of(&[label.as_bytes(), &index.to_le_bytes()]))
}

fn pool_id(name: &str) -> PoolId {
    PoolId::new(name)
}

fn creation(index: u32, pool: &PoolId, base: Decimal, quote: Decimal) -> Transaction {
    Transaction {
        hash: tx_hash(&format!("create-{pool}"), u64::from(index)),
        index,
        from: Address::new("0xdeployer"),
        to: None,
        status: TxStatus::Succeeded,
        gas_used: 1_000_000,
        effective_gas_price: Decimal::from_raw(20),
        traces: vec![Trace::new(
            0,
            TraceKind::PoolCreated {
                pool: pool.clone(),
                venue: venue(),
                base: ObjectId::from_string(format!("{pool}-base")),
                quote: ObjectId::from_string(format!("{pool}-quote")),
                curve: PoolCurve::ConstantProduct,
                fee: FeeBps::new(30).expect("30bp is a valid fee"),
                reserve_base: base,
                reserve_quote: quote,
            },
        )],
    }
}

fn swap(
    label: &str,
    index: u32,
    pool: &PoolId,
    base: Decimal,
    quote: Decimal,
    status: TxStatus,
) -> Transaction {
    Transaction {
        hash: tx_hash(label, u64::from(index)),
        index,
        from: Address::new("0xtrader"),
        to: Some(Address::new(pool.as_str())),
        status,
        gas_used: 140_000,
        effective_gas_price: Decimal::from_raw(25),
        traces: vec![Trace::new(
            0,
            TraceKind::Swap {
                pool: pool.clone(),
                object_id: ObjectId::from_string(format!("{pool}-base")),
                taker: BookSide::Ask,
                base_amount: base,
                quote_amount: quote,
            },
        )],
    }
}

fn block(label: &str, number: u64, parent: BlockHash, transactions: Vec<Transaction>) -> Block {
    let gas_used = transactions.iter().map(|tx| tx.gas_used).sum();
    Block {
        chain: chain(),
        number: BlockNumber::new(number),
        hash: block_hash(label, number),
        parent_hash: parent,
        timestamp: Timestamp::from_secs(1_700_000_000 + (number as i64) * 12),
        base_fee: Decimal::from_raw(20),
        gas_used,
        gas_limit: 30_000_000,
        transactions,
    }
}

/// Two pools, one of which is only ever touched below the fork point.
fn state_with_two_pools() -> (ChainState, PoolId, PoolId, BlockHash) {
    let mut state = ChainState::new(chain(), 512);
    let quiet = pool_id("quiet");
    let busy = pool_id("busy");

    let genesis = block_hash("genesis", 0);
    let one = block(
        "a",
        1,
        genesis,
        vec![
            creation(0, &quiet, Decimal::from_int(1_000), Decimal::from_int(2_000_000)),
            creation(1, &busy, Decimal::from_int(500), Decimal::from_int(1_000_000)),
        ],
    );
    let one_hash = one.hash;
    state.apply(one).expect("the first block applies");

    let two = block(
        "a",
        2,
        one_hash,
        vec![swap(
            "quiet-swap",
            0,
            &quiet,
            Decimal::from_int(10),
            Decimal::from_int(20_500),
            TxStatus::Succeeded,
        )],
    );
    let two_hash = two.hash;
    state.apply(two).expect("the second block applies");

    (state, quiet, busy, two_hash)
}

#[test]
fn a_reorganisation_invalidates_the_state_derived_from_the_withdrawn_blocks_and_nothing_else() {
    let (mut state, quiet, busy, fork_point) = state_with_two_pools();

    let three = block(
        "a",
        3,
        fork_point,
        vec![swap(
            "busy-swap-a",
            0,
            &busy,
            Decimal::from_int(5),
            Decimal::from_int(10_200),
            TxStatus::Succeeded,
        )],
    );
    let three_hash = three.hash;
    state.apply(three).expect("the third block applies");
    let four = block(
        "a",
        4,
        three_hash,
        vec![swap(
            "busy-swap-b",
            0,
            &busy,
            Decimal::from_int(5),
            Decimal::from_int(10_300),
            TxStatus::Succeeded,
        )],
    );
    state.apply(four).expect("the fourth block applies");

    let before = state.view(Confirmations::AT_RISK).expect("head view");
    let quiet_before = before.state().pool(&quiet).cloned().expect("quiet pool");
    assert_eq!(
        before.state().pool(&busy).expect("busy pool").trades,
        2,
        "the busy pool should have both swaps before the reorg"
    );

    // A longer branch from the same fork point, touching only the busy pool.
    let three_prime = block(
        "b",
        3,
        fork_point,
        vec![swap(
            "busy-swap-c",
            0,
            &busy,
            Decimal::from_int(1),
            Decimal::from_int(2_050),
            TxStatus::Succeeded,
        )],
    );
    let three_prime_hash = three_prime.hash;
    assert!(
        matches!(
            state.apply(three_prime).expect("a fork block is recorded"),
            Applied::SideBranch { .. }
        ),
        "a fork that is not longer must not move the canonical chain"
    );
    let four_prime = block("b", 4, three_prime_hash, Vec::new());
    let four_prime_hash = four_prime.hash;
    state.apply(four_prime).expect("the branch grows");
    let five_prime = block("b", 5, four_prime_hash, Vec::new());
    let applied = state.apply(five_prime).expect("the branch overtakes");

    let Applied::Reorganised(reorg) = applied else {
        panic!("a longer branch must reorganise the chain, got {applied:?}");
    };
    assert_eq!(reorg.depth(), 2, "two blocks were withdrawn");
    assert_eq!(reorg.common_ancestor, BlockNumber::new(2));
    assert_eq!(
        reorg.invalidated_pools,
        vec![busy.clone()],
        "only the pool the withdrawn blocks touched is invalidated"
    );
    assert_eq!(reorg.invalidated_trades, 1, "two swaps went, one came back");

    let after = state.view(Confirmations::AT_RISK).expect("head view");
    assert_eq!(
        after.state().pool(&quiet).expect("quiet pool"),
        &quiet_before,
        "a pool untouched by the reorg must be bit-identical afterwards"
    );
    let busy_after = after.state().pool(&busy).expect("busy pool");
    assert_eq!(
        busy_after.trades, 1,
        "the busy pool keeps only the swap on the winning branch"
    );
}

#[test]
fn a_withdrawn_block_becomes_orphaned_rather_than_merely_stale() {
    let (mut state, _, busy, fork_point) = state_with_two_pools();
    let three = block(
        "a",
        3,
        fork_point,
        vec![swap(
            "busy-swap-a",
            0,
            &busy,
            Decimal::from_int(5),
            Decimal::from_int(10_200),
            TxStatus::Succeeded,
        )],
    );
    let orphan = three.hash;
    state.apply(three).expect("applies");
    assert!(
        state
            .finality_of(&orphan, Confirmations::AT_RISK)
            .is_actionable(),
        "the head is actionable to a caller that asked for no confirmations"
    );

    let three_prime = block("b", 3, fork_point, Vec::new());
    let three_prime_hash = three_prime.hash;
    state.apply(three_prime).expect("fork recorded");
    let four_prime = block("b", 4, three_prime_hash, Vec::new());
    state.apply(four_prime).expect("branch overtakes");

    assert!(
        matches!(
            state.finality_of(&orphan, Confirmations::AT_RISK),
            Finality::Orphaned { .. }
        ),
        "a block that lost a reorg is void, not old"
    );
}

#[test]
fn rolling_back_and_replaying_reaches_the_same_state_as_applying_the_canonical_chain_directly() {
    // The seed drives block production, swap sizes, reverts and the reorg
    // sequence itself, so a failure here reproduces exactly.
    for seed in [1_u64, 7, 19, 23, 101] {
        let start = Timestamp::from_secs(1_700_000_000);
        let config = SyntheticChainConfig {
            reorg_probability: 0.35,
            max_reorg_depth: 4,
            ..SyntheticChainConfig::demo(seed).expect("demo config")
        };
        let mut adapter = SyntheticChain::new(config, start).expect("synthetic chain");
        // The synthetic chain names its own chain; align the state with it.
        let mut arrival = ChainState::new(adapter.descriptor().chain, 4096);

        let updates = adapter
            .poll(start.saturating_add(Duration::from_mins(40)))
            .expect("the synthetic chain polls");
        let mut reorgs = 0;
        for update in &updates {
            if let ChainUpdate::Block(block) = update {
                match arrival
                    .apply((**block).clone())
                    .expect("every emitted block is applicable")
                {
                    Applied::Reorganised(_) => reorgs += 1,
                    Applied::Extended { .. }
                    | Applied::SideBranch { .. }
                    | Applied::Duplicate => {}
                }
            }
        }
        assert!(
            reorgs > 0,
            "seed {seed} produced no reorg, so the property is untested"
        );

        // Replaying only the surviving chain, in order, into a fresh state.
        let mut direct = ChainState::new(adapter.descriptor().chain, 4096);
        for block in arrival.canonical() {
            direct
                .apply(block.clone())
                .expect("the canonical chain applies to a fresh state");
        }

        let unwound = arrival.view(Confirmations::AT_RISK).expect("head view");
        let replayed = direct.view(Confirmations::AT_RISK).expect("head view");
        let unwound: &DerivedState = unwound.state();
        assert_eq!(
            unwound,
            replayed.state(),
            "seed {seed}: unwinding and replaying must land where a direct application lands"
        );
    }
}

#[test]
fn the_state_a_synthetic_chain_reports_matches_the_state_derived_from_its_blocks() {
    let start = Timestamp::from_secs(1_700_000_000);
    let config = SyntheticChainConfig {
        reorg_probability: 0.2,
        ..SyntheticChainConfig::demo(31).expect("demo config")
    };
    let mut adapter = SyntheticChain::new(config.clone(), start).expect("synthetic chain");
    let mut state = ChainState::new(config.chain.clone(), 4096);
    for update in adapter
        .poll(start.saturating_add(Duration::from_mins(30)))
        .expect("poll")
    {
        if let ChainUpdate::Block(block) = update {
            state.apply(*block).expect("applies");
        }
    }
    let view = state.view(Confirmations::AT_RISK).expect("head view");
    let derived = view.pool(&config.pool).expect("the pool exists");
    assert_eq!(
        derived.reserve_base(),
        adapter.pool().reserve_base(),
        "a consumer replaying the blocks must reach the producer's reserves"
    );
    assert_eq!(derived.reserve_quote(), adapter.pool().reserve_quote());
}

#[test]
fn a_reverted_transaction_is_never_counted_as_a_trade_but_still_costs_gas() {
    let mut state = ChainState::new(chain(), 64);
    let pool = pool_id("pool");
    let genesis = block_hash("genesis", 0);
    let one = block(
        "a",
        1,
        genesis,
        vec![creation(
            0,
            &pool,
            Decimal::from_int(1_000),
            Decimal::from_int(2_000_000),
        )],
    );
    let one_hash = one.hash;
    let creation_gas = one.gas_spent();
    state.apply(one).expect("applies");

    let reverted = swap(
        "reverted",
        0,
        &pool,
        Decimal::from_int(10),
        Decimal::from_int(20_500),
        TxStatus::Reverted {
            reason: "INSUFFICIENT_OUTPUT_AMOUNT".to_string(),
        },
    );
    assert!(!reverted.is_trade(), "a reverted swap is not a trade");
    assert!(
        reverted.effective_traces().is_empty(),
        "a reverted transaction has no effective traces"
    );
    assert!(
        reverted.gas_cost().is_positive(),
        "a revert is still paid for"
    );

    let two = block("a", 2, one_hash, vec![reverted]);
    assert!(two.trades().is_empty(), "the block contains no trade");
    let expected_gas = creation_gas + two.gas_spent();
    assert!(
        two.market_messages(&venue(), "chain", 0, Timestamp::from_secs(1_700_000_100))
            .is_empty(),
        "a reverted swap must not become a market message"
    );
    state.apply(two).expect("applies");

    let view = state.view(Confirmations::AT_RISK).expect("head view");
    assert_eq!(view.state().trades(), 0, "no swap was counted");
    let pool_state = view.state().pool(&pool).expect("the pool exists");
    assert_eq!(
        pool_state.reserve_base,
        Decimal::from_int(1_000),
        "a revert moves no reserves"
    );
    assert_eq!(
        view.state().gas_spent(),
        expected_gas,
        "the gas of a failed attempt is still spent"
    );
}

#[test]
fn a_successful_swap_becomes_a_market_message_carrying_the_aggressor() {
    let pool = pool_id("pool");
    let filled = swap(
        "filled",
        0,
        &pool,
        Decimal::from_int(10),
        Decimal::from_int(20_500),
        TxStatus::Succeeded,
    );
    let one = block("a", 1, block_hash("genesis", 0), vec![filled]);
    let messages = one.market_messages(&venue(), "chain", 7, Timestamp::from_secs(1_700_000_100));
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.origin.sequence, 7, "the caller owns the stream");
    let MessageBody::Trade {
        price,
        quantity,
        aggressor,
        ..
    } = &message.body
    else {
        panic!("a swap must decode as a trade");
    };
    assert_eq!(*price, Decimal::from_int(2_050));
    assert_eq!(*quantity, Decimal::from_int(10));
    assert_eq!(*aggressor, Some(BookSide::Ask));
    assert!(
        message.transit().as_nanos() >= 0,
        "capture cannot precede the block"
    );
}

#[test]
fn a_view_reflects_only_the_blocks_that_reached_the_depth_the_caller_required() {
    let (mut state, _, busy, fork_point) = state_with_two_pools();
    let three = block(
        "a",
        3,
        fork_point,
        vec![swap(
            "busy-swap",
            0,
            &busy,
            Decimal::from_int(5),
            Decimal::from_int(10_200),
            TxStatus::Succeeded,
        )],
    );
    let three_hash = three.hash;
    state.apply(three).expect("applies");
    let four = block("a", 4, three_hash, Vec::new());
    state.apply(four).expect("applies");

    let head = state.view(Confirmations::AT_RISK).expect("head view");
    assert_eq!(
        head.state().pool(&busy).expect("busy pool").trades,
        1,
        "the head sees the swap"
    );

    let settled = state.view(Confirmations::exactly(2)).expect("deep view");
    assert_eq!(
        settled.state().pool(&busy).expect("busy pool").trades,
        0,
        "two confirmations deep, the swap has not happened yet"
    );
    assert_eq!(settled.as_of(), Some(BlockNumber::new(2)));
    assert_eq!(settled.required(), Confirmations::exactly(2));

    assert!(
        matches!(
            state.finality_of(&three_hash, Confirmations::exactly(2)),
            Finality::Included { confirmations: 1, .. }
        ),
        "one confirmation does not satisfy a requirement of two"
    );
    assert!(
        state
            .finality_of(&three_hash, Confirmations::exactly(1))
            .is_actionable(),
        "one confirmation satisfies a requirement of one"
    );
}

#[test]
fn a_view_deeper_than_the_retained_history_is_refused_rather_than_guessed() {
    let (state, _, _, _) = state_with_two_pools();
    let error = state
        .view(Confirmations::exactly(50))
        .expect_err("a view deeper than the chain must fail");
    assert_eq!(error.code(), "invalid");
    assert!(
        error.message().contains("undo history"),
        "the error should say what is missing, got {error}"
    );
}

#[test]
fn a_synthetic_chain_replays_identically_from_the_same_seed() {
    let start = Timestamp::from_secs(1_700_000_000);
    let render = |seed: u64| {
        let mut adapter =
            SyntheticChain::new(SyntheticChainConfig::demo(seed).expect("config"), start)
                .expect("chain");
        adapter
            .poll(start.saturating_add(Duration::from_mins(20)))
            .expect("poll")
            .iter()
            .map(|update| match update {
                ChainUpdate::Block(block) => format!("block:{}:{}", block.number, block.hash),
                ChainUpdate::Pending(tx) => format!("pending:{}", tx.hash),
                ChainUpdate::Dropped(hash) => format!("dropped:{hash}"),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(render(5), render(5), "the same seed must replay identically");
    assert_ne!(
        render(5),
        render(6),
        "a different seed must produce a different chain"
    );
}

#[test]
fn the_node_adapter_names_the_endpoint_credential_and_methods_it_is_missing() {
    let config = NodeConfig::ethereum_like(chain(), venue());
    let mut adapter = NodeChainAdapter::new(config, false, false);
    assert!(!adapter.is_available());

    let error = adapter
        .poll(Timestamp::from_secs(1_700_000_000))
        .expect_err("an unavailable node must not return blocks");
    assert_eq!(error.code(), "unavailable");
    let message = error.message();
    for required in [
        "QIP_CHAIN_RPC_ENDPOINT",
        "QIP_CHAIN_RPC_CREDENTIAL",
        "eth_getBlockByNumber",
        "debug_traceBlockByHash",
        "txpool_content",
    ] {
        assert!(
            message.contains(required),
            "the requirement should name {required}, got: {message}"
        );
    }
}
