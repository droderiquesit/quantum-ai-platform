//! Fixtures shared by `event_fabric_scaling.rs` and
//! `event_fabric_infrastructure.rs`: a real broker in a fresh directory, the
//! batches a drain would stamp, and the small HCL reader both files use to
//! judge Terraform by its blocks rather than by substrings of its prose.
//!
//! Each of the two test binaries compiles this module on its own and uses a
//! different part of it, so unused items are expected rather than a defect.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy, StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_events::retention::RetentionClass;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

pub(crate) const STREAM: &str = "fills";

/// A policy a class permits, at the replication factor the in-tree broker
/// can honour (one) and the class's own acknowledgement floor.
pub(crate) fn spec(class: QosClass) -> StreamPolicySpec {
    StreamPolicySpec {
        qos_class: class,
        partition_key: "account".to_string(),
        ordering: Ordering::PerPartition,
        retention: RetentionClass::Irreplaceable,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: class
            .required_overload_policy()
            .unwrap_or(OverloadPolicy::AllowBacklog),
        ack_profile: class.ack_floor(),
        byte_quota_per_producer: 1_000_000,
        message_quota_per_producer: 1_000,
        lag_limit: 1_000,
        entitlement: Entitlement::new("ledger", "trade").expect("valid"),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 20,
    }
}

/// A fresh directory unique to this call.
pub(crate) fn fresh_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-acceptance-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A broker in a fresh directory with `STREAM` declared at `partitions`
/// partitions under `policy`, and the one schema `batch` writes registered.
/// Returns the broker's data directory beside it.
pub(crate) fn broker_with(
    label: &str,
    partitions: u32,
    policy: StreamPolicySpec,
) -> (Broker, PathBuf) {
    let dir = fresh_dir(label);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)));
    let broker = Broker::open(dir.clone(), Arc::clone(&clock)).unwrap();
    broker
        .declare_stream(
            STREAM,
            partitions,
            StreamPolicy::new(policy).unwrap(),
            SegmentLogConfig::new(clock).with_roll_after_bytes(10_000_000),
        )
        .unwrap();
    broker
        .register_schema(STREAM, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    (broker, dir)
}

/// A batch of `ids`, stamped as `producer` at `epoch` from `sequence`, each
/// record carrying `payload`.
pub(crate) fn batch_of(
    producer: &str,
    epoch: u64,
    sequence: u64,
    ids: &[&str],
    payload: &[u8],
) -> Batch {
    let records = ids
        .iter()
        .map(|id| Record {
            event_id: (*id).to_string(),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: payload.to_vec(),
        })
        .collect();
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        records,
    )
    .expect("a batch");
    stamp_drain(&mut batch, producer, epoch, sequence);
    batch
}

/// A one-record batch with a small fixed payload.
pub(crate) fn batch(producer: &str, epoch: u64, sequence: u64, id: &str) -> Batch {
    batch_of(producer, epoch, sequence, &[id], b"{\"qty\":1}")
}

// --- Terraform, read by block -------------------------------------------------

/// An HCL line with its whitespace collapsed.
pub(crate) fn norm(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The lines of the first block opening with `opening` (a line whose trimmed
/// text starts with it), up to the line where its braces balance.
pub(crate) fn hcl_block(source: &str, opening: &str) -> Option<Vec<String>> {
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

/// Whether `block` holds a line equal to `want` once whitespace is collapsed.
/// A whole-line comparison, never a substring: `access_config` is a prefix of
/// nothing here, but `type = "A"` is a prefix of `type = "AAAA"`.
pub(crate) fn has_line(block: &[String], want: &str) -> bool {
    block.iter().any(|line| norm(line) == want)
}

/// Every `resource "<kind>" "<name>" {` opening in `source`, as
/// `(kind, name)`. A resource is found by its quoted pair, so a resource
/// named `x_tcp` is never taken for one named `x`.
pub(crate) fn resources(source: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in source.lines() {
        let line = norm(line);
        let Some(rest) = line.strip_prefix("resource \"") else {
            continue;
        };
        let Some((kind, rest)) = rest.split_once("\" \"") else {
            continue;
        };
        let Some((name, _)) = rest.split_once('"') else {
            continue;
        };
        found.push((kind.to_string(), name.to_string()));
    }
    found
}

/// The broker module's Terraform, read whole.
pub(crate) fn broker_module(file: &str) -> String {
    qip_acceptance::read(&format!(
        "infrastructure/terraform/modules/event-fabric-broker/{file}"
    ))
}

/// Every Terraform file under `infrastructure/`, with its text.
pub(crate) fn all_terraform() -> Vec<(String, String)> {
    qip_acceptance::files_with_extension("infrastructure", "tf")
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            Some((path.display().to_string().replace('\\', "/"), text))
        })
        .collect()
}
