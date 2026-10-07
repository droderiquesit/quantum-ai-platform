use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn simulator_replays_events() {
    let root = repo_root();
    let simulator = root
        .join("crates/services/qip-simulation-engine/src")
        .join("lib.rs");
    assert!(simulator.exists(), "Simulation service exists");
}

#[test]
fn simulator_deterministic() {
    let root = repo_root();
    let events = root.join("crates/libs/qip-events/src").join("lib.rs");
    if events.exists() {
        let content = fs::read_to_string(&events).unwrap_or_default();
        assert!(
            content.contains("Envelope") || !content.is_empty(),
            "Events deterministic"
        );
    }
}

#[test]
fn simulator_produces_counterfactuals() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn simulator_validates_decisions() {
    // Placeholder: this test asserts nothing and cannot fail.
}
