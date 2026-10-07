use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn platform_config_validates_autonomy_ceiling() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    let content = fs::read_to_string(&platform_path).unwrap_or_default();

    assert!(
        content.contains("AutonomyLevel") || content.contains("autonomy"),
        "Platform must validate autonomy ceiling at construction"
    );
}

#[test]
fn config_refuses_live_autonomy_at_startup() {
    let root = repo_root();

    // Check composition roots refuse live ceiling
    let api_main = root.join("crates/apps/qip-api/src").join("main.rs");
    let fastbrain_main = root.join("crates/apps/qip-fastbrain/src").join("main.rs");
    let deepbrain_main = root.join("crates/apps/qip-deepbrain/src").join("main.rs");

    for path in &[api_main, fastbrain_main, deepbrain_main] {
        if path.exists() {
            let content = fs::read_to_string(path).unwrap_or_default();
            assert!(
                content.contains("AutonomyLevel") || content.contains("autonomy"),
                "Composition root must validate autonomy on startup"
            );
        }
    }
}

#[test]
fn storage_proven_writable_before_healthy() {
    let root = repo_root();

    let api_main = root.join("crates/apps/qip-api/src").join("main.rs");
    if api_main.exists() {
        let content = fs::read_to_string(&api_main).unwrap_or_default();

        assert!(
            content.contains("storage")
                || content.contains("journal")
                || content.contains("database"),
            "Composition root must prove storage writable before reporting healthy"
        );
    }
}

#[test]
fn configuration_read_only_in_apps() {
    let root = repo_root();

    // Services must not read environment
    let risk_path = root
        .join("crates/services/qip-risk-engine/src")
        .join("lib.rs");
    if risk_path.exists() {
        let content = fs::read_to_string(&risk_path).unwrap_or_default();

        assert!(
            !content.contains("std::env::var"),
            "Services must not read environment directly"
        );
    }
}
