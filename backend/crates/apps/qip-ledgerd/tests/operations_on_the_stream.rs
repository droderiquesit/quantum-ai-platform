//! ADR 0123: a ledger operation delivered on the outcomes stream is booked
//! once by operation id and moves the partition's cursor in the same commit;
//! a refused one is noted and does not stall the partition.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::ledger::{Account, Settlement};
use qip_contracts::ledger_operation::{
    Book, LedgerOperation, LedgerOperationFields, OperationKind, Phase,
};
use qip_contracts::validated::Validated;
use qip_core::{Clock, Decimal, ManualClock, Timestamp, dec};
use qip_ledgerd::store::{LedgerStore, OperationApplied, OperationDelivery};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::Metrics;
use qip_storage::EngineConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const PARTITION: &str = "reflex-outcomes.local/0";

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-ledger-ops-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open(dir: &Path) -> LedgerStore {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
    LedgerStore::open(
        dir,
        EngineConfig::new(clock),
        LedgerTelemetry::new(Arc::new(Metrics::new("qip-ledgerd"))),
    )
    .unwrap()
}

fn funding(id: &str, amount: Decimal) -> LedgerOperation {
    Validated::new(LedgerOperationFields {
        schema_version: 1,
        operation_id: id.to_string(),
        origin: "desk".to_string(),
        sequence: 0,
        recorded_at: Timestamp::from_secs(1_790_000_100),
        settlement: Settlement::Simulated,
        kind: OperationKind::TreasuryAllocation {
            treasury: "central".to_string(),
            book: Book {
                cell: "desk".to_string(),
                strategy: "central-desk".to_string(),
            },
            unit: "USD".to_string(),
            amount,
            phase: Phase::Establish,
        },
    })
    .unwrap()
}

fn at(offset: u64, hash: &str, operation: LedgerOperation) -> OperationDelivery {
    OperationDelivery {
        partition: PARTITION.to_string(),
        offset,
        record_hash: hash.to_string(),
        operation,
    }
}

fn trading() -> Account {
    Account::Trading {
        cell: "desk".to_string(),
        strategy: "central-desk".to_string(),
    }
}

/// Booked once, and the cursor moved with it: a resend of the same operation
/// at a later offset (a new producer epoch after a crash) moves only the
/// cursor, and a reopened store resumes past both.
///
/// Mutation: in `apply_operation`, commit the posting batch without the
/// cursor row — `resume_offset` stays at zero and the cursor assertion fails.
/// Mutation: treat a held id with the same digest as new — the funding is
/// booked twice and the balance assertion fails.
#[test]
fn an_operation_is_booked_once_and_its_offset_moves_in_the_same_commit() {
    let dir = temp_dir("once");
    {
        let store = open(&dir);
        assert_eq!(store.resume_offset(PARTITION).unwrap(), 0, "premise");
        assert_eq!(
            store
                .apply_operation(&at(
                    0,
                    "h0",
                    funding("desk:treasury-funding:0", dec!("1000"))
                ))
                .unwrap(),
            OperationApplied::Posted { postings: 2 }
        );
        assert_eq!(store.resume_offset(PARTITION).unwrap(), 1);
        assert_eq!(
            store
                .apply_operation(&at(
                    1,
                    "h1",
                    funding("desk:treasury-funding:0", dec!("1000"))
                ))
                .unwrap(),
            OperationApplied::Duplicate
        );
        assert_eq!(store.resume_offset(PARTITION).unwrap(), 2);
    }
    let store = open(&dir);
    assert_eq!(
        store.resume_offset(PARTITION).unwrap(),
        2,
        "the cursor was not durable"
    );
    assert_eq!(store.balance(&trading(), "USD").unwrap(), dec!("1000"));
    assert_eq!(
        store
            .balance(
                &Account::Treasury {
                    treasury: "central".to_string()
                },
                "USD"
            )
            .unwrap(),
        dec!("-1000")
    );
    let trial = store.trial_balance().unwrap();
    assert!(trial.values().all(|total| total.is_zero()), "{trial:?}");
}

/// A second digest under a held id is refused, noted and passed: nothing is
/// booked for it, the partition's cursor moves past it, and the journal says
/// which operation and why — so one bad operation never holds the fills
/// behind it.
///
/// Mutation: return the refusal as `Err` from `apply_operation` instead of
/// noting it — the call fails and the partition would stall on it.
#[test]
fn a_conflicting_operation_is_refused_noted_and_does_not_stall_the_partition() {
    let dir = temp_dir("conflict");
    let store = open(&dir);
    store
        .apply_operation(&at(
            0,
            "h0",
            funding("desk:treasury-funding:0", dec!("1000")),
        ))
        .unwrap();
    let applied = store
        .apply_operation(&at(
            1,
            "h1",
            funding("desk:treasury-funding:0", dec!("2000")),
        ))
        .unwrap();
    assert!(
        matches!(&applied, OperationApplied::Refused { detail } if detail.contains("two different operations claim one id")),
        "{applied:?}"
    );
    assert_eq!(
        store.resume_offset(PARTITION).unwrap(),
        2,
        "the partition stalled"
    );
    assert_eq!(
        store.balance(&trading(), "USD").unwrap(),
        dec!("1000"),
        "the conflicting operation moved money"
    );
    let notes = store.journal().unwrap();
    assert!(
        notes.iter().any(|note| note.event == "operation_refused"
            && note.detail.contains("desk:treasury-funding:0")),
        "the refusal is not on the record: {notes:?}"
    );
}

/// R12-LEDGER-8137, the keys half: consecutive journal notes are never
/// written under keys that sort in write order, so on Spanner no single key
/// range takes every commit; and the journal still reads oldest first,
/// because the reader orders by the sequence each key names.
///
/// Eight refusals are noted one after another. Their raw keys, read from the
/// store underneath the ledger, must not be in ascending order, and must
/// spread over more than one shard. `LedgerStore::journal` must return the
/// eight in the offsets they were noted at.
///
/// Mutation: write `journal/<sequence>` without the shard prefix and the raw
/// keys sort in write order.
#[test]
fn consecutive_commits_never_write_sequence_keys_that_sort_in_write_order() {
    let dir = temp_dir("hotspot");
    {
        let store = open(&dir);
        store
            .apply_operation(&at(
                0,
                "h0",
                funding("desk:treasury-funding:0", dec!("1000")),
            ))
            .unwrap();
        for offset in 1..=8 {
            let applied = store
                .apply_operation(&at(
                    offset,
                    &format!("h{offset}"),
                    funding("desk:treasury-funding:0", dec!("2000")),
                ))
                .unwrap();
            assert!(
                matches!(applied, OperationApplied::Refused { .. }),
                "premise: each conflicting operation is refused and noted: {applied:?}"
            );
        }
        let offsets: Vec<u64> = store
            .journal()
            .unwrap()
            .iter()
            .filter(|note| note.event == "operation_refused")
            .map(|note| note.offset)
            .collect();
        assert_eq!(
            offsets,
            (1..=8).collect::<Vec<u64>>(),
            "the journal does not read oldest first"
        );
    }
    let raw = qip_storage::DurableStore::open(
        &dir,
        EngineConfig::new(Arc::new(ManualClock::new(Timestamp::from_secs(
            1_790_000_000,
        )))),
    )
    .unwrap();
    let keys: Vec<String> = raw
        .scan_prefix("journal/")
        .unwrap()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(keys.len(), 8, "premise: eight notes were written: {keys:?}");
    let sequences: Vec<u64> = keys
        .iter()
        .map(|key| key.rsplit('/').next().unwrap().parse().unwrap())
        .collect();
    assert!(
        sequences.windows(2).any(|pair| pair[0] > pair[1]),
        "the journal's keys sort in the order they were written: {keys:?}"
    );
    let shards: std::collections::BTreeSet<&str> = keys
        .iter()
        .filter_map(|key| key.split('/').nth(1))
        .collect();
    assert!(
        shards.len() > 1,
        "every note landed in one key range: {keys:?}"
    );
}
