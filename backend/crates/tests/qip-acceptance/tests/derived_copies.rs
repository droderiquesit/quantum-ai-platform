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
            code.contains(".insert("),
            "premise: {sink} writes to the warehouse"
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
