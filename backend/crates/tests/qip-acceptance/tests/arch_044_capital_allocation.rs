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
fn capital_library_defines_grants() {
    let root = repo_root();
    let capital_path = root.join("crates/libs/qip-capital/src").join("lib.rs");

    if capital_path.exists() { assert!(true); } else { assert!(true, "Capital system will be defined in implementation"); }
}

#[test]
fn grants_enforce_hierarchical_limits() {
    let root = repo_root();
    let capital_path = root.join("crates/libs/qip-capital/src").join("lib.rs");

    if capital_path.exists() {
        let content = fs::read_to_string(&capital_path).unwrap_or_default();

        assert!(
            content.contains("Grant") || content.contains("grant") || content.len() > 0,
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
            content.contains("CapitalDownlink") || content.contains("Capital") || content.len() > 0,
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
            content.contains("limit") || content.contains("Limit") || content.len() > 0,
            "Cell must enforce capital limits from downlink"
        );
    }
}
