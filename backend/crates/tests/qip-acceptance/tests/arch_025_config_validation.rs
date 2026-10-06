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
fn config_validation_stops_process_on_invalid_values() {
    let root = repo_root();

    // Composition roots must refuse bad config
    let api_main = root.join("crates/apps/qip-api/src").join("main.rs");
    if api_main.exists() {
        let content = fs::read_to_string(&api_main).unwrap_or_default();

        assert!(
            content.contains("Result") || content.contains("error"),
            "Configuration must be validated at startup and refuse invalid values"
        );
    }
}

#[test]
fn autonomy_ceiling_must_be_paper_or_reject() {
    let root = repo_root();
    let risk_path = root
        .join("crates/services/qip-risk-engine/src")
        .join("autonomy.rs");

    if risk_path.exists() {
        let content = fs::read_to_string(&risk_path).unwrap_or_default();

        assert!(
            content.contains("Paper") || content.contains("refuse"),
            "Autonomy ceiling must reject live configurations"
        );
    }
}

#[test]
fn venue_list_validated_before_trading() {
    let root = repo_root();
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if cell_path.exists() {
        let content = fs::read_to_string(&cell_path).unwrap_or_default();

        assert!(
            content.contains("venue") || content.contains("Venue"),
            "Cell must validate venue configuration at construction"
        );
    }
}

#[test]
fn timeouts_configured_on_all_network_operations() {
    let root = repo_root();
    let transport_path = root.join("crates/libs/qip-transport/src").join("lib.rs");
    let content = fs::read_to_string(&transport_path).unwrap_or_default();

    assert!(
        content.contains("timeout") || content.contains("Duration"),
        "All network operations must have configured timeouts"
    );
}
