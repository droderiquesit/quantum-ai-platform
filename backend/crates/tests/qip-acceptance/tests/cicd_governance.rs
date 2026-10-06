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
    let plan_section = plan_section.split("- name: up\n").next().unwrap_or_default();
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
