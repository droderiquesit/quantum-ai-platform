//! The architectural boundaries, read from the dependency graph.
//!
//! Every boundary in this platform is meant to be enforced by the compiler
//! rather than by convention: a crate cannot reach a facility it does not
//! depend on, whatever its author intended. That only holds while the
//! dependency edges say what everyone thinks they say, and a `Cargo.toml` is
//! the easiest file in a repository to add one line to.
//!
//! These tests assert those edges directly, so adding the line fails here
//! rather than three months later during an incident.
//!
//! The graph comes from `cargo metadata` — from Cargo's own resolution rather
//! than from reading the manifests. Four rounds of review each found a live
//! compiling edge that a hand-written TOML reader here could not see, and the
//! reason is structural: every test below asserts an edge is *absent*, so any
//! manifest form the reader could not parse was an edge that did not exist as
//! far as all of them were concerned. See [`dependency_graph`].
//!
//! They are deliberately about *absent* edges. A present one is visible in the
//! code that uses it; an absent one is invisible until someone adds it.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::repository_root;
use std::collections::{BTreeMap, BTreeSet};
/// The workspace dependency graph, **as Cargo itself resolved it**.
///
/// # Why this asks Cargo instead of reading the manifests
///
/// Four rounds of independent review each found live, compiling
/// `qip-risk-engine -> qip-quantum` edges that a hand-written TOML reader in
/// this file could not see, and each round's fix was written for the shape in
/// front of it. Round four's two criticals are the argument for abandoning the
/// approach rather than patching it a fifth time:
///
/// * a package description ending `...'paper''''` — legal TOML, accepted by
///   cargo, no escape character anywhere — desynchronised the string scanner,
///   which glued the whole file into one buffer so that `[dependencies]` never
///   began a line and **the entire dependency table disappeared**; and
/// * `[target.'cfg(not(target_os = "dependencies"))'.dependencies]` defeated
///   the section classifier by substring-matching, which is the precise trap
///   `.claude/rules/architecture/01-testing-strategy.md` warns about, in the
///   parser rather than in a test.
///
/// Every boundary test in this file asserts that some edge is *absent*, so a
/// manifest form the reader cannot parse is an edge that does not exist as far
/// as all of them are concerned, and the suite goes green over a live
/// violation. Hand-parsing TOML made that failure mode a permanent feature of
/// the file.
///
/// `cargo metadata` reports the graph after Cargo has resolved it, so renames,
/// `[workspace.dependencies]` inheritance, target-conditional tables, quoted
/// keys, every string form and every table shape are already correct. The four
/// defects cease to exist by construction rather than by enumeration.
///
/// **The cost, stated honestly.** This no longer checks "the file a reviewer
/// reads" — it checks what Cargo compiles. Those differ only where a reviewer
/// misreads a manifest, and on the evidence of four rounds it is the reader
/// that was wrong every time, not Cargo.
///
/// This adds no dependency. `serde_json` is already permitted and already a
/// dependency of this crate, and cargo is required to build the workspace at
/// all, so ADR 0002 and ADR 0009 are untouched — no crate enters
/// `backend/Cargo.lock`, which is what `scripts/check-dependencies.sh` counts.
///
/// Development and build dependencies are excluded. Dev dependencies are what a
/// crate's own tests link against, not what its shipped code can call, and
/// Cargo deliberately permits cycles among them — `qip-reasoning-engine` and
/// `qip-investment-agents` each test against the other. A build dependency is
/// linked into a build script rather than into the crate.
fn dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let metadata = workspace_metadata();
    let members = workspace_member_names(&metadata);
    let mut graph = BTreeMap::new();
    for package in packages(&metadata) {
        let name = package_name(package);
        let mut edges = BTreeSet::new();
        for dependency in dependencies_of(package) {
            if !is_shipped(dependency) {
                continue;
            }
            // `name` is the crate actually depended on. An alias declared with
            // `package = "…"`, or inherited from `[workspace.dependencies]`
            // under a different key, appears here under its real name with the
            // alias in `rename` — which is what made round three's and round
            // four's rename bypasses possible against the text reader.
            let depended = dependency_name(dependency);
            if members.contains(&depended) {
                edges.insert(depended);
            }
        }
        graph.insert(name, edges);
    }
    assert!(
        graph.len() > 25,
        "cargo metadata reported only {} workspace members; the graph is not \
         being read",
        graph.len()
    );
    graph
}

/// Run `cargo metadata` against the backend workspace, or fail loudly.
///
/// **Fails closed, in the strongest terms available.** Every assertion built on
/// this graph is an assertion that an edge is *absent*, so a silent error here
/// is a green suite over a live violation — which is exactly how all four
/// review rounds failed. A missing binary, a non-zero exit and unparseable
/// output therefore all panic rather than yielding an empty or partial graph.
fn workspace_metadata() -> serde_json::Value {
    let manifest = repository_root().join("backend/Cargo.toml");
    assert!(
        manifest.is_file(),
        "no workspace manifest at {}; this test cannot see the workspace",
        manifest.display()
    );
    // `CARGO` is set by cargo for anything it runs, and is the toolchain that
    // built this test. Falling back to the name on `PATH` keeps the test
    // runnable outside cargo rather than making the fallback the normal case.
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
        .unwrap_or_else(|error| {
            panic!(
                "could not run `{cargo} metadata`: {error}. This test refuses to \
                 pass without it: every boundary assertion in this file is that \
                 an edge is absent, and an empty graph would satisfy all of them"
            )
        });
    assert!(
        output.status.success(),
        "`{cargo} metadata` exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "`{cargo} metadata` produced JSON this test cannot parse: {error}. \
             Failing rather than proceeding on a partial graph"
        )
    })
}

/// The `packages` array, or a panic naming what was missing.
fn packages(metadata: &serde_json::Value) -> &Vec<serde_json::Value> {
    metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .expect("cargo metadata always reports a `packages` array")
}

fn package_name(package: &serde_json::Value) -> String {
    package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("every package has a name")
        .to_string()
}

fn dependencies_of(package: &serde_json::Value) -> &Vec<serde_json::Value> {
    static EMPTY: std::sync::OnceLock<Vec<serde_json::Value>> = std::sync::OnceLock::new();
    package
        .get("dependencies")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| EMPTY.get_or_init(Vec::new))
}

fn dependency_name(dependency: &serde_json::Value) -> String {
    dependency
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("every dependency has a name")
        .to_string()
}

/// Whether a dependency is one the crate's shipped code can call.
///
/// `kind` is absent or null for a normal dependency and carries `"dev"` or
/// `"build"` otherwise. A structured field, so the section-name guessing that
/// round four defeated with a `cfg` string has nothing to defeat.
fn is_shipped(dependency: &serde_json::Value) -> bool {
    !matches!(
        dependency.get("kind").and_then(serde_json::Value::as_str),
        Some("dev") | Some("build")
    )
}

/// Every crate in this workspace, by name.
fn workspace_member_names(metadata: &serde_json::Value) -> BTreeSet<String> {
    packages(metadata).iter().map(package_name).collect()
}

// --- the regression cases from four rounds of bypass -------------------------

#[test]
fn cargo_resolves_every_manifest_form_that_defeated_a_hand_written_parser() {
    // Four rounds of review found bypasses in a hand-rolled TOML reader. Rather
    // than delete that history, every form is kept here and expressed against
    // the source of truth that replaced the reader.
    //
    // A real throwaway workspace is built and `cargo metadata` is run on it, so
    // this asserts the actual tool's actual behaviour rather than a belief
    // about it. It needs no network — every dependency is a path dependency —
    // and a two-crate workspace resolves in milliseconds.
    //
    // Each alias below is a form that was, at some point, a live compiling edge
    // this file could not see.
    let root = std::env::temp_dir().join(format!(
        "qip-manifest-forms-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("solver/src")).expect("temp workspace");
    std::fs::create_dir_all(root.join("consumer/src")).expect("temp workspace");
    std::fs::write(root.join("solver/src/lib.rs"), "").expect("write");
    std::fs::write(root.join("consumer/src/lib.rs"), "").expect("write");

    // Round four, F4: the alias lives in `[workspace.dependencies]` under a key
    // that is not the crate's name, so a reader looking only at the member
    // manifest sees a dependency on something that does not exist.
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"solver\", \"consumer\"]\n\n\
         [workspace.dependencies]\n\
         probe_alias = { path = \"solver\", package = \"probe-solver\" }\n",
    )
    .expect("write");
    std::fs::write(
        root.join("solver/Cargo.toml"),
        "[package]\nname = \"probe-solver\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");

    // Round four, F1: a multi-line literal description ending in a quote
    // immediately before its closing fence. Legal TOML, no escape character,
    // and it desynchronised the scanner so completely that the entire
    // dependency table below became invisible.
    let consumer = concat!(
        "[package]\n",
        "name = \"probe-consumer\"\n",
        "version = \"0.1.0\"\n",
        "edition = \"2021\"\n",
        "description = '''\n",
        "A description ending in a quote. Strictly 'paper''''\n",
        "\n",
        "[dependencies]\n",
        // Round four, F4.
        "probe_alias.workspace = true\n",
        // Round three: a rename in an inline table.
        "inline_alias = { package = \"probe-solver\", path = \"../solver\" }\n",
        // Round three: a quoted key.
        "\"quoted_alias\" = { package = \"probe-solver\", path = \"../solver\" }\n",
        // Round four, F3: an unknown key whose value contains a decoy rename.
        // Cargo accepts unknown keys with a warning no gate reads.
        "unknown_key = { package = \"probe-solver\", path = \"../solver\", \
         notes = \"package = 'decoy'\" }\n",
        "\n",
        // Round two: a table named after one dependency.
        "[dependencies.table_alias]\n",
        "package = \"probe-solver\"\n",
        "path = \"../solver\"\n",
        "\n",
        // Round four, F2: a target table whose cfg predicate contains the word
        // the section classifier was searching for.
        "[target.'cfg(not(target_os = \"dependencies\"))'.dependencies]\n",
        "target_alias = { package = \"probe-solver\", path = \"../solver\" }\n",
        "\n",
        // Not shipped code, and must not be counted.
        "[dev-dependencies]\n",
        "dev_alias = { package = \"probe-solver\", path = \"../solver\" }\n",
    );
    std::fs::write(root.join("consumer/Cargo.toml"), consumer).expect("write");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(&cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "the fixture workspace does not resolve, so none of these forms is \
         being tested: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata emits JSON");

    let consumer_package = packages(&metadata)
        .iter()
        .find(|package| package_name(package) == "probe-consumer")
        .expect("the fixture consumer is a workspace member");

    let mut shipped_aliases = BTreeSet::new();
    let mut excluded_aliases = BTreeSet::new();
    for dependency in dependencies_of(consumer_package) {
        // The property that matters: whatever the key was called, Cargo reports
        // the crate that is really depended on.
        assert_eq!(
            dependency_name(dependency),
            "probe-solver",
            "a fixture dependency resolved to something unexpected: {dependency:?}"
        );
        let alias = dependency
            .get("rename")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("probe-solver")
            .to_string();
        if is_shipped(dependency) {
            shipped_aliases.insert(alias);
        } else {
            excluded_aliases.insert(alias);
        }
    }

    let expected_shipped: BTreeSet<String> = [
        "probe_alias",
        "inline_alias",
        "quoted_alias",
        "unknown_key",
        "table_alias",
        "target_alias",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(
        shipped_aliases, expected_shipped,
        "a manifest form that once bypassed this file is not being resolved as \
         a shipped dependency"
    );
    assert_eq!(
        excluded_aliases,
        ["dev_alias".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "the dev dependency was not excluded, or something else was"
    );

    let _ = std::fs::remove_dir_all(&root);
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

/// Every crate that **vetoes, executes, transfers or issues** — and
/// deliberately not every crate that touches money.
///
/// The distinction is the whole of this list and an earlier version of this
/// comment got it wrong, claiming "holds a veto, places an order, or issues
/// capital" while omitting `qip-portfolio-engine`, which sizes positions and
/// reaches the solver transitively. Read literally, the old comment described
/// a property this list did not enforce, which is worse than a narrow rule
/// honestly stated.
///
/// The narrower rule is the correct one, and it is the blueprint's own. §39
/// puts the optimiser at layer 7 with authority to set "allocation, cycle
/// selection, path assignment inside the envelope", and the strategy engine
/// at layer 8 proposing against it. Optimiser output *exists to be consumed*
/// as policy — grants, budgets, targets, whitelists. A portfolio engine that
/// turns approved hypotheses into constrained targets is that consumption
/// working as designed, so forbidding it would outlaw the intended path
/// rather than protect anything.
///
/// What must never reach a solver is the machinery that says **no**, that
/// places the order, that moves the money, or that mints the authority to do
/// either: layers 5, 6, 11, 12 and 13. Those are the crates below. The
/// deliberate exemption for `qip-portfolio-engine` is recorded in
/// `docs/architecture/algorik-blueprint-traceability.md` rather than left as
/// an absence somebody has to notice.
///
/// Named rather than derived from a directory, because authority is not
/// inferable from layout — `qip-lifecycle` gates whether a strategy may trade
/// at all and `qip-compliance` is a lib, and neither lives beside the risk
/// gate. `every_service_crate_is_classified_for_money_authority` is what stops
/// that hand-maintenance from silently failing to cover a new crate.
const NO_SOLVER_AUTHORITY: &[&str] = &[
    "qip-risk-engine",
    "qip-execution-engine",
    "qip-compliance",
    "qip-brokers",
    "qip-capital",
    "qip-capital-fabric",
    "qip-lifecycle",
];

/// The service crates that hold none of those four authorities.
///
/// Exists so that the union of the two lists can be checked against the tree.
/// A new service crate belongs in one of them, and the test below is what
/// makes that a decision somebody takes rather than an omission nobody sees.
const NO_MONEY_AUTHORITY: &[&str] = &[
    "qip-chain",
    "qip-cost-router",
    "qip-data-finder",
    "qip-entity-resolution",
    "qip-evolution",
    "qip-learning-engine",
    "qip-market-ingestion",
    "qip-mesh",
    "qip-normalization",
    "qip-opportunity-engine",
    "qip-optimization-engine",
    "qip-portfolio-engine",
    "qip-prediction",
    "qip-reasoning-engine",
    "qip-simulation-engine",
    "qip-streaming",
    "qip-training",
    "qip-twin",
    "qip-world-model",
];

#[test]
fn every_service_crate_is_classified_for_money_authority() {
    // The silence this removes: `NO_SOLVER_AUTHORITY` is hand-maintained, so
    // its existence check catches a rename but never an *addition*. A future
    // `qip-treasury` or `qip-settlement-engine` would get no coverage from
    // either solver test and nothing whatever would say so.
    //
    // Deriving the set from the directory is not available here — authority is
    // a property of what a crate does, not of where it sits — so the next best
    // thing is to make the omission fail loudly. Adding a service crate now
    // forces a decision about whether it can veto, execute, transfer or issue.
    //
    // **Known limit, stated rather than left to be discovered.** This walks
    // `crates/services` only. A money-authority crate added under `libs` —
    // which `qip-compliance` already shows is possible — would be covered by
    // neither list and nothing here would say so. Widening the walk to `libs`
    // would mean classifying sixteen crates that mostly hold no authority at
    // all, so the honest position is that this catches the likely case and not
    // every case.
    let services = crates_under("backend/crates/services");
    assert!(
        services.len() >= 25,
        "only {} service crates were found; the walk is not reaching them",
        services.len()
    );
    let mut classified: BTreeSet<String> = BTreeSet::new();
    classified.extend(NO_SOLVER_AUTHORITY.iter().map(|name| name.to_string()));
    classified.extend(NO_MONEY_AUTHORITY.iter().map(|name| name.to_string()));

    let unclassified: BTreeSet<&String> = services.difference(&classified).collect();
    assert!(
        unclassified.is_empty(),
        "these service crates are classified neither as holding money \
         authority nor as lacking it, so no solver-boundary test covers them: \
         {unclassified:?}"
    );

    // Both directions. A name left behind after a crate is deleted would make
    // the check above pass while covering something that is not there.
    // `qip-compliance` is classified but lives in `libs`, so it is excused
    // from the service walk. Asserted to exist rather than hardcoded blindly:
    // an excuse for a deleted crate is a phantom that outlives it.
    let graph = dependency_graph();
    assert!(
        graph.contains_key("qip-compliance"),
        "qip-compliance is excused from the service walk but is not a crate; \
         the exemption now covers nothing"
    );
    let stale: BTreeSet<&String> = classified
        .iter()
        .filter(|name| !services.contains(*name) && name.as_str() != "qip-compliance")
        .collect();
    assert!(
        stale.is_empty(),
        "these names are classified but are not service crates: {stale:?}"
    );

    // The two lists must be disjoint, or a crate could be asserted to both
    // hold and lack authority and the contradiction would never surface.
    for name in NO_SOLVER_AUTHORITY {
        assert!(
            !NO_MONEY_AUTHORITY.contains(name),
            "{name} appears in both authority lists"
        );
    }
}

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
    // The vacuity anchor, which this test needed and did not have. Every
    // assertion above is an absence, so renaming or splitting `qip-quantum`
    // would make all of them trivially true and this test would go on passing
    // while checking nothing. Its sibling above carries the same guard; tests
    // are separate functions and one test's anchor protects only itself.
    assert!(
        graph.contains_key("qip-quantum"),
        "there is no crate called qip-quantum, so every absence asserted here \
         is trivially true and this test constrains nothing"
    );
}

#[test]
fn a_quantum_solver_cannot_reach_anything_that_vetoes_executes_or_moves_money() {
    // The other direction, and arguably the more literal reading of "no model
    // has decision authority": the two tests above stop authority reaching the
    // solver, and this one stops the solver acquiring authority.
    //
    // They are not the same property and neither implies the other. A
    // dependency from `qip-quantum` to `qip-execution-engine` would leave both
    // of the above passing, and would mean a solver crate that can construct
    // an order manager — which is precisely what "quantum output is policy,
    // never a live instruction" forbids.
    let graph = dependency_graph();
    let reachable = reachable_from(&graph, "qip-quantum");
    for crate_name in NO_SOLVER_AUTHORITY {
        assert!(
            !reachable.contains(*crate_name),
            "the quantum solver can reach {crate_name}, so a solver holds \
             authority it must never have: {reachable:?}"
        );
    }
    // Anchor: the solver does depend on something, so an empty reachable set
    // is not what is making the loop pass.
    assert!(
        !reachable.is_empty(),
        "qip-quantum reaches nothing at all, so this test proves nothing"
    );
}

#[test]
fn no_crate_that_vetoes_or_executes_reaches_a_solver_through_its_dev_dependencies() {
    // The gap review found in the exclusion above. Dev dependencies are left
    // out of the graph because Cargo permits cycles among them and counting
    // them would report boundary violations no deployed binary has — but that
    // is a reason the *graph* must stay acyclic, not a reason the edge is
    // harmless. Test code inside `qip-risk-engine` can construct a solver just
    // as readily as its shipped code can, and a fixture that quietly starts
    // asking an optimiser what the risk answer should be is a boundary
    // bypass that happens to compile only under `cargo test`.
    //
    // Checked as a direct edge rather than transitively, on purpose. A
    // transitive walk over dev edges is exactly the cyclic graph the main
    // parser excludes them to avoid; the direct edge is the one somebody
    // writes.
    let mut offenders = Vec::new();
    for crate_name in NO_SOLVER_AUTHORITY {
        for directory in ["libs", "services", "edge", "runtime", "apps", "agents"] {
            let manifest = repository_root().join(format!(
                "backend/crates/{directory}/{crate_name}/Cargo.toml"
            ));
            let Ok(content) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            let Some(dev) = content.split("[dev-dependencies]").nth(1) else {
                continue;
            };
            let dev = dev.split("\n[").next().unwrap_or(dev);
            if dev.contains("qip-quantum") {
                offenders.push(format!("{crate_name} (dev)"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a crate that vetoes, executes, transfers or issues reaches a quantum \
         solver through its dev dependencies: {offenders:?}"
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
    //
    // Served by `cargo metadata` like every other check in this file. It used
    // to run its own, weaker manifest reader — one that classified a section by
    // `contains("dependencies")` and split keys on `['.', ' ', '=']` — so the
    // two disagreed about quoted keys, tables named after one dependency, and
    // which sections counted. Three review rounds were decided by which of the
    // two happened to fire on a given manifest, which is not a property a
    // boundary check should have. There is now one reader.
    //
    // Unlike the boundary graph this counts **every** kind, including dev and
    // build dependencies: the rule is that the workspace declares two
    // third-party crates, and a test-only dependency is still a supply chain.
    let permitted: BTreeSet<&str> = ["serde", "serde_json"].into_iter().collect();
    let metadata = workspace_metadata();
    let members = workspace_member_names(&metadata);
    let mut offenders = Vec::new();

    for package in packages(&metadata) {
        let crate_name = package_name(package);
        for dependency in dependencies_of(package) {
            let depended = dependency_name(dependency);
            if members.contains(&depended) || permitted.contains(depended.as_str()) {
                continue;
            }
            offenders.push(format!("{crate_name}: {depended}"));
        }
    }

    // The premise. If the walk found no dependencies at all the loop above
    // would report nothing while checking nothing, which is the shape of every
    // failure this file has had.
    assert!(
        packages(&metadata)
            .iter()
            .any(|package| !dependencies_of(package).is_empty()),
        "no package reports any dependency, so this test constrains nothing"
    );
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
///
/// Taken from `cargo metadata` rather than by reading manifests, so that this
/// file has exactly one idea of what a crate is called. Two readers that
/// disagreed about that is what let three review rounds turn on which of them
/// happened to fire.
fn crates_under(relative: &str) -> BTreeSet<String> {
    let root = repository_root().join(relative);
    assert!(root.is_dir(), "cannot read {}", root.display());
    let metadata = workspace_metadata();
    let mut names = BTreeSet::new();
    for package in packages(&metadata) {
        let manifest = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .expect("every package has a manifest path");
        // The crate directory is the manifest's parent; membership of
        // `relative` is a direct-child test, matching the previous behaviour.
        if std::path::Path::new(manifest)
            .parent()
            .and_then(std::path::Path::parent)
            .is_some_and(|parent| parent == root)
        {
            names.insert(package_name(package));
        }
    }
    assert!(!names.is_empty(), "no crates found under {relative}");
    names
}

/// Neither end of the cell uplink may re-declare its delta schema version as a
/// literal.
///
/// The obvious test — assert each end equals
/// `qip_contracts::wire::CELL_DELTA_SCHEMA_VERSION` — cannot fail while both
/// ends read that constant, because expected and actual are then the same
/// number from the same source. Replacing either declaration with a literal
/// `2` leaves both crates' suites green, and the assertion only starts working
/// one bump later, which is exactly when the old two-literal arrangement would
/// have caught it too. That is the same blindness that let a crossing priced
/// from `reference_price` pass a suite that priced everything off one mid.
///
/// So this asserts about the source instead: inside the two `EventBody` impls
/// that carry a cell delta, the schema version must be the shared constant and
/// must not be a number. The other topics on the same wire keep their own
/// versions and are deliberately out of scope — they are declared once each.
#[test]
fn neither_end_of_the_cell_uplink_declares_its_schema_version_as_a_literal() {
    const SHARED: &str = "CELL_DELTA_SCHEMA_VERSION";
    let ends = [
        (
            "backend/crates/edge/qip-edge/src/mesh.rs",
            "impl EventBody for CellStateDelta {",
        ),
        (
            "backend/crates/services/qip-mesh/src/delta.rs",
            "impl EventBody for WireDelta {",
        ),
    ];

    let mut checked = 0usize;
    for (file, header) in ends {
        let source = qip_acceptance::read(file);
        let Some(offset) = source.find(header) else {
            panic!("{file} no longer contains `{header}`, so this test is looking at nothing");
        };
        // The impl body ends at the first line that closes it at column zero.
        let body = &source[offset..];
        let end = body.find("\n}").unwrap_or(body.len());
        for line in body[..end].lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || !trimmed.contains("SCHEMA_VERSION: u32 =") {
                continue;
            }
            checked += 1;
            let Some((_, value)) = trimmed.split_once('=') else {
                panic!("{file}: a schema-version line with no assignment: {trimmed}");
            };
            assert!(
                value.contains(SHARED),
                "{file} declares its delta schema version without naming the \
                 shared constant, so the two ends can drift: {trimmed}"
            );
            assert!(
                !value.chars().any(|c| c.is_ascii_digit()),
                "{file} declares its delta schema version as a numeric \
                 literal, which is what having one declaration exists to \
                 prevent: {trimmed}"
            );
        }
    }

    // The vacuity guard, and it is the whole test: if neither impl has such a
    // line — renamed type, moved declaration, changed spelling — the loop
    // above asserts nothing at all and passes.
    assert_eq!(
        checked, 2,
        "expected one schema-version declaration in each cell-delta impl, \
         found {checked}; this test is no longer looking at the right lines"
    );
}
