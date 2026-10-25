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
//!
//! And, since ADR 0158 decisions 16-19 were built, the finer net beside the
//! graph: what a fetch hands back is `qip_core::untrusted::Untrusted<_>`, so
//! fetched bytes reach a typed value only through `validate` or a visible
//! `inspect`.
//!
//! * **Every fetch returns `Untrusted`.** Each `HttpClient::send` in a
//!   fetching crate sits in a function whose return type is `Untrusted<_>`,
//!   or a response type whose `body` field is. A source scan, the weaker kind
//!   of test, so it is paired with the type: once the return type is
//!   `Untrusted`, the compiler holds every caller.
//! * **No fetching crate opens a socket of its own**, so `send` is the
//!   whole of what the first check has to find.
//! * **`inspect()` is called only from an allow-list** of files that display,
//!   hash or store fetched content, so a new place reading untrusted text
//!   directly is a change to this file that a reviewer sees.
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

/// The production sources of the crates whose functions read bytes a
/// stranger chose. `qip-transport` is the client they read through, not a
/// reader of content, and its response type is shared with trusted peers
/// (KMS, storage, the mesh); the wrapping happens where a fetching crate
/// takes the bytes.
const FETCHING_SOURCES: [&str; 2] = [
    "backend/crates/services/qip-market-ingestion/src",
    "backend/crates/services/qip-data-finder/src",
];

/// The call through which a fetching crate reads from outside:
/// `qip_transport::HttpClient::send`.
const FETCH_CALL: &str = ".send(";

/// Each production Rust file under `relative`, as (path from the repository
/// root, production text).
fn production_files(relative: &str) -> Vec<(String, String)> {
    let root = repository_root();
    qip_acceptance::files_with_extension(relative, "rs")
        .into_iter()
        .map(|path| {
            let shown = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let text = production_text(
                &std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display())),
            );
            (shown, text)
        })
        .collect()
}

/// One place a fetching crate calls `send`, with the function it sits in.
#[derive(Debug)]
struct FetchSite {
    file: String,
    function: String,
    returns: String,
}

/// The signature of the last function declared in `before`: its name and
/// its return type (`()` where it declares none).
fn enclosing_function(before: &str) -> Option<(String, String)> {
    let start = before.lines().rev().find_map(|line| {
        let trimmed = line.trim_start();
        let declaration = ["fn ", "pub fn ", "pub(crate) fn ", "pub(super) fn "]
            .iter()
            .find_map(|prefix| trimmed.strip_prefix(prefix))?;
        let name: String = declaration
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some((name, line))
    })?;
    let (name, line) = start;
    let at = before.rfind(line)?;
    let signature = before[at..].split('{').next().unwrap_or("");
    let returns = signature
        .split_once("->")
        .map_or("()", |(_, returns)| {
            returns.split(" where").next().unwrap_or("")
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some((name, returns))
}

fn fetch_sites() -> Vec<FetchSite> {
    let mut sites = Vec::new();
    for crate_source in FETCHING_SOURCES {
        for (file, text) in production_files(crate_source) {
            for (at, _) in text.match_indices(FETCH_CALL) {
                let (function, returns) = enclosing_function(&text[..at])
                    .unwrap_or_else(|| panic!("{file}: a fetch outside any function"));
                sites.push(FetchSite {
                    file: file.clone(),
                    function,
                    returns,
                });
            }
        }
    }
    sites
}

/// The fields of `struct name` declared anywhere in `crate_source`, or
/// `None` where the crate declares no such struct.
fn struct_fields(crate_source: &str, name: &str) -> Option<String> {
    let declaration = format!("struct {name} {{");
    production_files(crate_source)
        .into_iter()
        .find_map(|(_, text)| {
            let at = text.find(&declaration)?;
            let body = &text[at + declaration.len()..];
            Some(body.split("\n}").next().unwrap_or("").to_string())
        })
}

#[test]
fn every_fetch_in_a_fetching_crate_hands_back_untrusted_content() {
    let sites = fetch_sites();
    // Premise: the scan finds the fetches it is about. The connector
    // transport and the data finder's probe are each one; an empty list would
    // make every assertion below true of nothing.
    for expected in [
        "qip-market-ingestion/src/connector/transport.rs",
        "qip-data-finder/src/probe.rs",
    ] {
        assert!(
            sites.iter().any(|site| site.file.ends_with(expected)),
            "premise: no `{FETCH_CALL}` was found in {expected}; sites found: {sites:?}"
        );
    }
    let mut plain = Vec::new();
    for site in &sites {
        if site.returns.contains("Untrusted<") {
            continue;
        }
        // A response type is admitted when its body is the untrusted part:
        // `Result<SourceResponse>` with `pub body: Untrusted<String>`.
        let named = site
            .returns
            .trim_start_matches("Result<")
            .trim_end_matches('>')
            .trim();
        let crate_source = FETCHING_SOURCES
            .iter()
            .find(|source| site.file.starts_with(*source))
            .unwrap_or_else(|| panic!("{} is outside the fetching crates", site.file));
        let wrapped = named.chars().all(|c| c.is_alphanumeric() || c == '_')
            && struct_fields(crate_source, named)
                .is_some_and(|fields| fields.contains("pub body: Untrusted<"));
        if !wrapped {
            plain.push(format!(
                "{}::{} returns `{}`",
                site.file, site.function, site.returns
            ));
        }
    }
    assert!(
        plain.is_empty(),
        "these fetches hand fetched bytes back as something other than Untrusted<_>, so the \
         content can reach a tool argument, a brief or a record without `validate` or \
         `inspect`: {plain:#?}"
    );
}

#[test]
fn no_fetching_crate_opens_a_socket_of_its_own() {
    // The scan above finds fetches by `HttpClient::send`. A crate that opened
    // a `TcpStream` would fetch without calling it and walk round the scan.
    let mut checked = 0;
    for crate_source in FETCHING_SOURCES {
        for (file, text) in production_files(crate_source) {
            checked += 1;
            for socket in ["TcpStream", "UdpSocket", "TcpListener", "ToSocketAddrs"] {
                assert!(
                    !text.contains(socket),
                    "{file} names {socket}; a fetching crate reads through \
                     qip_transport::HttpClient so its fetches can be found and wrapped"
                );
            }
        }
    }
    assert!(checked > 10, "premise: only {checked} files were scanned");
}

/// The files that may read untrusted content directly, and why each one
/// needs to. Everything else goes through `validate`.
const INSPECT_ALLOWED: [(&str, &str); 4] = [
    (
        "backend/crates/libs/qip-ai/src/language.rs",
        "UntrustedModelRequest::render: the one place a fetched block is shown to a model, under \
         the `untrusted/` prefix, inside `complete_over_untrusted`, whose answer is wrapped as \
         UntrustedCompletion before it is returned (ADR 0158 decision 18)",
    ),
    (
        "backend/crates/services/qip-market-ingestion/src/connector/transport.rs",
        "SourceResponse::body_excerpt: a bounded excerpt for an operator's health detail and \
         quarantine record",
    ),
    (
        "backend/crates/services/qip-market-ingestion/src/connector/runtime.rs",
        "the fetch digest hashes and measures the bytes as served, for the reference ledger",
    ),
    (
        "backend/crates/services/qip-market-ingestion/src/fetched.rs",
        "the bounded excerpt a vendor-adapter refusal quotes to the operator",
    ),
];

#[test]
fn untrusted_content_is_inspected_only_where_it_is_displayed_hashed_or_stored() {
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for (file, text) in production_files("backend/crates") {
        // Integration tests and fixtures may look at what they built.
        if file.contains("/tests/") || file.contains("/benches/") || file.contains("/examples/") {
            continue;
        }
        // The method takes no argument, which is what tells it apart from
        // `Iterator::inspect` and `Option::inspect`; the qualified form is
        // counted too, so `Untrusted::inspect(&x)` is no side door. Not every
        // `::inspect(` — `ManipulationRisk::inspect` is a different function.
        let calls =
            text.matches(".inspect()").count() + text.matches("Untrusted::inspect(").count();
        if calls > 0 {
            found.insert(file, calls);
        }
    }
    let allowed: BTreeSet<&str> = INSPECT_ALLOWED.iter().map(|(file, _)| *file).collect();
    let outside: Vec<_> = found
        .iter()
        .filter(|(file, _)| !allowed.contains(file.as_str()))
        .collect();
    assert!(
        outside.is_empty(),
        "untrusted content is read directly outside the allow-list; validate it into a narrower \
         type instead, or add the file to INSPECT_ALLOWED with the reason it must display, hash \
         or store the content: {outside:?}"
    );
    // And the allow-list does not rot: a file listed and no longer inspecting
    // is a permission nobody uses, waiting for somebody to use it.
    for file in allowed {
        assert!(
            found.contains_key(file),
            "{file} is allow-listed to inspect untrusted content and no longer does; remove it \
             from INSPECT_ALLOWED"
        );
    }
}
