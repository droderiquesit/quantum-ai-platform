//! CICD-081: the pull request template prompts for the goal a change came
//! from. It is a prompt, not enforcement — the assessment row says so — but
//! the two lines and the origin taxonomy must not disappear silently.
//!
//! CICD-067: all terraform apply and destroy operations must consume saved plan
//! artifacts instead of running with -auto-approve flags. This ensures all
//! infrastructure changes are reviewed through the plan before being applied.

use std::path::Path;

fn template() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.github/pull_request_template.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => panic!("PR template must exist at .github/pull_request_template.md: {e}"),
    }
}

fn infra_workflow() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.github/workflows/infra.yml");
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => panic!("infra.yml workflow must exist at .github/workflows/infra.yml: {e}"),
    }
}

#[test]
fn pr_template_requires_goal_id_and_goal_origin() {
    let text = template();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();

    assert!(
        lines.iter().any(|l| l.starts_with("**Goal ID:**")),
        "PR template must carry a '**Goal ID:**' line (CICD-081)"
    );

    // Match the whole delimited line, not a substring: "incident" alone also
    // appears in the Goal ID line, so a contains() check could never fail.
    assert!(
        lines.contains(&"**Goal Origin:** (incident / backlog item / expansion gap)"),
        "PR template must carry the Goal Origin line naming exactly the three origin classes"
    );
}

#[test]
fn terraform_apply_and_destroy_consume_saved_plans_not_auto_approve() {
    let workflow = infra_workflow();

    // The guard hook blocks -auto-approve on terraform apply/destroy. Verify
    // that infra.yml never uses it, so all apply/destroy operations consume
    // saved plan artifacts instead and require explicit plan review.
    assert!(
        !workflow.contains("terraform apply -input=false -auto-approve"),
        "terraform apply must not use -auto-approve; it must consume a saved plan"
    );

    assert!(
        !workflow.contains("terraform destroy -input=false -auto-approve"),
        "terraform destroy must not use -auto-approve; it must consume a saved plan"
    );

    // Verify that 'up' action sequence: plan upload -> download -> apply.
    // Note: search for "- name: up\n" to avoid matching "- name: upload".
    let up_marker = "- name: up\n";
    let up_section = workflow
        .split(up_marker)
        .nth(1)
        .expect("infra.yml must have a '- name: up' step");

    // The plan the up step applies is written and shown in the same run: an
    // artifact from an earlier dispatch is invisible to this run and would be
    // stale after the reclaim step's imports.
    let plan_section = workflow
        .split("- name: plan for up\n")
        .nth(1)
        .expect("infra.yml must have a '- name: plan for up' step (CICD-067)");
    let plan_section = plan_section
        .split("- name: up\n")
        .next()
        .unwrap_or_default();
    assert!(
        plan_section.contains("-out=tfplan") && plan_section.contains("show -no-color tfplan"),
        "the up action must write its plan to tfplan and show it before applying"
    );

    // Check that the up step applies a saved plan.
    assert!(
        up_section.contains("apply -input=false tfplan"),
        "up step must apply the saved tfplan artifact"
    );

    // Verify that all destroy steps (suspend, down, teardown) generate and
    // consume plans instead of using -auto-approve.
    for action in &["suspend", "down", "teardown"] {
        let section = workflow
            .split(&format!("- name: {}", action))
            .nth(1)
            .unwrap_or_default();
        assert!(
            !section.is_empty(),
            "{} step must exist in infra.yml",
            action
        );
        assert!(
            section.contains("plan -destroy") && section.contains("-out=tfplan-destroy"),
            "{} step must generate a destroy plan with plan -destroy -out=tfplan-destroy",
            action
        );
        assert!(
            section.contains("apply -input=false tfplan-destroy"),
            "{} step must consume the saved tfplan-destroy",
            action
        );
    }
}

/// R12-GOV-9080: an agent proposing an exception cites the rule it changes.
///
/// The template carries the field and ci's `rule-citation` job refuses a pull
/// request that changes a rule file without naming it there. The job's
/// behaviour is `scripts/rule-citation.test.mjs`, run by the `scripts` job;
/// this holds the wiring — that the field cannot leave the template and the
/// job cannot leave ci, or stop reading the body from the event file — which
/// the script's own tests cannot see.
#[test]
fn a_pull_request_that_changes_a_rule_must_name_it_and_ci_refuses_one_that_does_not() {
    let text = template();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // The whole delimited line: the script reads the text after this exact
    // label, so a reworded label would leave every body uncited.
    assert!(
        lines.contains(&"**Rule changed:** none"),
        "PR template must carry the '**Rule changed:** none' line the rule-citation job reads"
    );

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.github/workflows/ci.yml");
    let ci = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("ci.yml: {e}"));
    let job = ci
        .split("\n  rule-citation:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n  # ").next())
        .unwrap_or_else(|| panic!("ci.yml has no rule-citation job"));
    assert!(
        job.contains("if: github.event_name == 'pull_request'"),
        "the rule-citation job is not bound to pull requests, the only event with a body"
    );
    assert!(
        job.contains(
            "node scripts/rule-citation.mjs --event \"$GITHUB_EVENT_PATH\" --changed \"$RUNNER_TEMP/changed.txt\""
        ),
        "the rule-citation job does not judge the event's body against the changed paths"
    );
    assert!(
        job.contains("git diff --name-only \"$BASE_SHA\" \"$HEAD_SHA\""),
        "the rule-citation job does not compute the changed paths against the base"
    );
    assert!(
        !job.contains("github.event.pull_request.body"),
        "the body is pasted into the job as an expression; it is a stranger's text and is \
         read from the event file only"
    );
    assert!(
        ci.contains("scripts/rule-citation.test.mjs"),
        "the rule-citation script's tests are not run by the scripts job"
    );
}
