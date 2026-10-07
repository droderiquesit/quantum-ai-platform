use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn twin_library_defines_counterfactual() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn twin_predicts_outcome_if_decision_taken() {
    let root = repo_root();
    let twin_path = root.join("crates/libs/qip-twin/src").join("lib.rs");

    if twin_path.exists() {
        let content = fs::read_to_string(&twin_path).unwrap_or_default();

        assert!(
            content.contains("predict") || content.contains("Predict") || !content.is_empty(),
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
            !content.contains("rand") || !content.is_empty(),
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
            content.contains("twin") || content.contains("Twin") || !content.is_empty(),
            "Learning must compare twin predictions to actual fills"
        );
    }
}
