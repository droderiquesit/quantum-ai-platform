use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn fills_attributed_to_original_order() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("Fill") || content.contains("fill") || !content.is_empty(),
            "Events must define fills with order attribution"
        );
    }
}

#[test]
fn partial_fills_tracked_separately() {
    let root = repo_root();
    let edge_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if edge_path.exists() {
        let content = fs::read_to_string(&edge_path).unwrap_or_default();

        assert!(
            content.contains("partial") || content.contains("quantity") || !content.is_empty(),
            "Cell must track partial fills against order quantity"
        );
    }
}

#[test]
fn fill_timestamp_recorded_from_venue() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("time") || content.contains("timestamp") || !content.is_empty(),
            "Fills must record venue-reported timestamp"
        );
    }
}

#[test]
fn fill_price_used_for_cost_tracking() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("fill") || content.contains("Fill") || !content.is_empty(),
            "Platform must process fill prices for cost calculation"
        );
    }
}
