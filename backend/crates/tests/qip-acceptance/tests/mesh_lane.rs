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
//!
//! ADR 0116 gave it the node's teeth as well. The node hosts the peer link
//! (blueprint §16 makes the hop node to node), so the binary links Lane 1
//! code; what keeps the reflex pass from waiting on it is that the decision
//! loop holds only `qip_peer_link::boundary::ReflexPort`, whose methods cannot
//! wait. The last two tests read the node's modules and that port's file and
//! fail if either stops being true.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_mesh::peer::{Lane, MeshFunction};
use std::collections::{BTreeMap, BTreeSet};

const REGISTER: &str = "docs/architecture/lane-placement.md";

/// The crate that hosts the peer protocol's five functions, as the register
/// and a manifest spell it, and as source code spells it.
const HOST_CRATE: &str = "qip-mesh";
const HOST_PATH: &str = "qip_mesh";

/// The crate the protocol is defined in since ADR 0116, which `qip-mesh`
/// re-exports as `qip_mesh::peer`, and the file the definitions are in.
const PROTOCOL_CRATE: &str = "qip-peer";
const DEFINING_FILE: &str = "backend/crates/libs/qip-peer/src/lib.rs";

/// The peer link: the wire, the session and the port (ADR 0116 §2).
const LINK_CRATE: &str = "qip-peer-link";

/// Every Lane 1 crate the edge node hosts, and nothing else. ADR 0116 fixes
/// this set; a reviewer refuses to grow it casually, because a node hosting
/// a second coordinator is a node becoming a coordinator, and the ADR says
/// that reopens the sidecar rather than widening this constant.
const HOSTED: [&str; 2] = [LINK_CRATE, PROTOCOL_CRATE];

/// Every crate and path a Lane 0 crate may not reach, by manifest and by
/// source.
const PEER_CRATES: [&str; 3] = [HOST_CRATE, PROTOCOL_CRATE, LINK_CRATE];
const PEER_PATHS: [&str; 3] = [HOST_PATH, "qip_peer", "qip_peer_link"];

/// The node's composition root: the one node source that may name the link
/// beyond its port, because it spawns it.
const NODE: &str = "qip-edge-node";
const COMPOSITION_ROOT: &str = "main.rs";

/// The decision loop's whole view of Lane 1, and what it may declare.
const BOUNDARY_FILE: &str = "backend/crates/edge/qip-peer-link/src/boundary.rs";
const PORT_METHODS: [&str; 2] = ["try_publish", "status"];
const PORT_CONSTRUCTOR: &str = "new";
/// What would let the port's holder wait: a receive, a join, a lock, a wait
/// or a sleep. `Mutex` and `Condvar` are named too, since a lock that is
/// only ever taken through a helper is still a lock the loop can wait on.
const WAITS: [&str; 8] = [
    ".recv(",
    "recv_timeout",
    ".join(",
    ".lock(",
    ".wait(",
    "sleep",
    "Mutex",
    "Condvar",
];

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

/// Each workspace crate's normal `qip-*` dependencies.
fn graph(directories: &BTreeMap<String, std::path::PathBuf>) -> BTreeMap<String, BTreeSet<String>> {
    directories
        .iter()
        .map(|(name, directory)| {
            let manifest = std::fs::read_to_string(directory.join("Cargo.toml"))
                .expect("a crate has a manifest");
            (name.clone(), normal_dependencies(&manifest))
        })
        .collect()
}

/// `root` and everything it links through normal dependencies.
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
    let graph = graph(&directories);

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
    // And the crate the definitions live in is placed with them: a protocol
    // crate registered at Lane 0 would be one a reflex crate may link.
    assert_eq!(
        register.get(PROTOCOL_CRATE).copied(),
        Some(host_lane),
        "{REGISTER} must place {PROTOCOL_CRATE}, which defines the five functions, in lane \
         {host_lane} with {HOST_CRATE}"
    );

    // Premise: there is a Lane 0 to check, and it holds the cell. A register
    // edit that emptied Lane 0 would otherwise make everything below pass
    // over nothing.
    let lane_zero: BTreeSet<&str> = register
        .iter()
        .filter(|(_, lane)| **lane == 0)
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        lane_zero.len() >= 10 && lane_zero.contains("qip-edge"),
        "premise: Lane 0 holds the reflex cell, found {lane_zero:?}"
    );
    for peer in PEER_CRATES {
        assert!(!lane_zero.contains(peer), "{peer} is registered at Lane 0");
    }

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
    // Where the types are *defined* moved twice: to `qip_contracts::peer` in
    // `215138c2`, which put them in a Lane 0 crate and failed this test, and
    // to `qip-peer` under ADR 0116. The definitions are read there, and the
    // host crate is held to still re-exporting them, so every central
    // `qip_mesh::peer` path names the same types.
    let defining = without_comments(&qip_acceptance::read(DEFINING_FILE));
    assert!(
        COORDINATION_TYPES.iter().all(|name| names(&defining, name)),
        "premise: the scan finds every coordination type where it is defined, {DEFINING_FILE}"
    );
    assert!(
        sources(directories.get(HOST_CRATE).expect("the host crate exists"))
            .iter()
            .any(|(_, text)| text.contains("pub use qip_peer as peer;")),
        "premise: {HOST_CRATE} re-exports the protocol it is registered as hosting"
    );

    // The reflex set: the cell and everything it links, and every crate the
    // register places at Lane 0. The union, so neither a register edit nor a
    // new dependency of the cell can take a crate out of the scan.
    let mut reflex: BTreeSet<String> = closure("qip-edge", &graph);
    reflex.extend(lane_zero.iter().map(|name| (*name).to_string()));
    assert!(
        reflex.contains("qip-contracts") && reflex.contains("qip-edge"),
        "premise: the reflex set holds the cell and the shared vocabulary, which held the \
         protocol until ADR 0116 and must now come up clean: {reflex:?}"
    );

    let mut scanned = 0;
    for name in &reflex {
        let directory = directories
            .get(name)
            .unwrap_or_else(|| panic!("{name} is in the register and not in the workspace"));

        // The dependency: a crate that does not depend on the peer crates
        // cannot call them. Checked on each manifest directly, so the refusal
        // names the crate that took the edge.
        let dependencies = &graph[name];
        for peer in PEER_CRATES {
            assert!(
                !dependencies.contains(peer),
                "{name} is on the reflex path and depends on {peer}: a reflex crate that can \
                 call the peer protocol can wait on a peer. Coordinate from a Lane 1 process \
                 and hand the cell a decision it does not wait for (MESH-013)"
            );
        }

        // The source: no reflex code names a peer crate or a coordination
        // type, by import, by path or by a copied definition.
        for (path, text) in sources(directory) {
            scanned += 1;
            for peer in PEER_PATHS {
                assert!(
                    !names(&text, peer),
                    "{} names `{peer}` outside a comment; {name} is on the reflex path and \
                     may not reach a Lane 1 coordination function (MESH-013)",
                    path.display()
                );
            }
            for coordination in COORDINATION_TYPES {
                assert!(
                    !names(&text, coordination),
                    "{} names `{coordination}`; {name} is on the reflex path and a peer \
                     coordination type there is a coordination function in the reflex lane \
                     (MESH-013)",
                    path.display()
                );
            }
        }
    }
    assert!(
        scanned > 100,
        "premise: the reflex scan read the reflex crates' sources, and it read {scanned} files"
    );

    // The node hosts the link and nothing slower. Every crate it links at
    // Lane 1 or slower must be in the fixed hosted set; anything else is a
    // coordinator compiled beside the cell with no port in between.
    let node = closure(NODE, &graph);
    for hosted in HOSTED {
        assert!(
            node.contains(hosted),
            "premise: {NODE} links {hosted}; if it no longer hosts it, shrink the hosted set \
             rather than leave a constant that exempts a crate nobody links"
        );
        assert_eq!(
            register.get(hosted).copied(),
            Some(1),
            "{hosted} is hosted by {NODE} as Lane 1 and must be registered at Lane 1"
        );
    }
    let slower: Vec<String> = node
        .iter()
        .filter(|name| *name != NODE && !HOSTED.contains(&name.as_str()))
        .filter(|name| register.get(*name).is_some_and(|lane| *lane >= 1))
        .cloned()
        .collect();
    assert!(
        slower.is_empty(),
        "{NODE} links {slower:?}, registered at Lane 1 or slower and not in the hosted set \
         {HOSTED:?}: the node is becoming a coordinator. ADR 0116 says reopen the sidecar \
         rather than widen the set"
    );
}

/// Every node source other than the composition root reaches the peer link
/// only through its port, and names no coordination type, session or wire.
///
/// The failure this prevents: the scheduler holding a `PeerSession` again.
/// That compiles, it is what `215138c2` shipped, and it puts the endpoint's
/// receive, frame and expire work back inside the reflex budget; nothing at
/// crate level can see it, because the node legitimately links the link.
#[test]
fn no_node_module_but_the_composition_root_reaches_the_peer_link_past_its_port() {
    let directories = crate_directories();
    let node = directories
        .get(NODE)
        .expect("the node is a workspace crate");
    let files = sources(node);
    let is_root = |path: &std::path::Path| {
        path.file_name().is_some_and(|f| f == COMPOSITION_ROOT)
            && path.parent().is_some_and(|p| p.ends_with("src"))
    };

    // Premise: the scan sees the port where the decision loop holds it, and
    // the spawn where the root makes it. Without these, a node that had
    // stopped hosting the link would pass for the wrong reason.
    let named_port = files
        .iter()
        .any(|(path, text)| !is_root(path) && text.contains("qip_peer_link::boundary::ReflexPort"));
    assert!(
        named_port,
        "premise: a node module holds qip_peer_link::boundary::ReflexPort"
    );
    let root = files
        .iter()
        .find(|(path, _)| is_root(path))
        .expect("premise: the node has a composition root");
    assert!(
        root.1.contains("qip_peer_link::spawn("),
        "premise: the composition root spawns the peer link"
    );

    let forbidden: Vec<&str> = COORDINATION_TYPES
        .iter()
        .copied()
        .chain(["PeerSession", "PeerLink"])
        .collect();
    let mut scanned = 0;
    for (path, text) in files.iter().filter(|(path, _)| !is_root(path)) {
        scanned += 1;
        for path_name in ["qip_peer", HOST_PATH] {
            assert!(
                !names(text, path_name),
                "{} names `{path_name}`; a node module other than {COMPOSITION_ROOT} reaches \
                 Lane 1 only through qip_peer_link::boundary (ADR 0116, MESH-013)",
                path.display()
            );
        }
        let link = "qip_peer_link";
        for (at, _) in text.match_indices(link) {
            let before = text[..at].chars().next_back();
            let after = text[at + link.len()..].chars().next();
            let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
            if before.is_some_and(is_ident) || after.is_some_and(is_ident) {
                continue;
            }
            assert!(
                text[at + link.len()..].starts_with("::boundary::"),
                "{} names `{link}` other than as `{link}::boundary::…`; only {COMPOSITION_ROOT} \
                 may reach past the port (ADR 0116, MESH-013)",
                path.display()
            );
        }
        for name in &forbidden {
            assert!(
                !names(text, name),
                "{} names `{name}`; the decision loop holds the port and nothing it could wait \
                 on a peer through (ADR 0116, MESH-013)",
                path.display()
            );
        }
    }
    assert!(scanned > 20, "premise: the node scan read {scanned} files");
}

/// The port declares only the two methods ADR 0116 permits, and nothing in
/// its file can wait.
///
/// The failure this prevents: a `recv` added to the port "just for the
/// arbitrage desk". It would compile, every test with a peer present would
/// pass, and the first partition would stop the pass at that call
/// (MESH-002). The port is the whole of what the decision loop holds, so a
/// method that cannot wait on it is a decision loop that cannot wait.
#[test]
fn the_reflex_port_declares_only_its_two_methods_and_none_can_wait() {
    let text = without_comments(&qip_acceptance::read(BOUNDARY_FILE));

    let mut public = BTreeSet::new();
    let mut restricted = BTreeSet::new();
    for line in text.lines().map(str::trim) {
        let Some(at) = line.find("fn ") else {
            continue;
        };
        if !line.starts_with("pub") {
            continue;
        }
        let name: String = line[at + "fn ".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if line.starts_with("pub(") {
            restricted.insert(name);
        } else {
            public.insert(name);
        }
    }
    // Premise: the parse found the port's methods, so an empty "outside the
    // list" below is a finding and not a parser that saw nothing.
    for method in PORT_METHODS {
        assert!(
            public.contains(method),
            "premise: {BOUNDARY_FILE} declares `pub fn {method}`; found {public:?}"
        );
    }
    let extra: Vec<&String> = public
        .iter()
        .filter(|name| !PORT_METHODS.contains(&name.as_str()))
        .collect();
    assert!(
        extra.is_empty(),
        "{BOUNDARY_FILE} declares `pub fn` {extra:?} beyond {PORT_METHODS:?}; the port the \
         decision loop holds may offer and report and do nothing else (ADR 0116 §3)"
    );
    let extra: Vec<&String> = restricted
        .iter()
        .filter(|name| name.as_str() != PORT_CONSTRUCTOR)
        .collect();
    assert!(
        extra.is_empty(),
        "{BOUNDARY_FILE} declares restricted fn {extra:?}; only the `{PORT_CONSTRUCTOR}` \
         constructor is permitted (ADR 0116 §3)"
    );

    for wait in WAITS {
        assert!(
            !text.contains(wait),
            "{BOUNDARY_FILE} contains `{wait}`: the port the reflex decision loop holds can now \
             wait on a peer (ADR 0116 §3, MESH-013)"
        );
    }
    for name in COORDINATION_TYPES
        .iter()
        .chain(&["PeerSession", "PeerLink"])
    {
        assert!(
            !names(&text, name),
            "{BOUNDARY_FILE} names `{name}`; the port must hand the decision loop no \
             coordination type"
        );
    }
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
