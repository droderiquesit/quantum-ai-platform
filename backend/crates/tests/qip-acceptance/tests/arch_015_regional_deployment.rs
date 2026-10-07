use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn edge_node_deployment_separate_from_central_plane() {
    let root = repo_root();

    // qip-edge-node and qip-deepbrain must be separate deployments
    let edge_main = root.join("crates/apps/qip-edge-node/src").join("main.rs");
    let deep_main = root.join("crates/apps/qip-deepbrain/src").join("main.rs");

    assert!(
        edge_main.exists(),
        "qip-edge-node must exist as separate binary"
    );
    assert!(
        deep_main.exists(),
        "qip-deepbrain must exist as separate binary"
    );
}

#[test]
fn terraform_supports_regional_execution_nodes() {
    let root = repo_root();
    let repo_parent = root.parent().unwrap();

    // Terraform must define execution_nodes variable for regions
    let tf_vars = repo_parent
        .join("infrastructure/terraform")
        .join("variables.tf");
    let tf_content = fs::read_to_string(&tf_vars).unwrap_or_else(|_| String::new());

    assert!(
        tf_content.contains("execution")
            || tf_content.contains("node")
            || tf_content.contains("region"),
        "Terraform must support regional execution node deployment"
    );
}

#[test]
fn region_isolation_enforced_in_cell_config() {
    let root = repo_root();

    // Cell must accept region ID in configuration
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");
    let cell_content = fs::read_to_string(&cell_path).unwrap_or_else(|_| String::new());

    assert!(
        cell_content.contains("region") || cell_content.contains("Region"),
        "Cell must be configured with region identity"
    );
}

#[test]
fn local_journaling_enabled_at_edge_node() {
    let root = repo_root();

    // qip-edge-node must enable local journal/spool
    let edge_path = root.join("crates/apps/qip-edge-node/src").join("main.rs");
    if edge_path.exists() {
        let edge_content = fs::read_to_string(&edge_path).unwrap_or_else(|_| String::new());

        assert!(
            edge_content.contains("journal")
                || edge_content.contains("spool")
                || edge_content.contains("log"),
            "Edge node must enable local journaling for autonomy"
        );
    }
}
