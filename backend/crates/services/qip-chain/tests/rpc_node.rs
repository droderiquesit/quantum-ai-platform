//! The JSON-RPC chain adapter, against a real socket.
//!
//! Every test here binds a listener on loopback and lets the adapter connect to
//! it. A mocked client would prove that a decoder was called; it would not
//! prove that a node which answers with half a body, with a pending proposal
//! where a block was asked for, or with a receipt that belongs to a different
//! transaction, produces a named error rather than a block the chain never had.
//!
//! The finality tests are the ones this file exists for. An unfinalised block
//! is a fact that can be reorganised away, and an adapter that reports one as
//! settled hands the rest of the platform history that may not have happened.

mod node;

use node::{Behaviour, NodeScript, TestNode, address_with_no_listener};
use qip_chain::adapter::{ChainAdapter, ChainUpdate};
use qip_chain::block::{Address, Block, BlockNumber, ChainId, TraceKind, TxStatus};
use qip_chain::finality::Confirmations;
use qip_chain::rpc::{JsonRpcChainAdapter, RpcChainConfig, TokenBinding};
use qip_contracts::VenueId;
use qip_core::error::Error;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_transport::ClientLimits;
use std::time::Duration as StdDuration;

/// The credential the fixtures use. A literal so a test can assert it never
/// reaches a URL.
const CREDENTIAL: &str = "Bearer node-project-3f19";
/// The chain head every script reports, unless a test says otherwise.
const HEAD: u64 = 100;
/// Head less the twelve confirmations the fixtures require.
const SETTLED: u64 = 88;
/// A token this deployment has bound. Twenty bytes, as an address is.
const TOKEN_CONTRACT: &str = "0xc0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0";
/// A contract nobody bound.
const UNBOUND_CONTRACT: &str = "0xdededededededededededededededededededede";
const SENDER: &str = "0x1111111111111111111111111111111111111111";
const RECIPIENT: &str = "0x2222222222222222222222222222222222222222";
const TX_ONE: &str = "0xc1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1";
const TX_TWO: &str = "0xc2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2";
const TX_PENDING: &str = "0xd1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1";
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

/// 15:00 on 24 August 2026, the instant every poll is given.
fn poll_instant() -> Timestamp {
    Timestamp::from_secs(1_787_583_600)
}

/// One minute before the poll, so a block stamped with it is knowable.
const BLOCK_STAMP: &str = "0x6a8c5c34";
/// Ten minutes after the poll, for the block a proposer stamped in the future.
const FUTURE_STAMP: &str = "0x6a8c5ec8";

fn object_id() -> ObjectId {
    ObjectId::from_string("OBJ0000000000000000USDC")
}

fn bindings() -> Vec<TokenBinding> {
    vec![TokenBinding::new(TOKEN_CONTRACT, object_id(), 6)]
}

/// A block's hash, derived from its height so a test can name it.
fn block_hash(number: u64) -> String {
    format!("0x{number:064x}")
}

/// A 32-byte log topic carrying a 20-byte address in its low bytes.
fn topic_for(address: &str) -> String {
    format!("0x{}{}", "0".repeat(24), address.trim_start_matches("0x"))
}

/// Limits tight enough that a test trips them in bytes and milliseconds.
fn tight() -> ClientLimits {
    ClientLimits {
        max_body: 8192,
        max_headers: 16,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(200),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

fn config(base: &str) -> RpcChainConfig {
    RpcChainConfig {
        name: "node-1".into(),
        chain: ChainId::new("test-chain"),
        venue: VenueId::new("TEST-DEX"),
        endpoint: Some(base.to_string()),
        path: "/".into(),
        credential: Some(CREDENTIAL.into()),
        credential_header: "authorization".into(),
        confirmations: Confirmations::exactly(12),
        start_block: None,
        max_blocks_per_poll: 4,
        max_transactions_per_block: 100,
        include_pending: false,
        block_time: Duration::from_secs(12),
        http: tight(),
    }
}

/// A block with two transactions: one that succeeded and one that reverted.
fn block_json(number: u64, timestamp: &str) -> String {
    format!(
        r#"{{
  "number": "0x{number:x}",
  "hash": "{hash}",
  "parentHash": "{parent}",
  "timestamp": "{timestamp}",
  "baseFeePerGas": "0x3b9aca00",
  "gasUsed": "0x5208",
  "gasLimit": "0x1c9c380",
  "transactions": [
    {{"hash":"{TX_ONE}","from":"{SENDER}","to":"{RECIPIENT}","nonce":"0x7","gas":"0x5208",
      "maxFeePerGas":"0x77359400","maxPriorityFeePerGas":"0x3b9aca00"}},
    {{"hash":"{TX_TWO}","from":"{RECIPIENT}","to":"{SENDER}","nonce":"0x9","gas":"0x30d40",
      "gasPrice":"0x4a817c800"}}
  ]
}}"#,
        hash = block_hash(number),
        parent = block_hash(number.saturating_sub(1)),
    )
}

/// Receipts matching [`block_json`]: a success carrying one transfer log, and a
/// revert carrying none.
fn receipts_json() -> String {
    format!(
        r#"[
  {{"transactionHash":"{TX_ONE}","transactionIndex":"0x0","status":"0x1","gasUsed":"0x5208",
    "effectiveGasPrice":"0x3b9aca00",
    "logs":[{{"address":"{TOKEN_CONTRACT}","topics":["{TRANSFER_TOPIC}","{from}","{to}"],
              "data":"0x00000000000000000000000000000000000000000000000000000000000f4240",
              "logIndex":"0x0","removed":false}}]}},
  {{"transactionHash":"{TX_TWO}","transactionIndex":"0x1","status":"0x0","gasUsed":"0x30d40",
    "effectiveGasPrice":"0x4a817c800","logs":[]}}
]"#,
        from = topic_for(SENDER),
        to = topic_for(RECIPIENT),
    )
}

/// A script that serves head, one settled block and its receipts.
fn settled_script() -> NodeScript {
    NodeScript::new()
        .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
        .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
        .answering("eth_getBlockReceipts", receipts_json())
}

fn adapter_for(node: &TestNode) -> JsonRpcChainAdapter {
    JsonRpcChainAdapter::new(config(&node.url()), bindings())
        .expect("a fully specified configuration builds")
}

fn blocks_of(updates: &[ChainUpdate]) -> Vec<&Block> {
    updates
        .iter()
        .filter_map(|u| match u {
            ChainUpdate::Block(block) => Some(&**block),
            _ => None,
        })
        .collect()
}

// --- what arrives -----------------------------------------------------------

#[test]
fn a_configured_adapter_reaches_a_node_over_a_real_socket_and_decodes_a_settled_block() {
    let server = TestNode::running(settled_script());
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        server.served() >= 3,
        "the premise of this test is that requests crossed a socket; {} did",
        server.served()
    );
    assert_eq!(
        server.methods_called(),
        vec![
            "eth_blockNumber".to_string(),
            "eth_getBlockByNumber".to_string(),
            "eth_getBlockReceipts".to_string(),
        ],
        "one head call, then one block and its receipts"
    );

    let blocks = blocks_of(&updates);
    assert_eq!(blocks.len(), 1, "one settled block: {updates:?}");
    let block = blocks[0];
    assert_eq!(block.number, BlockNumber::new(SETTLED));
    assert_eq!(block.chain, ChainId::new("test-chain"));
    assert_eq!(block.gas_used, 21_000);
    assert_eq!(block.gas_limit, 30_000_000);
    assert_eq!(
        block.base_fee,
        Decimal::from_raw(1),
        "one gwei of base fee is exactly one raw unit of the platform's Decimal"
    );
    assert_eq!(block.transactions.len(), 2);
    assert_eq!(adapter.stats().blocks, 1);
    assert_eq!(adapter.stats().transactions, 2);
    assert_eq!(adapter.stats().withheld, 0);
    assert!(block.validate().is_ok(), "the decoded block is coherent");
}

#[test]
fn a_succeeded_transfer_becomes_a_trace_on_the_instrument_the_deployment_bound() {
    let server = TestNode::running(settled_script());
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");
    let block = blocks_of(&updates)[0];
    let traces = block.transactions[0].effective_traces();

    assert_eq!(traces.len(), 1, "one transfer log, one trace");
    match &traces[0].kind {
        TraceKind::Transfer {
            object_id: id,
            from,
            to,
            amount,
        } => {
            assert_eq!(
                *id,
                object_id(),
                "the instrument comes from the binding, never from the contract address"
            );
            assert_eq!(*from, Address::new(SENDER));
            assert_eq!(*to, Address::new(RECIPIENT));
            assert_eq!(amount.raw(), 1_000_000, "the raw amount is exact");
            assert_eq!(amount.decimals(), 6, "the decimals come from the binding");
        }
        other => panic!("expected a transfer trace, got {other:?}"),
    }
}

#[test]
fn a_reverted_transaction_is_decoded_as_reverted_and_yields_no_effective_traces() {
    let server = TestNode::running(settled_script());
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");
    let block = blocks_of(&updates)[0];
    let reverted = &block.transactions[1];

    assert!(
        matches!(reverted.status, TxStatus::Reverted { .. }),
        "a receipt status of 0x0 is a revert: {:?}",
        reverted.status
    );
    assert!(
        reverted.effective_traces().is_empty(),
        "a reverted transaction moved nothing, whatever it looks like"
    );
    assert!(!reverted.is_trade(), "and it is not a fill");
    assert_eq!(
        reverted.effective_gas_price,
        Decimal::from_raw(20),
        "twenty gwei, charged whether or not the transaction achieved anything"
    );
    assert!(
        reverted.gas_cost().is_positive(),
        "gas is a cost of trying, not a cost of trading"
    );
    assert_eq!(adapter.stats().reverts, 1);
    assert_eq!(
        block.trades().len(),
        0,
        "the transfer is not a swap, so this block reports no trades at all"
    );
}

// --- finality ---------------------------------------------------------------

#[test]
fn a_block_shallower_than_the_required_depth_is_withheld_and_never_fetched() {
    let server = TestNode::running(settled_script());
    let mut settings = config(&server.url());
    // Ninety-five is real, it is the chain's history, and it is seven blocks
    // from the head. This deployment has said it will not act on anything less
    // than twelve deep.
    settings.start_block = Some(95);
    let mut adapter =
        JsonRpcChainAdapter::new(settings, bindings()).expect("the configuration builds");

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        blocks_of(&updates).is_empty(),
        "an unfinalised block must not enter the platform as settled: {updates:?}"
    );
    assert_eq!(adapter.stats().withheld, 1);
    assert_eq!(
        server.methods_called(),
        vec!["eth_blockNumber".to_string()],
        "the adapter must not even ask for a block it would refuse to report"
    );
}

#[test]
fn the_head_is_reported_only_when_a_deployment_asks_for_it_at_risk() {
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(HEAD, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts_json()),
    );
    let mut settings = config(&server.url());
    settings.confirmations = Confirmations::AT_RISK;
    let mut adapter =
        JsonRpcChainAdapter::new(settings, bindings()).expect("the configuration builds");

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert_eq!(
        blocks_of(&updates)[0].number,
        BlockNumber::new(HEAD),
        "at zero confirmations the head is what there is"
    );
    let requirement = adapter
        .descriptor()
        .production_requirement
        .expect("the descriptor carries the requirement");
    assert!(
        requirement.contains("reorganised away"),
        "a feed reading the head has to say what that means: {requirement}"
    );
    assert!(
        requirement.contains("moves custody"),
        "and where it may not be used: {requirement}"
    );
}

#[test]
fn a_chain_younger_than_the_required_depth_yields_nothing_rather_than_underflowing() {
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", "\"0x5\"")
            .answering("eth_getBlockByNumber", block_json(5, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts_json()),
    );
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        updates.is_empty(),
        "a five-block chain has nothing twelve deep in it: {updates:?}"
    );
    assert_eq!(
        adapter.stats().withheld,
        1,
        "and that is a state to report, not an error and not a height near u64::MAX"
    );
    assert_eq!(server.methods_called(), vec!["eth_blockNumber".to_string()]);
}

#[test]
fn a_block_stamped_after_the_callers_clock_is_withheld_and_the_cursor_does_not_pass_it() {
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, FUTURE_STAMP))
            .answering("eth_getBlockReceipts", receipts_json()),
    );
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        blocks_of(&updates).is_empty(),
        "the caller owns the clock; a block stamped after it is not knowable yet"
    );
    assert_eq!(adapter.stats().withheld, 1);
    assert_eq!(
        adapter.cursor(),
        Some(BlockNumber::new(SETTLED)),
        "the cursor stays on the withheld height so the next poll asks again"
    );
}

#[test]
fn the_cursor_advances_so_a_second_poll_does_not_report_the_same_block_twice() {
    let server = TestNode::running(settled_script());
    let mut adapter = adapter_for(&server);

    let first = adapter
        .poll(poll_instant())
        .expect("the first poll succeeds");
    let second = adapter
        .poll(poll_instant())
        .expect("the second poll succeeds");

    assert_eq!(blocks_of(&first).len(), 1);
    assert!(
        blocks_of(&second).is_empty(),
        "the chain has not moved, so there is nothing new: {second:?}"
    );
    assert_eq!(adapter.cursor(), Some(BlockNumber::new(SETTLED + 1)));
    assert_eq!(adapter.stats().blocks, 1);
}

#[test]
fn a_pending_proposal_with_no_hash_is_refused_rather_than_entering_as_a_block() {
    // What `eth_getBlockByNumber` answers the `pending` tag with: a null hash
    // and a null number. Nothing was ever built on it.
    let proposal = format!(
        r#"{{"number":null,"hash":null,"parentHash":"{parent}","timestamp":"{BLOCK_STAMP}",
            "gasUsed":"0x0","gasLimit":"0x1c9c380","transactions":[]}}"#,
        parent = block_hash(SETTLED - 1),
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", proposal)
            .answering("eth_getBlockReceipts", "[]"),
    );
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a pending proposal was accepted as a block");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(
        error.message().contains("pending proposal"),
        "the refusal must say what it refused: {error}"
    );
    assert_eq!(adapter.stats().blocks, 0);
}

#[test]
fn pending_transactions_reach_the_platform_as_pending_and_never_as_a_block() {
    let pending = format!(
        r#"{{"number":null,"hash":null,"parentHash":"{parent}","timestamp":"{BLOCK_STAMP}",
            "gasUsed":"0x0","gasLimit":"0x1c9c380","transactions":[
              {{"hash":"{TX_PENDING}","from":"{SENDER}","to":"{RECIPIENT}","nonce":"0xa",
                "gas":"0x5208","maxFeePerGas":"0x77359400","maxPriorityFeePerGas":"0x3b9aca00"}},
              {{"hash":"{TX_TWO}","from":"{RECIPIENT}","to":"{SENDER}","nonce":"0xb",
                "gas":"0x5208","gasPrice":"0x4a817c800"}}
            ]}}"#,
        parent = block_hash(SETTLED),
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
            .answering("eth_getBlockByNumber", pending)
            .answering("eth_getBlockReceipts", receipts_json()),
    );
    let mut settings = config(&server.url());
    settings.include_pending = true;
    let mut adapter =
        JsonRpcChainAdapter::new(settings, bindings()).expect("the configuration builds");

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");

    let pending: Vec<_> = updates
        .iter()
        .filter_map(|u| match u {
            ChainUpdate::Pending(tx) => Some(&**tx),
            _ => None,
        })
        .collect();
    assert_eq!(
        pending.len(),
        1,
        "the 1559 transaction is pending; the legacy one prices itself with one number and is \
         skipped rather than given an invented priority fee"
    );
    assert_eq!(pending[0].nonce, 10);
    assert_eq!(pending[0].max_fee_per_gas, Decimal::from_raw(2));
    assert_eq!(pending[0].max_priority_fee_per_gas, Decimal::from_raw(1));
    assert_eq!(
        pending[0].first_seen,
        poll_instant(),
        "the earliest instant this deployment could have known about it, which is all `first \
         seen` can mean when the pool is polled rather than subscribed to"
    );
    assert!(
        pending[0].intent.is_none(),
        "calldata is not decoded, and an intent guessed from an opaque blob would drive an \
         ordering prediction"
    );
    assert_eq!(
        blocks_of(&updates).len(),
        1,
        "the pending set produced no block of its own"
    );
}

// --- a node whose two answers disagree --------------------------------------

#[test]
fn a_receipt_with_no_status_is_refused_rather_than_read_as_a_success() {
    let receipts = format!(
        r#"[{{"transactionHash":"{TX_ONE}","gasUsed":"0x5208","effectiveGasPrice":"0x3b9aca00",
             "logs":[]}},
           {{"transactionHash":"{TX_TWO}","status":"0x1","gasUsed":"0x30d40",
             "effectiveGasPrice":"0x3b9aca00","logs":[]}}]"#
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts),
    );
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a receipt with no status was read as a success");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("assumed successful"),
        "the refusal must say what it will not assume: {error}"
    );
}

#[test]
fn a_receipt_belonging_to_another_transaction_is_refused_as_two_answers_stitched_together() {
    let receipts = format!(
        r#"[{{"transactionHash":"{TX_TWO}","status":"0x1","gasUsed":"0x5208",
             "effectiveGasPrice":"0x3b9aca00","logs":[]}},
           {{"transactionHash":"{TX_ONE}","status":"0x1","gasUsed":"0x30d40",
             "effectiveGasPrice":"0x3b9aca00","logs":[]}}]"#
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts),
    );
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a receipt for the wrong transaction was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("revert counted as a fill"),
        "the refusal must say what the mismatch would have cost: {error}"
    );
}

#[test]
fn a_receipt_list_that_does_not_cover_the_block_is_refused() {
    let receipts = format!(
        r#"[{{"transactionHash":"{TX_ONE}","status":"0x1","gasUsed":"0x5208",
             "effectiveGasPrice":"0x3b9aca00","logs":[]}}]"#
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts),
    );
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("two transactions and one receipt were accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("2 transactions and 1 receipts"),
        "{error}"
    );
}

#[test]
fn a_node_that_answers_one_height_with_another_block_is_refused() {
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            // Asked for 88, answers with 87.
            .answering("eth_getBlockByNumber", block_json(SETTLED - 1, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts_json()),
    );
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a block from a different height was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(error.message().contains("asked for block"), "{error}");
}

#[test]
fn a_log_the_node_has_retracted_never_becomes_a_trace() {
    let receipts = format!(
        r#"[{{"transactionHash":"{TX_ONE}","status":"0x1","gasUsed":"0x5208",
             "effectiveGasPrice":"0x3b9aca00",
             "logs":[{{"address":"{TOKEN_CONTRACT}","topics":["{TRANSFER_TOPIC}","{from}","{to}"],
                       "data":"0x00000000000000000000000000000000000000000000000000000000000f4240",
                       "logIndex":"0x0","removed":true}}]}},
           {{"transactionHash":"{TX_TWO}","status":"0x0","gasUsed":"0x30d40",
             "effectiveGasPrice":"0x4a817c800","logs":[]}}]"#,
        from = topic_for(SENDER),
        to = topic_for(RECIPIENT),
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts),
    );
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");
    let block = blocks_of(&updates)[0];

    assert!(
        block.transactions[0].effective_traces().is_empty(),
        "a log the node has already retracted belongs to a block being reorganised out"
    );
    assert_eq!(
        adapter.stats().retracted_logs,
        1,
        "and a non-zero count is the signal that this depth is not settled after all"
    );
}

#[test]
fn a_transfer_from_an_unbound_contract_is_counted_rather_than_given_an_invented_object_id() {
    let receipts = format!(
        r#"[{{"transactionHash":"{TX_ONE}","status":"0x1","gasUsed":"0x5208",
             "effectiveGasPrice":"0x3b9aca00",
             "logs":[{{"address":"{UNBOUND_CONTRACT}","topics":["{TRANSFER_TOPIC}","{from}","{to}"],
                       "data":"0x00000000000000000000000000000000000000000000000000000000000f4240",
                       "logIndex":"0x0","removed":false}}]}},
           {{"transactionHash":"{TX_TWO}","status":"0x0","gasUsed":"0x30d40",
             "effectiveGasPrice":"0x4a817c800","logs":[]}}]"#,
        from = topic_for(SENDER),
        to = topic_for(RECIPIENT),
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts),
    );
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");

    assert!(
        blocks_of(&updates)[0].transactions[0]
            .effective_traces()
            .is_empty(),
        "an ObjectId invented from a contract address merges two assets"
    );
    assert_eq!(
        adapter.stats().unmapped_transfers,
        1,
        "the symptom of a missing binding is silence, so it is counted"
    );
}

// --- what a node may not say ------------------------------------------------

#[test]
fn a_json_rpc_error_inside_a_200_is_a_named_refusal_and_not_a_success() {
    let server = TestNode::running(NodeScript::new().failing(
        "eth_blockNumber",
        r#"{"code":-32000,"message":"exceeded the request quota"}"#,
    ));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a JSON-RPC error was read as a head");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(
        error.message().contains("-32000") && error.message().contains("request quota"),
        "the refusal must carry the node's own reason: {error}"
    );
}

#[test]
fn a_null_result_is_a_height_this_node_does_not_have_rather_than_an_empty_block() {
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", "null"),
    );
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a null block was accepted");

    assert_eq!(error.code(), "not_found", "got {error:?}");
    assert!(
        error.message().contains("pruned or lagging"),
        "the refusal must name what a null result actually is: {error}"
    );
}

#[test]
fn a_quantity_that_is_not_hex_is_refused_with_the_field_it_was_read_from() {
    let server = TestNode::running(NodeScript::new().answering("eth_blockNumber", "\"12345\""));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a decimal quantity was accepted as hex");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("the chain head"),
        "the refusal must name what was being read: {error}"
    );
}

#[test]
fn a_short_block_hash_is_refused_because_a_truncated_identity_merges_two_blocks() {
    let block = format!(
        r#"{{"number":"0x{SETTLED:x}","hash":"0xabcdef","parentHash":"{parent}",
            "timestamp":"{BLOCK_STAMP}","gasUsed":"0x0","gasLimit":"0x1c9c380",
            "transactions":[]}}"#,
        parent = block_hash(SETTLED - 1),
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block)
            .answering("eth_getBlockReceipts", "[]"),
    );
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a six-digit block hash was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(error.message().contains("32-byte digest"), "{error}");
}

#[test]
fn more_transactions_than_the_cap_are_refused_before_the_block_is_expanded() {
    let server = TestNode::running(settled_script());
    let mut settings = config(&server.url());
    settings.max_transactions_per_block = 1;
    let mut adapter =
        JsonRpcChainAdapter::new(settings, bindings()).expect("the configuration builds");

    let error = adapter
        .poll(poll_instant())
        .expect_err("a two-transaction block passed a cap of one");

    assert_eq!(error.code(), "guard", "got {error:?}");
    assert!(error.message().contains("the cap is 1"), "{error}");
}

#[test]
fn a_gas_price_below_one_gwei_is_rounded_away_from_zero_so_a_cost_is_never_understated() {
    let receipts = format!(
        r#"[{{"transactionHash":"{TX_ONE}","status":"0x1","gasUsed":"0x5208",
             "effectiveGasPrice":"0x1","logs":[]}},
           {{"transactionHash":"{TX_TWO}","status":"0x1","gasUsed":"0x30d40",
             "effectiveGasPrice":"0x3b9aca00","logs":[]}}]"#
    );
    let server = TestNode::running(
        NodeScript::new()
            .answering("eth_blockNumber", format!("\"0x{HEAD:x}\""))
            .answering("eth_getBlockByNumber", block_json(SETTLED, BLOCK_STAMP))
            .answering("eth_getBlockReceipts", receipts),
    );
    let mut adapter = adapter_for(&server);

    let updates = adapter.poll(poll_instant()).expect("the poll succeeds");
    let block = blocks_of(&updates)[0];

    assert_eq!(
        block.transactions[0].effective_gas_price,
        Decimal::from_raw(1),
        "one wei cannot be represented in nine fractional digits of a native unit, and rounding \
         it down to nothing would make a transaction look free"
    );
    assert_eq!(
        block.transactions[1].effective_gas_price,
        Decimal::from_raw(1),
        "a whole gwei converts exactly"
    );
}

// --- a peer having a bad day ------------------------------------------------

#[test]
fn a_body_larger_than_the_limit_is_refused_before_it_is_buffered() {
    let server =
        TestNode::running(NodeScript::new().behaving(Behaviour::Oversized { bytes: 64 * 1024 }));
    let mut adapter = adapter_for(&server);

    let error: Error = adapter
        .poll(poll_instant())
        .expect_err("a 64 kB body was accepted against an 8 kB limit");

    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
    assert!(error.message().contains("8192"), "{error}");
}

#[test]
fn a_node_that_dies_part_way_through_its_own_body_is_a_close_and_not_a_short_block() {
    let server = TestNode::running(NodeScript::new().behaving(Behaviour::Truncated {
        declared: 4096,
        written: 64,
    }));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("half a body was accepted as an answer");

    assert_eq!(
        error.code(),
        "io",
        "a node that stopped mid-body is a connection that failed, not a schema that is wrong: \
         {error:?}"
    );
    assert_eq!(adapter.stats().blocks, 0);
}

#[test]
fn a_node_that_accepts_the_connection_and_says_nothing_is_refused_within_the_timeout() {
    let server =
        TestNode::running(NodeScript::new().behaving(Behaviour::Silent(StdDuration::from_secs(3))));
    let mut adapter = adapter_for(&server);

    let started = std::time::Instant::now();
    let error = adapter
        .poll(poll_instant())
        .expect_err("a silent node was waited on indefinitely");

    assert_eq!(error.code(), "timeout", "got {error:?}");
    assert!(
        started.elapsed() < StdDuration::from_secs(2),
        "the wait must be bounded by the configured read timeout, not by the node"
    );
}

#[test]
fn an_unreachable_node_is_refused_rather_than_waited_on() {
    let mut settings = config(&address_with_no_listener());
    settings.http.connect_timeout = StdDuration::from_millis(500);
    let mut adapter =
        JsonRpcChainAdapter::new(settings, bindings()).expect("the configuration builds");

    let started = std::time::Instant::now();
    let error = adapter
        .poll(poll_instant())
        .expect_err("a poll against an address with no listener produced blocks");

    assert!(
        matches!(error, Error::Io(_) | Error::Timeout(_)),
        "an address nothing answers on is a connection failure: {error:?}"
    );
    assert!(started.elapsed() < StdDuration::from_secs(2));
}

#[test]
fn a_node_that_rejects_the_credential_produces_a_denial_that_does_not_quote_it() {
    let server = TestNode::running(NodeScript::new().behaving(Behaviour::Body {
        status: 401,
        body: "{\"message\":\"unauthorized\"}".into(),
    }));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unauthorised response was accepted");

    assert_eq!(error.code(), "denied", "got {error:?}");
    assert!(
        !error.message().contains(CREDENTIAL),
        "a refusal must not put the credential in a log line: {error}"
    );
}

// --- the request ------------------------------------------------------------

#[test]
fn the_call_is_a_posted_json_rpc_envelope_with_the_credential_in_a_header() {
    let server = TestNode::running(settled_script());
    let mut adapter = adapter_for(&server);

    adapter.poll(poll_instant()).expect("the poll succeeds");

    let requests = server.requests();
    let first = requests.first().expect("at least one request was made");
    assert_eq!(first.method, "POST", "a JSON-RPC call is a POST body");
    assert_eq!(first.target, "/", "and carries no query at all");
    assert!(
        !first.target.contains("node-project"),
        "a URL is written to every access log on the path: {}",
        first.target
    );
    assert_eq!(
        first.headers.get("authorization").map(String::as_str),
        Some(CREDENTIAL),
        "the credential must reach the node"
    );
    assert_eq!(
        first.headers.get("content-type").map(String::as_str),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(
        first.rpc_id(),
        Some(1),
        "every call is addressed, so an answer can be matched to it"
    );
    assert_eq!(
        requests.get(1).and_then(node::RawRequest::rpc_id),
        Some(2),
        "and the ids advance rather than repeating"
    );

    let redacted = format!("{:?}", adapter.config());
    assert!(
        !redacted.contains(CREDENTIAL),
        "the config's Debug must not print the credential: {redacted}"
    );
    assert!(redacted.contains("<redacted>"), "{redacted}");
}

// --- an adapter with nothing behind it --------------------------------------

#[test]
fn an_unconfigured_adapter_names_what_is_missing_and_opens_no_connection() {
    let server = TestNode::running(settled_script());
    let mut adapter = JsonRpcChainAdapter::new(RpcChainConfig::default(), Vec::new())
        .expect("an adapter that cannot fetch still has to exist in order to say so");

    assert!(!adapter.is_available());
    let missing = adapter.missing_configuration();
    assert_eq!(missing.len(), 2, "two things are missing: {missing:?}");
    let joined = missing.join(" | ");
    assert!(joined.contains("no endpoint"), "{joined}");
    assert!(joined.contains("no credential"), "{joined}");

    let requirement = adapter
        .descriptor()
        .production_requirement
        .expect("the descriptor must carry the requirement");
    assert!(
        requirement.contains("eth_getBlockReceipts"),
        "{requirement}"
    );
    assert!(requirement.contains("no token bindings"), "{requirement}");

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unconfigured adapter produced blocks");
    assert_eq!(error.code(), "unavailable", "got {error:?}");
    assert!(
        error
            .message()
            .contains("will not substitute generated blocks"),
        "the refusal must say that no data is deliberate: {error}"
    );

    let error = adapter
        .start(poll_instant())
        .expect_err("an unconfigured adapter started");
    assert_eq!(error.code(), "unavailable", "got {error:?}");
    assert_eq!(
        server.served(),
        0,
        "an unconfigured adapter must not open a connection at all"
    );
}

#[test]
fn an_endpoint_without_a_credential_is_still_unavailable_and_still_opens_nothing() {
    let server = TestNode::running(settled_script());
    let mut settings = config(&server.url());
    settings.credential = None;
    let mut adapter =
        JsonRpcChainAdapter::new(settings, bindings()).expect("the configuration builds");

    let error = adapter
        .poll(poll_instant())
        .expect_err("a credential-less poll was attempted");

    assert_eq!(error.code(), "unavailable", "got {error:?}");
    assert_eq!(
        server.served(),
        0,
        "an adapter with no credential must not open a connection at all"
    );
}

#[test]
fn a_configured_adapter_still_states_what_production_has_to_add() {
    let server = TestNode::running(settled_script());
    let adapter = adapter_for(&server);

    assert!(adapter.is_available(), "the premise: nothing is missing");
    let requirement = adapter
        .descriptor()
        .production_requirement
        .expect("a configured adapter is not by itself a production feed");
    assert!(requirement.contains("TLS"), "{requirement}");
    assert!(
        requirement.contains("confirmation depth"),
        "the depth is the decision this adapter most needs a human to make: {requirement}"
    );
    assert!(
        !adapter.descriptor().is_synthetic,
        "this one is not a stand-in; it is a node that has to be reachable"
    );
}

#[test]
fn an_https_endpoint_is_refused_at_configuration_time_rather_than_downgraded() {
    let mut settings = config("http://127.0.0.1:1");
    settings.endpoint = Some("https://node.example.com".into());

    let error = JsonRpcChainAdapter::new(settings, bindings())
        .expect_err("an https endpoint was accepted by a client with no TLS stack");

    assert_eq!(error.code(), "invalid", "got {error:?}");
}

#[test]
fn a_credential_header_the_transport_writes_itself_is_refused_at_configuration_time() {
    let mut settings = config("http://127.0.0.1:1");
    settings.credential_header = "Host".into();

    let error = JsonRpcChainAdapter::new(settings, bindings())
        .expect_err("a credential in a client-owned header was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(
        error.message().contains("without a credential"),
        "the refusal must say what would have happened: {error}"
    );
}

#[test]
fn two_bindings_claiming_one_contract_are_refused() {
    let error = JsonRpcChainAdapter::new(
        config("http://127.0.0.1:1"),
        vec![
            TokenBinding::new(TOKEN_CONTRACT, object_id(), 6),
            TokenBinding::new(
                TOKEN_CONTRACT.to_ascii_uppercase().replace("0X", "0x"),
                ObjectId::from_string("OBJ0000000000000000USDT"),
                6,
            ),
        ],
    )
    .expect_err("an ambiguous contract binding was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
}
