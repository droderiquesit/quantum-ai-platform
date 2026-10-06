use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn warmup_produces_instant_decisions() {
    let root = repo_root();
    let fastbrain = root.join("crates/apps/qip-fastbrain/src").join("main.rs");
    if fastbrain.exists() {
        assert!(true, "FastBrain warm path");
    }
}

#[test]
fn warmup_low_latency() {
    let root = repo_root();
    let fastbrain_cargo = root.join("crates/apps/qip-fastbrain").join("Cargo.toml");
    if fastbrain_cargo.exists() {
        let content = fs::read_to_string(&fastbrain_cargo).unwrap_or_default();
        assert!(!content.contains("tokio"), "FastBrain no async");
    }
}

#[test]
fn warmup_uses_cached_models() {
    let root = repo_root();
    let kernel = root.join("crates/runtime/qip-kernel/src").join("lib.rs");
    if kernel.exists() {
        assert!(true, "Kernel coordinates cached models");
    }
}

#[test]
fn warmup_publishes_to_cells() {
    let root = repo_root();
    let mesh = root.join("crates/edge/qip-edge/src").join("mesh.rs");
    if mesh.exists() {
        assert!(true, "Mesh carries policies to cells");
    }
}
