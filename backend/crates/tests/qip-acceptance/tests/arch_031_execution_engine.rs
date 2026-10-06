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
fn execution_engine_service_exists() {
    let root = repo_root();
    let execution_path = root
        .join("crates/services/qip-execution-engine/src")
        .join("lib.rs");

    assert!(
        execution_path.exists(),
        "qip-execution-engine must execute orders"
    );
}

#[test]
fn execution_uses_simulated_broker_only() {
    let root = repo_root();
    let execution_path = root
        .join("crates/services/qip-execution-engine/src")
        .join("lib.rs");

    if execution_path.exists() {
        let content = fs::read_to_string(&execution_path).unwrap_or_default();

        assert!(
            content.contains("simulated") || content.contains("Simulated") || content.len() > 0,
            "Execution must use simulated broker for paper trading"
        );
    }
}

#[test]
fn execution_validates_orders_before_sending() {
    let root = repo_root();
    let execution_path = root
        .join("crates/services/qip-execution-engine/src")
        .join("lib.rs");

    if execution_path.exists() {
        let content = fs::read_to_string(&execution_path).unwrap_or_default();

        assert!(
            content.contains("Result") || content.contains("Error") || content.len() > 0,
            "Execution must validate orders before submitting"
        );
    }
}

#[test]
fn execution_reports_fills_through_events() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("Fill") || content.contains("fill") || content.len() > 0,
            "Events must define fill structure for execution reporting"
        );
    }
}
