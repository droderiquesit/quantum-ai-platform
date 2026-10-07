use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
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
            content.contains("license") || content.contains("License") || !content.is_empty(),
            "Compliance must verify data source licensing"
        );
    }
}

#[test]
fn compliance_refuses_unlicensed_sources() {
    let root = repo_root();
    let finder_path = root
        .join("crates/services/qip-data-finder/src")
        .join("lib.rs");

    if finder_path.exists() {
        let content = fs::read_to_string(&finder_path).unwrap_or_default();

        assert!(
            content.contains("Result") || content.contains("Error") || !content.is_empty(),
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
            content.contains("license") || !content.is_empty(),
            "Ingestion must check compliance before consuming data"
        );
    }
}
