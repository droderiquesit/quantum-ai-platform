//! ADR 0385 (amending ADR 0127 decision 2): the Treasury Bank is
//! `services/qip-treasury`, extracted from `qip-capital::{treasury,
//! funding}`. `bank.rs` stays in the capital brain, because it drives
//! `ReservationLedger` and `DatedLadder`, and a type that carries behaviour
//! does not move to `qip-contracts`.
//!
//! What this suite holds is the boundary that decision draws, in both
//! directions and at both levels a boundary can leak:
//!
//! * the manifests: `qip-treasury` depends on `qip-core` and nothing else in
//!   the workspace, tests included; `qip-capital`'s closure does not reach
//!   it; and nothing below the kernel depends on it at all;
//! * the sources: no treasury file names `qip_capital`, no capital file
//!   names `qip_treasury` or still declares the two modules that left, and
//!   the bank is where the ADR says it is.
//!
//! The failure this prevents has a shape this repository has met before: a
//! boundary stated in an ADR and held by nothing, which one convenient
//! `use` then crosses. Treasury decides whether a unit exists to be
//! promised; the capital brain decides who is promised it. A treasury that
//! could read the allocator could be made to agree with it.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const TREASURY: &str = "backend/crates/services/qip-treasury";
const CAPITAL: &str = "backend/crates/services/qip-capital";

/// The workspace crates a manifest names: those that ship (`[dependencies]`,
/// `[target.<cfg>.dependencies]`, `[dependencies.<name>]`) and, separately,
/// every one it names in any dependency section, dev and build included. A
/// `package = "qip-…"` rename is read as the crate it names.
fn manifest_dependencies(manifest: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    fn quoted(value: &str) -> Option<String> {
        let rest = value.split_once('"')?.1;
        Some(rest.split_once('"')?.0.to_string())
    }
    let (mut shipped, mut any) = (BTreeSet::new(), BTreeSet::new());
    // (is a dependency section, ships, the crate a `[…dependencies.<name>]` table binds)
    let mut section = (false, false, None::<String>);
    let mut record = |name: String, ships: bool| {
        if name.starts_with("qip-") {
            if ships {
                shipped.insert(name.clone());
            }
            any.insert(name);
        }
    };
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            let header = line.trim_matches(|c| c == '[' || c == ']').trim();
            let (path, table) = match header.rsplit_once("dependencies.") {
                Some((before, name)) => (
                    format!("{before}dependencies"),
                    Some(name.trim_matches('"').to_string()),
                ),
                None => (header.to_string(), None),
            };
            let last = path.rsplit('.').next().unwrap_or("");
            let dependency = matches!(
                last,
                "dependencies" | "dev-dependencies" | "build-dependencies"
            );
            let ships = last == "dependencies";
            if let (true, Some(name)) = (dependency, &table) {
                record(name.clone(), ships);
            }
            section = (dependency, ships, table);
            continue;
        }
        let (dependency, ships, table) = &section;
        if !dependency {
            continue;
        }
        let renamed = line
            .split_once("package")
            .filter(|(_, after)| after.trim_start().starts_with('='))
            .and_then(|(_, after)| quoted(after));
        if let Some(renamed) = renamed {
            record(renamed, *ships);
        }
        if table.is_none() {
            let key: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            record(key, *ships);
        }
    }
    (shipped, any)
}

/// Every member crate's group and shipped workspace dependencies.
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
            found.insert(
                member.file_name().to_string_lossy().into_owned(),
                (
                    group.file_name().to_string_lossy().into_owned(),
                    manifest_dependencies(&manifest).0,
                ),
            );
        }
    }
    found
}

fn closure(root: &str, graph: &BTreeMap<String, (String, BTreeSet<String>)>) -> BTreeSet<String> {
    let mut reached = BTreeSet::from([root.to_string()]);
    let mut frontier = vec![root.to_string()];
    while let Some(next) = frontier.pop() {
        for dependency in graph.get(&next).map(|(_, d)| d).into_iter().flatten() {
            if reached.insert(dependency.clone()) {
                frontier.push(dependency.clone());
            }
        }
    }
    reached
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

/// The production code of every `.rs` file under a crate's `src`, by path
/// relative to the repository.
fn production_sources(krate: &str) -> BTreeMap<String, String> {
    let root = qip_acceptance::repository_root();
    qip_acceptance::files_with_extension(&format!("{krate}/src"), "rs")
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("a source file is readable");
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            (relative, qip_acceptance::production_text(&text))
        })
        .collect()
}

/// The reader is the instrument every manifest assertion below rests on, so
/// it is shown to see the four spellings a dependency can hide behind. A
/// reader blind to one of them would pass a manifest that used it.
#[test]
fn the_manifest_reader_sees_dev_target_table_and_renamed_dependencies() {
    let manifest = "[package]\nname = \"x\"\n\n[dependencies]\nserde.workspace = true\n\
         qip-core.workspace = true\n# qip-commented.workspace = true\n\
         alias = { package = \"qip-renamed\", path = \"../r\" }\n\n\
         [dependencies.qip-tabled]\npath = \"../t\"\n\n\
         [target.'cfg(unix)'.dependencies]\nqip-targeted.workspace = true\n\n\
         [dev-dependencies]\nqip-dev.workspace = true\n\n\
         [target.'cfg(unix)'.dev-dependencies.qip-dev-tabled]\npath = \"../d\"\n\n\
         [features]\nqip-feature = []\n";
    let (shipped, any) = manifest_dependencies(manifest);
    assert_eq!(
        shipped,
        set(&["qip-core", "qip-renamed", "qip-tabled", "qip-targeted"])
    );
    assert_eq!(
        any,
        set(&[
            "qip-core",
            "qip-renamed",
            "qip-tabled",
            "qip-targeted",
            "qip-dev",
            "qip-dev-tabled"
        ])
    );
}

/// ADR 0385 decision 1. The set is pinned exactly rather than checked for
/// the absence of one name: "does not depend on `qip-capital`" is also true
/// of a treasury that reached the allocator through a third crate, and a
/// pinned set cannot grow without this line changing in review.
///
/// Dev-dependencies are pinned too (to none). A test that built a treasury
/// book from an allocation plan would be the first place the two were
/// treated as one model.
///
/// Mutations, each fails this test: `qip-capital.workspace = true` under
/// `[dependencies]`; the same under `[dev-dependencies]`; the same under an
/// inactive `[target.'cfg(any())'.dependencies]`.
#[test]
fn the_treasury_crate_depends_on_qip_core_alone_and_reaches_no_capital_brain_crate() {
    let manifest = qip_acceptance::read(&format!("{TREASURY}/Cargo.toml"));
    let (shipped, any) = manifest_dependencies(&manifest);
    assert_eq!(
        shipped,
        set(&["qip-core"]),
        "qip-treasury's shipped workspace dependencies changed. ADR 0385 pins them: what \
         treasury needs from another brain crosses as a qip-contracts data type, and it needs \
         an ADR to say which"
    );
    assert_eq!(
        any,
        set(&["qip-core"]),
        "qip-treasury names a workspace crate in a dev, build or target dependency section \
         that it does not ship with"
    );
    let graph = workspace();
    let reached = closure("qip-treasury", &graph);
    assert!(
        reached.contains("qip-core"),
        "premise: the closure walk sees the treasury crate's one dependency: {reached:?}"
    );
    assert!(
        !reached.contains("qip-capital"),
        "qip-treasury can reach qip-capital: {reached:?}"
    );
}

/// ADR 0385 decision 1, the other direction, and decision 4: nothing below
/// the kernel depends on the treasury. A service that held it could draw on
/// a financing source outside the composition that journals the draw.
///
/// Mutations, each fails this test: `qip-treasury.workspace = true` in
/// qip-capital's `[dependencies]`; the same in qip-capital-fabric's.
#[test]
fn the_capital_brain_cannot_reach_the_treasury_and_nothing_below_the_kernel_depends_on_it() {
    let graph = workspace();
    let reached = closure("qip-capital", &graph);
    assert!(
        reached.len() > 1,
        "premise: qip-capital reaches something: {reached:?}"
    );
    assert!(
        !reached.contains("qip-treasury"),
        "qip-capital can reach qip-treasury: {reached:?}. The two meet in the kernel or not \
         at all (ADR 0385)"
    );
    assert!(
        graph.contains_key("qip-treasury"),
        "premise: the walk found the treasury crate"
    );
    let dependents: BTreeSet<String> = graph
        .iter()
        .filter(|(name, (group, deps))| {
            deps.contains("qip-treasury")
                && name.as_str() != "qip-kernel"
                && group != "apps"
                && group != "tests"
        })
        .map(|(name, _)| name.clone())
        .collect();
    assert_eq!(
        dependents,
        BTreeSet::new(),
        "a crate that is neither the kernel, an app nor the acceptance crate depends on \
         qip-treasury"
    );
}

/// The manifests bound what can be linked; this bounds what is written. It
/// also holds the two facts the extraction is: the modules are gone from
/// the capital brain, not copied, and the bank did not follow them.
///
/// Mutations, each fails this test: `pub mod funding;` restored to
/// qip-capital's `lib.rs`; `use qip_treasury::funding::FundingPlan;` added
/// to `qip-capital/src/bank.rs`; `bank.rs` copied into `qip-treasury/src`.
#[test]
fn neither_crates_source_names_the_other_and_the_bank_stays_in_the_capital_brain() {
    let treasury = production_sources(TREASURY);
    let capital = production_sources(CAPITAL);
    for expected in ["src/treasury.rs", "src/funding.rs", "src/lib.rs"] {
        assert!(
            treasury.contains_key(&format!("{TREASURY}/{expected}")),
            "premise: {expected} is in qip-treasury: {:?}",
            treasury.keys().collect::<Vec<_>>()
        );
    }
    assert!(
        capital.len() > 10,
        "premise: the walk found the capital brain's sources, found {}",
        capital.len()
    );
    // The planner still names the permission registry, so a scan that saw
    // no code at all would not pass for a clean one.
    assert!(
        treasury[&format!("{TREASURY}/src/funding.rs")].contains("FinancingPermissions"),
        "premise: the scan reads the funding planner's code"
    );

    let naming_capital: Vec<&String> = treasury
        .iter()
        .filter(|(_, code)| code.contains("qip_capital"))
        .map(|(path, _)| path)
        .collect();
    assert!(
        naming_capital.is_empty(),
        "treasury code names qip_capital: {naming_capital:?}"
    );
    let naming_treasury: Vec<&String> = capital
        .iter()
        .filter(|(_, code)| code.contains("qip_treasury"))
        .map(|(path, _)| path)
        .collect();
    assert!(
        naming_treasury.is_empty(),
        "capital-brain code names qip_treasury: {naming_treasury:?}"
    );

    let lib = &capital[&format!("{CAPITAL}/src/lib.rs")];
    let modules: BTreeSet<&str> = lib
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub mod "))
        .filter_map(|l| l.strip_suffix(';'))
        .collect();
    assert!(
        modules.contains("bank") && modules.contains("reservation"),
        "premise and decision 2: qip-capital still declares the bank and the reservation \
         ledger it drives: {modules:?}"
    );
    for moved in ["treasury", "funding"] {
        assert!(
            !modules.contains(moved),
            "qip-capital declares `{moved}` again; it was extracted to qip-treasury, and a \
             second copy is a second claim about the same books"
        );
        assert!(
            !Path::new(&qip_acceptance::repository_root())
                .join(format!("{CAPITAL}/src/{moved}.rs"))
                .exists(),
            "qip-capital/src/{moved}.rs exists beside the crate it moved to"
        );
    }
    assert!(
        !treasury.contains_key(&format!("{TREASURY}/src/bank.rs")),
        "bank.rs is in qip-treasury. ADR 0385 keeps it in the capital brain: it drives \
         ReservationLedger and DatedLadder, which carry behaviour and may not move to \
         qip-contracts"
    );
    let bank = &capital[&format!("{CAPITAL}/src/bank.rs")];
    assert!(
        bank.contains("ReservationLedger") && bank.contains("DatedLadder"),
        "premise: the reason the bank stays is still true of its code"
    );
}

/// Whether `code` holds `name` as a whole identifier: not as part of a longer
/// one. `CapitalBook` is not named by `CapitalBookkeeping`, and `EventLog` is
/// not named by `EventLogger`; a substring match would say both were.
fn names_identifier(code: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    code.match_indices(name).any(|(start, _)| {
        let before = code[..start].chars().next_back();
        let after = code[start + name.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// CAPITAL-022 at its new address. `architecture.rs` held that the capital
/// brain's deciding modules, `funding.rs` among them, name no type that
/// writes authoritative capital state and nothing of the event log. ADR 0385
/// moved `funding.rs` out of `qip-capital` together with two of those
/// writers, `CapitalBook` and `InternalFunding`, so the planner and both
/// writers now share a crate and the dependency graph cannot see the line
/// between them. This holds it by the planner's own text: the planner
/// returns a plan, and whoever holds the books applies it.
#[test]
fn the_funding_planner_names_no_type_that_writes_the_treasurys_books_and_nothing_of_the_event_log()
{
    let treasury = production_sources(TREASURY);
    let books = &treasury[&format!("{TREASURY}/src/treasury.rs")];
    let planner = &treasury[&format!("{TREASURY}/src/funding.rs")];

    // Premises. Both writers are declared where this test says they are, or
    // it would be guarding names that have moved again; and the scan finds an
    // identifier the planner does name, or a scan that saw nothing would
    // pass.
    const WRITERS: [&str; 2] = ["CapitalBook", "InternalFunding"];
    for writer in WRITERS {
        assert!(
            books.contains(&format!("pub struct {writer} {{")),
            "premise: {writer} is declared in qip-treasury/src/treasury.rs"
        );
    }
    assert!(
        names_identifier(planner, "FinancingPermissions")
            && names_identifier(planner, "plan_funding"),
        "premise: the scan reads the funding planner's code"
    );
    assert!(
        names_identifier("let book = CapitalBook::new();", "CapitalBook")
            && !names_identifier("let book = CapitalBookkeeping::new();", "CapitalBook")
            && !names_identifier("let log = MyEventLog;", "EventLog"),
        "premise: the match is on a whole identifier"
    );

    const EVENT_LOG: [&str; 3] = ["qip_events", "EventLog", "EventBody"];
    let named: Vec<&str> = WRITERS
        .into_iter()
        .chain(EVENT_LOG)
        .filter(|name| names_identifier(planner, name))
        .collect();
    assert!(
        named.is_empty(),
        "qip-treasury/src/funding.rs names {named:?}. The planner decides which source funds \
         which requirement and returns a plan; it does not write the treasury's books or the \
         event log. Return the value and let the kernel apply and journal it"
    );
}
