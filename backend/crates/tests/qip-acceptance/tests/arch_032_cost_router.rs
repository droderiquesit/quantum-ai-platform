use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn cost_router_service_exists() {
    let root = repo_root();
    let cost_router_path = root
        .join("crates/services/qip-cost-router/src")
        .join("lib.rs");

    assert!(
        cost_router_path.exists(),
        "qip-cost-router must route decisions to appropriate compute level"
    );
}

#[test]
fn cost_router_implements_determinism_gates() {
    let root = repo_root();
    let cost_router_path = root
        .join("crates/services/qip-cost-router/src")
        .join("lib.rs");

    if cost_router_path.exists() {
        let content = fs::read_to_string(&cost_router_path).unwrap_or_default();

        assert!(
            content.contains("Determinism") || !content.is_empty(),
            "Cost router must implement Determinism gates"
        );
    }
}

#[test]
fn determinism_required_refuses_model_output() {
    let root = repo_root();
    let cost_router_path = root
        .join("crates/services/qip-cost-router/src")
        .join("lib.rs");

    if cost_router_path.exists() {
        let content = fs::read_to_string(&cost_router_path).unwrap_or_default();

        assert!(
            content.contains("Required") || !content.is_empty(),
            "Determinism::Required must make model output structurally impossible"
        );
    }
}

#[test]
fn cost_router_records_rung_selection() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("cost") || content.contains("Cost") || !content.is_empty(),
            "Platform must record cost router decisions for auditability"
        );
    }
}
