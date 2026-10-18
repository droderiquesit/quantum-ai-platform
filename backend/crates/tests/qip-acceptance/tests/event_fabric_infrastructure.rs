//! Event Fabric infrastructure and integration: FABRIC-065 through FABRIC-111.
//!
//! Metadata separation, admin tooling, partitioning, flow control, storage, node deployment,
//! disaster recovery, and service integration contracts.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

mod event_fabric_support;

use event_fabric_support::{
    STREAM, batch, batch_of, broker_module, broker_with, hcl_block, norm, spec,
};
use qip_events::Topic;
use qip_events::event_fabric::catalogue::{Catalogue, Permission};
use qip_events::event_fabric::policy::QosClass;
use qip_streaming::event_fabric::group::GroupCoordinator;
use qip_streaming::event_fabric::partition::partition_for;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::consumer::{Consumer, ConsumerConfig};
use qip_transport::event_fabric::protocol::{FetchResponse, Refusal, Request, Response, Route};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::retry::{RecordingSleeper, RetryPolicy};
use std::sync::Mutex;

/// Every file under `dir`, as `(path relative to dir, bytes)`.
fn files_under(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).expect("a readable directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path.strip_prefix(dir).expect("under dir").to_path_buf();
                found.push((relative, std::fs::read(&path).expect("a readable file")));
            }
        }
    }
    found
}

/// The top-level directories under `dir` whose files contain `marker`.
/// Compared as a whole set by the callers, never by `contains` on a path:
/// `metadata` is a prefix of nothing here, but a path test would not know it.
fn holders_of(dir: &std::path::Path, marker: &[u8]) -> BTreeSet<String> {
    files_under(dir)
        .into_iter()
        .filter(|(_, bytes)| bytes.windows(marker.len()).any(|window| window == marker))
        .filter_map(|(relative, _)| {
            relative
                .components()
                .next()
                .map(|first| first.as_os_str().to_string_lossy().into_owned())
        })
        .collect()
}

/// FABRIC-065, the half that exists: the broker keeps its own facts — the
/// leader epoch, every isolation in force, the operator log — in a store of
/// its own (`metadata/`), consumer checkpoints in another (`consumers/`), and
/// a partition's batches in `streams/`. A marker written through each door
/// is found in that door's directory and in no other, and the metadata door
/// still answers while the partition's data door refuses writes. The earlier
/// version of this test built a struct with a `metadata_log` field and
/// asserted that a string in it did not contain `"metadata"`; it imported
/// nothing from the platform.
///
/// **Gap, not claimed:** the requirement says the metadata lives in a
/// *quorum's replicated* log. The broker's metadata store is one node's disk
/// (ADR 0100 §3: no in-tree consensus); membership, partition maps and
/// leases-as-metadata are not fabric records at all today. What is proven is
/// the separation, not the replication.
///
/// Mutation: in `Broker::open`, open the metadata store under
/// `data_dir.join("streams")` instead of `data_dir.join("metadata")` — the
/// operator marker is then found under `streams` and the holder-set
/// assertion fires.
#[test]
fn fabric_065_the_brokers_metadata_lives_in_stores_of_its_own_and_never_in_a_partitions_data_log() {
    const OPERATOR: &[u8] = b"operator-marker-7f3a19";
    const REASON: &[u8] = b"reason-marker-91c2d4";
    const GROUP: &str = "group-marker-55d1e8";
    const PAYLOAD: &[u8] = b"payload-marker-c0ffee42";
    let (broker, dir) = broker_with("fabric-065", 1, spec(QosClass::P1Outcomes));

    broker
        .produce(
            STREAM,
            "acct",
            batch_of("venue", 1, 0, &["fill-a"], PAYLOAD),
        )
        .expect("premise: the data door admits a batch");
    broker
        .commit_offset(GROUP, STREAM, 0, 0)
        .expect("premise: a checkpoint commits");
    broker
        .isolate(
            STREAM,
            0,
            std::str::from_utf8(OPERATOR).unwrap(),
            std::str::from_utf8(REASON).unwrap(),
        )
        .expect("premise: the partition isolates");

    let only = |name: &str| BTreeSet::from([name.to_string()]);
    assert_eq!(holders_of(&dir, OPERATOR), only("metadata"));
    assert_eq!(holders_of(&dir, REASON), only("metadata"));
    assert_eq!(holders_of(&dir, GROUP.as_bytes()), only("consumers"));
    assert_eq!(holders_of(&dir, PAYLOAD), only("streams"));

    // The data door is shut and the metadata door is not.
    let refused = match broker
        .produce_to(STREAM, 0, batch("venue", 1, 1, "fill-b"))
        .expect("a refusal is an answer, not an error")
    {
        Err(refused) => refused,
        Ok(_) => panic!("a parked partition admitted a write"),
    };
    assert!(
        matches!(refused.refusal, Refusal::Isolated { .. }),
        "{:?}",
        refused.refusal
    );
    assert!(
        broker.isolation(STREAM, 0).unwrap().is_some(),
        "the isolation could not be read back while the partition refused writes"
    );
    broker
        .release(STREAM, 0, "second-operator")
        .expect("the metadata door releases a partition whose data door is shut");
    assert!(
        broker
            .produce_to(STREAM, 0, batch("venue", 1, 1, "fill-b"))
            .unwrap()
            .is_ok(),
        "released, the partition still refuses writes"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// FABRIC-070, the half that exists: an operator can park a partition and
/// release it, each durable and attributed, the parked partition stays
/// readable and its siblings stay writable; retention is enforced only
/// against an archive that can hydrate what it deletes. **Gap, not claimed:**
/// moving a partition, draining a broker's leadership, repairing an
/// under-replicated partition and offline compaction have no operation — the
/// protocol's only admin routes are isolate and release, asserted below so a
/// third one makes this test say so.
///
/// Mutation: in `Broker::release`, delete the `self.metadata.delete(..)` of
/// the isolation key — the partition stays parked and the post-release
/// produce assertion fires.
#[test]
fn fabric_070_an_operator_can_park_and_release_a_partition_but_nothing_moves_drains_or_repairs_one()
{
    let (broker, dir) = broker_with("fabric-070", 2, spec(QosClass::P1Outcomes));
    for (partition, id) in [(0, "fill-a"), (1, "fill-b")] {
        assert!(
            broker
                .produce_to(STREAM, partition, batch("venue", 1, 0, id))
                .unwrap()
                .is_ok(),
            "premise: partition {partition} admits a write before any isolation"
        );
    }

    // Refused: an isolation nobody can attribute or explain.
    assert!(broker.isolate(STREAM, 0, "", "disk repair").is_err());
    assert!(broker.isolate(STREAM, 0, "alice", "  ").is_err());
    assert_eq!(broker.isolation(STREAM, 0).unwrap(), None);

    let at = broker.isolate(STREAM, 0, "alice", "disk repair").unwrap();
    assert_eq!(at, 1, "parked at the offset the partition had reached");

    let refused = match broker
        .produce_to(STREAM, 0, batch("venue", 1, 1, "fill-c"))
        .unwrap()
    {
        Err(refused) => refused,
        Ok(_) => panic!("a parked partition admitted a write"),
    };
    assert_eq!(
        refused.refusal,
        Refusal::Isolated {
            operator: "alice".to_string(),
            reason: "disk repair".to_string()
        }
    );
    assert!(
        broker
            .produce_to(STREAM, 1, batch("venue", 1, 1, "fill-d"))
            .unwrap()
            .is_ok(),
        "isolating partition 0 stopped partition 1"
    );
    let parked_read = broker.fetch(STREAM, 0, 0, 1 << 20).unwrap();
    assert!(
        !parked_read.batches().is_empty(),
        "the retained record stopped being readable while parked"
    );

    // A second isolation would overwrite the first one's reason.
    let second = broker
        .isolate(STREAM, 0, "mallory", "again")
        .expect_err("a second isolation was admitted");
    assert!(second.to_string().contains("by 'alice'"), "{second}");

    assert!(broker.release(STREAM, 0, "").is_err());
    assert!(
        broker.release(STREAM, 1, "bob").is_err(),
        "released a partition that was never isolated"
    );
    broker.release(STREAM, 0, "bob").unwrap();
    assert!(
        broker
            .produce_to(STREAM, 0, batch("venue", 1, 1, "fill-c"))
            .unwrap()
            .is_ok(),
        "released, the partition still refuses writes"
    );
    let log: Vec<(String, String)> = broker
        .admin_log()
        .unwrap()
        .into_iter()
        .map(|entry| (entry.action, entry.operator))
        .collect();
    assert_eq!(
        log,
        vec![
            ("isolate".to_string(), "alice".to_string()),
            ("release".to_string(), "bob".to_string())
        ]
    );

    // Retention is enforced against an archive, never against nothing.
    let retention = broker
        .retain_hot(10)
        .expect_err("retention ran with no archive to hydrate what it deleted");
    assert!(
        retention.to_string().contains("no archive reader"),
        "{retention}"
    );

    // GAP: the admin surface is isolate and release.
    assert_eq!(Route::ALL.len(), 9, "premise: the whole route table");
    let admin: Vec<&str> = Route::ALL
        .iter()
        .map(Route::path)
        .filter(|path| path.contains("/admin/"))
        .collect();
    assert_eq!(
        admin,
        vec![
            "/v1/event-fabric/admin/isolate",
            "/v1/event-fabric/admin/release"
        ],
        "an admin route was added — assert move, drain, repair or compaction here"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// FABRIC-075: a produce names an ordering key, the key routes to one
/// partition through `partition_for`, and every record of one key lands in
/// that partition. Driven through the broker: 24 keys over 8 partitions, each
/// produced twice, then each partition's high watermark read back and
/// compared with what the router said it would hold. The earlier version
/// hashed with `DefaultHasher` inside the test and compared a function to
/// itself.
///
/// Mutation: in `Broker::produce`, route `stream` instead of `key` through
/// `partition_for` — every key lands on one partition and the acknowledgement
/// assertion fires.
#[test]
fn fabric_075_every_record_of_one_ordering_key_lands_in_the_partition_the_router_names() {
    const PARTITIONS: u32 = 8;
    let (broker, dir) = broker_with("fabric-075", PARTITIONS, spec(QosClass::P1Outcomes));
    let keys: Vec<String> = (0..24).map(|n| format!("acct-{n}")).collect();

    let routed: Vec<u32> = keys
        .iter()
        .map(|key| partition_for(key, PARTITIONS).expect("a non-zero count routes"))
        .collect();
    let distinct: BTreeSet<u32> = routed.iter().copied().collect();
    assert!(
        distinct.len() >= 4,
        "premise: the keys spread over several partitions, else same-partition proves nothing: \
         {routed:?}"
    );
    assert!(routed.iter().all(|partition| *partition < PARTITIONS));

    let mut expected: BTreeMap<u32, u64> = BTreeMap::new();
    for (key, partition) in keys.iter().zip(&routed) {
        for round in 0..2 {
            let producer = format!("producer-{key}-{round}");
            let ack = broker
                .produce(
                    STREAM,
                    key,
                    batch(&producer, 1, 0, &format!("{key}-{round}")),
                )
                .expect("the key routes and the batch is admitted");
            assert_eq!(
                ack.partition(),
                *partition,
                "{key} round {round} was acknowledged by a partition the router did not name"
            );
            *expected.entry(*partition).or_default() += 1;
        }
    }
    for partition in 0..PARTITIONS {
        let held = broker.metadata(STREAM, partition).unwrap().high_watermark();
        assert_eq!(
            held,
            expected.get(&partition).copied().unwrap_or(0),
            "partition {partition} holds a different number of batches than its keys produced"
        );
    }

    // Refused: no partition to route to, a partition the stream lacks, a
    // stream nobody declared.
    assert!(partition_for("acct-0", 0).is_err());
    let beyond = broker
        .produce_to(STREAM, PARTITIONS, batch("p", 1, 0, "x"))
        .expect_err("a partition the stream does not have was written");
    assert!(beyond.to_string().contains("does not exist"), "{beyond}");
    assert!(
        broker
            .produce("undeclared", "acct-0", batch("p", 1, 0, "x"))
            .is_err()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A transport that answers every fetch with "nothing here" and remembers the
/// requests it was asked, so a test can read what credit each carried.
#[derive(Debug)]
struct RecordingTransport {
    seen: Arc<Mutex<Vec<Request>>>,
}

impl FabricTransport for RecordingTransport {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> qip_core::error::Result<Response> {
        self.seen.lock().unwrap().push(request.clone());
        match request {
            Request::Fetch(fetch) => Ok(Response::Fetch(FetchResponse::new(
                fetch.stream,
                fetch.partition,
                0,
                0,
                String::new(),
            )?)),
            other => Err(qip_core::error::Error::invalid(format!(
                "test bug: only fetch is answered, got {other:?}"
            ))),
        }
    }
}

fn consumer_with_credit(
    credit: u32,
    seen: &Arc<Mutex<Vec<Request>>>,
) -> qip_core::error::Result<Consumer> {
    Consumer::new(ConsumerConfig {
        transport: Box::new(RecordingTransport {
            seen: Arc::clone(seen),
        }),
        stream: STREAM.to_string(),
        partition: 0,
        group_id: "risk-desk".to_string(),
        retry_policy: RetryPolicy::default(),
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(qip_core::time::SystemClock),
        sleeper: Arc::new(RecordingSleeper::new()),
        retry_seed: 1,
        breaker_seed: 1,
        timeouts: Timeouts::default(),
        fetch_credit_bytes: credit,
    })
}

/// FABRIC-080: a session paces the sender by a credit the receiver grants.
/// Two ends are driven. The consumer's end: every fetch it sends carries
/// exactly the configured credit as its `max_bytes` — fixed at construction,
/// not a constant and not per call — and a zero credit is refused at
/// construction. The broker's end: a fetch is answered within the credit it
/// was given (one batch at minimum, so a single oversized batch still makes
/// progress), never past it. The earlier version moved integers in a struct
/// it had defined itself.
///
/// Mutation: in `Consumer::fetch`, send `max_bytes: u32::MAX` — the request
/// assertion fires. Mutation: in `Broker::fetch`, delete the
/// `encoded.len() + bytes.len() > max_bytes` break — the second batch is
/// served past a credit that cannot hold it and the broker assertion fires.
/// Mutation: delete the zero-credit refusal in `Consumer::new` — the
/// refusal assertion fires.
#[test]
fn fabric_080_a_fetch_is_paced_by_the_credit_the_receiver_granted_at_both_ends() {
    for credit in [4096_u32, 512] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut consumer = consumer_with_credit(credit, &seen).expect("a credit builds");
        for _ in 0..3 {
            assert!(consumer.fetch().unwrap().is_none());
        }
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3, "premise: three fetches reached the wire");
        for request in requests.iter() {
            let Request::Fetch(fetch) = request else {
                panic!("a non-fetch request reached the wire: {request:?}");
            };
            assert_eq!(fetch.max_bytes, credit, "the request exceeded its credit");
        }
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let refused = match consumer_with_credit(0, &seen) {
        Ok(_) => panic!("a consumer with no credit was built"),
        Err(error) => error,
    };
    assert!(
        refused.to_string().contains("zero-byte fetch credit"),
        "{refused}"
    );

    // The broker's end.
    let (broker, dir) = broker_with("fabric-080", 1, spec(QosClass::P1Outcomes));
    for sequence in 0..3 {
        broker
            .produce_to(
                STREAM,
                0,
                batch("venue", 1, sequence, &format!("fill-{sequence}")),
            )
            .unwrap()
            .expect("premise: a batch is admitted");
    }
    // Each batch's own size, in bytes: `batches()` is hex, two characters a
    // byte, and a credit of one byte still serves one whole batch.
    let sizes: Vec<u32> = (0..3)
        .map(|offset| {
            let alone = broker.fetch(STREAM, 0, offset, 1).unwrap();
            assert!(
                !alone.batches().is_empty(),
                "premise: a credit of one byte still serves the batch at {offset}"
            );
            (alone.batches().len() / 2) as u32
        })
        .collect();
    let first_two = sizes[0] + sizes[1];
    let two = broker.fetch(STREAM, 0, 0, first_two).unwrap();
    assert_eq!(
        two.batches().len() / 2,
        first_two as usize,
        "credit for exactly two batches serves exactly two"
    );
    let tight = broker.fetch(STREAM, 0, 0, first_two - 1).unwrap();
    assert_eq!(
        tight.batches().len() / 2,
        sizes[0] as usize,
        "a second batch was served past the credit that could not hold it"
    );
    let ample = broker.fetch(STREAM, 0, 0, u32::MAX).unwrap();
    assert_eq!(
        ample.batches().len() / 2,
        sizes.iter().sum::<u32>() as usize,
        "premise: all three exist"
    );
    let _ = std::fs::remove_dir_all(dir);
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
/// **The last gap closed with FABRIC-8097 (lane L22, 2026-10-18).** This
/// test pinned `disk_type` carrying no validation, so `pd-standard` or a
/// local-SSD type would have been accepted. The journal's IOPS and throughput
/// are now provisioned, which only a Hyperdisk accepts, and `disk_type`
/// refuses anything but a `hyperdisk-*` type; the pin became the assertion.
///
/// Mutation: add a `disk { type = "SCRATCH" ... }` block to the template — the
/// disk-count and scratch assertions fire. Change the `disk_type` default to
/// `pd-balanced` — the default assertion fires. Set the journal disk's
/// `auto_delete = true` — the persistence assertion fires. Point
/// `qip_storage_journal_device` back at `/dev/sdb` — the wiring assertion
/// fires. Delete the `disk_type` hyperdisk validation, or the journal's
/// `provisioned_iops` line — the refusal or the provisioning assertion fires.
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
    // The journal's IOPS and throughput are provisioned (FABRIC-8097), which
    // only a Hyperdisk accepts, so a persistent-disk type is refused at plan
    // rather than at apply. This pinned the absence of that refusal until it
    // landed.
    assert!(
        disk_type
            .iter()
            .any(|line| norm(line) == "condition = startswith(var.disk_type, \"hyperdisk-\")"),
        "nothing refuses a non-Hyperdisk disk_type: {disk_type:#?}"
    );
    for want in [
        "provisioned_iops = var.journal_provisioned_iops",
        "provisioned_throughput = var.journal_provisioned_throughput_mibps",
    ] {
        assert!(
            has(journal, want),
            "the journal disk does not carry `{want}`: {journal:#?}"
        );
    }
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
fn fabric_106_the_broker_template_has_no_external_address_and_the_module_is_instantiated_but_off_in_every_environment()
 {
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

    // Where the module is instantiated: `fabric.tf`, not the root `main.tf`
    // (an earlier version of this test looked in `main.tf`, found nothing and
    // concluded the module was undeployed, which was never what that file
    // could show). It is gated on a variable that defaults to null...
    let norm = |line: &str| line.split_whitespace().collect::<Vec<_>>().join(" ");
    let fabric = qip_acceptance::read("infrastructure/terraform/fabric.tf");
    // Scoped to the module's own block: other resources in this file carry the
    // same `count` expression, and an assertion over the whole file would pass
    // with this module's gate deleted.
    let module: Vec<String> = fabric
        .lines()
        .skip_while(|line| !line.starts_with("module \"event_fabric_broker\" {"))
        .take_while(|line| !line.starts_with("}"))
        .map(norm)
        .collect();
    assert!(
        module
            .iter()
            .any(|line| line == "source = \"./modules/event-fabric-broker\""),
        "premise: fabric.tf instantiates the broker module: {module:?}"
    );
    assert!(
        module
            .iter()
            .any(|line| line == "count = var.event_fabric_broker == null ? 0 : 1"),
        "the broker module is no longer gated on var.event_fabric_broker being set: {module:?}"
    );
    let variables = qip_acceptance::read("infrastructure/terraform/variables.tf");
    let declared: Vec<String> = variables
        .lines()
        .skip_while(|line| !line.starts_with("variable \"event_fabric_broker\""))
        .take_while(|line| !line.starts_with("}"))
        .map(norm)
        .collect();
    assert!(
        !declared.is_empty(),
        "premise: the variable is declared in variables.tf"
    );
    assert!(
        declared.iter().any(|line| line == "default = null"),
        "the broker is on by default now: {declared:?}"
    );
    // ...and no environment sets it, so it is off everywhere.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = qip_acceptance::read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        assert!(
            !tfvars.lines().any(|line| {
                let line = line.trim_start();
                !line.starts_with('#') && line.starts_with("event_fabric_broker")
            }),
            "{environment} now sets event_fabric_broker, so a broker is declared there; assert its \
             subnet's private Google access and the plan here"
        );
    }
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
// --- the committed stream catalogue, as the broker reads it ---------------------

/// The committed catalogue as JSON, for a test to add a grant to before the
/// production parser judges it.
fn committed_catalogue_json() -> serde_json::Value {
    serde_json::from_str(&qip_acceptance::read(
        "infrastructure/event-fabric/streams.local.json",
    ))
    .expect("the committed catalogue is JSON")
}

fn parse_catalogue(document: &serde_json::Value) -> qip_core::error::Result<Catalogue> {
    Catalogue::parse(document.to_string().as_bytes())
}

/// The committed catalogue with one grant added.
fn with_grant(
    identity: &str,
    stream: &str,
    permission: &str,
    key_scope: &str,
) -> serde_json::Value {
    let mut document = committed_catalogue_json();
    document["grants"]
        .as_array_mut()
        .expect("grants is a list")
        .push(serde_json::json!({
            "identity": identity,
            "stream": stream,
            "permission": permission,
            "key_scope": key_scope,
        }));
    document
}

/// FABRIC-104, what the platform classifies and what it does not. The topic
/// a regional service publishes carries its class in `Topic::pclass`, and the
/// committed catalogue — the table the broker is started from — must agree
/// with it wherever both speak. Driven: policy distribution is P0, settlement
/// P1, the research and knowledge topics P3, and every committed stream
/// carries only topics of its own class (a declared gap, which a stream
/// writes in whatever class it fell in, is the one named exception). The
/// earlier version defined `classify_event` in the test body and asserted its
/// own `match`.
///
/// **Gaps, pinned so closing one is noticed.** The requirement also names
/// risk-envelope and capital-grant events as P0, and the catalogue does carry
/// them on `control.local` — but `Topic::pclass` returns `None` for
/// `risk.approved` and `system.kill_switch_engaged`, so the two tables
/// disagree about what is P0. And no committed stream is P3 or carries
/// `settlement.recorded`: the classes exist on the topics, the streams that
/// would carry research and settlement do not. No regional warm service
/// publishes to the fabric at all (`bindings.rs` fixes six reflex topics).
///
/// Mutation: in `Topic::pclass`, class `PolicyDistributed` as P1 — the policy
/// assertion and the catalogue agreement both fire. Mutation: class
/// `SettlementRecorded` as P2 — the settlement assertion fires.
#[test]
fn fabric_104_policy_settlement_and_research_topics_are_classed_and_the_catalogue_agrees() {
    assert_eq!(
        Topic::PolicyDistributed.pclass(),
        Some(QosClass::P0Control),
        "policy is a control record"
    );
    assert_eq!(
        Topic::SettlementRecorded.pclass(),
        Some(QosClass::P1Outcomes),
        "settlement is a financial outcome"
    );
    for topic in [
        Topic::WorldModelSnapshot,
        Topic::OutcomeObserved,
        Topic::HypothesisScored,
        Topic::ModelEvaluated,
        Topic::LearningCompleted,
    ] {
        assert_eq!(topic.pclass(), Some(QosClass::P3Research), "{topic}");
    }

    let catalogue = parse_catalogue(&committed_catalogue_json())
        .expect("premise: the committed catalogue parses");
    assert!(catalogue.streams().len() >= 3, "premise: streams declared");
    let mut checked = 0;
    let mut disagreements = Vec::new();
    for (name, stream) in catalogue.streams() {
        for topic in &stream.topics {
            let Some(class) = topic.pclass() else {
                continue;
            };
            checked += 1;
            // A declared gap is written on whichever stream it fell in.
            if class != stream.policy.qos_class() && *topic != Topic::EventFabricGap {
                disagreements.push(format!(
                    "{topic} is {class:?} but {name} is a different class"
                ));
            }
        }
    }
    assert!(
        checked >= 4,
        "premise: the agreement was checked on real topics: {checked}"
    );
    assert!(disagreements.is_empty(), "{disagreements:?}");
    let control = catalogue
        .stream("control.local")
        .expect("premise: control stream");
    assert_eq!(control.policy.qos_class(), QosClass::P0Control);
    assert!(control.topics.contains(&Topic::PolicyDistributed));

    // GAP: the catalogue calls these P0, the topic table does not classify them.
    for topic in [Topic::RiskApproved, Topic::KillSwitchEngaged] {
        assert!(
            control.topics.contains(&topic),
            "premise: the catalogue carries {topic} on the P0 stream"
        );
        assert_eq!(
            topic.pclass(),
            None,
            "{topic} is now classed by Topic::pclass — assert P0 here"
        );
    }
    // GAP: no stream carries settlement or research.
    let carried: BTreeSet<Topic> = catalogue
        .streams()
        .values()
        .flat_map(|stream| stream.topics.iter().copied())
        .collect();
    assert!(!carried.is_empty(), "premise: some topics are carried");
    assert!(
        !carried.contains(&Topic::SettlementRecorded),
        "a committed stream now carries settlement — assert it is P1"
    );
    assert!(
        catalogue
            .streams()
            .values()
            .all(|stream| stream.policy.qos_class() != QosClass::P3Research),
        "a P3 stream is now declared — assert research topics ride it"
    );
}

/// FABRIC-105: a consumer group gives each partition to exactly one member,
/// a commit from a membership that has since changed is refused, and a
/// committed offset neither rewinds nor stops a reader replaying from an
/// earlier one. Driven through the broker's own `GroupCoordinator` and
/// `Broker`. The earlier version inserted into a `BTreeMap` and asserted the
/// keys were there.
///
/// Mutation: in `GroupCoordinator::admit_commit`, delete the
/// `generation != current` refusal — the stale commit is admitted and the
/// fencing assertion fires. Mutation: in `Broker::commit_offset`, delete the
/// `offset < held` refusal — the rewind is admitted and the checkpoint
/// assertion fires.
#[test]
fn fabric_105_a_group_gives_each_partition_to_one_member_fences_stale_commits_and_replays() {
    let (broker, dir) = broker_with("fabric-105", 3, spec(QosClass::P1Outcomes));
    let now = 1_000_000_000;
    let mut coordinator = GroupCoordinator::new(broker.leader_epoch(), 30_000_000_000).unwrap();
    let all = [0_u32, 1, 2];

    let alone = coordinator
        .join(now, "risk-svc", "risk", STREAM, Some("risk_0"), &all)
        .unwrap();
    assert_eq!(
        alone.partitions,
        vec![0, 1, 2],
        "premise: a lone member holds all"
    );
    let pair = coordinator
        .join(now, "risk-svc", "risk", STREAM, Some("risk_1"), &all)
        .unwrap();
    let members = coordinator.members("risk", STREAM);
    assert_eq!(members.len(), 2, "premise: two members");
    let mut held: Vec<u32> = members.values().flatten().copied().collect();
    held.sort_unstable();
    assert_eq!(held, vec![0, 1, 2], "every partition held once, none twice");
    assert!(members.values().all(|partitions| !partitions.is_empty()));

    // The first member's generation is stale now that the group changed.
    let stale = coordinator
        .admit_commit(
            now,
            "risk-svc",
            "risk",
            STREAM,
            "risk_0",
            alone.generation,
            0,
        )
        .expect_err("a commit from a superseded generation was admitted");
    assert!(stale.to_string().contains("rejoin"), "{stale}");
    let mine = members["risk_1"][0];
    let theirs = members["risk_0"][0];
    coordinator
        .admit_commit(
            now,
            "risk-svc",
            "risk",
            STREAM,
            "risk_1",
            pair.generation,
            mine,
        )
        .expect("a current member commits a partition it holds");
    assert!(
        coordinator
            .admit_commit(
                now,
                "risk-svc",
                "risk",
                STREAM,
                "risk_1",
                pair.generation,
                theirs
            )
            .is_err(),
        "a member committed a partition another member holds"
    );

    // Replay: the checkpoint moves forward only, and the log is still there.
    for sequence in 0..3 {
        broker
            .produce_to(
                STREAM,
                0,
                batch("venue", 1, sequence, &format!("fill-{sequence}")),
            )
            .unwrap()
            .expect("premise: a batch is admitted");
    }
    assert_eq!(broker.committed_offset("risk", STREAM, 0).unwrap(), None);
    broker.commit_offset("risk", STREAM, 0, 1).unwrap();
    assert_eq!(broker.committed_offset("risk", STREAM, 0).unwrap(), Some(1));
    assert!(
        broker.commit_offset("risk", STREAM, 0, 0).is_err(),
        "the checkpoint was rewound"
    );
    assert!(
        broker.commit_offset("risk", STREAM, 0, 3).is_err(),
        "a checkpoint was committed past what was ever written"
    );
    assert_eq!(broker.committed_offset("risk", STREAM, 0).unwrap(), Some(1));
    let sizes: Vec<usize> = (0..3)
        .map(|offset| broker.fetch(STREAM, 0, offset, 1).unwrap().batches().len())
        .collect();
    assert!(sizes.iter().all(|size| *size > 0), "premise: three batches");
    let replay = broker.fetch(STREAM, 0, 0, u32::MAX).unwrap();
    assert_eq!(
        replay.batches().len(),
        sizes.iter().sum::<usize>(),
        "replay from offset zero, behind the checkpoint, returns all three batches"
    );
    let resumed = broker.fetch(STREAM, 0, 2, u32::MAX).unwrap();
    assert_eq!(
        resumed.batches().len(),
        sizes[2],
        "resuming at the checkpoint plus one returns the one batch not yet committed"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// FABRIC-110: an ambient or development agent publishes at P3 and P4 and no
/// higher, and the refusal is in the catalogue's parser, so a catalogue that
/// carries the offending grant does not parse and no broker starts on it.
/// Each case adds one grant to the *committed* catalogue and hands it to the
/// production parser. Premises first: the unmodified catalogue parses; the
/// agent is admitted a produce on telemetry (P4) and a consume on outcomes
/// (P1); and a non-agent identity is admitted the very produce on P1 the
/// agent is refused — so each refusal below is about the identity and the
/// class together, not about a grant shape the parser dislikes. The earlier
/// version asserted that a `Vec<String>` it had just built lacked a string.
///
/// Prefix trap: `agents:x` and `agent-x` do not start with `agent:` and are
/// not caught — that is the parser's rule, not a hole this test hides; the
/// identity prefix is matched with its delimiter and only the delimited form
/// is asserted.
///
/// Mutation: in `Catalogue::grant_from_raw`, let the agent rule's class test
/// also admit `QosClass::P1Outcomes` — the P1 refusal fires.
#[test]
fn fabric_110_an_agent_identity_is_refused_produce_and_admin_above_research_and_telemetry() {
    parse_catalogue(&committed_catalogue_json()).expect("premise: the committed catalogue parses");
    parse_catalogue(&with_grant(
        "agent:ambient-1",
        "telemetry.local",
        "produce",
        "any",
    ))
    .expect("premise: an agent may publish telemetry");
    parse_catalogue(&with_grant(
        "agent:ambient-1",
        "reflex-outcomes.local",
        "consume",
        "any",
    ))
    .expect("premise: an agent may read outcomes");
    parse_catalogue(&with_grant(
        "drain-writer",
        "reflex-outcomes.local",
        "produce",
        "any",
    ))
    .expect("premise: a non-agent identity may produce to P1");

    let above = [
        ("control.local", "p0_control"),
        ("reflex-outcomes.local", "p1_outcomes"),
        ("reflex-journal.local", "p2_market_journal"),
    ];
    for (stream, class) in above {
        for permission in ["produce", "admin"] {
            let refused =
                parse_catalogue(&with_grant("agent:ambient-1", stream, permission, "any"))
                    .expect_err(&format!("an agent was granted {permission} on {stream}"));
            let message = refused.to_string();
            assert!(
                message.contains(&format!("may hold '{permission}' only on a p3_research")),
                "{message}"
            );
            assert!(
                message.contains(&format!("not on the {class} stream '{stream}'")),
                "{message}"
            );
        }
    }
}

/// FABRIC-111: only the release controller produces to a P0 control stream.
/// The committed catalogue's produce grants on `control.local` are exactly
/// `{release-controller}`, and the production parser refuses a produce grant
/// to anyone else — a cell, an operator, a drain, and the identity
/// `release-controller-2`, which contains the controller's name and is not
/// it (a substring test would admit it). **Gap, pinned:** no `Topic` names a
/// model, policy or package *activation*, so what the controller announces
/// is `policy.distributed` and the two control records beside it, not an
/// activation record; and nothing in the tree is the Release/Package
/// Controller beyond its identity string in this catalogue.
///
/// Mutation: in `Catalogue::grant_from_raw`, compare
/// `identity.starts_with("release-controller")` instead of `!=` — the
/// `release-controller-2` refusal fires. Mutation: delete the P0-produce
/// refusal — every refusal below fires.
#[test]
fn fabric_111_only_the_release_controller_may_produce_to_the_p0_control_stream() {
    let catalogue = parse_catalogue(&committed_catalogue_json())
        .expect("premise: the committed catalogue parses");
    let producers: BTreeSet<&str> = catalogue
        .grants()
        .iter()
        .filter(|grant| grant.stream == "control.local" && grant.permission == Permission::Produce)
        .map(|grant| grant.identity.as_str())
        .collect();
    assert_eq!(producers, BTreeSet::from(["release-controller"]));
    assert_eq!(
        catalogue
            .stream("control.local")
            .unwrap()
            .policy
            .qos_class(),
        QosClass::P0Control,
        "premise: the stream is P0"
    );

    for identity in [
        "reflex:cell-local",
        "operator",
        "drain-writer",
        "release-controller-2",
    ] {
        let key_scope = if identity.starts_with("reflex:") {
            "own_key"
        } else {
            "any"
        };
        let refused = parse_catalogue(&with_grant(identity, "control.local", "produce", key_scope))
            .expect_err(&format!("{identity} was granted produce on the P0 stream"));
        assert!(
            refused.to_string().contains(&format!(
                "only 'release-controller' may produce to the P0 stream 'control.local', not '{identity}'"
            )),
            "{refused}"
        );
    }
    // The controller itself, and a consume by anyone, are unaffected.
    parse_catalogue(&with_grant(
        "release-controller",
        "control.local",
        "produce",
        "any",
    ))
    .expect("premise: the controller may produce");
    parse_catalogue(&with_grant(
        "drain-writer",
        "control.local",
        "consume",
        "any",
    ))
    .expect("premise: consuming control is not producing it");

    // GAP: no topic is an activation announcement.
    let activation: Vec<&str> = Topic::ALL
        .iter()
        .map(Topic::name)
        .filter(|name| {
            name.split(['.', '_'])
                .any(|word| word == "activation" || word == "package")
        })
        .collect();
    assert!(!Topic::ALL.is_empty(), "premise: topics were enumerated");
    assert!(
        activation.is_empty(),
        "a topic now names activation or package ({activation:?}) — assert it is P0 and \
         controller-only"
    );
}
