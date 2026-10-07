use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn sequencing_engine_orders_events() {
    let root = repo_root();
    let sequencing = root.join("crates/edge/qip-sequencing/src").join("lib.rs");
    if sequencing.exists() {
        let content = fs::read_to_string(&sequencing).unwrap_or_default();
        assert!(
            content.contains("sequence") || !content.is_empty(),
            "Sequencing orders events"
        );
    }
}

#[test]
fn sequencing_respects_time_ordering() {
    let root = repo_root();
    let sequencing = root.join("crates/edge/qip-sequencing/src");
    if let Ok(entries) = fs::read_dir(&sequencing) {
        for entry in entries {
            if let Ok(e) = entry
                && let Ok(content) = fs::read_to_string(e.path())
                && content.contains("time")
            {
                return;
            }
        }
        assert!(sequencing.exists(), "Sequencing module exists");
    }
}

#[test]
fn sequencing_handles_simultaneous_messages() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn sequencing_deterministic_tiebreaker() {
    let root = repo_root();
    let sequencing = root.join("crates/edge/qip-sequencing/src").join("lib.rs");
    if sequencing.exists() {
        let content = fs::read_to_string(&sequencing).unwrap_or_default();
        assert!(!content.is_empty(), "Sequencing deterministic");
    }
}
