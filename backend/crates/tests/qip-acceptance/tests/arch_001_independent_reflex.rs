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
fn reflex_edge_node_has_no_compile_time_dependency_on_cognitive_crates() {
    let root = repo_root();
    let edge_node_cargo = root.join("crates/apps/qip-edge-node").join("Cargo.toml");

    let content =
        fs::read_to_string(&edge_node_cargo).expect("could not read qip-edge-node/Cargo.toml");

    let forbidden_crates = [
        "qip-reasoning-engine",
        "qip-world-model",
        "qip-twin",
        "qip-evolution",
        "qip-training",
        "qip-learning-engine",
        "qip-optimization-engine",
        "qip-simulation-engine",
        "qip-deepbrain",
    ];

    for forbidden in &forbidden_crates {
        assert!(
            !content.contains(forbidden),
            "qip-edge-node must not depend on {} (cognitive layer)",
            forbidden
        );
    }
}

#[test]
fn reflex_and_cognitive_are_separate_deployable_binaries() {
    let root = repo_root();

    // Verify both binaries exist as separate Cargo projects
    let edge_node_main = root.join("crates/apps/qip-edge-node").join("src/main.rs");
    let deepbrain_main = root.join("crates/apps/qip-deepbrain").join("src/main.rs");

    assert!(edge_node_main.exists(), "qip-edge-node/src/main.rs exists");
    assert!(deepbrain_main.exists(), "qip-deepbrain/src/main.rs exists");

    // Verify separate Cargo.toml files
    let edge_node_cargo = root.join("crates/apps/qip-edge-node").join("Cargo.toml");
    let deepbrain_cargo = root.join("crates/apps/qip-deepbrain").join("Cargo.toml");

    assert!(edge_node_cargo.exists(), "qip-edge-node/Cargo.toml exists");
    assert!(deepbrain_cargo.exists(), "qip-deepbrain/Cargo.toml exists");

    // Parse Cargo.toml to verify [[bin]] declarations
    let edge_node_content =
        fs::read_to_string(&edge_node_cargo).expect("could not read qip-edge-node/Cargo.toml");
    assert!(
        edge_node_content.contains(r#"name = "qip-edge-node""#)
            || edge_node_content.contains(r#"name='qip-edge-node'"#),
        "qip-edge-node declares its binary name"
    );

    let deepbrain_content =
        fs::read_to_string(&deepbrain_cargo).expect("could not read qip-deepbrain/Cargo.toml");
    assert!(
        deepbrain_content.contains(r#"name = "qip-deepbrain""#)
            || deepbrain_content.contains(r#"name='qip-deepbrain'"#),
        "qip-deepbrain declares its binary name"
    );
}
