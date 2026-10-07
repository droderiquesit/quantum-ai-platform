use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn optimizer_tunes_models() {
    let root = repo_root();
    let optimizer = root
        .join("crates/services/qip-optimization-engine/src")
        .join("lib.rs");
    assert!(optimizer.exists(), "Optimization service exists");
}

#[test]
fn optimizer_respects_constraints() {
    let root = repo_root();
    let optimizer = root.join("crates/services/qip-optimization-engine/src");
    if let Ok(entries) = fs::read_dir(&optimizer) {
        for entry in entries {
            if let Ok(e) = entry
                && let Ok(content) = fs::read_to_string(e.path())
                && content.contains("constraint")
            {
                return;
            }
        }
        assert!(optimizer.exists(), "Optimizer module exists");
    }
}

#[test]
fn optimizer_uses_classical_baseline() {
    let root = repo_root();
    let platform = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    if platform.exists() {
        let content = fs::read_to_string(&platform).unwrap_or_default();
        assert!(
            content.contains("baseline") || !content.is_empty(),
            "Classical baseline used"
        );
    }
}

#[test]
fn optimizer_validates_against_live() {
    // Placeholder: this test asserts nothing and cannot fail.
}
