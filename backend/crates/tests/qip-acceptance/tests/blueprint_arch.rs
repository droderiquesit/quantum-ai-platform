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

// --- ARCH-074: the v12.0 completeness targets --------------------------------

/// Text with every run of whitespace collapsed, so a target wrapped across
/// two lines of the source reads the same as one written on a single line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The COMPLETE TARGET items of blueprint v12.0 §31.1, read from the source
/// text: from the section's heading to the next numbered section. An item
/// runs from its marker to the next blank line, which is what keeps the
/// page footer after the tenth out of it.
fn section_31_1_targets() -> Vec<String> {
    const MARKER: &str = "COMPLETE TARGET:";
    let source = qip_acceptance::read("docs/blueprint/source/algorik-master-blueprint-v12.0.txt");
    let mut targets: Vec<String> = Vec::new();
    let mut inside = false;
    let mut open = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("31.1 ") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if trimmed.starts_with("32. ") {
            break;
        }
        // `split_once` and not `strip_prefix`: each item is preceded by the
        // PDF's bullet glyph, a private-use character no trim removes.
        if let Some((_, rest)) = trimmed.split_once(MARKER) {
            targets.push(rest.trim().to_string());
            open = true;
        } else if trimmed.is_empty() {
            open = false;
        } else if open && let Some(last) = targets.last_mut() {
            last.push(' ');
            last.push_str(trimmed);
        }
    }
    targets.iter().map(|target| one_line(target)).collect()
}

#[test]
fn the_ten_v12_completeness_targets_are_the_blueprints_own_words_and_each_is_scored_from_requirements_it_cites()
 {
    // ARCH-074. The failure this prevents has two halves. A list of targets
    // paraphrased from the blueprint drifts until it scores something the
    // blueprint never asked for; and a target with nothing cited under it is
    // scored on its own text. So the list is held to the source word for
    // word, and every entry must rest on requirement rows the register
    // actually holds — whose statuses the rendered view then quotes, and
    // which `documentation.rs` refuses to let go stale.
    let from_source = section_31_1_targets();
    assert_eq!(
        from_source.len(),
        10,
        "the walk of §31.1 found {} COMPLETE TARGET items, not the ten the section lists; it \
         is not reading the section: {from_source:?}",
        from_source.len()
    );

    let root = qip_acceptance::repository_root();
    let sources = qip_cli::blueprint::load(&root).expect("the blueprint sources load");
    let registered: Vec<String> = sources
        .completeness_targets
        .iter()
        .map(|target| one_line(target["target"].as_str().unwrap_or_default()))
        .collect();
    assert_eq!(
        registered, from_source,
        "docs/blueprint/v12-completeness-targets.json does not list §31.1's ten targets in \
         the blueprint's own words and order"
    );

    // `load` has already refused a target citing nothing or citing an id the
    // catalogue does not hold; what it cannot know is which requirement makes
    // the ninth target safe to adopt. "Can veto/reduce exposure" reads as a
    // second enforcement path unless the veto is a limit the Risk Gate
    // evaluates, and RISK-020 is the row that says enforcement is the Gate's.
    let veto = sources
        .completeness_targets
        .iter()
        .find(|target| {
            target["target"]
                .as_str()
                .is_some_and(|text| text.contains("veto/reduce exposure"))
        })
        .expect("one of the ten is the veto target");
    let cited: BTreeSet<&str> = veto["requirements"]
        .as_array()
        .expect("a list of ids")
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert!(
        cited.contains("RISK-020"),
        "the veto target cites {cited:?} and not RISK-020; without the row that keeps \
         enforcement with the Risk Gate, the target reads as a second enforcement path"
    );

    // And the view a reader opens carries one row per target.
    let view = qip_cli::blueprint::render_views(&sources)
        .into_iter()
        .find(|view| view.path == "docs/blueprint/v12-completeness-targets.md")
        .expect("the targets view is rendered");
    let rows = view
        .text
        .lines()
        .filter(|line| line.starts_with("| ") && !line.starts_with("| # "))
        .count();
    assert_eq!(
        rows, 10,
        "the rendered view does not hold one row per target"
    );
}

/// CICD-083: The dev environment explicitly declares mock venue adapters.
///
/// The simulated venue is hardcoded in `infrastructure/terraform/modules/
/// execution-node/templates/startup.sh.tftpl` and is the only value the
/// execution node binary accepts. To make this explicit in tfvars (the
/// requirement verification check: "The dev tfvars define mock venue adapters"),
/// the root variables.tf declares `venue_adapter_type`, and the dev
/// environment sets it to "simulated" for fast iteration.
///
/// This test verifies that the dev tfvars contains the explicit declaration
/// so the mock venue configuration is visible and maintained, not just
/// implicitly hardcoded.
#[test]
fn dev_environment_declares_mock_venue_adapter_explicitly() {
    let root = qip_acceptance::repository_root();
    let dev_tfvars =
        std::fs::read_to_string(root.join("infrastructure/environments/dev/terraform.tfvars"))
            .expect("dev tfvars is readable");

    // The dev tfvars must explicitly declare the venue adapter type as "simulated".
    assert!(
        dev_tfvars.contains("venue_adapter_type = \"simulated\""),
        "dev tfvars must explicitly declare venue_adapter_type = \"simulated\" for mock venue iteration"
    );

    // Assert the declaration is present without being commented out.
    let uncommented_lines: Vec<_> = dev_tfvars
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| line.contains("venue_adapter_type = \"simulated\""))
        .collect();
    assert!(
        !uncommented_lines.is_empty(),
        "venue_adapter_type must not be commented out in dev tfvars"
    );
}
