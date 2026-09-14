//! Blueprint §30.2's path router, across the two seams neither crate's own
//! tests can see.
//!
//! The first seam is the blueprint itself. `qip_routing::path` claims to
//! implement a numbered table in a document, and a claim like that rots in
//! one direction only: the table gains a row, the enum does not, and every
//! test in the crate keeps passing because they all read the enum. The two
//! tests below read the document.
//!
//! The second seam is `qip-arbitrage`. The router consumes a cycle the
//! search found, and the whole contract it relies on is one sentence of
//! `PathCandidate`'s doc comment — "edge indices in traversal order, the
//! last edge arrives where the first departs". If that ever stopped being
//! true the router's closure check would start refusing real cycles, and
//! the only test that could catch it is one that runs a real search and
//! routes what comes out. `qip-arbitrage` cannot depend on `qip-routing`
//! and `qip-routing`'s own tests build their graphs by hand, so it lives
//! here.
//!
//! The third seam is the paper-trading boundary as it applies to this
//! change: a router that selects a path must not become a router that
//! selects a venue or produces an order.
//!
//! The fourth seam appeared when the router gained a production caller. The
//! ADR that recorded it shipped saying, in as many words, that nothing called
//! it, and a capability with no caller is one nobody can rely on — the shape
//! it fails in being that every test in `qip-routing` keeps passing while the
//! platform never assigns a path to anything. Two tests hold the caller: one
//! that the edge cell really does reach the router and carry what it decided
//! out of a pass, and one that the *new dependency edge* did not quietly make
//! `qip-routing`'s gateway, its venue selection and its child orders reachable
//! from the file the paper-trading boundary rests on. The second is the one
//! that would not have existed before this change, because before it there was
//! no edge from `qip-edge` to `qip-routing` at all.

use qip_arbitrage::graph::{ArbitrageGraph, Node, VenueFacts};
use qip_arbitrage::search::{SearchSettings, search_candidates};
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::time::Duration;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_routing::path::{EdgeClass, ExecutionPath, MirrorFacts, PathPolicy};
use qip_routing::pathcycle::{CycleRouter, RepresentationClasses, VenueRegions};
use std::collections::{BTreeMap, BTreeSet};

const BLUEPRINT: &str = "docs/architecture/algorik-blueprint-v10.1-source.md";

/// Turn a blueprint table cell into the form `as_str` uses: lower case,
/// spaces and hyphens to underscores. "Firm-quote bridging" becomes
/// "firm_quote_bridging".
fn normalise(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// The rows of §30.2's assignment table, as `(number, normalised name)`.
///
/// The table renders one cell per line, and an assigned-path cell is the
/// only kind that begins with a digit and a space, so the rows are found by
/// that shape rather than by counting lines from a heading.
fn blueprint_paths() -> BTreeMap<u8, String> {
    let source = qip_acceptance::read(BLUEPRINT);
    let mut inside = false;
    let mut rows = BTreeMap::new();
    for line in source.lines() {
        if line.starts_with("30.2 Path Router") {
            inside = true;
            continue;
        }
        if inside && line.starts_with("31. The Eight Execution Paths") {
            break;
        }
        if !inside {
            continue;
        }
        let Some((number, name)) = line.split_once(" — ") else {
            continue;
        };
        let Ok(number) = number.trim().parse::<u8>() else {
            continue;
        };
        rows.insert(number, normalise(name));
    }
    rows
}

/// The names in §30's edge-class table.
///
/// The table is three header cells — "Edge class", "Connects", "Enables" —
/// and then one class per group of three. Taking every third line after the
/// header is what makes the names separable from their descriptions, which
/// are prose and would match any keyword scan.
fn blueprint_edge_classes() -> BTreeSet<String> {
    let source = qip_acceptance::read(BLUEPRINT);
    let mut rows: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in source.lines() {
        if line.trim() == "Edge class" {
            inside = true;
            continue;
        }
        if inside && line.starts_with("30.1 Detection") {
            break;
        }
        if inside {
            rows.push(line.trim());
        }
    }
    // "Connects" and "Enables" complete the header; the classes start after.
    rows.iter()
        .skip(2)
        .step_by(3)
        .map(|name| normalise(name))
        .collect()
}

#[test]
fn the_routers_eight_execution_paths_are_exactly_the_eight_the_blueprint_names() {
    let table = blueprint_paths();
    // Premise. A parser that found nothing would make every comparison
    // below vacuously true, which is precisely how a table that gained a
    // ninth row would go unnoticed.
    assert_eq!(
        table.len(),
        8,
        "§30.2's assignment table did not parse into eight rows: {table:?}"
    );
    assert_eq!(
        table.keys().copied().collect::<Vec<u8>>(),
        vec![1, 2, 3, 4, 5, 6, 7, 8],
        "§30.2's rows are not numbered one to eight: {table:?}"
    );

    let router: BTreeMap<u8, String> = ExecutionPath::ALL
        .iter()
        .map(|path| (path.number(), path.as_str().to_string()))
        .collect();
    // Whole-value equality on both the numbers and the names, not
    // `contains`: "cross_venue" is a substring of nothing here today and
    // would be of "intra_cross_venue" tomorrow.
    assert_eq!(
        router, table,
        "the router's eight paths and §30.2's eight rows disagree; the table is the \
         specification, so either the enum is missing a row or a row was renamed"
    );
}

#[test]
fn the_routers_six_edge_classes_are_exactly_the_six_the_blueprint_names() {
    let table = blueprint_edge_classes();
    assert_eq!(
        table.len(),
        6,
        "§30's edge-class table did not parse into six classes: {table:?}"
    );

    let router: BTreeSet<String> = [
        EdgeClass::Conversion,
        EdgeClass::Transport,
        EdgeClass::Mirror,
        EdgeClass::Basis,
        EdgeClass::Equivalence,
        EdgeClass::Settlement,
    ]
    .iter()
    .map(|class| class.as_str().to_string())
    .collect();
    assert_eq!(
        router, table,
        "the router's edge classes and §30's table disagree; the router routes over the \
         classes it can name, so a class present in the blueprint and absent here is a cycle \
         shape nothing can describe"
    );

    // The asymmetry that matters, asserted rather than left in prose:
    // settlement is one of the six classes and is named by no row of the
    // assignment table. The router refuses it for that reason, and if §30.2
    // ever gains a settlement row this assertion is where the refusal gets
    // revisited.
    let assigned: BTreeSet<String> = blueprint_paths().into_values().collect();
    assert!(
        table.contains("settlement"),
        "settlement is no longer one of §30's edge classes: {table:?}"
    );
    assert!(
        !assigned.contains("settlement"),
        "§30.2 now assigns a path named for settlement, so the router's refusal of a \
         settlement edge is no longer what the blueprint says: {assigned:?}"
    );
}

/// Source lines with doc comments removed.
///
/// A scan over a file that documents what it refuses to do finds its own
/// prose. `documentation.rs` learned this the hard way; the same trap is
/// live here, because `path.rs`'s module doc says in as many words that it
/// holds no gateway.
fn code_lines(relative: &str) -> String {
    qip_acceptance::read(relative)
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//!") && !trimmed.starts_with("///") && !trimmed.starts_with("//")
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

#[test]
fn the_path_router_names_nothing_that_could_place_an_order_or_name_a_venue_class() {
    // The paper-trading boundary as it applies to a router that chooses a
    // *path*. Selecting how a cycle would be executed must not become
    // selecting where, and must never name the venue class through which a
    // live class could enter. The router is inert by construction; this
    // asserts it stays inert as §31.1 and §33.1 extend it.
    const FORBIDDEN: [&str; 4] = ["Gateway", "ChildOrder", "is_simulated", "VenueClass"];

    let router = format!(
        "{}\n{}",
        code_lines("backend/crates/edge/qip-routing/src/path.rs"),
        code_lines("backend/crates/edge/qip-routing/src/pathcycle.rs")
    );
    // Premise: the files really were read and really hold code, or a scan
    // over an empty string would report the boundary intact forever.
    assert!(
        router.contains("pub enum ExecutionPath"),
        "the path router's source did not load; the scan below would pass over nothing"
    );

    // Vacuity guard. Every forbidden token must be *findable* by this scan
    // somewhere in the tree, or a typo in the list above would be a scan
    // whose precondition can never match — a gate that reads as protection
    // and cannot fire. `gateway.rs` is the venue-facing surface and
    // `qip-arbitrage`'s graph is where a venue class is named.
    let reachable = format!(
        "{}\n{}",
        code_lines("backend/crates/edge/qip-routing/src/gateway.rs"),
        code_lines("backend/crates/edge/qip-arbitrage/src/graph.rs")
    );
    for token in FORBIDDEN {
        assert!(
            reachable.contains(token),
            "{token} is forbidden in the path router but appears nowhere this scan can see, so \
             the scan proves nothing about it"
        );
    }

    for token in FORBIDDEN {
        assert!(
            !router.contains(token),
            "the path router names {token}; assignment is a classification over a description \
             of a cycle and must not reach an order path or a venue class"
        );
    }
}

fn now() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn object(id: &str) -> ObjectId {
    ObjectId::from_string(id)
}

fn venue(id: &str) -> VenueId {
    VenueId::new(id)
}

fn region(id: &str) -> qip_routing::path::RegionId {
    qip_routing::path::RegionId::new(id).expect("a test region id is valid")
}

fn rate(value: &str) -> Decimal {
    Decimal::parse(value).expect("a test rate parses")
}

/// A graph holding one profitable two-venue cycle, built the way the edge
/// node builds one.
fn profitable_two_venue_graph() -> ArbitrageGraph {
    let mut graph = ArbitrageGraph::new();
    for id in ["XNAS", "XLON"] {
        graph.register_venue(
            venue(id),
            VenueFacts::new(VenueClass::Exchange, VenueStatus::Open),
        );
    }
    graph
        .add_trade(
            Node::new(object("USD"), venue("XNAS")),
            Node::new(object("BTC"), venue("XNAS")),
            rate("0.00002"),
            Decimal::ZERO,
            object("BTCUSD"),
            BookSide::Ask,
            now(),
            8,
        )
        .expect("a trade edge");
    graph
        .add_transfer(
            object("BTC"),
            venue("XNAS"),
            venue("XLON"),
            Decimal::ZERO,
            now(),
            8,
        )
        .expect("a transfer edge");
    graph
        .add_trade(
            Node::new(object("BTC"), venue("XLON")),
            Node::new(object("USD"), venue("XLON")),
            rate("51000"),
            Decimal::ZERO,
            object("BTCUSD"),
            BookSide::Bid,
            now(),
            8,
        )
        .expect("a trade edge");
    graph
        .add_transfer(
            object("USD"),
            venue("XLON"),
            venue("XNAS"),
            Decimal::ZERO,
            now(),
            8,
        )
        .expect("a transfer edge");
    graph
}

#[test]
fn a_cycle_the_arbitrage_search_actually_found_is_assigned_a_path_from_the_searchs_own_edge_order()
{
    // The contract under test is one sentence of `PathCandidate`'s doc
    // comment: "edge indices in traversal order, the last edge arrives
    // where the first departs". The router's closure check depends on it
    // and nothing else in the tree asserts it across the two crates.
    let graph = profitable_two_venue_graph();
    let candidates = search_candidates(&graph, &SearchSettings::default());
    // Premise: the search really found something. A router asserted over an
    // empty candidate list would pass on a graph with no cycle at all.
    assert!(
        !candidates.is_empty(),
        "the search found no cycle in a graph built to contain one; the assertion below would \
         be over nothing"
    );

    let router = CycleRouter::new(
        PathPolicy::default(),
        VenueRegions::new()
            .with(venue("XNAS"), region("us-east"))
            .with(venue("XLON"), region("us-east")),
        RepresentationClasses::new(),
    );
    let mut routed = 0usize;
    for candidate in &candidates {
        let assignment = router
            .route(&graph, &candidate.edges, &BTreeMap::new())
            .expect("a cycle the search found should compose and route");
        assert_eq!(
            assignment.assigned(),
            ExecutionPath::CrossVenue,
            "a conversion-plus-transport cycle inside one region is §30.2's row 2"
        );
        routed += 1;
    }
    assert_eq!(
        routed,
        candidates.len(),
        "every found cycle should have been routed"
    );
}

#[test]
fn the_same_found_cycle_is_assigned_a_cross_region_path_when_its_venues_sit_in_two_regions() {
    // The failure this prevents, stated plainly: the graph cannot tell a
    // venue hop inside one region from one across an ocean, because
    // `EdgeKind::Transfer` holds no region. Routing the second as §30.2's
    // row 2 would dispatch a London leg under a mechanism whose entire
    // premise is one process and a 5-to-15 millisecond budget. Only the
    // region map differs between this test and the one above.
    let graph = profitable_two_venue_graph();
    let candidates = search_candidates(&graph, &SearchSettings::default());
    assert!(!candidates.is_empty(), "the search found no cycle");

    let router = CycleRouter::new(
        PathPolicy::default(),
        VenueRegions::new()
            .with(venue("XNAS"), region("us-east"))
            .with(venue("XLON"), region("eu-west")),
        RepresentationClasses::new(),
    );

    let mut checked = 0usize;
    for candidate in &candidates {
        let composition = router
            .composition(&graph, &candidate.edges)
            .expect("the cycle composes");
        // Premise for this candidate: it really does cross the two regions.
        assert_eq!(
            composition.regions().len(),
            2,
            "the candidate does not cross regions, so it proves nothing here"
        );
        let facts: BTreeMap<usize, MirrorFacts> = composition
            .mirror_edges()
            .into_iter()
            .map(|index| {
                (
                    index,
                    MirrorFacts::new(true, false, false, None, Duration::from_millis(28))
                        .expect("valid mirror facts"),
                )
            })
            .collect();
        assert!(
            !facts.is_empty(),
            "a cross-region cycle with no mirror edge is not a cross-region cycle"
        );
        let assignment = router
            .route(&graph, &candidate.edges, &facts)
            .expect("a cross-region cycle with inventory on both sides routes");
        assert_eq!(assignment.assigned(), ExecutionPath::MirroredInventory);
        assert!(
            !assignment.eligible().contains(&ExecutionPath::CrossVenue),
            "a cycle that leaves the region must never be eligible for the single-process path"
        );
        checked += 1;
    }
    assert_eq!(checked, candidates.len());
}

/// Every shipped source file of a crate, joined, with doc comments removed.
///
/// `tests/` is excluded deliberately: a test may name whatever it needs to
/// prove a refusal, and a scan that read them would be asserting a property
/// of the test suite rather than of the platform.
fn shipped_code(crate_relative: &str) -> String {
    let root = qip_acceptance::repository_root();
    qip_acceptance::files_with_extension(&format!("{crate_relative}/src"), "rs")
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            code_lines(&relative)
        })
        .collect::<Vec<String>>()
        .join("\n")
}

#[test]
fn the_edge_cell_calls_the_path_router_and_carries_what_it_decided_out_of_a_pass() {
    // ADR 0068 shipped recording that the router had **no production
    // caller**, and said so explicitly so that nobody would read the record
    // as a claim that it ships. This is the test that would fail if the
    // caller were removed again, and it lives here rather than in `qip-edge`
    // because the property spans two crates: that the edge's own pass report
    // carries the routing crate's own decision type.
    //
    // The compile-time half first. These bind nothing and assert what a grep
    // cannot: `qip_edge::RoutedCycle` really is built out of `qip_routing`'s
    // assignment, so the two crates cannot drift apart while a source scan
    // keeps matching. If `WorkReport::paths` were removed or retyped, this
    // stops compiling.
    let _: fn(&qip_edge::RoutedCycle) -> ExecutionPath = qip_edge::RoutedCycle::path;
    let report = qip_edge::WorkReport::default();
    let _: &Vec<qip_edge::RoutedCycle> = &report.paths;

    // And the call itself, in shipped code rather than in a test. The
    // compile-time half above would still hold if `Cell::work` never routed
    // anything, because a type can be carried and never filled — which is
    // exactly the failure mode this whole lane exists to close.
    let cell = code_lines("backend/crates/edge/qip-edge/src/cell.rs");
    // Premise: the file loaded and holds code. A scan over an empty string
    // proves nothing, forever.
    assert!(
        cell.contains("pub fn work("),
        "the cell's source did not load; the scan below would pass over nothing"
    );
    for named in ["CycleRouter", "GATE_PATH_ROUTER"] {
        assert!(
            cell.contains(named),
            "the edge cell no longer names {named}; §30.2's router has lost its production \
             caller and is built-and-uncalled again"
        );
    }
    assert!(
        cell.contains(".route("),
        "the edge cell holds a CycleRouter and never calls it; a router constructed and not \
         consulted is worse than none, because it reads in the source as a control"
    );
}

#[test]
fn the_edge_cell_reaches_the_path_vocabulary_and_no_other_part_of_the_routing_crate() {
    // What this change actually risked. Before it, `qip-edge` did not depend
    // on `qip-routing` at all; now it does, and `qip-routing` is the crate
    // that holds `Gateway`, `NativeGateway`, venue selection and child
    // orders. The dependency the path router needed also made every one of
    // those importable from `cell.rs` — the one file of the three the
    // paper-trading boundary structurally rests on.
    //
    // The cell must send through its own `Placer` seam and nowhere else.
    // `Cell::send` is the single place a `Placer` is called and the single
    // place a live venue is refused at the wire; a second order path reached
    // through this new edge would be a second place that refusal has to hold,
    // and the second place is always the one nobody adds it to.
    const PERMITTED: [&str; 2] = ["qip_routing::path", "qip_routing::pathcycle"];
    const FORBIDDEN: [&str; 6] = [
        "qip_routing::gateway",
        "qip_routing::router",
        "qip_routing::children",
        "qip_routing::reprice",
        "qip_routing::ordertype",
        "qip_routing::venue",
    ];

    let edge = shipped_code("backend/crates/edge/qip-edge");
    // Premise: the crate's shipped source loaded, and it really does reach
    // the routing crate. Without this the whole scan passes over a crate that
    // names `qip_routing` nowhere, which is true of most of the workspace.
    assert!(
        edge.contains("pub fn work("),
        "the edge cell's source did not load; the scan below would pass over nothing"
    );
    assert!(
        edge.contains("qip_routing::"),
        "qip-edge names qip_routing nowhere in shipped code, so this scan asserts nothing about \
         which part of it the cell may reach"
    );

    // Vacuity guard. Every named path must be a module that actually exists,
    // or a typo above is a gate that can never fire.
    let routing = code_lines("backend/crates/edge/qip-routing/src/lib.rs");
    for named in FORBIDDEN.iter().chain(PERMITTED.iter()) {
        let module = named.rsplit_once("::").map_or(*named, |(_, module)| module);
        assert!(
            routing.contains(&format!("pub mod {module};")),
            "{named} names no module of qip-routing, so naming it here proves nothing"
        );
    }

    for forbidden in FORBIDDEN {
        assert!(
            !edge.contains(forbidden),
            "qip-edge reaches {forbidden}; the path router is a classification and the cell \
             sends through its own Placer seam, so nothing here may name the routing crate's \
             order path or its venue selection"
        );
    }

    // And the positive half, so the test is not satisfied by a cell that
    // reaches nothing: the two permitted modules are the ones it does reach.
    for permitted in PERMITTED {
        assert!(
            edge.contains(permitted),
            "qip-edge no longer reaches {permitted}; §30.2's router is the only reason this \
             dependency edge exists, so an edge that reaches neither module should be removed \
             rather than left as a pinned claim"
        );
    }
}
