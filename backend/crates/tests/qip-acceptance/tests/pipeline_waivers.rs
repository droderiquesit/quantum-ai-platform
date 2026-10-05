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
