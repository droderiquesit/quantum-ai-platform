//! ARCH-013: the lane-placement register is real, complete, and refuses a
//! crate placed in a lane faster than its dependencies allow.
//!
//! The failure this prevents: a Lane 0 crate gaining a normal dependency on a
//! Lane 3 crate (a model, a world-model read, a quorum write) passes every
//! other gate here, because the dependency-direction rule only knows
//! libs/services/runtime/edge, not time budgets. The register names the
//! budget; this suite makes the register bite.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

/// Crate name to (lane, requirement) read from the register's table rows.
fn register() -> BTreeMap<String, (u8, String)> {
    let mut rows = BTreeMap::new();
    for line in qip_acceptance::read("docs/architecture/lane-placement.md").lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 5 || !cells[1].starts_with("qip-") {
            continue;
        }
        let lane: u8 = cells[2].parse().unwrap_or_else(|_| {
            panic!("{} has a lane that is not a number: {}", cells[1], cells[2])
        });
        let previous = rows.insert(cells[1].to_string(), (lane, cells[3].to_string()));
        assert!(
            previous.is_none(),
            "{} has two rows in the register",
            cells[1]
        );
    }
    rows
}

/// Every workspace crate with the normal (non-dev, non-build) `qip-*`
/// dependencies its manifest declares.
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
            let name = member.file_name().to_string_lossy().into_owned();
            found.insert(name, normal_dependencies(&manifest));
        }
    }
    found
}

fn normal_dependencies(manifest: &str) -> BTreeSet<String> {
    let mut in_section = false;
    let mut found = BTreeSet::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[dependencies]";
        } else if in_section && line.starts_with("qip-") {
            let name: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            found.insert(name);
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
fn every_workspace_crate_has_exactly_one_lane_and_a_stated_requirement() {
    let register = register();
    let graph = workspace();
    // Premise: both sides were actually read.
    assert!(
        register.len() > 50,
        "register parsed {} rows",
        register.len()
    );
    assert!(graph.len() > 50, "walk found {} crates", graph.len());

    let crates: BTreeSet<&String> = graph.keys().collect();
    let rows: BTreeSet<&String> = register.keys().collect();
    let unplaced: Vec<_> = crates.difference(&rows).collect();
    let phantom: Vec<_> = rows.difference(&crates).collect();
    assert!(
        unplaced.is_empty(),
        "crates with no lane in docs/architecture/lane-placement.md: {unplaced:?}"
    );
    assert!(
        phantom.is_empty(),
        "register rows naming no crate: {phantom:?}"
    );
    for (name, (lane, requirement)) in &register {
        assert!(
            *lane <= 4,
            "{name} is placed in lane {lane}; the lanes are 0 to 4"
        );
        assert!(
            requirement.len() > 10,
            "{name} states no correctness requirement"
        );
    }
}

#[test]
fn the_lane_zero_set_is_exactly_what_the_reflex_node_compiles() {
    let register = register();
    let graph = workspace();
    let compiled = closure("qip-edge-node", &graph);
    // Premise: the closure is not trivially small.
    assert!(
        compiled.len() > 15,
        "closure of qip-edge-node is {compiled:?}"
    );
    let lane_zero: BTreeSet<String> = register
        .iter()
        .filter(|(_, (lane, _))| *lane == 0)
        .map(|(name, _)| name.clone())
        .collect();
    assert_eq!(
        lane_zero, compiled,
        "the register's Lane 0 must be what qip-edge-node actually links: a crate in the \
         closure but registered slower is a slow function in the hot binary, and one \
         registered at Lane 0 but absent is a placement nobody exercises"
    );
}

#[test]
fn no_fast_lane_crate_depends_on_a_slower_lane_crate() {
    let register = register();
    let graph = workspace();
    let mut checked = 0;
    let mut offences = Vec::new();
    for (name, (lane, _)) in register.iter().filter(|(_, (lane, _))| *lane <= 1) {
        for dependency in &graph[name] {
            checked += 1;
            let (dependency_lane, _) = &register[dependency];
            if dependency_lane > lane {
                offences.push(format!(
                    "{name} (lane {lane}) -> {dependency} (lane {dependency_lane})"
                ));
            }
        }
    }
    assert!(checked > 20, "only {checked} fast-lane edges were examined");
    assert!(
        offences.is_empty(),
        "a faster lane depends on a slower one: {offences:?}"
    );
}
