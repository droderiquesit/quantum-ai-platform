//! The real-process harness ADR 0100 §8 needs, proven against itself.
//!
//! §8's eight tests spawn the five slice binaries — `qip-fabricd`,
//! `qip-ledgerd`, `qip-edge-node`, `qip-api` and `qip-cli`'s `qip` — for real,
//! on free ports, and drive faults between them. None of that is safe to
//! build on faith: a suite that silently skips a missing binary has stopped
//! testing anything, a suite that runs a stale one is testing yesterday's code
//! under today's name, a proxy whose fault does not do what its name says
//! proves a property nobody has, and a suite that leaks a child leaves an
//! orphan for whoever runs the next one.
//!
//! This file proves each of those properties of `support/` before any suite
//! leans on them, and needs none of the five binaries built to do it: every
//! refusal is exercised against a directory or a workspace the test controls,
//! and every fault against a loopback server it starts itself.

mod support;

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use qip_acceptance::repository_root;
use support::processes::{self, ManagedChild, ScratchDirectory};
use support::proxy::{FaultKind, FaultSchedule, Proxy};
use support::{poll_until, scrape};

/// How long a peer must stay quiet to count as silent: long enough to be
/// plain, short enough that the suite runs in seconds (m10).
const SILENCE: Duration = Duration::from_millis(300);

/// How long something that should happen may take on a loaded runner. Only
/// ever the bound on a wait for a condition that is expected to come, so it
/// costs nothing when the harness is right.
const EVENTUALLY: Duration = Duration::from_secs(10);

/// Extract a human-readable message from a `catch_unwind` payload.
///
/// A `panic!` payload in this codebase is always a `String` or a `&str` — see
/// `gitops.rs`'s identical idiom — so those are the only two shapes this looks
/// for.
///
/// Takes the payload already dereferenced (`&*boxed_payload`, never
/// `&boxed_payload`): `Box<dyn Any + Send>` itself also satisfies `Any`, so a
/// reference to the box coerces to `&(dyn Any + Send)` *for the box*, and
/// downcasting that answers "is the payload a `Box`" — always no, silently.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".to_string())
}

/// Whether a process with this pid still exists.
///
/// No `unsafe` FFI: this platform is deployed only on Linux, where
/// `/proc/<pid>` existing is exactly this fact, and `ManagedChild` always
/// `wait`s after `kill`, so a killed child is reaped rather than left a zombie
/// whose `/proc` entry would linger.
fn is_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or_else(|error| panic!("cannot read the mtime of {}: {error}", path.display()))
}

/// Write `contents` to `root/relative` and stamp it with the current time
/// explicitly, rather than trusting the filesystem's coarse clock to have
/// ticked since the last link.
fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    std::fs::write(&path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
    stamp_now(&path);
    path
}

fn append(root: &Path, relative: &str, text: &str) -> PathBuf {
    let path = root.join(relative);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap_or_else(|error| panic!("cannot open {}: {error}", path.display()));
    file.write_all(text.as_bytes())
        .unwrap_or_else(|error| panic!("cannot append to {}: {error}", path.display()));
    drop(file);
    stamp_now(&path);
    path
}

fn stamp_now(path: &Path) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(SystemTime::now()))
        .unwrap_or_else(|error| panic!("cannot stamp {}: {error}", path.display()));
}

// --- resolving a binary --------------------------------------------------------

#[test]
fn a_missing_sibling_binary_fails_naming_the_build_command_and_is_never_skipped() {
    // A fresh directory, never the shared `target/<profile>/`: the build line
    // every §8 suite runs first puts `qip-fabricd` there, and so does `cargo
    // test --workspace` once that crate has integration tests of its own, so a
    // premise of "absent from the target directory" is one with a date on
    // which it turns false and this test goes red for nothing. Absent from an
    // empty directory is absent by construction, and the refusal is the same
    // code either way.
    let empty = ScratchDirectory::new("missing-binary");
    let expected = empty
        .path()
        .join(format!("qip-fabricd{}", std::env::consts::EXE_SUFFIX));
    assert!(
        !expected.exists(),
        "premise: {} must not exist for this test to be exercising a missing binary",
        expected.display()
    );

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        processes::resolve_binary_in(empty.path(), "qip-fabricd")
    }));
    let payload = outcome.expect_err(
        "resolve_binary must refuse a missing sibling binary by panicking, never by returning a \
         path that does not exist and never by skipping the caller onward",
    );
    let message = panic_message(&*payload);
    assert!(
        message.contains(processes::BUILD_COMMAND),
        "the panic did not name the build command an operator should run: {message:?}"
    );
    assert!(
        message.contains(&expected.display().to_string()),
        "the panic did not say where it looked: {message:?}"
    );
}

const PROBE_MANIFEST: &str = "[package]
name = \"probe\"
version = \"0.1.0\"
edition = \"2024\"
publish = false

[dependencies]
probe-dep = { path = \"../probe-dep\" }

[build-dependencies]
probe-build = { path = \"../probe-build\" }
";

fn library_manifest(name: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n"
    )
}

/// Build the probe workspace for real, bounded, with its own target
/// directory so nothing it links lands beside this suite's binaries.
fn build_probe(workspace: &Path) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = std::process::Command::new(cargo);
    command
        .current_dir(workspace)
        .args([
            "build",
            "--offline",
            "--bins",
            "-p",
            "probe",
            "--manifest-path",
        ])
        .arg(workspace.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(workspace.join("target"));
    let mut build = ManagedChild::spawn("cargo-build-probe", command);
    let status = build.wait_for_exit(Duration::from_secs(300));
    assert!(
        status.success(),
        "building the probe workspace failed ({status}):\n{}",
        build.captured_stderr()
    );
}

#[test]
fn a_binary_older_than_its_crates_sources_is_refused_naming_the_build_command() {
    // A real workspace, built for real: a binary, the path dependency it links
    // (where most of `qip-api`'s code lives, in `qip-kernel` and below), a
    // build-dependency, and a crate it does not link yet. A fake binary and a
    // hand-written dep-info could only prove the harness agrees with this
    // file; cargo's own output is what the rule has to agree with, and only a
    // real build can show the refusal's named remedy actually clears it.
    let scratch = ScratchDirectory::new("stale-binary");
    let root = scratch.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"probe\", \"probe-dep\", \"probe-build\", \
         \"probe-extra\"]\n",
    );
    write(root, "probe/Cargo.toml", PROBE_MANIFEST);
    write(
        root,
        "probe/build.rs",
        "fn main() {\n    probe_build::announce();\n    println!(\"cargo::rerun-if-changed=build.rs\");\n}\n",
    );
    write(
        root,
        "probe/src/main.rs",
        "fn main() {\n    println!(\"{}\", probe_dep::answer());\n}\n",
    );
    write(root, "probe/tests/it.rs", "#[test]\nfn it() {}\n");
    write(root, "probe-dep/Cargo.toml", &library_manifest("probe-dep"));
    write(
        root,
        "probe-dep/src/lib.rs",
        "pub fn answer() -> u32 {\n    42\n}\n",
    );
    write(
        root,
        "probe-build/Cargo.toml",
        &library_manifest("probe-build"),
    );
    write(root, "probe-build/src/lib.rs", "pub fn announce() {}\n");
    write(
        root,
        "probe-extra/Cargo.toml",
        &library_manifest("probe-extra"),
    );
    write(root, "probe-extra/src/lib.rs", "pub fn extra() {}\n");
    // The repository's pinned toolchain, so the nested build resolves the same
    // compiler however `cargo` reached this process.
    std::fs::copy(
        repository_root().join("backend/rust-toolchain.toml"),
        root.join("rust-toolchain.toml"),
    )
    .expect("copy the pinned toolchain into the probe workspace");

    let manifest = root.join("Cargo.toml");
    let binary = root
        .join("target/debug")
        .join(format!("probe{}", std::env::consts::EXE_SUFFIX));
    const COMMAND: &str = "cargo build -p probe --bins";
    let judge = || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            processes::resolve_checked(&manifest, "probe", &binary, COMMAND)
        }))
        .map_err(|payload| panic_message(&*payload))
    };

    build_probe(root);
    // Premise: the rule admits what cargo has just linked, build-dependency
    // and all. A rule that refused every binary would pass every refusal
    // below, which is the difference between a gate and a wall.
    if let Err(message) = judge() {
        panic!("a binary cargo had just linked was refused: {message}");
    }
    let linked = mtime(&binary);

    // 1. Edits cargo relinks nothing for are not staleness. Refusing on them
    //    would name a build command that cannot clear the refusal: an
    //    integration test, and a comment in the package's and the
    //    workspace's manifests.
    let untracked = [
        append(root, "probe/tests/it.rs", "// edited after the link\n"),
        append(root, "probe/Cargo.toml", "# edited after the link\n"),
        append(root, "Cargo.toml", "# edited after the link\n"),
    ];
    for file in &untracked {
        assert!(
            mtime(file) > linked,
            "premise: {} must be newer than the binary for this step to mean anything",
            file.display()
        );
    }
    if let Err(message) = judge() {
        panic!(
            "an edit cargo relinks nothing for refused the binary, and the command the refusal \
             names cannot clear it: {message}"
        );
    }

    // 2. The path dependency's source changes after the link, and nothing of
    //    the probe's own does: a refusal here can only come from following
    //    the dependency.
    let dependency_source = write(
        root,
        "probe-dep/src/lib.rs",
        "pub fn answer() -> u32 {\n    43\n}\n",
    );
    assert!(
        mtime(&dependency_source) > linked,
        "premise: the dependency's source must be newer than the binary"
    );
    for own in ["probe/src/main.rs", "probe/build.rs"] {
        assert!(
            mtime(&root.join(own)) < linked,
            "premise: {own} must predate the binary, so a refusal is the dependency's doing"
        );
    }
    let message = judge().expect_err(
        "a binary linked before its path dependency's source changed was accepted — the check \
         looked at presence, or at the probe's own files, and not at what it was built from",
    );
    assert!(
        message.contains(COMMAND),
        "the refusal did not name the build command: {message:?}"
    );
    assert!(
        message.contains(&dependency_source.display().to_string()),
        "the refusal did not name the file that made the binary stale: {message:?}"
    );

    // 3. The refusal's own remedy clears it.
    build_probe(root);
    if let Err(message) = judge() {
        panic!("running the build the refusal named did not clear it: {message}");
    }

    // 4. A manifest that now links a crate the binary was not built from.
    let with_extra = PROBE_MANIFEST.replace(
        "probe-dep = { path = \"../probe-dep\" }\n",
        "probe-dep = { path = \"../probe-dep\" }\nprobe-extra = { path = \"../probe-extra\" }\n",
    );
    assert_ne!(
        with_extra, PROBE_MANIFEST,
        "premise: the manifest edit applied"
    );
    write(root, "probe/Cargo.toml", &with_extra);
    let dep_info = std::fs::read_to_string(binary.with_extension("d"))
        .expect("cargo wrote the probe's dep-info beside it");
    assert!(
        !dep_info.contains("probe-extra"),
        "premise: the binary's dep-info must not already list probe-extra"
    );
    let message = judge().expect_err(
        "a binary linked before its manifest named probe-extra was accepted — the manifests' \
         dependency graph was not consulted",
    );
    assert!(
        message.contains(COMMAND) && message.contains("\"probe-extra\""),
        "the refusal did not name the crate the binary was not built from and the build command: \
         {message:?}"
    );
    build_probe(root);
    if let Err(message) = judge() {
        panic!("running the build did not clear the manifest refusal: {message}");
    }
}

/// The value of an unconditional `[dependencies]` key such as
/// `qip-kernel.workspace = true` or `qip-kernel = { path = … }`.
fn manifest_key(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    line.split(['.', '=', ' ']).next()
}

fn dependencies_section(manifest: &str) -> String {
    manifest
        .lines()
        .skip_while(|line| line.trim() != "[dependencies]")
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A dep-info in the format cargo writes, for a binary this test fakes.
fn write_dep_info(binary: &Path, sources: &[PathBuf]) {
    let render = |path: &Path| path.display().to_string().replace(' ', "\\ ");
    let listed: String = sources
        .iter()
        .map(|source| format!(" {}", render(source)))
        .collect();
    let dep_info = binary.with_extension("d");
    std::fs::write(&dep_info, format!("{}:{listed}\n", render(binary)))
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", dep_info.display()));
}

#[test]
fn a_sibling_binary_is_judged_against_every_workspace_crate_it_is_built_from() {
    // The real 59-crate workspace, through the real `resolve_binary_in`: the
    // path a §8 suite takes, which the probe workspace above cannot stand in
    // for — workspace-inherited dependencies, and the depth of the graph under
    // `qip-api`, are this repository's shape and not a toy's.
    //
    // Premise, read from qip-api's own manifest rather than from the harness:
    // qip-kernel is one of the crates the binary is built from.
    let api_manifest = qip_acceptance::read("backend/crates/apps/qip-api/Cargo.toml");
    assert!(
        dependencies_section(&api_manifest)
            .lines()
            .any(|line| manifest_key(line) == Some("qip-kernel")),
        "premise: qip-api's [dependencies] must name qip-kernel"
    );

    let scratch = ScratchDirectory::new("real-workspace");
    let binary = scratch
        .path()
        .join(format!("qip-api{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &binary,
        b"not an executable; only its mtime and dep-info are read",
    )
    .expect("write the fake binary");
    let linked = mtime(&binary);

    // A dep-info listing only qip-api's own sources: every file predates the
    // binary, so a refusal can only be about what the list leaves out.
    let own = qip_acceptance::files_with_extension("backend/crates/apps/qip-api/src", "rs");
    assert!(!own.is_empty(), "premise: qip-api has sources to list");
    assert!(
        own.iter().all(|file| mtime(file) <= linked),
        "premise: every listed file must predate the fake binary"
    );
    write_dep_info(&binary, &own);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        processes::resolve_binary_in(scratch.path(), "qip-api")
    }));
    let message = panic_message(&*outcome.expect_err(
        "a qip-api binary whose dep-info lists nothing from qip-kernel was accepted — the walk \
         did not follow qip-api's manifest into the workspace",
    ));
    assert!(
        message.contains("\"qip-kernel\""),
        "the refusal did not name qip-kernel among the crates the binary was not built from: \
         {message:?}"
    );
    assert!(
        message.contains(processes::BUILD_COMMAND),
        "the refusal did not name the build command: {message:?}"
    );

    // The same real graph admits a binary whose dep-info covers every crate:
    // a rule that refused all fakes would pass the half above.
    let every = qip_acceptance::files_with_extension("backend/crates", "rs");
    assert!(
        every.iter().all(|file| mtime(file) <= linked),
        "premise: every listed file must predate the fake binary"
    );
    write_dep_info(&binary, &every);
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        processes::resolve_binary_in(scratch.path(), "qip-api")
    })) {
        panic!(
            "a qip-api binary newer than every workspace source, with a dep-info covering them \
             all, was refused: {}",
            panic_message(&*payload)
        );
    }
}

// --- banners --------------------------------------------------------------------

#[test]
fn every_real_startup_banner_ends_in_the_address_the_harness_parses() {
    // Read from the binaries' own sources rather than restated here, so a
    // banner reworded there fails in this file — not in a §8 suite waiting
    // out its deadline on a line that will never come. `println!` rather than
    // `eprintln!` is part of the check: `wait_for_line` reads stdout.
    //
    // qip-edge-node is deliberately absent. Its banner has the right shape, but
    // it prints the *configured* address (`format!("0.0.0.0:{}", config.health_port)`
    // in qip-edge-node/src/main.rs), not `listener.local_addr()`. Under the
    // harness's `:0` ports that is `0.0.0.0:0`, which names no port a test can
    // reach. Certifying its shape here would pass a banner the harness cannot
    // use. SLICE-36, which owns that main.rs, prints the bound address and adds
    // the entry back.
    let banners = [(
        "backend/crates/apps/qip-api/src/main.rs",
        "qip-api listening on ",
    )];
    for (file, prefix) in banners {
        let source = qip_acceptance::read(file);
        let opening = format!("println!(\"{prefix}{{");
        let printed: Vec<&str> = source
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(&opening))
            .collect();
        assert_eq!(
            printed.len(),
            1,
            "premise: {file} must print exactly one stdout line starting {prefix:?} with a format \
             argument; found {printed:?}"
        );
        let literal = printed[0]
            .strip_prefix("println!(\"")
            .and_then(|rest| rest.strip_suffix("\");"))
            .unwrap_or_else(|| panic!("{:?} is not a one-argument println! line", printed[0]));
        let placeholder = &literal[prefix.len()..];
        assert!(
            placeholder.starts_with('{')
                && placeholder.ends_with('}')
                && placeholder.matches('{').count() == 1,
            "premise: {file}'s banner {literal:?} must end in exactly one placeholder, the address"
        );

        for address in ["127.0.0.1:54321", "[::1]:54321"] {
            let expected: SocketAddr = address.parse().expect("a literal socket address");
            let output = format!("some earlier line\n{prefix}{address}\n");
            let line = processes::complete_line_containing(&output, prefix)
                .unwrap_or_else(|| panic!("no complete line containing {prefix:?} in {output:?}"));
            assert_eq!(
                processes::parse_bound_address(line),
                expected,
                "{file}'s banner, printed with {address}, parsed to the wrong address"
            );
        }
    }
}

#[test]
fn a_banner_read_mid_flush_is_not_taken_until_its_line_ends() {
    let torn = "qip-api listening on 127.0.0.1:5";
    // Premise: the torn read is not garbage. It parses — as the wrong
    // address — which is why taking it would fail silently, with a suite
    // talking to a port nothing bound.
    assert_eq!(
        processes::parse_bound_address(torn),
        "127.0.0.1:5"
            .parse::<SocketAddr>()
            .expect("a literal socket address"),
        "premise: the torn banner parses as a valid, wrong address"
    );
    assert_eq!(
        processes::complete_line_containing(torn, "listening on "),
        None,
        "a banner with no line ending yet was taken"
    );
    assert_eq!(
        processes::complete_line_containing(&format!("{torn}4321\n"), "listening on "),
        Some("qip-api listening on 127.0.0.1:54321"),
        "the finished banner was not taken"
    );
    assert_eq!(
        processes::complete_line_containing(
            "qip-api listening on 127.0.0.1:54321\r\n",
            "listening on "
        ),
        Some("qip-api listening on 127.0.0.1:54321"),
        "a CRLF-terminated banner kept its carriage return"
    );
}

// --- the fault proxy ------------------------------------------------------------

#[test]
fn one_seed_yields_one_fault_schedule() {
    let a = FaultSchedule::seeded(42, 16);
    let b = FaultSchedule::seeded(42, 16);
    assert_eq!(
        a.kinds(),
        b.kinds(),
        "the same seed must produce the same fault schedule every time"
    );

    // The premises that make the equality mean something: a different seed
    // produces a different schedule, so `seeded` reads its argument; and the
    // schedule draws more than one kind, so it is not a constant either.
    let c = FaultSchedule::seeded(43, 16);
    assert_ne!(
        a.kinds(),
        c.kinds(),
        "two different seeds produced the same schedule; the generator may not read the seed"
    );
    let first = a.kinds()[0];
    assert!(
        a.kinds().iter().any(|kind| *kind != first),
        "a sixteen-fault schedule drew one kind only: {:?}",
        a.kinds()
    );

    // And a proxy stepped through a replay of the schedule applies exactly
    // it, in order. No connection is made, so the upstream is never dialled.
    let proxy = Proxy::start("127.0.0.1:9".parse().expect("a literal socket address"));
    let mut replay = FaultSchedule::seeded(42, 16);
    let applied: Vec<FaultKind> = (0..16)
        .map(|_| {
            let fault = proxy.inject_next(&mut replay);
            assert_eq!(
                proxy.fault(),
                Some(fault),
                "inject_next did not apply what it drew"
            );
            fault
        })
        .collect();
    assert_eq!(
        applied,
        a.kinds(),
        "the proxy did not replay the seeded schedule in order"
    );
}

#[test]
fn a_stalled_upstream_accepts_the_connection_and_never_answers() {
    let upstream = LoopbackServer::echo();
    let proxy = Proxy::start(upstream.address());

    // Premise: the relay answers while healthy, on a connection that will stay
    // open across the fault, so the silence below is the fault's doing and
    // not a relay that never worked.
    let mut open = connect(proxy.address());
    round_trip(&mut open, b"before\n");

    proxy.inject(FaultKind::Stall);
    let received_before = upstream.received();

    // A connection made during the stall is accepted — it connects and its
    // request is written — and nothing answers it.
    let mut fresh = connect(proxy.address());
    fresh
        .write_all(b"during\n")
        .expect("a stalled proxy still accepts a connection and its bytes");
    assert_silent(&mut fresh, "a connection accepted during the stall");
    // One already open goes quiet too.
    open.write_all(b"held\n")
        .expect("a stalled connection still takes a small write");
    assert_silent(&mut open, "a connection open before the stall");
    assert_eq!(
        upstream.received(),
        received_before,
        "the stalled proxy forwarded bytes to the upstream"
    );

    // A stall is a pause, not a loss: healing delivers what it held.
    proxy.heal();
    expect_exact(&mut open, b"held\n");
    expect_exact(&mut fresh, b"during\n");
}

#[test]
fn a_blackholed_upstream_swallows_every_byte_and_answers_none() {
    let upstream = LoopbackServer::echo();
    let proxy = Proxy::start(upstream.address());
    let mut open = connect(proxy.address());
    round_trip(&mut open, b"before\n");

    proxy.inject(FaultKind::Blackhole);
    let received_before = upstream.received();
    let discarded_before = proxy.stats().discarded_from_client;

    let payload = vec![b'x'; 256 * 1024];
    let mut fresh = connect(proxy.address());
    fresh
        .write_all(&payload)
        .expect("a blackhole reads everything written into it");
    open.write_all(b"swallowed\n")
        .expect("a blackholed connection takes a write");
    // Read and thrown away, not left waiting in a buffer: the difference
    // between a blackhole and a stall, observed rather than assumed.
    let expected = discarded_before + payload.len() as u64 + b"swallowed\n".len() as u64;
    poll_until(
        "the blackhole reading every byte sent into it",
        EVENTUALLY,
        || (proxy.stats().discarded_from_client >= expected).then_some(()),
    );
    assert_silent(&mut fresh, "a connection accepted by the blackhole");
    assert_silent(&mut open, "a connection open when the blackhole began");
    assert_eq!(
        upstream.received(),
        received_before,
        "the blackhole forwarded bytes to the upstream"
    );

    // A stream that lost bytes is never resumed from the middle: healing
    // closes both connections instead of relaying on.
    proxy.heal();
    assert_closed(
        &mut open,
        "a connection whose bytes the blackhole discarded",
    );
    assert_closed(&mut fresh, "a connection accepted by the blackhole");
}

#[test]
fn a_dropped_ack_reaches_the_upstream_and_a_retry_after_healing_gets_its_answer() {
    let upstream = LoopbackServer::echo();
    let proxy = Proxy::start(upstream.address());
    let mut first = connect(proxy.address());
    round_trip(&mut first, b"hello\n");

    proxy.inject(FaultKind::DropAcks);
    let received_before = upstream.received();
    let request = b"produce 7\n";
    first
        .write_all(request)
        .expect("the request is written through the fault");
    // The request got through and was acted on…
    poll_until(
        "the request reaching the upstream through DropAcks",
        EVENTUALLY,
        || (upstream.received() >= received_before + request.len() as u64).then_some(()),
    );
    // …and its acknowledgement came back and was dropped — observed, so the
    // silence below is a reply thrown away and not one that never came.
    poll_until(
        "the upstream's reply being read and dropped",
        EVENTUALLY,
        || (proxy.stats().discarded_from_upstream >= request.len() as u64).then_some(()),
    );
    assert_silent(
        &mut first,
        "the connection whose acknowledgement was dropped",
    );

    // Healing closes the connection that lost its acknowledgement rather
    // than relaying on from mid-stream, and the retry — on a new connection,
    // the way a client recovers — gets its answer.
    proxy.heal();
    assert_closed(
        &mut first,
        "the connection whose acknowledgement was dropped",
    );
    let mut retry = connect(proxy.address());
    round_trip(&mut retry, request);
    assert_eq!(
        upstream.received(),
        received_before + 2 * request.len() as u64,
        "the upstream must have seen the request exactly twice: the original and the retry"
    );
}

#[test]
fn a_cut_severs_open_connections_and_closes_new_ones_before_they_reach_the_upstream() {
    let upstream = LoopbackServer::echo();
    let proxy = Proxy::start(upstream.address());
    let mut open = connect(proxy.address());
    round_trip(&mut open, b"before\n");
    let connections_before = upstream.accepted();
    assert_eq!(
        connections_before, 1,
        "premise: the healthy relay reached the upstream exactly once"
    );

    proxy.inject(FaultKind::Cut);
    assert_closed(&mut open, "a connection open when the cut came");
    // The kernel completes the handshake before the proxy's `accept`, so the
    // connect itself succeeds; what the cut does is close it at once.
    let mut fresh = connect(proxy.address());
    let _ = fresh.write_all(b"after\n");
    assert_closed(&mut fresh, "a connection accepted during the cut");
    assert_eq!(
        upstream.accepted(),
        connections_before,
        "a connection accepted during the cut reached the upstream"
    );
    assert!(
        proxy.stats().severed >= 2,
        "the proxy did not count both connections it cut: {:?}",
        proxy.stats()
    );
}

// --- scraping -------------------------------------------------------------------

#[test]
fn a_scrape_returns_only_a_successful_body_and_decodes_a_chunked_one() {
    let plain = LoopbackServer::canned(
        b"HTTP/1.1 200 OK\r\ncontent-length: 18\r\nconnection: close\r\n\r\nqip_test_metric 1\n",
        None,
    );
    assert_eq!(
        scrape::get(plain.address(), "/metrics", EVENTUALLY)
            .expect("premise: a plain 200 is scraped"),
        "qip_test_metric 1\n",
        "premise: a plain 200's body comes back whole"
    );

    // An error whose body looks like exposition: returned as a body, a suite
    // would read a series from an error page and never know.
    let failing = LoopbackServer::canned(
        b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 18\r\nconnection: close\r\n\r\nqip_test_metric 1\n",
        None,
    );
    let error = scrape::get(failing.address(), "/metrics", EVENTUALLY)
        .expect_err("a 500 was returned as a metrics body");
    assert!(
        error.to_string().contains("500"),
        "the refusal did not name the status: {error}"
    );

    let chunked = LoopbackServer::canned(
        b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n7\r\nqip_tes\r\nb\r\nt_metric 1\n\r\n0\r\n\r\n",
        None,
    );
    assert_eq!(
        scrape::get(chunked.address(), "/metrics", EVENTUALLY).expect("a chunked 200 is scraped"),
        "qip_test_metric 1\n",
        "a chunked body came back with its framing still in it"
    );
}

#[test]
fn a_peer_that_trickles_its_answer_cannot_hold_a_scrape_past_its_deadline() {
    // One byte every 20ms: 240 bytes take nearly five seconds to arrive, and
    // no single read ever waits anywhere near the scrape's 300ms.
    let body = "x".repeat(200);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let trickler = LoopbackServer::canned(response.as_bytes(), Some(Duration::from_millis(20)));

    // Premise: the peer is answering — two reads in a row each get a byte
    // well inside the window — so only a deadline on the whole exchange,
    // not one per read, can stop the scrape.
    let mut raw = connect(trickler.address());
    raw.write_all(b"GET /metrics HTTP/1.1\r\n\r\n")
        .expect("write a request to the trickling peer");
    raw.set_read_timeout(Some(SILENCE))
        .expect("set a read timeout");
    let mut byte = [0u8; 1];
    for _ in 0..2 {
        assert_eq!(
            raw.read(&mut byte)
                .expect("premise: the trickling peer is answering"),
            1,
            "premise: the trickling peer is answering"
        );
    }
    drop(raw);

    let started = Instant::now();
    let outcome = scrape::get(trickler.address(), "/metrics", SILENCE);
    let elapsed = started.elapsed();
    let error = outcome.expect_err(
        "a scrape of a peer trickling its answer returned a body; its deadline bounded each read, \
         not the exchange",
    );
    assert_eq!(
        error.kind(),
        ErrorKind::TimedOut,
        "expected the scrape's own deadline, got {error:?}"
    );
    assert!(
        elapsed < SILENCE * 4,
        "the scrape took {elapsed:?} against a {SILENCE:?} deadline"
    );
}

fn sample_bits(exposition: &str, series: &str) -> Option<u64> {
    scrape::sample(exposition, series).map(f64::to_bits)
}

#[test]
fn a_sample_is_read_by_its_whole_series_never_by_a_prefix() {
    let exposition = "# HELP qip_edge_orders_total Orders placed.\n\
                      # TYPE qip_edge_orders_total counter\n\
                      qip_edge_orders_total_expired 5\n\
                      qip_edge_orders_total 3\n\
                      qip_edge_journal_pressure{state=\"narrowed\"} 0\n\
                      qip_edge_journal_pressure{state=\"normal\"} 1\n";
    // Premise: the first sample's series begins with the whole of the one
    // asked for, so a prefix match would read 5 and not 3.
    let first_sample = exposition
        .lines()
        .find(|line| !line.starts_with('#'))
        .expect("premise: the exposition has samples");
    assert!(
        first_sample.starts_with("qip_edge_orders_total")
            && !first_sample.starts_with("qip_edge_orders_total "),
        "premise: the longer series must come first"
    );

    assert_eq!(
        sample_bits(exposition, "qip_edge_orders_total"),
        Some(3.0_f64.to_bits()),
        "the counter was read from a longer series that begins with its name"
    );
    assert_eq!(
        sample_bits(exposition, "qip_edge_journal_pressure{state=\"normal\"}"),
        Some(1.0_f64.to_bits()),
        "a labelled series was not read by its whole identifier"
    );
    assert_eq!(
        sample_bits(exposition, "qip_edge_orders"),
        None,
        "a name that is only a prefix of every series matched one"
    );
    assert_eq!(
        sample_bits(exposition, "qip_edge_journal_pressure"),
        None,
        "a bare name matched a labelled series"
    );
}

// --- children -------------------------------------------------------------------

#[test]
fn a_child_is_killed_when_its_handle_drops_even_when_the_test_panics() {
    const CHILD_ROLE: &str = "QIP_ACCEPTANCE_EVENT_FABRIC_HARNESS_CHILD";
    if std::env::var(CHILD_ROLE).is_ok() {
        // Child role: bind a real ephemeral port and announce it in the same
        // shape a sibling binary's banner uses, then block forever so the
        // parent's `Drop` is what ends this process — never a natural exit
        // racing the liveness check below.
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("the child role can bind a loopback port");
        let address = listener
            .local_addr()
            .expect("a bound listener has a local address");
        println!("test-child listening on {address}");
        let _ = std::io::stdout().flush();
        loop {
            std::thread::park();
        }
    }

    let exe = std::env::current_exe()
        .expect("this test binary can read its own executable path to re-invoke itself");
    let mut command = std::process::Command::new(exe);
    command
        .arg("--exact")
        .arg("a_child_is_killed_when_its_handle_drops_even_when_the_test_panics")
        .arg("--nocapture")
        .env(CHILD_ROLE, "1");

    let pid_cell = Arc::new(AtomicU32::new(0));
    let pid_cell_for_closure = Arc::clone(&pid_cell);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut managed = ManagedChild::spawn("test-child", command);
        pid_cell_for_closure.store(managed.pid(), Ordering::SeqCst);

        let address = managed.wait_for_bound_address("listening on ", EVENTUALLY);
        TcpStream::connect(address).unwrap_or_else(|error| {
            panic!("could not connect to the child's announced address {address}: {error}")
        });

        assert!(
            is_alive(managed.pid()),
            "premise: the spawned child (pid {}) must be alive right before dropping its handle",
            managed.pid()
        );
        panic!(
            "deliberate panic while `managed` (pid {}) is still owned by this scope, to prove its \
             Drop runs while the stack unwinds",
            managed.pid()
        );
    }));

    let payload = outcome.expect_err(
        "the inner closure was supposed to panic to exercise the unwind path, and did not",
    );
    let message = panic_message(&*payload);
    assert!(
        message.contains("deliberate panic"),
        "the closure panicked before reaching its deliberate panic, so this run proves nothing \
         about the unwind path: {message}"
    );

    let pid = pid_cell.load(Ordering::SeqCst);
    assert_ne!(
        pid, 0,
        "the child's pid was never recorded before the panic"
    );

    // `ManagedChild::drop` calls `wait`, which blocks until the kernel has
    // reaped the process, so this should hold the moment `catch_unwind`
    // returns; the bounded poll is a margin against scheduling, not the
    // mechanism that makes it true.
    poll_until(
        "the dropped child's process disappearing",
        Duration::from_secs(5),
        || (!is_alive(pid)).then_some(()),
    );
}

// --- loopback fixtures, local to this file ---------------------------------------

/// A loopback TCP server whose every accepted connection runs one handler on
/// its own thread, and which stops — acceptor and connections alike — when
/// dropped.
#[derive(Debug)]
struct LoopbackServer {
    address: SocketAddr,
    tally: Arc<Tally>,
    streams: Arc<Mutex<Vec<TcpStream>>>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    acceptor: Option<JoinHandle<()>>,
}

/// What a [`LoopbackServer`] has seen, for a test to assert what reached the
/// upstream through the proxy.
#[derive(Debug, Default)]
struct Tally {
    stop: AtomicBool,
    accepted: AtomicU64,
    received: AtomicU64,
}

type Handler = Arc<dyn Fn(TcpStream, &Tally) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl LoopbackServer {
    fn start(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let address = listener
            .local_addr()
            .expect("a bound listener has a local address");
        let tally = Arc::new(Tally::default());
        let streams: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let workers: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let acceptor = {
            let tally = Arc::clone(&tally);
            let streams = Arc::clone(&streams);
            let workers = Arc::clone(&workers);
            std::thread::spawn(move || {
                for incoming in listener.incoming() {
                    if tally.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(stream) = incoming else {
                        continue;
                    };
                    tally.accepted.fetch_add(1, Ordering::SeqCst);
                    if let Ok(handle) = stream.try_clone() {
                        lock(&streams).push(handle);
                    }
                    let handler = Arc::clone(&handler);
                    let tally = Arc::clone(&tally);
                    lock(&workers).push(std::thread::spawn(move || handler(stream, &tally)));
                }
            })
        };
        Self {
            address,
            tally,
            streams,
            workers,
            acceptor: Some(acceptor),
        }
    }

    /// Echoes every byte back, counting what it received.
    fn echo() -> Self {
        Self::start(Arc::new(|mut stream: TcpStream, tally: &Tally| {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        tally.received.fetch_add(n as u64, Ordering::SeqCst);
                        if stream.write_all(&buffer[..n]).is_err() {
                            return;
                        }
                    }
                }
            }
        }))
    }

    /// Answers each request with `response`, all at once, or — with `pace` —
    /// one byte at a time, `pace` apart. The pacing sleep is the fixture's
    /// behaviour, a slow peer, not a wait in the test.
    fn canned(response: &[u8], pace: Option<Duration>) -> Self {
        let response = response.to_vec();
        Self::start(Arc::new(move |mut stream: TcpStream, tally: &Tally| {
            let _ = stream.set_read_timeout(Some(EVENTUALLY));
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        tally.received.fetch_add(n as u64, Ordering::SeqCst);
                        request.extend_from_slice(&buffer[..n]);
                        if request.len() > 64 * 1024 {
                            return;
                        }
                    }
                }
            }
            match pace {
                None => {
                    let _ = stream.write_all(&response);
                }
                Some(pace) => {
                    for byte in &response {
                        if tally.stop.load(Ordering::SeqCst)
                            || stream.write_all(std::slice::from_ref(byte)).is_err()
                        {
                            return;
                        }
                        std::thread::sleep(pace);
                    }
                }
            }
            let _ = stream.shutdown(Shutdown::Write);
        }))
    }

    fn address(&self) -> SocketAddr {
        self.address
    }

    fn accepted(&self) -> u64 {
        self.tally.accepted.load(Ordering::SeqCst)
    }

    fn received(&self) -> u64 {
        self.tally.received.load(Ordering::SeqCst)
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.tally.stop.store(true, Ordering::SeqCst);
        // One connection of our own returns the blocking `accept` to find
        // `stop` set.
        if TcpStream::connect_timeout(&self.address, Duration::from_secs(1)).is_ok()
            && let Some(acceptor) = self.acceptor.take()
        {
            let _ = acceptor.join();
        }
        for stream in lock(&self.streams).drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let workers = std::mem::take(&mut *lock(&self.workers));
        for worker in workers {
            let _ = worker.join();
        }
    }
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&address, EVENTUALLY)
        .unwrap_or_else(|error| panic!("cannot connect to {address}: {error}"));
    stream
        .set_write_timeout(Some(EVENTUALLY))
        .expect("set a write timeout");
    stream
}

fn expect_exact(stream: &mut TcpStream, expected: &[u8]) {
    stream
        .set_read_timeout(Some(EVENTUALLY))
        .expect("set a read timeout");
    let mut received = vec![0u8; expected.len()];
    stream.read_exact(&mut received).unwrap_or_else(|error| {
        panic!(
            "expected {:?} back within {EVENTUALLY:?}: {error}",
            String::from_utf8_lossy(expected)
        )
    });
    assert_eq!(
        received,
        expected,
        "expected {:?} back, got {:?}",
        String::from_utf8_lossy(expected),
        String::from_utf8_lossy(&received)
    );
}

fn round_trip(stream: &mut TcpStream, message: &[u8]) {
    stream.write_all(message).expect("write through the relay");
    expect_exact(stream, message);
}

/// The connection stays open and nothing arrives for the whole of
/// [`SILENCE`] — neither an answer nor a close.
fn assert_silent(stream: &mut TcpStream, what: &str) {
    stream
        .set_read_timeout(Some(SILENCE))
        .expect("set a read timeout");
    let started = Instant::now();
    let mut buffer = [0u8; 256];
    match stream.read(&mut buffer) {
        Ok(0) => panic!("{what} was closed; it must stay open and silent"),
        Ok(n) => panic!(
            "{what} answered {:?}; it must never answer",
            String::from_utf8_lossy(&buffer[..n])
        ),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
        Err(error) => panic!("{what} failed with {error} instead of staying silent"),
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= SILENCE - Duration::from_millis(50),
        "{what} gave up after {elapsed:?}, inside its {SILENCE:?} window"
    );
}

/// The connection is closed by the far side — a clean close or a reset —
/// rather than left open or answered.
fn assert_closed(stream: &mut TcpStream, what: &str) {
    stream
        .set_read_timeout(Some(EVENTUALLY))
        .expect("set a read timeout");
    let mut buffer = [0u8; 256];
    match stream.read(&mut buffer) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
            ) => {}
        Ok(n) => panic!(
            "{what} answered {:?}; it must have been closed",
            String::from_utf8_lossy(&buffer[..n])
        ),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            panic!("{what} was still open after {EVENTUALLY:?}; it must have been closed")
        }
        Err(error) => panic!("{what} failed with {error}, which is not a closed connection"),
    }
}
