//! ADR 0127: a capital-side brain is a crate boundary with a typed contract,
//! proven by dependency closure rather than by process isolation. Decision 5
//! names what the acceptance test for a brain asserts: its closure reaches no
//! alpha crate; no other brain's crate depends on it except through
//! `qip-contracts`; and only a composition (the kernel, or an app that roots
//! one) holds two brains at once.
//!
//! This suite holds that for the Risk Brain (`qip-risk`, `qip-risk-engine`,
//! R12-CAPITAL-1097) and the Settlement Brain (`qip-capital-fabric`'s
//! `settlement` module and `qip-edge`'s, R12-CAPITAL-1100). Decision 5 said
//! "the kernel is the only crate depending on two brains"; the apps that root
//! a composition (`qip-cli`, `qip-deepbrain`) do too, and are composition
//! roots by the layering rule, so the assertion is "the kernel and apps".

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

/// Crates that generate or score alpha, or hold a model — the list
/// `survival_independence.rs` keeps.
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

const RISK_BRAIN: [&str; 2] = ["qip-risk", "qip-risk-engine"];

/// Every brain's crates by ADR 0127 decision 2, crate-level only. The
/// Settlement Brain is two modules, so its crate here is the one that hosts
/// only it and fabric types (`qip-capital-fabric`); `qip-edge` hosts the
/// cell, which is not a brain.
fn brains() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    BTreeMap::from([
        ("risk", BTreeSet::from(RISK_BRAIN)),
        ("capital", BTreeSet::from(["qip-capital"])),
        (
            "asset",
            BTreeSet::from(["qip-portfolio-engine", "qip-lifecycle"]),
        ),
        ("ledger", BTreeSet::from(["qip-ledgerd"])),
        ("settlement", BTreeSet::from(["qip-capital-fabric"])),
    ])
}

/// Each member crate under `backend/crates/<group>/` with its group and its
/// `[dependencies]` on workspace crates.
fn workspace() -> BTreeMap<String, (String, BTreeSet<String>)> {
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
                (
                    group.file_name().to_string_lossy().into_owned(),
                    dependencies,
                ),
            );
        }
    }
    found
}

fn closure(root: &str, graph: &BTreeMap<String, (String, BTreeSet<String>)>) -> BTreeSet<String> {
    let mut seen = BTreeSet::from([root.to_string()]);
    let mut pending = vec![root.to_string()];
    while let Some(next) = pending.pop() {
        for dependency in graph.get(&next).map(|(_, deps)| deps).into_iter().flatten() {
            if seen.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    seen
}

/// R12-CAPITAL-1097: the Risk Brain's crates reach no alpha crate and no
/// other brain's crate, and no other brain's crate depends on them.
///
/// Mutation: add `qip-capital.workspace = true` to qip-risk-engine's
/// `[dependencies]` — the risk brain then reaches the capital brain.
#[test]
fn the_risk_brain_reaches_no_alpha_and_no_other_brain_and_no_other_brain_reaches_it() {
    let graph = workspace();
    let brains = brains();
    for name in ALPHA.iter().chain(brains.values().flatten()) {
        assert!(
            graph.contains_key(*name),
            "premise: {name} is not a workspace crate, so its absence proves nothing"
        );
    }
    for root in RISK_BRAIN {
        let reached = closure(root, &graph);
        assert!(reached.len() > 1, "premise: {root} reaches nothing");
        let alpha: Vec<&str> = ALPHA
            .iter()
            .copied()
            .filter(|c| reached.contains(*c))
            .collect();
        assert!(alpha.is_empty(), "{root} can reach alpha crates {alpha:?}");
        for (brain, members) in &brains {
            if *brain == "risk" {
                continue;
            }
            let other: Vec<&&str> = members.iter().filter(|c| reached.contains(**c)).collect();
            assert!(
                other.is_empty(),
                "the risk brain's {root} reaches the {brain} brain's {other:?}; a brain may meet \
                 another only through qip-contracts"
            );
        }
    }
    for (brain, members) in &brains {
        if *brain == "risk" {
            continue;
        }
        for member in members {
            let (_, deps) = &graph[*member];
            let held: Vec<&&str> = RISK_BRAIN.iter().filter(|r| deps.contains(**r)).collect();
            assert!(
                held.is_empty(),
                "the {brain} brain's {member} depends on the risk brain's {held:?} directly"
            );
        }
    }
}

/// ADR 0127 decision 5, third clause: only a composition holds two brains —
/// the kernel, or an app (a composition root). A service or library holding
/// two could construct one brain's decision from another's.
///
/// Mutation: add `qip-capital.workspace = true` to qip-execution-engine's
/// `[dependencies]` — a service already on the risk brain then holds the
/// capital brain too.
#[test]
fn only_the_kernel_and_the_apps_hold_two_brains() {
    let graph = workspace();
    let brains = brains();
    let mut composers = BTreeSet::new();
    let mut offenders = Vec::new();
    for (name, (group, deps)) in &graph {
        let held: Vec<&str> = brains
            .iter()
            .filter(|(_, members)| {
                members
                    .iter()
                    .any(|m| deps.contains(*m) && !members.contains(name.as_str()))
            })
            .map(|(brain, _)| *brain)
            .collect();
        if held.len() < 2 {
            continue;
        }
        composers.insert(name.clone());
        if name != "qip-kernel" && group != "apps" {
            offenders.push(format!("{name} ({group}) holds {held:?}"));
        }
    }
    assert!(
        composers.contains("qip-kernel"),
        "premise: the kernel composes several brains, so the scan sees composition: {composers:?}"
    );
    assert!(
        offenders.is_empty(),
        "a crate that is neither the kernel nor an app holds two brains: {offenders:?}"
    );
}

/// The production part of a source file: everything before its first test
/// module.
fn production(path: &str) -> String {
    let text = std::fs::read_to_string(qip_acceptance::repository_root().join(path))
        .expect("a settlement module is readable");
    let boundary = ["\n#[cfg(test)]", "\nmod "].concat();
    match text.find(&boundary) {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// R12-CAPITAL-1100: both halves of the Settlement Brain name only
/// `qip_contracts`, `qip_core` and `qip_financial` among workspace crates,
/// and the crate hosting the central half reaches no alpha crate.
///
/// Mutation: add `use qip_capital::allocation::CapitalAllocator;` to
/// `qip-capital-fabric/src/settlement.rs` — the module then names the
/// capital brain.
#[test]
fn the_settlement_brain_names_only_contracts_core_and_financial_and_reaches_no_alpha() {
    const ALLOWED: [&str; 3] = ["qip_contracts", "qip_core", "qip_financial"];
    for path in [
        "backend/crates/services/qip-capital-fabric/src/settlement.rs",
        "backend/crates/edge/qip-edge/src/settlement.rs",
    ] {
        let source = production(path);
        assert!(
            source.contains("qip_contracts::"),
            "premise: {path} names the shared settlement contract"
        );
        let mut named = BTreeSet::new();
        for (at, _) in source.match_indices("qip_") {
            let path_start = &source[at..];
            let ident: String = path_start
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if path_start[ident.len()..].starts_with("::") {
                named.insert(ident);
            }
        }
        let foreign: Vec<&String> = named
            .iter()
            .filter(|crate_name| !ALLOWED.contains(&crate_name.as_str()))
            .collect();
        assert!(
            foreign.is_empty(),
            "{path} names {foreign:?}; the settlement brain meets other domains only through \
             qip-contracts"
        );
    }
    let graph = workspace();
    let reached = closure("qip-capital-fabric", &graph);
    let alpha: Vec<&str> = ALPHA
        .iter()
        .copied()
        .filter(|c| reached.contains(*c))
        .collect();
    assert!(
        alpha.is_empty(),
        "qip-capital-fabric reaches alpha crates {alpha:?}"
    );
}
