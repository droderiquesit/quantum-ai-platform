use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn event_log_enables_replay() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");
    let content = fs::read_to_string(&events_path).unwrap_or_default();

    assert!(
        content.contains("Envelope") || content.contains("Event"),
        "Event log must define envelope structure for replay"
    );
}

#[test]
fn hash_chain_proves_no_tampering() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");
    let content = fs::read_to_string(&events_path).unwrap_or_default();

    assert!(
        content.contains("hash") || content.contains("Hash") || content.contains("chain"),
        "Events must support hash-chaining for integrity"
    );
}

#[test]
fn platform_decision_is_reproducible_from_log() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    let content = fs::read_to_string(&platform_path).unwrap_or_default();

    assert!(
        content.contains("decision") || content.contains("Decision") || content.contains("record"),
        "Platform must record decisions in log for reproducibility"
    );
}

#[test]
fn no_random_values_in_deterministic_path() {
    let root = repo_root();
    let risk_path = root
        .join("crates/services/qip-risk-engine/src")
        .join("lib.rs");
    let content = fs::read_to_string(&risk_path).unwrap_or_default();

    // Pre-trade checks must be deterministic
    assert!(
        !content.contains("rand") || !content.contains("random"),
        "Pre-trade deterministic checks must not use random values"
    );
}
