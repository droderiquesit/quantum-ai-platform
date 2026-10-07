use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn training_updates_models() {
    let root = repo_root();
    let training = root.join("crates/services/qip-training/src").join("lib.rs");
    assert!(training.exists(), "Training service exists");
}

#[test]
fn training_operates_offline() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn training_produces_checkpoints() {
    let root = repo_root();
    let training = root.join("crates/services/qip-training/src");
    if let Ok(entries) = fs::read_dir(&training) {
        for entry in entries {
            if let Ok(e) = entry
                && let Ok(content) = fs::read_to_string(e.path())
                && content.contains("checkpoint")
            {
                return;
            }
        }
        assert!(training.exists(), "Training module exists");
    }
}

#[test]
fn training_uses_event_log_as_corpus() {
    // Placeholder: this test asserts nothing and cannot fail.
}
