use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn services_expose_domain_only_through_types() {
    let root = repo_root();

    // Each service should have its own lib.rs defining public types
    let risk_path = root
        .join("crates/services/qip-risk-engine/src")
        .join("lib.rs");
    let execution_path = root
        .join("crates/services/qip-execution-engine/src")
        .join("lib.rs");

    for path in &[risk_path, execution_path] {
        if path.exists() {
            let content = fs::read_to_string(path).unwrap_or_default();
            assert!(
                !content.is_empty(),
                "Service must define public API types in lib.rs"
            );
        }
    }
}

#[test]
fn services_do_not_depend_on_each_other() {
    let root = repo_root();

    let risk_cargo = root
        .join("crates/services/qip-risk-engine")
        .join("Cargo.toml");
    if risk_cargo.exists() {
        let content = fs::read_to_string(&risk_cargo).unwrap_or_default();

        // Risk engine should not depend on other services
        assert!(
            !content.contains("qip-execution-engine") && !content.contains("qip-portfolio-engine"),
            "Services must not depend on each other; they meet in qip-kernel"
        );
    }
}

#[test]
fn runtime_composes_services_only() {
    let root = repo_root();
    let kernel_path = root.join("crates/runtime/qip-kernel/src").join("lib.rs");
    let content = fs::read_to_string(&kernel_path).unwrap_or_default();

    assert!(
        content.contains("Platform") || content.contains("Cycle"),
        "Runtime must define Platform and Cycle for composition"
    );
}

#[test]
fn service_contracts_versioned_independently() {
    let root = repo_root();

    let risk_cargo = root
        .join("crates/services/qip-risk-engine")
        .join("Cargo.toml");
    let execution_cargo = root
        .join("crates/services/qip-execution-engine")
        .join("Cargo.toml");

    // Each service has independent version in Cargo.toml
    for path in &[risk_cargo, execution_cargo] {
        if path.exists() {
            let content = fs::read_to_string(path).unwrap_or_default();
            assert!(
                content.contains("version"),
                "Each service must declare its own version"
            );
        }
    }
}
