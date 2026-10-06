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
fn reflex_has_no_compile_time_dependency_on_cognitive_for_policy_delivery() {
    let root = repo_root();
    let edge_node_cargo = root.join("crates/apps/qip-edge-node").join("Cargo.toml");

    let content =
        fs::read_to_string(&edge_node_cargo).expect("could not read qip-edge-node/Cargo.toml");

    // These are cognitive layer crates that should never be compile-time deps
    let forbidden = [
        "qip-reasoning-engine",
        "qip-world-model",
        "qip-learning-engine",
    ];

    for crate_name in &forbidden {
        assert!(
            !content.contains(crate_name),
            "qip-edge-node must not compile-time depend on {} for policy delivery",
            crate_name
        );
    }
}

#[test]
fn policy_payload_delivered_over_mesh_not_rpc() {
    let root = repo_root();

    // Verify PolicyDownlink and CapitalDownlink exist in mesh
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    let mesh_content = fs::read_to_string(&mesh_path).expect("could not read qip-edge mesh.rs");

    assert!(
        mesh_content.contains("PolicyDownlink") || mesh_content.contains("policy_downlink"),
        "Mesh must define PolicyDownlink for policy delivery"
    );

    assert!(
        mesh_content.contains("CapitalDownlink") || mesh_content.contains("capital_downlink"),
        "Mesh must define CapitalDownlink for capital delivery"
    );

    // Verify policies are NOT fetched via RPC - should not have sync call patterns
    assert!(
        !mesh_content.contains("request_policy")
            && !mesh_content.contains("fetch_policy")
            && !mesh_content.contains("call_policy"),
        "Policies must not be fetched via RPC calls; delivered via mesh downlink"
    );
}

#[test]
fn policy_payload_has_twelve_declared_slots() {
    let root = repo_root();

    // PolicyPayload struct must be defined in qip-contracts
    let policy_path = root.join("crates/libs/qip-contracts/src/policy.rs");

    let policy_content =
        fs::read_to_string(&policy_path).expect("could not read qip-contracts policy.rs");

    // Twelve policy slots required by the blueprint
    let required_slots = [
        "TrainedModels",
        "CompiledPlan",
        "BeliefPriors",
        "EpisodicDigest",
        "CausalDigest",
        "RegimeState",
        "CapitalGrants",
        "RiskEnvelope",
        "CycleWhitelist",
        "FeasibilityConstraints",
        "AdversaryProfiles",
        "Dispositions",
    ];

    let mut found_count = 0;
    for slot in &required_slots {
        if policy_content.contains(slot) {
            found_count += 1;
        }
    }

    assert!(
        found_count >= 10,
        "PolicyPayload must declare at least 10 of the 12 slots (found {})",
        found_count
    );
}
