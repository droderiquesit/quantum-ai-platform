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
//! And a third, about the source of the kernel modules themselves: they read
//! measurements (a Brier score, a dissent count, a stability index, a regime)
//! and never import the component that produced them, so the governor cannot
//! be handed a model to call.

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

/// The crates that hold capital survival and the limit engine.
const SURVIVAL: [&str; 3] = ["qip-capital", "qip-risk", "qip-risk-engine"];

fn workspace() -> BTreeMap<String, BTreeSet<String>> {
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
            let Ok(manifest) = std::fs::read_to_string(member.path().join("Cargo.toml")) else {
                continue;
            };
            let mut in_section = false;
            let mut dependencies = BTreeSet::new();
            for line in manifest.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_section = line == "[dependencies]";
                } else if in_section && line.starts_with("qip-") {
                    let name: String = line
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                        .collect();
                    dependencies.insert(name);
                }
            }
            found.insert(
                member.file_name().to_string_lossy().into_owned(),
                dependencies,
            );
        }
    }
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

#[test]
fn no_alpha_crate_can_reach_the_survival_kernel() {
    let graph = workspace();
    assert!(
        closure("qip-api", &graph).contains("qip-kernel"),
        "premise: a composition root reaches qip-kernel, so the check below is not vacuous"
    );
    for root in ALPHA {
        assert!(
            !closure(root, &graph).contains("qip-kernel"),
            "{root} can reach qip-kernel and so could construct the survival kernel whose \
             ceiling the Risk Gate reads"
        );
    }
}

#[test]
fn the_kernel_survival_modules_read_measurements_and_import_no_alpha_component() {
    let source = qip_acceptance::repository_root().join("backend/crates/runtime/qip-kernel/src");
    for module in ["survival.rs", "confidence.rs", "tail.rs"] {
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
/// R12-CAPITAL-7204's hedge and model-risk halves: the Hedge Brain (the
/// kernel's `hedge.rs` and `qip-risk`'s `hedge_search.rs`) and the
/// confidence governor that narrows grants (`confidence.rs`) import no alpha
/// component, so no alpha or model crate can construct, relax or bypass a
/// hedge plan or a grant contraction. `qip-risk` is already in [`SURVIVAL`],
/// so its crate closure is checked above; this pins the modules themselves.
/// Mutation: add `use qip_strategy::catalogue;` to the kernel's `hedge.rs`
/// and this fails naming it.
#[test]
fn the_hedge_brain_and_the_grant_governor_import_no_alpha_component() {
    let root = qip_acceptance::repository_root().join("backend/crates");
    let modules = [
        root.join("runtime/qip-kernel/src/hedge.rs"),
        root.join("runtime/qip-kernel/src/confidence.rs"),
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
