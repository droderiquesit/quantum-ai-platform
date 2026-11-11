//! R12-LEDGER-8138: change streams and the warehouse feed analytics only,
//! never the system of record. "No component reads balances from a derived
//! copy for a decision."
//!
//! The derived copies this tree can build are the research warehouse
//! (`qip_mesh::bigquery::BigQueryWarehouse`) and, once Spanner holds the
//! ledger, its change streams. The ledger of record is `qip-ledgerd`'s store,
//! read through `LedgerStore::balances` and `LedgerObligations`. So the
//! property is held at the only seams a derived copy could enter a decision
//! through:
//!
//! - outside `qip-mesh`, the one production source that names the warehouse
//!   client is the deep brain's outcome sink, and it only inserts;
//! - no production source names a change-stream reader at all.
//!
//! A new warehouse caller, or a query added to the sink, fails here and has
//! to argue its way past this file rather than arriving unnoticed.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The production sources that may name the warehouse client: write-only
/// sinks into the research warehouse, each insert-only.
const WAREHOUSE_SINKS: [&str; 1] = ["backend/crates/apps/qip-deepbrain/src/warehouse.rs"];

/// Every `.rs` file under each crate's `src/`, outside test modules' own
/// files. Comments are stripped line by line, so prose naming the client is
/// not read as a use of it.
fn production_sources() -> Vec<(String, String)> {
    let root = qip_acceptance::repository_root();
    let mut out = Vec::new();
    let mut pending = vec![root.join("backend/crates")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("crates are readable")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "tests" && name != "target" && name != "benches" && name != "examples" {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") && in_src(&path) {
                let text = std::fs::read_to_string(&path).expect("a source file is readable");
                let code = without_test_modules(&text)
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let relative = path
                    .strip_prefix(&root)
                    .expect("under the repository")
                    .to_string_lossy()
                    .into_owned();
                out.push((relative, code));
            }
        }
    }
    out
}

/// `text` with every `#[cfg(test)] mod … { … }` block removed, braces
/// matched, so production code written after a test module is still read.
/// Truncating at the first test module would let anything appended below it
/// pass unseen.
fn without_test_modules(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("#[cfg(test)]\nmod ") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let Some(open) = after.find('{') else {
            rest = "";
            break;
        };
        let mut depth = 0usize;
        let mut end = after.len();
        for (offset, c) in after[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Whether `code` names `token` as code rather than inside backticks in a
/// message or doc string: an error text telling an operator which type to use
/// is not a use of it.
fn names_in_code(code: &str, token: &str) -> bool {
    code.match_indices(token)
        .any(|(at, _)| !code[..at].ends_with('`') && !code[at + token.len()..].starts_with('`'))
}

fn in_src(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "src")
}

/// Mutations: add `use qip_mesh::bigquery::BigQueryWarehouse;` to any
/// kernel module (the caller is not an allowed sink); or add a
/// `self.warehouse.query(` call to the deep brain's sink (the sink reads the
/// copy back).
#[test]
fn no_decision_path_reads_a_balance_from_a_derived_copy() {
    let sources = production_sources();
    assert!(
        sources.len() > 100,
        "premise: the walk reads the workspace's production sources, found {}",
        sources.len()
    );
    let mesh = PathBuf::from("backend/crates/services/qip-mesh");
    let callers: BTreeSet<String> = sources
        .iter()
        .filter(|(path, _)| !Path::new(path).starts_with(&mesh))
        .filter(|(_, code)| {
            names_in_code(code, "BigQueryWarehouse") || names_in_code(code, "qip_mesh::bigquery")
        })
        .map(|(path, _)| path.clone())
        .collect();
    let allowed: BTreeSet<String> = WAREHOUSE_SINKS.iter().map(|s| s.to_string()).collect();
    assert!(
        !callers.is_empty(),
        "premise: the deep brain's outcome sink names the warehouse client, so the scan sees a use"
    );
    assert_eq!(
        callers, allowed,
        "a production source outside the write-only sinks names the warehouse client; a \
         warehouse is a derived copy and no decision may read from it"
    );
    for sink in WAREHOUSE_SINKS {
        let (_, code) = sources
            .iter()
            .find(|(path, _)| path == sink)
            .expect("the sink is among the sources");
        assert!(
            code.contains(".load("),
            "premise: {sink} writes to the warehouse by batch load (ADR 0240)"
        );
        assert!(
            !code.contains(".query("),
            "{sink} reads the warehouse back; a sink feeds analytics and reads nothing"
        );
    }
    let streams: Vec<&String> = sources
        .iter()
        .filter(|(_, code)| {
            code.contains("change_stream")
                || code.contains("ChangeStream")
                || code.contains("changeStream")
        })
        .map(|(path, _)| path)
        .collect();
    assert!(
        streams.is_empty(),
        "production sources name a change-stream reader: {streams:?}; a change stream feeds \
         analytics and derived caches, never a decision"
    );
}

/// ADR 0240 decision 5: the warehouse is fed by batch load, and no shipped
/// path streams rows. Every streaming write goes through `InsertRow` and
/// BigQuery's `insertAll`, so a production source outside the client that
/// names either is a streaming path. The client keeps `insert` for its own
/// tests; nothing shipped may call it.
///
/// Mutation: restore `OutcomeSink::ship`'s streaming call
/// (`self.warehouse.insert(OUTCOMES_TABLE, rows)` over `InsertRow`s) — the
/// sink names `InsertRow` and this fails.
#[test]
fn no_shipped_path_streams_rows_into_the_warehouse() {
    let sources = production_sources();
    assert!(
        sources.len() > 100,
        "premise: the walk reads the workspace's production sources, found {}",
        sources.len()
    );
    let client = "backend/crates/services/qip-mesh/src/bigquery.rs";
    assert!(
        sources
            .iter()
            .any(|(path, code)| path == client && names_in_code(code, "insertAll")),
        "premise: the client still holds the streaming call this guard keeps out of shipped code"
    );
    let streaming: Vec<&String> = sources
        .iter()
        .filter(|(path, _)| path != client)
        .filter(|(_, code)| names_in_code(code, "InsertRow") || names_in_code(code, "insertAll"))
        .map(|(path, _)| path)
        .collect();
    assert!(
        streaming.is_empty(),
        "shipped sources stream rows into the warehouse; it is fed by batch load from Cloud \
         Storage (ADR 0240): {streaming:?}"
    );
}

/// The warehouse client's own module: the one production source allowed to
/// define the derived copy and to name its REST host.
const WAREHOUSE_CLIENT: &str = "backend/crates/services/qip-mesh/src/bigquery.rs";

/// Names the warehouse client as code.
fn queries_a_derived_copy(code: &str) -> bool {
    names_in_code(code, "BigQueryWarehouse")
        || names_in_code(code, "qip_mesh::bigquery")
        || names_in_code(code, "crate::bigquery")
}

/// Calls the BigQuery REST API: either the absolute URL, or the REST path
/// the shipped client actually sends, which is relative, because it goes
/// through `GcpAccess::request` to the configured plaintext egress proxy
/// and never names the host at all. The skeptic on this guard found the
/// URL form alone matched a call that cannot run while the real call form
/// passed. An operator message naming the host in prose is neither.
fn calls_the_bigquery_host(code: &str) -> bool {
    code.contains("://bigquery.googleapis.com") || code.contains("/bigquery/v2/")
}

/// Reads a Spanner change stream: the Rust names a reader would carry, or
/// the SQL a change-stream query is written in (`SELECT … FROM READ_<stream>`
/// and `CREATE CHANGE STREAM`).
fn reads_a_change_stream(code: &str) -> bool {
    code.contains("change_stream")
        || code.contains("ChangeStream")
        || code.contains("changeStream")
        || code.contains("FROM READ_")
        || code.contains("CHANGE STREAM")
}

/// R12-LEDGER-8138, tightened after its skeptic. The guard above exempted
/// the whole of `qip-mesh`, so a wrapper around `BigQueryWarehouse::query`
/// placed beside the client passed; it saw no raw HTTP to the BigQuery host;
/// and its change-stream check missed `READ_<stream>` SQL. Here every
/// production source is scanned, `qip-mesh` included, and only the client's
/// own module and the one write-only sink may name the warehouse.
///
/// Each detector is first run against a seeded fixture, so a detector that
/// can no longer fire fails here instead of passing every tree.
///
/// Mutations: a `qip-mesh` module calling `BigQueryWarehouse::query`; a
/// kernel source calling `https://bigquery.googleapis.com/...`, or sending
/// the client's own relative `/bigquery/v2/projects/{}/queries` path; a
/// `SELECT * FROM READ_ledger_stream(...)` string in `qip-ledgerd`. Each
/// fails this test.
#[test]
fn no_production_source_outside_the_warehouse_module_and_its_one_insert_only_sink_queries_a_derived_copy_calls_the_bigquery_rest_host_or_reads_a_spanner_change_stream()
 {
    // Premise: every detector fires on what it is meant to refuse and stays
    // quiet on prose and backticked names.
    assert!(queries_a_derived_copy(
        "let rows = qip_mesh::bigquery::BigQueryWarehouse::query(&w, sql)?;"
    ));
    assert!(!queries_a_derived_copy(
        "Err(\"use `BigQueryWarehouse` for analytics\")"
    ));
    assert!(calls_the_bigquery_host(
        "client.get(\"https://bigquery.googleapis.com/bigquery/v2/projects/p/queries\")"
    ));
    assert!(calls_the_bigquery_host(
        "access.request(Method::Post, &format!(\"/bigquery/v2/projects/{}/queries\", p))"
    ));
    assert!(!calls_the_bigquery_host(
        "\"a proxy that forwards to bigquery.googleapis.com is required\""
    ));
    assert!(reads_a_change_stream(
        "\"SELECT ChangeRecord FROM READ_ledger_stream(start_timestamp => @t)\""
    ));
    assert!(reads_a_change_stream(
        "\"CREATE CHANGE STREAM ledger_stream FOR ALL\""
    ));
    assert!(!reads_a_change_stream("const READ_TIMEOUT: u64 = 5;"));

    let sources = production_sources();
    assert!(
        sources.len() > 100,
        "premise: the walk reads the workspace's production sources, found {}",
        sources.len()
    );
    assert!(
        sources.iter().any(|(path, _)| path == WAREHOUSE_CLIENT),
        "premise: the warehouse client's module is among the sources"
    );

    let mut allowed: BTreeSet<String> = WAREHOUSE_SINKS.iter().map(|s| s.to_string()).collect();
    allowed.insert(WAREHOUSE_CLIENT.to_string());
    let callers: BTreeSet<String> = sources
        .iter()
        .filter(|(_, code)| queries_a_derived_copy(code))
        .map(|(path, _)| path.clone())
        .collect();
    let outside: Vec<&String> = callers
        .iter()
        .filter(|path| !allowed.contains(*path))
        .collect();
    assert!(
        outside.is_empty(),
        "production sources outside the warehouse client and its write-only sink name the \
         warehouse: {outside:?}; a warehouse is a derived copy and no decision may read from it"
    );
    for sink in WAREHOUSE_SINKS {
        assert!(
            callers.contains(sink),
            "premise: the sink {sink} names the warehouse client, so the scan sees a use"
        );
        let (_, code) = sources
            .iter()
            .find(|(path, _)| path == sink)
            .expect("the sink is among the sources");
        assert!(
            code.contains(".load("),
            "premise: {sink} writes to the warehouse by its one call, a batch load (ADR 0240)"
        );
        assert!(
            !code.contains(".query("),
            "{sink} reads the warehouse back; a sink feeds analytics and reads nothing"
        );
    }

    let host: Vec<&String> = sources
        .iter()
        .filter(|(path, _)| path != WAREHOUSE_CLIENT)
        .filter(|(_, code)| calls_the_bigquery_host(code))
        .map(|(path, _)| path)
        .collect();
    assert!(
        host.is_empty(),
        "production sources call the BigQuery REST host outside the warehouse client: {host:?}"
    );

    let streams: Vec<&String> = sources
        .iter()
        .filter(|(_, code)| reads_a_change_stream(code))
        .map(|(path, _)| path)
        .collect();
    assert!(
        streams.is_empty(),
        "production sources read a change stream: {streams:?}; a change stream feeds \
         analytics and derived caches, never a decision"
    );
}
