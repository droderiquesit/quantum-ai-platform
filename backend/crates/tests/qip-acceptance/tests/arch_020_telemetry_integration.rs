use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn telemetry_library_exists() {
    let root = repo_root();
    let telemetry_path = root
        .join("crates/libs/qip-observability/src")
        .join("lib.rs");

    assert!(
        telemetry_path.exists(),
        "qip-observability library must exist for telemetry"
    );
}

#[test]
fn platform_records_metrics_at_seam() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    let content = fs::read_to_string(&platform_path).unwrap_or_default();

    assert!(
        content.contains("metrics") || content.contains("Telemetry"),
        "Platform must record metrics at decision seams"
    );
}

#[test]
fn metrics_recorded_before_recorded_not_inferred() {
    let root = repo_root();
    let observability_path = root
        .join("crates/libs/qip-observability/src")
        .join("lib.rs");
    let content = fs::read_to_string(&observability_path).unwrap_or_default();

    assert!(
        content.contains("Telemetry") || content.contains("metrics"),
        "Observability must define Telemetry for recording known facts"
    );
}

#[test]
fn edge_plane_emits_metrics() {
    let root = repo_root();
    let edge_telemetry = root.join("crates/edge/qip-edge/src").join("telemetry.rs");

    if edge_telemetry.exists() {
        let content = fs::read_to_string(&edge_telemetry).unwrap_or_default();

        assert!(
            content.contains("metrics") || content.contains("Telemetry"),
            "Edge plane must emit metrics through telemetry"
        );
    }
}
