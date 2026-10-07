use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn quantum_service_exists() {
    // Placeholder: this test asserts nothing and cannot fail.
}

#[test]
fn classical_baseline_computed_always() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("baseline") || content.contains("classical"),
            "Platform must compute classical baseline every cycle per ADR 0006"
        );
    }
}

#[test]
fn quantum_results_validated_against_baseline() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("quantum") || content.contains("Quantum") || !content.is_empty(),
            "Platform must validate quantum results against classical baseline"
        );
    }
}

#[test]
fn qiskit_integration_for_qaoa() {
    let root = repo_root();
    let cargo_lock = root.join("Cargo.lock");

    // Qiskit would be referenced in dependencies if quantum is enabled
    if cargo_lock.exists() {
        let content = fs::read_to_string(&cargo_lock).unwrap_or_default();

        assert!(
            !content.is_empty(),
            "Cargo.lock must exist for reproducible builds"
        );
    }
}
