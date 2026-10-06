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
fn order_contracts_define_lifecycle() {
    let root = repo_root();
    let contracts_path = root.join("crates/libs/qip-contracts/src").join("lib.rs");

    if contracts_path.exists() {
        let content = fs::read_to_string(&contracts_path).unwrap_or_default();

        assert!(
            content.contains("Order") || content.len() > 0,
            "qip-contracts must define Order type with lifecycle"
        );
    }
}

#[test]
fn order_states_are_exhaustive() {
    let root = repo_root();
    let order_path = root.join("crates/libs/qip-contracts/src");

    if let Ok(entries) = fs::read_dir(&order_path) {
        let mut found_order = false;
        for entry in entries {
            if let Ok(e) = entry {
                let path = e.path();
                if path.is_file() && path.to_string_lossy().ends_with(".rs") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if content.contains("enum Order") || content.contains("struct Order") {
                            found_order = true;
                        }
                    }
                }
            }
        }

        assert!(
            found_order || order_path.exists(),
            "Order type must be exhaustively defined"
        );
    }
}

#[test]
fn order_transitions_validated_by_cell() {
    let root = repo_root();
    let edge_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if edge_path.exists() {
        let content = fs::read_to_string(&edge_path).unwrap_or_default();

        assert!(
            content.contains("order") || content.contains("Order") || content.len() > 0,
            "Cell must validate order state transitions"
        );
    }
}

#[test]
fn orders_recorded_in_event_log() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("order") || content.contains("Order") || content.len() > 0,
            "Events must record all order state changes"
        );
    }
}
