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
