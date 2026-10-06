#![allow(clippy::unwrap_used, clippy::expect_used)]
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

// --- the cell's pass path (CICD-018) -----------------------------------------

/// What a pass must never wait on. A sleep or a parked thread is a pass that
/// stops for a wall-clock reason; a spawned thread or a channel is work whose
/// completion the pass then waits for or never sees; a socket or a child
/// process of the cell's own is I/O outside `qip-transport`'s timeouts.
const BLOCKING_TOKENS: [&str; 9] = [
    "thread::sleep",
    "thread::spawn",
    "thread::park",
    "mpsc",
    "Condvar",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "std::process",
];

/// The one file on the pass path that opens a file: the cell's journal, which
/// is the durable record the pass exists to write.
const JOURNAL: &str = "edge/qip-edge/src/journal.rs";

/// Every `(file, token)` under `dirs` that names `tokens` outside a comment,
/// as a delimited token, with the number of files walked.
fn names_any(crates: &Path, dirs: &[PathBuf], tokens: &[&str]) -> (usize, Vec<(String, String)>) {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut files = Vec::new();
    for dir in dirs {
        rust_files(dir, &mut files);
    }
    let mut found = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).unwrap_or_default();
        let name = file
            .strip_prefix(crates)
            .expect("under crates/")
            .to_string_lossy()
            .replace('\\', "/");
        for token in tokens {
            let named = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .any(|line| {
                    line.match_indices(token).any(|(i, _)| {
                        !line[..i].ends_with(is_ident)
                            && !line[i + token.len()..].starts_with(is_ident)
                    })
                });
            if named {
                found.push((name.clone(), token.to_string()));
            }
        }
    }
    (files.len(), found)
}

/// CICD-018's third seeded case: "adds a blocking call on the hot path".
///
/// The dependency rules in `architecture.rs` stop the cell reaching a model or
/// a store. Nothing stopped a `std::thread::sleep` being written straight
/// into `qip-routing`, which needs no dependency at all — and a pass that
/// sleeps is a cell that is late on every venue at once.
///
/// What this cannot see, stated so the green is not over-read: a lock held
/// across I/O, and a call into `qip-transport` whose timeout is too long.
/// Those are behaviours, not tokens.
///
/// Mutation: add `std::thread::sleep(std::time::Duration::ZERO);` to a
/// function in `crates/edge/qip-routing/src/lib.rs`. The `found.is_empty()`
/// assertion fails naming that file and `thread::sleep`.
#[test]
fn no_crate_on_the_cells_pass_path_sleeps_spawns_a_thread_or_opens_a_socket_of_its_own() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("crates/ exists");

    // The pass path: every crate of the regional cell, and the two engines a
    // pass calls before an order exists.
    let mut pass_path: Vec<PathBuf> = fs::read_dir(crates.join("edge"))
        .expect("crates/edge")
        .map(|entry| entry.expect("entry").path().join("src"))
        .filter(|src| src.is_dir())
        .collect();
    pass_path.push(crates.join("services/qip-risk-engine/src"));
    pass_path.push(crates.join("services/qip-execution-engine/src"));
    pass_path.sort();

    // Premise one: the detector sees the tokens where they are known to be.
    // `qip-transport` sleeps between retries and spawns its server's workers,
    // by design and off the pass path; a detector that finds neither there
    // would find nothing anywhere.
    let (_, known) = names_any(
        &crates,
        &[crates.join("libs/qip-transport/src")],
        &BLOCKING_TOKENS,
    );
    for token in ["thread::sleep", "thread::spawn", "TcpStream"] {
        assert!(
            known.iter().any(|(_, found)| found == token),
            "the detector did not find `{token}` in qip-transport, where it is; it is blind"
        );
    }

    // Premise two: the walk covered the cell itself.
    let (walked, found) = names_any(&crates, &pass_path, &BLOCKING_TOKENS);
    assert!(
        walked > 50
            && pass_path
                .iter()
                .any(|dir| dir.ends_with("edge/qip-edge/src")),
        "walked only {walked} files over {pass_path:?}"
    );
    assert!(
        found.is_empty(),
        "a crate on the cell's pass path names a blocking primitive: {found:?}. A pass waits on \
         nothing; move the wait to the composition root (qip-edge-node) or behind qip-transport"
    );

    // And the file system: the journal, and only the journal.
    let (_, files) = names_any(&crates, &pass_path, &["std::fs"]);
    let files: Vec<&str> = files.iter().map(|(file, _)| file.as_str()).collect();
    assert_eq!(
        files,
        vec![JOURNAL],
        "std::fs is named on the pass path somewhere other than the cell's journal, or the \
         journal no longer writes a file and this exemption is stale"
    );
}
