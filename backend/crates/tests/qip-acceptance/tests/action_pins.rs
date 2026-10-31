//! R12-CICD-10203: every action a workflow uses is pinned by its full commit
//! SHA, with the tag it was resolved from kept as a comment.
//!
//! A tag is movable. `actions/checkout@v4` names whatever commit the
//! action's owner last pointed `v4` at, so a workflow pinned by tag runs code
//! nobody here reviewed the day the tag moves, with the job's token and the
//! workload identity it federates to. A 40-hex SHA names one tree and cannot
//! be moved. The comment keeps the tag readable, so a person updating a pin
//! can see what it was resolved from.

use qip_acceptance::{files_with_extension, repository_root};

/// The `uses:` value on a workflow line, if the line is a step's or a job's
/// `uses:` key (a `- uses:` step or a job-level `uses:`), with the comment
/// after it, if any.
fn uses_of(line: &str) -> Option<(&str, Option<&str>)> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("- uses:")
        .or_else(|| trimmed.strip_prefix("uses:"))?;
    let (value, comment) = match rest.split_once(" #") {
        Some((value, comment)) => (value.trim(), Some(comment.trim())),
        None => (rest.trim(), None),
    };
    Some((value, comment))
}

/// Why a `uses:` value is refused, or nothing when it is admitted.
fn refusal(value: &str, comment: Option<&str>) -> Option<String> {
    // A reusable workflow in this repository is reviewed with the change that
    // calls it; it has no ref to move.
    if value.starts_with("./") {
        return None;
    }
    let Some((action, reference)) = value.rsplit_once('@') else {
        return Some(format!(
            "`{value}` names no ref at all, so it runs the default branch"
        ));
    };
    let is_sha = reference.len() == 40
        && reference
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !is_sha {
        return Some(format!(
            "`{action}@{reference}` is pinned by a movable ref; pin the 40-hex commit SHA the \
             tag resolves to and keep the tag as a `# vN` comment"
        ));
    }
    let tagged = comment.is_some_and(|c| {
        c.strip_prefix('v')
            .is_some_and(|v| v.starts_with(|ch: char| ch.is_ascii_digit()))
    });
    if !tagged {
        return Some(format!(
            "`{action}@{reference}` carries no `# vN` comment naming the tag it was resolved from"
        ));
    }
    None
}

fn refusals_in(workflows: &[(String, String)]) -> (usize, Vec<String>) {
    let mut seen = 0;
    let mut refused = Vec::new();
    for (path, text) in workflows {
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            let Some((value, comment)) = uses_of(line) else {
                continue;
            };
            seen += 1;
            if let Some(why) = refusal(value, comment) {
                refused.push(format!("{path}:{}: {why}", number + 1));
            }
        }
    }
    (seen, refused)
}

fn committed_workflows() -> Vec<(String, String)> {
    let root = repository_root();
    // All of .github, not only workflows: a composite action under
    // .github/actions runs its own `uses:` steps with the caller's token.
    let mut files: Vec<_> = files_with_extension(".github", "yml")
        .into_iter()
        .chain(files_with_extension(".github", "yaml"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} is unreadable: {error}", path.display()));
            let shown = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            (shown, text)
        })
        .collect()
}

#[test]
fn every_action_a_workflow_uses_is_pinned_by_a_full_commit_sha_with_its_tag_as_a_comment() {
    let workflows = committed_workflows();
    let (seen, refused) = refusals_in(&workflows);
    // Premise: the walk found the workflows and their steps, so an empty
    // refusal list is not the empty list of a walk that read nothing.
    assert!(
        workflows.len() >= 10,
        "premise: .github holds the platform's workflows"
    );
    assert!(
        seen >= 50,
        "premise: the walk found the workflows' `uses:` lines, found {seen}"
    );
    assert!(refused.is_empty(), "movable action refs: {refused:#?}");
}

#[test]
fn a_tag_a_branch_a_short_sha_or_an_untagged_sha_is_refused_and_a_tagged_sha_is_admitted() {
    let sha = "11d5960a326750d5838078e36cf38b85af677262";
    let workflow = |line: &str| vec![("w.yml".to_string(), format!("steps:\n{line}\n"))];
    let admitted = workflow(&format!("      - uses: actions/checkout@{sha} # v4"));
    assert_eq!(refusals_in(&admitted), (1, Vec::new()));
    let local = workflow("    uses: ./.github/workflows/codeql-analysis.yml");
    assert_eq!(refusals_in(&local), (1, Vec::new()));
    for refused in [
        "      - uses: actions/checkout@v4".to_string(),
        "      - uses: actions/checkout@main".to_string(),
        "      - uses: actions/checkout@11d5960".to_string(),
        format!("      - uses: actions/checkout@{sha}"),
        format!("      - uses: actions/checkout@{}", sha.to_uppercase()),
        "      - uses: actions/checkout".to_string(),
    ] {
        let (seen, refusals) = refusals_in(&workflow(&refused));
        assert_eq!(seen, 1, "premise: `{refused}` is read as a uses line");
        assert_eq!(refusals.len(), 1, "`{refused}` was admitted");
    }
}
