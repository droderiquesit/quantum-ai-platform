//! R12-LEDGER-5018: a physical inventory/commerce event posts to the
//! inventory subledger (`Account::Inventory`) and rolls up into the one
//! global double-entry journal, whose trial balance still closes.
//!
//! The desk ships the operation `Platform::queue_inventory_moves` builds;
//! this drives the same shape (receipt from a counterparty, sale back out)
//! through `LedgerStore::record_operation`, the consumer's own path.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::ledger::{Account, Settlement};
use qip_contracts::ledger_operation::{
    InventoryEnd, LedgerOperation, LedgerOperationFields, OperationKind,
};
use qip_contracts::validated::Validated;
use qip_core::{Clock, Decimal, ManualClock, Timestamp, dec};
use qip_ledgerd::store::{LedgerStore, OperationDisposition};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::Metrics;
use qip_storage::EngineConfig;
use std::path::Path;
use std::sync::Arc;

const SKU: &str = "SKU-LAMP";
const WAREHOUSE: &str = "warehouse-1";
const SUPPLIER: &str = "sim-supplier";
const BUYER: &str = "sim-buyer";

fn open(dir: &Path) -> LedgerStore {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
    LedgerStore::open(
        dir,
        EngineConfig::new(clock),
        LedgerTelemetry::new(Arc::new(Metrics::new("qip-ledgerd"))),
    )
    .expect("opens")
}

fn movement(id: &str, from: InventoryEnd, to: InventoryEnd, amount: Decimal) -> LedgerOperation {
    Validated::new(LedgerOperationFields {
        schema_version: 1,
        operation_id: id.to_string(),
        origin: "desk".to_string(),
        sequence: 1,
        recorded_at: Timestamp::from_secs(1_790_000_100),
        settlement: Settlement::Simulated,
        kind: OperationKind::PhysicalInventory {
            from,
            to,
            unit: SKU.to_string(),
            amount,
        },
    })
    .expect("an admissible operation")
}

fn counterparty(name: &str) -> InventoryEnd {
    InventoryEnd::Counterparty {
        counterparty: name.to_string(),
    }
}

fn warehouse() -> InventoryEnd {
    InventoryEnd::Location {
        location: WAREHOUSE.to_string(),
    }
}

/// A receipt lands in the warehouse's inventory account and a sale takes
/// it out; each is two postings, the global trial balance closes for the
/// unit, and the ledger reopens on its own history.
///
/// Mutation: in `OperationKind::postings` (qip-contracts
/// `ledger_operation.rs`), build the `PhysicalInventory` pair against
/// `from.account()` on both legs — the warehouse balance reads zero and
/// the first balance assertion fails.
#[test]
fn a_physical_inventory_event_posts_to_the_inventory_subledger_and_the_global_trial_balance_closes()
{
    let dir = std::env::temp_dir().join(format!("qip-ledgerd-inventory-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let store = open(&dir);
    let inventory = Account::Inventory {
        location: WAREHOUSE.to_string(),
    };

    let received = movement("recv-1", counterparty(SUPPLIER), warehouse(), dec!("3"));
    assert!(matches!(
        store.record_operation(&received).expect("records"),
        OperationDisposition::Posted { postings: 2 }
    ));
    assert_eq!(
        store.balance(&inventory, SKU).expect("balance"),
        dec!("3"),
        "a receipt did not land in the warehouse's inventory subledger"
    );
    assert_eq!(
        store
            .balance(
                &Account::Venue {
                    venue: SUPPLIER.to_string()
                },
                SKU
            )
            .expect("balance"),
        dec!("-3")
    );

    let sold = movement("sale-1", warehouse(), counterparty(BUYER), dec!("1"));
    assert!(matches!(
        store.record_operation(&sold).expect("records"),
        OperationDisposition::Posted { postings: 2 }
    ));
    assert_eq!(store.balance(&inventory, SKU).expect("balance"), dec!("2"));

    let trial = store.trial_balance().expect("trial balance");
    assert!(
        trial.contains_key(SKU),
        "premise: the unit moved in the global journal: {trial:?}"
    );
    assert!(
        trial.values().all(|total| total.is_zero()),
        "the global trial balance does not close: {trial:?}"
    );
    let balances = store.balances().expect("balances");
    assert_eq!(store.refold().expect("refold"), balances);
    drop(store);
    let reopened = open(&dir);
    assert_eq!(reopened.balances().expect("balances"), balances);
    let _ = std::fs::remove_dir_all(dir);
}
