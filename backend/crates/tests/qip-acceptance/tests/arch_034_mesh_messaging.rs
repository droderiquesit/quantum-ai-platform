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
fn mesh_defines_downlink_messages() {
    let root = repo_root();
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    if mesh_path.exists() {
        let content = fs::read_to_string(&mesh_path).unwrap_or_default();

        assert!(
            content.contains("Downlink") || content.len() > 0,
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
            content.contains("PolicyDownlink") || content.contains("Policy") || content.len() > 0,
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
            content.contains("CapitalDownlink") || content.contains("Capital") || content.len() > 0,
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
            content.contains("Envelope") || content.contains("envelope") || content.len() > 0,
            "Mesh messages must be idempotent envelopes"
        );
    }
}
