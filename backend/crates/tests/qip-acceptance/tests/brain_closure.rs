//! ADR 0127: a capital-side brain is a crate boundary with a typed contract,
//! proven by dependency closure rather than by process isolation. Decision 5
//! names what the acceptance test for a brain asserts: its closure reaches no
//! alpha crate; no other brain's crate depends on it except through
//! `qip-contracts`; and only a composition (the kernel, or an app that roots
//! one) holds two brains at once.
//!
//! This suite holds that for the Risk Brain (`qip-risk`, `qip-risk-engine`,
//! R12-CAPITAL-1097), the Ledger Brain (`qip-ledgerd`, R12-CAPITAL-1099) and
//! the Settlement Brain (`qip-capital-fabric`'s `settlement` module and
//! `qip-edge`'s, R12-CAPITAL-1100).
//!
//! **`qip-edge` is not a brain crate, and that is checked, not assumed.** It
//! hosts the cell, which by ADR 0008 decides alone and so depends on the risk
//! brain's pre-trade checks directly; listing it as a brain would make every
//! cell a crate holding two brains. What ADR 0127 places in it is a module,
//! the cell's settlement gate, so the settlement half is held at module
//! level: the module, and every `crate::` module it reaches, names no
//! workspace crate but contracts, core and the financial calendar. Decision 5 said
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
/// cell, which is not a brain, and its settlement module is held at module
/// level by the settlement test below.
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

/// Each member crate under `backend/crates/<group>/`, keyed by its
/// `[package]` name, with its group and the workspace crates it depends on
/// outside tests: `[dependencies]`, `[target.<cfg>.dependencies]` and
/// `[dependencies.<name>]` tables, with a `package = "qip-…"` rename read as
/// the crate it names rather than the key it is bound to. Dev-dependencies
/// are left out: a test may reach anything, and a brain's boundary is what
/// ships.
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
            let (name, dependencies) = parse_manifest(&manifest);
            let name = name.unwrap_or_else(|| member.file_name().to_string_lossy().into_owned());
            found.insert(
                name,
                (
                    group.file_name().to_string_lossy().into_owned(),
                    dependencies,
                ),
            );
        }
    }
    found
}

/// The `[package]` name and the production `qip-*` dependencies of one
/// manifest.
fn parse_manifest(manifest: &str) -> (Option<String>, BTreeSet<String>) {
    enum Section {
        Package,
        Dependencies,
        /// `[dependencies.<key>]`: the key, until a `package =` renames it.
        Table(String),
        Other,
    }
    fn quoted(value: &str) -> Option<String> {
        let rest = value.split_once('"')?.1;
        Some(rest.split_once('"')?.0.to_string())
    }
    fn key(line: &str) -> String {
        line.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }
    let mut name = None;
    let mut dependencies = BTreeSet::new();
    let mut section = Section::Other;
    fn close(section: &Section, dependencies: &mut BTreeSet<String>) {
        if let Section::Table(crate_name) = section
            && crate_name.starts_with("qip-")
        {
            dependencies.insert(crate_name.clone());
        }
    }
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            close(&section, &mut dependencies);
            let header = line.trim_matches(|c| c == '[' || c == ']').trim();
            let target = header.starts_with("target.");
            section = if header == "package" {
                Section::Package
            } else if header == "dependencies" || (target && header.ends_with(".dependencies")) {
                Section::Dependencies
            } else if let Some(table) = header.strip_prefix("dependencies.") {
                Section::Table(table.trim_matches('"').to_string())
            } else if target && header.contains(".dependencies.") {
                let table = header.rsplit_once(".dependencies.").map_or("", |(_, t)| t);
                Section::Table(table.trim_matches('"').to_string())
            } else {
                Section::Other
            };
            continue;
        }
        match &mut section {
            Section::Package if key(line) == "name" && name.is_none() => {
                name = line.split_once('=').and_then(|(_, v)| quoted(v));
            }
            Section::Dependencies => {
                let bound = key(line);
                if bound.is_empty() {
                    continue;
                }
                let renamed = line
                    .split_once("package")
                    .filter(|(_, after)| after.trim_start().starts_with('='))
                    .and_then(|(_, after)| quoted(after));
                let crate_name = renamed.unwrap_or(bound);
                if crate_name.starts_with("qip-") {
                    dependencies.insert(crate_name);
                }
            }
            Section::Table(crate_name) if key(line) == "package" => {
                if let Some(renamed) = line.split_once('=').and_then(|(_, v)| quoted(v)) {
                    *crate_name = renamed;
                }
            }
            _ => {}
        }
    }
    close(&section, &mut dependencies);
    (name, dependencies)
}

/// The parser is the instrument every closure here reads through, so it is
/// proven on each manifest shape that could hide an edge.
///
/// Mutation: drop the `target.` arm, the table arm or the rename read in
/// `parse_manifest` — each hides one dependency below.
#[test]
fn the_manifest_reader_sees_target_table_and_renamed_dependencies() {
    let (name, dependencies) = parse_manifest(
        r#"
[package]
name = "qip-example"
version.workspace = true

[dependencies]
qip-core.workspace = true
alias = { package = "qip-capital", path = "../qip-capital" }
serde.workspace = true

[target.'cfg(unix)'.dependencies]
qip-risk.workspace = true

[dependencies.qip-ledgerd]
path = "../../apps/qip-ledgerd"

[dependencies.hidden]
package = "qip-strategy"
version = "0"

[target.'cfg(unix)'.dependencies.qip-ai]
path = "../qip-ai"

[dev-dependencies]
qip-training.workspace = true
"#,
    );
    assert_eq!(name.as_deref(), Some("qip-example"));
    assert_eq!(
        dependencies,
        BTreeSet::from(
            [
                "qip-ai",
                "qip-capital",
                "qip-core",
                "qip-ledgerd",
                "qip-risk",
                "qip-strategy",
            ]
            .map(String::from)
        ),
        "every production dependency, renamed ones by the crate they name, and no dev-dependency"
    );
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

/// The production code of a source file: everything before its test
/// module, with comments removed so a crate a doc comment mentions is not
/// read as one the code names.
fn production(path: &std::path::Path) -> String {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
    let code = match text.find("\n#[cfg(test)]\nmod tests") {
        Some(at) => &text[..at],
        None => &text[..],
    };
    code.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `qip_*` crate a module names, following each `crate::<module>` it
/// reaches inside its own crate, with the modules visited.
fn named_by_module(src: &std::path::Path, module: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    fn identifiers_after<'a>(code: &'a str, prefix: &'a str) -> impl Iterator<Item = String> + 'a {
        code.match_indices(prefix).filter_map(move |(at, _)| {
            let before = code[..at].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let ident: String = code[at..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            Some(ident)
        })
    }
    let mut named = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![module.to_string()];
    while let Some(next) = pending.pop() {
        if !visited.insert(next.clone()) {
            continue;
        }
        let file = [
            src.join(format!("{next}.rs")),
            src.join(&next).join("mod.rs"),
        ]
        .into_iter()
        .find(|candidate| candidate.exists())
        .unwrap_or_else(|| panic!("module {next} has a source file under {}", src.display()));
        let code = production(&file);
        named.extend(identifiers_after(&code, "qip_"));
        for (at, _) in code.match_indices("crate::") {
            let reached: String = code[at + "crate::".len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !reached.is_empty()
                && reached
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase())
            {
                pending.push(reached);
            }
        }
    }
    (named, visited)
}

/// R12-CAPITAL-1100: both halves of the Settlement Brain — and every
/// module either reaches inside its own crate — name only `qip_contracts`,
/// `qip_core` and `qip_financial` among workspace crates, and the crate
/// hosting the central half reaches no alpha crate.
///
/// Mutations: add `use qip_capital::allocation::CapitalAllocator;` to
/// `qip-capital-fabric/src/settlement.rs`; or add
/// `pub use qip_capital::LedgerObligations as NeighbourObligations;` to
/// `qip-capital-fabric/src/location.rs`, a module the settlement module
/// reaches — the module then names a brain only through a neighbour; or add
/// `use qip_risk_engine::monitor;` to `qip-edge/src/settlement.rs`.
#[test]
fn the_settlement_brain_names_only_contracts_core_and_financial_and_reaches_no_alpha() {
    const ALLOWED: [&str; 3] = ["qip_contracts", "qip_core", "qip_financial"];
    let root = qip_acceptance::repository_root().join("backend/crates");
    for (src, reaches) in [
        ("services/qip-capital-fabric/src", Some("location")),
        ("edge/qip-edge/src", None),
    ] {
        let src = root.join(src);
        let (named, visited) = named_by_module(&src, "settlement");
        assert!(
            named.contains("qip_contracts"),
            "premise: {}/settlement.rs names the shared contracts, so the scan reads it",
            src.display()
        );
        if let Some(neighbour) = reaches {
            assert!(
                visited.contains(neighbour),
                "premise: the scan follows settlement into crate::{neighbour}: {visited:?}"
            );
        }
        let foreign: Vec<&String> = named
            .iter()
            .filter(|crate_name| !ALLOWED.contains(&crate_name.as_str()))
            .collect();
        assert!(
            foreign.is_empty(),
            "{}: the settlement modules {visited:?} name {foreign:?}; the settlement brain meets \
             other domains only through qip-contracts",
            src.display()
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

/// `qip-edge` hosts the risk brain's pre-trade checks inside the cell
/// (ADR 0008), so it is deliberately absent from [`brains`]. This pins that
/// choice to a fact rather than a comment: if the cell ever stops depending
/// on the risk brain, the reason for leaving it out is gone and the list
/// should be revisited.
#[test]
fn the_cell_crate_is_not_a_brain_because_it_composes_the_risk_brains_checks() {
    let graph = workspace();
    let (_, deps) = &graph["qip-edge"];
    assert!(
        RISK_BRAIN.iter().any(|r| deps.contains(*r)),
        "qip-edge no longer depends on the risk brain; revisit whether it is a brain crate"
    );
    assert!(
        !brains().values().flatten().any(|c| *c == "qip-edge"),
        "qip-edge is listed as a brain crate while composing the risk brain's checks"
    );
}

/// R12-CAPITAL-1099: the Ledger Brain (`qip-ledgerd` over
/// `qip_contracts::ledger` and `ledger_operation`) reaches no alpha crate
/// and no other brain's crate, and no other brain's crate depends on it.
/// It read `LedgerObligations` from `qip-capital` until that type moved to
/// `qip_contracts::ledger`; the capital brain was the one other brain it
/// held.
///
/// Mutation: re-add `qip-capital.workspace = true` to qip-ledgerd's
/// `[dependencies]` and `use qip_capital::LedgerObligations;` in
/// `store.rs` — the ledger brain then reaches the capital brain.
#[test]
fn the_ledger_brain_reaches_no_alpha_and_no_other_brain_and_meets_them_through_contracts() {
    let graph = workspace();
    let brains = brains();
    let (_, direct) = &graph["qip-ledgerd"];
    assert!(
        direct.contains("qip-contracts"),
        "premise: the ledger brain's contract is qip_contracts::ledger"
    );
    let reached = closure("qip-ledgerd", &graph);
    assert!(
        reached.len() > 2,
        "premise: qip-ledgerd reaches its libraries"
    );
    let alpha: Vec<&str> = ALPHA
        .iter()
        .copied()
        .filter(|c| reached.contains(*c))
        .collect();
    assert!(
        alpha.is_empty(),
        "qip-ledgerd can reach alpha crates {alpha:?}"
    );
    for (brain, members) in &brains {
        if *brain == "ledger" {
            continue;
        }
        let other: Vec<&&str> = members.iter().filter(|c| reached.contains(**c)).collect();
        assert!(
            other.is_empty(),
            "the ledger brain reaches the {brain} brain's {other:?}; a brain may meet another \
             only through qip-contracts"
        );
        for member in members {
            let (_, deps) = &graph[*member];
            assert!(
                !deps.contains("qip-ledgerd"),
                "the {brain} brain's {member} depends on the ledger brain directly"
            );
        }
    }
}
