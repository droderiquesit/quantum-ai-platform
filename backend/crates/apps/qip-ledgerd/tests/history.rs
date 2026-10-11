//! Balances are a fold of the ledger's history and nothing else
//! (R12-LEDGER-5001), and the operation subledgers book through the same
//! one-batch writer as fills (R12-LEDGER-5005..5011, 5016..5021).
//!
//! `LedgerStore::import_state` wrote balance rows straight from an export,
//! with no event behind them, and silently skipped every key with more than
//! one `:` — every key a trading account renders to. It is removed; these
//! tests prove what replaced it: a ledger whose rows are not the fold of its
//! history does not open, and a key it cannot read stops it rather than
//! being skipped.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::ledger::{Account, Settlement};
use qip_contracts::ledger_operation::{
    Book, LedgerOperation, LedgerOperationFields, OperationKind, Phase,
};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{ChainVersion, Decision, JournalEntry, OutcomeRecord, seal_v2};
use qip_contracts::validated::Validated;
use qip_core::{Clock, Decimal, ManualClock, Timestamp, dec, sha256_hex};
use qip_ledgerd::store::{
    Delivery, Disposition, GENESIS, LedgerStore, OperationDisposition, P1Record, parse_balance_key,
};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::Metrics;
use qip_storage::{DurableStore, EngineConfig, WriteBatch};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const UNIT: &str = "USD";
const VENUE: &str = "sim-xnys";

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-ledger-history-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn config() -> EngineConfig {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
    EngineConfig::new(clock)
}

fn open(dir: &Path) -> qip_core::error::Result<LedgerStore> {
    LedgerStore::open(
        dir,
        config(),
        LedgerTelemetry::new(Arc::new(Metrics::new("qip-ledgerd"))),
    )
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

fn balance_key(account: &Account, unit: &str) -> String {
    format!("balance/{}/{}", hex(&account.to_string()), hex(unit))
}

/// Write one row behind the ledger's back, the way `import_state` did.
fn write_behind(dir: &Path, key: &str, value: serde_json::Value) {
    let store = DurableStore::open(dir, config()).expect("the raw store opens");
    store
        .commit(WriteBatch::new().put(key, value))
        .expect("the raw write commits");
}

fn row(account: &Account, unit: &str, amount: &str) -> serde_json::Value {
    serde_json::json!({ "account": account, "unit": unit, "amount": amount })
}

/// Two simulated fills on one cell's chain, as P1 deliveries.
fn two_fills() -> Vec<Delivery> {
    let mut previous = GENESIS.to_string();
    let mut deliveries = Vec::new();
    for (sequence, (order, side, price)) in [
        ("o-0", BookSide::Bid, "10.50"),
        ("o-1", BookSide::Ask, "11.25"),
    ]
    .into_iter()
    .enumerate()
    {
        let sequence = sequence as u64;
        let at = Timestamp::from_secs(1_790_000_000 + sequence as i64);
        let decision = Decision::Filled {
            order_id: order.to_string(),
            venue: VENUE.to_string(),
            object: "OBJ-ACME".to_string(),
            quantity: "10".to_string(),
            price: price.to_string(),
            simulated: true,
            shares: vec![("alpha".to_string(), "10".to_string())],
            side: Some(side),
            quote_unit: Some(UNIT.to_string()),
            fee: None,
        };
        let (decision, digest) = seal_v2(&previous, sequence, at, decision);
        previous = digest.clone();
        let entry = JournalEntry {
            sequence,
            at,
            decision,
            digest: digest.clone(),
            version: ChainVersion::V2,
        };
        let record = P1Record::Outcome(Box::new(OutcomeRecord {
            cell: "cell-a".to_string(),
            session: 1,
            journal_sequence: sequence,
            journal_digest: digest,
            entry,
        }));
        deliveries.push(Delivery {
            partition: "p0".to_string(),
            offset: sequence,
            record_hash: sha256_hex(&serde_json::to_vec(&record).expect("serialises")),
            record,
        });
    }
    deliveries
}

/// A ledger directory holding two booked fills, closed.
fn booked(label: &str) -> PathBuf {
    let dir = temp_dir(label);
    let store = open(&dir).expect("a fresh ledger opens");
    for delivery in two_fills() {
        let disposition = store.apply(&delivery).expect("apply");
        assert!(
            matches!(disposition, Disposition::Posted { .. }),
            "premise: each fill posts, got {disposition:?}"
        );
    }
    dir
}

fn book() -> Book {
    Book {
        cell: "cell-a".to_string(),
        strategy: "alpha".to_string(),
    }
}

fn op(id: &str, kind: OperationKind) -> LedgerOperation {
    Validated::new(LedgerOperationFields {
        schema_version: 1,
        operation_id: id.to_string(),
        origin: "capital-desk".to_string(),
        sequence: 1,
        recorded_at: Timestamp::from_secs(1_790_000_100),
        settlement: Settlement::Simulated,
        kind,
    })
    .expect("an admissible operation")
}

fn reservation(id: &str, amount: &str, phase: Phase) -> LedgerOperation {
    op(
        id,
        OperationKind::InventoryReservation {
            book: book(),
            unit: "OBJ-ACME".to_string(),
            amount: Decimal::parse(amount).expect("literal"),
            phase,
        },
    )
}

fn trading() -> Account {
    Account::Trading {
        cell: "cell-a".to_string(),
        strategy: "alpha".to_string(),
    }
}

// --- R2.2 / R12-LEDGER-5001 ---------------------------------------------------

/// A balance row with no event behind it — the row `import_state` used to
/// write — stops the ledger opening, and so does a row whose amount the
/// history does not produce. Premise first: the untampered ledger reopens
/// and holds balances.
///
/// Mutation: delete the `ledger.verify_history()?` call in
/// `LedgerStore::with_backend` — both tampered ledgers open and this fails.
#[test]
fn a_balance_written_outside_the_event_history_stops_the_ledger_opening() {
    let clean = booked("clean");
    let reopened = open(&clean).expect("premise: an untampered ledger reopens");
    let balances = reopened.balances().expect("balances");
    assert!(!balances.is_empty(), "premise: the fills booked balances");
    assert_eq!(reopened.refold().expect("refold"), balances);
    drop(reopened);

    let phantom = booked("phantom");
    let account = Account::Venue {
        venue: "phantom".to_string(),
    };
    write_behind(
        &phantom,
        &balance_key(&account, UNIT),
        row(&account, UNIT, "5"),
    );
    let refused = open(&phantom).expect_err("a balance with no history is refused at open");
    assert!(
        refused.to_string().contains("no event that posts to it"),
        "refused for the wrong reason: {refused}"
    );

    let inflated = booked("inflated");
    let (key, amount) = {
        let store = open(&inflated).expect("premise: opens before the tamper");
        let held = store.balance(&trading(), UNIT).expect("balance");
        assert!(!held.is_zero(), "premise: the trading book moved cash");
        (balance_key(&trading(), UNIT), held + Decimal::ONE)
    };
    write_behind(&inflated, &key, row(&trading(), UNIT, &amount.to_string()));
    let refused = open(&inflated).expect_err("an amount the history does not fold to is refused");
    assert!(
        refused.to_string().contains("its history folds to"),
        "refused for the wrong reason: {refused}"
    );
    for dir in [clean, phantom, inflated] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Every key the ledger writes parses back to the account and unit its row
/// holds — including the trading keys, whose text has two `:`, that the old
/// parser skipped — and a key that does not parse stops the ledger rather
/// than being skipped.
///
/// Mutation: in `LedgerStore::verify_history`, replace
/// `let (account, unit) = parse_balance_key(&key)?;` with a `match` that
/// `continue`s on `Err` — the unreadable rows are skipped, the ledger opens,
/// and this fails.
#[test]
fn a_balance_key_that_cannot_be_parsed_stops_the_ledger_rather_than_being_skipped() {
    let dir = booked("parse");
    let raw = DurableStore::open(&dir, config()).expect("raw store");
    let rows = raw.scan_prefix("balance/").expect("scan");
    drop(raw);
    let trading_rows = rows
        .iter()
        .filter(|(key, _)| {
            parse_balance_key(key).is_ok_and(|(account, unit)| {
                format!("{account}:{unit}").matches(':').count() >= 2
                    && matches!(account, Account::Trading { .. })
            })
        })
        .count();
    assert!(
        trading_rows > 0,
        "premise: the ledger wrote a trading key, the kind the old parser skipped"
    );
    for (key, value) in &rows {
        let (account, unit) =
            parse_balance_key(key).unwrap_or_else(|e| panic!("{key} does not parse: {e}"));
        assert_eq!(value["unit"], serde_json::json!(unit));
        assert_eq!(
            value["account"],
            serde_json::to_value(&account).expect("account serialises")
        );
    }

    for (label, bad_key) in [
        ("not-hex", "balance/zz/555344".to_string()),
        (
            "not-an-account",
            format!("balance/{}/{}", hex("ledger:x"), hex(UNIT)),
        ),
        ("no-unit", format!("balance/{}", hex("venue:x"))),
        (
            "uppercase-hex",
            format!("balance/{}/{}", hex("venue:x").to_uppercase(), hex(UNIT)),
        ),
    ] {
        let tampered = booked(label);
        let account = Account::Venue {
            venue: "x".to_string(),
        };
        write_behind(&tampered, &bad_key, row(&account, UNIT, "0"));
        let refused = open(&tampered).expect_err("a balance key the ledger cannot read is refused");
        assert!(
            refused.to_string().contains("balance key"),
            "{label}: refused for the wrong reason: {refused}"
        );
        let _ = std::fs::remove_dir_all(tampered);
    }
    let _ = std::fs::remove_dir_all(dir);
}

// --- operations -------------------------------------------------------------------

/// R12-LEDGER-5016's check, through the writer: an operation posts to its
/// subledger, the global trial balance closes in every unit, the balances
/// equal the refold of the history, and the ledger reopens on that history.
///
/// Mutation: in `LedgerStore::record_operation`, drop the loop that puts
/// the folded balance rows into the batch — the treasury balance reads zero
/// and the first assertion fails (and the reopen refuses, since the history
/// folds to something no row holds).
#[test]
fn an_operation_posts_to_its_subledger_and_the_ledger_reopens_on_its_own_history() {
    let dir = booked("operations");
    let store = open(&dir).expect("opens");
    let operations = [
        op(
            "treasury-1",
            OperationKind::TreasuryAllocation {
                treasury: "paper-pool".to_string(),
                book: book(),
                unit: UNIT.to_string(),
                amount: dec!("1000"),
                phase: Phase::Establish,
            },
        ),
        op(
            "custody-1",
            OperationKind::CustodyMovement {
                custodian: "sim-custodian".to_string(),
                book: book(),
                unit: "OBJ-ACME".to_string(),
                amount: dec!("4"),
                phase: Phase::Establish,
            },
        ),
        op(
            "margin-1",
            OperationKind::MarginRequirement {
                venue: VENUE.to_string(),
                unit: UNIT.to_string(),
                amount: dec!("120"),
                phase: Phase::Establish,
            },
        ),
        op(
            "tax-1",
            OperationKind::TaxCharge {
                book: book(),
                jurisdiction: "us-federal".to_string(),
                unit: UNIT.to_string(),
                amount: dec!("12.5"),
            },
        ),
    ];
    for operation in &operations {
        assert!(matches!(
            store.record_operation(operation).expect("records"),
            OperationDisposition::Posted { postings: 2 }
        ));
    }
    let treasury = Account::Treasury {
        treasury: "paper-pool".to_string(),
    };
    assert_eq!(
        store.balance(&treasury, UNIT).expect("balance"),
        dec!("-1000")
    );
    assert_eq!(
        store
            .balance(
                &Account::Custody {
                    custodian: "sim-custodian".to_string()
                },
                "OBJ-ACME"
            )
            .expect("balance"),
        dec!("4")
    );
    assert_eq!(
        store
            .balance(
                &Account::Obligation {
                    counterparty: VENUE.to_string()
                },
                UNIT
            )
            .expect("balance"),
        dec!("-120")
    );
    let trial = store.trial_balance().expect("trial balance");
    assert!(
        trial.contains_key(UNIT) && trial.contains_key("OBJ-ACME"),
        "premise: both units moved: {trial:?}"
    );
    assert!(
        trial.values().all(|total| total.is_zero()),
        "the trial balance does not close: {trial:?}"
    );
    let balances = store.balances().expect("balances");
    assert_eq!(store.refold().expect("refold"), balances);
    drop(store);
    let reopened = open(&dir).expect("the ledger reopens on its own history");
    assert_eq!(reopened.balances().expect("balances"), balances);
    let _ = std::fs::remove_dir_all(dir);
}

/// R12-LEDGER-8024 for operations: the same operation delivered again moves
/// nothing, and a different operation under the same id is refused rather
/// than booked or chosen between.
///
/// Mutation: delete the `held.digest == digest` early return in
/// `LedgerStore::record_operation` — the redelivery is refused as a
/// conflict instead of answered as a duplicate, and this fails; delete the
/// whole held-record check and the redelivery posts twice.
#[test]
fn a_redelivered_operation_moves_nothing_and_a_different_one_under_its_id_is_refused() {
    let dir = temp_dir("dedup");
    let store = open(&dir).expect("opens");
    let reserve = reservation("res-1", "5", Phase::Establish);
    assert_eq!(
        store.record_operation(&reserve).expect("first"),
        OperationDisposition::Posted { postings: 2 }
    );
    let reserved = Account::Reserved {
        cell: "cell-a".to_string(),
        strategy: "alpha".to_string(),
    };
    let after_first = store.balances().expect("balances");
    assert_eq!(
        store.record_operation(&reserve).expect("redelivery"),
        OperationDisposition::Duplicate
    );
    assert_eq!(store.balances().expect("balances"), after_first);
    assert_eq!(
        store.balance(&reserved, "OBJ-ACME").expect("balance"),
        dec!("5")
    );

    let impostor = reservation("res-1", "6", Phase::Establish);
    let refused = store
        .record_operation(&impostor)
        .expect_err("a different operation under a held id is refused");
    assert!(
        refused
            .to_string()
            .contains("two different operations claim one id")
    );
    assert_eq!(store.balances().expect("balances"), after_first);
    let _ = std::fs::remove_dir_all(dir);
}

/// More cannot be released than was reserved: the event that would take a
/// quantity-holding account below zero is refused and moves nothing.
///
/// Mutation: delete the `holds_a_quantity()` refusal in `LedgerStore::fold`
/// — the over-release books, the reserved balance reads -2, and this fails.
#[test]
fn releasing_more_than_is_reserved_is_refused_and_moves_nothing() {
    let dir = temp_dir("over-release");
    let store = open(&dir).expect("opens");
    store
        .record_operation(&reservation("res-1", "5", Phase::Establish))
        .expect("reserve");
    store
        .record_operation(&reservation("rel-1", "3", Phase::Unwind))
        .expect("premise: a release within the reservation books");
    let before = store.balances().expect("balances");
    let refused = store
        .record_operation(&reservation("rel-2", "4", Phase::Unwind))
        .expect_err("releasing 4 of the 2 still reserved is refused");
    assert!(
        refused.to_string().contains("more cannot be released"),
        "refused for the wrong reason: {refused}"
    );
    assert_eq!(store.balances().expect("balances"), before);
    assert_eq!(
        store
            .record_operation(&reservation("rel-2", "2", Phase::Unwind))
            .expect("the refused id was not consumed"),
        OperationDisposition::Posted { postings: 2 }
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// R12-LEDGER-5008: a lot's open quantity and remaining basis are read from
/// its postings, and its holding period from the operation that opened it;
/// a lot id is opened once, and more of it cannot be closed than is open.
///
/// Mutation: drop the `lot_key` check in `LedgerStore::record_operation` —
/// the second opening of `lot-1` books, the lot reads 16, and this fails.
#[test]
fn a_tax_lot_is_read_from_its_postings_with_the_acquisition_time_its_opening_recorded() {
    let dir = temp_dir("lots");
    let store = open(&dir).expect("opens");
    let acquired = Timestamp::from_secs(1_789_000_000);
    let lot = |id: &str, quantity: &str, cost: &str, phase: Phase| {
        op(
            id,
            OperationKind::TaxLot {
                book: book(),
                lot: "lot-1".to_string(),
                unit: "OBJ-ACME".to_string(),
                quantity: Decimal::parse(quantity).expect("literal"),
                cost_unit: UNIT.to_string(),
                cost: Decimal::parse(cost).expect("literal"),
                acquired_at: acquired,
                phase,
            },
        )
    };
    store
        .record_operation(&lot("open-1", "10", "105", Phase::Establish))
        .expect("open");
    store
        .record_operation(&lot("close-1", "4", "42", Phase::Unwind))
        .expect("partial close");
    let lots = store.tax_lots().expect("lots");
    assert_eq!(lots.len(), 1, "premise: one lot");
    assert_eq!(lots[0].quantity, dec!("6"));
    assert_eq!(lots[0].cost, dec!("63"));
    assert_eq!(lots[0].acquired_at, acquired);

    let reopened = store
        .record_operation(&lot("open-2", "6", "70", Phase::Establish))
        .expect_err("a lot id is opened once");
    assert!(reopened.to_string().contains("already open"));
    assert!(
        store
            .record_operation(&lot("close-2", "7", "70", Phase::Unwind))
            .is_err(),
        "closing 7 of an open 6 is refused"
    );
    assert_eq!(store.tax_lots().expect("lots")[0].quantity, dec!("6"));
    let _ = std::fs::remove_dir_all(dir);
}
