//! R12-REFLEX-9114: the drain step of a rollout. Reflex components roll out
//! by drain, fence and epoch activation: the old incarnation ships what its
//! spool holds before the new one's producer epoch fences it out. The epoch
//! half was built (`standby.rs`, the broker's producer epochs); until this
//! the spool had no drain-out: the drain thread stepped on as if nothing
//! had changed, and nothing said whether the old node left anything behind.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{Clock, CorrelationId, Decimal, Duration, EventId, Lineage, ManualClock, Timestamp};
use qip_edge::fencing::Fence;
use qip_edge_node::event_fabric::drain::{Drain, DrainConfig, DrainedTo, Sink};
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};
use qip_events::{AnyEvent, Topic};
use qip_observability::metrics::Metrics;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-drain-out-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn batch(n: u64) -> Batch {
    let payload = serde_json::json!({ "n": n });
    let event = AnyEvent {
        event_id: EventId::from_string(format!("evt-drain-{n}")),
        topic: Topic::MarketTick,
        schema_version: 1,
        occurred_at: t(0),
        recorded_at: t(0),
        sequence: n,
        lineage: Lineage {
            correlation_id: CorrelationId::from_string("COR00000000000000000000077"),
            causation_id: None,
            trace_id: None,
            producer: "drain-out-test".to_string(),
        },
        idempotency_key: None,
        payload_hash: qip_core::sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    };
    Batch::new(
        MessageType::Data,
        QosClass::P2MarketJournal.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap()],
    )
    .unwrap()
}

/// A sink that keeps the spool offsets it took, and refuses as fenced when
/// told to.
#[derive(Debug, Clone, Default)]
struct Recording {
    taken: Arc<Mutex<Vec<u64>>>,
    fenced: bool,
}

impl Sink for Recording {
    fn name(&self) -> &'static str {
        "recording"
    }
    fn deliver(&mut self, offset: u64, _stream: QosClass, _batch: &Batch) -> Result<()> {
        if self.fenced {
            return Err(Error::denied("a newer producer epoch owns the partition"));
        }
        self.taken.lock().unwrap().push(offset);
        Ok(())
    }
    fn fenced(&self) -> bool {
        self.fenced
    }
}

/// A spool holding `batches` batches and a drain over it delivering to
/// `sink`, stepping at most one batch per step so a drain-out is several
/// steps rather than one.
fn drain_over(label: &str, batches: u64, sink: Recording) -> Drain {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let log =
        Arc::new(SegmentLog::open(temp_dir(label), SegmentLogConfig::new(clock.clone())).unwrap());
    for n in 0..batches {
        log.append(&batch(n)).unwrap();
    }
    let thresholds = Thresholds::new(
        1 << 40,
        Decimal::parse("0.5").unwrap(),
        Decimal::parse("0.9").unwrap(),
    )
    .unwrap();
    let (_gauge, _spool, publisher) =
        PressureGauge::new(thresholds, clock, Duration::from_secs(5)).unwrap();
    let telemetry = Arc::new(OutboxTelemetry::new(Arc::new(Metrics::new(
        "qip-edge-node",
    ))));
    Drain::open(
        log,
        vec![Box::new(sink)],
        publisher,
        telemetry,
        DrainConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_secs(3600),
            1,
        )
        .unwrap(),
    )
    .unwrap()
}

/// Mutation: in `drain_to_tail`, return after the first step and the tail
/// is not reached; drop the fenced-sink check and the second case reports
/// the step bound instead of the fence.
#[test]
fn a_drain_out_ships_everything_the_spool_held_or_names_the_fenced_sink_that_stopped_it() {
    let sink = Recording::default();
    let mut drain = drain_over("tail", 5, sink.clone());
    let outcome = drain.drain_to_tail(64).unwrap();
    assert_eq!(
        outcome,
        DrainedTo::Tail {
            delivered: 5,
            steps: 5
        }
    );
    assert_eq!(*sink.taken.lock().unwrap(), vec![0, 1, 2, 3, 4]);

    let fenced = Recording {
        fenced: true,
        ..Recording::default()
    };
    let mut drain = drain_over("fenced", 3, fenced);
    let DrainedTo::Stalled {
        delivered, reason, ..
    } = drain.drain_to_tail(64).unwrap()
    else {
        panic!("a fenced sink was reported drained")
    };
    assert_eq!(delivered, 0);
    assert!(reason.contains("recording sink is fenced"), "{reason}");
}

/// Wait, bounded, for `ready`; a hang is a failure, never a pass.
fn within(what: &str, ready: impl Fn() -> bool) {
    for _ in 0..400 {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("{what} did not happen within two seconds");
}

/// The rollout's ordering: a node whose held fence stops admitting drains
/// its spool out and stops; a standby, whose fence never held, does not.
/// Mutation: in `spawn_with`, never take the drain-out branch and
/// `drained_on_fence` stays empty, so the bounded wait below fails.
#[test]
fn a_held_fence_that_lapses_drains_the_spool_out_and_a_standby_never_does() {
    let clock = Arc::new(ManualClock::new(t(0)));
    let fence = Fence::new();
    fence.grant(1, t(10)).unwrap();
    let sink = Recording::default();
    let drain = drain_over("lapse", 4, sink.clone())
        .spawn_fenced(fence.clone(), clock.clone())
        .unwrap();
    // Premise: while the fence holds the drain runs and has not drained out.
    within("the drain shipping while the fence holds", || {
        sink.taken.lock().unwrap().len() == 4
    });
    assert!(drain.is_running());
    assert_eq!(drain.drained_on_fence(), None);

    clock.set(t(11));
    within("the drain-out on fence", || {
        drain.drained_on_fence().is_some()
    });
    assert!(
        matches!(drain.drained_on_fence(), Some(DrainedTo::Tail { .. })),
        "{:?}",
        drain.drained_on_fence()
    );
    within("the drain thread stopping after its drain-out", || {
        !drain.is_running()
    });

    let standby_clock = Arc::new(ManualClock::new(t(0)));
    let standby = drain_over("standby", 2, Recording::default())
        .spawn_fenced(Fence::new(), standby_clock.clone())
        .unwrap();
    standby_clock.set(t(100));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(standby.drained_on_fence(), None, "a standby drained out");
    assert!(standby.is_running());
}
