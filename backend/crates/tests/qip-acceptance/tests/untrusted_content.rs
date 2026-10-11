//! R12-SEC-9065: untrusted web content cannot directly invoke a
//! high-authority tool or reach a production secret.
//!
//! The crates that fetch from outside — market and vendor connectors, the
//! data finder, and the HTTP client both are built on — handle bytes a
//! stranger chose. Two paths would turn those bytes into authority, and each
//! is closed here by something a reviewer can check:
//!
//! * **To a tool.** A fetching crate that could reach the agency engine, the
//!   agent organisation, a language model, the execution or capital crates
//!   or the edge cell could call one with what it fetched. The dependency
//!   graph has no such edge, read from `cargo metadata` rather than from the
//!   manifests' text, transitively.
//! * **To a secret.** The connector sends a credential as a header, so a
//!   fetched value that could choose *which* credential would send the venue
//!   key to whatever host asked for it. The only secret reads are at connect
//!   time, before any response exists, from the reference the operator's
//!   manifest names.
#![allow(clippy::panic_in_result_fn)]

use std::collections::{BTreeMap, BTreeSet};

use qip_acceptance::{production_text, read, repository_root};

/// The workspace's shipped dependency graph, from `cargo metadata`. Fails
/// closed: every assertion below is an absence, and an empty graph would
/// satisfy all of them.
fn dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let manifest = repository_root().join("backend/Cargo.toml");
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
        .unwrap_or_else(|error| panic!("could not run `{cargo} metadata`: {error}"));
    assert!(
        output.status.success(),
        "`{cargo} metadata` exited with {}",
        output.status
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("`{cargo} metadata` is not JSON: {error}"));
    let packages = metadata["packages"]
        .as_array()
        .unwrap_or_else(|| panic!("cargo metadata reported no packages"));
    let members: BTreeSet<String> = packages
        .iter()
        .filter_map(|package| package["name"].as_str().map(str::to_string))
        .collect();
    let mut graph = BTreeMap::new();
    for package in packages {
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        let edges = package["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|dependency| {
                !matches!(dependency["kind"].as_str(), Some("dev") | Some("build"))
            })
            .filter_map(|dependency| dependency["name"].as_str().map(str::to_string))
            .filter(|dependency| members.contains(dependency))
            .collect();
        graph.insert(name.to_string(), edges);
    }
    assert!(
        graph.len() > 25,
        "cargo metadata reported only {} workspace members",
        graph.len()
    );
    graph
}

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

/// The crates whose input is bytes from outside the platform.
const FETCHERS: [&str; 3] = ["qip-market-ingestion", "qip-data-finder", "qip-transport"];

/// What a fetched value must never be able to call: the agency engine and its
/// action adapters, the agent organisation and the model interface, the
/// execution, broker, chain and capital crates, the edge cell, and the
/// kernel that composes all of them.
const HIGH_AUTHORITY: [&str; 11] = [
    "qip-agency",
    "qip-agents",
    "qip-ai",
    "qip-execution-engine",
    "qip-brokers",
    "qip-chain",
    "qip-capital",
    "qip-capital-fabric",
    "qip-edge",
    "qip-kernel",
    "qip-cost-router",
];

#[test]
fn nothing_that_fetches_untrusted_content_can_reach_a_tool_an_order_or_capital() {
    let graph = dependency_graph();
    for name in FETCHERS.iter().chain(HIGH_AUTHORITY.iter()) {
        // The vacuity guard: an absence asserted of a name that is not a
        // crate is true by not existing.
        assert!(
            graph.contains_key(*name),
            "{name} is not a crate in this workspace; the boundary below names nothing"
        );
    }
    // Premise: the walk sees real edges, so an empty reachable set is a
    // finding and not a misread graph.
    assert!(
        reachable_from(&graph, "qip-data-finder").contains("qip-market-ingestion"),
        "premise: the data finder no longer reaches the connectors it admits"
    );
    for fetcher in FETCHERS {
        let reachable = reachable_from(&graph, fetcher);
        for authority in HIGH_AUTHORITY {
            assert!(
                !reachable.contains(authority),
                "{fetcher} handles fetched bytes and can reach {authority}; untrusted content \
                 would then be one call away from a tool, an order or capital"
            );
        }
    }
}

#[test]
fn a_credential_is_chosen_by_the_operators_manifest_and_read_before_any_response_exists() {
    // The connector resolves its credential in `connect_with`, from the
    // reference the manifest names, and holds the value from then on. A
    // second call site — say, one reading a variable a response named — is
    // the shape this refuses.
    let transport = production_text(&read(
        "backend/crates/services/qip-market-ingestion/src/connector/transport.rs",
    ));
    assert!(
        transport.contains("pub fn connect_with("),
        "premise: the connector's constructor is not where this test looks"
    );
    let calls: Vec<&str> = transport
        .match_indices("Self::require_secret(")
        .map(|(at, _)| {
            let rest = &transport[at + "Self::require_secret(".len()..];
            rest.split(')').next().unwrap_or("")
        })
        .collect();
    assert!(
        !calls.is_empty(),
        "premise: no credential read was found in the connector"
    );
    for arguments in &calls {
        let reference = arguments.split(',').nth(1).map(str::trim).unwrap_or("");
        assert!(
            reference == "primary" || reference == "&companion.secret",
            "a credential is read from {reference:?}, not from the reference the manifest's \
             auth block names"
        );
    }
    let primaries = transport
        .matches("let primary = manifest.auth.secret.as_ref()")
        .count();
    assert!(
        primaries >= 1,
        "`primary` is no longer the manifest's own secret reference"
    );
    // The resolver is invoked in exactly one place: inside `require_secret`.
    let invocations = transport.matches("resolve(reference)").count();
    let any_resolve = transport.matches("resolve(").count();
    assert_eq!(
        (invocations, any_resolve),
        (1, 1),
        "the secret resolver is called somewhere other than `require_secret`"
    );
    // And a response's handling path holds no resolver at all: the type that
    // reads a credential is not a field of the connected transport.
    let fields_start = transport
        .find("pub struct HttpSourceTransport")
        .unwrap_or_else(|| panic!("premise: the connected transport's type was not found"));
    let declaration = &transport[fields_start..];
    let body = declaration
        .split_once('{')
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or("", |(fields, _)| fields);
    assert!(
        !body.contains("SecretResolver"),
        "the connected transport keeps a secret resolver, so code handling a response could \
         read a credential a response named"
    );
}
