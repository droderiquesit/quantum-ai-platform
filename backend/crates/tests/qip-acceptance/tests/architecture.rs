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
    let mut stack = vec![repository_root().join("backend/crates")];
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
///
/// The four edge entries are here for the second of the two checks below
/// rather than the first. `no_edge_cell_can_reach_a_language_model` already
/// makes the manifest-transitive claim for every crate under `crates/edge`,
/// whether it appears in this list or not, so their presence in the coarse
/// check is free duplication and kept only because one list is easier to
/// maintain than two. The source-level check is what their presence buys: a
/// manifest edge is one line, and the grep is what makes the first line of
/// *code* that reaches for a model fail in the same commit, next to what a
/// reviewer is reading.
///
/// The other four edge crates are deliberately absent. Protocol decoders,
/// sequencers, books and feature DAGs transform; they do not decide. Listing
/// them would make this "the edge" rather than "the crates whose output is an
/// order", and a list that means everything constrains nothing.
const SAFETY_CRITICAL: [&str; 9] = [
    "backend/crates/services/qip-execution-engine",
    "backend/crates/services/qip-risk-engine",
    "backend/crates/services/qip-portfolio-engine",
    "backend/crates/services/qip-optimization-engine",
    "backend/crates/services/qip-simulation-engine",
    "backend/crates/edge/qip-strategy",
    "backend/crates/edge/qip-arbitrage",
    "backend/crates/edge/qip-routing",
    "backend/crates/edge/qip-edge",
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
        // The venue adapters implement the execution engine's `Broker` port
        // and speak its `Order`/`Fill` vocabulary. That is the port-adapter
        // direction — the far side of an order path necessarily knows what an
        // order is — and refusing it would mean a second, parallel order
        // vocabulary that has to be kept in agreement with this one by hand.
        //
        // What that concession costs is checked by
        // `nothing_outside_a_composition_root_holds_an_order_manager` below:
        // reaching the crate is permitted, holding the order manager is not.
        "qip-brokers",
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
fn nothing_outside_a_composition_root_holds_an_order_manager() {
    // The manifest rule above says which crates may *reach* the execution
    // engine. This one says what they may do with it, and it is the property
    // that actually matters: an `OrderManager` is the thing that submits, and
    // a crate that constructs one has an order path the capability system
    // never reviewed.
    //
    // Checked in the source rather than in the graph, because the concession
    // made to `qip-brokers` above is exactly the shape of change that would
    // otherwise let an order manager in unnoticed.
    let permitted = [
        "backend/crates/runtime/qip-kernel",
        "backend/crates/edge/qip-edge",
        "backend/crates/apps",
        "backend/crates/tests",
        // Where it is defined.
        "backend/crates/services/qip-execution-engine",
    ];

    let mut offenders = Vec::new();
    for path in qip_acceptance::files_with_extension("backend/crates", "rs") {
        let display = path.display().to_string();
        let relative = display
            .rsplit_once("backend/crates/")
            .map_or(display.clone(), |(_, tail)| {
                format!("backend/crates/{tail}")
            });
        if permitted.iter().any(|prefix| relative.starts_with(prefix)) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        if source.contains("OrderManager") {
            offenders.push(relative);
        }
    }
    assert!(
        offenders.is_empty(),
        "these hold an order manager without being a composition root: {offenders:?}"
    );

    // The vacuity guard: the name must actually exist, or this test passes by
    // checking for something that was renamed.
    assert!(
        qip_acceptance::read("backend/crates/services/qip-execution-engine/src/oms.rs")
            .contains("pub struct OrderManager"),
        "the order manager has been renamed and this test constrains nothing"
    );
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

// --- the edge cells ---------------------------------------------------------

/// Every crate that is part of an edge cell, including the deployable.
///
/// `qip-edge-node` lives under `crates/apps` rather than `crates/edge`, so a
/// rule written against the directory would miss the one artifact that
/// actually runs next to a venue. It is named here so it cannot.
fn edge_crates() -> BTreeSet<String> {
    let mut names = crates_under("backend/crates/edge");
    assert!(
        names.len() >= 8,
        "only {} crates were found under crates/edge",
        names.len()
    );
    names.insert("qip-edge-node".to_string());
    names
}

#[test]
fn no_edge_cell_can_reach_a_language_model() {
    // ADR 0008's third consequence, made total: nothing on the hot path can
    // consult a model. The ADR states it of `qip-strategy`; a rule that held
    // for one crate and not its neighbours would be defeated by putting the
    // call one crate over, so it is asserted here of every crate in a cell.
    //
    // Transitively, because the route back in is not a direct edge to
    // `qip-ai` — nobody would write that — but a plausible-looking edge to
    // `qip-compliance`, which extends the model registry and therefore drags
    // the whole language-model surface behind it. `qip-edge`'s manifest was
    // stripped of exactly that dependency, and this test is what keeps it
    // stripped once the reason has been forgotten.
    let graph = dependency_graph();
    for crate_name in edge_crates() {
        let reachable = reachable_from(&graph, &crate_name);
        assert!(
            !reachable.contains("qip-ai"),
            "the edge crate {crate_name} can reach a language model: {reachable:?}"
        );
    }
}

#[test]
fn only_the_edge_cell_itself_holds_an_order_manager() {
    // A cell is a composition root, and like the kernel it is the one place
    // the pieces are allowed to meet. A protocol decoder, an order book, a
    // feature DAG, a strategy, the arbitrage graph and the router all have
    // legitimate work to do and none of it involves submitting an order:
    // they hand their output to the cell, which is the thing that was
    // reviewed as being allowed to act on it.
    //
    // Stated as reachability rather than as a direct edge, because an order
    // manager acquired through a helper crate is still an order manager.
    let graph = dependency_graph();
    let mut roots = 0;
    for crate_name in edge_crates() {
        let reachable = reachable_from(&graph, &crate_name);
        let is_root = matches!(crate_name.as_str(), "qip-edge" | "qip-edge-node");
        if is_root {
            roots += 1;
            continue;
        }
        assert!(
            !reachable.contains("qip-execution-engine"),
            "{crate_name} can reach the order manager without being a cell: {reachable:?}"
        );
    }
    // The vacuity guard. If the cell itself stopped reaching execution the
    // loop above would pass while proving nothing, and the hot path would
    // have quietly moved somewhere this file does not look.
    assert_eq!(roots, 2, "the edge composition roots have been renamed");
    assert!(
        reachable_from(&graph, "qip-edge").contains("qip-execution-engine"),
        "the edge cell no longer reaches execution, so this test constrains nothing"
    );
}

#[test]
fn no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy() {
    // The core safety argument of ADR 0008. A cell is safe while disconnected
    // precisely because its authority is a `CapitalEnvelope` somebody else
    // signed and bounded: the worst a partitioned cell can do is spend an
    // amount already approved, for as long as the envelope has left to run.
    //
    // That holds only while issuance stays central. A cell that could reach
    // `qip-capital` could widen its own bound, and one that could reach
    // `qip-lifecycle` could promote a strategy past the gates that exist to
    // decide whether it may trade at all. Cells receive envelopes and
    // promotions; they never mint them.
    let graph = dependency_graph();
    for crate_name in edge_crates() {
        let reachable = reachable_from(&graph, &crate_name);
        for issuer in ["qip-lifecycle", "qip-capital"] {
            assert!(
                !reachable.contains(issuer),
                "the edge crate {crate_name} can reach {issuer}, so a cell could grant itself \
                 what the central plane is supposed to grant it: {reachable:?}"
            );
        }
    }
}

// --- the solvers ------------------------------------------------------------

/// Every crate that holds a veto, places an order, or issues capital.
///
/// Named rather than derived from a directory, because the property is about
/// authority and not about layout: `qip-lifecycle` decides whether a strategy
/// may trade at all and `qip-capital` decides how much it may spend, and
/// neither of them lives beside the risk gate.
const NO_SOLVER_AUTHORITY: &[&str] = &[
    "qip-risk-engine",
    "qip-execution-engine",
    "qip-compliance",
    "qip-brokers",
    "qip-capital",
    "qip-lifecycle",
];

#[test]
fn nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver() {
    // The companion to `no_safety_critical_engine_can_reach_a_language_model`,
    // for the other kind of model. A solver's output is policy — a budget, a
    // whitelist, a target — and policy is an input to a decision somebody else
    // makes deterministically. The moment a risk gate, an order manager or the
    // capital issuer can call a solver directly, an optimiser's answer becomes
    // the answer, and the thing that was supposed to constrain it is the thing
    // asking it.
    //
    // Enforced by absence and transitively, for the same reason the
    // language-model rule is: a boundary one intermediate crate can defeat is
    // not a boundary. The route back in here would not be a direct edge to
    // `qip-quantum`, which a reviewer would question, but an edge to
    // `qip-optimization-engine`, which looks entirely reasonable on a risk
    // crate until you notice what it drags behind it.
    let graph = dependency_graph();
    for crate_name in NO_SOLVER_AUTHORITY {
        assert!(
            graph.contains_key(*crate_name),
            "{crate_name} is not a crate in this workspace; this test is naming \
             something that no longer exists and constrains nothing"
        );
        let reachable = reachable_from(&graph, crate_name);
        assert!(
            !reachable.contains("qip-quantum"),
            "{crate_name} holds a veto or moves money and can reach a quantum \
             solver: {reachable:?}"
        );
    }
    // The vacuity guard. Every assertion above is about an absent edge, so if
    // the solver crate were renamed or deleted the loop would pass while
    // proving nothing. The optimiser is the one place the edge is supposed to
    // exist, and its presence is what makes the absences above meaningful.
    assert!(
        reachable_from(&graph, "qip-optimization-engine").contains("qip-quantum"),
        "the optimisation engine no longer reaches a quantum solver, so the \
         absences this test asserts elsewhere prove nothing"
    );
}

#[test]
fn no_edge_cell_can_reach_a_quantum_solver() {
    // Stated separately from the rule above because it fails for a different
    // reason, and a reader needs to know which.
    //
    // A region receives a solved answer; it never solves. That is not a
    // performance argument — although a QPU round trip inside a microsecond
    // budget is its own absurdity — it is ADR 0008's argument. A cell is safe
    // while partitioned precisely because everything it is allowed to do was
    // decided in advance and shipped to it. A cell that could solve could
    // reach a conclusion the centre never approved, at exactly the moment
    // nobody can see it.
    let graph = dependency_graph();
    for crate_name in edge_crates() {
        let reachable = reachable_from(&graph, &crate_name);
        assert!(
            !reachable.contains("qip-quantum"),
            "the edge crate {crate_name} can reach a quantum solver: {reachable:?}"
        );
    }
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
fn the_contract_layer_sits_at_the_bottom_of_everything_that_shares_it() {
    // Fifteen crates across the edge stack and the central plane are written
    // against `qip-contracts` rather than against each other, and that is the
    // whole reason the graph is a fan-out from one vocabulary rather than a
    // mesh: an order book does not know what a strategy is, and neither knows
    // how capital is allocated. It only works while the vocabulary itself is
    // at the bottom. A contract crate that reached a service would put that
    // service behind every crate that speaks the language.
    //
    // "At the bottom" is not "depends on nothing" — that is `qip-core`'s job
    // and the test above. The contract layer legitimately borrows the exact
    // types the vocabulary is made of, so what is pinned here is the precise
    // set it declares today. Pinning the exact set rather than a forbidden
    // list is the point: it makes any growth of the contract layer a decision
    // somebody takes deliberately rather than one that arrives with a commit
    // about something else.
    let graph = dependency_graph();
    let declared = graph.get("qip-contracts").expect("qip-contracts exists");
    let expected: BTreeSet<String> = [
        "qip-core",
        "qip-financial",
        "qip-market",
        "qip-numerics",
        "qip-portfolio",
        "qip-risk",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(
        *declared, expected,
        "the contract layer's dependencies have changed, so every crate that speaks the \
         vocabulary now reaches whatever was added"
    );

    // Transitively it stays inside the library layer: no service, no
    // application, no cell, and above all no model — a shared vocabulary that
    // dragged the language-model surface behind it would defeat every
    // no-model rule in this file at once.
    let reachable = reachable_from(&graph, "qip-contracts");
    let above: BTreeSet<String> = crates_under("backend/crates/services")
        .into_iter()
        .chain(crates_under("backend/crates/apps"))
        .chain(crates_under("backend/crates/edge"))
        .chain(crates_under("backend/crates/runtime"))
        .chain(crates_under("backend/crates/agents"))
        .chain(["qip-ai".to_string(), "qip-compliance".to_string()])
        .collect();
    for forbidden in &above {
        assert!(
            !reachable.contains(forbidden),
            "the contract layer reaches {forbidden}, which is above it: {reachable:?}"
        );
    }
}

#[test]
fn a_library_never_depends_on_a_service_or_an_application() {
    // Layers only hold in one direction. A library that reaches up into a
    // service makes the service's assumptions everyone's assumptions.
    let graph = dependency_graph();
    let libraries = crates_under("backend/crates/libs");
    let upper: BTreeSet<String> = crates_under("backend/crates/services")
        .into_iter()
        .chain(crates_under("backend/crates/apps"))
        .chain(crates_under("backend/crates/runtime"))
        .chain(crates_under("backend/crates/agents"))
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

    let mut stack = vec![repository_root().join("backend/crates")];
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

#[test]
fn the_decision_core_named_by_adr_0009_is_the_set_actually_held_to_two() {
    // ADR 0009 tiers the dependency policy: a named decision core keeps serde
    // and serde_json, and an I/O edge may take from an allowlist so that the
    // managed-service clients can exist at all. The ADR says that boundary "is
    // enforced in crates/tests/qip-acceptance/tests/architecture.rs".
    //
    // It was not. The test above holds *every* crate to two, with no tier —
    // stricter than the ADR, which sounds harmless and is not: the document
    // claimed an enforcement that did not exist, and the day the first Pub/Sub
    // client lands, whoever adds it will relax the strict check and discover
    // that nothing then holds the core to anything.
    //
    // So the list is read out of the ADR rather than copied into this file.
    // A crate added to the decision core in prose is a crate this test starts
    // checking; a crate quietly dropped from the prose to let a dependency in
    // is a diff a reviewer sees.
    let adr = qip_acceptance::read("docs/adr/0009-tiered-dependency-policy.md");
    let core: BTreeSet<String> = adr
        .split("```")
        .nth(1)
        .expect("the ADR names the decision core in a fenced block")
        .split_whitespace()
        .filter(|token| token.starts_with("qip-"))
        .map(str::to_string)
        .collect();
    assert!(
        core.len() >= 15,
        "the ADR's decision core has shrunk to {} crate(s), which is the shape \
         of a change that widens the dependency policy without saying so",
        core.len()
    );

    let graph = dependency_graph();
    let mut missing = Vec::new();
    for crate_name in &core {
        if !graph.contains_key(crate_name) {
            missing.push(crate_name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "the ADR names crates that are not in the workspace: {missing:?}"
    );

    // And each of them declares nothing beyond the two. Read from the manifest
    // rather than from the lockfile: what a crate *asked for* is the line in
    // the diff, and a transitive arrival is the shell script's job.
    let permitted: BTreeSet<&str> = ["serde", "serde_json"].into_iter().collect();
    let mut offenders = Vec::new();
    for path in qip_acceptance::files_with_extension("backend/crates", "toml") {
        if path.file_name().is_none_or(|name| name != "Cargo.toml") {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("readable manifest");
        let Some(name) = content
            .lines()
            .find_map(|line| line.trim().strip_prefix("name = "))
            .map(|value| value.trim_matches('"').to_string())
        else {
            continue;
        };
        if !core.contains(&name) {
            continue;
        }
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
            let Some(dependency) = line.split(['.', ' ', '=']).next() else {
                continue;
            };
            if dependency.is_empty()
                || dependency.starts_with("qip-")
                || permitted.contains(dependency)
            {
                continue;
            }
            offenders.push(format!("{name}: {dependency}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "these are in ADR 0009's decision core and declare more than serde and \
         serde_json: {offenders:?}"
    );
}

// --- the workspace itself ---------------------------------------------------

#[test]
fn every_crate_on_disk_is_a_member_of_the_workspace() {
    // A directory holding a manifest that the workspace does not list is
    // compiled by nobody, linted by nobody and tested by nobody, while
    // looking in a file listing exactly like a crate that is. Every other
    // test in this repository — including all of the ones above — is silent
    // about it, because they read manifests rather than build outputs.
    //
    // Read as text rather than through `cargo metadata`, for the same reason
    // as the graph above: the members list is the file a reviewer reads, and
    // a crate is left out by editing that file and nothing else.
    let manifest = qip_acceptance::read("backend/Cargo.toml");
    let members: BTreeSet<&str> = manifest
        .split("members = [")
        .nth(1)
        .expect("the workspace declares its members")
        .lines()
        .take_while(|line| !line.trim().starts_with(']'))
        .filter_map(|line| line.split('"').nth(1))
        .collect();

    let on_disk = crate_directories();
    for directory in &on_disk {
        assert!(
            members.contains(directory.as_str()),
            "{directory} holds a manifest but is not a workspace member, so nothing builds it"
        );
    }

    // The vacuity guard, aimed at the newest crates because they are the ones
    // a members list is most likely to be missing: a walk that found nothing
    // would satisfy the loop above without checking anything.
    for expected in [
        "crates/edge/qip-protocols",
        "crates/edge/qip-sequencing",
        "crates/edge/qip-orderbook",
        "crates/edge/qip-feature-dag",
        "crates/edge/qip-strategy",
        "crates/edge/qip-arbitrage",
        "crates/edge/qip-routing",
        "crates/edge/qip-edge",
        "crates/apps/qip-edge-node",
        "crates/services/qip-mesh",
        "crates/services/qip-chain",
        "crates/services/qip-prediction",
        "crates/services/qip-lifecycle",
        "crates/services/qip-capital",
    ] {
        assert!(
            on_disk.contains(expected),
            "{expected} was not found on disk, so the check above proved nothing about it"
        );
        assert!(
            members.contains(expected),
            "{expected} exists but is not a workspace member"
        );
    }
}

/// Every directory under `backend/crates/` that holds a manifest, written the
/// way the workspace members list writes it — relative to the workspace root
/// at `backend/`, because that is the path a reviewer compares against the
/// members list.
fn crate_directories() -> BTreeSet<String> {
    let root = repository_root().join("backend");
    let mut found = BTreeSet::new();
    let mut stack = vec![root.join("crates")];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == "Cargo.toml") {
                let relative = directory
                    .strip_prefix(&root)
                    .expect("the manifest is under the workspace root");
                found.insert(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found
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
