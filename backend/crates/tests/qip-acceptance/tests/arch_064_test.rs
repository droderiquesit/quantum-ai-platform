use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn requirement_64_part_1() {
    let root = repo_root();
    assert!(root.exists(), "Working directory accessible");
}

#[test]
fn requirement_64_part_2() {
    let root = repo_root();
    let crates = root.join("crates");
    assert!(crates.exists(), "Crates directory exists");
}

#[test]
fn requirement_64_part_3() {
    let root = repo_root();
    let kernel = root.join("crates/runtime/qip-kernel/src/lib.rs");
    assert!(kernel.exists(), "Kernel exists");
}

#[test]
fn requirement_64_part_4() {
    // Placeholder: this test asserts nothing and cannot fail.
}
