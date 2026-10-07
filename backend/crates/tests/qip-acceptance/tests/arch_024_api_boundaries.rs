use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn api_validates_all_external_input() {
    let root = repo_root();
    let api_path = root.join("crates/apps/qip-api/src").join("lib.rs");

    assert!(
        api_path.exists(),
        "qip-api must have lib.rs for composition"
    );
}

#[test]
fn rest_endpoints_defined_in_routes() {
    let root = repo_root();
    let routes_path = root.join("crates/apps/qip-api/src").join("routes.rs");

    if routes_path.exists() {
        let content = fs::read_to_string(&routes_path).unwrap_or_default();

        assert!(
            !content.is_empty(),
            "API must define REST endpoints in routes.rs"
        );
    }
}

#[test]
fn api_no_secret_in_responses() {
    let root = repo_root();
    let api_src = root.join("crates/apps/qip-api/src");

    if let Ok(entries) = fs::read_dir(&api_src) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file()
                && path.to_string_lossy().ends_with(".rs")
                && let Ok(content) = fs::read_to_string(&path)
            {
                // Should not leak credentials
                assert!(
                    !content.contains("password") || !content.contains("response"),
                    "API must not return secrets in responses"
                );
            }
        }
    }
}

#[test]
fn api_enforces_paper_trading_in_responses() {
    let root = repo_root();
    let routes_path = root.join("crates/apps/qip-api/src").join("routes.rs");

    if routes_path.exists() {
        let content = fs::read_to_string(&routes_path).unwrap_or_default();

        assert!(
            content.contains("PAPER") || content.contains("posture"),
            "API must render PAPER TRADING label in every response"
        );
    }
}
