//! CICD-081: the pull request template prompts for the goal a change came
//! from. It is a prompt, not enforcement — the assessment row says so — but
//! the two lines and the origin taxonomy must not disappear silently.

use std::path::Path;

fn template() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.github/pull_request_template.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => panic!("PR template must exist at .github/pull_request_template.md: {e}"),
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
