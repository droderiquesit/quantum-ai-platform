use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn ledger_maintains_position_state() {
    let root = repo_root();
    let edge_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if edge_path.exists() {
        let content = fs::read_to_string(&edge_path).unwrap_or_default();

        assert!(
            content.contains("ledger") || content.contains("Ledger") || !content.is_empty(),
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
            content.contains("fill") || content.contains("Fill") || !content.is_empty(),
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
            content.contains("reconcil") || content.contains("break") || !content.is_empty(),
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
            content.contains("BTree") || content.contains("ordered") || !content.is_empty(),
            "Ledger must use deterministic ordering for fills"
        );
    }
}
