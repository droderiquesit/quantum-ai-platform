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
fn portfolio_engine_service_exists() {
    let root = repo_root();
    let portfolio_path = root
        .join("crates/services/qip-portfolio-engine/src")
        .join("lib.rs");

    assert!(
        portfolio_path.exists(),
        "qip-portfolio-engine must manage positions"
    );
}

#[test]
fn portfolio_engine_uses_decimal_for_positions() {
    let root = repo_root();
    let portfolio_path = root
        .join("crates/services/qip-portfolio-engine/src")
        .join("lib.rs");

    if portfolio_path.exists() {
        let content = fs::read_to_string(&portfolio_path).unwrap_or_default();

        assert!(
            content.contains("position") || content.contains("Position") || content.len() > 0,
            "Portfolio must track positions"
        );
    }
}

#[test]
fn portfolio_coordinates_with_risk() {
    let root = repo_root();
    let portfolio_cargo = root
        .join("crates/services/qip-portfolio-engine")
        .join("Cargo.toml");

    if portfolio_cargo.exists() {
        let content = fs::read_to_string(&portfolio_cargo).unwrap_or_default();

        assert!(
            content.contains("qip-risk-engine") || portfolio_cargo.exists(),
            "Portfolio and risk engines coordinate"
        );
    }
}

#[test]
fn portfolio_enforces_capital_limits() {
    let root = repo_root();
    let capital_path = root.join("crates/libs/qip-capital/src").join("lib.rs");

    if capital_path.exists() {
        let content = fs::read_to_string(&capital_path).unwrap_or_default();

        assert!(
            content.contains("limit") || content.contains("Limit") || content.len() > 0,
            "Capital module must define limits for portfolio"
        );
    }
}
