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

    // Verify no Delete action applies to internal data, one rule at a time.
    // The split is on the rule block: GCS spells the condition
    // `matches_prefix`, so splitting on "prefix_match" found no boundary and
    // judged the whole file as one rule, where the market rule's Delete
    // always sat beside the internal prefix.
    let mut found_internal = false;
    let mut found_delete = false;
    for section in text.split("lifecycle_rule") {
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

    // R12-DATA-7013 and R12-DATA-8124: the simulation bucket and the regional
    // segment store each carry a retention policy and a lifecycle rule, and
    // the simulation bucket names a class for each of its three prefixes. The
    // training branch above is conditional and the training bucket lives in
    // modules/ai, so until these were added the test asserted nothing about
    // this file at all.
    let bucket = |label: &str| -> String {
        let header = format!("resource \"google_storage_bucket\" \"{label}\" {{");
        let body = archive_config
            .split(header.as_str())
            .nth(1)
            .unwrap_or_else(|| panic!("modules/data declares no google_storage_bucket.{label}"));
        body.split("\nresource ")
            .next()
            .unwrap_or_default()
            .to_string()
    };
    for label in ["simulation", "segments"] {
        let body = bucket(label);
        assert!(
            body.lines().any(|l| collapsed(l) == "retention_policy {"),
            "google_storage_bucket.{label} sets no retention_policy (R12-DATA-7013)"
        );
        assert!(
            body.lines().any(|l| collapsed(l) == "lifecycle_rule {"),
            "google_storage_bucket.{label} has no lifecycle rule; derived data must not be \
             retained forever (FINOPS-014)"
        );
    }
    let simulation = bucket("simulation");
    for prefix in ["outputs/", "campaigns/", "manifests/"] {
        let quoted = format!("\"{prefix}\"");
        assert!(
            simulation.lines().any(|l| {
                let l = collapsed(l);
                l.starts_with("matches_prefix") && l.contains(quoted.as_str())
            }),
            "google_storage_bucket.simulation names no lifecycle class for {prefix} (R12-DATA-7013)"
        );
    }
}

/// Every Cloud Storage bucket this repository declares, and the class of
/// data it is for. A bucket missing from this table fails
/// [`no_bucket_or_dataset_lands_external_world_content`] until somebody
/// states what it holds — which is the review R12-DATA-8012 asks for, made a
/// step nobody can skip by adding a resource.
const BUCKET_CLASSES: [(&str, &str); 12] = [
    (
        "training",
        "model training staging: derived features and weights",
    ),
    (
        "staging",
        "build pool staging: build inputs this repository produced",
    ),
    (
        "bootstrap",
        "egress proxy bootstrap: committed Envoy configuration",
    ),
    (
        "archive",
        "event-fabric archive: tick and internal history (Tier T)",
    ),
    (
        "artifacts",
        "model artifacts: weights this platform trained",
    ),
    (
        "payload",
        "image-bake payload: binaries this repository built",
    ),
    ("collector_config", "metrics collector configuration"),
    ("config_files", "workload configuration files"),
    ("static_shell", "the public edge's static shell"),
    ("evidence", "internal evidence, retention-locked"),
    (
        "segments",
        "regional segment store: sealed tick and internal replay segments",
    ),
    (
        "simulation",
        "simulation outputs, campaign datasets and their manifests",
    ),
];

/// Tokens that, in a bucket's or dataset's Terraform label or name, would
/// mean a store for content fetched from the outside world. Matched as whole
/// `-`/`_`-delimited tokens, so `rawhide` or `drawing` do not trip it and
/// `raw-world` does.
const EXTERNAL_CONTENT_TOKENS: [&str; 7] = [
    "landing", "raw", "crawl", "crawler", "scrape", "scout", "corpus",
];

/// `(resource type, label, name expression)` of every storage resource in
/// `hcl`, comments removed.
fn storage_resources(hcl: &str) -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    let lines: Vec<&str> = hcl.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let line = collapsed(line);
        let Some(rest) = line.strip_prefix("resource \"") else {
            continue;
        };
        let mut parts = rest.split('"');
        let kind = parts.next().unwrap_or_default().to_string();
        if kind != "google_storage_bucket" && kind != "google_bigquery_dataset" {
            continue;
        }
        let label = parts.nth(1).unwrap_or_default().to_string();
        let name = lines
            .iter()
            .skip(index + 1)
            .take_while(|line| !line.starts_with('}'))
            .map(|line| collapsed(line))
            .find_map(|line| {
                line.strip_prefix("name =")
                    .or_else(|| line.strip_prefix("dataset_id ="))
                    .map(|value| value.trim().to_string())
            })
            .unwrap_or_default();
        found.push((kind, label, name));
    }
    found
}

/// The violations in `resources`: an external-content token in a label or
/// name, or a bucket nobody classified.
fn landing_violations(resources: &[(String, String, String)]) -> Vec<String> {
    let mut violations = Vec::new();
    for (kind, label, name) in resources {
        let tokens: Vec<String> = format!("{label} {name}")
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect();
        if let Some(token) = EXTERNAL_CONTENT_TOKENS
            .iter()
            .find(|token| tokens.iter().any(|t| t == *token))
        {
            violations.push(format!(
                "{kind}.{label} ({name}) names `{token}`: external-world content is streamed \
                 through memory and never landed (R12-DATA-8012, R12-DATA-8121)"
            ));
        }
        if kind == "google_storage_bucket"
            && !BUCKET_CLASSES.iter().any(|(known, _)| known == label)
        {
            violations.push(format!(
                "{kind}.{label} ({name}) is not in BUCKET_CLASSES; state what it holds before \
                 it exists"
            ));
        }
    }
    violations
}

/// R12-DATA-8012 and R12-DATA-8121. Scout and connector workers stream source
/// bytes through memory (the HTTP client reads a response into a buffer and
/// the connector runtime digests and drops it), and there is no landing
/// bucket and no general-purpose raw-world lake. Both held only because
/// nobody had declared one; this makes declaring one a test failure. The
/// first half proves the checker refuses the shapes it exists to refuse —
/// without it, a checker that matched nothing would pass forever over a
/// tree that happened to be clean.
#[test]
fn no_bucket_or_dataset_lands_external_world_content() {
    // The checker refuses what it exists to refuse, and admits a plain one.
    let refused = landing_violations(&storage_resources(
        "resource \"google_storage_bucket\" \"landing\" {\n  name = \"${local.prefix}-raw-world\"\n}\n\
         resource \"google_bigquery_dataset\" \"corpus\" {\n  dataset_id = \"web_corpus\"\n}\n",
    ));
    assert_eq!(refused.len(), 3, "{refused:#?}");
    assert!(refused[0].contains("`landing`"), "{refused:#?}");
    assert!(refused[1].contains("not in BUCKET_CLASSES"), "{refused:#?}");
    assert!(refused[2].contains("`corpus`"), "{refused:#?}");
    assert!(
        landing_violations(&storage_resources(
            "resource \"google_storage_bucket\" \"evidence\" {\n  name = \"qip-drawing-evidence\"\n}\n"
        ))
        .is_empty(),
        "a classified bucket whose name merely contains `raw` inside a word was refused"
    );

    // The tree.
    let mut resources = Vec::new();
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let text = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        resources.extend(storage_resources(&text));
    }
    let buckets = resources
        .iter()
        .filter(|(kind, ..)| kind == "google_storage_bucket")
        .count();
    assert!(
        buckets >= BUCKET_CLASSES.len() && resources.len() > buckets,
        "premise: the walk found the declared buckets and the research dataset; found {resources:#?}"
    );
    let violations = landing_violations(&resources);
    assert!(violations.is_empty(), "{violations:#?}");
}

/// R12-DATA-1029 / R12-DATA-7021. The raw body a connector fetched is
/// discarded once evidence and knowledge are extracted. The unit test beside
/// `ConnectorFeed` proves it for the digest and the journal of one feed; this
/// drives the node's own arm (`ConnectorArm`, the type `qip-deepbrain`'s
/// `main` builds) through a `Platform` and looks in every durable store the
/// arm writes: the platform's hash-chained event log, the journal's key-value
/// store, the knowledge-pack blob store and the tick lake's blob store.
///
/// The canary rides in the JSON-LD `@context` of the served document, a field
/// no connector keeps. The premises matter as much as the denial: a platform
/// that recorded nothing, or stores that stayed empty, would pass `!contains`
/// for a source that never carried the canary.
///
/// Mutated by building `FetchDigest::of`'s `locator` as the locator followed
/// by the lossy-UTF-8 body: the reference the platform journals then carries
/// the body and the event-log assertion fails; restored byte for byte.
#[test]
fn a_canary_in_a_fetched_body_is_found_in_no_durable_store_after_a_poll_through_the_platform() {
    use qip_core::{Context, Timestamp};
    use qip_data_finder::registration::RegistrationRegistry;
    use qip_deepbrain::connectors::{ConnectorArm, TICK_LAKE_RETENTION_VARIABLE, TickLakeSettings};
    use qip_financial::universe::Universe;
    use qip_kernel::config::PlatformConfig;
    use qip_kernel::platform::Platform;
    use qip_market_ingestion::connector::emulator::SourceEmulator;
    use qip_market_ingestion::connectors::NwsStationObservationsConnector;
    use qip_market_ingestion::connectors::nws_station_observations::FIXTURE;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;
    use qip_storage::{BlobStore, KeyValueStore, MemoryBlobStore, MemoryKeyValueStore};
    use std::sync::Arc;

    const CANARY: &str = "CANARY-5d2a8e41-not-for-retention";
    let served = FIXTURE.replace(
        r#"\"city\": \"s:addressLocality\""#,
        &format!(r#"\"city\": \"s:addressLocality-{CANARY}\""#),
    );
    assert!(
        served.contains(CANARY) && served != FIXTURE,
        "premise: the canary was planted in the document the source serves"
    );

    // 2026-09-19T05:55:00Z, the horizon the arm's own pack test polls at.
    let horizon = Timestamp::from_secs(1_789_797_300);
    let mut manifest = NwsStationObservationsConnector::shipped_manifest().expect("manifest");
    manifest.endpoint.base_url = Some("http://127.0.0.1:1".to_string());
    let entitlement = format!("{:?}", manifest.licensing).to_ascii_lowercase();

    let kv = Arc::new(MemoryKeyValueStore::default());
    let packs = Arc::new(MemoryBlobStore::new());
    let lake = Arc::new(MemoryBlobStore::new());
    let lookup = |name: &str| {
        (name == TICK_LAKE_RETENTION_VARIABLE).then(|| format!("{entitlement}=canonical+raw"))
    };
    let settings = TickLakeSettings::from_lookup(&lookup)
        .expect("the retention variable parses")
        .expect("a retention right was configured");
    let mut arm = ConnectorArm::over_transport(
        Box::new(NwsStationObservationsConnector::new(manifest.clone()).expect("connector")),
        manifest,
        Box::new(SourceEmulator::from_json(&served).expect("the emulator parses")),
        &RegistrationRegistry::shipped(),
        7,
        horizon,
    )
    .expect("the arm opens through the catalogue")
    .with_knowledge_packs(packs.clone())
    .with_tick_lake(settings.writer(lake.clone()).expect("the lake opens"));
    arm.journal_to(kv.clone()).expect("a journal opens");

    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(horizon, config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .expect("the platform builds");
    let before = platform.event_log().records().len();

    let released = arm.poll(&mut platform, horizon).expect("the poll delivers");
    arm.shutdown(horizon).expect("the arm flushes and closes");
    assert!(!released.is_empty(), "premise: the arm released readings");

    let log = platform.event_log().records();
    assert!(
        log.len() > before,
        "premise: the fetch was referenced on the log"
    );
    assert!(
        !format!("{log:?}").contains(CANARY),
        "the event log retained the fetched body"
    );

    let keys = kv.keys_with_prefix("").expect("the journal lists");
    assert!(!keys.is_empty(), "premise: the journal wrote something");
    for key in keys {
        let value = kv.get(&key).expect("reads").expect("listed key exists");
        assert!(
            !value.to_string().contains(CANARY),
            "the journal key {key} retained the body"
        );
    }

    for (name, store) in [("knowledge-pack", &packs), ("tick-lake", &lake)] {
        let blobs = store.list("").expect("the store lists");
        if name == "knowledge-pack" {
            assert!(!blobs.is_empty(), "premise: a pack was kept");
        }
        for key in blobs {
            let bytes = store.get(&key).expect("reads").expect("listed key exists");
            assert!(
                !String::from_utf8_lossy(&bytes).contains(CANARY),
                "the {name} store retained the body under {key}"
            );
        }
    }
}
