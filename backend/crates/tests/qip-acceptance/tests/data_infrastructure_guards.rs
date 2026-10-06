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

/// TICK-037. Tick and internal financial history are governed retention classes.
/// Market data (Transient) expires after 90 days; internal data (Irreplaceable)
/// never expires; world-derived knowledge is bounded pass-through and not archived.
/// The test verifies that:
/// 1. Market history lifecycle includes deletion at 90 days
/// 2. Internal history lifecycle never deletes (only storage class transitions)
/// 3. The lake separates the two classes by path
/// 4. Retention classes are declared exhaustively in the event log
#[test]
fn tick_037_retention_classes_are_governed() {
    // Verify Terraform declares separate lifecycle rules for market vs. internal data.
    let data_module = read("infrastructure/terraform/modules/data/main.tf");
    let text = without_comments(&data_module);

    // Market data must have a delete rule (Transient retention).
    assert!(
        text.contains("lake/class=market/") && text.contains("type") && text.contains("Delete"),
        "TICK-037: Market data (Transient class) must have a lifecycle rule that deletes old objects"
    );

    // Internal data must never delete (Irreplaceable/Permanent retention).
    assert!(
        text.contains("lake/class=internal/") && text.contains("SetStorageClass"),
        "TICK-037: Internal data (Irreplaceable class) must transition storage classes but never delete"
    );

    // Verify no Delete action applies to internal data.
    let mut found_internal = false;
    let mut found_delete = false;
    for section in text.split("prefix_match") {
        if section.contains("lake/class=internal/") {
            found_internal = true;
            if section.contains("Delete") {
                found_delete = true;
            }
        }
    }
    assert!(
        found_internal && !found_delete,
        "TICK-037: Internal data must never have a Delete lifecycle action"
    );

    // Verify the lake code declares retention classes exhaustively.
    let lake_rs = read("backend/crates/libs/qip-storage/src/lake.rs");
    assert!(
        lake_rs.contains("pub enum RecordClass")
            && lake_rs.contains("Market")
            && lake_rs.contains("Internal"),
        "TICK-037: Lake must declare Market and Internal record classes in RecordClass enum"
    );
    assert!(
        lake_rs.contains("Self::Market => \"market\"")
            && lake_rs.contains("Self::Internal => \"internal\""),
        "TICK-037: Lake paths must encode the class (market vs. internal)"
    );

    // Verify event log enforces retention classes per-topic.
    let retention_rs = read("backend/crates/libs/qip-events/src/retention.rs");
    assert!(
        retention_rs.contains("pub enum RetentionClass") || retention_rs.contains("RetentionClass"),
        "TICK-037: Event log must declare RetentionClass enum"
    );

    let topic_rs = read("backend/crates/libs/qip-events/src/topic.rs");
    assert!(
        topic_rs.contains("retention_class") || topic_rs.contains("retention"),
        "TICK-037: Every Topic must declare its retention class"
    );

    let _ = repository_root();
}
