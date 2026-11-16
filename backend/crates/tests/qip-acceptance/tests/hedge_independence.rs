//! The Hedge Brain's source reaches no alpha crate by closure, and its one
//! known exception is named (R12-CAPITAL-7204).
//!
//! `qip-kernel/src/hedge.rs` is still in the runtime (ADR 0464), whose closure
//! holds every alpha crate, so the crate graph cannot vouch for it the way it
//! does for `qip-survival`. `survival_independence` scans the module for an
//! alpha crate's *name*, which an alias (`use qip_simulation_engine as s;`
//! then `s::...`) or a crate that itself reaches alpha evades. This test reads
//! every `qip_*` crate the hedge modules name and requires each one's
//! dependency closure to be free of alpha crates, so a new import that
//! reaches alpha through a middle crate fails here.
//!
//! One crate is excepted and named: `qip-simulation-engine`, whose scenario
//! library `hedge.rs` reads and which reaches `qip-strategy` and
//! `qip-evolution`. It is the whole residual of R12-CAPITAL-7204's Hedge Brain
//! half. The exception is a ratchet: when the scenario library leaves that
//! crate and `hedge.rs` stops naming it, `the_named_exception_is_still_in_use`
//! fails until the exception is deleted, so it cannot outlive its cause.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

const ALPHA: [&str; 11] = [
    "qip-agents",
    "qip-ai",
    "qip-investment-agents",
    "qip-reasoning-engine",
    "qip-opportunity-engine",
    "qip-prediction",
    "qip-world-model",
    "qip-training",
    "qip-learning-engine",
    "qip-evolution",
    "qip-strategy",
];

/// Crates a hedge module may name though their closure reaches alpha.
const EXCEPTIONS: [&str; 1] = ["qip-simulation-engine"];

/// The kernel modules that make up the Hedge Brain and the modules it reads.
const MODULES: [&str; 5] = [
    "runtime/qip-kernel/src/cross_margin.rs",
    "runtime/qip-kernel/src/hedge.rs",
    "runtime/qip-kernel/src/hedge_review.rs",
    "runtime/qip-kernel/src/tail.rs",
    "runtime/qip-kernel/src/asset_class_registry.rs",
];

fn workspace() -> BTreeMap<String, BTreeSet<String>> {
    let manifest = qip_acceptance::repository_root().join("backend/Cargo.toml");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(&cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .unwrap_or_else(|error| panic!("could not run `{cargo} metadata`: {error}"));
    assert!(output.status.success(), "`{cargo} metadata` failed");
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let packages = metadata["packages"].as_array().expect("packages");
    let members: BTreeSet<String> = packages
        .iter()
        .map(|p| p["name"].as_str().expect("name").to_string())
        .collect();
    let mut found = BTreeMap::new();
    for package in packages {
        let mut deps = BTreeSet::new();
        for dep in package["dependencies"].as_array().into_iter().flatten() {
            if matches!(dep["kind"].as_str(), Some("dev") | Some("build")) {
                continue;
            }
            let name = dep["name"].as_str().expect("dependency name");
            if members.contains(name) {
                deps.insert(name.to_string());
            }
        }
        found.insert(package["name"].as_str().expect("name").to_string(), deps);
    }
    assert!(found.len() > 25, "the graph is not being read");
    found
}

fn closure(root: &str, graph: &BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    let mut seen = BTreeSet::from([root.to_string()]);
    let mut pending = vec![root.to_string()];
    while let Some(next) = pending.pop() {
        for dep in graph.get(&next).into_iter().flatten() {
            if seen.insert(dep.clone()) {
                pending.push(dep.clone());
            }
        }
    }
    seen
}

/// Every `qip_<name>` identifier in `text`, as a crate name with hyphens.
fn named_crates(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(offset) = text[at..].find("qip_") {
        let start = at + offset;
        let boundary =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if boundary {
            found.insert(text[start..end].replace('_', "-"));
        }
        at = end.max(start + 4);
    }
    found
}

fn crates_named_by_the_hedge_modules() -> BTreeSet<String> {
    let root = qip_acceptance::repository_root().join("backend/crates");
    let mut all = BTreeSet::new();
    for module in MODULES {
        let path = root.join(module);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
        let named = named_crates(&text);
        assert!(
            !named.is_empty(),
            "premise: {module} names at least one qip crate"
        );
        all.extend(named);
    }
    all
}

#[test]
fn every_crate_the_hedge_modules_name_reaches_no_alpha_crate_but_the_named_exception() {
    let graph = workspace();
    for krate in crates_named_by_the_hedge_modules() {
        // A path like `qip_kernel`-less identifier that is not a package
        // (a local binding, a doc word) is not a crate.
        // `qip-kernel` is the crate these modules live in: a mention of it is
        // prose in a string or a doc, and the real way into the rest of the
        // kernel is a `crate::` path, pinned by the next test.
        if krate == "qip-kernel"
            || !graph.contains_key(&krate)
            || EXCEPTIONS.contains(&krate.as_str())
        {
            continue;
        }
        let reached = closure(&krate, &graph);
        for alpha in ALPHA {
            assert!(
                !reached.contains(alpha),
                "the Hedge Brain names `{krate}`, whose closure reaches alpha crate `{alpha}`; a \
                 hedge an alpha component can reach is one it can relax"
            );
        }
    }
}

/// The kernel modules the Hedge Brain may reach by a `crate::` path. A new one
/// is a new way for the hedge to read something the kernel composed from alpha
/// output, so it is added here by a reviewed change or the test fails.
///
/// `platform` is the second named residual beside `qip-simulation-engine`:
/// `hedge.rs` reads `StressReport` from it and `asset_class_registry.rs` names
/// `Platform`, and `platform.rs` composes every alpha component. `confidence`
/// and `survival` are the kernel's re-exports of `qip-survival` (ADR 0464).
const KERNEL_MODULES: [&str; 9] = [
    "Platform",
    "hedge_review",
    "platform",
    "asset_class_registry",
    "tail",
    "shared_cause",
    "cross_margin",
    "confidence",
    "survival",
];

#[test]
fn the_hedge_modules_reach_only_the_named_kernel_modules() {
    let root = qip_acceptance::repository_root().join("backend/crates");
    for module in MODULES {
        let text = std::fs::read_to_string(root.join(module)).expect("module is readable");
        let mut at = 0;
        let mut seen = 0;
        while let Some(offset) = text[at..].find("crate::") {
            let start = at + offset + "crate::".len();
            let name: String = text[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            seen += 1;
            assert!(
                KERNEL_MODULES.contains(&name.as_str()) || name.is_empty() || name == "tests",
                "{module} reaches `crate::{name}`, a kernel module the Hedge Brain is not \
                 declared to read"
            );
            at = start;
        }
        let _ = seen;
    }
}

/// The third residual, found by this test's first run, and a larger one than
/// the first guess: `hedge_review.rs` reaches `shared_cause.rs`, which names
/// the agent framework's fed-slot cell (`qip_agents::runtime::Upstream`) and
/// the world model's causal graph (`qip_world_model::...`), and both crates
/// are on the alpha list. Reading a causal graph is reading a model's output,
/// so this is the part of the Hedge Brain's independence that is open, not a
/// formality: the shared-cause review should receive its graph as data the way
/// `qip-survival` receives `StressedWorlds`. The test pins the exact set so a
/// further alpha path in this module fails here, and shrinking it is progress
/// this test will insist is recorded.
#[test]
fn shared_cause_names_exactly_the_alpha_paths_recorded_as_the_residual() {
    let root = qip_acceptance::repository_root().join("backend/crates");
    let text = std::fs::read_to_string(root.join("runtime/qip-kernel/src/shared_cause.rs"))
        .expect("shared_cause.rs is readable");
    let mut paths = BTreeSet::new();
    for alpha in ALPHA {
        let prefix = alpha.replace('-', "_");
        let mut at = 0;
        while let Some(offset) = text[at..].find(&format!("{prefix}::")) {
            let start = at + offset;
            let path: String = text[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                .collect();
            paths.insert(path);
            at = start + prefix.len();
        }
    }
    assert_eq!(
        paths,
        BTreeSet::from(
            [
                "qip_agents::runtime::Upstream",
                "qip_world_model::CausalGraph",
                "qip_world_model::WorldModel",
                "qip_world_model::causal::",
                "qip_world_model::causal::CausalGraph",
                "qip_world_model::granger",
                "qip_world_model::granger::TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING",
            ]
            .map(str::to_string)
        ),
        "shared_cause.rs names alpha paths other than the recorded residual"
    );
}

#[test]
fn the_named_exception_is_still_in_use() {
    let graph = workspace();
    let named = crates_named_by_the_hedge_modules();
    for exception in EXCEPTIONS {
        assert!(
            named.contains(exception),
            "`{exception}` is excepted but no hedge module names it any more: delete it from \
             EXCEPTIONS, the Hedge Brain half of R12-CAPITAL-7204 no longer needs it"
        );
        let reached = closure(exception, &graph);
        assert!(
            ALPHA.iter().any(|alpha| reached.contains(*alpha)),
            "`{exception}` reaches no alpha crate any more: delete it from EXCEPTIONS"
        );
    }
}

#[test]
fn the_name_reader_finds_a_crate_and_ignores_a_longer_identifier() {
    let found = named_crates("use qip_risk::x; let my_qip_strategy = 1; qip_core::y;");
    assert!(found.contains("qip-risk") && found.contains("qip-core"));
    assert!(
        !found.contains("qip-strategy"),
        "a suffix of another identifier was read as a crate"
    );
}
