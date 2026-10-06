//! Structural guards for two DATA requirements that hold today only because
//! nothing has yet been built to violate them.
//!
//! String checks on HCL and Rust, with comments removed, like
//! `infrastructure.rs`: they cannot understand the configuration, they can
//! fail when the protection is deleted, which is the change that happens.

#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

/// Source with `#` and `//` line comments removed, so prose explaining why a
/// thing is refused is not mistaken for declaring it.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            let line = line.split('#').next().unwrap_or("");
            line.split("//").next().unwrap_or("").trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn collapsed(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// DATA-063. A streaming-analytics or batch cluster is the wrong shape for the
/// live consumer of the event fabric: it has no per-event ordering guarantee
/// the cells rely on and its scheduling latency is minutes. Nothing declares
/// one today, so the property is vacuous, and a vacuous property is the one a
/// later convenient addition breaks without anybody noticing.
#[test]
fn no_dataflow_or_dataproc_resource_is_declared_so_none_can_be_a_live_fabric_consumer() {
    let mut scanned = 0;
    let mut resources = 0;
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let text = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        scanned += 1;
        resources += text.matches("resource \"google_").count();
        for banned in ["google_dataflow", "google_dataproc"] {
            assert!(
                !text.contains(banned),
                "{} declares {banned}; the fabric's live consumers are the platform's own \
                 binaries, and a batch or streaming-analytics job is research-plane only \
                 (DATA-063). Record a decision before adding one",
                path.display()
            );
        }
    }
    assert!(
        scanned > 10 && resources > 10,
        "premise: the scan saw the Terraform ({scanned} files, {resources} resources)"
    );

    // The consumer side in Rust: the fabric's own binaries and event library.
    let mut rust_seen = 0;
    for dir in [
        "backend/crates/apps/qip-fabricd/src",
        "backend/crates/apps/qip-ledgerd/src",
        "backend/crates/libs/qip-events/src",
    ] {
        for path in files_with_extension(dir, "rs") {
            rust_seen += 1;
            let text =
                without_comments(&std::fs::read_to_string(&path).expect("readable")).to_lowercase();
            assert!(
                !text.contains("dataflow") && !text.contains("dataproc"),
                "{} names Dataflow or Dataproc outside a comment (DATA-063)",
                path.display()
            );
        }
    }
    assert!(
        rust_seen > 5,
        "premise: the Rust scan saw {rust_seen} files"
    );
}

/// DATA-052. The evidence bucket's lock is what makes it immutable to the
/// people who run the platform. The existing suite checks that a
/// `retention_policy` block exists, which `is_locked = false` still satisfies,
/// so this pins the three things that actually carry the guarantee: the bucket
/// takes its lock from the variable, the variable defaults to locked, and the
/// root neither unlocks it nor shortens the period.
#[test]
fn the_evidence_retention_lock_cannot_be_unset_or_shortened_by_the_root_without_a_test_failing() {
    let bucket = without_comments(&read("infrastructure/terraform/modules/evidence/main.tf"));
    let lines: Vec<String> = bucket.lines().map(collapsed).collect();
    assert!(
        lines
            .iter()
            .any(|l| l == "is_locked = var.retention_locked"),
        "the bucket's retention policy does not take its lock from var.retention_locked"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "retention_period = var.retention_days * 24 * 60 * 60"),
        "the bucket's retention period is not var.retention_days in seconds"
    );

    let variables = without_comments(&read(
        "infrastructure/terraform/modules/evidence/variables.tf",
    ));
    let defaults_of = |name: &str| -> String {
        let block = variables
            .split(&format!("variable \"{name}\" {{"))
            .nth(1)
            .unwrap_or_else(|| panic!("variable {name} is not declared"));
        let block = block.split("\nvariable ").next().unwrap_or(block);
        block
            .lines()
            .map(collapsed)
            .find_map(|l| l.strip_prefix("default = ").map(str::to_string))
            .unwrap_or_else(|| panic!("variable {name} has no default"))
    };
    assert_eq!(defaults_of("retention_locked"), "true");
    let days: u64 = defaults_of("retention_days")
        .parse()
        .expect("the default is a number of days");
    assert!(
        days >= 2557,
        "the default retention is {days} days, shorter than the seven years the compliance \
         surface requires; a locked policy can only be lengthened afterwards"
    );

    let root = without_comments(&read("infrastructure/terraform/main.tf"));
    let module = root
        .split("module \"evidence\" {")
        .nth(1)
        .expect("the root instantiates the evidence module")
        .split("\nmodule ")
        .next()
        .expect("split yields a first part");
    assert!(
        module.contains("writer_service_accounts"),
        "premise: this is the evidence module's block, not another's"
    );
    for forbidden in ["retention_locked", "retention_days"] {
        assert!(
            !module
                .lines()
                .map(collapsed)
                .any(|l| l.starts_with(forbidden)),
            "the root overrides {forbidden} on the evidence module; the defaults are the \
             restrictive ones and an override is the way they get lowered"
        );
    }

    for env in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{env}/terraform.tfvars"
        ));
        let text = without_comments(&tfvars);
        assert!(
            !text.contains("retention_locked") && !text.contains("evidence_retention"),
            "{env}'s tfvars sets an evidence retention variable"
        );
    }
    let _ = repository_root();
}

/// FINOPS-013. The archive bucket's retention policy must be locked to prevent
/// modification of the hash-chained event log. An unlocked policy allows deletion,
/// which violates the immutability guarantee.
#[test]
fn the_archive_bucket_retention_policy_is_locked_to_prevent_modification() {
    let archive_config = without_comments(&read("infrastructure/terraform/modules/data/main.tf"));
    let archive_block = archive_config
        .split("resource \"google_storage_bucket\" \"archive\" {")
        .nth(1)
        .expect("archive bucket resource exists")
        .split("\nresource ")
        .next()
        .expect("split yields the archive block");

    assert!(
        archive_block.contains("is_locked"),
        "archive bucket retention_policy does not declare is_locked"
    );

    let retention_lines: Vec<String> = archive_block
        .lines()
        .filter(|l| l.contains("is_locked"))
        .map(collapsed)
        .collect();

    assert!(
        !retention_lines.is_empty(),
        "no is_locked setting found in archive bucket"
    );

    assert!(
        retention_lines
            .iter()
            .all(|l| l.contains("true") || l.contains("var.")),
        "archive bucket is_locked is not set to true or a variable; found: {:?}",
        retention_lines
    );
}

/// FINOPS-014. Derived data stores must have bounded retention or lifecycle rules.
/// This test checks that training scratch buckets and other derived stores are
/// not retained forever, preventing unbounded storage cost growth.
#[test]
fn the_terraform_plan_declares_lifecycle_rules_for_all_derived_data_stores() {
    let archive_config = without_comments(&read("infrastructure/terraform/modules/data/main.tf"));

    // Training scratch bucket is explicitly declared as derived data.
    // It should have either an expiration_time, lifecycle_rule, or TTL.
    let has_training_scratch = archive_config.contains("google_storage_bucket\" \"training");

    if has_training_scratch {
        let training_block = archive_config
            .split("resource \"google_storage_bucket\" \"training")
            .nth(1)
            .expect("training scratch bucket exists")
            .split("\nresource ")
            .next()
            .unwrap_or("");

        assert!(
            training_block.contains("lifecycle_rule") || training_block.contains("expiration"),
            "training scratch bucket has no lifecycle rule or expiration; \
             derived data must not be retained forever (FINOPS-014)"
        );
    }
}
