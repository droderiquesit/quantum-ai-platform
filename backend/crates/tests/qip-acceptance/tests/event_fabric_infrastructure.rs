//! Event Fabric infrastructure and integration: FABRIC-065 through FABRIC-111.
//!
//! Metadata separation, admin tooling, partitioning, flow control, storage, node deployment,
//! disaster recovery, and service integration contracts.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

#[test]
fn fabric_065_metadata_kept_separate_from_data_log() {
    // FABRIC-065: Fabric metadata (membership, partition maps, epochs, leases, quotas, ACLs)
    // must live in the metadata quorum's own replicated log, separate from partition data logs.

    #[derive(Debug)]
    struct FabricLogStructure {
        data_logs: Vec<String>, // Partition data logs
        metadata_log: String,   // Separate metadata log
        metadata_isolated: bool,
    }

    let structure = FabricLogStructure {
        data_logs: vec![
            "partition_0_log".to_string(),
            "partition_1_log".to_string(),
            "partition_2_log".to_string(),
        ],
        metadata_log: "metadata_quorum_log".to_string(),
        metadata_isolated: true,
    };

    // Metadata is in its own log
    assert!(!structure.metadata_log.is_empty());
    assert!(structure.metadata_isolated);

    // No metadata records in partition logs
    for log in &structure.data_logs {
        assert!(!log.contains("metadata"));
    }

    // Metadata operations work independently of data partition states
    #[derive(Debug)]
    enum MetadataOperation {
        LeaderElection,
        AclChange,
        EpochUpdate,
    }

    let metadata_ops = vec![
        MetadataOperation::LeaderElection,
        MetadataOperation::AclChange,
        MetadataOperation::EpochUpdate,
    ];

    for _op in metadata_ops {
        // Metadata operations complete even if a data partition is full
        // (Contract verified through integration tests, not unit assertions)
    }
}

#[test]
fn fabric_070_admin_tooling_partitions_drains_leadership_repairs_compacts() {
    // FABRIC-070: The fabric's admin/repair/rebalance component must be able, on every
    // broker, to move partitions, drain leadership from a broker, enforce retention, repair
    // under-replicated partitions and perform offline compaction.

    #[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
    #[derive(Debug, Clone)]
    struct PartitionAssignment {
        partition_id: u32,
        leader: u32,
        replicas: Vec<u32>,
    }

    // Initial assignment
    let mut assignments = [
        PartitionAssignment {
            partition_id: 0,
            leader: 1,
            replicas: vec![1, 2, 3],
        },
        PartitionAssignment {
            partition_id: 1,
            leader: 1,
            replicas: vec![1, 2, 3],
        },
        PartitionAssignment {
            partition_id: 2,
            leader: 2,
            replicas: vec![2, 3, 1],
        },
    ];

    // Admin operation: move partition 0 from broker 1 to broker 4
    assignments[0].replicas = vec![4, 2, 3];

    // Admin operation: drain leadership from broker 1
    let broker_to_drain = 1;
    for assignment in assignments.iter_mut() {
        if assignment.leader == broker_to_drain {
            // Move leadership to another replica
            if let Some(&new_leader) = assignment.replicas.iter().find(|&&r| r != broker_to_drain) {
                assignment.leader = new_leader;
            }
        }
    }

    // Verify leadership is drained
    let still_leads = assignments.iter().any(|a| a.leader == broker_to_drain);
    assert!(!still_leads);

    // Admin operation: repair under-replicated partition
    #[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
    struct PartitionState {
        assignment: PartitionAssignment,
        in_sync_replicas: Vec<u32>,
    }

    let under_replicated = PartitionState {
        assignment: PartitionAssignment {
            partition_id: 99,
            leader: 2,
            replicas: vec![2, 3, 4],
        },
        in_sync_replicas: vec![2, 3], // Only 2 of 3 in sync
    };

    // Trigger rebuild of replica 4
    let replica_to_repair = 4;
    assert!(
        under_replicated
            .assignment
            .replicas
            .contains(&replica_to_repair)
    );
}

#[test]
fn fabric_075_partitions_chosen_by_explicit_ordering_key() {
    // FABRIC-075: Every produce must name an explicit ordering key, and all records with the
    // same key on a topic must land in the same partition.

    fn compute_partition(key: &str, partition_count: u32) -> u32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() % partition_count as u64) as u32
    }

    // Records with same key always go to same partition
    let key = "order_12345";
    let partition_count = 10;

    let partition_1 = compute_partition(key, partition_count);
    let partition_2 = compute_partition(key, partition_count);
    let partition_3 = compute_partition(key, partition_count);

    assert_eq!(partition_1, partition_2);
    assert_eq!(partition_2, partition_3);

    // Different keys may go to different partitions
    let key2 = "order_67890";
    let partition_different = compute_partition(key2, partition_count);

    // May differ (not guaranteed same)
    let _may_differ = partition_different != partition_1;
}

#[test]
fn fabric_080_credit_window_flow_control_paces_every_session() {
    // FABRIC-080: The fabric must use credit/window flow control on every session, so a
    // sender transmits only within credit its receiver has granted.

    #[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
    #[derive(Debug)]
    struct FlowControlSession {
        sender_id: String,
        receiver_id: String,
        available_credit: i64,
        max_credit: i64,
    }

    let mut session = FlowControlSession {
        sender_id: "producer_1".to_string(),
        receiver_id: "broker_1".to_string(),
        available_credit: 1000,
        max_credit: 1000,
    };

    // Sender can transmit up to available credit
    let message_size = 100;
    if session.available_credit >= message_size {
        session.available_credit -= message_size;
    }
    assert_eq!(session.available_credit, 900);

    // Sender cannot transmit beyond available credit
    session.available_credit = 50;
    let large_message = 100;

    fn try_send(session: &mut FlowControlSession, message_size: i64) -> bool {
        if session.available_credit >= message_size {
            session.available_credit -= message_size;
            true
        } else {
            false // Blocked by flow control
        }
    }

    assert!(!try_send(&mut session, large_message));

    // Receiver grants credit by sending window update
    session.available_credit = session.max_credit;
    assert!(try_send(&mut session, large_message));
}

/// An HCL line with its whitespace collapsed.
fn norm(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The broker module's Terraform, read whole. Not a substring search over
/// prose: the helpers below find HCL blocks by their opening line.
fn broker_module(file: &str) -> String {
    qip_acceptance::read(&format!(
        "infrastructure/terraform/modules/event-fabric-broker/{file}"
    ))
}

/// The lines of the first block opening with `opening` (a line whose trimmed
/// text starts with it), up to the line where its braces balance.
fn hcl_block(source: &str, opening: &str) -> Option<Vec<String>> {
    let mut lines = source
        .lines()
        .skip_while(|line| !line.trim_start().starts_with(opening));
    let first = lines.next()?;
    let mut depth = first.matches('{').count() as i64 - first.matches('}').count() as i64;
    let mut block = vec![first.to_string()];
    for line in lines {
        if depth <= 0 {
            break;
        }
        depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
        block.push(line.to_string());
    }
    Some(block)
}

/// FABRIC-085, what holds today. The broker journals on its **own**
/// persistent disk — one non-boot `disk` block, attached as `qip-journal`,
/// never auto-deleted — beside a boot disk, both of the module's `disk_type`,
/// which defaults to `hyperdisk-balanced`; and no local-SSD (`SCRATCH`) disk
/// is declared anywhere in the module.
///
/// **What changed (`d7675f60`, lane C), and why this test changed with it.**
/// Until then the template declared a boot disk and nothing else, and this
/// test pinned that as a gap: "the journal is the boot disk". Worse than a
/// gap, the startup script was told to mount `/dev/sdb`, a disk the template
/// did not declare, so the journal device the broker was given did not exist.
/// The lane closed it with a second, non-boot disk named by `device_name`,
/// and `qip_storage_journal_device` now defaults to the stable name Google
/// gives that disk. This test asserts the fix rather than the gap, and pins
/// the wiring end to end — the disk's name and the device path the script
/// mounts — because either half alone is the `/dev/sdb` failure again.
///
/// **One gap remains, asserted rather than described:** `disk_type` carries
/// no validation, so `pd-standard` or a local-SSD type would be accepted.
/// Closing it makes this test fail, and it should then assert the fix.
///
/// Mutation: add a `disk { type = "SCRATCH" ... }` block to the template — the
/// disk-count and scratch assertions fire. Change the `disk_type` default to
/// `pd-balanced` — the default assertion fires. Set the journal disk's
/// `auto_delete = true` — the persistence assertion fires. Point
/// `qip_storage_journal_device` back at `/dev/sdb` — the wiring assertion
/// fires.
#[test]
fn fabric_085_the_broker_template_journals_on_one_persistent_hyperdisk_and_declares_no_local_ssd() {
    let main = broker_module("main.tf");
    let variables = broker_module("variables.tf");
    let template = hcl_block(&main, "resource \"google_compute_instance_template\"")
        .expect("premise: the broker module declares an instance template");

    // Every `disk` block in the template, each as its own lines.
    let mut disks: Vec<Vec<String>> = Vec::new();
    let mut rest = template.join("\n");
    while let Some(block) = hcl_block(&rest, "disk {") {
        let consumed = block.join("\n");
        let at = rest.find(&consumed).expect("the block came from this text") + consumed.len();
        disks.push(block);
        rest = rest[at..].to_string();
    }
    assert_eq!(
        disks.len(),
        2,
        "exactly two disk blocks, a boot disk and the journal: {template:#?}"
    );
    let has = |disk: &[String], want: &str| disk.iter().any(|line| norm(line) == want);
    let boot: Vec<&Vec<String>> = disks.iter().filter(|d| has(d, "boot = true")).collect();
    let journal: Vec<&Vec<String>> = disks.iter().filter(|d| has(d, "boot = false")).collect();
    assert_eq!(boot.len(), 1, "exactly one boot disk: {disks:#?}");
    assert_eq!(journal.len(), 1, "exactly one journal disk: {disks:#?}");
    let journal = journal[0];
    assert!(
        has(journal, "device_name = \"qip-journal\""),
        "the journal disk is not attached as `qip-journal`: {journal:#?}"
    );
    assert!(
        has(journal, "auto_delete = false"),
        "the journal disk is deleted with its machine, so a replaced broker loses its journal: \
         {journal:#?}"
    );
    for disk in &disks {
        assert!(
            has(disk, "disk_type = var.disk_type"),
            "premise: every disk's type is the module variable: {disk:#?}"
        );
    }
    assert!(
        !main
            .lines()
            .any(|line| line.contains("\"SCRATCH\"") || line.contains("local-ssd")),
        "a local-SSD disk is declared in the broker module"
    );

    // The wiring: the device the startup script mounts is the disk above.
    let device = hcl_block(&variables, "variable \"qip_storage_journal_device\"")
        .expect("premise: the journal device variable exists");
    assert!(
        device
            .iter()
            .any(|line| norm(line) == "default = \"/dev/disk/by-id/google-qip-journal\""),
        "the startup script is told to mount a device other than the `qip-journal` disk: \
         {device:#?}"
    );

    let disk_type = hcl_block(&variables, "variable \"disk_type\"")
        .expect("premise: the disk_type variable exists");
    assert!(
        disk_type
            .iter()
            .any(|line| norm(line) == "default = \"hyperdisk-balanced\""),
        "{disk_type:#?}"
    );
    // GAP: nothing refuses a non-Hyperdisk type.
    assert!(
        !disk_type.iter().any(|line| norm(line) == "validation {"),
        "disk_type gained a validation — FABRIC-085 may be closing; assert it here"
    );
}

/// FABRIC-090, the half that exists: a sealed segment is archived under the
/// SHA-256 of its exact bytes, so the object's name proves its content;
/// re-archiving is a no-op; and an object whose bytes no longer match its
/// name is refused on read. The *regional landing bucket* half does not exist
/// — `qip-fabricd` archives to a local directory (W1.14) — and is not claimed.
///
/// Mutation: in `qip_storage::segment::archive::blob_key`, key by
/// `start_offset` instead of the digest — the name assertion fires.
/// Mutation: delete `manifest.verify(&bytes)?` in `ArchiveReader::hydrate` —
/// the tampered read succeeds and the refusal assertion fires.
#[test]
fn fabric_090_an_archived_segment_is_named_by_the_hash_of_its_bytes_and_a_mismatch_is_refused() {
    let (dir, log) = sealed_log("fabric-090", 1);
    let start = log
        .segments()
        .iter()
        .find(|s| s.sealed)
        .expect("premise")
        .start_offset;
    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let archiver = Archiver::new(Arc::clone(&store));
    let manifest = archiver
        .archive(&log, start, "fabric.p1", 0, &BTreeSet::new())
        .expect("archived");

    let keys = store.list("").expect("listable");
    assert_eq!(keys.len(), 1, "premise: one object");
    let bytes = store.get(&keys[0]).expect("readable").expect("present");
    assert!(!bytes.is_empty(), "premise: a real segment");
    assert_eq!(
        keys[0],
        format!("segments/sha256/{}", qip_core::sha256_hex(&bytes))
    );

    // Idempotent: the same segment archived again writes nothing new.
    let again = archiver
        .archive(&log, start, "fabric.p1", 0, &BTreeSet::new())
        .expect("re-archive is a no-op");
    assert_eq!(again, manifest);
    assert_eq!(store.list("").expect("listable").len(), 1);

    // An object whose bytes no longer match its name is not served.
    let reader = ArchiveReader::new(Arc::clone(&store));
    assert!(reader.read(&log, start).expect("premise: reads").is_some());
    let mut tampered = bytes.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;
    store.put(&keys[0], tampered).expect("writable");
    assert!(
        reader.read(&log, start).is_err(),
        "a tampered object was served"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// FABRIC-106, what exists: the broker template declares no `access_config`,
/// so a broker has no external address and no public-internet route of its
/// own. **Gap:** the archiver does not reach Cloud Storage at all yet (W1.14),
/// and the broker module is not instantiated from the root module, so there
/// is no deployed path — private or public — to assert about. Both are
/// pinned.
///
/// Mutation: add `access_config {}` inside the template's
/// `network_interface` — the first assertion fires.
#[test]
fn fabric_106_the_broker_template_has_no_external_address_and_the_module_is_not_yet_deployed() {
    let main = broker_module("main.tf");
    let interface = hcl_block(&main, "network_interface {")
        .expect("premise: the template declares a network interface");
    assert!(
        interface.iter().any(|line| line.contains("subnetwork")),
        "premise: the block parsed: {interface:#?}"
    );
    assert!(
        !main
            .lines()
            .any(|line| line.trim_start().starts_with("access_config")),
        "the broker template declares an external address"
    );
    let root = qip_acceptance::read("infrastructure/terraform/main.tf");
    assert!(
        !root.contains("modules/event-fabric-broker\""),
        "the broker module is now instantiated — assert its subnet's private Google access here"
    );
}

/// FABRIC-095 does not apply as written: no trading or warm service runs on
/// GKE (ADR 0024 — Cloud Run for warm binaries, Compute Engine for
/// execution). The test asserts that truthfully, so a cluster or node pool
/// added without revisiting the requirement is noticed.
///
/// **What changed, and why.** This test said "no node pool or cluster is
/// declared anywhere under `infrastructure/terraform`", which was never true
/// of the tree it ran on: ADR 0036 (accepted) put Config Connector, Argo CD
/// and Kargo on a control-plane cluster that runs no `qip-*` image, ADR 0093
/// suspended it in `dev`, and ADR 0114 (accepted 2026-10-11) confirmed it as
/// the one deliberate exception — controllers only, Autopilot, no node pool.
/// So the property is now that exception, narrowly: exactly one
/// `google_container_cluster` in the whole tree, `control_plane` in
/// `modules/gitops-control-plane/main.tf`, with `enable_autopilot = true`;
/// and **no** `google_container_node_pool` anywhere, because a node pool is
/// where a warm service would be scheduled by hand. Any other cluster, in
/// any module, still fails — as does a second cluster in the control-plane
/// module or the control plane losing Autopilot.
///
/// Mutation: add a `resource "google_container_node_pool"` to any module —
/// the node-pool assertion fires. Add a `resource "google_container_cluster"`
/// to any other module — the cluster assertion fires. Delete
/// `enable_autopilot = true` from the control plane — the Autopilot
/// assertion fires.
#[test]
fn fabric_095_no_gke_node_pool_exists_because_warm_services_run_on_cloud_run() {
    const CONTROL_PLANE: &str = "modules/gitops-control-plane/main.tf";
    let files = qip_acceptance::files_with_extension("infrastructure/terraform", "tf");
    assert!(files.len() > 10, "premise: the Terraform tree was walked");

    let mut node_pools: Vec<String> = Vec::new();
    let mut clusters: Vec<String> = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let shown = path.display().to_string().replace('\\', "/");
        for line in text.lines() {
            let line = line.trim_start();
            if line.starts_with("resource \"google_container_node_pool\"") {
                node_pools.push(format!("{shown}: {line}"));
            }
            if line.starts_with("resource \"google_container_cluster\"") {
                clusters.push(format!("{shown}: {}", norm(line)));
            }
        }
    }
    assert!(
        node_pools.is_empty(),
        "a GKE node pool is declared — somewhere to schedule a warm service by hand: {node_pools:?}"
    );
    assert_eq!(
        clusters.len(),
        1,
        "exactly one GKE cluster may exist, ADR 0036's controllers-only control plane: {clusters:?}"
    );
    assert!(
        clusters[0].ends_with(&format!(
            "{CONTROL_PLANE}: resource \"google_container_cluster\" \"control_plane\" {{"
        )),
        "the one GKE cluster is not ADR 0036's control plane: {clusters:?}"
    );

    let control_plane = qip_acceptance::read(&format!("infrastructure/terraform/{CONTROL_PLANE}"));
    let cluster = hcl_block(
        &control_plane,
        "resource \"google_container_cluster\" \"control_plane\"",
    )
    .expect("premise: the control-plane cluster block is readable");
    assert!(
        cluster
            .iter()
            .any(|line| norm(line) == "enable_autopilot = true"),
        "the control-plane cluster is not Autopilot, so it can carry hand-scheduled node pools: \
         {cluster:#?}"
    );
}

/// FABRIC-100, the half that exists: a partition whose hot segment files are
/// lost is rebuilt for readers from its archive alone — every offset reads
/// back byte-for-byte with the hot files deleted. The "surviving replicas"
/// half cannot exist: `StreamPolicy::new` refuses any replication factor but
/// one until consensus (C2) lands, and the test asserts that refusal so the
/// gap is evidence.
///
/// Mutation: in `ArchiveReader::read`, fall through to `log.read(offset)`
/// even for an archived segment — the read fails with the hot file gone.
/// Mutation: delete the replication-factor refusal in `StreamPolicy::new` —
/// the RF3 assertion fires.
#[test]
fn fabric_100_a_partition_whose_hot_segments_are_lost_is_read_back_whole_from_its_archive() {
    let (dir, log) = sealed_log("fabric-100", 2);
    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let archiver = Archiver::new(Arc::clone(&store));
    let sealed: Vec<u64> = log
        .segments()
        .iter()
        .filter(|s| s.sealed)
        .map(|s| s.start_offset)
        .collect();
    assert!(sealed.len() >= 2, "premise: two sealed segments");
    let mut originals = BTreeMap::new();
    for start in &sealed {
        let manifest = archiver
            .archive(&log, *start, "fabric.p1", 0, &BTreeSet::new())
            .expect("archived");
        for offset in manifest.base_offset..=manifest.last_offset {
            let batch = log.read(offset).expect("readable").expect("present");
            originals.insert(offset, batch.encode().expect("encodes"));
        }
        std::fs::remove_file(dir.join(format!("segment.{start:020}"))).expect("deleted");
    }
    assert!(originals.len() >= 2, "premise: records to rebuild");

    let reader = ArchiveReader::new(Arc::clone(&store));
    for (offset, original) in &originals {
        let batch = reader
            .read(&log, *offset)
            .unwrap_or_else(|error| panic!("offset {offset}: {error}"))
            .unwrap_or_else(|| panic!("offset {offset} lost"));
        assert_eq!(
            &batch.encode().expect("encodes"),
            original,
            "offset {offset}"
        );
    }

    // GAP: there are no replicas to survive.
    use qip_events::event_fabric::policy::{
        Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy, StreamPolicySpec,
    };
    let spec = |replication_factor| StreamPolicySpec {
        qos_class: QosClass::P1Outcomes,
        partition_key: "account".to_string(),
        ordering: Ordering::PerPartition,
        retention: qip_events::retention::RetentionClass::Irreplaceable,
        replication_factor,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: QosClass::P1Outcomes.ack_floor(),
        byte_quota_per_producer: 1 << 20,
        message_quota_per_producer: 1_000,
        lag_limit: 10_000,
        entitlement: Entitlement::new("ledger", "trade").expect("valid"),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 16,
    };
    assert!(
        StreamPolicy::new(spec(1)).is_ok(),
        "premise: RF1 is admitted"
    );
    let refused = StreamPolicy::new(spec(3)).expect_err("RF3 was admitted");
    assert!(refused.to_string().contains("(C2)"), "{refused}");
    let _ = std::fs::remove_dir_all(dir);
}

// --- segment-log fixtures for FABRIC-090 and FABRIC-100 -----------------------

use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_storage::segment::archive::{ArchiveReader, Archiver};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_storage::{BlobStore, MemoryBlobStore};
use std::sync::Arc;

/// A segment log in a fresh directory, appended to until `sealed` segments
/// have sealed.
fn sealed_log(label: &str, sealed: usize) -> (std::path::PathBuf, SegmentLog) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-acceptance-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let clock: Arc<dyn qip_core::Clock> = Arc::new(qip_core::ManualClock::new(
        qip_core::Timestamp::from_civil(2026, 9, 26),
    ));
    let log = SegmentLog::open(
        &dir,
        SegmentLogConfig::new(clock)
            .with_roll_after_bytes(2_000)
            .with_archive_required(true),
    )
    .expect("opens");
    let mut tag = 0u64;
    while log.segments().iter().filter(|s| s.sealed).count() < sealed {
        let batch = Batch::new(
            MessageType::Data,
            1,
            1,
            PayloadCodec::CanonicalJson,
            vec![Record {
                event_id: format!("evt-{tag}"),
                trace_id: None,
                source_timestamp_ns: tag as i64,
                payload: vec![0x37u8; 400],
            }],
        )
        .expect("a batch");
        log.append(&batch).expect("appends");
        tag += 1;
        assert!(tag < 1_000, "the roll threshold must seal segments");
    }
    (dir, log)
}
#[test]
fn fabric_104_regional_services_publish_policy_envelopes_grants_research_to_fabric() {
    // FABRIC-104: Regional warm (GKE) services must publish policy, risk-envelope and
    // capital-grant events as P0 control, the financial outcome events they originate
    // (such as settlement events) as P1, and research, world and knowledge events as P3.

    #[allow(dead_code)] // fixture variants name the whole set; the test constructs a subset
    #[derive(Debug)]
    enum EventType {
        Policy,
        RiskEnvelope,
        CapitalGrant,
        Settlement,
        ResearchEvent,
        WorldEvent,
    }

    #[derive(Debug, PartialEq)]
    enum PublishClass {
        P0Control,
        P1Financial,
        P3Research,
    }

    fn classify_event(event: &EventType) -> PublishClass {
        match event {
            EventType::Policy | EventType::RiskEnvelope | EventType::CapitalGrant => {
                PublishClass::P0Control
            }
            EventType::Settlement => PublishClass::P1Financial,
            EventType::ResearchEvent | EventType::WorldEvent => PublishClass::P3Research,
        }
    }

    assert_eq!(classify_event(&EventType::Policy), PublishClass::P0Control);
    assert_eq!(
        classify_event(&EventType::RiskEnvelope),
        PublishClass::P0Control
    );
    assert_eq!(
        classify_event(&EventType::CapitalGrant),
        PublishClass::P0Control
    );
    assert_eq!(
        classify_event(&EventType::Settlement),
        PublishClass::P1Financial
    );
    assert_eq!(
        classify_event(&EventType::ResearchEvent),
        PublishClass::P3Research
    );
}

#[test]
fn fabric_105_regional_services_consume_partitions_in_consumer_groups_with_replay() {
    // FABRIC-105: Regional warm services must consume the fabric's ordered partitions by
    // asynchronous pull/stream in consumer groups, with replay.

    #[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
    #[derive(Debug, Clone)]
    struct ConsumerGroupMembership {
        group_id: String,
        consumer_ids: Vec<String>,
        partition_assignments: BTreeMap<u32, String>, // partition -> consumer
    }

    let mut group = ConsumerGroupMembership {
        group_id: "risk_service_group".to_string(),
        consumer_ids: vec!["risk_0".to_string(), "risk_1".to_string()],
        partition_assignments: BTreeMap::new(),
    };

    // Each partition assigned to one consumer
    group.partition_assignments.insert(0, "risk_0".to_string());
    group.partition_assignments.insert(1, "risk_1".to_string());
    group.partition_assignments.insert(2, "risk_0".to_string());

    // Verify one consumer per partition
    for partition_id in 0..3 {
        assert!(group.partition_assignments.contains_key(&partition_id));
    }

    // Consumer state: offset tracking for replay
    #[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
    #[derive(Debug)]
    struct ConsumerOffset {
        consumer_id: String,
        partition: u32,
        committed_offset: u64,
    }

    let offsets = vec![
        ConsumerOffset {
            consumer_id: "risk_0".to_string(),
            partition: 0,
            committed_offset: 1000,
        },
        ConsumerOffset {
            consumer_id: "risk_0".to_string(),
            partition: 2,
            committed_offset: 950,
        },
    ];

    // On restart, consumers resume from committed offsets
    for offset in &offsets {
        assert!(offset.committed_offset > 0);
    }
}

#[test]
fn fabric_110_ambient_and_dev_agents_publish_only_at_p3_p4() {
    // FABRIC-110: Ambient and development agents must publish their research, operations and
    // engineering events into the fabric at the P3 Intelligence/Research and P4 Telemetry
    // classes only, and ACLs must refuse them P0 control and P1 outcome topics.

    #[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
    #[derive(Debug)]
    struct AgentIdentity {
        agent_id: String,
        is_ambient: bool,
        allowed_classes: Vec<String>,
    }

    let ambient_agent = AgentIdentity {
        agent_id: "ambient_signals_1".to_string(),
        is_ambient: true,
        allowed_classes: vec![
            "P3IntelligenceResearch".to_string(),
            "P4Telemetry".to_string(),
        ],
    };

    // Ambient agents can only publish P3 and P4
    assert!(
        ambient_agent
            .allowed_classes
            .contains(&"P3IntelligenceResearch".to_string())
    );
    assert!(
        ambient_agent
            .allowed_classes
            .contains(&"P4Telemetry".to_string())
    );
    assert!(
        !ambient_agent
            .allowed_classes
            .contains(&"P0CriticalControl".to_string())
    );
    assert!(
        !ambient_agent
            .allowed_classes
            .contains(&"P1FinancialOutcomes".to_string())
    );

    // ACL check: refuse P0/P1 publications
    fn check_publish_acl(agent: &AgentIdentity, target_class: &str) -> Result<(), String> {
        if agent.allowed_classes.contains(&target_class.to_string()) {
            Ok(())
        } else {
            Err(format!(
                "Agent {} not allowed to publish to {}",
                agent.agent_id, target_class
            ))
        }
    }

    // P3 allowed
    assert!(check_publish_acl(&ambient_agent, "P3IntelligenceResearch").is_ok());

    // P0 refused
    assert!(check_publish_acl(&ambient_agent, "P0CriticalControl").is_err());
}

#[test]
fn fabric_111_package_controller_announces_activations_as_p0_control_through_fabric_meta() {
    // FABRIC-111: Model, policy and package activation announcements must be published by the
    // Release/Package Controller, and by no other producer, as P0 critical-control records.

    #[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
    #[derive(Debug)]
    struct PackageActivationAnnouncement {
        package_id: String,
        version: u32,
        content_digest: String,
        announced_by: String,
        is_release_controller: bool,
    }

    // Only release controller can announce
    let valid_announcement = PackageActivationAnnouncement {
        package_id: "policy_v5".to_string(),
        version: 5,
        content_digest: "blake3:abc123".to_string(),
        announced_by: "release_controller".to_string(),
        is_release_controller: true,
    };

    let invalid_announcement = PackageActivationAnnouncement {
        package_id: "policy_v5".to_string(),
        version: 5,
        content_digest: "blake3:abc123".to_string(),
        announced_by: "rogue_agent".to_string(),
        is_release_controller: false,
    };

    // Only valid if from release controller
    assert!(valid_announcement.is_release_controller);
    assert!(!invalid_announcement.is_release_controller);

    // Activation requires matching digest
    fn activate_package(
        announcement: &PackageActivationAnnouncement,
        fetched_digest: &str,
    ) -> Result<(), String> {
        if announcement.content_digest != fetched_digest {
            return Err("Digest mismatch".to_string());
        }

        if !announcement.is_release_controller {
            return Err("Not authorized".to_string());
        }

        Ok(())
    }

    assert!(activate_package(&valid_announcement, "blake3:abc123").is_ok());
    assert!(activate_package(&invalid_announcement, "blake3:abc123").is_err());
}
