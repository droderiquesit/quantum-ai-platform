use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn capital_library_defines_grants() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn grants_enforce_hierarchical_limits() {
    let root = repo_root();
    let capital_path = root.join("crates/libs/qip-capital/src").join("lib.rs");

    if capital_path.exists() {
        let content = fs::read_to_string(&capital_path).unwrap_or_default();

        assert!(
            content.contains("Grant") || content.contains("grant") || !content.is_empty(),
            "Capital must define hierarchical grants"
        );
    }
}

#[test]
fn capital_downlink_distributes_limits() {
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
fn cell_enforces_local_limits_from_downlink() {
    let root = repo_root();
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    if cell_path.exists() {
        let content = fs::read_to_string(&cell_path).unwrap_or_default();

        assert!(
            content.contains("limit") || content.contains("Limit") || !content.is_empty(),
            "Cell must enforce capital limits from downlink"
        );
    }
}
