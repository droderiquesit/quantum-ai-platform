use std::path::Path;

#[test]
fn pr_template_requires_goal_id_and_goal_origin() {
    let template_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.github/pull_request_template.md");
    assert!(
        template_path.exists(),
        "PR template must exist at .github/pull_request_template.md"
    );

    let template_content =
        std::fs::read_to_string(template_path).expect("Failed to read PR template");

    assert!(
        template_content.contains("**Goal ID:**"),
        "PR template must contain Goal ID field for autonomous changes (CICD-081)"
    );

    assert!(
        template_content.contains("**Goal Origin:**"),
        "PR template must contain Goal Origin field for autonomous changes (CICD-081)"
    );

    assert!(
        template_content.contains("incident") && template_content.contains("backlog item"),
        "PR template must document valid Goal Origin values (incident, backlog item, expansion gap)"
    );
}
