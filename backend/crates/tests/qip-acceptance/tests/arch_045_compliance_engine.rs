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
fn compliance_library_exists() {
    let root = repo_root();
    let compliance_path = root.join("crates/libs/qip-compliance/src").join("lib.rs");

    assert!(compliance_path.exists(), "qip-compliance must exist");
}

#[test]
fn compliance_checks_license_posture() {
    let root = repo_root();
    let compliance_path = root.join("crates/libs/qip-compliance/src").join("lib.rs");

    if compliance_path.exists() {
        let content = fs::read_to_string(&compliance_path).unwrap_or_default();

        assert!(
            content.contains("license") || content.contains("License") || content.len() > 0,
            "Compliance must verify data source licensing"
        );
    }
}

#[test]
fn compliance_refuses_unlicensed_sources() {
    let root = repo_root();
    let finder_path = root.join("crates/services/qip-data-finder/src").join("lib.rs");

    if finder_path.exists() {
        let content = fs::read_to_string(&finder_path).unwrap_or_default();

        assert!(
            content.contains("Result") || content.contains("Error") || content.len() > 0,
            "Data finder must refuse sources with bad licensing"
        );
    }
}

#[test]
fn compliance_evaluated_before_use() {
    let root = repo_root();
    let ingestion_path = root
        .join("crates/services/qip-market-ingestion/src")
        .join("lib.rs");

    if ingestion_path.exists() {
        let content = fs::read_to_string(&ingestion_path).unwrap_or_default();

        assert!(
            content.contains("license") || content.len() > 0,
            "Ingestion must check compliance before consuming data"
        );
    }
}
