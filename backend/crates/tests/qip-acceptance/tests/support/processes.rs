//! Resolving and spawning the five slice binaries, and never leaking one.
//!
//! `cargo test -p qip-acceptance` links no sibling app's binary — a suite
//! that wants `qip-api` or `qip-edge-node` running gets nothing from Cargo for
//! free, because neither is a dependency of this crate, only a workspace
//! member. Two failures follow from that if this file did not exist: a suite
//! that just shells out to a hard-coded path silently runs whatever was built
//! there last, possibly by a different lane's `cargo build` against different
//! sources; and a suite that finds nothing there either skips (green over a
//! process nobody ran) or hangs waiting on a port nothing will ever bind.
//!
//! So resolution here always panics rather than returning an `Option` a
//! caller could shrug off: [`resolve_binary`] finds the binary this test
//! binary's own `target/<profile>/` directory holds, and refuses — naming the
//! exact `cargo build` an operator should run — both when it is absent and
//! when it is older than the sources that produced it, found through `cargo
//! metadata` rather than a directory listing for the same reason
//! `architecture.rs` gave up on hand-parsing manifests: renames, workspace
//! inheritance and path-dependency edges are exactly the shapes a text reader
//! gets wrong.

use qip_acceptance::repository_root;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// The five binaries a slice suite may need, by the name the built executable
/// actually has — `qip-cli`'s crate produces a binary called `qip`, so it is
/// the odd one out here and in [`package_for`].
pub(crate) const SIBLING_BINARIES: [&str; 5] = [
    "qip-fabricd",
    "qip-ledgerd",
    "qip-edge-node",
    "qip-api",
    "qip",
];

/// The exact command an operator runs to produce all five, named in every
/// panic this file raises so the fix is one line away from the failure.
pub(crate) const BUILD_COMMAND: &str = "cd backend && cargo build -p qip-fabricd -p qip-ledgerd -p qip-edge-node -p qip-api -p qip-cli --bins";

/// The crate that owns a given binary name, for the `cargo metadata` lookup
/// that decides whether it is stale.
///
/// Panics on anything else: that is a bug in the caller, not a condition a
/// suite should recover from, and is why this is a plain `match` rather than
/// an `Option` the caller has to unwrap anyway.
fn package_for(binary_name: &str) -> &'static str {
    match binary_name {
        "qip-fabricd" => "qip-fabricd",
        "qip-ledgerd" => "qip-ledgerd",
        "qip-edge-node" => "qip-edge-node",
        "qip-api" => "qip-api",
        "qip" => "qip-cli",
        other => panic!(
            "{other} is not one of the five sibling binaries this harness resolves \
             ({SIBLING_BINARIES:?}); teach package_for its owning crate before calling \
             resolve_binary with it"
        ),
    }
}

/// The `target/<profile>/` directory this test binary was itself built into.
///
/// `current_exe()` for a test binary is `.../target/<profile>/deps/<name>-<hash>`;
/// the sibling binaries — not built with a test harness — land one level up,
/// directly in `<profile>/`, so two `parent()` calls reach them.
fn target_directory() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|error| {
        panic!("cannot read this test binary's own executable path: {error}")
    });
    exe.parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| {
            panic!(
                "{} is not nested two directories under a cargo target directory (expected \
                 .../target/<profile>/deps/<binary>); binary resolution assumes cargo's own layout",
                exe.display()
            )
        })
        .to_path_buf()
}

/// Where `resolve_binary` expects to find a sibling binary, whether or not it
/// is actually there yet.
///
/// Exposed so a test can assert its own premise — that the binary really is
/// absent — before it exercises the panic that absence causes.
pub(crate) fn expected_binary_path(binary_name: &str) -> PathBuf {
    target_directory().join(format!("{binary_name}{}", std::env::consts::EXE_SUFFIX))
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or_else(|error| {
            panic!(
                "cannot read the modification time of {}: {error}",
                path.display()
            )
        })
}

// --- cargo metadata, read the same way architecture.rs reads it ------------
//
// A second, independent copy of the same handful of accessors
// `architecture.rs` and `api_boundary.rs` each already carry. That repetition
// is deliberate rather than an oversight: each test binary is its own crate
// root under `cargo test`, `tests/support` is a module rather than a shared
// library, and the alternative — exporting these from `qip-acceptance`'s
// `src/lib.rs` — would make a change to a JSON accessor ripple into every
// acceptance suite in one step instead of the one file that actually needed
// it.

fn workspace_metadata(manifest: &Path) -> serde_json::Value {
    assert!(
        manifest.is_file(),
        "no workspace manifest at {}; staleness cannot be checked against it",
        manifest.display()
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(&cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .unwrap_or_else(|error| panic!("could not run `{cargo} metadata`: {error}"));
    assert!(
        output.status.success(),
        "`{cargo} metadata` exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("`{cargo} metadata` produced JSON this harness cannot parse: {error}")
    })
}

fn packages(metadata: &serde_json::Value) -> &Vec<serde_json::Value> {
    metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .expect("cargo metadata always reports a `packages` array")
}

fn package_name(package: &serde_json::Value) -> String {
    package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("every package has a name")
        .to_string()
}

fn manifest_path_of(package: &serde_json::Value) -> PathBuf {
    PathBuf::from(
        package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .expect("every package has a manifest path"),
    )
}

fn dependencies_of(package: &serde_json::Value) -> &Vec<serde_json::Value> {
    static EMPTY: std::sync::OnceLock<Vec<serde_json::Value>> = std::sync::OnceLock::new();
    package
        .get("dependencies")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| EMPTY.get_or_init(Vec::new))
}

fn dependency_name(dependency: &serde_json::Value) -> String {
    dependency
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("every dependency has a name")
        .to_string()
}

fn is_shipped(dependency: &serde_json::Value) -> bool {
    !matches!(
        dependency.get("kind").and_then(serde_json::Value::as_str),
        Some("dev") | Some("build")
    )
}

/// Every `.rs` file under `directory`, recursively, skipping hidden
/// directories and `target` — a crate's own build output must never be able
/// to make its binary look stale against itself.
fn rust_sources_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let skip = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "target" || name.starts_with('.'));
            if skip {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    found
}

/// The newest modification time among `package`'s own `.rs`/`Cargo.toml`
/// files and those of every in-workspace path dependency it ships — the set
/// the constraint names, found by the same shipped-edge, `kind`-based walk
/// `architecture.rs`'s `dependency_graph` uses, so a rename or an inherited
/// `[workspace.dependencies]` alias cannot hide a dependency from this walk
/// any more than it can hide one from that file's boundary tests.
fn newest_source_mtime(manifest: &Path, package: &str) -> SystemTime {
    let metadata = workspace_metadata(manifest);
    let pkgs = packages(&metadata);
    let members: BTreeSet<String> = pkgs.iter().map(package_name).collect();
    assert!(
        members.contains(package),
        "{package} is not a package `cargo metadata --manifest-path {}` reports; its sources \
         cannot be found this way",
        manifest.display()
    );

    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut manifests: BTreeMap<String, PathBuf> = BTreeMap::new();
    for pkg in pkgs {
        let name = package_name(pkg);
        let mut edges = BTreeSet::new();
        for dependency in dependencies_of(pkg) {
            if !is_shipped(dependency) {
                continue;
            }
            let dep_name = dependency_name(dependency);
            if members.contains(&dep_name) {
                edges.insert(dep_name);
            }
        }
        manifests.insert(name.clone(), manifest_path_of(pkg));
        graph.insert(name, edges);
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![package.to_string()];
    seen.insert(package.to_string());
    while let Some(current) = stack.pop() {
        for dependency in graph.get(&current).into_iter().flatten() {
            if seen.insert(dependency.clone()) {
                stack.push(dependency.clone());
            }
        }
    }

    let mut newest = SystemTime::UNIX_EPOCH;
    for crate_name in &seen {
        let crate_manifest = manifests
            .get(crate_name)
            .unwrap_or_else(|| panic!("no manifest path recorded for {crate_name}"));
        newest = newest.max(mtime(crate_manifest));
        let directory = crate_manifest
            .parent()
            .unwrap_or_else(|| panic!("{} has no parent directory", crate_manifest.display()));
        for file in rust_sources_under(directory) {
            newest = newest.max(mtime(&file));
        }
    }
    newest
}

// --- resolution, the part a suite actually calls ----------------------------

/// Refuse `binary` unless it exists and is at least as new as `package`'s
/// sources under `manifest`'s workspace, naming `build_command` in either
/// refusal.
///
/// Split out from [`resolve_binary`] so the staleness rule can be proven
/// against a disposable temporary workspace rather than against this
/// repository's own binaries, whose freshness a test cannot control.
pub(crate) fn resolve_checked(
    manifest: &Path,
    package: &str,
    binary: &Path,
    build_command: &str,
) -> PathBuf {
    if !binary.is_file() {
        panic!(
            "{package}'s binary is missing at {}; run `{build_command}`",
            binary.display()
        );
    }
    let binary_mtime = mtime(binary);
    let newest_source = newest_source_mtime(manifest, package);
    assert!(
        binary_mtime >= newest_source,
        "{package}'s binary at {} was built at {binary_mtime:?}, older than its sources (newest \
         change {newest_source:?}); run `{build_command}`",
        binary.display()
    );
    binary.to_path_buf()
}

/// Resolve one of the five sibling binaries against this repository's
/// `backend` workspace, refusing rather than skipping when it is missing or
/// stale.
pub(crate) fn resolve_binary(binary_name: &str) -> PathBuf {
    let package = package_for(binary_name);
    let manifest = repository_root().join("backend/Cargo.toml");
    let binary = expected_binary_path(binary_name);
    resolve_checked(&manifest, package, &binary, BUILD_COMMAND)
}

/// The trailing whitespace-separated token of a startup banner, parsed as a
/// socket address.
///
/// Every sibling binary's banner differs in its prefix — `"qip-api listening
/// on 127.0.0.1:54321"`, `"qip-edge-node: health on 127.0.0.1:54321"` — and
/// agrees only on ending with the address, because the port is `:0` until the
/// operating system assigns one and nothing printed it in advance.
pub(crate) fn parse_bound_address(line: &str) -> SocketAddr {
    line.trim()
        .rsplit(' ')
        .next()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| {
            panic!("{line:?} has no whitespace-separated token to parse an address from")
        })
        .parse()
        .unwrap_or_else(|error| {
            panic!("{line:?}'s trailing token is not a socket address: {error}")
        })
}

// --- spawning, and never leaking one ----------------------------------------

static NEXT_CHILD_ID: AtomicU64 = AtomicU64::new(0);

/// A spawned process, SIGKILLed and reaped when this value drops — panic
/// unwinding through the scope that owns it included, since that unwind runs
/// every live value's `Drop` on its way out and this one is no exception.
///
/// stdout and stderr are captured to files rather than inherited, so a
/// failing suite can print exactly what the process said without racing the
/// parent's own output for the terminal.
pub(crate) struct ManagedChild {
    name: String,
    child: std::process::Child,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl std::fmt::Debug for ManagedChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedChild")
            .field("name", &self.name)
            .field("pid", &self.child.id())
            .field("stdout_path", &self.stdout_path)
            .field("stderr_path", &self.stderr_path)
            .finish()
    }
}

impl ManagedChild {
    /// Spawn `command`, capturing its stdout and stderr under a fresh
    /// directory named for `name` so two children of the same binary never
    /// collide.
    pub(crate) fn spawn(name: &str, mut command: std::process::Command) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "qip-acceptance-{name}-{}-{}",
            std::process::id(),
            NEXT_CHILD_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&directory)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", directory.display()));
        let stdout_path = directory.join("stdout.log");
        let stderr_path = directory.join("stderr.log");
        let stdout = std::fs::File::create(&stdout_path)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", stdout_path.display()));
        let stderr = std::fs::File::create(&stderr_path)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", stderr_path.display()));
        let child = command
            .stdout(std::process::Stdio::from(stdout))
            .stderr(std::process::Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|error| panic!("cannot spawn {name}: {error}"));
        Self {
            name: name.to_string(),
            child,
            stdout_path,
            stderr_path,
        }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Poll the child's captured stdout for a line containing `needle`,
    /// returning it whole.
    ///
    /// A bounded poll rather than a fixed sleep: the moment a process bound
    /// to `:0` announces its real address cannot be predicted, only waited
    /// for, and the deadline is what keeps a process that never prints one
    /// from hanging the suite instead of failing it.
    pub(crate) fn wait_for_line(&self, needle: &str, deadline: Duration) -> String {
        let started = Instant::now();
        loop {
            if let Ok(content) = std::fs::read_to_string(&self.stdout_path)
                && let Some(line) = content.lines().find(|line| line.contains(needle))
            {
                return line.to_string();
            }
            if started.elapsed() >= deadline {
                panic!(
                    "{}'s stdout at {} never printed a line containing {needle:?} within \
                     {deadline:?}; stderr is at {}",
                    self.name,
                    self.stdout_path.display(),
                    self.stderr_path.display()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        // SIGKILL unconditionally, then reap. `kill` on an already-exited
        // child returns an error this ignores on purpose: the property that
        // matters is that nothing this harness spawned outlives the value
        // that owns it, not that the kill call itself always succeeds.
        // `wait` afterwards is what prevents a zombie surviving the process
        // that ran the test.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
