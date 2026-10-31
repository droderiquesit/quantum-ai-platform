//! The scheduled drift plan (R12-CICD-8032, R12-GCP-9086).
//!
//! `infra.yml` plans only when a person dispatches it, so nothing compared
//! dev with main between dispatches, and a change made by hand stayed
//! invisible until somebody read it as their own. `drift.yml` plans dev once
//! a day and fails on a non-empty plan. These tests pin what makes it a check
//! and not a second path to change dev: it runs on a schedule, it can only
//! plan, it plans dev and nothing else, no agent starts it, and its verdict
//! step turns Terraform's exit 2 into a failure. The last is driven under
//! bash with `terraform` stubbed, because a test matching the spelling of a
//! `case` arm pins the spelling and not what the arm does.

// The workspace denies `panic_in_result_fn` for production code. In a test
// the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::read;
use std::process::Command;

const DRIFT: &str = ".github/workflows/drift.yml";
const POLICY: &str = "scripts/agent-policy.json";

/// The workflow with its comment lines removed.
fn code() -> String {
    read(DRIFT)
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `run: |` body of the step with this name, dedented to its own margin.
fn run_body(step: &str) -> String {
    let text = read(DRIFT);
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim() != format!("- name: {step}"));
    assert!(lines.next().is_some(), "{DRIFT} has no step named `{step}`");
    let mut lines = lines.skip_while(|line| line.trim() != "run: |");
    let opening = lines
        .next()
        .unwrap_or_else(|| panic!("`{step}` has no `run: |` body"));
    let margin = opening.len() - opening.trim_start().len() + 2;
    lines
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() >= margin)
        .map(|line| line.get(margin..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn drift_is_planned_on_a_schedule_and_on_demand() {
    let code = code();
    let triggers: Vec<&str> = code
        .lines()
        .skip_while(|line| *line != "on:")
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with(' '))
        .collect();
    assert!(
        triggers.iter().any(|line| line.trim() == "schedule:"),
        "{DRIFT} has no schedule, so it compares dev with main only when someone remembers to"
    );
    assert!(
        triggers
            .iter()
            .any(|line| line.trim().starts_with("- cron: '")),
        "{DRIFT}'s schedule names no cron"
    );
    // A push or pull_request trigger would plan on every merge, with the
    // infra identity, for code nobody asked to compare.
    for refused in [
        "push:",
        "pull_request:",
        "pull_request_target:",
        "workflow_run:",
    ] {
        assert!(
            !triggers.iter().any(|line| line.trim() == refused),
            "{DRIFT} runs on `{refused}`; a drift check runs on a clock, not on a change"
        );
    }
}

#[test]
fn the_drift_workflow_can_only_plan() {
    let code = code();
    // Premise: it does plan, with the exit code that distinguishes drift.
    assert!(
        code.contains("plan -input=false") && code.contains("-detailed-exitcode"),
        "{DRIFT} does not run `terraform plan -detailed-exitcode`; it cannot tell a clean plan \
         from a drifted one"
    );
    assert!(
        code.contains("-lock=false"),
        "{DRIFT} plans with the state lock, so a daily read could hold the lock an apply needs"
    );
    for verb in [
        " apply",
        " destroy",
        " import",
        " state ",
        " taint",
        " untaint",
        "force-unlock",
        " -out",
        "gcloud ",
        "kubectl ",
    ] {
        assert!(
            !code
                .lines()
                .filter(|line| line.contains("terraform") || line.trim_start().starts_with('-'))
                .any(|line| line.contains(verb)),
            "{DRIFT} contains `{}`; a drift check reads and changes nothing",
            verb.trim()
        );
    }
    // No self-grant: infra.yml's `add-iam-policy-binding` loop is a write.
    assert!(
        !code.contains("add-iam-policy-binding"),
        "{DRIFT} grants itself a role; a drift check must not change IAM to see IAM"
    );
}

#[test]
fn the_drift_workflow_plans_dev_and_nothing_else() {
    let code = code();
    assert!(
        code.lines().any(|line| line.trim() == "ENVIRONMENT: dev"),
        "{DRIFT} does not fix its environment to the literal `dev`"
    );
    // The environment is never an input, so no dispatch can point it at prod.
    for open in ["inputs.", "workflow_dispatch:\n    inputs"] {
        assert!(
            !code.contains(open),
            "{DRIFT} reads `{open}`; the environment it plans must not be chosen at dispatch"
        );
    }
    assert!(
        !code.contains("prod"),
        "{DRIFT} names prod; prod is never touched from a workflow"
    );
    // It queues behind infra.yml's dev operations rather than racing them.
    assert!(
        code.lines().any(|line| line.trim() == "group: infra-dev")
            && code
                .lines()
                .any(|line| line.trim() == "cancel-in-progress: false"),
        "{DRIFT} does not share infra.yml's dev concurrency group, so it can plan mid-apply"
    );
}

#[test]
fn no_agent_starts_the_drift_workflow() {
    let code = code();
    assert!(
        code.contains("agent-guard.mjs dispatch") && code.contains("--workflow drift.yml"),
        "{DRIFT} does not run the agent guard for itself"
    );
    let policy: serde_json::Value = serde_json::from_str(&read(POLICY)).expect("policy is JSON");
    let refused: Vec<&str> = policy["dispatch_refused_for_agents"]["workflows"]
        .as_array()
        .expect("dispatch_refused_for_agents.workflows is a list")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    // Premise: the list is the one infra.yml relies on.
    assert!(
        refused.contains(&"infra.yml"),
        "premise: {POLICY} no longer lists infra.yml"
    );
    assert!(
        refused.contains(&"drift.yml"),
        "{POLICY} does not list drift.yml, so its guard step admits an agent: the guard refuses \
         only the workflows the list names"
    );
}

/// The verdict step's exit status when `terraform plan` exits `status`.
fn verdict_for(status: i32) -> i32 {
    let body = run_body("plan, and fail on drift");
    let script = format!("terraform() {{ return {status}; }}\nENVIRONMENT=dev\n{body}\n");
    Command::new("bash")
        .arg("-c")
        .arg(&script)
        .output()
        .expect("bash runs")
        .status
        .code()
        .expect("an exit code")
}

#[test]
fn a_non_empty_plan_fails_the_run_and_a_clean_one_passes() {
    // Premise: the body was found and calls terraform at all.
    assert!(
        run_body("plan, and fail on drift").contains("terraform -chdir"),
        "the verdict step does not run terraform"
    );
    assert_eq!(verdict_for(0), 0, "a clean plan failed the drift check");
    assert_ne!(
        verdict_for(2),
        0,
        "terraform's exit 2 (the plan has changes) passed the drift check, so drift alerts nobody"
    );
    assert_ne!(
        verdict_for(1),
        0,
        "a plan that errored passed the drift check, so a check that cannot plan reads as clean"
    );
}
