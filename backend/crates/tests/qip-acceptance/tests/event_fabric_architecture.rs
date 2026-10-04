//! The library layer's socket boundary (`.claude/rules/architecture/00-boundaries.md`).
//!
//! Only `qip-transport` and `qip-storage/src/redis.rs` may name a std::net
//! socket type under `libs/*/src`. A lib that opens a socket is a service in
//! the wrong directory, and until this test existed the rule was prose.

use std::fs;
use std::path::{Path, PathBuf};

const SOCKET_TOKENS: [&str; 4] = ["TcpStream", "TcpListener", "UdpSocket", "ToSocketAddrs"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.expect("directory entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Delimited-token match: `TcpStream` must not match inside `MockTcpStreamX`.
fn names_socket(src: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .any(|line| {
            SOCKET_TOKENS.iter().any(|t| {
                line.match_indices(t).any(|(i, _)| {
                    !line[..i].ends_with(is_ident) && !line[i + t.len()..].starts_with(is_ident)
                })
            })
        })
}

fn socket_openers(libs: &Path) -> (usize, Vec<String>) {
    let mut files = Vec::new();
    for lib in fs::read_dir(libs)
        .expect("libs dir")
        .map(|e| e.expect("entry").path())
    {
        let src = lib.join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    let found = files
        .iter()
        .filter(|f| names_socket(&fs::read_to_string(f).unwrap_or_default()))
        .map(|f| {
            f.strip_prefix(libs)
                .expect("under libs")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    (files.len(), found)
}

#[test]
fn only_the_named_libraries_open_sockets() {
    let libs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libs");
    let (walked, found) = socket_openers(&libs);

    // Premise first: an empty walk would pass the assertion below vacuously.
    assert!(walked > 0, "walked no files under {}", libs.display());
    let allowed = |f: &str| f.starts_with("qip-transport/src/") || f == "qip-storage/src/redis.rs";
    assert!(
        found.iter().any(|f| f.starts_with("qip-transport/src/")),
        "qip-transport was not found opening sockets; the detector is blind: {found:?}"
    );
    assert!(
        found.iter().any(|f| f == "qip-storage/src/redis.rs"),
        "qip-storage/src/redis.rs was not found opening sockets; the detector is blind: {found:?}"
    );

    let offenders: Vec<_> = found.iter().filter(|f| !allowed(f)).collect();
    assert!(
        offenders.is_empty(),
        "libs other than the two named exceptions name a std::net socket: {offenders:?}"
    );
}
