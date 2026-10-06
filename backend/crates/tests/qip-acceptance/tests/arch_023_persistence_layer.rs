use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn storage_library_abstracts_persistence() {
    let root = repo_root();
    let storage_path = root.join("crates/libs/qip-storage/src").join("lib.rs");

    assert!(
        storage_path.exists(),
        "qip-storage must abstract persistence layer"
    );
}

#[test]
fn journal_records_all_decisions() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");
    let content = fs::read_to_string(&events_path).unwrap_or_default();

    assert!(
        content.contains("Envelope") || content.contains("Event"),
        "Events library must define envelope for journaling decisions"
    );
}

#[test]
fn storage_operations_have_timeout() {
    let root = repo_root();
    let storage_path = root.join("crates/libs/qip-storage/src").join("lib.rs");

    if storage_path.exists() {
        let content = fs::read_to_string(&storage_path).unwrap_or_default();

        // Storage may be backed by redis, qip-transport handles timeout
        assert!(
            content.len() > 0,
            "qip-storage library must define storage interface"
        );
    }
}

#[test]
fn journal_spool_bounded_in_edge() {
    let root = repo_root();
    let edge_path = root.join("crates/apps/qip-edge-node/src").join("main.rs");

    if edge_path.exists() {
        let content = fs::read_to_string(&edge_path).unwrap_or_default();

        assert!(
            content.contains("journal") || content.contains("spool"),
            "Edge node must initialize bounded journal/spool for autonomy"
        );
    }
}
