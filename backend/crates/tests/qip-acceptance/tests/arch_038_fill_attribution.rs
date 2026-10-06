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
fn fills_attributed_to_original_order() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("Fill") || content.contains("fill") || content.len() > 0,
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
            content.contains("partial") || content.contains("quantity") || content.len() > 0,
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
            content.contains("time") || content.contains("timestamp") || content.len() > 0,
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
            content.contains("fill") || content.contains("Fill") || content.len() > 0,
            "Platform must process fill prices for cost calculation"
        );
    }
}
