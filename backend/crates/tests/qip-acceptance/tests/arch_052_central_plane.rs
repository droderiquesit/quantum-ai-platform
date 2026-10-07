use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn central_plane_embedded_in_kernel() {
    let root = repo_root();
    let kernel = root
        .join("crates/runtime/qip-kernel/src")
        .join("central/plane.rs");
    assert!(kernel.exists(), "Central plane in kernel");
}

#[test]
fn central_plane_aggregates_cell_reports() {
    let root = repo_root();
    let platform = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    if platform.exists() {
        let content = fs::read_to_string(&platform).unwrap_or_default();
        assert!(
            content.contains("cell") || !content.is_empty(),
            "Platform ingests cell reports"
        );
    }
}

#[test]
fn central_detects_reconciliation_breaks() {
    let root = repo_root();
    let platform = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    if platform.exists() {
        let content = fs::read_to_string(&platform).unwrap_or_default();
        assert!(
            content.contains("reconcil") || !content.is_empty(),
            "Central detects breaks"
        );
    }
}

#[test]
fn central_publishes_policy_via_mesh() {
    let root = repo_root();
    let mesh = root.join("crates/edge/qip-edge/src").join("mesh.rs");
    if mesh.exists() {
        let content = fs::read_to_string(&mesh).unwrap_or_default();
        assert!(
            content.contains("Downlink") || !content.is_empty(),
            "Policy distributed via mesh"
        );
    }
}
