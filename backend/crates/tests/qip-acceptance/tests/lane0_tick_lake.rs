//! REFLEX-1189: Lane 0 never reads the historical Tick Lake in the order
//! path.
//!
//! The failure this prevents is a reflex pass that consults history — a
//! feature warmed from yesterday's segments, a fill model calibrated on the
//! fly from the lake — and so puts object-store latency, unbounded, between a
//! tick and an order. It would compile, pass every test that runs against an
//! in-memory store, and stall the first time the deployed bucket is slow.
//!
//! Two facts hold that line today, and neither had a test:
//!
//! - the cell, `qip-edge`, cannot link `qip-storage` at all, so nothing on
//!   `Cell::work` can name the lake;
//! - the node, `qip-edge-node`, links `qip-storage` for its journal and
//!   *writes* the lake from the drain thread (`event_fabric::drain::LakeSink`,
//!   ADR 0100 §1), which is off the decision thread by construction — and no
//!   node module calls a lake *read*.
//!
//! Both are read from the tree rather than asserted, so adding a dependency
//! or a read call fails here.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const CELL: &str = "qip-edge";
const NODE: &str = "qip-edge-node";
const STORAGE: &str = "qip-storage";

/// Every way `qip_storage::lake::Lake` hands back stored bytes or a stored
/// manifest's contents. A node module naming one is reading history.
///
/// Until 2026-10-18 this held three names and the lake had grown four more
/// readers — `read_market_history`, `list_market_history` and
/// `pin_market_history` in `lake_history.rs`, and `Lake::verify`, which
/// opens a manifest to check it — so a history read spelled with any of
/// them passed the scan (R12-REFLEX-1189). `verify` is a common method
/// name, so it is judged by [`LAKE_VERIFY`] in a module that names `Lake`
/// rather than listed here, where it would fire on a signature check.
const LAKE_READS: [&str; 6] = [
    "read_canonical",
    "materialize",
    "Materialized",
    "read_market_history",
    "list_market_history",
    "pin_market_history",
];

/// `Lake::verify` reads a stored manifest, and is a reader only in a module
/// that names the lake at all.
const LAKE_VERIFY: &str = "verify";

/// The node modules that may name the lake module at all, and why: the
/// drain's sink writes segments, and `main.rs` names the namespace and the
/// codec it configures that sink with.
const LAKE_WRITERS: [&str; 2] = ["event_fabric/drain.rs", "main.rs"];

fn crate_directories() -> BTreeMap<String, PathBuf> {
    let crates = qip_acceptance::repository_root().join("backend/crates");
    let mut found = BTreeMap::new();
    for group in std::fs::read_dir(&crates)
        .expect("backend/crates is readable")
        .flatten()
    {
        let Ok(members) = std::fs::read_dir(group.path()) else {
            continue;
        };
        for member in members.flatten() {
            if member.path().join("Cargo.toml").is_file() {
                found.insert(
                    member.file_name().to_string_lossy().into_owned(),
                    member.path(),
                );
            }
        }
    }
    found
}

/// The `qip-*` crates a manifest names under `[dependencies]`.
fn normal_dependencies(manifest: &str) -> BTreeSet<String> {
    let mut in_section = false;
    let mut found = BTreeSet::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[dependencies]";
        } else if in_section && line.starts_with("qip-") {
            found.insert(
                line.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect(),
            );
        }
    }
    found
}

/// `root` and everything it links through normal dependencies.
fn closure(root: &str, directories: &BTreeMap<String, PathBuf>) -> BTreeSet<String> {
    let graph: BTreeMap<String, BTreeSet<String>> = directories
        .iter()
        .map(|(name, directory)| {
            let manifest = std::fs::read_to_string(directory.join("Cargo.toml"))
                .expect("a crate has a manifest");
            (name.clone(), normal_dependencies(&manifest))
        })
        .collect();
    let mut seen = BTreeSet::from([root.to_string()]);
    let mut pending = vec![root.to_string()];
    while let Some(next) = pending.pop() {
        for dependency in graph.get(&next).into_iter().flatten() {
            if seen.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    seen
}

/// Whether `text` holds `word` as a whole identifier. A substring match
/// would fire on `materialized_view` or be satisfied by a neighbour.
fn names(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Every source file under `src/`, relative path and text with line
/// comments removed — a module doc that *explains* the lake is not a use.
fn sources(directory: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, directory: &Path, found: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file is readable");
                let stripped = text
                    .lines()
                    .map(|line| line.split("//").next().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n");
                let relative = path
                    .strip_prefix(root)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((relative, stripped));
            }
        }
    }
    let root = directory.join("src");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found
}

#[test]
fn the_reflex_cell_cannot_link_the_storage_crate_that_holds_the_tick_lake() {
    let directories = crate_directories();
    // Premise: the scan follows real edges — the node does link storage.
    assert!(
        closure(NODE, &directories).contains(STORAGE),
        "the closure walk did not find {STORAGE} under {NODE}, which links it for its journal; \
         the scan is not reading the manifests"
    );
    let cell = closure(CELL, &directories);
    assert!(
        cell.len() > 3,
        "premise: {CELL}'s closure has its libraries"
    );
    assert!(
        !cell.contains(STORAGE),
        "{CELL} now links {STORAGE} (via {cell:?}); the reflex pass could read the tick lake \
         on the order path. Keep history behind the drain thread and out of the cell"
    );
}

#[test]
fn no_edge_node_module_reads_the_tick_lake_and_only_the_drain_and_root_name_it() {
    let directories = crate_directories();
    let node = sources(directories.get(NODE).expect("the node crate exists"));
    // Premise: the scan sees the drain's write, so an empty result below is
    // a finding and not a scan that read nothing.
    let drain = node
        .iter()
        .find(|(path, _)| path == "event_fabric/drain.rs")
        .expect("the drain module exists");
    assert!(
        names(&drain.1, "write_segment") && names(&drain.1, "Lake"),
        "premise: the drain's LakeSink writes segments through Lake"
    );

    let mut readers = Vec::new();
    let mut namers = Vec::new();
    for (path, text) in &node {
        for read in LAKE_READS {
            if names(text, read) {
                readers.push(format!("{path}: {read}"));
            }
        }
        if names(text, "Lake") && names(text, LAKE_VERIFY) {
            readers.push(format!("{path}: Lake::{LAKE_VERIFY}"));
        }
        if (text.contains("qip_storage::lake::") || text.contains("lake::{"))
            && !LAKE_WRITERS.contains(&path.as_str())
        {
            namers.push(path.clone());
        }
    }
    assert!(
        readers.is_empty(),
        "edge-node modules read the tick lake: {readers:?}. History stays off the reflex \
         node; a replay or training job reads it centrally"
    );
    assert!(
        namers.is_empty(),
        "edge-node modules beyond {LAKE_WRITERS:?} name qip_storage::lake: {namers:?}"
    );
}
