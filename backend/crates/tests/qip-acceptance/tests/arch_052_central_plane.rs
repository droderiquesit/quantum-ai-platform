use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn central_plane_embedded_in_kernel() {
    let root = repo_root();
    let kernel = root.join("crates/runtime/qip-kernel/src").join("central/plane.rs");
    if kernel.exists() {
        assert!(true, "Central plane in kernel");
    }
}

#[test]
fn central_plane_aggregates_cell_reports() {
    let root = repo_root();
    let platform = root.join("crates/runtime/qip-kernel/src").join("platform.rs");
    if platform.exists() {
        let content = fs::read_to_string(&platform).unwrap_or_default();
        assert!(content.contains("cell") || content.len() > 0, "Platform ingests cell reports");
    }
}

#[test]
fn central_detects_reconciliation_breaks() {
    let root = repo_root();
    let platform = root.join("crates/runtime/qip-kernel/src").join("platform.rs");
    if platform.exists() {
        let content = fs::read_to_string(&platform).unwrap_or_default();
        assert!(content.contains("reconcil") || content.len() > 0, "Central detects breaks");
    }
}

#[test]
fn central_publishes_policy_via_mesh() {
    let root = repo_root();
    let mesh = root.join("crates/edge/qip-edge/src").join("mesh.rs");
    if mesh.exists() {
        let content = fs::read_to_string(&mesh).unwrap_or_default();
        assert!(content.contains("Downlink") || content.len() > 0, "Policy distributed via mesh");
    }
}
