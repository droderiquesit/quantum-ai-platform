//! ADR 0143: phases are gated by a committed register whose exit evidence
//! this suite checks (R12-ARCH-7134).
//!
//! The tree carries every phase at once, and nothing recorded which phase a
//! capability belonged to or whether its predecessor's exit had been shown.
//! `docs/architecture/phase-register.json` is that record; this suite refuses
//! a register that claims more than the tree shows: a phase entered over an
//! unexited predecessor, an exited phase with no exit tests, an exit test that
//! no longer exists under its name, a phase that is not the blueprint's, and
//! a capability carried ahead of its phase with no stated switch.

use qip_acceptance::{files_with_extension, read};
use serde_json::Value;

const REGISTER: &str = "docs/architecture/phase-register.json";

fn phases() -> Vec<Value> {
    let register: Value = serde_json::from_str(&read(REGISTER))
        .unwrap_or_else(|error| panic!("{REGISTER} is not JSON: {error}"));
    register["phases"]
        .as_array()
        .unwrap_or_else(|| panic!("{REGISTER} has no phases array"))
        .clone()
}

fn text<'a>(phase: &'a Value, key: &str) -> &'a str {
    phase[key]
        .as_str()
        .unwrap_or_else(|| panic!("{REGISTER}: phase {} has no {key}", phase["phase"]))
}

/// Why each register is refused, or nothing when it is admitted.
fn refusals(phases: &[Value], test_source: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut refused = Vec::new();
    let mut previous_exited = true;
    for (index, phase) in phases.iter().enumerate() {
        let number = phase["phase"].as_u64();
        if number != Some(index as u64) {
            refused.push(format!(
                "entry {index} is phase {number:?}; phases must run 0, 1, 2 ... with none missing"
            ));
        }
        let state = phase["state"].as_str().unwrap_or("");
        if !matches!(state, "not_entered" | "entered" | "exited") {
            refused.push(format!("phase {index} has state {state:?}"));
        }
        if state != "not_entered" && !previous_exited {
            refused.push(format!(
                "phase {index} is {state} while phase {} has not exited",
                index.saturating_sub(1)
            ));
        }
        let tests = phase["exit_tests"].as_array().cloned().unwrap_or_default();
        if state == "exited" && tests.is_empty() {
            refused.push(format!(
                "phase {index} is exited with no test demonstrating its exit"
            ));
        }
        for test in &tests {
            let file = test["file"].as_str().unwrap_or("");
            let name = test["test"].as_str().unwrap_or("");
            let found = test_source(file).is_some_and(|source| defines_test(&source, name));
            if !found {
                refused.push(format!(
                    "phase {index} names exit test {file}::{name}, which does not exist as a #[test]"
                ));
            }
        }
        if state == "not_entered" {
            for ahead in phase["ahead_of_phase"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                for key in ["capability", "reason", "switch"] {
                    if ahead[key].as_str().is_none_or(|v| v.trim().is_empty()) {
                        refused.push(format!(
                            "phase {index} carries a capability ahead of phase with no {key}"
                        ));
                    }
                }
            }
        }
        previous_exited = state == "exited";
    }
    refused
}

/// Whether `source` defines `name` as a test: a line `fn name(` (the name
/// delimited, so `fn a_b(` is not found by `a`) with `#[test]` among the few
/// attribute lines above it.
fn defines_test(source: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let lines: Vec<&str> = source.lines().collect();
    let head = format!("fn {name}(");
    lines.iter().enumerate().any(|(i, line)| {
        line.trim_start().starts_with(&head)
            && lines[i.saturating_sub(4)..i]
                .iter()
                .any(|above| above.trim() == "#[test]")
    })
}

fn committed_source(file: &str) -> Option<String> {
    let path = qip_acceptance::repository_root().join(file);
    std::fs::read_to_string(path).ok()
}

#[test]
fn the_committed_phase_register_is_admitted() {
    let phases = phases();
    // Premise: the register is not empty, so "nothing refused" is not
    // vacuous, and phase 0 is entered, so its exit tests are checked.
    assert!(
        phases.len() >= 10,
        "premise: the register lists the blueprint's phases"
    );
    assert_ne!(
        text(&phases[0], "state"),
        "not_entered",
        "premise: phase 0 is entered"
    );
    let refused = refusals(&phases, &committed_source);
    assert!(refused.is_empty(), "{REGISTER} is refused: {refused:#?}");
}

#[test]
fn every_phase_is_the_blueprints_own_build_phase() {
    // A register drifting from the blueprint would gate phases nobody asked
    // for. Each row must be the requirement that names this phase's number.
    let requirements = read("docs/blueprint/requirements.md");
    for phase in phases() {
        let row = text(&phase, "row");
        let number = phase["phase"].as_u64().unwrap_or(u64::MAX);
        let line = requirements
            .lines()
            .find(|line| line.starts_with(&format!("| {row} |")))
            .unwrap_or_else(|| panic!("{REGISTER}: {row} is not a requirement"));
        assert!(
            line.contains(&format!("**Build phase {number}: ")),
            "{REGISTER}: {row} is not the requirement for build phase {number}"
        );
    }
}

#[test]
fn every_named_switch_exists_in_the_tree() {
    // A switch the tree does not contain keeps nothing off. Each named one
    // must be a variable the Terraform root declares or one a composition root reads.
    let variables: String = files_with_extension("infrastructure/terraform", "tf")
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect();
    let apps: String = files_with_extension("backend/crates/apps", "rs")
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect();
    let mut named = 0;
    for phase in phases() {
        for ahead in phase["ahead_of_phase"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let switch = ahead["switch"].as_str().unwrap_or("");
            if switch == "none" {
                continue;
            }
            named += 1;
            let declared = variables.contains(&format!("variable \"{switch}\""))
                || apps.contains(&format!("\"{switch}\""));
            assert!(
                declared,
                "{REGISTER}: switch {switch} is neither a Terraform variable nor read by an app"
            );
        }
    }
    assert!(named > 0, "premise: at least one switch is named");
}

#[test]
fn a_phase_entered_over_an_unexited_predecessor_is_refused() {
    let mut mutated = phases();
    mutated[1]["state"] = Value::from("entered");
    let refused = refusals(&mutated, &committed_source);
    assert!(
        refused
            .iter()
            .any(|r| r == "phase 1 is entered while phase 0 has not exited"),
        "{refused:?}"
    );
}

#[test]
fn an_exited_phase_with_no_exit_test_or_a_renamed_one_is_refused() {
    let mut empty = phases();
    empty[0]["state"] = Value::from("exited");
    empty[0]["exit_tests"] = Value::Array(Vec::new());
    assert!(
        refusals(&empty, &committed_source)
            .iter()
            .any(|r| r == "phase 0 is exited with no test demonstrating its exit")
    );

    // A prefix of a real test name is not that test.
    let mut renamed = phases();
    let real = renamed[0]["exit_tests"][0]["test"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!real.is_empty(), "premise: phase 0 names an exit test");
    let prefix = &real[..real.len() - 3];
    renamed[0]["exit_tests"][0]["test"] = Value::from(prefix);
    let refused = refusals(&renamed, &committed_source);
    assert!(
        refused
            .iter()
            .any(|r| r.ends_with("which does not exist as a #[test]")),
        "{refused:?}"
    );
}
