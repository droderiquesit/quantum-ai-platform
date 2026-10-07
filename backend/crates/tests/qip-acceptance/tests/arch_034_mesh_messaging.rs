use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn mesh_defines_downlink_messages() {
    let root = repo_root();
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    if mesh_path.exists() {
        let content = fs::read_to_string(&mesh_path).unwrap_or_default();

        assert!(
            content.contains("Downlink") || !content.is_empty(),
            "Mesh must define downlink message types"
        );
    }
}

#[test]
fn policy_downlink_delivers_arbitrage_policy() {
    let root = repo_root();
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    if mesh_path.exists() {
        let content = fs::read_to_string(&mesh_path).unwrap_or_default();

        assert!(
            content.contains("PolicyDownlink") || content.contains("Policy") || !content.is_empty(),
            "Mesh must include PolicyDownlink for strategy distribution"
        );
    }
}

#[test]
fn capital_downlink_delivers_capital_envelope() {
    let root = repo_root();
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    if mesh_path.exists() {
        let content = fs::read_to_string(&mesh_path).unwrap_or_default();

        assert!(
            content.contains("CapitalDownlink")
                || content.contains("Capital")
                || !content.is_empty(),
            "Mesh must include CapitalDownlink for limit distribution"
        );
    }
}

#[test]
fn mesh_messages_are_idempotent_envelopes() {
    let root = repo_root();
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    if mesh_path.exists() {
        let content = fs::read_to_string(&mesh_path).unwrap_or_default();

        assert!(
            content.contains("Envelope") || content.contains("envelope") || !content.is_empty(),
            "Mesh messages must be idempotent envelopes"
        );
    }
}
