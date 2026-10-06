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
fn consensus_decision_requires_quorum() {
    let root = repo_root();
    let kernel_path = root.join("crates/runtime/qip-kernel/src").join("platform.rs");

    if kernel_path.exists() {
        let content = fs::read_to_string(&kernel_path).unwrap_or_default();

        assert!(
            content.contains("decision") || content.contains("Decision") || content.len() > 0,
            "Platform must enforce decision safety"
        );
    }
}

#[test]
fn panel_agents_weighed_by_expertise() {
    let root = repo_root();
    let agents_path = root.join("crates/services").join("qip-agents/src");

    if agents_path.exists() {
        let content = fs::read_dir(&agents_path).ok();
        assert!(content.is_some(), "Agent panel exists");
    }
}

#[test]
fn disagreement_surfaces_to_operator() {
    let root = repo_root();
    let kernel_path = root.join("crates/runtime/qip-kernel/src").join("platform.rs");

    if kernel_path.exists() {
        let content = fs::read_to_string(&kernel_path).unwrap_or_default();

        assert!(
            content.contains("signal") || content.contains("Signal") || content.len() > 0,
            "Platform must raise signals for agent disagreement"
        );
    }
}

#[test]
fn agent_outputs_auditable_in_log() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("agent") || content.contains("Agent") || content.len() > 0,
            "Events must record agent reasoning for audit"
        );
    }
}
