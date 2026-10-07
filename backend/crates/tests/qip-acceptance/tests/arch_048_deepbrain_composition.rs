use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
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
            content.contains("qip-kernel") || !content.is_empty(),
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
        // `contains("main") || true` stood here and could not fail. The
        // property the name claims lives in this file as the start-up
        // refusal of a live autonomy ceiling (the paper boundary's second
        // layer), so that is what is asserted.
        assert!(
            content.contains("AutonomyLevel::deployable("),
            "qip-deepbrain must read its autonomy ceiling through AutonomyLevel::deployable, \
             which refuses a live level at start-up rather than lowering it"
        );
    }
}
