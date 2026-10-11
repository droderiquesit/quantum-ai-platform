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

/// TICK-024: historical tick replay, training, representation learning and
/// fusion are Lane 3 workloads. The failure this prevents: a convenience
/// dependency from the reflex binary on one of them compiles replay or
/// training code into the hot path, and nothing else here names those four.
#[test]
fn replay_training_and_world_model_crates_are_lane_three_and_absent_from_the_reflex_node() {
    let register = register();
    let graph = workspace();
    let compiled = closure("qip-edge-node", &graph);
    // Premise: the closure is real, so absence below is not vacuous.
    assert!(compiled.contains("qip-edge"), "closure is {compiled:?}");
    for slow in [
        "qip-simulation-engine",
        "qip-training",
        "qip-twin",
        "qip-world-model",
    ] {
        let (lane, _) = register
            .get(slow)
            .unwrap_or_else(|| panic!("{slow} has no row in the lane register"));
        assert_eq!(*lane, 3, "{slow} must be registered as Lane 3");
        assert!(
            !compiled.contains(slow),
            "qip-edge-node links {slow}: move the work to a Lane 3 service, not the hot binary"
        );
    }
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

/// WORLD-027: world models, their competing branches, arbitration, the
/// reasoner, the planner and the specialist agents run in Lane 3, the
/// cognitive slow lane, and nothing faster can link one.
///
/// The failure this prevents: the three tests above hold the *reflex* lanes.
/// None of them holds the cognitive crates to Lane 3. Editing one cell of the
/// register re-places the world model in Lane 2 with this whole suite green,
/// and a Lane 2 crate taking a dependency on the reasoner passes too, because
/// `no_fast_lane_crate_depends_on_a_slower_lane_crate` examines Lanes 0 and 1
/// only. Either one is a minutes-to-days computation placed where a cycle
/// budget is promised.
///
/// No symbolic reasoner and no ambient world model exists as a component to
/// place (the ambient vocabulary is types in `qip-contracts`, fenced from the
/// order path by the `ambient_attention` suite); when one is built it joins
/// the table below.
#[test]
fn every_world_model_arbitration_reasoner_planner_and_specialist_agent_crate_runs_in_the_cognitive_slow_lane()
 {
    const COGNITIVE_SLOW: u8 = 3;
    let register = register();
    let graph = workspace();

    // Each component the requirement names, the crate it lives in, and a line
    // of source proving it lives there. Without the proof this is a list of
    // crate names, and a component moved to a new crate would leave the old
    // name passing while the component itself sat wherever it landed.
    let components = [
        (
            "the world model",
            "qip-world-model",
            "services/qip-world-model/src/world.rs",
            "pub struct WorldModel {",
        ),
        (
            "competing hypothesis branches",
            "qip-world-model",
            "services/qip-world-model/src/federation.rs",
            "pub fn branch(",
        ),
        (
            "model arbitration",
            "qip-world-model",
            "services/qip-world-model/src/federation.rs",
            "pub fn arbitrate(",
        ),
        (
            "the reasoner",
            "qip-reasoning-engine",
            "services/qip-reasoning-engine/src/engine.rs",
            "pub struct ReasoningEngine {",
        ),
        (
            "the generalist planner",
            "qip-agency",
            "services/qip-agency/src/plan.rs",
            "pub struct InterventionPlan {",
        ),
        (
            "the specialist agent contract",
            "qip-agents",
            "libs/qip-agents/src/manifest.rs",
            "pub struct AgentManifest {",
        ),
        (
            "the specialist agents",
            "qip-investment-agents",
            "agents/qip-investment-agents/src/chief.rs",
            "pub struct Organisation {",
        ),
    ];
    for (component, crate_name, file, marker) in components {
        let source = qip_acceptance::read(&format!("backend/crates/{file}"));
        assert!(
            source.contains(marker),
            "{component} is no longer at {file} (`{marker}` not found): find the crate it moved \
             to and place that crate here, or this test is asserting about an empty name"
        );
        let (lane, _) = register
            .get(crate_name)
            .unwrap_or_else(|| panic!("{crate_name} has no row in the lane register"));
        assert_eq!(
            *lane, COGNITIVE_SLOW,
            "{component} ({crate_name}) is registered to lane {lane}; world models, arbitration, \
             reasoners, planners and specialist agents run in Lane 3, the cognitive slow lane"
        );
    }

    // And none can run in a faster lane by being linked into one: every crate
    // whose normal-dependency closure reaches a cognitive crate is itself
    // registered at Lane 3 or slower.
    let cognitive: BTreeSet<&str> = components.iter().map(|c| c.1).collect();
    let mut hosts = Vec::new();
    for (name, (lane, _)) in &register {
        let reached: Vec<String> = closure(name, &graph)
            .into_iter()
            .filter(|linked| linked != name && cognitive.contains(linked.as_str()))
            .collect();
        if reached.is_empty() {
            continue;
        }
        hosts.push(name.clone());
        assert!(
            *lane >= COGNITIVE_SLOW,
            "{name} is registered to lane {lane} and links {reached:?}: a world model, reasoner \
             or specialist agent would run in a lane faster than the cognitive slow lane"
        );
    }
    // Premise: the walk saw the hosts. The kernel composes all of them and
    // three central binaries compose the kernel; a walk that found fewer is
    // not reading the graph, and the loop above proved nothing.
    for host in ["qip-kernel", "qip-deepbrain", "qip-fastbrain", "qip-api"] {
        assert!(
            hosts.iter().any(|found| found == host),
            "{host} does not appear to link a cognitive crate; hosts found: {hosts:?}"
        );
    }

    // The reflex node, by name: none of them is in what it links.
    let reflex = closure("qip-edge-node", &graph);
    assert!(reflex.contains("qip-edge"), "closure is {reflex:?}");
    for crate_name in &cognitive {
        assert!(
            !reflex.contains(*crate_name),
            "qip-edge-node links {crate_name}: cognition is on the reflex path"
        );
    }
}
// --- below the crate: what the Lane 0 pass actually reaches ------------------

/// Every body of a function named `name` in `sources`, brace-matched.
/// Several types may share a method name; all of their bodies are returned,
/// which over-approximates what a call reaches and so errs towards a finding.
fn bodies_of<'a>(sources: &'a [String], name: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    for text in sources {
        for opening in [format!("fn {name}("), format!("fn {name}<")] {
            let mut from = 0;
            while let Some(offset) = text[from..].find(&opening) {
                let start = from + offset;
                from = start + opening.len();
                // Not a longer name ending in this one: `fn re_flush(` is not
                // `fn flush(`.
                let preceded = text[..start].chars().next_back();
                if preceded.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let Some(brace) = text[start..].find(['{', ';']) else {
                    continue;
                };
                if text[start + brace..].starts_with(';') {
                    continue; // a trait declaration with no body
                }
                let mut depth = 0usize;
                let mut end = start + brace;
                for (index, c) in text[start + brace..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = start + brace + index + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                found.push(&text[start..end]);
            }
        }
    }
    found
}

/// The names of the `self.<name>(` calls in `body`.
fn self_calls(body: &str) -> BTreeSet<String> {
    let mut calls = BTreeSet::new();
    let mut rest = body;
    while let Some(at) = rest.find("self.") {
        rest = &rest[at + "self.".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && rest[name.len()..].starts_with('(') {
            calls.insert(name);
        }
    }
    calls
}

/// ARCH-013 below the crate. The four tests above place crates in lanes; a
/// Lane 0 crate can still hold a slow function, and nothing at crate level
/// sees it. `qip-edge` is Lane 0 *and* holds the mesh client and the journal
/// flush, both of which leave the process. This test follows `Cell::work`,
/// the reflex pass, through every `self.` method it reaches inside
/// `qip-edge/src`, and asserts none of those bodies is the journal flush or
/// names a socket, a sleep, a file or the mesh publisher.
///
/// What it cannot see: calls through a field (`self.journal.record(..)`) or a
/// free function are not followed, so it is a floor, not a proof. It does
/// catch the change most likely to happen — the pass calling a cell method
/// that does I/O.
///
/// Mutation: add `let _ = self.flush(mirror, now);` (any call to `Cell::flush`,
/// with a `&mut dyn Mirror` threaded in) inside
/// `Cell::work` — `flush` joins the reached set and the assertion fires.
/// Mutation: add `std::thread::sleep(std::time::Duration::from_millis(1));`
/// inside any method `work` reaches (e.g. `review_venues`) — the token
/// assertion fires.
#[test]
fn the_reflex_pass_reaches_no_journal_flush_socket_sleep_or_file_through_the_cells_own_methods() {
    let sources: Vec<String> =
        qip_acceptance::files_with_extension("backend/crates/edge/qip-edge/src", "rs")
            .iter()
            .map(|path| {
                qip_acceptance::production_text(
                    &std::fs::read_to_string(path).expect("a readable source file"),
                )
            })
            .collect();
    // Premise: the pass is where this test thinks it is.
    let work = bodies_of(&sources, "work");
    assert!(
        work.iter()
            .any(|body| body.contains("gateway: &mut dyn Placer")),
        "premise: Cell::work was found"
    );

    let mut reached = BTreeSet::from(["work".to_string()]);
    let mut pending = vec!["work".to_string()];
    while let Some(name) = pending.pop() {
        for body in bodies_of(&sources, &name) {
            for call in self_calls(body) {
                if reached.insert(call.clone()) {
                    pending.push(call);
                }
            }
        }
    }
    // Premise: the walk is a walk, not the one body it started from.
    assert!(
        reached.len() > 20,
        "only {} methods reached: {reached:?}",
        reached.len()
    );
    assert!(
        reached.contains("refuse"),
        "premise: the walk follows the pass's own calls"
    );

    assert!(
        !reached.contains("flush"),
        "the reflex pass reaches the journal flush, which ships off-process"
    );
    let forbidden = [
        "TcpStream",
        "UdpSocket",
        "thread::sleep",
        "MeshPublisher",
        "std::fs::",
        "File::",
        "HttpClient",
    ];
    for name in &reached {
        for body in bodies_of(&sources, name) {
            for token in forbidden {
                assert!(
                    !body.contains(token),
                    "`{name}`, reached from the reflex pass, names `{token}`"
                );
            }
        }
    }
}
