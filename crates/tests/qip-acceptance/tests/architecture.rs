//! The architectural boundaries, read from the dependency graph.
//!
//! Every boundary in this platform is meant to be enforced by the compiler
//! rather than by convention: a crate cannot reach a facility it does not
//! depend on, whatever its author intended. That only holds while the
//! dependency edges say what everyone thinks they say, and a `Cargo.toml` is
//! the easiest file in a repository to add one line to.
//!
//! These tests read the manifests and assert the edges directly, so adding the
//! line fails here rather than three months later during an incident.
//!
//! They are deliberately about *absent* edges. A present one is visible in the
//! code that uses it; an absent one is invisible until someone adds it.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::repository_root;
use std::collections::{BTreeMap, BTreeSet};

/// The in-tree dependencies each crate declares, by crate name.
///
/// Parsed rather than taken from `cargo metadata`, because the point is to
/// check the file a reviewer reads.
///
/// Development dependencies are excluded. They are what a crate's own tests
/// link against, not what its shipped code can call, and Cargo deliberately
/// permits cycles among them — `qip-reasoning-engine` and
/// `qip-investment-agents` each test against the other. Counting them would
/// mean reporting a boundary violation for a dependency no deployed binary
/// has.
fn dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let mut graph = BTreeMap::new();
    let mut stack = vec![repository_root().join("crates")];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == "Cargo.toml") {
                let (name, dependencies) = parse_manifest(&path);
                graph.insert(name, dependencies);
            }
        }
    }
    assert!(
        graph.len() > 25,
        "only {} manifests were found; the walk is not reaching the crates",
        graph.len()
    );
    graph
}

/// The crate's name and every `qip-*` dependency its shipped code can call.
fn parse_manifest(path: &std::path::Path) -> (String, BTreeSet<String>) {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut name = String::new();
    let mut dependencies = BTreeSet::new();
    let mut section = String::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        if section == "[package]"
            && let Some(value) = line.strip_prefix("name")
            && let Some(quoted) = value.split('"').nth(1)
        {
            name = quoted.to_string();
        }
        if section == "[dependencies]"
            && let Some(crate_name) = line.split_once('.').map(|(left, _)| left.trim())
            && crate_name.starts_with("qip-")
        {
            dependencies.insert(crate_name.to_string());
        }
    }
    assert!(
        !name.is_empty(),
        "{} declares no package name",
        path.display()
    );
    (name, dependencies)
}

/// Everything `root` can reach, transitively.
fn reachable_from(graph: &BTreeMap<String, BTreeSet<String>>, root: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root.to_string()];
    while let Some(current) = stack.pop() {
        for dependency in graph.get(&current).into_iter().flatten() {
            if seen.insert(dependency.clone()) {
                stack.push(dependency.clone());
            }
        }
    }
    seen
}

// --- the boundary that matters most -----------------------------------------

/// The crates that decide what the platform does with money.
///
/// A wrong answer here is a wrong trade, so nothing in them may be produced by
/// asking a model.
const SAFETY_CRITICAL: [&str; 5] = [
    "crates/services/qip-execution-engine",
    "crates/services/qip-risk-engine",
    "crates/services/qip-portfolio-engine",
    "crates/services/qip-optimization-engine",
    "crates/services/qip-simulation-engine",
];

#[test]
fn no_safety_critical_engine_can_reach_a_language_model() {
    // The rule from the brief: isolate language-model behaviour from
    // safety-critical execution. Enforced by absence — these crates have no
    // edge to `qip-ai` at all, so no amount of well-meaning code inside them
    // can ask a model what to do.
    //
    // Transitively, because a boundary one intermediate crate can defeat is
    // not a boundary.
    let graph = dependency_graph();
    for path in SAFETY_CRITICAL {
        let engine = path.rsplit('/').next().expect("a crate name");
        let reachable = reachable_from(&graph, engine);
        assert!(
            !reachable.contains("qip-ai"),
            "{engine} can reach a language model: {reachable:?}"
        );
    }
}

#[test]
fn nothing_that_decides_or_executes_names_the_language_model_interface() {
    // The manifest edge above is the coarse check, and it is not enough on its
    // own: `qip-ai` holds the deterministic retrieval machinery — a hashing
    // embedder and a BM25 index — as well as the model interface, and the
    // world model legitimately uses the former. A crate reaching `qip-ai` is
    // therefore not proof it reaches a *model*.
    //
    // So this asserts the precise thing: no crate that sizes, checks or places
    // a trade so much as names the language-model interface. The two checks
    // are kept separate because they fail for different reasons and a reader
    // needs to know which.
    //
    // The agent crate is deliberately not in this set. Agents do hold a model,
    // because writing a thesis is what a model is for — and the next test is
    // what keeps that from mattering.
    let mut offenders = Vec::new();
    for path in SAFETY_CRITICAL {
        for file in qip_acceptance::files_with_extension(&format!("{path}/src"), "rs") {
            let content = std::fs::read_to_string(&file).expect("readable source");
            for interface in ["qip_ai::language", "LanguageModel"] {
                if content.contains(interface) {
                    offenders.push(format!("{}: {interface}", file.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a crate that decides or executes names the language-model interface: {offenders:?}"
    );
}

#[test]
fn an_agent_that_holds_a_language_model_cannot_touch_the_market() {
    // Where the agents are concerned the boundary is finer than a crate edge:
    // consulting a model is a declared capability, and the manifest is what
    // says who has it. The property that matters is that the two grants are
    // disjoint — nothing that can ask a model what to do can also place,
    // cancel or override.
    use qip_agents::Capability;

    let roster =
        qip_investment_agents::manifests::roster(qip_core::Timestamp::from_secs(1_760_000_000));
    let mut holders = 0;
    for manifest in roster.iter() {
        if !manifest
            .capabilities
            .contains(Capability::CallLanguageModel)
        {
            continue;
        }
        holders += 1;
        let market: Vec<&str> = Capability::all()
            .into_iter()
            .filter(|capability| {
                capability.touches_market() && manifest.capabilities.contains(*capability)
            })
            .map(|capability| capability.as_str())
            .collect();
        assert!(
            market.is_empty(),
            "{} may both consult a model and {market:?}",
            manifest.id
        );
    }
    assert!(
        holders > 0,
        "no agent holds CallLanguageModel, so this test proves nothing"
    );
}

#[test]
fn the_execution_engine_is_reachable_from_almost_nothing() {
    // An agent crate that could reach the order manager could submit an order
    // and the capability system would never see it. Only the composition root,
    // the deployables it assembles and the workspace tests may depend on it.
    let graph = dependency_graph();
    let permitted: BTreeSet<&str> = [
        "qip-kernel",
        "qip-api",
        "qip-cli",
        "qip-web",
        "qip-fastbrain",
        "qip-deepbrain",
        // The edge cell is the second composition root: it owns the hot
        // order path, which is exactly what the execution engine is.
        "qip-edge",
        "qip-edge-node",
        "qip-acceptance",
    ]
    .into_iter()
    .collect();

    for (crate_name, dependencies) in &graph {
        if permitted.contains(crate_name.as_str()) {
            continue;
        }
        assert!(
            !dependencies.contains("qip-execution-engine"),
            "{crate_name} depends on the execution engine directly"
        );
    }
}

#[test]
fn the_agent_organisation_cannot_reach_execution_at_all() {
    // Stronger than the rule above and stated separately, because this is the
    // one an enthusiastic change is most likely to break: the eighteen agents
    // reach the desk, and the desk is read-only.
    let graph = dependency_graph();
    let reachable = reachable_from(&graph, "qip-investment-agents");
    assert!(
        !reachable.contains("qip-execution-engine"),
        "an investment agent can reach the order manager: {reachable:?}"
    );
}

// --- layering ---------------------------------------------------------------

#[test]
fn the_foundation_crate_depends_on_nothing_in_the_workspace() {
    // If `qip-core` could depend on anything else, every crate could depend on
    // everything through it and the layering would be decorative.
    let graph = dependency_graph();
    let core = graph.get("qip-core").expect("qip-core exists");
    assert!(
        core.is_empty(),
        "qip-core has grown in-tree dependencies: {core:?}"
    );
}

#[test]
fn a_library_never_depends_on_a_service_or_an_application() {
    // Layers only hold in one direction. A library that reaches up into a
    // service makes the service's assumptions everyone's assumptions.
    let graph = dependency_graph();
    let libraries = crates_under("crates/libs");
    let upper: BTreeSet<String> = crates_under("crates/services")
        .into_iter()
        .chain(crates_under("crates/apps"))
        .chain(crates_under("crates/runtime"))
        .chain(crates_under("crates/agents"))
        .collect();

    for library in &libraries {
        for dependency in graph.get(library).into_iter().flatten() {
            assert!(
                !upper.contains(dependency),
                "the library {library} depends on {dependency}, which is above it"
            );
        }
    }
}

#[test]
fn only_the_composition_root_assembles_the_platform() {
    // Exactly one crate knows how the pieces fit together. Two would mean two
    // different platforms with the same name, differing in whichever control
    // one of them forgot.
    let graph = dependency_graph();
    let assemblers: Vec<&String> = graph
        .iter()
        .filter(|(name, dependencies)| {
            name.as_str() != "qip-kernel"
                && dependencies.contains("qip-risk-engine")
                && dependencies.contains("qip-execution-engine")
                && dependencies.contains("qip-portfolio-engine")
        })
        .map(|(name, _)| name)
        .collect();
    assert!(
        assemblers
            .iter()
            .all(|name| name.as_str() == "qip-acceptance"),
        "these crates assemble their own platform instead of using the kernel: {assemblers:?}"
    );
}

#[test]
fn the_web_application_renders_and_nothing_else() {
    // The surface a browser talks to holds no engine at all. It renders domain
    // types; the API is what depends on it and hands it the data. Stated as
    // "reaches no engine" rather than "goes through the kernel", because the
    // edge runs API → web, and a rendering layer that cannot reach an engine
    // is a stronger guarantee than one that reaches it through a chaperone.
    let graph = dependency_graph();
    let reachable = reachable_from(&graph, "qip-web");
    for forbidden in [
        "qip-execution-engine",
        "qip-optimization-engine",
        "qip-risk-engine",
        "qip-kernel",
        "qip-ai",
    ] {
        assert!(
            !reachable.contains(forbidden),
            "the web application can reach {forbidden}: {reachable:?}"
        );
    }
    assert!(
        graph
            .get("qip-api")
            .is_some_and(|api| api.contains("qip-web")),
        "nothing serves the rendered pages"
    );
}

#[test]
fn the_dependency_graph_is_acyclic() {
    // Cargo refuses a cycle among shipped dependencies, so this cannot fail
    // today. It is here because the traversals above are only safe on an
    // acyclic graph, and because it pins the reason dev-dependencies are
    // excluded: among those, Cargo permits cycles and this repository has one.
    let graph = dependency_graph();
    for crate_name in graph.keys() {
        let reachable = reachable_from(&graph, crate_name);
        assert!(
            !reachable.contains(crate_name),
            "{crate_name} is reachable from itself"
        );
    }
}

// --- external surface -------------------------------------------------------

#[test]
fn no_crate_declares_a_third_party_dependency_beyond_the_two_permitted() {
    // The lockfile check in `scripts/check-dependencies.sh` catches what was
    // resolved. This catches what was *asked for*, which is the line a
    // reviewer sees in the diff.
    let permitted: BTreeSet<&str> = ["serde", "serde_json"].into_iter().collect();
    let mut offenders = Vec::new();

    let mut stack = vec![repository_root().join("crates")];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().is_none_or(|name| name != "Cargo.toml") {
                continue;
            }
            let content = std::fs::read_to_string(&path).expect("readable manifest");
            let mut section = String::new();
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    section = line.to_string();
                    continue;
                }
                if !section.contains("dependencies") || line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some(name) = line.split(['.', ' ', '=']).next() else {
                    continue;
                };
                if name.is_empty() || name.starts_with("qip-") || permitted.contains(name) {
                    continue;
                }
                offenders.push(format!("{}: {name}", path.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "third-party dependencies were declared: {offenders:?}"
    );
}

/// Crate names declared under a workspace directory.
fn crates_under(relative: &str) -> BTreeSet<String> {
    let root = repository_root().join(relative);
    let mut names = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        panic!("cannot read {}", root.display());
    };
    for entry in entries.flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if manifest.is_file() {
            names.insert(parse_manifest(&manifest).0);
        }
    }
    assert!(!names.is_empty(), "no crates found under {relative}");
    names
}
