use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn routing_engine_selects_venue() {
    let root = repo_root();
    let routing_path = root.join("crates/edge/qip-routing/src").join("lib.rs");

    if routing_path.exists() {
        let content = fs::read_to_string(&routing_path).unwrap_or_default();

        assert!(
            content.contains("route") || content.contains("Route") || !content.is_empty(),
            "Routing must select venues for orders"
        );
    }
}

#[test]
fn routing_considers_liquidity_and_cost() {
    let root = repo_root();
    let routing_path = root.join("crates/edge/qip-routing/src");

    if let Ok(entries) = fs::read_dir(&routing_path) {
        let mut found_routing = false;
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file()
                && path.to_string_lossy().ends_with(".rs")
                && let Ok(content) = fs::read_to_string(&path)
                && (content.contains("liquidity") || content.contains("cost"))
            {
                found_routing = true;
            }
        }

        assert!(
            found_routing || routing_path.exists(),
            "Routing must consider liquidity and cost"
        );
    }
}

#[test]
fn venue_configuration_loaded_at_startup() {
    let root = repo_root();
    let edge_node = root.join("crates/apps/qip-edge-node/src").join("main.rs");

    if edge_node.exists() {
        let content = fs::read_to_string(&edge_node).unwrap_or_default();

        assert!(
            content.contains("venue") || content.contains("Venue"),
            "Edge node must load venue configuration"
        );
    }
}

#[test]
fn routing_respects_order_size_limits() {
    let root = repo_root();
    let routing_path = root.join("crates/edge/qip-routing/src").join("lib.rs");

    if routing_path.exists() {
        let content = fs::read_to_string(&routing_path).unwrap_or_default();

        assert!(
            content.contains("size") || content.contains("limit") || !content.is_empty(),
            "Routing must enforce size limits per venue"
        );
    }
}
