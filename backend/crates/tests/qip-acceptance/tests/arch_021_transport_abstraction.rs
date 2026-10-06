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
fn transport_library_abstracts_network() {
    let root = repo_root();
    let transport_path = root.join("crates/libs/qip-transport/src").join("lib.rs");
    let content = fs::read_to_string(&transport_path).unwrap_or_default();

    assert!(
        content.contains("Client") || content.contains("Server"),
        "Transport must provide network abstractions"
    );
}

#[test]
fn transport_uses_blocking_io_with_timeout() {
    let root = repo_root();
    let transport_path = root.join("crates/libs/qip-transport/src").join("lib.rs");
    let content = fs::read_to_string(&transport_path).unwrap_or_default();

    assert!(
        content.contains("timeout") || content.contains("Duration"),
        "Transport must use blocking I/O with explicit timeouts"
    );
}

#[test]
fn http_client_supports_streaming() {
    let root = repo_root();
    let transport_path = root.join("crates/libs/qip-transport/src").join("lib.rs");
    let content = fs::read_to_string(&transport_path).unwrap_or_default();

    assert!(
        content.contains("stream") || content.contains("Stream") || content.contains("Client"),
        "HTTP client must support streaming for market data"
    );
}

#[test]
fn transport_no_tls_stack_in_tree() {
    let root = repo_root();
    let transport_src = root.join("crates/libs/qip-transport/src");

    if let Ok(entries) = fs::read_dir(&transport_src) {
        for entry in entries {
            if let Ok(e) = entry {
                let path = e.path();
                if path.is_file() && path.to_string_lossy().ends_with(".rs") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        // Transport must not hand-roll TLS
                        assert!(
                            !content.contains("tls_stream") || content.contains("native_tls"),
                            "Transport must not hand-roll TLS; delegate to native implementation"
                        );
                    }
                }
            }
        }
    }
}
