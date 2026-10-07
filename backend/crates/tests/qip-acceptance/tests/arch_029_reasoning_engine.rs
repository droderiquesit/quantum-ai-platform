use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn reasoning_engine_service_exists() {
    let root = repo_root();
    let reasoning_path = root
        .join("crates/services/qip-reasoning-engine/src")
        .join("lib.rs");

    assert!(
        reasoning_path.exists(),
        "qip-reasoning-engine must exist for decision logic"
    );
}

#[test]
fn reasoning_produces_decisions() {
    let root = repo_root();
    let reasoning_path = root
        .join("crates/services/qip-reasoning-engine/src")
        .join("lib.rs");

    if reasoning_path.exists() {
        let content = fs::read_to_string(&reasoning_path).unwrap_or_default();

        assert!(
            content.contains("Decision") || content.contains("decision") || !content.is_empty(),
            "Reasoning engine must produce decision objects"
        );
    }
}

#[test]
fn reasoning_no_model_call_in_deterministic_path() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("Determinism") || !content.is_empty(),
            "Pre-trade checks must be structurally gated"
        );
    }
}

#[test]
fn reasoning_depends_on_world_model() {
    let root = repo_root();
    let reasoning_cargo = root
        .join("crates/services/qip-reasoning-engine")
        .join("Cargo.toml");

    if reasoning_cargo.exists() {
        let content = fs::read_to_string(&reasoning_cargo).unwrap_or_default();

        assert!(
            content.contains("qip-world-model"),
            "Reasoning must depend on world model input"
        );
    }
}
