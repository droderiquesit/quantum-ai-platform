//! ARCH-001: the reflex plane runs without the cognitive plane.
//!
//! The property is about the **compiled** graph: nothing `qip-edge-node`
//! links may be a cognitive crate, directly or through any crate it depends
//! on. The previous version of this file read the edge node's own
//! `Cargo.toml` and asserted `!content.contains("qip-world-model")` and
//! friends. That missed every transitive path — `qip-edge` gaining a
//! dependency on `qip-world-model` passed it — and, being a substring match
//! over the whole file, it would also have failed on a comment that merely
//! named a forbidden crate. These tests walk the workspace's own manifests,
//! read each crate's `[dependencies]` keys as whole names, and close over
//! them.
//!
//! Dev-dependencies are deliberately excluded: a crate's tests are not linked
//! into the binary, and counting them would fail this test on a fixture.
//! Build-dependencies and target-specific dependency tables are included,
//! because they are compiled for the binary.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn backend() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

const COGNITIVE: [&str; 9] = [
    "qip-reasoning-engine",
    "qip-world-model",
    "qip-twin",
    "qip-evolution",
    "qip-training",
    "qip-learning-engine",
    "qip-optimization-engine",
    "qip-simulation-engine",
    "qip-deepbrain",
];

/// The `[package] name` and the linked dependency names of one manifest.
///
/// A dependency line is `name.workspace = true`, `name = "1"` or
/// `name = { ... }`; the key is everything before the first `.`, ` ` or
/// `=`, and is compared whole — never by substring.
fn parse_manifest(text: &str) -> (Option<String>, BTreeSet<String>) {
    let mut name = None;
    let mut deps = BTreeSet::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if section == "package" {
            if let Some(rest) = line.strip_prefix("name") {
                let value = rest.trim_start().trim_start_matches('=').trim();
                name = Some(value.trim_matches('"').to_string());
            }
            continue;
        }
        let linked = (section == "dependencies"
            || section == "build-dependencies"
            || section.ends_with(".dependencies")
            || section.ends_with(".build-dependencies"))
            && !section.ends_with("dev-dependencies");
        if linked {
            let key: String = line
                .chars()
                .take_while(|c| !matches!(c, '.' | ' ' | '='))
                .collect();
            if !key.is_empty() {
                deps.insert(key.trim_matches('"').to_string());
            }
        }
    }
    (name, deps)
}

/// Every workspace crate's linked dependencies, keyed by package name.
fn workspace_graph() -> BTreeMap<String, BTreeSet<String>> {
    let mut graph = BTreeMap::new();
    let crates = backend().join("crates");
    for group in fs::read_dir(&crates)
        .expect("backend/crates is readable")
        .flatten()
    {
        let Ok(members) = fs::read_dir(group.path()) else {
            continue;
        };
        for member in members.flatten() {
            let manifest = member.path().join("Cargo.toml");
            let Ok(text) = fs::read_to_string(&manifest) else {
                continue;
            };
            let (name, deps) = parse_manifest(&text);
            let name = name.unwrap_or_else(|| panic!("{} has no package name", manifest.display()));
            graph.insert(name, deps);
        }
    }
    graph
}

fn closure(graph: &BTreeMap<String, BTreeSet<String>>, root: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root.to_string()];
    while let Some(next) = stack.pop() {
        for dep in graph.get(&next).into_iter().flatten() {
            if seen.insert(dep.clone()) {
                stack.push(dep.clone());
            }
        }
    }
    seen
}

/// Mutation: add `qip-world-model.workspace = true` to the `[dependencies]` of
/// `crates/edge/qip-edge/Cargo.toml` (a *transitive* path the old substring
/// test could not see) — the closure then contains `qip-world-model` and the
/// assertion fires. Adding it to `[dev-dependencies]` instead must not fire,
/// and does not, because dev tables are skipped.
#[test]
fn nothing_the_edge_node_links_directly_or_transitively_is_a_cognitive_crate() {
    let graph = workspace_graph();
    // Premise: the walk found the workspace, and the parser read real edges.
    assert!(
        graph.len() > 20,
        "premise: the workspace was walked, found {}",
        graph.len()
    );
    let direct = graph
        .get("qip-edge-node")
        .expect("premise: qip-edge-node is a workspace crate");
    assert!(
        direct.contains("qip-edge"),
        "premise: the parser reads dependency keys: {direct:?}"
    );
    for forbidden in COGNITIVE {
        assert!(
            graph.contains_key(forbidden),
            "premise: `{forbidden}` names a real crate, so its absence below means something"
        );
    }

    let linked = closure(&graph, "qip-edge-node");
    // Premise: the closure is transitive. `qip-numerics` is not a direct
    // dependency of the edge node; it is reached through `qip-market` and
    // others, so finding it proves the walk followed second-order edges.
    assert!(
        !direct.contains("qip-numerics"),
        "premise: not a direct edge"
    );
    assert!(
        linked.contains("qip-numerics"),
        "the closure is not transitive: {linked:?}"
    );

    let reached: Vec<&str> = COGNITIVE
        .iter()
        .copied()
        .filter(|forbidden| linked.contains(*forbidden))
        .collect();
    assert!(
        reached.is_empty(),
        "qip-edge-node links cognitive crates {reached:?}; the reflex plane must run without them"
    );
}

/// The parser compares keys whole: a crate named after a forbidden one's
/// prefix or suffix is not mistaken for it, and a comment naming one is not a
/// dependency. This is the substring trap the old test had.
///
/// Mutation: change `deps.insert(key...)` to insert the whole `line` — the
/// first assertion fails because the stored entry is
/// `qip-world-model-lite.workspace = true`, not a name.
#[test]
fn the_manifest_parser_reads_dependency_names_whole_and_ignores_comments_and_dev_tables() {
    let manifest = r#"
[package]
name = "probe"

[dependencies]
# qip-world-model is mentioned here only in a comment
qip-world-model-lite.workspace = true
serde = { version = "1" }

[dev-dependencies]
qip-twin.workspace = true

[target.'cfg(unix)'.dependencies]
qip-training = "0.1"
"#;
    let (name, deps) = parse_manifest(manifest);
    assert_eq!(name.as_deref(), Some("probe"));
    assert_eq!(
        deps,
        BTreeSet::from([
            "qip-world-model-lite".to_string(),
            "serde".to_string(),
            "qip-training".to_string(),
        ])
    );
    assert!(!deps.contains("qip-world-model"));
    assert!(!deps.contains("qip-twin"));
}

/// The two planes are separate packages, each producing its own binary, and
/// neither links the other.
///
/// Mutation: add `qip-deepbrain = { path = "../qip-deepbrain" }` to
/// qip-edge-node's `[dependencies]` — the last assertion fires.
#[test]
fn reflex_and_cognitive_are_separate_deployable_binaries_that_do_not_link_each_other() {
    let graph = workspace_graph();
    for app in ["qip-edge-node", "qip-deepbrain"] {
        assert!(graph.contains_key(app), "{app} is a workspace package");
        let main = backend().join("crates/apps").join(app).join("src/main.rs");
        assert!(main.exists(), "{} exists", main.display());
    }
    assert!(!closure(&graph, "qip-edge-node").contains("qip-deepbrain"));
    assert!(!closure(&graph, "qip-deepbrain").contains("qip-edge-node"));
}
