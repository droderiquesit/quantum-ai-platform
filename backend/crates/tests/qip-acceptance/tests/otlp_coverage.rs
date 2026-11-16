//! R12-OBS-9140 (ADR 0682): every deployable binary exports OTLP through the
//! one drain, or is named, with its reason, on a committed list.
//!
//! The requirement says a binary without an exporter fails the check. Before
//! this, "without an exporter" was a thing nobody could enumerate: the three
//! central roots and the fabric daemon adopted the drain, the ledger and the
//! reflex node did not, and the only record was prose in a register row.
//! A binary added tomorrow with neither would have passed every test.

use std::collections::BTreeSet;

/// Binaries that export no OTLP, and why. The row stays open while this list
/// is non-empty; an entry is removed by adopting the drain, never by editing
/// the reason.
const EXPORTERLESS: [(&str, &str); 3] = [
    (
        "qip-edge-node",
        "the reflex closure: a drain puts a thread that performs network I/O beside the \
         decision loop, qip-telemetry-drain is a Lane 1 crate and ADR 0116 says a second \
         hosted Lane 1 crate reopens the sidecar; waits on that record (ADR 0682)",
    ),
    (
        "qip-web",
        "serves the public front door and holds no trading state; its telemetry surface \
         is not decided and no ADR names it, so it is listed rather than assumed",
    ),
    (
        "qip-cli",
        "a short-lived operator command, not a process anything operates; an exporter on \
         a command that exits in seconds would only ever lose its last pass",
    ),
];

fn app_directories() -> BTreeSet<String> {
    let root = qip_acceptance::repository_root().join("backend/crates/apps");
    std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", root.display()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect()
}

fn depends_on_the_drain(app: &str) -> bool {
    qip_acceptance::read(&format!("backend/crates/apps/{app}/Cargo.toml"))
        .lines()
        .any(|line| line.trim_start().starts_with("qip-telemetry-drain"))
}

/// Mutation: not run (nothing was compiled). Removing
/// `qip-telemetry-drain.workspace = true` from qip-ledgerd's manifest leaves
/// it on neither side, and the first assertion names it.
#[test]
fn every_binary_exports_through_the_drain_or_is_named_on_the_exporterless_list() {
    let apps = app_directories();
    assert!(
        apps.len() >= 8 && apps.contains("qip-api"),
        "premise: the scan reached the apps directory, found {apps:?}"
    );
    let named: BTreeSet<&str> = EXPORTERLESS.iter().map(|(name, _)| *name).collect();
    let unaccounted: Vec<&String> = apps
        .iter()
        .filter(|app| !depends_on_the_drain(app) && !named.contains(app.as_str()))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "{unaccounted:?} link no exporter and are not on the exporterless list; adopt \
         qip-telemetry-drain or name the binary and the reason"
    );
}

/// A stale entry is as wrong as a missing one: a binary that has adopted the
/// drain and is still listed tells a reader it exports nothing.
/// Mutation: not run. Adding the drain to qip-edge-node's manifest trips it.
#[test]
fn a_binary_on_the_exporterless_list_really_has_no_exporter() {
    let apps = app_directories();
    for (name, reason) in EXPORTERLESS {
        assert!(apps.contains(name), "{name} is listed and is not an app");
        assert!(!reason.trim().is_empty(), "{name} is listed with no reason");
        assert!(
            !depends_on_the_drain(name),
            "{name} depends on qip-telemetry-drain and is still on the exporterless list"
        );
    }
}

/// The ledger adopts the drain and spawns it from its composition root, as
/// the fabric daemon does. Mutation: not run. Deleting the `spawn(` call in
/// main.rs leaves the module adopted and nothing running it.
#[test]
fn the_ledger_daemon_spawns_the_drain_from_its_composition_root() {
    assert!(depends_on_the_drain("qip-ledgerd"));
    let main = qip_acceptance::read("backend/crates/apps/qip-ledgerd/src/main.rs");
    assert!(
        main.contains("qip_ledgerd::openobserve::spawn("),
        "qip-ledgerd's main.rs no longer spawns the drain"
    );
}
