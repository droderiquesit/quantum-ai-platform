//! The real-process harness ADR 0100 §8 needs, proven against itself.
//!
//! §8's eight tests spawn the five slice binaries — `qip-fabricd`,
//! `qip-ledgerd`, `qip-edge-node`, `qip-api` and `qip-cli`'s `qip` — for real,
//! on free ports, and drive faults between them. None of that is safe to
//! build on faith: a suite that silently skips a missing binary is a suite
//! that has stopped testing anything, a suite that runs a stale one is
//! testing yesterday's code under today's name, and a suite that leaks a
//! child leaves an orphan process behind for whoever runs the next one.
//!
//! This file proves the five properties `support/` exists to hold, one test
//! each, before any suite that needs the five binaries themselves — none of
//! which this packet's `depends_on` names, and none of which this file
//! requires: `qip-fabricd` and `qip-ledgerd` do not exist in this tree yet,
//! and their absence is exactly what
//! `a_missing_sibling_binary_fails_naming_the_build_command_and_is_never_skipped`
//! is testing.

mod support;

/// Extract a human-readable message from a `catch_unwind` payload.
///
/// A `panic!` payload in this codebase is always a `String` or a `&str` — see
/// `gitops.rs`'s identical idiom — never anything else, so those are the only
/// two shapes this looks for.
///
/// Takes the payload already dereferenced (`&*boxed_payload`, never
/// `&boxed_payload`): `Box<dyn Any + Send>` itself also satisfies `Any` via
/// the blanket impl, so a reference to the box coerces straight to `&(dyn Any
/// + Send)` *for the box*, and downcasting that answers "is the payload a
/// `Box`" rather than "what is inside it" — always no, silently.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".to_string())
}

/// Whether a process with this pid still exists.
///
/// No `unsafe` FFI: this platform is deployed only on Linux (GKE, Cloud Run),
/// where `/proc/<pid>` existing is exactly this fact, and `ManagedChild`
/// always `wait`s after `kill` so a killed child is reaped rather than left a
/// zombie whose `/proc` entry would otherwise linger.
fn is_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[test]
fn a_missing_sibling_binary_fails_naming_the_build_command_and_is_never_skipped() {
    // Real absence rather than a fixture: `qip-fabricd` is not a crate this
    // workspace has yet, and even once it is, `cargo test -p qip-acceptance`
    // links no sibling app's binary (that is this packet's own `why`), so its
    // executable will not exist in this worker's target directory unless
    // someone ran the build command by hand.
    let expected_path = support::processes::expected_binary_path("qip-fabricd");
    assert!(
        !expected_path.exists(),
        "premise: {} must not exist for this test to mean anything; if a stray build put a \
         binary there, this test is no longer exercising a missing binary",
        expected_path.display()
    );

    let outcome = std::panic::catch_unwind(|| support::processes::resolve_binary("qip-fabricd"));
    let payload = outcome.expect_err(
        "resolve_binary must refuse a missing sibling binary by panicking, never by returning a \
         path that does not exist and never by silently skipping the caller onward",
    );
    let message = panic_message(&*payload);
    assert!(
        message.contains(support::processes::BUILD_COMMAND),
        "the panic did not name the build command an operator should run: {message:?}"
    );
}

#[test]
fn a_binary_older_than_its_crates_sources_is_refused_naming_the_build_command() {
    let root = std::env::temp_dir().join(format!(
        "qip-acceptance-stale-binary-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("probe/src")).expect("create the temporary workspace");
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"probe\"]\n",
    )
    .expect("write the temporary workspace manifest");
    std::fs::write(
        root.join("probe/Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \
         \"probe\"\npath = \"src/main.rs\"\n",
    )
    .expect("write the probe manifest");
    std::fs::write(root.join("probe/src/main.rs"), "fn main() {}\n")
        .expect("write the probe source");

    let binary = root.join("probe-binary");
    std::fs::write(&binary, b"not a real executable; only its mtime is read")
        .expect("write the fake binary");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&binary)
        .expect("reopen the fake binary to set its mtime")
        .set_modified(old)
        .expect("set the fake binary's mtime an hour into the past");

    // The premise: touching the binary's source *after* the binary landed on
    // disk really did leave the source newer, which is the one fact the named
    // mutation ("check presence only") throws away.
    let source_mtime = std::fs::metadata(root.join("probe/src/main.rs"))
        .and_then(|meta| meta.modified())
        .expect("read the probe source's mtime");
    let binary_mtime = std::fs::metadata(&binary)
        .and_then(|meta| meta.modified())
        .expect("read the fake binary's mtime");
    assert!(
        source_mtime > binary_mtime,
        "premise: the probe's source ({source_mtime:?}) must be newer than its fake binary \
         ({binary_mtime:?}) for this test to be checking staleness at all"
    );

    let manifest = root.join("Cargo.toml");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        support::processes::resolve_checked(&manifest, "probe", &binary, "cargo build -p probe")
    }));
    let payload = outcome.expect_err(
        "resolve_checked must refuse a binary older than its own crate's sources, never accept \
         it on presence alone",
    );
    let message = panic_message(&*payload);
    assert!(
        message.contains("cargo build -p probe"),
        "the panic did not name the build command: {message:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn one_seed_yields_one_fault_schedule() {
    let a = support::proxy::FaultSchedule::seeded(42, 16);
    let b = support::proxy::FaultSchedule::seeded(42, 16);
    assert_eq!(
        a.kinds(),
        b.kinds(),
        "the same seed must produce the same fault schedule every time"
    );

    // The premise that makes the equality above mean something: a different
    // seed really does produce a different schedule, so `seeded` is reading
    // its argument rather than returning a constant.
    let c = support::proxy::FaultSchedule::seeded(43, 16);
    assert_ne!(
        a.kinds(),
        c.kinds(),
        "two different seeds produced the same schedule; the generator may not be reading the \
         seed at all"
    );
}

#[test]
fn a_stalled_upstream_accepts_the_connection_and_never_answers() {
    // A real server first, so the timeout asserted below is evidence the
    // proxy's fault fired and not evidence that `scrape::get` is simply
    // broken against a server that would otherwise have answered.
    let upstream = FakeMetricsServer::start("qip_test_metric 1\n");
    let direct = support::scrape::get(
        upstream.address(),
        "/metrics",
        std::time::Duration::from_secs(2),
    )
    .expect("a real server must answer scrape::get");
    assert!(
        direct.contains("qip_test_metric 1"),
        "the direct scrape did not contain the body the fake server was configured with: \
         {direct:?}"
    );

    let proxy = support::proxy::Proxy::single_fault(
        support::proxy::FaultKind::Stall,
        Some(upstream.address()),
    );
    let started = std::time::Instant::now();
    let result = support::scrape::get(
        proxy.address(),
        "/metrics",
        std::time::Duration::from_millis(300),
    );
    let elapsed = started.elapsed();

    match result {
        Ok(body) => panic!("the stalled proxy answered with {body:?}; it must never answer"),
        Err(error) => {
            assert!(
                matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ),
                "expected a bounded read timeout through the stalled proxy, got {error:?}"
            );
        }
    }
    assert!(
        elapsed >= std::time::Duration::from_millis(250),
        "the scrape through the stalled proxy returned after {elapsed:?}, well under its 300ms \
         timeout; a connection the proxy actually closed returns almost immediately, which is \
         the exact bug (`stall closes the socket`) this test exists to catch"
    );
}

#[test]
fn a_child_is_killed_when_its_handle_drops_even_when_the_test_panics() {
    const CHILD_ROLE: &str = "QIP_ACCEPTANCE_EVENT_FABRIC_HARNESS_CHILD";
    if std::env::var(CHILD_ROLE).is_ok() {
        // Child role: bind a real ephemeral port and announce it in the same
        // shape a sibling binary's startup banner uses, then block forever so
        // the parent's `Drop` is what ends this process — never a natural
        // exit racing the liveness check below.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("the child role can bind a loopback port");
        let address = listener
            .local_addr()
            .expect("a bound listener has a local address");
        println!("test-child listening on {address}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
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

    let pid_cell = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let pid_cell_for_closure = std::sync::Arc::clone(&pid_cell);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let managed = support::processes::ManagedChild::spawn("test-child", command);
        pid_cell_for_closure.store(managed.pid(), std::sync::atomic::Ordering::SeqCst);

        let line = managed.wait_for_line("listening on ", std::time::Duration::from_secs(10));
        let address = support::processes::parse_bound_address(&line);
        std::net::TcpStream::connect(address).unwrap_or_else(|error| {
            panic!("could not connect to the child's announced address {address}: {error}")
        });

        assert!(
            is_alive(managed.pid()),
            "premise: the spawned child (pid {}) must still be alive right before we test that \
             dropping its handle kills it",
            managed.pid()
        );
        panic!(
            "deliberate panic while `managed` (pid {}) is still owned by this scope, to prove \
             its Drop still runs while the stack unwinds",
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

    let pid = pid_cell.load(std::sync::atomic::Ordering::SeqCst);
    assert_ne!(
        pid, 0,
        "the child's pid was never recorded before the panic"
    );

    // Bounded poll, not a fixed sleep: `ManagedChild::drop` calls `wait`,
    // which blocks until the kernel has reaped the process, so this should
    // already be true the moment `catch_unwind` returns — the poll is a
    // margin against scheduling, not the mechanism that makes it true.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while is_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        !is_alive(pid),
        "pid {pid} is still alive after its ManagedChild handle was dropped during a panic; the \
         Drop impl either did not run or did not kill it"
    );
}

// --- a minimal fixture server, local to this file ---------------------------

/// A loopback HTTP server answering every request with the same fixed body.
///
/// Deliberately smaller than `qip-edge-node`'s `TestVenue`: this file needs
/// exactly one canned response, never a script of them, so it does not carry
/// that fixture's request log or per-route matching.
struct FakeMetricsServer {
    address: std::net::SocketAddr,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for FakeMetricsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeMetricsServer")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl FakeMetricsServer {
    fn start(body: &'static str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind a loopback port for the fixture");
        listener
            .set_nonblocking(true)
            .expect("the listener can poll");
        let address = listener
            .local_addr()
            .expect("a bound listener has a local address");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            while !thread_stop.load(std::sync::atomic::Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                        let mut request = [0u8; 1024];
                        let _ = stream.read(&mut request);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            stop,
            handle: Some(handle),
        }
    }

    fn address(&self) -> std::net::SocketAddr {
        self.address
    }
}

impl Drop for FakeMetricsServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
