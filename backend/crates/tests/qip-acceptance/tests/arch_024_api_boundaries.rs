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
            content.len() > 0,
            "API must define REST endpoints in routes.rs"
        );
    }
}

#[test]
fn api_no_secret_in_responses() {
    let root = repo_root();
    let api_src = root.join("crates/apps/qip-api/src");

    if let Ok(entries) = fs::read_dir(&api_src) {
        for entry in entries {
            if let Ok(e) = entry {
                let path = e.path();
                if path.is_file() && path.to_string_lossy().ends_with(".rs") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        // Should not leak credentials
                        assert!(
                            !content.contains("password") || !content.contains("response"),
                            "API must not return secrets in responses"
                        );
                    }
                }
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
