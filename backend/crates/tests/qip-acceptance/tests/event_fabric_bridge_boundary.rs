#![allow(clippy::unwrap_used, clippy::expect_used)]
//! FABRIC-014 (and the Rust half of FABRIC-001): a Kafka or Pub/Sub client
//! may exist only inside the one edge bridge, nothing internal may depend on
//! that bridge, and the bridge holds no durable state of its own.
//!
//! No bridge exists today, which is the strongest form of the rule and not a
//! gap in it: the test below holds the present state (no manifest in the
//! workspace declares a broker client of either family) and states what a
//! bridge would be allowed, so the first one to appear is judged by this
//! file rather than argued into the tree. `qip-streaming`'s `PubSubTransport`
//! is a port that returns `Unavailable`, and this test is what keeps it from
//! quietly acquiring a client.
//!
//! What it deliberately does not check: Terraform. `modules/secrets` owns a
//! Pub/Sub topic for secret rotation, which is a Google-managed notification
//! channel and not an internal service-to-service message path, so a blanket
//! "no topic in Terraform" assertion would be false on day one. Judging that
//! topic against FABRIC-001 is a decision for a person, recorded on that
//! requirement, not something this scan can settle.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The only crate name allowed to hold a Kafka or Pub/Sub client.
const BRIDGE: &str = "qip-fabric-bridge";

/// Dependency-name fragments that identify a Kafka or Pub/Sub client.
const CLIENT_FRAGMENTS: [&str; 5] = ["kafka", "pubsub", "pub-sub", "pub_sub", "librdkafka"];

fn backend() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn manifests() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(read) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = read.map(|e| e.expect("entry").path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n == "Cargo.toml") {
                out.push(p);
            }
        }
    }
    let mut out = vec![backend().join("Cargo.toml")];
    walk(&backend().join("crates"), &mut out);
    out
}

/// The package name and every dependency key a manifest declares, from any
/// `[dependencies]`-family table. Comments are ignored: `qip-streaming`'s own
/// manifest names Pub/Sub in prose and must not trip the scan.
fn parse(manifest: &str) -> (Option<String>, BTreeSet<String>) {
    let mut name = None;
    let mut deps = BTreeSet::new();
    let mut section = String::new();
    for raw in manifest.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            // `[dependencies.some-crate]` names the dependency in the header.
            if let Some((_, dep)) = section.split_once("dependencies.") {
                deps.insert(dep.trim().trim_matches('"').to_string());
            }
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        if section == "package" && key == "name" {
            name = line
                .split_once('=')
                .map(|(_, v)| v.trim().trim_matches('"').to_string());
        }
        if section.ends_with("dependencies") {
            deps.insert(key.split('.').next().unwrap_or(key).to_string());
        }
    }
    (name, deps)
}

fn names_a_client(dep: &str) -> bool {
    let lower = dep.to_lowercase();
    CLIENT_FRAGMENTS.iter().any(|f| lower.contains(f))
}

/// Offending `(package, dependency)` pairs among `manifests`.
fn violations(manifests: &[(Option<String>, BTreeSet<String>)]) -> Vec<String> {
    let mut out = Vec::new();
    for (name, deps) in manifests {
        let package = name.as_deref().unwrap_or("<workspace>");
        for dep in deps {
            if names_a_client(dep) && package != BRIDGE {
                out.push(format!(
                    "{package} depends on {dep}, a Kafka or Pub/Sub client; only {BRIDGE} may"
                ));
            }
            if dep == BRIDGE {
                out.push(format!(
                    "{package} depends on {BRIDGE}; nothing internal may, because the bridge is \
                     an external boundary and not a service"
                ));
            }
        }
    }
    out
}

/// The workspace holds the rule today.
///
/// Mutation: add `rdkafka = "0.36"` under `[dependencies]` in any crate's
/// `Cargo.toml` other than a crate named `qip-fabric-bridge` — fails naming
/// that crate and the dependency (the manifest need not resolve; the scan
/// reads text).
#[test]
fn no_workspace_crate_declares_a_kafka_or_pub_sub_client_or_depends_on_the_bridge() {
    let parsed: Vec<_> = manifests()
        .iter()
        .map(|p| parse(&fs::read_to_string(p).unwrap()))
        .collect();
    // Premise: the walk found the workspace and its crates, and the parser
    // is reading dependency tables; an empty read would pass vacuously.
    assert!(parsed.len() > 50, "found only {} manifests", parsed.len());
    assert!(
        parsed
            .iter()
            .any(|(name, deps)| name.as_deref() == Some("qip-streaming") && deps.contains("serde")),
        "the parser did not read qip-streaming's dependency table"
    );

    let found = violations(&parsed);
    assert!(
        found.is_empty(),
        "the stateless-edge-bridge rule (FABRIC-014) is broken:\n{}",
        found.join("\n")
    );
}

/// If a bridge crate ever exists it holds no durable state of its own: its
/// source must not name the storage crate's durable types. The bridge
/// commits fabric consumer offsets, which live in the fabric, not here.
///
/// Mutation: create `crates/services/qip-fabric-bridge/src/lib.rs` containing
/// `use qip_storage::DurableStore;` — fails naming the file.
#[test]
fn a_bridge_crate_if_one_exists_holds_no_durable_state() {
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(read) = fs::read_dir(dir) else { return };
        for e in read {
            let p = e.expect("entry").path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut bridge_sources = Vec::new();
    for manifest in manifests() {
        let (name, _) = parse(&fs::read_to_string(&manifest).unwrap());
        if name.as_deref() == Some(BRIDGE) {
            rust_files(&manifest.parent().unwrap().join("src"), &mut bridge_sources);
        }
    }
    let durable = [
        "DurableStore",
        "SegmentLog",
        "ChainArchive",
        "std::fs::write",
        "File::create",
    ];
    for source in &bridge_sources {
        let text = fs::read_to_string(source).unwrap();
        for token in durable {
            assert!(
                !text.contains(token),
                "{} names {token}: the bridge must hold no durable state of its own",
                source.display()
            );
        }
    }
}

/// The scan itself on fixtures, so a clean tree is evidence and not blindness.
#[test]
fn the_scan_refuses_a_client_outside_the_bridge_and_a_dependency_on_it() {
    let bad = parse("[package]\nname = \"qip-api\"\n[dependencies]\nrdkafka = \"0.36\"\n");
    assert_eq!(violations(&[bad]).len(), 1);
    let pubsub = parse(
        "[package]\nname = \"qip-kernel\"\n[dependencies.google-cloud-pubsub]\nversion = \"1\"\n",
    );
    assert_eq!(violations(&[pubsub]).len(), 1);
    let depends = parse(
        "[package]\nname = \"qip-kernel\"\n[dependencies]\nqip-fabric-bridge = { path = \"x\" }\n",
    );
    assert_eq!(violations(&[depends]).len(), 1);
    let bridge =
        parse("[package]\nname = \"qip-fabric-bridge\"\n[dependencies]\nrdkafka = \"0.36\"\n");
    assert!(violations(&[bridge]).is_empty());
    let prose = parse(
        "[package]\nname = \"qip-streaming\"\n[dependencies]\n# Pub/Sub lives elsewhere\nserde.workspace = true\n",
    );
    assert!(violations(&[prose]).is_empty());
}
