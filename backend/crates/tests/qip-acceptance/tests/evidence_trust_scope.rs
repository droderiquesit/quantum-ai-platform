//! R12-EVID-1154: the "Evidence & trust" capability's nine scope items, each
//! mapped to production code a composition root links and to a test that
//! proves it.
//!
//! The requirement is an inspection, and an inspection nobody can re-run is
//! a claim. Every scope item had code before this file; what was missing is
//! a place that fails when an item's code is renamed away, loses its last
//! production caller, falls out of the root's dependency graph, or loses the
//! test that proved it. Each of those has happened to some control in this
//! repository: `RiskState::expected_shortfall` shipped in every default
//! limit set while nothing filled it, and `with_v2` existed for a release
//! with only tests calling it.
//!
//! # What "reached" means here, exactly
//!
//! This is a source inspection, not a runtime call graph. For each row it
//! establishes four things, and a reader should not take it for more:
//!
//! 1. the symbol is *defined* in the named production file (not in a
//!    comment, not under `#[cfg(test)]`);
//! 2. a *different* production file names it — a production caller, so the
//!    symbol is not dead code its own module merely declares;
//! 3. the crates of both files are in the named composition root's
//!    workspace dependency closure (`[dependencies]` only, never
//!    `[dev-dependencies]`), so the root links the code;
//! 4. the named test function exists, is a `#[test]`, in the named file.
//!
//! Whether a deployment actually exercises the code is configuration:
//! the triage for this row records that freshness and manipulation run via
//! the deep brain's discovery desk and identity, provenance and licensing
//! via connector arms, both off by default. That switch is owner-decided
//! terraform, outside this test's reach.

use qip_acceptance::{production_text, read, repository_root};
use std::collections::{BTreeMap, BTreeSet};

/// One scope item and the evidence for it.
struct Item {
    scope: &'static str,
    symbol: &'static str,
    defined_in: &'static str,
    caller_in: &'static str,
    root: &'static str,
    test_file: &'static str,
    test_fn: &'static str,
}

/// The nine items, in the requirement's order. Paths are relative to
/// `backend/crates/`.
const ITEMS: [Item; 9] = [
    Item {
        scope: "source identity",
        symbol: "source_identity",
        defined_in: "libs/qip-evidence/src/evidence.rs",
        caller_in: "services/qip-market-ingestion/src/connector/evidence.rs",
        root: "qip-api",
        test_file: "services/qip-world-model/tests/evidence_fabric.rs",
        test_fn: "a_tls_domain_is_credited_for_the_sources_host_and_refused_as_spoofing_otherwise",
    },
    Item {
        scope: "provenance",
        symbol: "Provenance",
        defined_in: "libs/qip-financial/src/quality.rs",
        caller_in: "services/qip-market-ingestion/src/narrative.rs",
        root: "qip-api",
        test_file: "services/qip-market-ingestion/tests/narrative_feed.rs",
        test_fn: "a_filing_is_stamped_with_its_filing_time_rather_than_the_end_of_the_period_it_covers",
    },
    Item {
        scope: "licensing",
        symbol: "LicensingDecision",
        defined_in: "services/qip-data-finder/src/admission.rs",
        caller_in: "apps/qip-api/src/feed.rs",
        root: "qip-api",
        test_file: "apps/qip-api/tests/feed.rs",
        test_fn: "a_connector_whose_licensing_posture_is_not_evaluated_is_refused_before_any_socket_opens",
    },
    Item {
        scope: "freshness",
        symbol: "freshness_sla",
        defined_in: "services/qip-market-ingestion/src/connector/manifest.rs",
        caller_in: "services/qip-market-ingestion/src/connector/runtime.rs",
        root: "qip-api",
        test_file: "services/qip-market-ingestion/tests/connector_manifest.rs",
        test_fn: "a_freshness_sla_shorter_than_the_poll_interval_is_refused",
    },
    Item {
        scope: "corroboration",
        symbol: "corroborated",
        defined_in: "libs/qip-evidence/src/fabric.rs",
        caller_in: "services/qip-world-model/src/poisoning.rs",
        root: "qip-api",
        test_file: "services/qip-world-model/tests/evidence_fabric.rs",
        test_fn: "a_second_independent_source_corroborates_and_a_copy_of_the_same_document_does_not",
    },
    Item {
        scope: "contradictions",
        symbol: "BecameContested",
        defined_in: "libs/qip-evidence/src/fabric.rs",
        caller_in: "runtime/qip-kernel/src/evidence_intake.rs",
        root: "qip-api",
        test_file: "runtime/qip-kernel/tests/evidence_intake.rs",
        test_fn: "two_sources_disagreeing_are_a_contest_only_once_forwarded_into_the_world_fabric",
    },
    Item {
        scope: "manipulation detection",
        symbol: "detect_poisoning",
        defined_in: "services/qip-world-model/src/poisoning.rs",
        caller_in: "runtime/qip-kernel/src/evidence_intake.rs",
        root: "qip-api",
        test_file: "runtime/qip-kernel/tests/evidence_intake.rs",
        test_fn: "a_coordinated_campaign_forwarded_from_connectors_is_found_by_the_poisoning_detector",
    },
    Item {
        scope: "confidence",
        symbol: "belief_weight",
        defined_in: "libs/qip-evidence/src/fabric.rs",
        caller_in: "runtime/qip-kernel/src/evidence_intake.rs",
        root: "qip-api",
        test_file: "runtime/qip-kernel/tests/evidence_intake.rs",
        test_fn: "an_unchecked_claim_under_a_forged_admissible_verdict_is_held_inadmissible_and_weighs_nothing",
    },
    Item {
        scope: "audit trail",
        symbol: "verify_chain",
        defined_in: "libs/qip-events/src/log.rs",
        caller_in: "apps/qip-api/src/console.rs",
        root: "qip-api",
        test_file: "libs/qip-events/tests/backbone.rs",
        test_fn: "editing_stored_history_is_detected_by_the_chain",
    },
];

/// The scope items the requirement names, in its words.
const REQUIRED_SCOPE: [&str; 9] = [
    "source identity",
    "provenance",
    "licensing",
    "freshness",
    "corroboration",
    "contradictions",
    "manipulation detection",
    "confidence",
    "audit trail",
];

fn crates_path(relative: &str) -> String {
    format!("backend/crates/{relative}")
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `text` holds `token` with no identifier character either side —
/// `verify_chain` must not be found inside `verify_chain_from`.
fn names_token(text: &str, token: &str) -> bool {
    text.match_indices(token).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + token.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Whether `text` defines `symbol`: an item keyword before it, a field
/// declaration, or an enum variant on its own line.
fn defines(text: &str, symbol: &str) -> bool {
    let keyword = [
        "fn ", "struct ", "enum ", "const ", "type ", "trait ", "static ",
    ]
    .iter()
    .any(|k| names_token(text, &format!("{k}{symbol}")));
    keyword
        || text.lines().any(|line| {
            let line = line.trim_start();
            let line = line.strip_prefix("pub ").unwrap_or(line);
            [":", ",", " {", "(", " ="]
                .iter()
                .any(|end| line.starts_with(&format!("{symbol}{end}")))
        })
}

/// The `[package] name` and the workspace dependencies of every crate,
/// keyed by package name, read from the manifests.
fn workspace() -> BTreeMap<String, (String, BTreeSet<String>)> {
    let mut crates = BTreeMap::new();
    let root = repository_root().join("backend/crates");
    let Ok(groups) = std::fs::read_dir(&root) else {
        panic!("cannot read {}", root.display());
    };
    for group in groups.flatten() {
        let Ok(members) = std::fs::read_dir(group.path()) else {
            continue;
        };
        for member in members.flatten() {
            let manifest = member.path().join("Cargo.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            let mut name = None;
            let mut section = String::new();
            let mut dependencies = BTreeSet::new();
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    section = line.to_string();
                    continue;
                }
                if section == "[package]" && line.starts_with("name") {
                    name = line.split('"').nth(1).map(str::to_string);
                }
                let runtime = section == "[dependencies]"
                    || (section.starts_with("[target.") && section.ends_with(".dependencies]"));
                if runtime
                    && line.starts_with("qip-")
                    && let Some(dependency) = line.split(['.', ' ', '=']).next()
                {
                    dependencies.insert(dependency.to_string());
                }
            }
            let directory = member
                .path()
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(name) = name {
                crates.insert(name, (directory, dependencies));
            }
        }
    }
    crates
}

/// The crate directories (relative to `backend/crates/`) `root` links.
fn closure(crates: &BTreeMap<String, (String, BTreeSet<String>)>, root: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut stack = vec![root.to_string()];
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some((directory, dependencies)) = crates.get(&name) {
            directories.insert(directory.clone());
            stack.extend(dependencies.iter().cloned());
        }
    }
    directories
}

/// The crate directory a path under `backend/crates/` belongs to.
fn crate_of(relative: &str) -> String {
    relative.split('/').take(2).collect::<Vec<_>>().join("/")
}

/// The failure this prevents: a scope item whose code is renamed away,
/// loses its last production caller, drops out of the root's graph, or
/// loses the test that proved it, while the requirement still reads as
/// held. Every problem is collected and reported together, naming the item
/// and the symbol, so one run says everything that is wrong.
///
/// Mutation: rename one row's symbol to one that does not exist
/// (`detect_poisoning` to `detect_poisoning_nowhere`) — the test fails
/// naming "manipulation detection" and the missing symbol.
#[test]
fn every_evidence_and_trust_scope_item_names_a_symbol_a_composition_root_reaches_and_a_test_that_proves_it()
 {
    // Premise: the table covers exactly the requirement's nine items, once.
    let scopes: Vec<&str> = ITEMS.iter().map(|item| item.scope).collect();
    assert_eq!(
        scopes, REQUIRED_SCOPE,
        "the table must name the requirement's nine scope items, in its order"
    );

    let crates = workspace();
    assert!(
        crates.contains_key("qip-api") && crates.contains_key("qip-evidence"),
        "premise: the manifests were read ({} crates)",
        crates.len()
    );

    let mut problems = Vec::new();
    for item in &ITEMS {
        let label = format!("{} ({})", item.scope, item.symbol);

        let definition = production_text(&read(&crates_path(item.defined_in)));
        assert!(
            !definition.trim().is_empty(),
            "premise: {} holds production code",
            item.defined_in
        );
        if !defines(&definition, item.symbol) {
            problems.push(format!(
                "{label}: `{}` is not defined in the production text of {}",
                item.symbol, item.defined_in
            ));
        }

        if item.caller_in == item.defined_in {
            problems.push(format!(
                "{label}: the caller must be a different file from the definition"
            ));
        }
        let caller = production_text(&read(&crates_path(item.caller_in)));
        if !names_token(&caller, item.symbol) {
            problems.push(format!(
                "{label}: no production caller — {} does not name `{}` outside comments and tests",
                item.caller_in, item.symbol
            ));
        }

        let linked = closure(&crates, item.root);
        let main = repository_root().join(format!(
            "backend/crates/{}/src/main.rs",
            crates.get(item.root).map(|c| c.0.as_str()).unwrap_or("")
        ));
        if !main.is_file() {
            problems.push(format!(
                "{label}: {} is not a composition root with a main.rs",
                item.root
            ));
        }
        for file in [item.defined_in, item.caller_in] {
            if !linked.contains(&crate_of(file)) {
                problems.push(format!(
                    "{label}: {} is not in {}'s dependency closure",
                    crate_of(file),
                    item.root
                ));
            }
        }

        let tests = read(&crates_path(item.test_file));
        let signature = format!("fn {}(", item.test_fn);
        match tests.find(&signature) {
            None => problems.push(format!(
                "{label}: {} has no `{}`",
                item.test_file, item.test_fn
            )),
            Some(at) => {
                // The attributes and doc comment directly above the
                // signature, and nothing further up: a `#[test]` on the
                // previous function must not vouch for this one.
                let above = &tests[..tests[..at].rfind('\n').unwrap_or(0)];
                let attributed = above
                    .lines()
                    .rev()
                    .take_while(|line| {
                        let line = line.trim_start();
                        line.starts_with("#[") || line.starts_with("///")
                    })
                    .any(|line| line.trim() == "#[test]");
                if !attributed {
                    problems.push(format!(
                        "{label}: `{}` in {} is not a #[test]",
                        item.test_fn, item.test_file
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "Evidence & trust scope items without the evidence the requirement asks for:\n{}",
        problems.join("\n")
    );
}
