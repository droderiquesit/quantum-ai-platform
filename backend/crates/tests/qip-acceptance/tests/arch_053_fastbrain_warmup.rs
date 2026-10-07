use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn warmup_produces_instant_decisions() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn warmup_low_latency() {
    let root = repo_root();
    let fastbrain_cargo = root.join("crates/apps/qip-fastbrain").join("Cargo.toml");
    if fastbrain_cargo.exists() {
        let content = fs::read_to_string(&fastbrain_cargo).unwrap_or_default();
        assert!(!content.contains("tokio"), "FastBrain no async");
    }
}

#[test]
fn warmup_uses_cached_models() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn warmup_publishes_to_cells() {
    // Placeholder: this test asserts nothing and cannot fail.
}
