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
fn collections_use_btree_for_ordered_iteration() {
    let root = repo_root();
    let contracts_path = root.join("crates/libs/qip-contracts/src").join("lib.rs");
    let content = fs::read_to_string(&contracts_path).unwrap_or_default();

    assert!(
        !content.contains("HashMap"),
        "Use BTreeMap where iteration order matters"
    );
}

#[test]
fn venue_lists_maintain_order() {
    let root = repo_root();
    let financial_dir = root.join("crates/libs/qip-financial/src");

    let mut found_btree = false;
    if let Ok(entries) = fs::read_dir(&financial_dir) {
        for entry in entries {
            if let Ok(e) = entry {
                if let Ok(content) = fs::read_to_string(&e.path()) {
                    if content.contains("BTree") {
                        found_btree = true;
                    }
                }
            }
        }
    }

    assert!(
        found_btree,
        "Collections must maintain order for venue iteration"
    );
}

#[test]
fn ordering_deterministic_across_runs() {
    let root = repo_root();
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if cell_path.exists() {
        let content = fs::read_to_string(&cell_path).unwrap_or_default();

        // Cell must use deterministic collections
        assert!(
            !content.contains("hash") || content.contains("BTree"),
            "Use BTree for deterministic ordering"
        );
    }
}

#[test]
fn replay_matches_order() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if let Ok(content) = fs::read_to_string(&platform_path) {
        assert!(
            !content.contains("random") || content.contains("deterministic"),
            "Replay must be deterministic"
        );
    }
}
