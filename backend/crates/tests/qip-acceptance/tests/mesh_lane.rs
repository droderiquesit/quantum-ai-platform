//! MESH-013: the five mesh coordination functions run in Lane 1, and no
//! Lane 0 code path can await one.
//!
//! The failure this prevents is the one Lane 1 exists to make impossible: a
//! reflex pass written to wait for a peer's reservation, a FIRE or an unwind
//! command. Such a pass still compiles, still passes every test that runs
//! with a peer present, and stops deciding the moment a region is
//! partitioned — which is when a cell most needs to decide alone (MESH-002).
//!
//! `qip_mesh::peer::MeshFunction` is the registry: it places each of the five
//! functions blueprint §4 names in a lane, and `PeerMessage::function` maps
//! every wire kind onto one. That is a claim. This suite is what makes it a
//! check, by reading the lane-placement register and the Lane 0 crates'
//! sources: a function cannot be awaited by code that cannot name it.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_mesh::peer::{Lane, MeshFunction};
use std::collections::{BTreeMap, BTreeSet};

const REGISTER: &str = "docs/architecture/lane-placement.md";

/// The crate that defines the peer protocol's message kinds, as the
/// register and a manifest spell it, and as source code spells it.
const HOST_CRATE: &str = "qip-mesh";
const HOST_PATH: &str = "qip_mesh";

/// The types a coordination message is built from or handled by. A Lane 0
/// crate naming one has either reached the host crate or copied its
/// protocol, and either way holds a coordination function in the wrong lane.
const COORDINATION_TYPES: [&str; 5] = [
    "PeerMessage",
    "OpportunityEpoch",
    "PeerEndpoint",
    "ReservationOutcome",
    "MeshFunction",
];

/// Crate name to lane, from the register's table rows.
fn register() -> BTreeMap<String, u8> {
    let mut rows = BTreeMap::new();
    for line in qip_acceptance::read(REGISTER).lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 5 || !cells[1].starts_with("qip-") {
            continue;
        }
        let lane: u8 = cells[2]
            .parse()
            .unwrap_or_else(|_| panic!("{} has a lane that is not a number", cells[1]));
        rows.insert(cells[1].to_string(), lane);
    }
    rows
}

/// Where each workspace crate lives, by name.
fn crate_directories() -> BTreeMap<String, std::path::PathBuf> {
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
            if member.path().join("Cargo.toml").is_file() {
                found.insert(
                    member.file_name().to_string_lossy().into_owned(),
                    member.path(),
                );
            }
        }
    }
    found
}

/// The `qip-*` crates a manifest names under `[dependencies]`.
fn normal_dependencies(manifest: &str) -> BTreeSet<String> {
    let mut in_section = false;
    let mut found = BTreeSet::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[dependencies]";
        } else if in_section && line.starts_with("qip-") {
            found.insert(
                line.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect(),
            );
        }
    }
    found
}

/// A source file with its line comments removed.
///
/// Comments are removed because Lane 0 crates legitimately *mention* the
/// host crate — `qip_edge::mesh` explains at length why it cannot name
/// `qip_mesh::spine` — and a mention is not a use. A `//` inside a string
/// literal would be cut short by this, which can only hide text after it on
/// the same line; the identifiers searched for are never written after a
/// URL on one line in these crates, and the premise below proves the scan
/// still finds a real use.
fn without_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `text` holds `word` as a whole identifier, not as part of a
/// longer one. `contains("PeerEndpoint")` would be true of
/// `SpinePeerEndpointConfig`, and a substring match is how a guard fires on
/// a neighbour or, worse, is satisfied by one.
fn names(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Every non-comment source text under a crate's `src/`.
fn sources(directory: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    fn walk(directory: &std::path::Path, found: &mut Vec<(std::path::PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file is readable");
                found.push((path, without_comments(&text)));
            }
        }
    }
    let mut found = Vec::new();
    walk(&directory.join("src"), &mut found);
    found
}

#[test]
fn the_five_coordination_functions_are_lane_one_and_no_lane_zero_crate_can_name_one_to_await_it() {
    let register = register();
    let directories = crate_directories();

    // Premise: the register places the crate that hosts the functions, and
    // it is the lane the registry in the code declares for every one of
    // them. Two claims about one fact, compared rather than trusted.
    let host_lane = *register
        .get(HOST_CRATE)
        .unwrap_or_else(|| panic!("{REGISTER} has no row for {HOST_CRATE}"));
    assert_eq!(MeshFunction::ALL.len(), 5, "premise: five functions");
    for function in MeshFunction::ALL {
        assert_eq!(
            function.lane(),
            Lane::CoordinatedFast,
            "{} is not registered in Lane 1 (Coordinated Fast)",
            function.as_str()
        );
        assert_eq!(
            function.lane().number(),
            host_lane,
            "{} is registered in lane {} and the crate that implements it, {HOST_CRATE}, is \
             placed in lane {host_lane} by {REGISTER}",
            function.as_str(),
            function.lane().number()
        );
    }

    // Premise: there is a Lane 0 to check, and it holds the cell and the
    // binary that runs it. A register edit that emptied Lane 0 would
    // otherwise make everything below pass over nothing.
    let lane_zero: BTreeSet<&str> = register
        .iter()
        .filter(|(_, lane)| **lane == 0)
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        lane_zero.len() >= 10
            && lane_zero.contains("qip-edge")
            && lane_zero.contains("qip-edge-node"),
        "premise: Lane 0 holds the reflex cell and its node, found {lane_zero:?}"
    );
    assert!(!lane_zero.contains(HOST_CRATE));

    // Premise: the scan can see a use where there is one. The kernel is a
    // slower-lane crate that really does import the host crate; if the
    // scanner cannot find it there, finding nothing in Lane 0 means nothing.
    let kernel = directories
        .get("qip-kernel")
        .expect("the kernel is a workspace crate");
    assert!(
        sources(kernel)
            .iter()
            .any(|(_, text)| names(text, HOST_PATH)),
        "premise: the scan finds `{HOST_PATH}` in qip-kernel, which imports it"
    );
    assert!(
        sources(directories.get(HOST_CRATE).expect("the host crate exists"))
            .iter()
            .any(|(_, text)| COORDINATION_TYPES.iter().all(|name| names(text, name))),
        "premise: the scan finds every coordination type where it is defined"
    );

    let mut scanned = 0;
    for name in &lane_zero {
        let directory = directories
            .get(*name)
            .unwrap_or_else(|| panic!("{name} is in the register and not in the workspace"));

        // The dependency: a crate that does not depend on the host cannot
        // call it. Checked on each Lane 0 manifest directly, so the refusal
        // names the crate that took the edge.
        let manifest =
            std::fs::read_to_string(directory.join("Cargo.toml")).expect("a crate has a manifest");
        assert!(
            !normal_dependencies(&manifest).contains(HOST_CRATE),
            "{name} is Lane 0 and depends on {HOST_CRATE}: a reflex crate that can call the \
             peer protocol can wait on a peer. Coordinate from a Lane 1 process and hand the \
             cell a decision it does not wait for (MESH-013)"
        );

        // The source: no Lane 0 code names the host crate or a coordination
        // type, by import, by path or by a copied definition.
        for (path, text) in sources(directory) {
            scanned += 1;
            assert!(
                !names(&text, HOST_PATH),
                "{} names `{HOST_PATH}` outside a comment; {name} is Lane 0 and may not reach \
                 a Lane 1 coordination function (MESH-013)",
                path.display()
            );
            for coordination in COORDINATION_TYPES {
                assert!(
                    !names(&text, coordination),
                    "{} names `{coordination}`; {name} is Lane 0 and a peer coordination type \
                     there is a coordination function in the reflex lane (MESH-013)",
                    path.display()
                );
            }
        }
    }
    assert!(
        scanned > 100,
        "premise: the Lane 0 scan read the reflex crates' sources, and it read {scanned} files"
    );
}

#[test]
fn a_word_is_matched_as_an_identifier_and_a_comment_is_not_a_use() {
    // The scanner's own two edges, because a guard that misreads is worse
    // than none: it either fires on a neighbour or is satisfied by one.
    assert!(names("use qip_mesh::peer::PeerMessage;", "qip_mesh"));
    assert!(names("use qip_mesh::peer::PeerMessage;", "PeerMessage"));
    assert!(!names("struct SpinePeerEndpointConfig;", "PeerEndpoint"));
    assert!(!names("let qip_mesh_peer = 1;", "qip_mesh"));
    assert!(!names(
        &without_comments("// `qip_mesh::spine` is a service and cannot be named here"),
        "qip_mesh"
    ));
    assert!(names(
        &without_comments("use qip_mesh::spine; // the central half"),
        "qip_mesh"
    ));
}
