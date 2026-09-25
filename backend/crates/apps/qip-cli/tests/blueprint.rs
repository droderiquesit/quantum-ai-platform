//! The blueprint registers are views of their JSON sources, and nothing else.
//!
//! These tests hold the properties that make the traceability matrix a
//! register rather than a document. Every requirement gets exactly one row,
//! including before anyone has assessed it. COMPLETE counts toward
//! completion only when the row says COMPLETE. A view edited by hand is
//! caught, not silently kept. A malformed source is refused, not skipped.

use qip_cli::blueprint::{load, render_views, stale, write};
use std::path::{Path, PathBuf};

/// A throwaway repository root holding just the blueprint sources.
struct Root(PathBuf);

impl Root {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("qip-blueprint-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        for directory in [
            "docs/blueprint/requirements",
            "docs/blueprint/assessment",
            "docs/architecture",
        ] {
            std::fs::create_dir_all(path.join(directory)).expect("create fixture directories");
        }
        Root(path)
    }

    fn put(&self, relative: &str, contents: &str) {
        std::fs::write(self.0.join(relative), contents).expect("write fixture file");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn requirement(id: &str, priority: &str) -> String {
    format!(
        r#"{{"id":"{id}","domain":"LEDGER","title":"title of {id}","statement":"statement of {id}","kind":"invariant","priority":"{priority}","sources":[{{"doc":"M","page":"23","section":"§15"}}],"verification":{{"method":"unit","check":"x"}},"policy_flags":[]}}"#
    )
}

fn three_requirements(root: &Root) {
    root.put(
        "docs/blueprint/requirements/LEDGER.json",
        &format!(
            "[{},{},{}]",
            requirement("LEDGER-001", "P0"),
            requirement("LEDGER-002", "P1"),
            requirement("LEDGER-003", "P2")
        ),
    );
}

fn matrix(root: &Root) -> String {
    let sources = load(root.path()).expect("sources load");
    render_views(&sources)
        .into_iter()
        .find(|view| view.path.ends_with("traceability-matrix.md"))
        .expect("the matrix is always rendered")
        .text
}

fn rows_for(matrix: &str, id: &str) -> usize {
    // Match the id as the whole first cell, so LEDGER-001 does not also count
    // a row for LEDGER-0010.
    matrix
        .lines()
        .filter(|line| line.starts_with(&format!("| {id} |")))
        .count()
}

#[test]
fn every_requirement_gets_exactly_one_matrix_row_before_anyone_has_assessed_it() {
    // An unassessed requirement with no row is invisible, and invisible
    // requirements are how a register overstates completion: the
    // denominator shrinks instead of the numerator growing.
    let root = Root::new("unassessed");
    three_requirements(&root);
    let matrix = matrix(&root);

    assert!(
        matrix.contains("**3 requirements; 3 applicable**"),
        "{matrix}"
    );
    for id in ["LEDGER-001", "LEDGER-002", "LEDGER-003"] {
        assert_eq!(rows_for(&matrix, id), 1, "{id} must have exactly one row");
    }
    assert!(
        matrix.contains("| UNASSESSED | 3 |"),
        "every row reads UNASSESSED: {matrix}"
    );
    assert!(
        matrix.contains("| Blueprint completion (COMPLETE) | 0 | 0.0% |"),
        "{matrix}"
    );
}

#[test]
fn completion_counts_only_rows_whose_status_is_complete_not_rows_that_merely_claim_tests() {
    // The premise first: one COMPLETE row and one PARTIAL row whose flags are
    // all true. A register that scored completion from "tested" would count
    // both.
    let root = Root::new("complete");
    three_requirements(&root);
    root.put(
        "docs/blueprint/assessment/LEDGER.json",
        r#"[{"id":"LEDGER-001","status":"COMPLETE","implemented":true,"tested":true,"integrated":true},
            {"id":"LEDGER-002","status":"PARTIAL","implemented":true,"tested":true,"integrated":true}]"#,
    );
    let matrix = matrix(&root);

    assert!(
        matrix.contains("| Tested (a named test demonstrates it) | 2 | 66.7% |"),
        "{matrix}"
    );
    assert!(
        matrix.contains("| Blueprint completion (COMPLETE) | 1 | 33.3% |"),
        "only the COMPLETE row counts toward completion: {matrix}"
    );
    assert!(matrix.contains("| UNASSESSED | 1 |"), "{matrix}");
}

#[test]
fn an_obsolete_requirement_leaves_the_denominator_and_nothing_else_does() {
    let root = Root::new("obsolete");
    three_requirements(&root);
    root.put(
        "docs/blueprint/assessment/LEDGER.json",
        r#"[{"id":"LEDGER-001","status":"COMPLETE"},{"id":"LEDGER-002","status":"OBSOLETE"},{"id":"LEDGER-003","status":"BLOCKED"}]"#,
    );
    let matrix = matrix(&root);

    // BLOCKED stays in the denominator on purpose: a refused requirement is
    // still a requirement the platform does not meet.
    assert!(
        matrix.contains("**3 requirements; 2 applicable**"),
        "{matrix}"
    );
    assert!(
        matrix.contains("| Blueprint completion (COMPLETE) | 1 | 50.0% |"),
        "{matrix}"
    );
    assert_eq!(
        rows_for(&matrix, "LEDGER-002"),
        1,
        "an obsolete requirement keeps its row"
    );
}

#[test]
fn a_view_edited_by_hand_is_reported_stale_and_a_render_restores_it() {
    let root = Root::new("stale");
    three_requirements(&root);
    let views = render_views(&load(root.path()).expect("sources load"));
    let written = write(root.path(), &views).expect("views write");
    assert_eq!(
        written.len(),
        views.len(),
        "a fresh root has every view stale"
    );
    assert!(
        stale(root.path(), &views).is_empty(),
        "nothing is stale right after a render"
    );

    let matrix_path = root.path().join("docs/blueprint/traceability-matrix.md");
    let edited = std::fs::read_to_string(&matrix_path)
        .expect("read matrix")
        .replace("| UNASSESSED | 3 |", "| COMPLETE | 3 |");
    std::fs::write(&matrix_path, edited).expect("hand-edit the matrix");

    assert_eq!(
        stale(root.path(), &views),
        vec!["docs/blueprint/traceability-matrix.md"]
    );
    write(root.path(), &views).expect("re-render");
    assert!(stale(root.path(), &views).is_empty());
}

#[test]
fn a_source_that_is_not_an_array_is_refused_rather_than_skipped() {
    // A skipped file would render a catalogue with a domain missing, and the
    // matrix would report a smaller blueprint than the one adopted.
    let root = Root::new("malformed");
    three_requirements(&root);
    root.put(
        "docs/blueprint/requirements/RISK.json",
        r#"{"id":"RISK-001"}"#,
    );
    let error = load(root.path()).expect_err("a non-array source must be refused");
    assert!(
        error.message().contains("RISK.json"),
        "the refusal names the file: {}",
        error.message()
    );
}
