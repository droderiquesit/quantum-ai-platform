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

/// The two Pub/Sub topics Terraform may create, each with the one binding
/// that lets a Google service agent publish to it. The first carries a Secret
/// Manager rotation notice to an operator; the second carries Cloud Asset
/// Inventory's record of every resource and IAM change (R12 SEC-9135, the
/// root's `audit-logs.tf`). Both are notices from a Google control plane to
/// an operator, and neither is internal messaging: nothing in the tree
/// publishes to or subscribes from either.
///
/// The trust-zones module once declared two more — `fabric_publish` and
/// `fabric_attach`, `for_each` over an empty map behind a
/// `control_fabric_topic` the root could only set to null. They were removed
/// because inert bindings read as a Pub/Sub control fabric §1.1 contradicts;
/// the second test refuses the variable coming back.
const ALLOWED_PUBSUB_RESOURCES: [&str; 4] = [
    "google_pubsub_topic.rotation",
    "google_pubsub_topic_iam_member.rotation_publisher",
    "google_pubsub_topic.asset_changes",
    "google_pubsub_topic_iam_member.asset_changes_publisher",
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
        "a Pub/Sub or Kafka resource other than the rotation notice and the asset-change feed \
         is internal messaging ARCH-047 and ARCH-048 forbid; Algorik semantics are defined by \
         the Rust Fabric"
    );
}

#[test]
fn the_control_fabric_has_no_pubsub_topic_and_no_partner_bridge_exists() {
    // The premise: the module and the root are where a topic would be wired,
    // and both are read; then neither may name one. A variable that exists
    // can be set, and a binding that can be set is a Pub/Sub control fabric
    // waiting for a value.
    let variables =
        qip_acceptance::read("infrastructure/terraform/modules/trust-zones/variables.tf");
    assert!(
        variables.contains("variable \"zones\""),
        "the trust-zones variables file no longer declares `zones`; this test is reading the \
         wrong file"
    );
    assert!(
        !variables.contains("variable \"control_fabric_topic\""),
        "the trust-zones module takes a control fabric topic again: a topic here is an internal \
         consumer of Pub/Sub, which §1.1 refuses"
    );
    let main = qip_acceptance::read("infrastructure/terraform/main.tf");
    assert!(
        main.contains("module \"trust_zones\""),
        "the root no longer instantiates trust_zones; this test is reading the wrong file"
    );
    assert!(
        !main
            .lines()
            .any(|l| l.trim_start().starts_with("control_fabric_topic")),
        "the root passes a control fabric topic again"
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
fn the_v12_blueprint_source_holds_the_ten_completeness_targets_of_section_31_1() {
    // ARCH-074, as far as it can be held with no register. Until 2026-10-11
    // this also held `docs/blueprint/v12-completeness-targets.json` to these
    // words and required each target to cite requirement rows; that register
    // scored the v11.6 decomposition and was removed with it, and the
    // targets are to be re-cited against the v12.0 one when it exists. What
    // stays true without it is that the source text is the one the targets
    // will be scored from: a `.txt` re-extracted with a different layout, or
    // a different edition dropped in under the same name, would make that
    // future citation quote sentences the blueprint does not say.
    let from_source = section_31_1_targets();
    assert_eq!(
        from_source.len(),
        10,
        "the walk of §31.1 found {} COMPLETE TARGET items, not the ten the section lists; it \
         is not reading the section: {from_source:?}",
        from_source.len()
    );
    // The ninth is the one that reads as a second enforcement path unless a
    // later register ties it to the Risk Gate, so it is named here to keep
    // that obligation visible to whoever rebuilds the citations.
    assert!(
        from_source
            .iter()
            .any(|target| target.contains("can veto/reduce exposure")),
        "none of the ten targets is the capital/risk veto target: {from_source:?}"
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

/// The resource blocks a Terraform text declares of the given kinds, each as
/// `(kind, body)`. A block ends at the first line that is a lone `}` in column
/// zero, which is how `terraform fmt` leaves every top-level block.
fn resource_blocks(text: &str, kinds: &[&str]) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some((_, body)) = current.as_mut() {
            if line == "}" {
                blocks.push(current.take().expect("a block is open"));
            } else {
                body.push_str(line);
                body.push('\n');
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("resource \"") else {
            continue;
        };
        let kind = rest.split('"').next().unwrap_or_default();
        if kinds.contains(&kind) {
            current = Some((kind.to_string(), line.to_string() + "\n"));
        }
    }
    blocks
}

/// What a scheduler job or workflow must not be pointed at: the event fabric
/// and anything that places or routes an order.
const FABRIC_OR_TRADING_MARKERS: [&str; 6] = [
    "fabric",
    "/v1/events",
    "order",
    "execution",
    "ledger",
    "broker",
];

fn fabric_or_trading_targets(text: &str) -> Vec<String> {
    resource_blocks(
        text,
        &["google_cloud_scheduler_job", "google_workflows_workflow"],
    )
    .into_iter()
    .filter(|(_, body)| {
        let lowered = body.to_ascii_lowercase();
        FABRIC_OR_TRADING_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker))
    })
    .map(|(kind, body)| format!("{kind}: {}", body.lines().next().unwrap_or_default()))
    .collect()
}

#[test]
fn no_scheduler_or_workflow_resource_targets_a_fabric_or_trading_endpoint() {
    // R12-FABRIC-10069. Cloud Scheduler and Workflows are timers and glue; the
    // event backbone is the Rust Fabric, and a scheduled job that posts to a
    // fabric or order endpoint would make a Google timer the thing that
    // decides when a trading event happens, outside the hash-chained log.
    // None exists today, which is a fact about the tree that one added
    // resource would undo with every other test still green.
    let mut scanned = 0;
    let mut offenders = Vec::new();
    for file in qip_acceptance::files_with_extension("infrastructure/terraform", "tf") {
        scanned += 1;
        let text = std::fs::read_to_string(&file).expect("terraform is readable");
        offenders.extend(fabric_or_trading_targets(&text));
    }
    assert!(scanned > 20, "only {scanned} terraform files were scanned");
    assert_eq!(
        offenders,
        Vec::<String>::new(),
        "a scheduler job or workflow names the fabric or a trading endpoint"
    );

    // The detector sees what it claims to: a scheduler job aimed at the
    // fabric is found, a workflow aimed at an order route is found, and a
    // job with an unrelated target is not, so an empty result above is a
    // finding and not a blind scanner.
    let fabric_job = "resource \"google_cloud_scheduler_job\" \"tick\" {\n  http_target {\n    uri = \"https://qip-fabricd.example/v1/events\"\n  }\n}\n";
    let order_flow = "resource \"google_workflows_workflow\" \"rebalance\" {\n  source_contents = \"POST /orders\"\n}\n";
    let harmless = "resource \"google_cloud_scheduler_job\" \"report\" {\n  http_target {\n    uri = \"https://reports.example/daily\"\n  }\n}\n";
    assert_eq!(fabric_or_trading_targets(fabric_job).len(), 1);
    assert_eq!(fabric_or_trading_targets(order_flow).len(), 1);
    assert!(fabric_or_trading_targets(harmless).is_empty());
}

/// The tfvars keys that give an environment data to ingest: a live market
/// connector, the deep brain's polled sources, its discovery cadence, and the
/// candidate list the discovery desk reads. Uncommented assignments only.
const DATA_SOURCE_KEYS: [&str; 4] = [
    "market_data_connector",
    "deepbrain_connector",
    "deepbrain_discover_every",
    "source_candidates_file",
];

fn declared_data_sources(tfvars: &str) -> Vec<String> {
    tfvars
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .filter(|(key, value)| DATA_SOURCE_KEYS.contains(key) && *value != "null")
        .map(|(key, _)| key.to_string())
        .collect()
}

#[test]
fn dev_declares_no_more_data_sources_than_any_later_environment() {
    // R12-GCP-10144. The blueprint has dev run on reduced data. The knobs that
    // give a deployment data to ingest are the four tfvars keys above, and no
    // environment sets one today: the fast brain runs the synthetic exchange,
    // the deep brain polls no vendor and runs no discovery pass. So "reduced"
    // is met by dev holding the minimum, and what this pins is that dev never
    // holds more than a later environment: a vendor connector wired into dev
    // first is how a research-only licence reaches the catalogue.
    let root = qip_acceptance::repository_root();
    let read = |environment: &str| {
        std::fs::read_to_string(root.join(format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )))
        .expect("the environment's tfvars is readable")
    };
    let dev = read("dev");
    assert!(
        dev.lines()
            .any(|line| line.trim_start().starts_with("environment")),
        "the dev tfvars no longer names its environment, so the file read is not dev's"
    );
    let dev_sources = declared_data_sources(&dev);
    for later in ["test", "stage", "prod"] {
        let sources = declared_data_sources(&read(later));
        assert!(
            dev_sources.len() <= sources.len(),
            "dev declares {dev_sources:?} but {later} declares {sources:?}"
        );
    }
    assert_eq!(
        dev_sources,
        Vec::<String>::new(),
        "dev declares a data source"
    );

    // The scanner sees an assignment and ignores a comment, so the empty
    // result above is a finding.
    let mutated = format!("{dev}\ndeepbrain_discover_every = \"4\"\n");
    assert_eq!(
        declared_data_sources(&mutated),
        vec!["deepbrain_discover_every".to_string()]
    );
    let commented = format!("{dev}\n# deepbrain_discover_every = \"4\"\n");
    assert!(declared_data_sources(&commented).is_empty());
}
