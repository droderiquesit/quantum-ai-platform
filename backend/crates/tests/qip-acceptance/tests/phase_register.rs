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
//!
//! ADR 0258 adds a ratchet: a capability carried ahead of its phase must name
//! a switch that keeps it off, unless it is one of [`UNGATED`], the ten that
//! had none on 2026-10-31. That list may only shrink.

use qip_acceptance::{files_with_extension, read};
use serde_json::Value;

const REGISTER: &str = "docs/architecture/phase-register.json";

/// The capabilities carried ahead of their phase with switch `none` on
/// 2026-10-31, as `(phase, capability)` (ADR 0258). The owner declined to add
/// switches to code that already runs paper, because switching off the
/// survival gate or the cycle to satisfy an ordering clause would remove
/// protections, but ruled that nothing new arrives ungated. So this list may
/// only shrink: a new `none` entry is refused, and an entry that gains a
/// switch, or whose phase is entered, must leave the list, so that a later
/// `none` cannot reuse its place.
const UNGATED: [(u64, &str); 10] = [
    (
        1,
        "Artifact Registry with immutable tags, Binary Authorization attestation and the reproducibility check in deploy.yml",
    ),
    (
        6,
        "Cloud Run warm services (qip-api, fastbrain, deepbrain, portal)",
    ),
    (
        8,
        "The eight-stage cycle with agents, world model, reasoning and learning in qip-kernel",
    ),
    (
        9,
        "agent-guard, agent-lanes, the sandbox workflow and the model gateway",
    ),
    (
        10,
        "QAOA against SimulatedProvider with a classical baseline every time (ADR 0006); qip-agency",
    ),
    (11, "NOW lattice and forecast lattice in qip-kernel"),
    (12, "model tournament and verifier society"),
    (14, "solver routing in qip-kernel"),
    (15, "survival gate, hedge and capital calls in qip-kernel"),
    (16, "ambient intake and specialist spawning"),
];

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
    let mut ungated_seen = Vec::new();
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
                if ahead["switch"].as_str() == Some("none") {
                    let capability = ahead["capability"].as_str().unwrap_or("").to_string();
                    let key = (index as u64, capability);
                    if UNGATED.iter().any(|(p, c)| *p == key.0 && *c == key.1) {
                        ungated_seen.push(key);
                    } else {
                        refused.push(format!(
                            "phase {index} carries {:?} ahead of phase with switch none; \
                             a capability added ahead of its phase must name a switch that keeps it off (ADR 0258)",
                            key.1
                        ));
                    }
                }
            }
        }
        previous_exited = state == "exited";
    }
    for (phase, capability) in UNGATED {
        if !ungated_seen
            .iter()
            .any(|(p, c)| *p == phase && c == capability)
        {
            refused.push(format!(
                "UNGATED still lists phase {phase}'s {capability:?}, which the register no longer \
                 carries ahead of phase with switch none; remove it, the list may only shrink (ADR 0258)"
            ));
        }
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

/// ADR 0258: an eleventh capability ahead of its phase with no switch is
/// refused. The failure this prevents is the one the owner named: a new
/// capability arriving ungated beside the ten that were allowed to stay.
#[test]
fn a_capability_added_ahead_of_its_phase_with_no_switch_is_refused() {
    let committed = phases();
    // Premise: the committed register carries exactly the ten, so the
    // ratchet is not vacuous and the list is not wider than the register.
    let ungated: Vec<(u64, String)> = committed
        .iter()
        .filter(|p| p["state"] == "not_entered")
        .flat_map(|p| {
            let number = p["phase"].as_u64().unwrap_or(u64::MAX);
            p["ahead_of_phase"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|a| a["switch"] == "none")
                .map(move |a| (number, a["capability"].as_str().unwrap_or("").to_string()))
        })
        .collect();
    assert_eq!(ungated.len(), UNGATED.len(), "{ungated:#?}");

    let mut mutated = committed;
    let added = serde_json::json!({
        "capability": "a new capability nobody gated",
        "reason": "arrived ahead of phase",
        "switch": "none",
    });
    mutated[2]["ahead_of_phase"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("premise: phase 2 has an ahead_of_phase array"))
        .push(added);
    let refused = refusals(&mutated, &committed_source);
    assert!(
        refused.iter().any(|r| r.starts_with(
            "phase 2 carries \"a new capability nobody gated\" ahead of phase with switch none;"
        )),
        "{refused:?}"
    );
    // One of the ten copied to another phase is a new entry, not the old one.
    let mut moved = phases();
    let entry = moved[1]["ahead_of_phase"][0].clone();
    assert_eq!(
        entry["switch"], "none",
        "premise: phase 1's first entry is ungated"
    );
    moved[2]["ahead_of_phase"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("premise: phase 2 has an ahead_of_phase array"))
        .push(entry);
    let refused = refusals(&moved, &committed_source);
    assert!(
        refused
            .iter()
            .any(|r| r.starts_with("phase 2 carries \"Artifact Registry")),
        "{refused:?}"
    );
}

/// ADR 0258: one of the ten that gains a switch must leave the list. The
/// failure this prevents is a ratchet that never shrinks: a slot freed by a
/// capability that gained a switch, left in the list, would admit a later
/// ungated capability under the same name.
#[test]
fn an_ungated_capability_that_gains_a_switch_must_leave_the_list() {
    let mut mutated = phases();
    assert_eq!(
        mutated[1]["ahead_of_phase"][0]["switch"], "none",
        "premise: phase 1's first entry is one of the ten"
    );
    mutated[1]["ahead_of_phase"][0]["switch"] = Value::from("gitops_enabled");
    let refused = refusals(&mutated, &committed_source);
    assert!(
        refused
            .iter()
            .any(|r| r.starts_with("UNGATED still lists phase 1's \"Artifact Registry")),
        "{refused:?}"
    );
}
