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

// --- no fourth cross-process path (ADR 0128 decision 4) --------------------

/// A send across a process boundary that is none of the admitted seams,
/// as it would appear in an app crate's production source: a socket the
/// crate opens itself, or the mesh's raw publisher, which only
/// `qip_mesh::spine` may drive.
const RAW_SEND_TOKENS: [&str; 7] = [
    "TcpStream::connect",
    "TcpStream::connect_timeout",
    "UdpSocket",
    "UnixStream",
    "tokio::net",
    "MeshPublisher",
    "MeshTransport",
];

/// The four admitted seams, by symbol (ADR 0128 decision 4): a new
/// transport fails the scan by name because it is not one of these.
const ADMITTED_SEND_SEAMS: [&str; 4] = [
    "qip_mesh::spine",
    "qip_transport::event_fabric",
    "qip_peer_link",
    "HttpClient",
];

/// Every `(file, token)` a raw send is admitted at, and why — the whole
/// allow-list, by file, so a new one fails the scan by name:
///
/// * the two self-pokes: a process connecting to its own listener to
///   unblock `accept` so the listener thread can see the shutdown flag, a
///   loopback poke of its own socket and not a send to another process;
/// * the mesh spine's own publisher, which is the first admitted path
///   (ADR 0128 decision 1), in the service that is the spine and in the
///   cell's delta uplink, the spine's upward half;
/// * the peer link's wire, the third admitted path, which opens its own
///   socket because it is the one edge crate licensed to (ADR 0116);
/// * `qip-streaming`'s `MeshTransport`, the bus registry's wrapper of the
///   raw publisher: admitted in its own file and the registry that builds
///   it, and nowhere else. No shipped binary selects it today — nothing
///   outside `qip-streaming` names `TransportKind` or its registry — and
///   that is a fact of the tree, not a guarantee of this scan: an app that
///   asked the registry for a transport by a configured name would publish
///   over the raw mesh without naming either type. ADR 0128 does not name
///   this wrapper as an admitted path; whether it is deleted or amended in
///   is the ADR's owner's call, and until then it is listed here so that a
///   reader sees it rather than a scan that found nothing.
///
/// The third element is the number of recording lines a socket-opening
/// token may have in the file, so that a second `connect` added beside an
/// admitted one is a finding and not hidden by the first. A type name
/// (`MeshPublisher`, `MeshTransport`) carries `None`: the count of lines
/// naming a type moves with every refactor and a change in it is not a new
/// send.
const ADMITTED_RAW_SENDS: [(&str, &str, Option<usize>); 8] = [
    ("apps/qip-ledgerd/src/lib.rs", "TcpStream::connect", Some(1)),
    ("apps/qip-fabricd/src/lib.rs", "TcpStream::connect", Some(1)),
    ("services/qip-mesh/src/spine.rs", "MeshPublisher", None),
    ("edge/qip-edge/src/mesh.rs", "MeshPublisher", None),
    (
        "edge/qip-peer-link/src/wire.rs",
        "TcpStream::connect_timeout",
        Some(1),
    ),
    ("services/qip-streaming/src/mesh.rs", "MeshPublisher", None),
    ("services/qip-streaming/src/mesh.rs", "MeshTransport", None),
    (
        "services/qip-streaming/src/registry.rs",
        "MeshTransport",
        None,
    ),
];

/// The crate groups the scan walks: every crate that is not a library.
/// `libs/` is left to `only_the_named_libraries_open_sockets`, which holds
/// the two libraries licensed to own sockets (ADR 0100, the redis module)
/// to their names.
const SCANNED_GROUPS: [&str; 4] = ["apps", "services", "runtime", "edge"];

/// ADR 0128 decision 4 (R12-FABRIC-10177): no crate outside the libraries
/// sends across a process except through the mesh spine, the fabric
/// producer, the peer link or a request through the transport's HTTP client
/// to a named synchronous route. Every other way out of a process — a socket
/// the crate opens, a `tokio` socket, the mesh's raw publisher or its
/// registry wrapper — is refused by name, in production source only
/// (`#[cfg(test)]` modules are stripped), with `ADMITTED_RAW_SENDS` as the
/// whole allow-list. Until 2026-10-31 the scan walked `apps/` only and
/// missed `connect_timeout`; a service or runtime crate that sent, which an
/// app then called through a typed API, put no token where the scan looked.
///
/// Mutation: add `let _ = std::net::TcpStream::connect("127.0.0.1:1");` to
/// a production `fn` in `crates/apps/qip-api/src/main.rs`, or to one in
/// `crates/runtime/qip-kernel/src/platform.rs` — fails at the offenders
/// assertion naming that file. Mutation: add the same line a second time
/// inside `crates/apps/qip-ledgerd/src/lib.rs` — fails at the site count,
/// not hidden by the admitted self-poke. Mutation: remove the peer link's
/// entry from `ADMITTED_RAW_SENDS` (and the array length) — fails at the
/// offenders assertion naming `edge/qip-peer-link/src/wire.rs`. Mutation:
/// set `SCANNED_GROUPS` back to `["apps"]` — fails at the premise that the
/// scan sees `services/qip-mesh/src/spine.rs`.
#[test]
fn no_crate_outside_the_libraries_sends_across_a_process_except_through_the_spine_the_fabric_producer_the_peer_link_or_a_named_synchronous_route()
 {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for group in SCANNED_GROUPS {
        rust_files(&crates.join(group), &mut files);
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    // `(file, token, recording lines)`.
    let mut found: Vec<(String, String, usize)> = Vec::new();
    let mut seams_used: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut walked = 0usize;
    for file in &files {
        let name = file
            .strip_prefix(&crates)
            .expect("under crates/")
            .to_string_lossy()
            .replace('\\', "/");
        // Production source only: `src/` files, with every `#[cfg(test)]`
        // module stripped, so a test's loopback socket is not a finding
        // and a real one cannot hide in one.
        if !name.contains("/src/") {
            continue;
        }
        let source = qip_acceptance::production_text(&fs::read_to_string(file).unwrap_or_default());
        walked += 1;
        for seam in ADMITTED_SEND_SEAMS {
            if source.contains(seam) {
                seams_used.insert(seam);
            }
        }
        for token in RAW_SEND_TOKENS {
            let sites = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .filter(|line| {
                    line.match_indices(token).any(|(i, _)| {
                        !line[..i].ends_with(is_ident)
                            && !line[i + token.len()..].starts_with(is_ident)
                    })
                })
                .count();
            if sites > 0 {
                found.push((name.clone(), token.to_string(), sites));
            }
        }
    }
    assert!(
        walked > 0,
        "walked no production files under {}",
        crates.display()
    );
    // Premise: the admitted seams are live in the scanned crates, so the
    // list is the platform's and not an aspiration; and the scan sees every
    // admitted site, so a green below is a scan that looked.
    for seam in ADMITTED_SEND_SEAMS {
        assert!(
            seams_used.contains(seam),
            "no scanned crate names the admitted seam {seam}; the list is stale"
        );
    }
    for (file, token, sites) in ADMITTED_RAW_SENDS {
        let seen = found
            .iter()
            .find(|(f, t, _)| f == file && t == token)
            .map(|(_, _, n)| *n);
        assert!(
            seen.is_some(),
            "the scan did not see {file}'s {token}; it is blind, or the allow-list is stale"
        );
        if let (Some(admitted), Some(seen)) = (sites, seen) {
            assert_eq!(
                seen, admitted,
                "{file} names {token} on {seen} lines and {admitted} are admitted: a socket \
                 was added beside the admitted one, or one was removed and the list is stale"
            );
        }
    }
    let offenders: Vec<_> = found
        .iter()
        .filter(|(file, token, _)| {
            !ADMITTED_RAW_SENDS
                .iter()
                .any(|(f, t, _)| f == file && t == token)
        })
        .map(|(file, token, _)| (file, token))
        .collect();
    assert!(
        offenders.is_empty(),
        "a crate outside the libraries sends across a process outside the spine, the fabric \
         producer, the peer link and the HTTP client: {offenders:?}"
    );
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
    //
    // Less the peer link. ADR 0116 put `qip-peer-link` under `edge/` because
    // it opens sockets and so cannot be a library, but it is Lane 1, off the
    // pass by construction: no crate on the pass may depend on it or name it
    // (`mesh_lane.rs`, MESH-013), and its threads and sockets are the whole
    // point of it. Named, not pattern-matched, so a second exclusion is a
    // diff a reviewer sees, and held to existing so it cannot go stale.
    const OFF_THE_PASS: &str = "qip-peer-link";
    assert!(
        crates.join("edge").join(OFF_THE_PASS).join("src").is_dir(),
        "{OFF_THE_PASS} is no longer under crates/edge; drop the exclusion"
    );
    let mut pass_path: Vec<PathBuf> = fs::read_dir(crates.join("edge"))
        .expect("crates/edge")
        .map(|entry| entry.expect("entry").path())
        .filter(|dir| !dir.ends_with(OFF_THE_PASS))
        .map(|dir| dir.join("src"))
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
