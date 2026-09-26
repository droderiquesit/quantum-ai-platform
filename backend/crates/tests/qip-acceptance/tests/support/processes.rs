//! Resolving and spawning the five slice binaries, and never leaking one.
//!
//! `cargo test -p qip-acceptance` links no sibling app's binary — a suite
//! that wants `qip-api` or `qip-edge-node` running gets nothing from Cargo for
//! free, because neither is a dependency of this crate, only a workspace
//! member. Two failures follow from that if this file did not exist: a suite
//! that shells out to a hard-coded path silently runs whatever was built there
//! last, possibly against different sources; and a suite that finds nothing
//! there either skips (green over a process nobody ran) or hangs waiting on a
//! port nothing will ever bind.
//!
//! So resolution here always panics rather than returning an `Option` a
//! caller could shrug off: [`resolve_binary`] finds the binary this test
//! binary's own `target/<profile>/` directory holds, and refuses — naming the
//! exact `cargo build` an operator should run — both when it is absent and
//! when it is stale.
//!
//! # What "stale" means, and why it is cargo's own record
//!
//! A binary is stale when a file it was built from changed after it was
//! linked. The list of those files is the dep-info cargo writes beside every
//! binary `cargo build` links, `target/<profile>/<binary>.d`: the binary's own
//! sources, every in-workspace crate it links — build-dependencies included —
//! and every file an `include_str!` or a build script's `rerun-if-changed`
//! named.
//!
//! The obvious rule — the newest `.rs` or `Cargo.toml` anywhere under the
//! crate's directory and its path dependencies' — is wrong in the direction
//! that matters. Cargo relinks nothing for an edit under `tests/`, `benches/`
//! or `examples/`, for an out-of-line `#[cfg(test)]` module, or for a comment
//! in a manifest, so a refusal raised on any of them names a build command
//! that cannot clear it; any edit to `qip-kernel/tests/*.rs` would refuse
//! `qip-api` in every suite until someone touched the binary by hand. A
//! refusal its own remedy cannot clear teaches the operator to `touch` the
//! binary, and then the check guards nothing. Measured, not argued: on the
//! pinned 1.94.1 toolchain, an edit to `tests/it.rs`, to an out-of-line
//! `#[cfg(test)]` module and to a manifest comment each left the binary's
//! mtime unchanged across `cargo build`, and none of those files was in its
//! dep-info. `a_binary_older_than_its_crates_sources_is_refused_naming_the_build_command`
//! re-measures the integration-test and manifest-comment cases against a real
//! build on every run.
//!
//! Manifests still count, through what they say rather than when they were
//! saved: `cargo metadata --format-version 1` names every in-workspace crate
//! the package reaches through an unconditional normal or build dependency,
//! and a binary whose dep-info lists nothing from one of them was linked
//! before the manifest named it — which `cargo build` does clear.
//!
//! What this does not see, stated rather than left to be discovered: a change
//! cargo acts on that moves no listed file — a registry version in
//! `Cargo.lock`, or a profile, edition or feature setting in a manifest. A
//! check on those files' mtimes would also fire on the edits cargo ignores,
//! and so could not tell the two apart.

use qip_acceptance::repository_root;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
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
/// refusal this file raises so the fix is one line away from the failure.
pub(crate) const BUILD_COMMAND: &str = "cd backend && cargo build -p qip-fabricd -p qip-ledgerd -p qip-edge-node -p qip-api -p qip-cli --bins";

/// How often a bounded wait in this file looks again. The deadline, not this,
/// is what bounds the wait; this only bounds how late the wait notices.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How much of a child's captured stdout or stderr is printed when a test
/// fails with it running: the end, which is where a process says why it
/// stopped, and bounded so a chatty process cannot bury the assertion.
const REPORT_TAIL_BYTES: usize = 64 * 1024;

/// The crate that owns a given binary name, for the `cargo metadata` lookup
/// that decides whether it is stale.
///
/// Panics on anything else: that is a bug in the caller, not a condition a
/// suite should recover from.
fn package_for(binary_name: &str) -> &'static str {
    match binary_name {
        "qip-fabricd" => "qip-fabricd",
        "qip-ledgerd" => "qip-ledgerd",
        "qip-edge-node" => "qip-edge-node",
        "qip-api" => "qip-api",
        "qip" => "qip-cli",
        other => panic!(
            "{other} is not one of the five sibling binaries this harness resolves \
             ({SIBLING_BINARIES:?}); teach package_for its owning crate before resolving it"
        ),
    }
}

/// The `target/<profile>/` directory this test binary was itself built into.
///
/// `current_exe()` for a test binary is `.../target/<profile>/deps/<name>-<hash>`;
/// the sibling binaries — not built with a test harness — land one level up,
/// directly in `<profile>/`, so two `parent()` calls reach them.
pub(crate) fn target_directory() -> PathBuf {
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

// --- resolution, the part a suite actually calls ----------------------------

/// Resolve one of the five sibling binaries from this test binary's own
/// `target/<profile>/` directory, refusing rather than skipping when it is
/// missing or stale.
pub(crate) fn resolve_binary(binary_name: &str) -> PathBuf {
    resolve_binary_in(&target_directory(), binary_name)
}

/// [`resolve_binary`] against a directory the caller names instead of this
/// test binary's own.
///
/// Exists so the refusals can be proven against a directory whose contents a
/// test controls. The shared `target/<profile>/` directory is not one: the
/// build line every §8 suite runs first puts all five binaries there, and so
/// does `cargo test --workspace` for any sibling with integration tests of its
/// own, so a test whose premise is "the binary is absent from the target
/// directory" is a test with a date on which it starts failing.
pub(crate) fn resolve_binary_in(directory: &Path, binary_name: &str) -> PathBuf {
    let package = package_for(binary_name);
    let manifest = repository_root().join("backend/Cargo.toml");
    let binary = directory.join(format!("{binary_name}{}", std::env::consts::EXE_SUFFIX));
    resolve_checked(&manifest, package, &binary, BUILD_COMMAND)
}

/// Refuse `binary` unless it exists and nothing it was built from — by
/// `package` in the workspace `manifest` names — changed after it was linked,
/// naming `build_command` in either refusal.
///
/// Split out so the rule can be proven against a disposable workspace a test
/// builds for real, including the half that matters as much as the refusal:
/// that the command it names clears it.
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
    if let Err(reason) = judge_freshness(manifest, package, binary) {
        panic!(
            "{package}'s binary at {} is stale: {reason}; run `{build_command}`",
            binary.display()
        );
    }
    binary.to_path_buf()
}

fn modified(path: &Path) -> std::io::Result<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified())
}

/// `Ok` when every file `binary` was built from predates its link and its
/// dep-info covers every crate its manifest builds it from; otherwise why not.
fn judge_freshness(manifest: &Path, package: &str, binary: &Path) -> Result<(), String> {
    let linked = modified(binary)
        .map_err(|error| format!("its modification time cannot be read ({error})"))?;
    let dep_info = dep_info_path(binary);
    let sources = dep_info_sources(&dep_info, binary)?;

    let mut changed: Vec<(SystemTime, &Path)> = Vec::new();
    for source in &sources {
        if source.is_relative() {
            return Err(format!(
                "{} lists {} relative to a base this harness cannot see (is \
                 `build.dep-info-basedir` set?); it reads absolute paths only rather than guess \
                 which file was meant",
                dep_info.display(),
                source.display()
            ));
        }
        match modified(source) {
            Ok(when) if when > linked => changed.push((when, source)),
            Ok(_) => {}
            Err(_) => {
                return Err(format!(
                    "{} lists {}, which no longer exists",
                    dep_info.display(),
                    source.display()
                ));
            }
        }
    }
    if let Some((when, newest)) = changed.iter().max() {
        return Err(format!(
            "{} of the {} files it was built from changed after it was linked at {linked:?}; the \
             newest is {} at {when:?}",
            changed.len(),
            sources.len(),
            newest.display()
        ));
    }

    let crates = WorkspaceCrates::read(manifest);
    let required = crates.built_from(package, manifest);
    let covered: BTreeSet<&str> = sources
        .iter()
        .filter_map(|source| crates.owner_of(source))
        .collect();
    let absent: Vec<&str> = required
        .iter()
        .map(String::as_str)
        .filter(|name| !covered.contains(name))
        .collect();
    if !absent.is_empty() {
        return Err(format!(
            "its manifests build it from {absent:?}, and {} lists nothing from there — a \
             manifest named that crate after the link",
            dep_info.display()
        ));
    }
    Ok(())
}

// --- the dep-info cargo writes beside a binary --------------------------------

/// `target/<profile>/qip-api` → `target/<profile>/qip-api.d`, and on Windows
/// `qip-api.exe` → `qip-api.d`, which is where cargo writes it.
fn dep_info_path(binary: &Path) -> PathBuf {
    binary.with_extension("d")
}

/// The files `binary`'s dep-info says it was built from.
///
/// The format is make's: `<target>: <dependency> <dependency> ...`, one line
/// per output, with a space inside a path escaped as `\ ` — the only escape
/// cargo writes. Lines are matched to `binary` by file name rather than by
/// full path, because `current_exe()` resolves symbolic links and cargo writes
/// the target directory as configured, and a harness that compared the two
/// would refuse a fresh binary no build could make acceptable.
fn dep_info_sources(dep_info: &Path, binary: &Path) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read_to_string(dep_info).map_err(|error| {
        format!(
            "its dep-info {} cannot be read ({error}); cargo writes one beside every binary \
             `cargo build` links, and a binary without one — left by `cargo test`, which does not \
             write it, or copied in — cannot say what it was built from",
            dep_info.display()
        )
    })?;
    let wanted = binary.file_name();
    let mut sources = BTreeSet::new();
    let mut described = false;
    for line in text.lines() {
        let tokens = dep_info_tokens(line);
        let Some((first, rest)) = tokens.split_first() else {
            continue;
        };
        let Some(target) = first.strip_suffix(':') else {
            continue;
        };
        if Path::new(target).file_name() != wanted {
            continue;
        }
        described = true;
        sources.extend(rest.iter().map(PathBuf::from));
    }
    if !described {
        return Err(format!(
            "its dep-info {} has no line for {}",
            dep_info.display(),
            binary.display()
        ));
    }
    if sources.is_empty() {
        return Err(format!(
            "its dep-info {} lists no file it was built from",
            dep_info.display()
        ));
    }
    Ok(sources.into_iter().collect())
}

/// Split one dep-info line on unescaped whitespace, unescaping `\ `.
fn dep_info_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                current.push(' ');
                chars.next();
            }
            ' ' | '\t' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// --- the workspace, as `cargo metadata` reports it ---------------------------
//
// Read through cargo's own structured output, the way `architecture.rs` and
// `api_boundary.rs` read it, because renames, `[workspace.dependencies]`
// inheritance and path edges are exactly the shapes a hand-written manifest
// reader gets wrong. A second copy of their few accessors rather than a shared
// export from `src/lib.rs`, so a change here ripples into this harness and not
// into every acceptance suite at once.

/// Every workspace member's directory, and the in-workspace crates each one
/// links unconditionally.
#[derive(Debug)]
struct WorkspaceCrates {
    /// Canonical, so a dep-info path and a manifest path that reach the same
    /// file through different symbolic links still agree on its owner.
    directories: BTreeMap<String, PathBuf>,
    edges: BTreeMap<String, BTreeSet<String>>,
}

impl WorkspaceCrates {
    fn read(manifest: &Path) -> Self {
        let metadata = workspace_metadata(manifest);
        let packages = metadata
            .get("packages")
            .and_then(serde_json::Value::as_array)
            .expect("cargo metadata always reports a `packages` array");
        let members: BTreeSet<String> = packages.iter().map(package_name).collect();

        let mut directories = BTreeMap::new();
        let mut edges = BTreeMap::new();
        for package in packages {
            let name = package_name(package);
            let manifest_path = PathBuf::from(
                package
                    .get("manifest_path")
                    .and_then(serde_json::Value::as_str)
                    .expect("every package has a manifest path"),
            );
            let directory = manifest_path
                .parent()
                .unwrap_or_else(|| panic!("{} has no parent directory", manifest_path.display()));
            let directory = std::fs::canonicalize(directory).unwrap_or_else(|error| {
                panic!("cannot canonicalize {}: {error}", directory.display())
            });
            let linked: BTreeSet<String> = package
                .get("dependencies")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter(|dependency| is_unconditional_link(dependency))
                .filter_map(|dependency| dependency.get("name")?.as_str())
                .filter(|dependency| members.contains(*dependency))
                .map(str::to_string)
                .collect();
            directories.insert(name.clone(), directory);
            edges.insert(name, linked);
        }
        Self { directories, edges }
    }

    /// `package` and every workspace crate it reaches through unconditional
    /// normal and build edges, transitively.
    fn built_from(&self, package: &str, manifest: &Path) -> BTreeSet<String> {
        assert!(
            self.edges.contains_key(package),
            "{package} is not a package `cargo metadata --manifest-path {}` reports; what it is \
             built from cannot be found this way",
            manifest.display()
        );
        let mut seen = BTreeSet::from([package.to_string()]);
        let mut stack = vec![package.to_string()];
        while let Some(current) = stack.pop() {
            for dependency in self.edges.get(&current).into_iter().flatten() {
                if seen.insert(dependency.clone()) {
                    stack.push(dependency.clone());
                }
            }
        }
        seen
    }

    /// The workspace member whose directory holds `source` most closely — the
    /// longest match, so a crate nested inside another's directory is not
    /// credited to its parent.
    fn owner_of(&self, source: &Path) -> Option<&str> {
        let source = std::fs::canonicalize(source).ok()?;
        self.directories
            .iter()
            .filter(|(_, directory)| source.starts_with(directory))
            .max_by_key(|(_, directory)| directory.components().count())
            .map(|(name, _)| name.as_str())
    }
}

/// A normal or build edge the manifest does not make conditional.
///
/// Build edges count because a build-dependency's sources reach the binary
/// through its build script and cargo lists them in the dep-info. Dev edges do
/// not, because no binary links one. Optional and platform-specific edges do
/// not, because they may not be compiled at all, and requiring their sources
/// in the dep-info would refuse a binary no build could make acceptable.
fn is_unconditional_link(dependency: &serde_json::Value) -> bool {
    let kind = dependency.get("kind").and_then(serde_json::Value::as_str);
    let optional = dependency
        .get("optional")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let platform_specific = dependency
        .get("target")
        .is_some_and(|target| !target.is_null());
    kind != Some("dev") && !optional && !platform_specific
}

fn package_name(package: &serde_json::Value) -> String {
    package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("every package has a name")
        .to_string()
}

fn workspace_metadata(manifest: &Path) -> serde_json::Value {
    assert!(
        manifest.is_file(),
        "no workspace manifest at {}; staleness cannot be checked against it",
        manifest.display()
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(&cargo)
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

// --- startup banners ----------------------------------------------------------

/// The first complete line of `content` containing `needle`, without its line
/// ending.
///
/// Complete means terminated by `\n`. A banner read while the child is still
/// writing it — `"qip-api listening on 127.0.0.1:5"` of what will be
/// `127.0.0.1:54321` — is a valid address and the wrong one, and a suite that
/// took it would talk to a port nothing bound.
pub(crate) fn complete_line_containing<'a>(content: &'a str, needle: &str) -> Option<&'a str> {
    content
        .split_inclusive('\n')
        .filter_map(|line| line.strip_suffix('\n'))
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .find(|line| line.contains(needle))
}

/// The trailing whitespace-separated token of a startup banner, parsed as a
/// socket address.
///
/// Every sibling binary's banner differs in its prefix — `"qip-api listening
/// on 127.0.0.1:54321"`, `"qip-edge-node: health on 127.0.0.1:54321"` — and
/// agrees only on ending with the address, because the port is `:0` until the
/// operating system assigns one and nothing printed it in advance.
pub(crate) fn parse_bound_address(line: &str) -> SocketAddr {
    line.split_whitespace()
        .next_back()
        .unwrap_or_else(|| panic!("{line:?} has no token to parse an address from"))
        .parse()
        .unwrap_or_else(|error| {
            panic!("{line:?}'s trailing token is not a socket address: {error}")
        })
}

// --- scratch directories -------------------------------------------------------

static NEXT_SCRATCH_ID: AtomicU64 = AtomicU64::new(0);

/// A fresh directory under the system temporary directory, removed when this
/// value drops — unless the test is failing, when it is kept and its path
/// printed, because a fabric's data directory is the evidence.
#[derive(Debug)]
pub(crate) struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    pub(crate) fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "qip-acceptance-{}-{}-{}",
            sanitise(label),
            std::process::id(),
            NEXT_SCRATCH_ID.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", path.display()));
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("kept {} for inspection", self.path.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn sanitise(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// --- spawning, and never leaking one ------------------------------------------

/// A spawned process, SIGKILLed and reaped when this value drops — panic
/// unwinding through the scope that owns it included, since that unwind runs
/// every live value's `Drop` on its way out and this one is no exception.
///
/// stdout and stderr are captured to files rather than inherited, so the
/// process's output never races the parent's for the terminal; when the value
/// drops during a panic, the tail of both is printed, so a failure in CI —
/// where the files vanish with the runner — still shows what the process said.
///
/// The limit, stated: a test process that is itself SIGKILLed runs no `Drop`,
/// and its children outlive it. Tying a child's life to its parent's needs
/// `prctl(PR_SET_PDEATHSIG)`, which is `unsafe` FFI this workspace forbids.
pub(crate) struct ManagedChild {
    name: String,
    child: Child,
    directory: PathBuf,
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
    /// Spawn `command` with stdin closed, capturing its stdout and stderr
    /// under a fresh directory named for `name`, so two children of the same
    /// binary never collide.
    pub(crate) fn spawn(name: &str, mut command: Command) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "qip-acceptance-child-{}-{}-{}",
            sanitise(name),
            std::process::id(),
            NEXT_SCRATCH_ID.fetch_add(1, Ordering::SeqCst)
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
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|error| panic!("cannot spawn {name}: {error}"));
        Self {
            name: name.to_string(),
            child,
            directory,
            stdout_path,
            stderr_path,
        }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn stdout_path(&self) -> &Path {
        &self.stdout_path
    }

    pub(crate) fn stderr_path(&self) -> &Path {
        &self.stderr_path
    }

    /// Everything the child has written to stdout so far.
    pub(crate) fn captured_stdout(&self) -> String {
        read_lossy(&self.stdout_path)
    }

    /// Everything the child has written to stderr so far.
    pub(crate) fn captured_stderr(&self) -> String {
        read_lossy(&self.stderr_path)
    }

    /// Poll the child's captured stdout for a complete line containing
    /// `needle`, returning it without its line ending.
    ///
    /// A bounded poll rather than a fixed sleep: the moment a process bound
    /// to `:0` announces its real address cannot be predicted, only waited
    /// for, and the deadline is what keeps a process that never prints one
    /// from hanging the suite instead of failing it. A child that exits first
    /// fails the wait at once rather than at the deadline.
    pub(crate) fn wait_for_line(&mut self, needle: &str, deadline: Duration) -> String {
        let started = Instant::now();
        loop {
            if let Some(line) = complete_line_containing(&self.captured_stdout(), needle) {
                return line.to_string();
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                // The line may have landed between the read above and the exit.
                if let Some(line) = complete_line_containing(&self.captured_stdout(), needle) {
                    return line.to_string();
                }
                panic!(
                    "{} exited ({status}) before printing a complete line containing {needle:?}; \
                     its output is printed below as the handle drops",
                    self.name
                );
            }
            assert!(
                started.elapsed() < deadline,
                "{} printed no complete line containing {needle:?} within {deadline:?}; its \
                 output is printed below as the handle drops",
                self.name
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// [`wait_for_line`](Self::wait_for_line), then [`parse_bound_address`].
    pub(crate) fn wait_for_bound_address(
        &mut self,
        needle: &str,
        deadline: Duration,
    ) -> SocketAddr {
        parse_bound_address(&self.wait_for_line(needle, deadline))
    }

    /// Poll until the child exits, returning its status, or panic at
    /// `deadline` — after which the handle's `Drop` kills it.
    ///
    /// For the short-lived commands a suite runs to completion (a build, a
    /// grant, a verify), where `Command::output` would block without bound.
    pub(crate) fn wait_for_exit(&mut self, deadline: Duration) -> ExitStatus {
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status,
                Ok(None) => {}
                Err(error) => panic!("cannot poll {} for its exit: {error}", self.name),
            }
            assert!(
                started.elapsed() < deadline,
                "{} was still running after {deadline:?}; it is killed and its output printed \
                 below as the handle drops",
                self.name
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn report(&self) -> String {
        format!(
            "--- {name} (pid {pid}) stdout, {stdout_path} ---\n{stdout}\n--- {name} stderr, \
             {stderr_path} ---\n{stderr}\n--- end of {name} ---",
            name = self.name,
            pid = self.child.id(),
            stdout_path = self.stdout_path.display(),
            stdout = tail(&self.stdout_path),
            stderr_path = self.stderr_path.display(),
            stderr = tail(&self.stderr_path),
        )
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
        // Printed through the test harness's own capture, so it appears under
        // the failing test's output and nowhere else.
        if std::thread::panicking() {
            eprintln!("{}", self.report());
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn read_lossy(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn tail(path: &Path) -> String {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return format!("<unreadable: {error}>"),
    };
    if bytes.len() <= REPORT_TAIL_BYTES {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    let omitted = bytes.len() - REPORT_TAIL_BYTES;
    format!(
        "[{omitted} earlier bytes omitted]\n{}",
        String::from_utf8_lossy(&bytes[omitted..])
    )
}
