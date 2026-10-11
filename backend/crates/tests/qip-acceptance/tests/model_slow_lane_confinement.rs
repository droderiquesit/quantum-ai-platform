//! R12-MODEL-4002: a generalist (hosted) language model is constructed only by
//! the slow-lane composition root, and no reflex crate links the crate that
//! holds it.
//!
//! The failure this prevents: `qip-fastbrain` links `qip-kernel`, which links
//! `qip-reasoning-engine`, which holds `HuggingFaceModel`. So the fast brain
//! *can* build the hosted adapter — the type is in its link graph — and until
//! this suite the only thing stopping it was a sentence in
//! `qip-deepbrain/src/language.rs` saying it was the only root that did. A
//! one-line `HuggingFaceModel::new(…)` in the fast brain's `main.rs` would
//! have put a remote model's latency and availability on the path the fast
//! brain serves, and nothing would have failed.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeSet;

/// The one directory whose production code may construct a hosted adapter.
const SLOW_LANE_ROOT: &str = "backend/crates/apps/qip-deepbrain/src/";

/// Where hosted adapters are defined; their own constructors live here.
const PROVIDERS: &str = "backend/crates/services/qip-reasoning-engine/src/providers/";

/// The text of a source file with comment lines dropped — and, deliberately,
/// `#[cfg(test)]` items kept. `qip_acceptance::production_text` skips those by
/// brace depth, and a test module whose string literals hold an unbalanced
/// brace makes it skip to the end of the file: a construction appended to
/// `qip-fastbrain/src/main.rs` after its test module passed this suite in its
/// first draft for exactly that reason. Keeping test code is the stricter
/// reading — a fixture building the hosted adapter in a fast-lane crate is a
/// finding too.
fn code_text(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every type under `providers/` that implements `LanguageModel` — the hosted
/// adapters, found rather than listed, so a second provider added beside the
/// first is fenced the day it lands.
fn hosted_adapters() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for path in qip_acceptance::files_with_extension(PROVIDERS, "rs") {
        let source = std::fs::read_to_string(&path).unwrap();
        for line in source.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("impl LanguageModel for ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                found.insert(name);
            }
        }
    }
    found
}

/// Production files outside the providers directory that construct `adapter`,
/// by associated constructor or by struct literal.
fn construction_sites(adapter: &str) -> Vec<String> {
    let root = qip_acceptance::repository_root();
    let mut sites = Vec::new();
    for path in qip_acceptance::files_with_extension("backend/crates", "rs") {
        let relative = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if relative.starts_with(PROVIDERS)
            || relative.contains("/tests/")
            || relative.contains("/benches/")
            || relative.contains("/examples/")
        {
            continue;
        }
        let production = code_text(&std::fs::read_to_string(&path).unwrap());
        let constructs = production.contains(&format!("{adapter}::new("))
            || production.contains(&format!("{adapter} {{"))
            || production.contains(&format!("{adapter}::with_"));
        if constructs {
            sites.push(relative);
        }
    }
    sites
}

#[test]
fn the_hosted_language_adapter_is_constructed_only_by_the_slow_lane_composition_root() {
    let adapters = hosted_adapters();
    // Premise: the scan found the adapter it exists to fence. A providers
    // directory renamed or emptied would otherwise make every assertion below
    // vacuously true.
    assert!(
        adapters.contains("HuggingFaceModel"),
        "the hosted adapter was not found under {PROVIDERS}; found {adapters:?}"
    );

    for adapter in &adapters {
        let sites = construction_sites(adapter);
        // Premise: the slow lane does construct it — the scan can see a real
        // construction site, so an empty result elsewhere means something.
        assert!(
            sites.iter().any(|site| site.starts_with(SLOW_LANE_ROOT)),
            "{adapter} has no construction site under {SLOW_LANE_ROOT}; the scan cannot see the \
             one site it is meant to admit, so its silence elsewhere proves nothing: {sites:?}"
        );
        let outside: Vec<&String> = sites
            .iter()
            .filter(|site| !site.starts_with(SLOW_LANE_ROOT))
            .collect();
        assert!(
            outside.is_empty(),
            "{adapter} is a hosted generalist model and is constructed outside the slow-lane \
             root ({SLOW_LANE_ROOT}): {outside:?}. R12-MODEL-4002 / ADR 0008: nothing on the \
             fast path or the order path consults a remote model."
        );
    }
}

#[test]
fn no_reflex_or_fast_lane_root_names_the_hosted_provider_module() {
    // The fast brain and the reflex node link the providers crate only
    // transitively; naming its module in their sources is the first step of
    // constructing an adapter there, and it is refused here before it gets
    // further.
    for root in [
        "backend/crates/apps/qip-fastbrain/src",
        "backend/crates/apps/qip-edge-node/src",
        "backend/crates/edge",
    ] {
        let files = qip_acceptance::files_with_extension(root, "rs");
        assert!(!files.is_empty(), "{root} holds no Rust sources");
        for path in files {
            let production = code_text(&std::fs::read_to_string(&path).unwrap());
            for forbidden in ["providers::", "HuggingFace", "RemoteModel"] {
                assert!(
                    !production.contains(forbidden),
                    "{} names `{forbidden}`: a fast or reflex lane must not reach a hosted \
                     generalist model (R12-MODEL-4002)",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn the_reflex_node_does_not_link_the_crate_holding_the_hosted_adapter() {
    // `cargo tree` is not available to a test; read the manifests. The edge
    // node and every edge crate must not declare qip-reasoning-engine or
    // qip-kernel (which carries it) as a normal dependency.
    let manifests = qip_acceptance::files_with_extension("backend/crates/edge", "toml")
        .into_iter()
        .chain([
            qip_acceptance::repository_root().join("backend/crates/apps/qip-edge-node/Cargo.toml")
        ])
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .collect::<Vec<_>>();
    assert!(manifests.len() > 1, "no edge manifests found");
    for manifest in manifests {
        let text = std::fs::read_to_string(&manifest).unwrap();
        let normal = text
            .split("\n[")
            .find(|section| section.starts_with("dependencies]"))
            .unwrap_or("");
        for line in normal.lines() {
            let name = line.split(['=', '.']).next().unwrap_or("").trim();
            assert!(
                name != "qip-reasoning-engine" && name != "qip-kernel",
                "{} declares `{name}`, which links the hosted language adapter into the reflex \
                 lane (R12-MODEL-4002)",
                manifest.display()
            );
        }
    }
}
