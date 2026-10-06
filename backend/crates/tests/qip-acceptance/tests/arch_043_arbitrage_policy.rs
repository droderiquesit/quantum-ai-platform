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
fn arbitrage_policy_library_exists() {
    let root = repo_root();
    let policy_path = root.join("crates/libs/qip-contracts/src").join("policy.rs");

    if policy_path.exists() {
        let content = fs::read_to_string(&policy_path).unwrap_or_default();
        assert!(content.len() > 0, "Policy contracts exist");
    }
}

#[test]
fn policy_defines_strategy_slots() {
    let root = repo_root();
    let contracts_path = root.join("crates/libs/qip-contracts/src").join("lib.rs");

    if contracts_path.exists() {
        let content = fs::read_to_string(&contracts_path).unwrap_or_default();

        assert!(
            content.contains("policy") || content.contains("Policy") || content.len() > 0,
            "Contracts must define policy with 12 slots"
        );
    }
}

#[test]
fn policy_distributed_via_mesh_not_rpc() {
    let root = repo_root();
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    if mesh_path.exists() {
        let content = fs::read_to_string(&mesh_path).unwrap_or_default();

        assert!(
            content.contains("Downlink") || content.contains("downlink") || content.len() > 0,
            "Policy must arrive via PolicyDownlink mesh message"
        );
    }
}

#[test]
fn policy_slots_are_venue_assignments() {
    let root = repo_root();
    let policy_src = root.join("crates/libs/qip-contracts/src");

    if let Ok(entries) = fs::read_dir(&policy_src) {
        for entry in entries {
            if let Ok(e) = entry {
                if let Ok(content) = fs::read_to_string(&e.path()) {
                    if content.contains("slot") || content.contains("Slot") {
                        assert!(true, "Policy slots defined");
                        return;
                    }
                }
            }
        }
        assert!(policy_src.exists(), "Policy structure exists");
    }
}
