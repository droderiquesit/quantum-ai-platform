//! Capital, survival and risk are independent of alpha (R12-CAPITAL-7204,
//! R12-RISK-7205).
//!
//! The requirement is that no alpha or model component can construct, relax
//! or bypass a survival or risk decision. Two structural facts carry it, and
//! both are checked here rather than asserted in prose:
//!
//! 1. **The survival and risk crates cannot reach an alpha crate.** If
//!    `qip-capital`, `qip-risk` or `qip-risk-engine` could name the reasoning
//!    engine, an agent, a predictor or a strategy, a limit could be written to
//!    consult one — and a limit that asks a model is not a limit.
//! 2. **No alpha crate can reach the kernel's survival kernel.** The survival
//!    ceiling the Risk Gate reads is computed by `qip_kernel::survival`; an
//!    alpha crate that could depend on `qip-kernel` could construct a
//!    `SurvivalGovernor` with any policy and hand its ceiling to a gate. The
//!    layering already forbids a service depending on the runtime; this pins
//!    that the alpha crates in particular do not.
//!
//! Since ADR 0464 the survival kernel and the confidence (model-risk)
//! governor are their own crate, `qip-survival`, so both facts are checked of
//! it by closure. They lived in `qip-kernel`, whose closure holds every alpha
//! crate, and their independence was a scan of their source for an alpha
//! crate's name, which an aliased import or a `crate::` path into a module
//! wrapping an alpha component evaded. The kernel's `tail.rs` and `hedge.rs`
//! and `qip-risk`'s `hedge_search.rs` are still scanned below: the Hedge
//! Brain has not left the runtime yet (R12-CAPITAL-7204 stays PARTIAL on it).

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

/// Crates that generate or score alpha, or hold a model. A survival or risk
/// crate reaching any of them could consult it.
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

/// The crates that hold capital survival and the limit engine. `qip-survival`
/// holds the survival kernel and the confidence (model-risk) governor since
/// ADR 0464.
const SURVIVAL: [&str; 4] = ["qip-capital", "qip-risk", "qip-risk-engine", "qip-survival"];

/// The workspace's shipped dependency graph, as Cargo resolved it.
///
/// This read the manifests by hand until ADR 0464, keeping lines that began
/// `qip-` inside a section spelled exactly `[dependencies]`. A dependency
/// declared as a `[dependencies.qip-strategy]` table, under a
/// `[target.'cfg(..)'.dependencies]` section, or renamed with `package =`
/// was an edge that reader never saw — and every assertion here is that an
/// edge is absent. `cargo metadata` reports what Cargo compiles, so those
/// forms are resolved rather than enumerated (the same argument as
/// `architecture.rs`'s `dependency_graph`). Development and build
/// dependencies are excluded: they are not what a crate's shipped code can
/// call. Fails closed: a graph that cannot be read panics rather than
/// reading as a graph with no edges.
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
    assert!(
        output.status.success(),
        "`{cargo} metadata` exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata produced JSON");
    let packages = metadata["packages"]
        .as_array()
        .expect("cargo metadata reports a packages array");
    let members: BTreeSet<String> = packages
        .iter()
        .map(|package| {
            package["name"]
                .as_str()
                .expect("a package has a name")
                .to_string()
        })
        .collect();
    let mut found = BTreeMap::new();
    for package in packages {
        let name = package["name"].as_str().expect("a package has a name");
        let mut dependencies = BTreeSet::new();
        for dependency in package["dependencies"].as_array().into_iter().flatten() {
            if matches!(dependency["kind"].as_str(), Some("dev") | Some("build")) {
                continue;
            }
            // `name` is the crate depended on, whatever key renamed it.
            let depended = dependency["name"]
                .as_str()
                .expect("a dependency has a name");
            if members.contains(depended) {
                dependencies.insert(depended.to_string());
            }
        }
        found.insert(name.to_string(), dependencies);
    }
    assert!(
        found.len() > 25,
        "cargo metadata reported only {} workspace members; the graph is not being read",
        found.len()
    );
    found
}

fn closure(root: &str, graph: &BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
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

#[test]
fn no_survival_or_risk_crate_can_reach_an_alpha_crate() {
    let graph = workspace();
    for name in ALPHA.iter().chain(SURVIVAL.iter()) {
        assert!(
            graph.contains_key(*name),
            "premise: {name} is not a crate in this workspace, so its absence below proves \
             nothing"
        );
    }
    for root in SURVIVAL {
        let reachable = closure(root, &graph);
        assert!(
            reachable.len() > 1,
            "premise: {root} reaches nothing, so the absence below is vacuous"
        );
        let alpha: Vec<&&str> = ALPHA
            .iter()
            .filter(|crate_name| reachable.contains(**crate_name))
            .collect();
        assert!(
            alpha.is_empty(),
            "{root} holds survival or risk authority and can reach {alpha:?}; a limit that can \
             consult an alpha component can be relaxed by one"
        );
    }
}

/// The permit half of R12-CAPITAL-7204: no alpha crate can construct a
/// survival or model-risk decision. A crate that cannot name `qip-survival`
/// cannot build a `SurvivalGovernor` with a policy of its choosing, a
/// `SizingBudget`, or a `ConfidenceDecision` with a multiplier above one, and
/// cannot hand any of them to the gate; nor can it reach `qip-kernel`, which
/// composes them. Mutation: add `qip-survival.workspace = true` to
/// `qip-strategy`'s `[dependencies]` and this fails naming it.
#[test]
fn no_alpha_crate_can_reach_the_survival_kernel() {
    let graph = workspace();
    assert!(
        closure("qip-api", &graph).contains("qip-kernel"),
        "premise: a composition root reaches qip-kernel, so the check below is not vacuous"
    );
    assert!(
        closure("qip-kernel", &graph).contains("qip-survival"),
        "premise: the kernel composes qip-survival, so its absence below is not the absence \
         of a crate nobody uses"
    );
    for root in ALPHA {
        let reachable = closure(root, &graph);
        for authority in ["qip-kernel", "qip-survival"] {
            assert!(
                !reachable.contains(authority),
                "{root} can reach {authority} and so could construct the survival kernel or \
                 the confidence governor whose ceiling the Risk Gate reads"
            );
        }
    }
}

/// The survival kernel and the confidence governor are no longer modules of
/// the runtime: a `survival.rs` or `confidence.rs` reappearing in
/// `qip-kernel/src` would be code the closure test above cannot see, which is
/// the state ADR 0464 ended. Mutation: copy `qip-survival/src/survival.rs`
/// into `qip-kernel/src/` and this fails naming it.
#[test]
fn the_survival_kernel_and_the_confidence_governor_live_outside_the_runtime() {
    let root = qip_acceptance::repository_root().join("backend/crates");
    for module in ["survival.rs", "confidence.rs"] {
        assert!(
            root.join("services/qip-survival/src")
                .join(module)
                .is_file(),
            "premise: qip-survival holds {module}"
        );
        assert!(
            !root.join("runtime/qip-kernel/src").join(module).exists(),
            "qip-kernel/src/{module} exists; the survival kernel's independence from alpha is \
             proven by qip-survival's closure, and a module of the runtime has every alpha \
             crate in its"
        );
    }
}

/// `tail.rs` builds the worlds the survival kernel reads and stays in the
/// runtime; it is still scanned until it moves (ADR 0464).
#[test]
fn the_kernel_tail_module_reads_measurements_and_imports_no_alpha_component() {
    let source = qip_acceptance::repository_root().join("backend/crates/runtime/qip-kernel/src");
    for module in ["tail.rs"] {
        let text = std::fs::read_to_string(source.join(module))
            .unwrap_or_else(|error| panic!("{module} is readable: {error}"));
        assert!(
            text.contains("use "),
            "premise: {module} was read and imports something"
        );
        for alpha in ALPHA {
            let path = alpha.replace('-', "_");
            assert!(
                !text.contains(&format!("{path}::")),
                "{module} names {path}; the survival kernel reads measurements, never the \
                 component that produced them"
            );
        }
    }
}
/// R12-CAPITAL-7204's hedge half: the Hedge Brain (the kernel's `hedge.rs`
/// and `qip-risk`'s `hedge_search.rs`) imports no alpha component. The
/// model-risk half — the confidence governor that narrows grants — is
/// `qip-survival` since ADR 0464 and is checked by closure above. `qip-risk`
/// is already in [`SURVIVAL`]; this pins the kernel's hedge module, which a
/// source scan is still all that can check until it leaves the runtime.
/// Mutation: add `use qip_strategy::catalogue;` to the kernel's `hedge.rs`
/// and this fails naming it.
#[test]
fn the_hedge_brain_imports_no_alpha_component() {
    let root = qip_acceptance::repository_root().join("backend/crates");
    let modules = [
        root.join("runtime/qip-kernel/src/hedge.rs"),
        root.join("libs/qip-risk/src/hedge_search.rs"),
    ];
    for module in &modules {
        let text = std::fs::read_to_string(module)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", module.display()));
        assert!(
            text.contains("use "),
            "premise: {} was read and imports something",
            module.display()
        );
        for alpha in ALPHA {
            let path = alpha.replace('-', "_");
            assert!(
                !text.contains(&format!("{path}::")),
                "{} names {path}; a hedge or a grant contraction an alpha component can reach \
                 is one it can relax",
                module.display()
            );
        }
    }
}
