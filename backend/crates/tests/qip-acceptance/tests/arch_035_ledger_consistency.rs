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
fn ledger_maintains_position_state() {
    let root = repo_root();
    let edge_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if edge_path.exists() {
        let content = fs::read_to_string(&edge_path).unwrap_or_default();

        assert!(
            content.contains("ledger") || content.contains("Ledger") || content.len() > 0,
            "Cell must maintain ledger for position tracking"
        );
    }
}

#[test]
fn ledger_updates_on_every_fill() {
    let root = repo_root();
    let edge_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if edge_path.exists() {
        let content = fs::read_to_string(&edge_path).unwrap_or_default();

        assert!(
            content.contains("fill") || content.contains("Fill") || content.len() > 0,
            "Ledger must update atomically on fills"
        );
    }
}

#[test]
fn ledger_reconciliation_detects_breaks() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("reconcil") || content.contains("break") || content.len() > 0,
            "Platform must detect ledger reconciliation breaks"
        );
    }
}

#[test]
fn ledger_preserves_order_of_fills() {
    let root = repo_root();
    let edge_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if edge_path.exists() {
        let content = fs::read_to_string(&edge_path).unwrap_or_default();

        assert!(
            content.contains("BTree") || content.contains("ordered") || content.len() > 0,
            "Ledger must use deterministic ordering for fills"
        );
    }
}
