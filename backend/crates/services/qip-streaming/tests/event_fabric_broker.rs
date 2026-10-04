//! The broker library: partitions over segment logs, a static leader epoch,
//! schema admission and `archived_through` — ADR 0100 §§1, 3 and 4.
//!
//! Each test below is paired with the mutation the packet named for it in
//! its own doc comment, so a reviewer breaking the implementation the way
//! the comment describes should see exactly this test fail, not a different
//! one.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::error::Result;
use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{
    Batch, DecodeOutcome, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::partition;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A directory this test alone owns, under the system temporary directory.
fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-broker-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A fully declared policy: the one thing `Broker::declare_stream` takes in
/// place of the defaults a broker would otherwise supply (FABRIC-057).
fn policy() -> qip_events::event_fabric::policy::StreamPolicy {
    policy_with_lag(1_000)
}

fn policy_with_lag(lag_limit: u64) -> qip_events::event_fabric::policy::StreamPolicy {
    use qip_events::event_fabric::policy::{
        AckProfile, Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, StreamPolicy,
        StreamPolicySpec,
    };
    StreamPolicy::new(StreamPolicySpec {
        qos_class: qip_events::event_fabric::policy::QosClass::P1Outcomes,
        partition_key: "account".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::EventAnchored,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: AckProfile::Quorum,
        byte_quota_per_producer: 1_048_576,
        message_quota_per_producer: 10_000,
        lag_limit,
        entitlement: Entitlement::new("internal-reflex", "trade").unwrap(),
        seal_age_ms: 500,
        peak_bytes_per_second: 5_000_000,
    })
    .unwrap()
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 9, 26)))
}

fn config(roll_after_bytes: u64) -> SegmentLogConfig {
    SegmentLogConfig::new(clock()).with_roll_after_bytes(roll_after_bytes)
}

/// A schema shape simple enough that nothing in this suite depends on its
/// content, only that it is registered.
fn sample_shape() -> Shape {
    Shape::of(&1u32).expect("a plain integer always serialises to a shape")
}

/// A one-record batch, drain-stamped as ADR 0100 §4 requires before it can
/// ever reach [`Broker::produce`].
fn drain_batch(producer_id: &str, epoch: u64, sequence: u64, tag: u64, payload: &[u8]) -> Batch {
    let record = Record {
        event_id: format!("evt-{tag}"),
        trace_id: None,
        source_timestamp_ns: tag as i64,
        payload: payload.to_vec(),
    };
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .expect("a one-record batch is always constructible");
    stamp_drain(&mut batch, producer_id, epoch, sequence);
    batch
}

/// Decode only the first batch a `fetch` response's hex-encoded `batches`
/// carries. `Batch::decode` reads exactly one batch's own declared length
/// and never looks past it, so this is correct even when a response
/// concatenates several — the assertions below only ever check one offset
/// at a time.
fn first_batch(hex: &str) -> Batch {
    let bytes = qip_core::hash::from_hex(hex).expect("fetch always returns valid hex");
    match Batch::decode(&bytes).expect("fetch never returns a corrupt batch") {
        DecodeOutcome::Complete(batch) => batch,
        DecodeOutcome::Torn => panic!("expected at least one complete batch in a non-empty fetch"),
    }
}

// --- a_restarted_broker_serves_every_acknowledged_record_byte_for_byte_from_offset_zero

/// FABRIC-006. Mutation: in `Broker::produce`'s success arm, stop calling
/// `PartitionLog::append` and acknowledge the predicted offset anyway —
/// exactly the M3 defect ("a batch counted as durable before it actually
/// was") that skipping `fsync` produces, reached through the one seam this
/// packet's own code controls rather than through `SegmentLog`'s internals
/// (SLICE-16, already mutation-tested for exactly this there).
#[test]
fn a_restarted_broker_serves_every_acknowledged_record_byte_for_byte_from_offset_zero() -> Result<()>
{
    let dir = temp_dir("restart");
    let stream = "orders";
    let mut payloads = Vec::new();
    {
        let broker = Broker::open(&dir, clock())?;
        broker.declare_stream(stream, 1, policy(), config(10_000_000))?;
        broker.register_schema(stream, 1, 1, sample_shape())?;
        for i in 0..6u64 {
            let payload = format!("payload-{i:03}").into_bytes();
            let batch = drain_batch("producer-a", 1, i, i, &payload);
            let ack = broker.produce(stream, "same-key", batch)?;
            assert_eq!(
                ack.base_offset(),
                i,
                "premise: offsets are assigned densely from zero"
            );
            payloads.push(payload);
        }
    } // the broker drops here, as a clean process exit would

    assert!(
        !payloads.is_empty(),
        "premise: something was actually produced before the restart"
    );

    let broker = Broker::open(&dir, clock())?;
    broker.declare_stream(stream, 1, policy(), config(10_000_000))?;
    let partition = partition::partition_for("same-key", 1)?;

    for (offset, expected) in payloads.iter().enumerate() {
        let response = broker.fetch(stream, partition, offset as u64, 65_536)?;
        let batch = first_batch(response.batches());
        assert_eq!(
            &batch.records[0].payload, expected,
            "offset {offset} must read back byte for byte after a restart"
        );
    }
    Ok(())
}

// --- every_broker_start_bumps_and_persists_the_leader_epoch_before_it_accepts_a_produce

/// Mutation: move the epoch bump from `Broker::open` into `Broker::produce`,
/// run lazily on the first produce call instead. This test never calls
/// `produce` at all, so a broker that only bumps there would report the same
/// epoch on every restart.
#[test]
fn every_broker_start_bumps_and_persists_the_leader_epoch_before_it_accepts_a_produce() -> Result<()>
{
    let dir = temp_dir("epoch");
    let epoch1 = Broker::open(&dir, clock())?.leader_epoch();
    let epoch2 = Broker::open(&dir, clock())?.leader_epoch();
    let epoch3 = Broker::open(&dir, clock())?.leader_epoch();

    assert!(
        epoch1 >= 1,
        "premise: opening a broker assigns a real, non-zero epoch"
    );
    assert_eq!(
        epoch2,
        epoch1 + 1,
        "a second start, with no produce call ever made, must still have bumped and persisted \
         a new epoch"
    );
    assert_eq!(epoch3, epoch2 + 1);
    Ok(())
}

// --- a_produce_acknowledgement_and_the_metadata_both_report_archived_through_for_the_partition

/// FABRIC B3. Mutation: `PartitionLog::archived_through` returns
/// `self.high_water()` unconditionally — reporting the just-acked offset as
/// archived rather than deriving from the segment log's own archive marks.
#[test]
fn a_produce_acknowledgement_and_the_metadata_both_report_archived_through_for_the_partition()
-> Result<()> {
    let dir = temp_dir("archived-through");
    let stream = "journal";
    let broker = Broker::open(&dir, clock())?;
    // A small roll threshold so a handful of batches actually seal segments.
    broker.declare_stream(stream, 1, policy(), config(200))?;
    broker.register_schema(stream, 1, 1, sample_shape())?;
    let partition = partition::partition_for("k", 1)?;

    let mut tag = 0u64;
    while broker.sealed_segment_starts(stream, partition)?.len() < 2 {
        let batch = drain_batch("p", 1, tag, tag, &[0x42u8; 64]);
        broker.produce(stream, "k", batch)?;
        tag += 1;
        assert!(
            tag < 10_000,
            "the roll threshold in this test must be small enough to reach two sealed segments"
        );
    }

    let sealed = broker.sealed_segment_starts(stream, partition)?;
    assert!(
        sealed.len() >= 2,
        "premise: at least two segments are actually sealed before archiving anything"
    );
    let first_sealed = sealed[0];

    let before = broker.metadata(stream, partition)?;
    assert_eq!(
        before.archived_through(),
        0,
        "premise: nothing has been marked archived yet"
    );

    broker.mark_archived(stream, partition, first_sealed)?;

    let batch = drain_batch("p", 1, tag, tag, &[0x99u8; 8]);
    let ack = broker.produce(stream, "k", batch)?;
    let after = broker.metadata(stream, partition)?;

    assert!(
        ack.archived_through() > 0,
        "the acknowledgement must report the archive mark that now exists"
    );
    assert_eq!(
        ack.archived_through(),
        after.archived_through(),
        "the produce acknowledgement and the metadata answer must report the same \
         archived_through for the same partition"
    );
    assert!(
        ack.archived_through() < ack.base_offset(),
        "archived_through must reflect only the segment actually marked archived, never the \
         offset the batch that triggered this check was itself just acked at"
    );
    Ok(())
}

// --- a_fetch_never_passes_the_last_fsynced_offset

/// M3. The structural guarantee — `PartitionLog::read` refuses any offset at
/// or past `high_water` — is SLICE-16's own, already mutation-tested there.
/// The one fact this packet's own code computes that is not already gated a
/// layer down is the `high_watermark` number `fetch` reports alongside the
/// data; mutating the fetch loop's own bound to "serve appended offsets" is
/// unobservable through this broker, because `PartitionLog::read` would
/// still correctly answer `None` for an offset nothing has fsynced —
/// there is no phantom batch on disk for a wider loop bound to reach. The
/// mutation below is the provably observable stand-in: `Broker::fetch`
/// reports `high_watermark + 1` instead of the true value.
#[test]
fn a_fetch_never_passes_the_last_fsynced_offset() -> Result<()> {
    let dir = temp_dir("fetch-boundary");
    let stream = "ticks";
    let broker = Broker::open(&dir, clock())?;
    broker.declare_stream(stream, 1, policy(), config(10_000_000))?;
    broker.register_schema(stream, 1, 1, sample_shape())?;
    let partition = partition::partition_for("k", 1)?;

    let batch = drain_batch("p", 1, 0, 0, b"only-record");
    broker.produce(stream, "k", batch)?;

    let high_water = broker.metadata(stream, partition)?.high_watermark();
    assert_eq!(
        high_water, 1,
        "premise: exactly one record has actually been acknowledged"
    );

    let at_boundary = broker.fetch(stream, partition, high_water, 65_536)?;
    assert_eq!(
        at_boundary.high_watermark(),
        high_water,
        "fetch's own reported watermark must match what was actually fsynced"
    );
    assert_eq!(
        at_boundary.batches(),
        "",
        "a fetch starting exactly at the high watermark must return nothing: there is nothing \
         past it that has been fsynced"
    );

    let before_boundary = broker.fetch(stream, partition, 0, 65_536)?;
    let served = first_batch(before_boundary.batches());
    assert_eq!(
        served.records[0].payload, b"only-record",
        "the one record that actually was fsynced must still be served"
    );
    Ok(())
}

// --- an_unregistered_schema_is_refused_before_any_consumer_can_fetch_it

/// FABRIC-024. Mutation: `Broker::require_registered_schema` always returns
/// `Ok(())`, admitting any `(schema_id, schema_version)`.
#[test]
fn an_unregistered_schema_is_refused_before_any_consumer_can_fetch_it() -> Result<()> {
    let dir = temp_dir("schema-refusal");
    let stream = "orders";
    let broker = Broker::open(&dir, clock())?;
    broker.declare_stream(stream, 1, policy(), config(10_000_000))?;
    let partition = partition::partition_for("k", 1)?;

    let batch = drain_batch("p", 1, 0, 0, b"unregistered-schema-payload");
    let result = broker.produce(stream, "k", batch);
    assert!(
        result.is_err(),
        "a produce naming a schema this stream never registered must be refused"
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("no registered schema"),
        "the refusal must name why: {message}"
    );

    let after_refusal = broker.fetch(stream, partition, 0, 65_536)?;
    assert_eq!(
        after_refusal.high_watermark(),
        0,
        "the refused batch must never have been appended, so nothing is fetchable"
    );
    assert_eq!(after_refusal.batches(), "");

    broker.register_schema(stream, 1, 1, sample_shape())?;
    let batch2 = drain_batch("p", 1, 0, 0, b"now-registered-payload");
    let ack = broker.produce(stream, "k", batch2)?;
    assert_eq!(
        ack.base_offset(),
        0,
        "once the schema is registered, the same shape of batch is accepted from offset zero"
    );
    Ok(())
}

// --- records_sharing_a_key_share_a_partition_in_produce_order

/// Mutation: `Broker::produce` ignores `key` and assigns partitions
/// round-robin (an incrementing counter modulo the partition count) instead
/// of calling `partition::partition_for`.
#[test]
fn records_sharing_a_key_share_a_partition_in_produce_order() -> Result<()> {
    let dir = temp_dir("partition-routing");
    let stream = "orders";
    let broker = Broker::open(&dir, clock())?;
    let partition_count = 4;
    broker.declare_stream(stream, partition_count, policy(), config(10_000_000))?;
    broker.register_schema(stream, 1, 1, sample_shape())?;

    let key_a = "account-alpha";
    let key_b = "account-beta";
    let partition_a = partition::partition_for(key_a, partition_count)?;
    let partition_b = partition::partition_for(key_b, partition_count)?;
    assert_ne!(
        partition_a, partition_b,
        "premise: this test's two keys actually hash to different partitions"
    );

    let mut acks_a = Vec::new();
    for i in 0..5u64 {
        let batch = drain_batch("producer-a", 1, i, i, format!("a-{i}").as_bytes());
        acks_a.push(broker.produce(stream, key_a, batch)?);
    }
    assert!(
        !acks_a.is_empty(),
        "premise: multiple records were actually produced under the same key"
    );
    for ack in &acks_a {
        assert_eq!(
            ack.partition(),
            partition_a,
            "every produce under the same key must land on the same partition, never spread \
             round-robin across them"
        );
    }
    let offsets: Vec<u64> = acks_a.iter().map(|a| a.base_offset()).collect();
    assert_eq!(
        offsets,
        vec![0, 1, 2, 3, 4],
        "records sharing a key must append densely and in produce order within their shared \
         partition"
    );

    let batch_b = drain_batch("producer-b", 1, 0, 0, b"b-0");
    let ack_b = broker.produce(stream, key_b, batch_b)?;
    assert_eq!(ack_b.partition(), partition_b);
    assert_eq!(
        ack_b.base_offset(),
        0,
        "a different partition has its own independent dense stream"
    );
    Ok(())
}

// --- a_read_only_opener_reads_what_a_running_broker_wrote_and_holds_no_lock

/// Mutation: `ReadOnlyPartition::read_from` opens each segment file with
/// `OpenOptions::new().append(true)` instead of `.read(true)` — literally
/// opening the segment for append.
#[test]
fn a_read_only_opener_reads_what_a_running_broker_wrote_and_holds_no_lock() -> Result<()> {
    let dir = temp_dir("read-only");
    let stream = "orders";
    let broker = Broker::open(&dir, clock())?;
    broker.declare_stream(stream, 1, policy(), config(10_000_000))?;
    broker.register_schema(stream, 1, 1, sample_shape())?;
    let partition = partition::partition_for("k", 1)?;

    let mut payloads = Vec::new();
    for i in 0..4u64 {
        let payload = format!("live-{i}").into_bytes();
        let batch = drain_batch("p", 1, i, i, &payload);
        broker.produce(stream, "k", batch)?;
        payloads.push(payload);
    }
    assert!(
        !payloads.is_empty(),
        "premise: the still-running broker actually wrote something for the reader to find"
    );

    // The broker above is still open here — its `SegmentLog` still holds
    // this process's one-writer-per-directory guard on this partition — while
    // this reader reads the very same directory.
    let reader = partition::open_read_only(&dir, stream, partition)?;
    let batches = reader.read_from(0)?;
    assert_eq!(
        batches.len(),
        payloads.len(),
        "every record the still-running broker wrote must already be visible to the reader"
    );
    for (batch, expected) in batches.iter().zip(payloads.iter()) {
        assert_eq!(&batch.records[0].payload, expected);
    }

    // The broker's own next append must still succeed: the read above must
    // have taken no lock and no write handle that could have blocked it.
    let more = drain_batch("p", 1, 4, 4, b"after-the-read");
    let ack = broker.produce(stream, "k", more)?;
    assert_eq!(
        ack.base_offset(),
        4,
        "the running broker's own next append must not be refused by a concurrent read-only read"
    );
    Ok(())
}

// --- FABRIC-075: the property over any keys and any produce order

/// FABRIC-075's stated check: for any set of keys and any sequence of
/// produces, all records sharing a key sit in one partition and appear in
/// produce order. The two-key test above fixes one example; this one lets a
/// seeded generator interleave twelve keys across four partitions, so a
/// router that is only stable for the keys somebody thought to try (a
/// per-call counter, a time-seeded hash) is caught by the keys nobody did.
///
/// Mutation: in `partition::partition_for`, mix a process-wide counter into
/// the hash (`hashed.wrapping_add(COUNTER.fetch_add(1, ..))`) — fails,
/// because one key then lands in more than one partition.
#[test]
fn for_any_interleaving_of_keys_every_key_stays_in_one_partition_and_in_produce_order() -> Result<()>
{
    use qip_core::{Rng, Xoshiro256};
    use std::collections::BTreeMap;

    const KEYS: usize = 12;
    const PRODUCES: usize = 120;
    let dir = temp_dir("key-property");
    let stream = "orders";
    let partition_count = 4;
    let broker = Broker::open(&dir, clock())?;
    broker.declare_stream(stream, partition_count, policy(), config(10_000_000))?;
    broker.register_schema(stream, 1, 1, sample_shape())?;

    let keys: Vec<String> = (0..KEYS).map(|i| format!("account-{i}")).collect();
    let mut rng = Xoshiro256::seeded(75);
    // key -> (partition seen, base offsets in produce order)
    let mut seen: BTreeMap<String, (u32, Vec<u64>)> = BTreeMap::new();
    // One producer per key, so each key's own sequence is dense and the
    // property is about routing rather than about producer bookkeeping.
    let mut next_sequence: BTreeMap<String, u64> = BTreeMap::new();

    for tag in 0..PRODUCES as u64 {
        let key = keys[rng.below(KEYS as u64) as usize].clone();
        let sequence = next_sequence.entry(key.clone()).or_insert(0);
        let batch = drain_batch(
            &format!("producer-{key}"),
            1,
            *sequence,
            tag,
            format!("{key}-{tag}").as_bytes(),
        );
        *sequence += 1;
        let ack = broker.produce(stream, &key, batch)?;
        let entry = seen
            .entry(key.clone())
            .or_insert_with(|| (ack.partition(), Vec::new()));
        assert_eq!(
            entry.0,
            ack.partition(),
            "key {key} was produced to two different partitions"
        );
        assert_eq!(
            ack.partition(),
            partition::partition_for(&key, partition_count)?,
            "the broker must route by the declared function of the key and nothing else"
        );
        entry.1.push(ack.base_offset());
    }

    // Premise: the generator actually spread keys over several partitions
    // and used most of the keys, so "one partition per key" is a claim that
    // could have failed.
    let partitions_used: std::collections::BTreeSet<u32> = seen.values().map(|(p, _)| *p).collect();
    assert!(
        partitions_used.len() >= 2,
        "the keys all hashed to one partition"
    );
    assert!(seen.len() >= KEYS / 2, "too few keys were exercised");

    for (key, (_, offsets)) in &seen {
        assert!(
            offsets.windows(2).all(|w| w[0] < w[1]),
            "key {key}'s records must append in produce order, got offsets {offsets:?}"
        );
    }
    Ok(())
}

// --- FABRIC-016: checkpoints survive a restart and any retained offset can be replayed

/// Every batch offset `from..high_watermark` of `(stream, 0)` as
/// `(offset, event_id)`, read back through `fetch` exactly as a consumer
/// would.
fn read_event_ids(broker: &Broker, stream: &str, from: u64) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut offset = from;
    loop {
        let response = broker.fetch(stream, 0, offset, 1_000_000).unwrap();
        let hex = response.batches().to_string();
        if hex.is_empty() {
            return out;
        }
        let bytes = qip_core::hash::from_hex(&hex).unwrap();
        match Batch::decode(&bytes).unwrap() {
            DecodeOutcome::Complete(batch) => {
                out.push((offset, batch.records[0].event_id.clone()));
                offset += 1;
            }
            DecodeOutcome::Torn => panic!("a fetch returned a torn batch at offset {offset}"),
        }
    }
}

/// FABRIC-016's stated check: a consumer that commits offset N and is killed
/// resumes at N+1, with only the uncommitted tail redelivered, across a
/// broker restart; and a fresh consumer sought to any retained offset reads
/// the records the original read.
///
/// Mutation: in `Broker::commit_offset`, skip the `self.checkpoints.put`
/// (acknowledge without storing) — fails at the first
/// `committed_offset` assertion, which then reads `None`.
#[test]
fn a_group_committing_n_resumes_at_n_plus_one_after_a_broker_restart_and_a_fresh_consumer_can_replay_from_any_offset()
-> Result<()> {
    let dir = temp_dir("checkpoint");
    let stream = "orders";
    let group = "sink-a";
    let seen_by_original;
    {
        let broker = Broker::open(&dir, clock())?;
        broker.declare_stream(stream, 1, policy(), config(10_000_000))?;
        broker.register_schema(stream, 1, 1, sample_shape())?;
        for i in 0..10u64 {
            broker.produce(
                stream,
                "k",
                drain_batch("producer-a", 1, i, i, format!("p-{i}").as_bytes()),
            )?;
        }
        seen_by_original = read_event_ids(&broker, stream, 0);
        // Premise: the original consumer saw all ten, so "resume at 5" has a
        // tail of five to redeliver and not nothing.
        assert_eq!(seen_by_original.len(), 10);

        assert_eq!(broker.committed_offset(group, stream, 0)?, None);
        assert_eq!(broker.commit_offset(group, stream, 0, 4)?, 4);
        assert_eq!(broker.committed_offset(group, stream, 0)?, Some(4));
        // A rewind is refused, and so is a commit past what exists.
        assert!(broker.commit_offset(group, stream, 0, 3).is_err());
        assert!(broker.commit_offset(group, stream, 0, 10).is_err());
        assert_eq!(broker.committed_offset(group, stream, 0)?, Some(4));
        // Another group's checkpoint is independent.
        assert_eq!(broker.committed_offset("sink-b", stream, 0)?, None);
    } // the broker and the consumer are "killed".

    let broker = Broker::open(&dir, clock())?;
    broker.declare_stream(stream, 1, policy(), config(10_000_000))?;
    broker.register_schema(stream, 1, 1, sample_shape())?;
    let committed = broker
        .committed_offset(group, stream, 0)?
        .expect("the checkpoint must survive a restart");
    assert_eq!(committed, 4);

    let resumed = read_event_ids(&broker, stream, committed + 1);
    assert_eq!(
        resumed,
        seen_by_original[5..].to_vec(),
        "resuming at N+1 must redeliver exactly the uncommitted tail, no gap and no overlap"
    );

    for from in [0u64, 2, 7, 9] {
        assert_eq!(
            read_event_ids(&broker, stream, from),
            seen_by_original[from as usize..].to_vec(),
            "a fresh consumer sought to offset {from} must read what the original read"
        );
    }
    Ok(())
}

// --- FABRIC-057: a stream is declared with its whole policy or not at all

/// FABRIC-057's creation check at the broker: a stream is declared with a
/// `StreamPolicy` (so no declaration can be left to a default — the call
/// does not compile without one), the stored definition returns every
/// declared value, and redeclaring under a different policy is refused.
///
/// Mutation: in `Broker::declare_stream`, delete the `existing.policy !=
/// policy` refusal — fails at the redeclaration assertion, because the
/// stream is then silently re-promised under a weaker policy.
#[test]
fn a_declared_stream_returns_its_whole_policy_and_cannot_be_redeclared_under_another() -> Result<()>
{
    let broker = Broker::open(temp_dir("policy"), clock())?;
    let declared = policy();
    broker.declare_stream("orders", 2, declared.clone(), config(10_000_000))?;

    let stored = broker.stream_policy("orders")?;
    assert_eq!(
        stored, declared,
        "the stored definition must round-trip every declared value"
    );
    assert_eq!(stored.lag_limit(), 1_000);
    assert_eq!(stored.partition_key(), "account");

    // Premise: the second policy really differs, so a refusal below is about
    // the policy and not about an identical redeclaration.
    let weaker = policy_with_lag(5);
    assert_ne!(weaker, declared);
    let err = broker
        .declare_stream("orders", 2, weaker, config(10_000_000))
        .expect_err("a different policy must be refused");
    assert!(err.to_string().contains("different stream policy"), "{err}");
    assert_eq!(broker.stream_policy("orders")?, declared);

    // The same policy again is idempotent, as a restarted caller's is.
    broker.declare_stream("orders", 2, declared, config(10_000_000))?;
    assert!(broker.stream_policy("never-declared").is_err());
    Ok(())
}
