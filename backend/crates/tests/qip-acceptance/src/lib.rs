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
//! * `architecture.rs` reads the dependency graph and asserts the edges that
//!   are *absent*. A present edge is visible in the code that uses it; an
//!   absent one is invisible until someone adds it.
//! * `acceptance.rs` is the end-to-end scenario.
//! * `resilience.rs` is the same platform under load, a degraded feed and an
//!   operator pulling things out from under it.

/// The repository root, found by walking up from this crate.
///
/// Tests read files from the repository, and `CARGO_MANIFEST_DIR` is the only
/// path that is correct whether `cargo test` was run from the root or from the
/// crate.
// Test-support crate: every caller is a test, where a panic is the failure report.
#[allow(clippy::expect_used)]
pub fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("the crate is four levels below the repository root: backend/crates/tests/qip-acceptance")
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

/// The production text of a Rust source file: comment lines dropped and every
/// `#[cfg(test)]` item skipped by brace depth, wherever in the file it sits.
///
/// For the scans that assert a name is *absent* from production code. Cutting
/// at the first `#[cfg(test)]` instead would stop reading at a test-only
/// helper near the top of a file and call everything below it unscanned
/// production clean; keeping the tests would report a fixture as a violation.
/// A caller asserts the result is non-empty for a file it expects to hold
/// production code, because a helper that returned nothing would make every
/// absence trivially true.
pub fn production_text(source: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        if line.trim() != "#[cfg(test)]" {
            kept.push(line);
            continue;
        }
        // Skip the attributed item. One with no body ends at its semicolon;
        // one with a body ends where its braces balance.
        let mut depth = 0usize;
        let mut opened = false;
        for body in lines.by_ref() {
            depth += body.matches('{').count();
            opened |= depth > 0;
            depth = depth.saturating_sub(body.matches('}').count());
            if (opened && depth == 0) || (!opened && body.trim_end().ends_with(';')) {
                break;
            }
        }
    }
    kept.join("\n")
}
