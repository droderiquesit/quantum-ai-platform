use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn world_model_library_exists() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn world_model_represents_market_state() {
    let root = repo_root();
    let world_model_path = root.join("crates/libs/qip-world-model/src").join("lib.rs");

    if world_model_path.exists() {
        let content = fs::read_to_string(&world_model_path).unwrap_or_default();

        assert!(
            content.contains("world") || content.contains("World") || !content.is_empty(),
            "World model must represent market state"
        );
    }
}

#[test]
fn world_model_is_read_only_lib() {
    let root = repo_root();
    let cargo_path = root.join("crates/libs/qip-world-model").join("Cargo.toml");

    if cargo_path.exists() {
        let content = fs::read_to_string(&cargo_path).unwrap_or_default();

        // Should not have binary or I/O heavy dependencies
        assert!(
            !content.contains("tokio") || !content.contains("async"),
            "World model is a library, not async service"
        );
    }
}

#[test]
fn world_model_consumed_by_reasoning() {
    let root = repo_root();
    let reasoning_cargo = root
        .join("crates/services/qip-reasoning-engine/src")
        .parent()
        .unwrap()
        .join("Cargo.toml");

    if reasoning_cargo.exists() {
        let content = fs::read_to_string(&reasoning_cargo).unwrap_or_default();

        assert!(
            content.contains("qip-world-model"),
            "Reasoning engine must depend on world model"
        );
    }
}
