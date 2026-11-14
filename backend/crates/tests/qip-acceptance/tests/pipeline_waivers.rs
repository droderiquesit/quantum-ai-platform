//! No gate can be waived from inside a change (CICD-015), and the admission
//! policy's own rule is pinned (CICD-055).
//!
//! `the_pipeline_gates_on_everything_it_claims_to` is a substring check over
//! `ci.yml`; it would pass a change that marked the performance suite
//! ignored, or a job that never fails. These tests refuse the two ways a
//! failing gate is quietly turned green from inside a diff. What they cannot
//! hold is the merge half: branch protection on `main` is a repository
//! setting, not a file.

#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

fn workflows() -> Vec<std::path::PathBuf> {
    let mut all = files_with_extension(".github/workflows", "yml");
    all.extend(files_with_extension(".github/workflows", "yaml"));
    all
}

/// The needle is assembled at run time so this file does not contain the
/// text it forbids elsewhere.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

#[test]
fn no_workflow_marks_a_step_as_allowed_to_fail() {
    let files = workflows();
    assert!(
        files.len() >= 3,
        "premise: the workflows were found, got {}",
        files.len()
    );
    let forbidden = needle(&["continue-on-", "error"]);
    let offenders: Vec<String> = files
        .iter()
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains(&forbidden)))
        .map(|p| p.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "a step allowed to fail turns a red gate green; remove it from {offenders:?}"
    );
}

#[test]
fn no_test_in_the_backend_is_ignored() {
    let files = files_with_extension("backend/crates", "rs");
    assert!(
        files.len() > 100,
        "premise: the backend sources were found, got {}",
        files.len()
    );
    let forbidden = needle(&["#[", "ignore"]);
    let offenders: Vec<String> = files
        .iter()
        .filter(|p| !p.ends_with("pipeline_waivers.rs"))
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains(&forbidden)))
        .map(|p| {
            p.strip_prefix(repository_root())
                .unwrap_or(p)
                .display()
                .to_string()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "an ignored test is a waived gate; run or delete it, in {offenders:?}"
    );
}

#[test]
fn the_binary_authorization_default_rule_requires_a_named_attestation_and_blocks() {
    let text = read("infrastructure/terraform/modules/binaryauthorization/main.tf");
    let start = text
        .find("default_admission_rule {")
        .expect("premise: the policy declares a default admission rule");
    let rule: &str = text[start..].split('}').next().unwrap_or("");
    for required in [
        r#"evaluation_mode         = "REQUIRE_ATTESTATION""#,
        r#"enforcement_mode        = "ENFORCED_BLOCK_AND_AUDIT_LOG""#,
        "require_attestations_by = [google_binary_authorization_attestor.build.name]",
    ] {
        assert!(
            rule.contains(required),
            "default rule lost `{required}`:\n{rule}"
        );
    }
}

/// The Dockerfile's instructions, comments dropped and `\` continuations
/// joined, so a check reads what Docker runs rather than what the prose says.
fn dockerfile_instructions(text: &str) -> Vec<String> {
    let mut instructions = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || (trimmed.is_empty() && current.is_empty()) {
            continue;
        }
        match trimmed.strip_suffix('\\') {
            Some(head) => {
                current.push_str(head);
                current.push(' ');
            }
            None => {
                current.push_str(trimmed);
                if !current.trim().is_empty() {
                    instructions.push(std::mem::take(&mut current));
                }
                current.clear();
            }
        }
    }
    instructions
}

/// CICD-048: the build stage fetches nothing the base digest does not pin.
///
/// It once ran `apk add --no-cache musl-dev`, refreshing alpine's index over
/// the network on every build, so two builds of one commit could compile
/// against different toolchain revisions. A lane then "fixed" that with a
/// version pin naming a revision no alpine release has ever served, which
/// would have failed every image build; the test it shipped checked the text
/// `musl-dev=` and would have passed. The base image already carries musl-dev
/// and gcc in its digest-pinned layers, so the rule is that no `RUN` invokes
/// a package manager at all.
#[test]
fn the_build_stage_installs_nothing_the_digest_does_not_pin() {
    let instructions = dockerfile_instructions(&read("infrastructure/docker/Dockerfile"));
    let runs: Vec<&String> = instructions
        .iter()
        .filter(|i| i.split_whitespace().next() == Some("RUN"))
        .collect();
    // Premise: the parser found the build's RUN, or "no RUN calls apk" is vacuous.
    assert!(
        runs.iter()
            .any(|r| r.split_whitespace().any(|w| w == "cargo")),
        "expected a RUN invoking cargo; the Dockerfile parser found {runs:?}"
    );
    for run in &runs {
        assert!(
            !run.split_whitespace().any(|w| w == "apk" || w == "apt-get"),
            "a RUN fetches packages the base image digest does not pin: {run}"
        );
    }
    let from_build = instructions
        .iter()
        .find(|i| i.starts_with("FROM ") && i.ends_with(" AS build"));
    assert!(
        from_build.is_some_and(|f| f.contains("@sha256:")),
        "the build stage must start from a digest-pinned base: {from_build:?}"
    );
}

/// CICD-052: the artifact registry holds container images, Rust packages,
/// and deployment bundles.
///
/// The registry module must declare three repositories: images (DOCKER format),
/// rust_packages (GENERIC format for Cargo crates), and deployment_bundles
/// (GENERIC format for versioned OCI artifacts). Each is configured with
/// immutable tags (where applicable), cleanup policies in dry-run mode, and
/// IAM bindings that allow CI to push but not delete.
#[test]
fn the_artifact_registry_declares_all_three_repositories() {
    let text = read("infrastructure/terraform/modules/registry/main.tf");

    // Verify the container image repository is declared.
    assert!(
        text.contains("resource \"google_artifact_registry_repository\" \"images\""),
        "container image repository (images) not declared"
    );
    assert!(
        text.contains("format        = \"DOCKER\""),
        "container image repository must use DOCKER format"
    );
    assert!(
        text.contains("immutable_tags = true"),
        "container image repository must have immutable tags enabled"
    );

    // Verify the Rust package repository is declared.
    assert!(
        text.contains("resource \"google_artifact_registry_repository\" \"rust_packages\""),
        "Rust package repository (rust_packages) not declared"
    );
    let rust_section = text
        .split("resource \"google_artifact_registry_repository\" \"rust_packages\"")
        .nth(1)
        .expect("rust_packages section not found")
        .split("resource \"google_artifact_registry_repository\"")
        .next()
        .unwrap_or("");
    assert!(
        rust_section.contains("format        = \"GENERIC\""),
        "Rust package repository must use GENERIC format"
    );
    assert!(
        rust_section.contains("cleanup_policy_dry_run = true"),
        "Rust package repository must have cleanup policy in dry-run mode"
    );

    // Verify the deployment bundle repository is declared.
    assert!(
        text.contains("resource \"google_artifact_registry_repository\" \"deployment_bundles\""),
        "deployment bundle repository (deployment_bundles) not declared"
    );
    let bundle_section = text
        .split("resource \"google_artifact_registry_repository\" \"deployment_bundles\"")
        .nth(1)
        .expect("deployment_bundles section not found")
        .split("resource \"google_artifact_registry_repository\"")
        .next()
        .unwrap_or("");
    assert!(
        bundle_section.contains("format        = \"GENERIC\""),
        "deployment bundle repository must use GENERIC format"
    );
    assert!(
        bundle_section.contains("cleanup_policy_dry_run = true"),
        "deployment bundle repository must have cleanup policy in dry-run mode"
    );

    // Verify CI has push access to all three repositories via writer role.
    assert!(
        text.contains("resource \"google_artifact_registry_repository_iam_member\" \"ci_push\""),
        "CI push access to container images not configured"
    );
    assert!(
        text.contains(
            "resource \"google_artifact_registry_repository_iam_member\" \"rust_packages_ci_push\""
        ),
        "CI push access to rust packages not configured"
    );
    assert!(
        text.contains("resource \"google_artifact_registry_repository_iam_member\" \"deployment_bundles_ci_push\""),
        "CI push access to deployment bundles not configured"
    );

    // Verify all three use the writer role (push without delete).
    let ci_push_sections: Vec<&str> = text.split("\"ci_push\"").collect();
    assert!(
        ci_push_sections
            .iter()
            .any(|section| { section.contains("role       = \"roles/artifactregistry.writer\"") }),
        "container image CI push must use writer role"
    );
    let rust_push_sections: Vec<&str> = text.split("\"rust_packages_ci_push\"").collect();
    assert!(
        rust_push_sections
            .iter()
            .any(|section| { section.contains("role       = \"roles/artifactregistry.writer\"") }),
        "Rust package CI push must use writer role"
    );
    let bundle_push_sections: Vec<&str> = text.split("\"deployment_bundles_ci_push\"").collect();
    assert!(
        bundle_push_sections
            .iter()
            .any(|section| { section.contains("role       = \"roles/artifactregistry.writer\"") }),
        "deployment bundle CI push must use writer role"
    );
}

/// CodeQL is configured, gates ci, and still scans `main`.
///
/// **What changed (`d2e79d3e`, R12 CICD-9105), and why this test changed.**
/// CodeQL used to trigger itself on push and pull request as a workflow
/// beside ci, and this test asserted those triggers. But `deploy.yml` waited
/// only on ci, so a CodeQL finding stopped nothing. The workflow is now
/// `workflow_call`ed by ci as its `sast` job, which runs on every push to
/// `main` and every pull request into it because ci does, and deploy reads
/// the `sast / …` jobs of the run it trusts by name. Its own push and
/// pull_request triggers were removed on purpose: a second independent run is
/// a second result nobody gates on. So "scans main" is now asserted where it
/// is true — ci's triggers and ci's `sast` job — and the daily schedule, for
/// advisories against code that has not changed, is asserted on the workflow.
/// The old assertion was `contains("branches:") && contains("main")`, which
/// any comment mentioning `main` satisfied; these read the trigger lines.
///
/// Mutation: delete ci's `uses: ./.github/workflows/codeql-analysis.yml` —
/// the gate assertion fires. Drop `"main"` from ci's push branches — the
/// main-branch assertion fires. Delete the workflow's `schedule:` — the
/// schedule assertion fires.
#[test]
fn codeql_analysis_is_configured_for_security_scanning() {
    let workflows = workflows();
    let codeql_workflow = workflows
        .iter()
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n == "codeql-analysis.yml" || n == "codeql-analysis.yaml")
        })
        .expect("CodeQL workflow file (codeql-analysis.yml) must exist");
    let content = read(codeql_workflow.to_str().expect("valid path"));
    assert!(
        content.contains("github/codeql-action/init"),
        "CodeQL workflow must use github/codeql-action/init to initialize analysis"
    );
    assert!(
        content.contains("github/codeql-action/analyze"),
        "CodeQL workflow must use github/codeql-action/analyze to perform analysis"
    );
    assert!(
        content.contains("rust") || content.contains("Rust"),
        "CodeQL analysis must include Rust language for code scanning"
    );

    // The workflow's own triggers: callable by ci, and on a daily schedule.
    let triggers = top_level_block(&content, "on:");
    assert!(
        triggers.iter().any(|line| line.trim() == "workflow_call:"),
        "CodeQL must be callable as ci's sast job: {triggers:?}"
    );
    assert!(
        triggers.iter().any(|line| line.trim() == "schedule:")
            && triggers
                .iter()
                .any(|line| line.trim_start().starts_with("- cron:")),
        "CodeQL must keep its daily schedule, for advisories against unchanged code: {triggers:?}"
    );

    // ci calls it, as a job of its own, so ci cannot succeed unless it does.
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.lines()
            .any(|line| line.trim() == "uses: ./.github/workflows/codeql-analysis.yml"),
        "ci.yml does not call codeql-analysis.yml, so nothing gates on CodeQL and, with its push \
         and pull_request triggers gone, nothing runs it on main"
    );
    let ci_triggers = top_level_block(&ci, "on:");
    let branches_of = |event: &str| -> String {
        ci_triggers
            .iter()
            .skip_while(|line| line.trim() != event)
            .skip(1)
            .take_while(|line| line.starts_with("    "))
            .find(|line| line.trim_start().starts_with("branches:"))
            .map(|line| line.to_string())
            .unwrap_or_default()
    };
    assert!(
        branches_of("push:").contains("\"main\""),
        "CodeQL must run on the main branch for continuous security scanning: ci, which runs \
         it, does not trigger on pushes to main: {ci_triggers:?}"
    );
    assert!(
        branches_of("pull_request:").contains("\"main\""),
        "CodeQL must run on pull requests to catch security issues before merge: ci, which \
         runs it, does not trigger on pull requests into main: {ci_triggers:?}"
    );

    // And deploy trusts a ci run only if that run's sast jobs ran.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("startswith(\"sast / \")"),
        "deploy.yml no longer reads the sast jobs of the ci run it trusts"
    );
}

/// The lines of a top-level YAML key's block: the key's own line excluded,
/// every following line up to the next line at column zero that is not blank
/// or a comment.
fn top_level_block<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|line| *line != key)
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('#'))
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .collect()
}

// --- CICD-9107: a repository the pipeline never writes to ---------------------

/// The `repository_id` a registry resource declares, with the module's
/// `${var.environment}` rewritten to the workflow's `${TARGET_ENVIRONMENT}`.
fn registry_repository_id(terraform: &str, resource: &str) -> String {
    let header = format!("resource \"google_artifact_registry_repository\" \"{resource}\"");
    let body = terraform
        .split(&header)
        .nth(1)
        .unwrap_or_else(|| panic!("{resource} is not declared in the registry module"))
        .split("\nresource ")
        .next()
        .unwrap_or("");
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("repository_id"))
        .unwrap_or_else(|| panic!("{resource} declares no repository_id"));
    let quoted = line
        .split('"')
        .nth(1)
        .unwrap_or_else(|| panic!("{resource}'s repository_id is not a quoted string"));
    quoted.replace("${var.environment}", "${TARGET_ENVIRONMENT}")
}

/// Whether `token` occurs in `text` as a whole name: the character after it
/// is not one a repository id can continue with. `qip-rust-${X}` must not be
/// satisfied by `qip-rust-${X}-secondary`.
fn contains_name(text: &str, token: &str) -> bool {
    text.match_indices(token).any(|(at, _)| {
        text[at + token.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    })
}

/// Whether workflow text publishes to a repository: a generic upload, the
/// repository named as a whole token, and the by-digest evidence (the
/// bundle's `@${digest}` references and a hash of each file). Comment lines
/// are dropped first, so prose describing a publish is not one.
fn publishes_to(workflow: &str, repository_id: &str) -> bool {
    let code: String = workflow
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    code.contains("artifacts generic upload")
        && code.contains("sha256sum")
        && code.contains("@${digest}")
        && contains_name(&code, repository_id)
}

/// The lines a protected patch adds, without the diff's `+` marker.
fn added_lines(patch: &str) -> String {
    patch
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| &l[1..])
        .collect::<Vec<_>>()
        .join("\n")
}

/// CICD-9107, the decision's half that is about the check itself: it must be
/// able to tell a publish from its absence.
///
/// A check that every workflow satisfies guards nothing, and the real test
/// below reads a file that today carries no publish step at all.
#[test]
fn the_publish_check_tells_a_publish_from_prose_a_neighbour_and_nothing() {
    let publish = "run: |\n  h=$(sha256sum f)\n  echo \"x@${digest}\"\n  \
                   gcloud artifacts generic upload --repository=\"qip-rust-${TARGET_ENVIRONMENT}\" \
                   --version=v\n";
    assert!(
        publishes_to(publish, "qip-rust-${TARGET_ENVIRONMENT}"),
        "premise: a real publish was not recognised"
    );
    assert!(
        !publishes_to(publish, "qip-deployments-${TARGET_ENVIRONMENT}"),
        "a publish to one repository was read as a publish to another"
    );
    let neighbour = publish.replace(
        "qip-rust-${TARGET_ENVIRONMENT}",
        "qip-rust-${TARGET_ENVIRONMENT}-secondary",
    );
    assert!(
        !publishes_to(&neighbour, "qip-rust-${TARGET_ENVIRONMENT}"),
        "a longer repository name satisfied the shorter one"
    );
    let prose: String = publish.lines().map(|l| format!("# {l}\n")).collect();
    assert!(
        !publishes_to(&prose, "qip-rust-${TARGET_ENVIRONMENT}"),
        "a commented-out publish was read as a publish"
    );
    let by_tag = publish.replace("@${digest}", ":latest");
    assert!(
        !publishes_to(&by_tag, "qip-rust-${TARGET_ENVIRONMENT}"),
        "a publish that names no digest was accepted"
    );
}

/// CICD-9107 (owner decision 36, 2026-11-14): every registry repository the
/// module declares has a workflow step that publishes to it, by digest.
///
/// `modules/registry` declares `rust_packages` and `deployment_bundles` with a
/// writer grant for CI, and nothing wrote to either, so two repositories
/// claimed to hold what a build and a deployment are reproduced from while
/// holding nothing. The decision is to publish, not to remove them: removal
/// is a destructive apply that also rewrites the waiver test above.
///
/// Until a person applies `docs/blueprint/lanes/cicd-9107-protected.patch`
/// (`.github/**` is protected, so the step could not be written in the lane
/// that built this test), the step is read from that patch's added lines.
/// Once `deploy.yml` carries it, the first arm is the live one and the patch
/// should be deleted with the fallback.
///
/// Mutation: delete the `deployment-bundle` publish call (or rename
/// `qip-deployments-` there) in `deploy.yml`, or in the patch while
/// `deploy.yml` lacks the step: this test fails naming the repository.
#[test]
fn every_registry_repository_has_a_workflow_that_publishes_to_it() {
    let terraform = read("infrastructure/terraform/modules/registry/main.tf");
    let repositories = [
        registry_repository_id(&terraform, "rust_packages"),
        registry_repository_id(&terraform, "deployment_bundles"),
    ];
    assert!(
        repositories
            .iter()
            .all(|r| r.contains("${TARGET_ENVIRONMENT}") && r.starts_with("qip-")),
        "premise: both repository ids were read from the module and name the environment: {repositories:?}"
    );

    let deploy = read(".github/workflows/deploy.yml");
    let patch = std::fs::read_to_string(
        repository_root().join("docs/blueprint/lanes/cicd-9107-protected.patch"),
    )
    .map(|p| added_lines(&p))
    .unwrap_or_default();
    for repository in &repositories {
        assert!(
            publishes_to(&deploy, repository) || publishes_to(&patch, repository),
            "nothing publishes to {repository} by digest: deploy.yml has no generic upload \
             naming it, and the protected patch does not carry one"
        );
    }
}
