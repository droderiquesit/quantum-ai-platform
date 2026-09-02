//! A reorganisation reaches the bridge transfers whose deposits it withdrew.
//!
//! `BridgeLedger::on_reorg` fails every open transfer whose source block a
//! reorganisation reverted, and until this seam existed nothing called it:
//! the kernel held no bridge ledger, so a transfer waiting for finality on a
//! block that stopped existing kept waiting, and the value it was supposed
//! to move stayed on the books as in flight.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_chain::{
    Block, BlockHash, BlockNumber, BridgeFailure, BridgeRoute, BridgeTransfer, ChainId,
    ChainUpdate, Confirmations, FeeBps, Hash32, TransferId, TransferStatus,
};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};

// --- fixtures ---------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                ObjectId::from_string("obj-AAA"),
                "AAA",
                InstrumentType::CommonStock,
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default().with_chain_confirmations(1);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn chain() -> ChainId {
    ChainId::new("sim-source")
}

fn hash(label: &str) -> BlockHash {
    BlockHash::new(Hash32::of(&[label.as_bytes()]))
}

/// An empty block on `chain` at `number`, hashed from its label so two
/// branches at the same height have different identities.
fn block(chain: &ChainId, number: u64, label: &str, parent: &str) -> ChainUpdate {
    ChainUpdate::Block(Box::new(Block {
        chain: chain.clone(),
        number: BlockNumber::new(number),
        hash: hash(label),
        parent_hash: hash(parent),
        timestamp: start().saturating_add(Duration::from_secs(12 * number as i64)),
        base_fee: Decimal::ZERO,
        gas_used: 0,
        gas_limit: 30_000_000,
        transactions: Vec::new(),
    }))
}

fn transfer(source_block: u64, source_label: &str) -> Result<BridgeTransfer> {
    let route = BridgeRoute::new(
        "sim-bridge",
        chain(),
        ChainId::new("sim-destination"),
        Duration::from_mins(5),
        Duration::from_hours(1),
        Confirmations::exactly(3),
        FeeBps::new(10)?,
        vec![BridgeFailure::SourceReorg],
    )?;
    BridgeTransfer::open(
        TransferId::new("xfer-1"),
        route,
        ObjectId::from_string("obj-AAA"),
        dec!("1000"),
        start(),
        BlockNumber::new(source_block),
        hash(source_label),
    )
}

// --- the seam ---------------------------------------------------------------

#[test]
fn a_reorganisation_that_withdraws_a_deposit_block_fails_the_transfer_riding_on_it() -> Result<()> {
    let mut platform = platform()?;
    let chain = chain();

    // The canonical chain: genesis, then block 2a, which is where the
    // transfer's deposit lands.
    let absorbed = platform.observe_chain(vec![
        block(&chain, 1, "b1", "genesis"),
        block(&chain, 2, "b2a", "b1"),
    ]);
    assert_eq!(absorbed.extended, 2, "{absorbed:?}");
    assert_eq!(absorbed.reorgs, 0);

    platform.open_bridge_transfer(transfer(2, "b2a")?)?;

    // Premise: one transfer, open, and nothing has failed anything.
    assert_eq!(platform.bridges().len(), 1);
    let before = platform
        .bridges()
        .get(&TransferId::new("xfer-1"))
        .expect("the transfer was opened");
    assert!(
        before.status().is_open(),
        "a fresh transfer is open: {:?}",
        before.status()
    );
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(names::BRIDGE_TRANSFERS_FAILED),
        0
    );

    // A block the chain state refuses — it belongs to another chain — fails
    // nothing: the deposit block is still canonical, and a refusal is not a
    // reorganisation.
    let refused = platform.observe_chain(vec![block(
        &ChainId::new("some-other-chain"),
        3,
        "foreign",
        "b2a",
    )]);
    assert_eq!(refused.problems.len(), 1, "{refused:?}");
    assert_eq!(refused.reorgs, 0);
    assert_eq!(refused.bridged_transfers_failed, 0);
    assert!(
        platform
            .bridges()
            .get(&TransferId::new("xfer-1"))
            .expect("still there")
            .status()
            .is_open(),
        "a refused block must not fail a transfer"
    );

    // A longer branch from block 1 displaces 2a. The deposit block is now
    // reverted, and the transfer that was waiting on it has nothing to wait
    // for.
    let reorganised = platform.observe_chain(vec![
        block(&chain, 2, "b2b", "b1"),
        block(&chain, 3, "b3b", "b2b"),
    ]);
    assert_eq!(reorganised.reorgs, 1, "{reorganised:?}");
    assert_eq!(reorganised.deepest_reorg, 1);
    assert_eq!(
        reorganised.bridged_transfers_failed, 1,
        "the transfer riding on the withdrawn block was not failed: {reorganised:?}"
    );
    assert!(
        reorganised
            .describe()
            .contains("1 bridge transfer(s) failed"),
        "{}",
        reorganised.describe()
    );

    let after = platform
        .bridges()
        .get(&TransferId::new("xfer-1"))
        .expect("a failed transfer is kept, not dropped");
    assert!(
        matches!(
            after.status(),
            TransferStatus::Failed {
                failure: BridgeFailure::SourceReorg,
                ..
            }
        ),
        "the transfer must be failed for the reorganisation, not left open or failed for \
         something else: {:?}",
        after.status()
    );
    assert_eq!(
        platform.telemetry().metrics.snapshot().counter(
            names::BRIDGE_TRANSFERS_FAILED,
            &labels([("failure", "source_reorg")])
        ),
        1
    );

    // A second reorganisation cannot fail it twice: it is no longer open.
    let again = platform.observe_chain(vec![
        block(&chain, 3, "b3c", "b2a"),
        block(&chain, 4, "b4c", "b3c"),
    ]);
    assert_eq!(again.reorgs, 1, "{again:?}");
    assert_eq!(again.bridged_transfers_failed, 0);
    Ok(())
}
