//! The blueprint registers are views of their JSON sources, and nothing else.
//!
//! These tests hold the properties that make the traceability matrix a
//! register rather than a document. Every requirement gets exactly one row,
//! including before anyone has assessed it. COMPLETE counts toward
//! completion only when the row says COMPLETE. A view edited by hand is
//! caught, not silently kept. A malformed source is refused, not skipped.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

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

// --- ARCH-074: the v12.0 completeness targets --------------------------------

const TARGETS: &str = "docs/blueprint/v12-completeness-targets.json";

/// The rendered targets view's rows, as `(cited cell, scored cell)` in order.
fn target_rows(root: &Root) -> Vec<(String, String)> {
    let sources = load(root.path()).expect("sources load");
    let view = render_views(&sources)
        .into_iter()
        .find(|view| view.path.ends_with("v12-completeness-targets.md"))
        .expect("a targets list renders a targets view")
        .text;
    view.lines()
        .filter(|line| line.starts_with("| ") && !line.starts_with("| # "))
        .map(|line| {
            let cells: Vec<&str> = line.split(" | ").collect();
            (cells[2].to_string(), cells[3].to_string())
        })
        .collect()
}

#[test]
fn a_completeness_target_is_scored_complete_only_when_every_requirement_it_cites_is_complete_and_tested()
 {
    // The failure this prevents: a target reading COMPLETE because somebody
    // typed it, or because one of the rows it rests on was marked complete
    // with no test under it. A target has no status field at all; what it is
    // scored from is below, and each of the three ways to fall short is here.
    let root = Root::new("targets");
    three_requirements(&root);
    root.put(
        "docs/blueprint/assessment/LEDGER.json",
        r#"[{"id":"LEDGER-001","status":"COMPLETE","tested":true},
            {"id":"LEDGER-002","status":"COMPLETE","tested":false},
            {"id":"LEDGER-003","status":"PARTIAL","tested":true}]"#,
    );
    root.put(
        TARGETS,
        r#"[{"target":"rests on evidence","requirements":["LEDGER-001"]},
            {"target":"rests on a row nobody tested","requirements":["LEDGER-001","LEDGER-002"]},
            {"target":"rests on a row that is not finished","requirements":["LEDGER-001","LEDGER-003"]}]"#,
    );
    let rows = target_rows(&root);
    // Premise: three targets rendered, each citing the status it was given.
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0].0, "LEDGER-001: COMPLETE");
    assert_eq!(
        rows[1].0,
        "LEDGER-001: COMPLETE; LEDGER-002: COMPLETE (untested)"
    );
    assert_eq!(rows[2].0, "LEDGER-001: COMPLETE; LEDGER-003: PARTIAL");

    assert_eq!(rows[0].1, "COMPLETE");
    assert_eq!(
        rows[1].1, "OPEN (1 of 2 complete)",
        "a row marked complete with no test under it counted toward a target"
    );
    assert_eq!(rows[2].1, "OPEN (1 of 2 complete)");
}

#[test]
fn a_completeness_target_that_cites_nothing_or_a_requirement_nobody_holds_is_refused_rather_than_rendered()
 {
    // A row with no evidence under it is a target scored on its own text.
    let root = Root::new("targets-refused");
    three_requirements(&root);
    root.put(
        TARGETS,
        r#"[{"target":"cites a ghost","requirements":["LEDGER-009"]}]"#,
    );
    let error = load(root.path()).expect_err("an unknown requirement id must be refused");
    assert!(
        error.message().contains("LEDGER-009"),
        "the refusal names the id: {}",
        error.message()
    );

    root.put(TARGETS, r#"[{"target":"cites nothing","requirements":[]}]"#);
    assert!(
        load(root.path()).is_err(),
        "a target citing no requirement was accepted"
    );

    // And the admitting half: the same list citing a real id loads.
    root.put(
        TARGETS,
        r#"[{"target":"cites a row","requirements":["LEDGER-001"]}]"#,
    );
    assert_eq!(
        target_rows(&root),
        [(
            "LEDGER-001: UNASSESSED".to_string(),
            "OPEN (0 of 1 complete)".to_string()
        )]
    );
}
