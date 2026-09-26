//! The broker library: partitions over segment logs, a static leader epoch,
//! schema admission and `archived_through` — ADR 0100 §§1, 3 and 4.
//!
//! Each test below is paired with the mutation the packet named for it in
//! its own doc comment, so a reviewer breaking the implementation the way
//! the comment describes should see exactly this test fail, not a different
//! one.

#![allow(clippy::panic_in_result_fn)]

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
        broker.declare_stream(stream, 1, config(10_000_000))?;
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
    broker.declare_stream(stream, 1, config(10_000_000))?;
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
    broker.declare_stream(stream, 1, config(200))?;
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
    broker.declare_stream(stream, 1, config(10_000_000))?;
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
    broker.declare_stream(stream, 1, config(10_000_000))?;
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
    broker.declare_stream(stream, partition_count, config(10_000_000))?;
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
    broker.declare_stream(stream, 1, config(10_000_000))?;
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
