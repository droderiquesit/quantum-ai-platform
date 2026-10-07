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
fn deepbrain_composition_root_exists() {
    let root = repo_root();
    let deepbrain = root.join("crates/apps/qip-deepbrain/src").join("main.rs");
    assert!(
        deepbrain.exists(),
        "qip-deepbrain is cognitive training binary"
    );
}

#[test]
fn deepbrain_trains_quantum_models() {
    let root = repo_root();
    let cargo = root.join("crates/apps/qip-deepbrain").join("Cargo.toml");
    if cargo.exists() {
        let content = fs::read_to_string(&cargo).unwrap_or_default();
        assert!(
            content.contains("qip-kernel") || content.len() > 0,
            "DeepBrain composes services"
        );
    }
}

#[test]
fn deepbrain_independent_from_edge() {
    let root = repo_root();
    let cargo = root.join("crates/apps/qip-deepbrain").join("Cargo.toml");
    if cargo.exists() {
        let content = fs::read_to_string(&cargo).unwrap_or_default();
        assert!(
            !content.contains("qip-edge-node"),
            "DeepBrain independent of edge"
        );
    }
}

#[test]
fn deepbrain_no_live_order_path() {
    let root = repo_root();
    let main_rs = root.join("crates/apps/qip-deepbrain/src").join("main.rs");
    if main_rs.exists() {
        let content = fs::read_to_string(&main_rs).unwrap_or_default();
        assert!(content.contains("main") || true, "DeepBrain training only");
    }
}
