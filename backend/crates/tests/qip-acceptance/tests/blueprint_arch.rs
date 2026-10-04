//! ARCH-072 (one monorepo) and ARCH-048 / ARCH-047 (no Pub/Sub or Kafka as an
//! internal dependency; a partner bridge could only ever be stateless and at
//! the boundary).
//!
//! These claims were held by directory inspection and by prose. A prose claim
//! stays true after someone adds a second repository or a Kafka topic; a test
//! does not.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeSet;

#[test]
fn the_whole_platform_resolves_to_one_repository_built_from_one_commit() {
    let root = qip_acceptance::repository_root();
    // Premise: each artefact class the requirement names resolves to a path.
    for path in [
        "backend/Cargo.toml",
        "backend/crates/libs/qip-events",
        "backend/crates/libs/qip-transport",
        "infrastructure/terraform/main.tf",
        "docs/ops/policies",
        "frontend/portal/package.json",
        ".github/workflows/ci.yml",
    ] {
        assert!(root.join(path).exists(), "{path} is not in this repository");
    }
    assert!(
        !root.join(".gitmodules").exists(),
        "a submodule makes part of the platform a second repository"
    );
    // A workflow that checks out another repository builds more than one
    // commit's worth of platform. Only the `repository:` key does that.
    let mut checked = 0;
    for workflow in qip_acceptance::files_with_extension(".github/workflows", "yml") {
        let text = std::fs::read_to_string(&workflow).expect("workflow is readable");
        checked += 1;
        for line in text.lines() {
            assert!(
                !line.trim_start().starts_with("repository:"),
                "{} checks out another repository: {line}",
                workflow.display()
            );
        }
    }
    assert!(checked >= 3, "only {checked} workflows were scanned");
}

/// The one Pub/Sub topic Terraform may create, and the binding that lets the
/// secret store publish to it. It carries a Secret Manager rotation notice to
/// an operator and is not internal messaging.
///
/// The two trust-zones bindings are declared but `for_each` over an empty map
/// while `control_fabric_topic` is null; the second test pins that it is null.
const ALLOWED_PUBSUB_RESOURCES: [&str; 4] = [
    "google_pubsub_topic.rotation",
    "google_pubsub_topic_iam_member.rotation_publisher",
    "google_pubsub_topic_iam_member.fabric_attach",
    "google_pubsub_topic_iam_member.fabric_publish",
];

#[test]
fn terraform_creates_no_pubsub_or_kafka_resource_beyond_the_recorded_rotation_notice() {
    let mut declared = BTreeSet::new();
    let mut scanned = 0;
    for file in qip_acceptance::files_with_extension("infrastructure/terraform", "tf") {
        scanned += 1;
        let text = std::fs::read_to_string(&file).expect("terraform is readable");
        for line in text.lines() {
            let line = line.trim_start();
            let Some(rest) = line.strip_prefix("resource \"") else {
                continue;
            };
            let mut parts = rest.split('"');
            let kind = parts.next().unwrap_or_default();
            let name = parts.nth(1).unwrap_or_default();
            let lowered = kind.to_ascii_lowercase();
            if lowered.contains("pubsub") || lowered.contains("kafka") {
                declared.insert(format!("{kind}.{name}"));
            }
        }
    }
    // Premise: the walk saw Terraform, and saw the one allowed topic, so an
    // empty set cannot mean the scan found nothing to scan.
    assert!(scanned > 20, "only {scanned} terraform files were scanned");
    assert!(
        declared.contains("google_pubsub_topic.rotation"),
        "the scan did not find the rotation topic it is meant to allow: {declared:?}"
    );
    let allowed: BTreeSet<String> = ALLOWED_PUBSUB_RESOURCES.map(str::to_string).into();
    assert_eq!(
        declared, allowed,
        "a Pub/Sub or Kafka resource other than the rotation notice is internal messaging \
         ARCH-047 and ARCH-048 forbid; Algorik semantics are defined by the Rust Fabric"
    );
}

#[test]
fn the_control_fabric_has_no_pubsub_topic_and_no_partner_bridge_exists() {
    let main = qip_acceptance::read("infrastructure/terraform/main.tf");
    assert!(
        main.lines()
            .any(|l| l.trim() == "control_fabric_topic = null"),
        "the trust-zones control fabric topic must stay null: a topic here is an internal \
         consumer of Pub/Sub"
    );

    // A bridge would be a crate, a module or a workflow step named for the
    // system it bridges. None exists, so none may appear unreviewed: adding
    // one is an ADR, and the ADR is where "stateless, at the boundary" is
    // argued.
    let mut names = 0;
    // Depth is bounded: a test file elsewhere may legitimately say "bridge"
    // (the kernel has one about something else); a crate, module or workflow
    // named for one is the thing under guard.
    for (relative, depth) in [
        ("backend/crates", 2),
        ("infrastructure/terraform/modules", 1),
        (".github/workflows", 1),
    ] {
        let directory = qip_acceptance::repository_root().join(relative);
        let mut stack = vec![(directory, 0)];
        while let Some((next, level)) = stack.pop() {
            for entry in std::fs::read_dir(&next)
                .expect("directory is readable")
                .flatten()
            {
                names += 1;
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                assert!(
                    !name.contains("kafka") && !name.contains("bridge"),
                    "{} looks like a partner bridge; it needs an ADR proving it is stateless \
                     and confined to the external boundary",
                    entry.path().display()
                );
                if entry.path().is_dir() && level + 1 < depth {
                    stack.push((entry.path(), level + 1));
                }
            }
        }
    }
    assert!(names > 80, "only {names} entries were scanned");

    // No manifest names a Kafka or Pub/Sub client.
    let mut manifests = 0;
    for manifest in qip_acceptance::files_with_extension("backend", "toml") {
        let text = std::fs::read_to_string(&manifest).expect("manifest is readable");
        manifests += 1;
        for line in text.lines() {
            let line = line.trim_start().to_ascii_lowercase();
            assert!(
                !(line.starts_with("rdkafka")
                    || line.starts_with("google-cloud-pubsub")
                    || line.starts_with("kafka")),
                "{} declares a messaging client: {line}",
                manifest.display()
            );
        }
    }
    assert!(manifests > 50, "only {manifests} manifests were scanned");
}
