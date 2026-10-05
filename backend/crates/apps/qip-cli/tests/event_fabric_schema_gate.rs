//! `qip event-fabric schema-gate`: the CI compatibility gate (CICD-070).
//!
//! The schema lock fails when a body's shape and its committed row disagree.
//! Regenerating the row makes it pass again whatever the change was, so the
//! lock alone cannot say whether a change was one consumers survive. These
//! tests pin the judgement that can: a base lock and a head lock, handed to
//! the command the pipeline runs, through the family dispatcher the binary
//! routes to.
//!
//! Every lock here is written to a scratch file and read back by the command,
//! because the command's contract is two paths and an exit code.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_cli::event_fabric::schema_gate::{COMPATIBLE, INCOMPATIBLE, SUBCOMMAND};
use qip_cli::event_fabric::{Environment, Outcome, dispatch};
use qip_core::error::Result;
use qip_events::event_fabric::schema_id::{SchemaId, Shape};
use serde_json::{Value, json};
use std::path::PathBuf;

/// One lock row for `topic` at `version`, shaped like `sample`, with the id
/// that shape really hashes to — what regenerating the lock would commit.
fn row(topic: &str, version: u32, sample: &Value) -> Value {
    let shape = Shape::from_json(sample);
    json!({
        "topic": topic,
        "version": version,
        "type_name": "fixture::Body",
        "schema_id": SchemaId::new(topic, version, &shape).as_str(),
        "shape": serde_json::to_value(&shape).unwrap(),
    })
}

fn write_lock(name: &str, rows: &[Value]) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "qip-schema-gate-{}-{name}.lock.json",
        std::process::id()
    ));
    std::fs::write(&path, serde_json::to_string_pretty(rows).unwrap()).unwrap();
    path
}

/// The command as the pipeline runs it: `schema-gate --base <file> --head <file>`.
fn gate(name: &str, base: &[Value], head: &[Value]) -> Result<Outcome> {
    let base = write_lock(&format!("{name}-base"), base);
    let head = write_lock(&format!("{name}-head"), head);
    let arguments: Vec<String> = [
        SUBCOMMAND,
        "--base",
        base.to_str().unwrap(),
        "--head",
        head.to_str().unwrap(),
    ]
    .iter()
    .map(|part| part.to_string())
    .collect();
    let outcome = dispatch(&arguments, &Environment::process());
    let _ = std::fs::remove_file(base);
    let _ = std::fs::remove_file(head);
    outcome
}

fn order() -> Value {
    json!({"id": "o-1", "quantity": 3, "venue": {"name": "sim", "lot": 1}})
}

/// The failure this prevents: a field consumers read is removed, the lock
/// row is regenerated in place at the same version, and CI is green because
/// the lock agrees with the tree again.
///
/// Mutation: make `judge` treat a changed id at the same version as
/// admitted without calling `check_compatible`. The removal then exits 0.
#[test]
fn a_field_removed_at_the_same_version_fails_the_gate_and_a_field_added_passes_it() {
    let base = [row("reflex.outcome", 1, &order())];

    // Premise: an unchanged lock passes and reports that it compared a row,
    // so the verdicts below are about the change and not about the fixture.
    let unchanged = gate("unchanged", &base, &base).unwrap();
    assert_eq!(unchanged.code, COMPATIBLE);
    assert_eq!(
        unchanged.lines,
        vec![
            "schema gate: COMPATIBLE — 1 base topic(s) compared, 0 changed and still readable"
                .to_string()
        ]
    );

    let mut wider = order();
    wider["note"] = json!("added");
    let added = [row("reflex.outcome", 1, &wider)];
    // Premise: adding a field moved the id, so the gate is judging a real
    // change by its shape and not passing it for looking identical.
    assert_ne!(base[0]["schema_id"], added[0]["schema_id"]);
    let outcome = gate("added", &base, &added).unwrap();
    assert_eq!(outcome.code, COMPATIBLE, "{:?}", outcome.lines);
    assert!(
        outcome.lines[0].starts_with("reflex.outcome: version 1 gained fields only"),
        "{:?}",
        outcome.lines
    );

    let mut narrower = order();
    narrower.as_object_mut().unwrap().remove("quantity");
    let removed = [row("reflex.outcome", 1, &narrower)];
    let outcome = gate("removed", &base, &removed).unwrap();
    assert_eq!(outcome.code, INCOMPATIBLE, "{:?}", outcome.lines);
    assert!(
        outcome.lines[0].contains("reflex.outcome")
            && outcome.lines[0].contains("field 'quantity' is absent from the new shape"),
        "the finding does not name the topic and the field that went: {:?}",
        outcome.lines
    );
    assert!(
        outcome.lines[1].starts_with("schema gate: INCOMPATIBLE — 1 of 1 base topic(s)"),
        "{:?}",
        outcome.lines
    );
}

/// The failure this prevents: a nested field changes kind under an unchanged
/// list of field names — the case the registry's top-level fingerprint
/// cannot see — and every consumer reading it as a number misparses it.
///
/// Mutation: the same one as the test above — admit a changed id without
/// calling `check_compatible`. The retype then exits 0. The test is separate
/// because a gate rewritten to compare field names would pass the one above
/// and fail only here.
#[test]
fn a_nested_field_that_changed_kind_fails_the_gate_though_no_field_name_moved() {
    let base = [row("reflex.outcome", 1, &order())];
    let mut retyped = order();
    retyped["venue"]["lot"] = json!("one");
    let head = [row("reflex.outcome", 1, &retyped)];

    // Premise: the two shapes name exactly the same fields at the top level.
    let keys =
        |value: &Value| -> Vec<String> { value.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&order()), keys(&retyped));

    let outcome = gate("retyped", &base, &head).unwrap();
    assert_eq!(outcome.code, INCOMPATIBLE, "{:?}", outcome.lines);
    assert!(
        outcome.lines[0].contains("a field changed kind without a version bump"),
        "{:?}",
        outcome.lines
    );
}

/// The failure this prevents: three ways to lose a consumer that are not a
/// changed shape — the version going backwards, the topic's row being
/// deleted, and the gate refusing the one change that is sanctioned.
///
/// Mutation: drop the `new.version < old.version` arm. The rollback exits 0.
#[test]
fn a_version_bump_admits_a_break_and_a_rollback_or_a_deleted_row_does_not() {
    let mut narrower = order();
    narrower.as_object_mut().unwrap().remove("quantity");

    // The same removal that fails at version 1 passes at version 2: ADR 0100
    // §5's deliberate break.
    let base = [row("reflex.outcome", 1, &order())];
    assert_eq!(
        gate(
            "same-version",
            &base,
            &[row("reflex.outcome", 1, &narrower)]
        )
        .unwrap()
        .code,
        INCOMPATIBLE,
        "premise: the removal is incompatible when the version does not move"
    );
    let bumped = gate("bumped", &base, &[row("reflex.outcome", 2, &narrower)]).unwrap();
    assert_eq!(bumped.code, COMPATIBLE, "{:?}", bumped.lines);
    assert!(
        bumped.lines[0].starts_with("reflex.outcome: version 1 -> 2"),
        "{:?}",
        bumped.lines
    );

    // Backwards, with an identical shape: still refused.
    let at_two = [row("reflex.outcome", 2, &order())];
    let rolled_back = gate("rollback", &at_two, &[row("reflex.outcome", 1, &order())]).unwrap();
    assert_eq!(rolled_back.code, INCOMPATIBLE, "{:?}", rolled_back.lines);
    assert!(
        rolled_back.lines[0].contains("rolls version 2 back to 1"),
        "{:?}",
        rolled_back.lines
    );

    // The row deleted, another topic left in its place so the head is not
    // simply empty.
    let other = [row("reflex.pass_marked", 1, &json!({"pass": 1}))];
    let deleted = gate("deleted", &base, &other).unwrap();
    assert_eq!(deleted.code, INCOMPATIBLE, "{:?}", deleted.lines);
    assert!(
        deleted.lines[0].contains("reflex.outcome: locked at version 1 on the base and absent"),
        "{:?}",
        deleted.lines
    );

    // A topic the head adds has no consumer at the base to break.
    let both = [base[0].clone(), other[0].clone()];
    let grown = gate("grown", &base, &both).unwrap();
    assert_eq!(grown.code, COMPATIBLE, "{:?}", grown.lines);
}

/// The failure this prevents: the lock on the base branch was written before
/// rows carried shapes. Comparing a changed id against no shape, the gate
/// must not guess "additive" — that guess is how a removal merges during the
/// one change that introduces the gate.
///
/// Mutation: push the missing-shape case onto `admitted` instead of
/// `findings`.
#[test]
fn a_changed_id_with_no_shape_to_compare_is_refused_rather_than_assumed_additive() {
    let mut shapeless = row("reflex.outcome", 1, &order());
    shapeless.as_object_mut().unwrap().remove("shape");

    // Premise: with the id unchanged a shapeless base row is fine — this is
    // the transition commit itself, where the head only adds shapes.
    let same = gate(
        "transition",
        &[shapeless.clone()],
        &[row("reflex.outcome", 1, &order())],
    )
    .unwrap();
    assert_eq!(same.code, COMPATIBLE, "{:?}", same.lines);

    let mut wider = order();
    wider["note"] = json!("added");
    let outcome = gate(
        "shapeless",
        &[shapeless],
        &[row("reflex.outcome", 1, &wider)],
    )
    .unwrap();
    assert_eq!(outcome.code, INCOMPATIBLE, "{:?}", outcome.lines);
    assert!(
        outcome.lines[0].contains("carries no shape to compare. Bump the schema version"),
        "{:?}",
        outcome.lines
    );
}

/// The failure this prevents: a script that cannot tell "the gate could not
/// look" from either verdict. A lock that is missing, malformed, or edited
/// by hand so its shape no longer hashes to its id is an error — exit 1 from
/// the binary — never COMPATIBLE and never INCOMPATIBLE.
///
/// Mutation: delete the id check in `parse`. The hand-edited lock then
/// returns a verdict.
#[test]
fn a_lock_that_cannot_be_trusted_or_read_is_a_refusal_and_never_a_verdict() {
    let honest = [row("reflex.outcome", 1, &order())];
    // Premise: the honest row is accepted, so the refusals below are about
    // what was done to it.
    assert!(gate("honest", &honest, &honest).is_ok());

    // A shape edited after the id was computed: the row claims an id for a
    // shape nothing ever published under it.
    let mut edited = honest[0].clone();
    let mut narrower = order();
    narrower.as_object_mut().unwrap().remove("quantity");
    edited["shape"] = serde_json::to_value(Shape::from_json(&narrower)).unwrap();
    let refusal = gate("edited", &honest, &[edited]).unwrap_err().to_string();
    assert!(
        refusal.contains("carries a shape that does not hash to its schema_id"),
        "{refusal}"
    );

    let not_a_lock = write_lock("not-a-lock", &[json!({"topic": 7})]);
    let arguments = |base: &str, head: &str| -> Vec<String> {
        [SUBCOMMAND, "--base", base, "--head", head]
            .iter()
            .map(|part| part.to_string())
            .collect()
    };
    let path = not_a_lock.to_str().unwrap();
    let refusal = dispatch(&arguments(path, path), &Environment::process())
        .unwrap_err()
        .to_string();
    assert!(refusal.contains("is not a schema lock"), "{refusal}");
    let _ = std::fs::remove_file(&not_a_lock);

    let refusal = dispatch(
        &arguments("/nonexistent/base.lock.json", "/nonexistent/head.lock.json"),
        &Environment::process(),
    )
    .unwrap_err()
    .to_string();
    assert!(refusal.contains("cannot read the schema lock"), "{refusal}");

    // One lock is not a comparison.
    let only_base: Vec<String> = [SUBCOMMAND, "--base", "x"]
        .iter()
        .map(|p| p.to_string())
        .collect();
    let refusal = dispatch(&only_base, &Environment::process())
        .unwrap_err()
        .to_string();
    assert!(refusal.contains("both locks are required"), "{refusal}");
}
