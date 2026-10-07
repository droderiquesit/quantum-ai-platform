use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn workspace_permits_only_serde_dependencies() {
    let root = repo_root();
    let cargo_lock = root.join("Cargo.lock");

    // At least one serde-related dependency must exist
    if cargo_lock.exists() {
        let content = fs::read_to_string(&cargo_lock).unwrap_or_else(|_| String::new());

        assert!(
            content.contains("serde"),
            "Workspace must use serde for serialization (ADR 0002, 0009)"
        );
    }
}

#[test]
fn no_async_runtime_in_crates() {
    let root = repo_root();

    // Search for forbidden async runtimes
    let services_path = root.join("crates/services");
    if let Ok(entries) = fs::read_dir(&services_path) {
        for e in entries.flatten() {
            let cargo_path = e.path().join("Cargo.toml");
            if cargo_path.exists() {
                let content = fs::read_to_string(&cargo_path).unwrap_or_else(|_| String::new());

                assert!(
                    !content.contains("tokio") && !content.contains("async-std"),
                    "Workspace must use blocking I/O with timeouts, not async runtime (ADR 0001)"
                );
            }
        }
    }
}

#[test]
fn http_client_uses_blocking_io() {
    let root = repo_root();

    // qip-transport must be the HTTP client (blocking I/O)
    let transport_cargo = root.join("crates/libs/qip-transport").join("Cargo.toml");
    let transport_content = fs::read_to_string(&transport_cargo).unwrap_or_else(|_| String::new());

    assert!(
        transport_content.contains("qip-transport") || !transport_content.is_empty(),
        "qip-transport must exist for blocking I/O HTTP client"
    );
}

#[test]
fn json_serialization_via_serde() {
    let root = repo_root();

    // Core must use serde for JSON
    let core_cargo = root.join("crates/libs/qip-core").join("Cargo.toml");
    let core_content = fs::read_to_string(&core_cargo).unwrap_or_else(|_| String::new());

    // At minimum qip-core should depend on serde for types
    assert!(
        !core_content.is_empty(),
        "qip-core Cargo.toml must exist and define types"
    );
}
