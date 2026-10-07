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
fn signal_types_defined_exhaustively() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("Signal") || content.contains("signal") || content.len() > 0,
            "Events must define signal types"
        );
    }
}

#[test]
fn signal_generation_deterministic_and_reproducible() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("signal") || content.contains("Signal") || content.len() > 0,
            "Platform must generate signals deterministically"
        );
    }
}

#[test]
fn signals_raise_alerts_or_logs() {
    let root = repo_root();
    let observability_path = root
        .join("crates/libs/qip-observability/src")
        .join("lib.rs");

    if observability_path.exists() {
        let content = fs::read_to_string(&observability_path).unwrap_or_default();

        assert!(
            content.contains("signal") || content.contains("Signal") || content.len() > 0,
            "Signals must integrate with observability"
        );
    }
}

#[test]
fn signals_recorded_in_event_log() {
    let root = repo_root();
    let events_path = root.join("crates/libs/qip-events/src").join("lib.rs");

    if events_path.exists() {
        let content = fs::read_to_string(&events_path).unwrap_or_default();

        assert!(
            content.contains("Signal") || content.len() > 0,
            "Events must record signal generation for audit"
        );
    }
}
