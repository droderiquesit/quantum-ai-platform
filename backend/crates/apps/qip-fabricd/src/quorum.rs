//! The quorum acknowledgement, timed where it happens (R12-OBS-9146, ADR 0111
//! Amendment 1).
//!
//! A P0 or P1 produce is acknowledged only after the managed log commits it,
//! so the managed commit's latency is the fabric's quorum-acknowledgement
//! latency: the one number that says how long a fill waited for "survives a
//! zone". The broker core holds a [`ManagedLog`] and never knows what is
//! behind it, and it carries no metrics dependency, so the timing is a
//! decorator the composition root wraps around whichever log it opened —
//! Spanner in a deployment, the in-memory log in a test — and every commit
//! the broker makes goes through it.
//!
//! Before this the commit was timed by nothing: the produce path's own
//! latency histogram folds the local append, the archive wait and the commit
//! into one figure, so a Spanner instance that had become slow read as a slow
//! broker, and the publish-quorum objective had no series to read.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use qip_core::Clock;
use qip_core::error::Result;
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::policy::QosClass;
use qip_streaming::event_fabric::managed::{
    CommitOutcome, CommittedBatch, Lease, LeaseOutcome, ManagedLog, PartitionRef,
};

use crate::telemetry::{FabricdTelemetry, quorum_outcome};

/// The class label of a stream the applied catalogue does not declare. Kept
/// as one literal so a stream name never becomes a label value.
pub const UNDECLARED: &str = "undeclared";

/// Each declared stream's QoS class, as of the catalogue last applied.
///
/// Shared between the decorator, which reads it on every commit, and the
/// catalogue reload, which replaces it once the broker has accepted a new
/// catalogue, so a stream added at runtime is labelled by its class from its
/// first commit rather than as undeclared.
#[derive(Clone, Debug, Default)]
pub struct StreamClasses(Arc<Mutex<BTreeMap<String, QosClass>>>);

impl StreamClasses {
    pub fn from_catalogue(catalogue: &Catalogue) -> Self {
        let classes = Self::default();
        classes.replace(catalogue);
        classes
    }

    /// Take `catalogue`'s classes in place of the ones held.
    pub fn replace(&self, catalogue: &Catalogue) {
        let declared = catalogue
            .streams()
            .iter()
            .map(|(name, declaration)| (name.clone(), declaration.policy.qos_class()))
            .collect();
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = declared;
    }

    /// `stream`'s class label, or [`UNDECLARED`].
    pub fn label(&self, stream: &str) -> &'static str {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(stream)
            .map_or(UNDECLARED, QosClass::as_str)
    }
}

/// A [`ManagedLog`] that times every commit's answer and records it as
/// `qip_event_fabric_quorum_ack_ms{class,outcome}`. Every other call passes
/// straight through, the defaulted ones included: a decorator that let
/// `acquire_after_expiry` or `release` fall back to the trait's defaults
/// would quietly drop placement grace and lease hand-back from the log it
/// wraps.
#[derive(Debug)]
pub struct TimedManagedLog {
    inner: Arc<dyn ManagedLog>,
    telemetry: Arc<FabricdTelemetry>,
    classes: StreamClasses,
    clock: Arc<dyn Clock>,
}

impl TimedManagedLog {
    pub fn new(
        inner: Arc<dyn ManagedLog>,
        telemetry: Arc<FabricdTelemetry>,
        classes: StreamClasses,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner,
            telemetry,
            classes,
            clock,
        }
    }
}

impl ManagedLog for TimedManagedLog {
    fn acquire(&self, partition: &PartitionRef, holder: &str, ttl_ms: u64) -> Result<LeaseOutcome> {
        self.inner.acquire(partition, holder, ttl_ms)
    }

    fn acquire_after_expiry(
        &self,
        partition: &PartitionRef,
        holder: &str,
        ttl_ms: u64,
        grace_ms: u64,
    ) -> Result<LeaseOutcome> {
        self.inner
            .acquire_after_expiry(partition, holder, ttl_ms, grace_ms)
    }

    fn release(&self, partition: &PartitionRef, lease: &Lease) -> Result<bool> {
        self.inner.release(partition, lease)
    }

    fn commit(&self, lease: &Lease, row: &CommittedBatch) -> Result<CommitOutcome> {
        let started = self.clock.now();
        let answer = self.inner.commit(lease, row);
        // Nanoseconds to milliseconds for a latency histogram: a statistic,
        // so an f64. A clock that stepped backwards reads as zero rather
        // than as a negative latency.
        let elapsed_ms = self
            .clock
            .now()
            .as_nanos()
            .saturating_sub(started.as_nanos())
            .max(0) as f64
            / 1_000_000.0;
        self.telemetry.quorum_ack_ms(
            self.classes.label(&row.partition.stream),
            quorum_outcome(&answer),
            elapsed_ms,
        );
        answer
    }

    fn read(
        &self,
        partition: &PartitionRef,
        from_offset: u64,
        limit: u32,
    ) -> Result<Vec<CommittedBatch>> {
        self.inner.read(partition, from_offset, limit)
    }

    fn committed_through(&self, partition: &PartitionRef) -> Result<u64> {
        self.inner.committed_through(partition)
    }

    fn commit_offset(&self, group: &str, partition: &PartitionRef, offset: u64) -> Result<()> {
        self.inner.commit_offset(group, partition, offset)
    }

    fn committed_offset(&self, group: &str, partition: &PartitionRef) -> Result<Option<u64>> {
        self.inner.committed_offset(group, partition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::{Duration, ManualClock, Timestamp};
    use qip_observability::metrics::{MetricValue, Metrics, labels, names};
    use qip_streaming::event_fabric::managed::MemoryManagedLog;

    const OUTCOMES: &str = "outcomes.managed";

    /// The in-memory log, made to take `COMMIT_MS` of the shared clock to
    /// answer a commit, so the timer is proven to measure the inner call
    /// rather than to record a constant.
    #[derive(Debug)]
    struct SlowLog {
        inner: MemoryManagedLog,
        clock: Arc<ManualClock>,
    }

    const COMMIT_MS: i64 = 7;

    impl ManagedLog for SlowLog {
        fn acquire(&self, p: &PartitionRef, holder: &str, ttl_ms: u64) -> Result<LeaseOutcome> {
            self.inner.acquire(p, holder, ttl_ms)
        }
        fn release(&self, p: &PartitionRef, lease: &Lease) -> Result<bool> {
            self.inner.release(p, lease)
        }
        fn commit(&self, lease: &Lease, row: &CommittedBatch) -> Result<CommitOutcome> {
            self.clock.advance(Duration::from_millis(COMMIT_MS));
            self.inner.commit(lease, row)
        }
        fn read(&self, p: &PartitionRef, from: u64, limit: u32) -> Result<Vec<CommittedBatch>> {
            self.inner.read(p, from, limit)
        }
        fn committed_through(&self, p: &PartitionRef) -> Result<u64> {
            self.inner.committed_through(p)
        }
        fn commit_offset(&self, group: &str, p: &PartitionRef, offset: u64) -> Result<()> {
            self.inner.commit_offset(group, p, offset)
        }
        fn committed_offset(&self, group: &str, p: &PartitionRef) -> Result<Option<u64>> {
            self.inner.committed_offset(group, p)
        }
    }

    fn catalogue() -> Catalogue {
        let json = format!(
            r#"{{"streams":[{{"name":"{OUTCOMES}","qos_class":"p1_outcomes","partition_key":"cell",
                "ordering":"per_partition","retention":"irreplaceable","replication_factor":3,
                "mirroring":"none","overload_policy":"refuse_producer",
                "ack_profile":"managed_majority","byte_quota_per_producer":67108864,
                "message_quota_per_producer":1000000,"lag_limit":1000000,
                "entitlement_dataset":"qip-test","entitlement_usage":"research",
                "seal_age_ms":20,"peak_bytes_per_second":1048576,"topics":[]}}],"grants":[]}}"#
        );
        Catalogue::parse(json.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn row(stream: &str, offset: u64, epoch: u64) -> CommittedBatch {
        let bytes = format!("batch-{offset}").into_bytes();
        CommittedBatch {
            partition: PartitionRef::new(stream, 0),
            offset,
            lease_epoch: epoch,
            batch_hash: qip_core::hash::sha256_hex(&bytes),
            previous_batch_hash: None,
            bytes,
        }
    }

    fn held(outcome: Result<LeaseOutcome>) -> Lease {
        match outcome {
            Ok(LeaseOutcome::Held(lease)) => lease,
            other => panic!("expected a held lease, got {other:?}"),
        }
    }

    /// The histogram for `class` and `outcome`: its count and sum.
    fn observed(metrics: &Metrics, class: &str, outcome: &str) -> Option<(u64, f64)> {
        metrics
            .snapshot()
            .series
            .iter()
            .find(|s| {
                s.name == names::EVENT_FABRIC_QUORUM_ACK_MS
                    && s.labels == labels([("class", class), ("outcome", outcome)])
            })
            .and_then(|s| match &s.value {
                MetricValue::Histogram(h) => Some((h.count, h.sum)),
                _ => None,
            })
    }

    /// R12-OBS-9146 at the seam: every answer a commit can get - committed,
    /// conflict, an unknown outcome, fenced - is timed once under the
    /// stream's class and its own outcome literal, for the time the inner
    /// log took; a stream the catalogue does not declare is labelled
    /// `undeclared`; and the lease calls pass through untimed, `release`
    /// included, which the trait would otherwise default to a refusal.
    ///
    /// Mutation: in `TimedManagedLog::commit`, delete the
    /// `self.telemetry.quorum_ack_ms(..)` call - the `committed` histogram
    /// is absent and the first `observed` assertion fires.
    #[test]
    fn every_commit_answer_is_timed_by_class_and_outcome_and_the_lease_calls_pass_through() {
        let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 24)));
        let slow: Arc<dyn ManagedLog> = Arc::new(SlowLog {
            inner: MemoryManagedLog::new(clock.clone()),
            clock: clock.clone(),
        });
        let metrics = Arc::new(Metrics::new("qip-fabricd"));
        let telemetry = Arc::new(FabricdTelemetry::new(metrics.clone()));
        let log = TimedManagedLog::new(
            slow,
            telemetry,
            StreamClasses::from_catalogue(&catalogue()),
            clock.clone(),
        );
        let partition = PartitionRef::new(OUTCOMES, 0);
        let lease = held(log.acquire(&partition, "zone-a", 1_000));
        assert!(
            !metrics
                .snapshot()
                .series
                .iter()
                .any(|s| s.name == names::EVENT_FABRIC_QUORUM_ACK_MS),
            "premise: taking a lease is not a quorum acknowledgement"
        );

        assert_eq!(
            log.commit(&lease, &row(OUTCOMES, 0, lease.epoch)).ok(),
            Some(CommitOutcome::Committed)
        );
        let (count, sum) =
            observed(&metrics, "p1_outcomes", "committed").expect("the commit was timed");
        assert_eq!(count, 1);
        assert!(
            (sum - COMMIT_MS as f64).abs() < 1e-9,
            "the timer measured {sum} ms around a {COMMIT_MS} ms commit"
        );

        assert!(matches!(
            log.commit(&lease, &row(OUTCOMES, 5, lease.epoch)),
            Ok(CommitOutcome::Conflict { .. })
        ));
        assert_eq!(
            observed(&metrics, "p1_outcomes", "conflict").map(|o| o.0),
            Some(1)
        );

        log.commit(&lease, &row("not.declared", 0, lease.epoch))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            observed(&metrics, UNDECLARED, "fenced").is_some(),
            "a stream the catalogue does not hold is labelled undeclared, never by its name"
        );

        assert_eq!(
            log.release(&partition, &lease).ok(),
            Some(true),
            "release reached the log rather than the trait's refusing default"
        );
        clock.advance(Duration::from_millis(1));
        let usurper = held(log.acquire(&partition, "zone-b", 1_000));
        assert!(usurper.epoch > lease.epoch, "premise: the lease moved on");
        assert!(matches!(
            log.commit(&lease, &row(OUTCOMES, 1, lease.epoch)),
            Ok(CommitOutcome::Fenced { .. })
        ));
        assert_eq!(
            observed(&metrics, "p1_outcomes", "fenced").map(|o| o.0),
            Some(1)
        );

        let unknown = row("outcomes.managed", 1, usurper.epoch);
        let mut corrupt = unknown.clone();
        corrupt.batch_hash = "00".into();
        assert!(log.commit(&usurper, &corrupt).is_err());
        assert_eq!(
            observed(&metrics, "p1_outcomes", "error").map(|o| o.0),
            Some(1)
        );
    }
}
