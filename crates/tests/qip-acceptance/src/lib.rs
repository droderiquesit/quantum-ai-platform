//! `qip-acceptance` — workspace-level tests.
//!
//! This crate has no library code. It exists so that tests spanning several
//! crates, and tests checking things outside the Rust code entirely, have
//! somewhere to live that participates in `cargo test --workspace`.
//!
//! The tests are in `tests/`:
//!
//! * `infrastructure.rs` checks the Terraform and Kubernetes configuration
//!   structurally. `terraform validate` catches a malformed configuration;
//!   these catch a well-formed one that would deploy something unsafe.
//! * `documentation.rs` checks that what the documentation claims matches what
//!   the code does. Documentation that has drifted is worse than none, because
//!   someone will believe it.
//! * `acceptance.rs` is the end-to-end scenario.

/// The repository root, found by walking up from this crate.
///
/// Tests read files from the repository, and `CARGO_MANIFEST_DIR` is the only
/// path that is correct whether `cargo test` was run from the root or from the
/// crate.
pub fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate is three levels below the repository root")
        .to_path_buf()
}

/// Read a file relative to the repository root, failing loudly if it is
/// missing.
///
/// A test that silently skips when its input is absent is a test that passes
/// after someone deletes the thing it was checking.
pub fn read(relative: &str) -> String {
    let path = repository_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// Every file under a directory, recursively, with the given extension.
pub fn files_with_extension(relative: &str, extension: &str) -> Vec<std::path::PathBuf> {
    let root = repository_root().join(relative);
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}
