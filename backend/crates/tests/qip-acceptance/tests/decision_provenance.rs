//! R12-GOV-6121's `code_version` reaches a deployed binary, and a deployed
//! binary without it does not start.
//!
//! The kernel reads `QIP_SOURCE_COMMIT` at compile time
//! (`qip_kernel::decision_record::SOURCE_COMMIT`), so the fact lives in four
//! places no single crate can see together: the Dockerfile that must require
//! it, the two build paths that must pass it, and the three composition
//! roots that must refuse to run without it. Until 2026-10-11 none of the
//! four held it, and every deployed decision's `code_version` was the crate
//! version alone — the same string for every commit since the last bump.
//!
//! Text checks, as `infrastructure.rs` makes them: they cannot run a build,
//! and they fail when a link in the chain is deleted, which is the change
//! that actually happens. The behaviour of the refusal itself is
//! `qip-kernel/tests/decision_versions.rs`'s.

#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{production_text, read};

/// Every root that runs the cycle and so journals decision versions.
const CENTRAL_ROOTS: [&str; 3] = [
    "backend/crates/apps/qip-api/src/main.rs",
    "backend/crates/apps/qip-fastbrain/src/main.rs",
    "backend/crates/apps/qip-deepbrain/src/main.rs",
];

#[test]
fn every_central_root_refuses_to_run_deployed_without_a_source_commit() {
    for root in CENTRAL_ROOTS {
        let text = production_text(&read(root));
        // The premise: this is the root that names its deployment.
        let parsed = text
            .find("Deployment::parse(")
            .unwrap_or_else(|| panic!("{root} no longer parses its deployment"));
        let required = text
            .find("decision_record::require_source_commit(deployment)")
            .unwrap_or_else(|| {
                panic!(
                    "{root} starts without checking that its build names a source commit, so \
                     every decision it journals may name code nobody can check out"
                )
            });
        assert!(
            parsed < required,
            "{root} checks the stamp before it knows its deployment"
        );
        // And before it binds a listener, where a root has one in its own
        // text: a process that serves first has already answered as healthy.
        if let Some(serving) = text
            .find("TcpListener::bind")
            .or_else(|| text.find(".serve("))
        {
            assert!(
                required < serving,
                "{root} serves before it checks its stamp"
            );
        }
    }
}

#[test]
fn the_image_requires_the_commit_and_both_build_paths_pass_it() {
    let dockerfile = read("infrastructure/docker/Dockerfile");
    assert!(
        dockerfile.contains("ARG QIP_SOURCE_COMMIT"),
        "the Dockerfile declares no QIP_SOURCE_COMMIT build argument"
    );
    assert!(
        dockerfile.contains("test \"${#QIP_SOURCE_COMMIT}\" -eq 40")
            && dockerfile.contains("export SOURCE_DATE_EPOCH QIP_SOURCE_COMMIT"),
        "the Dockerfile builds without requiring a full commit sha, or does not hand it to cargo"
    );

    let deploy = read(".github/workflows/deploy.yml");
    assert_eq!(
        deploy
            .matches("--build-arg \"QIP_SOURCE_COMMIT=${DEPLOY_SHA}\"")
            .count(),
        2,
        "deploy.yml's runner build and its reproducibility rebuild must both pass the commit, \
         or the two builds differ and one of them is unstamped"
    );
    let pool = read("infrastructure/cloudbuild/build-image.yaml");
    assert!(
        pool.contains("--build-arg \"QIP_SOURCE_COMMIT=${_REVISION}\""),
        "the private pool builds an unstamped image"
    );
    assert!(
        !deploy.contains("vars.QIP_SOURCE_COMMIT"),
        "the commit must come from the run, never a repository variable"
    );
}
