//! The ten Financial-OS capabilities are first-class (R12-CAPITAL-1020).
//!
//! The requirement is that each named capability has an owning crate with a
//! typed contract reached from a composition root. The failure it prevents is
//! a capability that exists only as a paragraph: a crate nothing deployable
//! links, or a name with no type behind it, reads as owned in a register and
//! is neither. Each row below names the crate and the type, and the test
//! proves the type is declared in that crate's source and the crate is in the
//! dependency closure of at least one binary that composes the platform.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

/// Binaries that compose the platform. `qip-ledgerd` is its own root: the
/// ledger is a process, not a library the others link.
const ROOTS: [&str; 5] = [
    "qip-api",
    "qip-fastbrain",
    "qip-deepbrain",
    "qip-edge-node",
    "qip-ledgerd",
];

/// (capability, owning crate, path under backend/crates (a source file or
/// directory), the contract's type name).
const CAPABILITIES: [(&str, &str, &str, &str); 10] = [
    (
        "asset management",
        "qip-portfolio-engine",
        "services/qip-portfolio-engine/src",
        "ProposalLeg",
    ),
    (
        "capital management",
        "qip-capital",
        "services/qip-capital/src",
        "CapitalAllocator",
    ),
    (
        "risk management",
        "qip-risk-engine",
        "services/qip-risk-engine/src",
        "AutonomyLevel",
    ),
    (
        "treasury/custody",
        "qip-treasury",
        "services/qip-treasury/src",
        "FundingSource",
    ),
    (
        "accounting/ledger",
        "qip-ledgerd",
        "apps/qip-ledgerd/src",
        "PartitionStatus",
    ),
    (
        "execution",
        "qip-execution-engine",
        "services/qip-execution-engine/src/oms.rs",
        "OrderManager",
    ),
    (
        "settlement",
        "qip-edge",
        "edge/qip-edge/src/settlement.rs",
        "SettlementTerms",
    ),
    (
        "market making",
        "qip-execution-engine",
        "services/qip-execution-engine/src/quoting.rs",
        "QuotePolicy",
    ),
    (
        "market creation",
        "qip-kernel",
        "runtime/qip-kernel/src/market_factory_stage.rs",
        "FactoryPlan",
    ),
    (
        "commerce",
        "qip-financial",
        "libs/qip-financial/src/commerce.rs",
        "Listing",
    ),
];

fn graph() -> BTreeMap<String, BTreeSet<String>> {
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

/// Every `.rs` file under `relative`, or the file itself.
fn sources(relative: &str) -> Vec<String> {
    let path = qip_acceptance::repository_root()
        .join("backend/crates")
        .join(relative);
    if path.is_file() {
        return vec![std::fs::read_to_string(&path).expect("source file")];
    }
    let mut out = Vec::new();
    let mut pending = vec![path];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                pending.push(p);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(std::fs::read_to_string(&p).expect("source file"));
            }
        }
    }
    out
}

/// True when some line declares `pub struct|enum|trait <name>` followed by a
/// delimiter, so `Order` does not match `OrderBook`.
fn declares(text: &str, name: &str) -> bool {
    ["struct", "enum", "trait"].iter().any(|kind| {
        text.lines().any(|line| {
            line.trim_start()
                .strip_prefix(&format!("pub {kind} {name}"))
                .is_some_and(|rest| {
                    rest.chars()
                        .next()
                        .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                })
        })
    })
}

#[test]
fn every_financial_os_capability_has_an_owning_crate_with_a_typed_contract() {
    for (capability, krate, path, symbol) in CAPABILITIES {
        let files = sources(path);
        assert!(!files.is_empty(), "{capability}: no source under {path}");
        assert!(
            files.iter().any(|text| declares(text, symbol)),
            "{capability}: `{krate}` declares no `pub` type named `{symbol}` under {path}"
        );
    }
}

#[test]
fn every_financial_os_owning_crate_is_reached_from_a_composition_root() {
    let graph = graph();
    let reached: BTreeSet<String> = ROOTS
        .iter()
        .flat_map(|root| {
            assert!(
                graph.contains_key(*root),
                "premise: root {root} is a member"
            );
            closure(root, &graph)
        })
        .collect();
    for (capability, krate, ..) in CAPABILITIES {
        assert!(
            graph.contains_key(krate),
            "{capability}: owning crate `{krate}` is not a workspace member"
        );
        assert!(
            reached.contains(krate),
            "{capability}: `{krate}` is linked by no composition root"
        );
    }
}

#[test]
fn the_declaration_matcher_does_not_confuse_a_type_with_its_longer_neighbour() {
    assert!(declares("pub struct Order {", "Order"));
    assert!(declares("pub enum Order\n", "Order"));
    assert!(!declares("pub struct OrderBook {", "Order"));
    assert!(!declares("struct Order {", "Order"));
}
