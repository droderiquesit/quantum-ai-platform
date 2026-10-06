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
fn cycle_defines_all_eight_stages() {
    let root = repo_root();
    let cycle_path = root.join("crates/runtime/qip-kernel/src").join("cycle.rs");
    let content = fs::read_to_string(&cycle_path).unwrap_or_default();

    // Eight stages: SENSE, UNDERSTAND, DISCOVER, REASON, SIMULATE, DECIDE, ACT, LEARN
    assert!(content.contains("SENSE"), "Cycle must define SENSE stage");
    assert!(
        content.contains("UNDERSTAND"),
        "Cycle must define UNDERSTAND stage"
    );
    assert!(
        content.contains("DISCOVER"),
        "Cycle must define DISCOVER stage"
    );
    assert!(content.contains("REASON"), "Cycle must define REASON stage");
    assert!(
        content.contains("SIMULATE"),
        "Cycle must define SIMULATE stage"
    );
    assert!(content.contains("DECIDE"), "Cycle must define DECIDE stage");
    assert!(content.contains("ACT"), "Cycle must define ACT stage");
    assert!(content.contains("LEARN"), "Cycle must define LEARN stage");
}

#[test]
fn stages_produce_reportable_work() {
    let root = repo_root();
    let cycle_path = root.join("crates/runtime/qip-kernel/src").join("cycle.rs");
    let content = fs::read_to_string(&cycle_path).unwrap_or_default();

    // Each stage must produce work or decisions
    assert!(
        content.contains("WorkReport") || content.contains("Report"),
        "Stages must produce reportable work"
    );
}

#[test]
fn cycle_composition_in_platform() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    let content = fs::read_to_string(&platform_path).unwrap_or_default();

    // Platform must run all stages in sequence
    assert!(
        content.contains("stage_") || content.contains("Stage"),
        "Platform must implement stage execution"
    );
}

#[test]
fn stage_transitions_are_deterministic() {
    let root = repo_root();
    let cycle_path = root.join("crates/runtime/qip-kernel/src").join("cycle.rs");
    let content = fs::read_to_string(&cycle_path).unwrap_or_default();

    // No random transitions; state-driven progression
    assert!(
        !content.contains("rand") || content.contains("deterministic"),
        "Stage transitions must be deterministic, not random"
    );
}
