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
    let (name, dependencies) = parse_manifest_content(&content);
    assert!(
        !name.is_empty(),
        "{} declares no package name",
        path.display()
    );
    (name, dependencies)
}

/// Whether a TOML table introduces dependencies the crate's *shipped* code can
/// call, and the crate name where the header names one directly.
///
/// `Some(None)` is a table of dependency keys; `Some(Some(name))` is a table
/// describing one named dependency; `None` is a table this file must ignore.
///
/// Four shapes reach here, because Cargo accepts four:
/// `[dependencies]`, `[dependencies.qip-foo]`,
/// `[target.'cfg(unix)'.dependencies]` and
/// `[target.'cfg(unix)'.dependencies.qip-foo]`. A target-conditional
/// dependency is shipped code — the condition selects a platform, not a
/// build stage — so it counts, and missing it would have been a boundary
/// anyone could step over by adding a `cfg` nobody reads.
///
/// Dev and build dependencies are excluded wherever they appear. Dev
/// dependencies are what a crate's own tests link against and Cargo
/// deliberately permits cycles among them; a build dependency is linked into
/// a build script rather than into the crate, so neither is something the
/// shipped code can call.
fn shipped_dependency_table(inner: &str) -> Option<Option<&str>> {
    if inner.contains("dev-dependencies") || inner.contains("build-dependencies") {
        return None;
    }
    let index = inner.find("dependencies")?;
    let rest = &inner[index + "dependencies".len()..];
    match rest.strip_prefix('.') {
        Some(named) => Some(Some(named)),
        None if rest.is_empty() => Some(None),
        None => None,
    }
}

/// The parser itself, split from the file handling so it can be tested against
/// the shapes Cargo accepts rather than only against the shapes this
/// repository happens to use today.
///
/// **Three forms, because Cargo accepts three.** This used to recognise only
/// the dotted key `qip-quantum.workspace = true`, which is what every manifest
/// in the tree is written in. A dependency written
/// `qip-quantum = { workspace = true }` contains no `.` in its key and was
/// therefore invisible to the entire dependency graph — and so was
/// `[dependencies.qip-quantum]`, because the section header never equalled
/// `[dependencies]`.
///
/// That is the failure this function exists to prevent, and it is worth being
/// precise about how bad it was: every boundary test in this file is an
/// assertion that some edge is *absent*, so an edge the parser could not see
/// was an edge that did not exist as far as any of them were concerned.
/// Someone adding `qip-ai = { workspace = true }` to the risk engine — a form
/// cargo accepts and a reviewer would not blink at — would have kept the whole
/// file green.
///
/// No style is mandated in exchange. Enforcing the dotted form everywhere
/// would work, and would make the guarantee depend on a convention nothing
/// checks rather than on the parser.
fn parse_manifest_content(content: &str) -> (String, BTreeSet<String>) {
    let mut name = String::new();
    let mut dependencies = BTreeSet::new();
    let mut in_package = false;
    let mut in_shipped_dependencies = false;
    // A `[dependencies.foo]` table. The key, plus the `package = "..."` rename
    // if the table declares one, resolved when the table ends — the rename can
    // appear on any line inside it, so the name is not known at the header.
    let mut pending: Option<(String, Option<String>)> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            flush_pending(&mut pending, &mut dependencies);
            let section = section_name(line);
            in_package = section == Some("package");
            in_shipped_dependencies = false;
            if let Some(section) = section {
                match shipped_dependency_table(section) {
                    // `[dependencies.qip-foo]` is itself the declaration. The
                    // keys that follow describe that one dependency, so they
                    // are not scanned as further names.
                    Some(Some(named)) => pending = Some((unquote(named).to_string(), None)),
                    Some(None) => in_shipped_dependencies = true,
                    None => {}
                }
            }
            continue;
        }
        if in_package
            && let Some(value) = line.strip_prefix("name")
            && let Some(quoted) = value.split('"').nth(1)
        {
            name = quoted.to_string();
        }
        // Inside `[dependencies.foo]`, a `package = "bar"` line renames it.
        if let Some((_, rename)) = pending.as_mut()
            && let Some(renamed) = package_rename(line)
        {
            *rename = Some(renamed);
        }
        if in_shipped_dependencies {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            // `qip-foo.workspace = true` and `"qip-foo".workspace = true` both
            // reduce to the key before the first `.`; `qip-foo = { ... }` has
            // no `.` and is already the key.
            let key = key.trim();
            let key = key.split_once('.').map_or(key, |(left, _)| left.trim());
            // A rename wins over the key: `alias = { package = "qip-foo" }`
            // depends on `qip-foo`, whatever the alias is called.
            let crate_name = package_rename(value).unwrap_or_else(|| unquote(key).to_string());
            if crate_name.starts_with("qip-") {
                dependencies.insert(crate_name);
            }
        }
    }
    flush_pending(&mut pending, &mut dependencies);
    (name, dependencies)
}

/// The table name in a section header, tolerating comments and inner spaces.
///
/// **This is where a whole section used to be silently dropped.** The header
/// was previously reduced with `trim_start_matches('[').trim_end_matches(']')`,
/// which works only when the `]` is the final character. `[dependencies] # in-tree only`
/// ends in `y`, so nothing was trimmed, the table was not recognised, and every
/// key under it was skipped — the entire crate's dependency list vanished and
/// every absent-edge assertion about it passed vacuously. `[ dependencies ]`,
/// which is legal TOML, failed the same way.
///
/// Taking the text between `[` and the *first* `]` and trimming it handles
/// both, and `[[bin]]` yields `[bin`, which matches no dependency table.
fn section_name(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some(rest[..end].trim())
}

/// Strip the quotes TOML permits around a key or value.
///
/// `"qip-quantum".workspace = true` is a legal way to write a dependency, and
/// an unquoted comparison against `qip-` does not match it.
fn unquote(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'')
}

/// The crate a `package = "..."` rename points at.
///
/// Cargo lets a dependency be declared under any key and renamed to its real
/// crate with `package`. Reading only the key means
/// `qip-numerics-ext = { package = "qip-quantum" }` looks like a dependency on
/// a crate that does not exist, and the real edge to `qip-quantum` is invisible
/// — which is a boundary bypass written in one line of ordinary Cargo.
///
/// Matched on `package` followed by `=` so that `path = "../packages/x"` does
/// not qualify.
fn package_rename(fragment: &str) -> Option<String> {
    let after = fragment.split_once("package")?.1;
    let after = after.trim_start().strip_prefix('=')?;
    after.split('"').nth(1).map(str::to_string)
}

/// Resolve a `[dependencies.foo]` table into the crate it actually names.
fn flush_pending(pending: &mut Option<(String, Option<String>)>, into: &mut BTreeSet<String>) {
    if let Some((key, rename)) = pending.take() {
        let crate_name = rename.unwrap_or(key);
        if crate_name.starts_with("qip-") {
            into.insert(crate_name);
        }
    }
}

/// A manifest fragment and whether it declares a shipped edge to `qip-quantum`.
///
/// Table-driven because the parser's failures have all been *one shape it did
/// not think of*, twice now, and a list is the only form in which the next
/// shape is cheap to add.
const MANIFEST_FORMS: &[(&str, &str, bool)] = &[
    // --- the four table shapes ---
    (
        "dotted",
        "[dependencies]\nqip-quantum.workspace = true\n",
        true,
    ),
    (
        "inline",
        "[dependencies]\nqip-quantum = { workspace = true }\n",
        true,
    ),
    (
        "table",
        "[dependencies.qip-quantum]\nworkspace = true\n",
        true,
    ),
    (
        "target-table",
        "[target.'cfg(unix)'.dependencies]\nqip-quantum.workspace = true\n",
        true,
    ),
    // --- target-conditional, both spellings ---
    (
        "target-named",
        "[target.'cfg(unix)'.dependencies.qip-quantum]\nworkspace = true\n",
        true,
    ),
    (
        "target-double-quoted",
        "[target.\"cfg(windows)\".dependencies]\nqip-quantum.workspace = true\n",
        true,
    ),
    // --- exclusions: not shipped code ---
    (
        "dev",
        "[dev-dependencies]\nqip-quantum.workspace = true\n",
        false,
    ),
    (
        "dev-table",
        "[dev-dependencies.qip-quantum]\nworkspace = true\n",
        false,
    ),
    (
        "build",
        "[build-dependencies]\nqip-quantum.workspace = true\n",
        false,
    ),
    (
        "commented-out",
        "[dependencies]\n# qip-quantum.workspace = true\n",
        false,
    ),
    // A feature named after a crate is not a dependency on it.
    ("features", "[features]\nqip-quantum = []\n", false),
    // --- P1: the section header itself ---
    // A trailing comment used to delete the entire section: the header does
    // not end in `]`, so nothing was trimmed and every key under it was
    // skipped. Verified live — a real `qip-quantum` edge on the risk engine
    // left the whole file green at 23 passed, 0 failed.
    (
        "trailing-comment",
        "[dependencies] # in-tree only\nqip-quantum.workspace = true\n",
        true,
    ),
    (
        "spaces-in-brackets",
        "[ dependencies ]\nqip-quantum.workspace = true\n",
        true,
    ),
    (
        "trailing-comment-on-table",
        "[dependencies.qip-quantum] # the solver\nworkspace = true\n",
        true,
    ),
    // --- P2: renames ---
    // Cargo lets a dependency sit under any key and name its real crate with
    // `package`. Reading only the key made the edge invisible.
    (
        "rename-inline",
        "[dependencies]\nqip-numerics-ext = { package = \"qip-quantum\", version = \"0.1\" }\n",
        true,
    ),
    (
        "rename-table",
        "[dependencies.qip-numerics-ext]\npackage = \"qip-quantum\"\nversion = \"0.1\"\n",
        true,
    ),
    // A rename under a non-`qip` key is still the same edge.
    (
        "rename-under-foreign-key",
        "[dependencies]\nsolver = { package = \"qip-quantum\", version = \"0.1\" }\n",
        true,
    ),
    // And the converse: a `qip-*` key renamed to a third party is not an
    // in-tree edge, so the rename must win over the key in both directions.
    (
        "rename-away-from-qip",
        "[dependencies]\nqip-lookalike = { package = \"serde\", version = \"1\" }\n",
        false,
    ),
    // `path = \"../packages/x\"` contains the substring `package` and must not
    // be read as a rename.
    (
        "path-containing-package",
        "[dependencies]\nqip-quantum = { path = \"../packages/qip-quantum\" }\n",
        true,
    ),
    // --- P3: quoted keys ---
    (
        "quoted-key",
        "[dependencies]\n\"qip-quantum\".workspace = true\n",
        true,
    ),
    (
        "quoted-table",
        "[dependencies.\"qip-quantum\"]\nworkspace = true\n",
        true,
    ),
    // --- third parties are not in-tree edges ---
    (
        "third-party",
        "[dependencies]\nserde = { workspace = true }\n",
        false,
    ),
];

#[test]
fn the_manifest_parser_sees_every_dependency_form_that_reaches_a_crate() {
    // The regression suite for two rounds of the same defect. Every entry here
    // was verified against the real tree by putting the form in a real
    // manifest and running this file; each `true` row is a boundary bypass if
    // the parser cannot see it, and each `false` row is a false edge if it can.
    //
    // The failure class is worth naming once more because it has recurred:
    // every boundary test in this file asserts that some edge is *absent*, so
    // a form the parser cannot read is an edge that does not exist as far as
    // any of them are concerned, and the whole file goes green over a live
    // violation.
    for (label, fragment, expected) in MANIFEST_FORMS {
        let manifest = format!("[package]\nname = \"probe\"\n{fragment}");
        let (name, dependencies) = parse_manifest_content(&manifest);
        assert_eq!(name, "probe", "{label}: the package name was not parsed");
        assert_eq!(
            dependencies.contains("qip-quantum"),
            *expected,
            "{label}: expected qip-quantum edge = {expected}, parsed {dependencies:?}"
        );
    }

    // The premise, without which the loop above could pass on an empty table.
    assert!(
        MANIFEST_FORMS.iter().any(|(_, _, expected)| *expected),
        "no form is expected to produce an edge"
    );
    assert!(
        MANIFEST_FORMS.iter().any(|(_, _, expected)| !*expected),
        "no form is expected to be excluded, so the exclusions are untested"
    );
}

#[test]
fn the_manifest_parser_sees_every_dependency_form_cargo_accepts() {
    // The regression test for the hole described above. Each of these three
    // manifests declares exactly one in-tree dependency, in a different one of
    // the three shapes Cargo permits, and all three must be seen.
    let dotted = "[package]\nname = \"a\"\n[dependencies]\nqip-quantum.workspace = true\n";
    let inline = "[package]\nname = \"b\"\n[dependencies]\nqip-quantum = { workspace = true }\n";
    let table = "[package]\nname = \"c\"\n[dependencies.qip-quantum]\nworkspace = true\n";

    for (label, manifest) in [("dotted", dotted), ("inline", inline), ("table", table)] {
        let (name, dependencies) = parse_manifest_content(manifest);
        assert!(!name.is_empty(), "{label}: no package name parsed");
        assert!(
            dependencies.contains("qip-quantum"),
            "{label}: the dependency was invisible to the parser, so every \
             absent-edge test in this file would pass without checking it"
        );
    }

    // And the exclusions still hold, or the parser would be seeing edges that
    // are not there — which fails in the opposite, noisier direction.
    let dev = "[package]\nname = \"d\"\n[dev-dependencies]\nqip-quantum.workspace = true\n";
    let dev_table = "[package]\nname = \"e\"\n[dev-dependencies.qip-quantum]\nworkspace = true\n";
    for (label, manifest) in [("dev", dev), ("dev-table", dev_table)] {
        let (_, dependencies) = parse_manifest_content(manifest);
        assert!(
            !dependencies.contains("qip-quantum"),
            "{label}: a dev dependency was counted as a shipped one"
        );
    }

    // Third parties are not in-tree edges and must not be collected.
    let third = "[package]\nname = \"f\"\n[dependencies]\nserde = { workspace = true }\n";
    assert!(parse_manifest_content(third).1.is_empty());

    // Target-conditional dependencies are shipped code — the condition selects
    // a platform, not a build stage — so they count. Missing them would leave
    // a boundary anyone could step over by adding a `cfg` nobody reads.
    let target_table = "[package]\nname = \"g\"\n[target.'cfg(unix)'.dependencies]\nqip-quantum.workspace = true\n";
    let target_named = "[package]\nname = \"h\"\n[target.'cfg(unix)'.dependencies.qip-quantum]\nworkspace = true\n";
    for (label, manifest) in [("target", target_table), ("target-named", target_named)] {
        assert!(
            parse_manifest_content(manifest).1.contains("qip-quantum"),
            "{label}: a target-conditional dependency was invisible"
        );
    }

    // Build dependencies are linked into a build script rather than into the
    // crate, so they are not something the shipped code can call.
    let build = "[package]\nname = \"i\"\n[build-dependencies]\nqip-quantum.workspace = true\n";
    assert!(!parse_manifest_content(build).1.contains("qip-quantum"));
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
