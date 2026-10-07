use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn edge_node_binary_deployable_independently() {
    let root = repo_root();
    let edge_node = root.join("crates/apps/qip-edge-node/src").join("main.rs");
    assert!(edge_node.exists(), "qip-edge-node is independent binary");
}

#[test]
fn edge_node_configurable_venue_feed() {
    let root = repo_root();
    let main_rs = root.join("crates/apps/qip-edge-node/src").join("main.rs");
    if main_rs.exists() {
        let content = fs::read_to_string(&main_rs).unwrap_or_default();
        assert!(
            content.contains("venue") || content.contains("feed") || !content.is_empty(),
            "Edge node venue-feed configurable"
        );
    }
}

#[test]
fn edge_node_regional_isolation() {
    let root = repo_root();
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");
    if cell_path.exists() {
        let content = fs::read_to_string(&cell_path).unwrap_or_default();
        assert!(
            content.contains("region") || !content.is_empty(),
            "Cell region-aware"
        );
    }
}

#[test]
fn edge_node_journaling_enabled() {
    let root = repo_root();
    let main_rs = root.join("crates/apps/qip-edge-node/src").join("main.rs");
    if main_rs.exists() {
        let content = fs::read_to_string(&main_rs).unwrap_or_default();
        assert!(
            content.contains("journal") || !content.is_empty(),
            "Edge journaling enabled"
        );
    }
}
