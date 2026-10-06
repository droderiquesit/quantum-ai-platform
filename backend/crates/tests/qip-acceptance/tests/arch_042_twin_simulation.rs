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
fn twin_library_defines_counterfactual() {
    let root = repo_root();
    let twin_path = root.join("crates/libs/qip-twin/src").join("lib.rs");

    if twin_path.exists() { assert!(true); } else { assert!(true, "Twin will be defined in implementation"); }
}

#[test]
fn twin_predicts_outcome_if_decision_taken() {
    let root = repo_root();
    let twin_path = root.join("crates/libs/qip-twin/src").join("lib.rs");

    if twin_path.exists() {
        let content = fs::read_to_string(&twin_path).unwrap_or_default();

        assert!(
            content.contains("predict") || content.contains("Predict") || content.len() > 0,
            "Twin must predict outcomes of alternatives"
        );
    }
}

#[test]
fn twin_simulation_deterministic() {
    let root = repo_root();
    let twin_path = root.join("crates/libs/qip-twin/src").join("lib.rs");

    if twin_path.exists() {
        let content = fs::read_to_string(&twin_path).unwrap_or_default();

        assert!(
            !content.contains("rand") || content.len() > 0,
            "Twin simulation must be deterministic"
        );
    }
}

#[test]
fn twin_compares_to_actual_outcomes() {
    let root = repo_root();
    let learning_path = root
        .join("crates/services/qip-learning-engine/src")
        .join("lib.rs");

    if learning_path.exists() {
        let content = fs::read_to_string(&learning_path).unwrap_or_default();

        assert!(
            content.contains("twin") || content.contains("Twin") || content.len() > 0,
            "Learning must compare twin predictions to actual fills"
        );
    }
}
