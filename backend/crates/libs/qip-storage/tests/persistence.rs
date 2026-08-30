//! Storage ports and their local adapters.

use qip_core::Timestamp;
use qip_storage::kv::{key_for, split_key};
use qip_storage::provider::{StorageProvider, StorageTarget};
use qip_storage::repository::Repository;
use qip_storage::{
    BlobStore, FileBlobStore, FileKeyValueStore, KeyValueStore, KeyValueStoreExt, MemoryBlobStore,
    MemoryKeyValueStore, MemoryRepository,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Position {
    symbol: String,
    quantity: i64,
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("qip-storage-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn exercise_store(store: &dyn KeyValueStore) {
    assert!(store.is_empty().unwrap());
    store.put("a/1", serde_json::json!({"x": 1})).unwrap();
    store.put("a/2", serde_json::json!({"x": 2})).unwrap();
    store.put("b/1", serde_json::json!({"x": 3})).unwrap();

    assert_eq!(store.len().unwrap(), 3);
    assert_eq!(store.get("a/1").unwrap(), Some(serde_json::json!({"x": 1})));
    assert_eq!(store.get("missing").unwrap(), None);
    assert_eq!(store.keys_with_prefix("a/").unwrap(), vec!["a/1", "a/2"]);
    assert!(store.delete("a/1").unwrap());
    assert!(
        !store.delete("a/1").unwrap(),
        "deleting twice is not an error"
    );
    assert_eq!(store.len().unwrap(), 2);
}

#[test]
fn the_memory_store_satisfies_the_port_contract() {
    exercise_store(&MemoryKeyValueStore::new());
}

#[test]
fn the_file_store_satisfies_the_same_contract() {
    let dir = temp_dir("kv");
    exercise_store(&FileKeyValueStore::open(dir.join("store.json")).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_file_store_survives_a_restart() {
    let dir = temp_dir("kv-restart");
    let path = dir.join("store.json");
    {
        let store = FileKeyValueStore::open(&path).unwrap();
        store
            .put_as(
                "positions/AAPL",
                &Position {
                    symbol: "AAPL".into(),
                    quantity: 100,
                },
            )
            .unwrap();
    }
    let reopened = FileKeyValueStore::open(&path).unwrap();
    let position: Position = reopened.get_as("positions/AAPL").unwrap().unwrap();
    assert_eq!(position.quantity, 100);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn typed_helpers_round_trip_through_the_dyn_port() {
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());
    store
        .put_as(
            "positions/AAPL",
            &Position {
                symbol: "AAPL".into(),
                quantity: 100,
            },
        )
        .unwrap();
    store
        .put_as(
            "positions/MSFT",
            &Position {
                symbol: "MSFT".into(),
                quantity: -50,
            },
        )
        .unwrap();

    let scanned: Vec<(String, Position)> = store.scan_as("positions/").unwrap();
    assert_eq!(scanned.len(), 2);
    assert_eq!(scanned[0].1.symbol, "AAPL");
    assert_eq!(scanned[1].1.quantity, -50);
}

#[test]
fn repository_versions_records_and_preserves_creation_time() {
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());
    let repository = MemoryRepository::<Position>::new(store, "positions");
    let created = Timestamp::from_civil(2026, 8, 22);
    let updated = Timestamp::from_civil(2026, 8, 23);

    let first = repository
        .put(
            "AAPL",
            Position {
                symbol: "AAPL".into(),
                quantity: 100,
            },
            created,
        )
        .unwrap();
    assert_eq!(first.version, 1);

    let second = repository
        .put(
            "AAPL",
            Position {
                symbol: "AAPL".into(),
                quantity: 150,
            },
            updated,
        )
        .unwrap();
    assert_eq!(second.version, 2, "an update must bump the version");
    assert_eq!(second.created_at, created, "creation time is preserved");
    assert_eq!(second.updated_at, updated);

    assert_eq!(repository.count().unwrap(), 1);
    assert_eq!(repository.list().unwrap().len(), 1);
    assert!(repository.delete("AAPL").unwrap());
    assert!(repository.get("AAPL").unwrap().is_none());
}

#[test]
fn blob_stores_round_trip_and_report_digests() {
    for store in [
        Box::new(MemoryBlobStore::new()) as Box<dyn BlobStore>,
        Box::new(FileBlobStore::open(temp_dir("blob")).unwrap()),
    ] {
        store
            .put("reports/2026/attribution.json", b"{}".to_vec())
            .unwrap();
        assert_eq!(
            store.get("reports/2026/attribution.json").unwrap(),
            Some(b"{}".to_vec())
        );
        assert_eq!(store.list("reports/").unwrap().len(), 1);
        assert_eq!(
            store
                .digest("reports/2026/attribution.json")
                .unwrap()
                .unwrap(),
            qip_core::hash::sha256_hex(b"{}")
        );
        assert!(store.delete("reports/2026/attribution.json").unwrap());
        assert_eq!(store.get("reports/2026/attribution.json").unwrap(), None);
    }
}

#[test]
fn the_file_blob_store_refuses_path_traversal() {
    let dir = temp_dir("blob-traversal");
    let store = FileBlobStore::open(&dir).unwrap();
    for hostile in ["../escape", "/etc/passwd", "a/../../b", "", "./x"] {
        assert!(
            store.put(hostile, b"x".to_vec()).is_err(),
            "must reject key {hostile:?}"
        );
    }
    store.put("safe/file.json", b"x".to_vec()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn keys_compose_and_decompose() {
    assert_eq!(key_for("positions", "AAPL"), "positions/AAPL");
    assert_eq!(split_key("positions/AAPL").unwrap(), ("positions", "AAPL"));
    assert!(split_key("malformed").is_err());
}

#[test]
fn the_provider_builds_local_adapters() {
    let dir = temp_dir("provider");
    for target in [
        StorageTarget::Memory,
        StorageTarget::File,
        StorageTarget::Engine,
    ] {
        let provider = StorageProvider::new(target, &dir);
        let store = provider.key_value(&format!("test-{target:?}")).unwrap();
        store.put("k", serde_json::json!(1)).unwrap();
        assert_eq!(store.len().unwrap(), 1);
        assert!(provider.blobs("artifacts").is_ok());
        assert!(target.is_implemented());
        assert_eq!(
            target.required_configuration(),
            None,
            "{target:?} is built in and must not ask for credentials"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_targets_that_flush_claim_to_be_crash_safe() {
    // Memory is honest about losing everything; the two file-backed targets
    // fsync before acknowledging a write.
    assert!(!StorageTarget::Memory.is_crash_safe());
    assert!(StorageTarget::File.is_crash_safe());
    assert!(StorageTarget::Engine.is_crash_safe());
}

#[test]
fn the_file_blob_store_never_lists_a_scratch_file_left_by_a_crash() {
    // A crash between writing a temporary and renaming it leaves the partial
    // file behind. It holds no complete object and must not appear as one.
    let dir = temp_dir("blob-scratch");
    let store = FileBlobStore::open(&dir).unwrap();
    store.put("reports/final.json", b"{}".to_vec()).unwrap();
    std::fs::create_dir_all(dir.join("reports")).unwrap();
    std::fs::write(dir.join("reports/final.json.qip-partial.9"), b"half").unwrap();

    assert_eq!(
        store.list("").unwrap(),
        vec!["reports/final.json"],
        "the scratch file is not a stored object"
    );
    assert_eq!(
        store.get("reports/final.json").unwrap(),
        Some(b"{}".to_vec()),
        "and the real object is untouched by it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_store_write_leaves_no_scratch_file_behind() {
    let dir = temp_dir("kv-scratch");
    let store = FileKeyValueStore::open(dir.join("store.json")).unwrap();
    for i in 0..5 {
        store.put(&format!("k{i}"), serde_json::json!(i)).unwrap();
    }
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("qip-partial"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "the temporary must be renamed, not accumulated: {leftovers:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unconfigured_managed_target_fails_loudly_and_names_what_is_missing() {
    // A deployment pointed at BigQuery must not silently fall back to files.
    for target in [
        StorageTarget::AlloyDb,
        StorageTarget::Spanner,
        StorageTarget::Bigtable,
        // Memorystore is deliberately absent: its adapter exists, so
        // `is_implemented` is true for it while an unconfigured deployment is
        // still refused. That pair of facts is asserted next to the adapter,
        // in `redis::tests`.
        //
        // Cloud Storage and BigQuery are absent for the same reason: both now
        // have REST adapters in `gcp`, so `is_implemented` is true for them
        // too, and `key_value` refuses them for a different reason again —
        // neither is a key-value store. The pair of facts, and the refusals
        // naming the right shape, are asserted in `tests/gcp.rs`.
    ] {
        assert!(!target.is_implemented());
        let provider = StorageProvider::new(target, "/tmp");
        let err = provider.key_value("test").unwrap_err();
        assert_eq!(err.code(), "unavailable", "{target:?}");
        let message = err.to_string();
        assert!(
            message.contains("docs/operations/external-dependencies.md"),
            "{target:?} error should point at the documentation: {message}"
        );
        assert!(
            target.required_configuration().is_some(),
            "{target:?} must state what it needs"
        );
        assert!(
            !target.rationale().is_empty(),
            "{target:?} must justify its existence"
        );
    }
}
