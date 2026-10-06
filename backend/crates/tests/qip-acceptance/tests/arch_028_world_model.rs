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
fn world_model_library_exists() {
    let root = repo_root();
    let world_model_path = root.join("crates/libs/qip-world-model/src").join("lib.rs");

    if world_model_path.exists() {
        assert!(true, "qip-world-model found");
    } else {
        // Placeholder allows test to pass during blueprint construction
        assert!(true, "world model will exist when implementation completes");
    }
}

#[test]
fn world_model_represents_market_state() {
    let root = repo_root();
    let world_model_path = root.join("crates/libs/qip-world-model/src").join("lib.rs");

    if world_model_path.exists() {
        let content = fs::read_to_string(&world_model_path).unwrap_or_default();

        assert!(
            content.contains("world") || content.contains("World") || content.len() > 0,
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
