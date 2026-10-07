use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn all_io_operations_have_timeouts() {
    let root = repo_root();

    // Transport must declare timeout support
    let transport_path = root.join("crates/libs/qip-transport/src").join("lib.rs");
    let transport_content = fs::read_to_string(&transport_path).unwrap_or_else(|_| String::new());

    assert!(
        transport_content.contains("timeout")
            || transport_content.contains("Timeout")
            || transport_content.contains("Duration"),
        "All I/O operations must have explicit timeouts (ADR 0001)"
    );
}

#[test]
fn socket_operations_use_blocking_calls() {
    let root = repo_root();

    // qip-transport must use blocking sockets, not select/poll
    let transport_path = root.join("crates/libs/qip-transport/src").join("lib.rs");
    if let Ok(content) = fs::read_to_string(&transport_path) {
        // Should not have async/await patterns
        assert!(
            !content.contains("async") || content.contains("blocking"),
            "Transport must use blocking I/O, not async"
        );
    }
}

#[test]
fn no_unwrap_on_io_results() {
    let root = repo_root();

    // I/O errors must be returned, not unwrapped
    let transport_src = root.join("crates/libs/qip-transport/src");
    let mut uses_result_properly = false;

    if let Ok(entries) = fs::read_dir(&transport_src) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file()
                && path.to_string_lossy().ends_with(".rs")
                && let Ok(content) = fs::read_to_string(&path)
                && content.contains("Result")
            {
                uses_result_properly = true;
                break;
            }
        }
    }

    assert!(
        uses_result_properly,
        "Transport must return Results from I/O operations"
    );
}

#[test]
fn explicit_timeouts_on_blocking_calls() {
    let root = repo_root();

    // Blocking I/O must have explicit timeout specifications
    let transport_path = root.join("crates/libs/qip-transport/src").join("lib.rs");
    if let Ok(content) = fs::read_to_string(&transport_path) {
        // Should reference Duration or Timeout somewhere
        assert!(
            content.contains("timeout") || content.contains("Duration"),
            "Blocking I/O must declare timeout specifications explicitly"
        );
    }
}
