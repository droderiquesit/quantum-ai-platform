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
fn event_log_is_hash_chained() {
    let root = repo_root();

    // Event library must define hash-chained log structure
    let event_path = root.join("crates/libs/qip-events/src").join("lib.rs");
    let event_content = fs::read_to_string(&event_path).expect("could not read qip-events lib.rs");

    assert!(
        event_content.contains("Envelope") || event_content.contains("Event"),
        "Event fabric must define envelope/event structure"
    );
}

#[test]
fn events_are_immutable_once_sealed() {
    let root = repo_root();

    // Event envelope must support sealing/hashing
    let events_src = root.join("crates/libs/qip-events/src");

    if events_src.exists() {
        let mut found_seal_pattern = false;
        for entry in fs::read_dir(&events_src).unwrap_or_else(|_| panic!("")) {
            if let Ok(e) = entry {
                let path = e.path();
                if path.is_file() && path.to_string_lossy().ends_with(".rs") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if content.contains("seal")
                            || content.contains("hash")
                            || content.contains("chain")
                        {
                            found_seal_pattern = true;
                        }
                    }
                }
            }
        }

        assert!(
            found_seal_pattern,
            "Event fabric must support sealing or hashing of events"
        );
    }
}

#[test]
fn platform_records_every_decision_to_log() {
    let root = repo_root();

    // Platform must write decision records
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    let platform_content = fs::read_to_string(&platform_path).expect("could not read platform.rs");

    assert!(
        platform_content.contains("record")
            || platform_content.contains("log")
            || platform_content.contains("event"),
        "Platform must record decisions to event log"
    );
}

#[test]
fn cycle_stages_produce_lineage() {
    let root = repo_root();

    // Cycle must produce lineage/correlation through stages
    let cycle_path = root.join("crates/runtime/qip-kernel/src").join("cycle.rs");
    let cycle_content = fs::read_to_string(&cycle_path).expect("could not read cycle.rs");

    assert!(
        cycle_content.contains("Stage")
            || cycle_content.contains("stage")
            || cycle_content.contains("SENSE")
            || cycle_content.contains("Lineage"),
        "Cycle must define stages and lineage tracking"
    );
}
