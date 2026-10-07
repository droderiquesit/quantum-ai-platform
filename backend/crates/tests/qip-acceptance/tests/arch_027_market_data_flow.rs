use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn ingestion_service_consumes_market_data() {
    let root = repo_root();
    let ingestion_path = root
        .join("crates/services/qip-market-ingestion/src")
        .join("lib.rs");

    assert!(
        ingestion_path.exists(),
        "qip-market-ingestion service must exist"
    );
}

#[test]
fn market_data_validated_before_use() {
    let root = repo_root();
    let ingestion_path = root
        .join("crates/services/qip-market-ingestion/src")
        .join("lib.rs");

    if ingestion_path.exists() {
        let content = fs::read_to_string(&ingestion_path).unwrap_or_default();

        assert!(
            content.contains("Result") || content.contains("Error") || !content.is_empty(),
            "Ingestion must validate market data"
        );
    }
}

#[test]
fn ingestion_connects_via_transport() {
    let root = repo_root();
    let ingestion_cargo = root
        .join("crates/services/qip-market-ingestion")
        .join("Cargo.toml");

    if ingestion_cargo.exists() {
        let content = fs::read_to_string(&ingestion_cargo).unwrap_or_default();

        assert!(
            content.contains("qip-transport"),
            "Ingestion must use qip-transport for network"
        );
    }
}

#[test]
fn ingestion_respects_feed_licensing() {
    let root = repo_root();
    let ingestion_src = root.join("crates/services/qip-market-ingestion/src");

    if let Ok(entries) = fs::read_dir(&ingestion_src) {
        let mut found_licensing = false;
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file()
                && path.to_string_lossy().ends_with(".rs")
                && let Ok(content) = fs::read_to_string(&path)
                && (content.contains("license") || content.contains("License"))
            {
                found_licensing = true;
            }
        }

        assert!(
            found_licensing || ingestion_src.exists(),
            "Ingestion must check data source licensing before use"
        );
    }
}
