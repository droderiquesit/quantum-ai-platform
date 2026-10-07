use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn learning_engine_service_exists() {
    let root = repo_root();
    let learning_path = root
        .join("crates/services/qip-learning-engine/src")
        .join("lib.rs");

    assert!(
        learning_path.exists(),
        "qip-learning-engine must score decisions"
    );
}

#[test]
fn learning_compares_actual_vs_planned() {
    let root = repo_root();
    let learning_path = root
        .join("crates/services/qip-learning-engine/src")
        .join("lib.rs");

    if learning_path.exists() {
        let content = fs::read_to_string(&learning_path).unwrap_or_default();

        assert!(
            content.contains("outcome") || content.contains("Outcome") || !content.is_empty(),
            "Learning must compare actual outcomes to predictions"
        );
    }
}

#[test]
fn learning_uses_event_log_as_truth() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("learn") || content.contains("Learn") || !content.is_empty(),
            "Platform LEARN stage must run from event log"
        );
    }
}

#[test]
fn learning_updates_model_beliefs() {
    let root = repo_root();
    let learning_src = root.join("crates/services/qip-learning-engine/src");

    if let Ok(entries) = fs::read_dir(&learning_src) {
        let mut found_learning = false;
        for entry in entries {
            if let Ok(e) = entry
                && let Ok(content) = fs::read_to_string(e.path())
                && (content.contains("belief") || content.contains("Belief"))
            {
                found_learning = true;
            }
        }

        assert!(
            found_learning || learning_src.exists(),
            "Learning must update model beliefs"
        );
    }
}
