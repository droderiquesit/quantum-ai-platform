//! The P0 control consumer and its start barrier (ADR 0100 §8, SLICE-57).
//!
//! Every test drives the real [`ControlConsumer`] on its own thread against a
//! scripted broker behind the `FabricTransport` seam, through SLICE-28's
//! `Consumer`, and observes it only from the decision thread's side: the
//! barrier, the handoff drained at a boundary, the refusal channel and the
//! metrics registry. Nothing here reaches into the consumer's state, because
//! what matters is what crosses to the thread that trades.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{HaltCommand, PolicyPayload};
use qip_contracts::replay::ControlPosition;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::hash::to_hex;
use qip_core::{
    Clock, CorrelationId, Decimal, Duration, EventId, Id, Lineage, ManualClock, ObjectId,
    Timestamp, dec,
};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{CapitalGrantTopic, DEFAULT_GRANT_MEMORY, HaltTopic, PolicyPayloadTopic};
use qip_edge_node::control::{
    Applied, Barrier, CaughtUp, ControlKind, Handoff, Outcome, Provenance,
};
use qip_edge_node::event_fabric::control::{
    CONTROL_STREAM, CONTROL_THREAD, ControlConfig, ControlConsumer, ControlThread, RefusalReason,
    Refusals,
};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{MetricValue, Metrics, names};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::consumer::{Consumer, ConsumerConfig};
use qip_transport::event_fabric::protocol::{FetchResponse, Request, Response};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration as StdDuration, Instant};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const STRATEGY: &str = "mean-reversion-1";
const KEY: &[u8] = b"control-consumer-test-envelope-key";
const OTHER_KEY: &[u8] = b"a-key-this-cell-has-never-held";

/// Generous against a loaded machine; a healthy consumer catches up with a
/// handful of records in milliseconds.
const DEADLINE: StdDuration = StdDuration::from_secs(10);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

// --- the frames the centre sends -----------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct GrantBody(CapitalEnvelope);

impl EventBody for GrantBody {
    const TOPIC: Topic = CapitalGrantTopic::TOPIC;
    const SCHEMA_VERSION: u32 = CapitalGrantTopic::SCHEMA_VERSION;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct PolicyBody(PolicyPayload);

impl EventBody for PolicyBody {
    const TOPIC: Topic = PolicyPayloadTopic::TOPIC;
    const SCHEMA_VERSION: u32 = PolicyPayloadTopic::SCHEMA_VERSION;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct HaltBody(HaltCommand);

impl EventBody for HaltBody {
    const TOPIC: Topic = HaltTopic::TOPIC;
    const SCHEMA_VERSION: u32 = HaltTopic::SCHEMA_VERSION;
}

fn frame<B: EventBody>(body: B, event_id: &str, at: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(event_id.to_string()),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "qip-edge-node-tests",
        ),
        body,
    )
    .erase()
}

/// A grant for the deployed strategy, signed with `key` — or, with no key,
/// carrying the literal signature `unsigned`.
fn grant(gross: Decimal, key: Option<&[u8]>) -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            gross,
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(7_200),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    match key {
        Some(key) => build(&sign_payload(key, &unsigned.signing_payload())),
        None => Ok(unsigned),
    }
}

fn policy(sequence: u64, key: &[u8]) -> Result<PolicyPayload> {
    PolicyPayload::unproduced(sequence, CELL, t(10)).signed(key)
}

fn position(offset: u64, event_id: &str) -> ControlPosition {
    ControlPosition::new(CONTROL_STREAM, 0, offset, EventId::from_string(event_id))
}

// --- cells -----------------------------------------------------------------

fn bare_cell() -> Result<Cell> {
    Cell::new(
        CellConfig::new(CELL, REGION).with_venue(VenueId::new("XLON")),
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
}

/// A cell running the strategy the grants fund, under a first grant, so a
/// renewal is something it applies rather than refuses.
fn funded_cell() -> Result<Cell> {
    use qip_strategy::catalogue::FeatureCatalogue;
    use qip_strategy::compile::StrategyCompiler;
    use qip_strategy::ir::{Expr, Rule, StrategySpec};

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(
        StrategyId::new(STRATEGY),
        ObjectId::from_string("obj-ACME"),
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "never",
        SignalKind::Enter,
        Expr::Flag(false),
        Expr::Exact(dec!("1")),
        Expr::Statistic(0.5),
        100,
    ));
    let strategy = compiler.compile(&spec)?;
    let program = compiler.into_program();
    let mut cell = bare_cell()?;
    let first = VerifiedEnvelope::verify(grant(dec!("1000"), Some(KEY))?, KEY, CELL, t(10))?;
    cell.deploy(strategy, program, first)?;
    Ok(cell)
}

// --- a scripted broker behind the FabricTransport seam ---------------------

#[derive(Debug, Default)]
struct BrokerState {
    /// The control partition, one record per offset.
    partition: Vec<AnyEvent>,
    /// Every fetch answered: the offset asked for and the asking thread.
    fetches: Vec<(u64, String)>,
    /// Fetches past this many wait until `released`.
    hold_after: Option<usize>,
    released: bool,
    /// A fetch is waiting on the hold.
    waiting: bool,
}

/// One partition of `control.local`, answering each fetch with the one
/// record at the offset asked for and the partition's high watermark — so a
/// backlog of N records takes N fetches, which is the shape in which "the
/// first fetch came back" and "the start watermark was read through" differ.
#[derive(Clone, Debug, Default)]
struct Broker {
    state: Arc<(Mutex<BrokerState>, Condvar)>,
}

impl Broker {
    fn with(partition: Vec<AnyEvent>) -> Self {
        let broker = Self::default();
        broker.lock().partition = partition;
        broker
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrokerState> {
        self.state.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn hold_after(&self, fetches: usize) {
        self.lock().hold_after = Some(fetches);
    }

    fn release(&self) {
        self.lock().released = true;
        self.state.1.notify_all();
    }

    fn fetches(&self) -> Vec<(u64, String)> {
        self.lock().fetches.clone()
    }

    /// Wait until a fetch is parked on the hold, or fail naming what the
    /// broker had served by then.
    fn await_parked(&self) {
        let started = Instant::now();
        while !self.lock().waiting {
            assert!(
                started.elapsed() < DEADLINE,
                "no fetch reached the hold within {DEADLINE:?}; served {:?}",
                self.fetches()
            );
            std::thread::sleep(StdDuration::from_millis(1));
        }
    }

    fn answer(&self, offset: u64, partition: u32, stream: String) -> Result<Response> {
        let (lock, released) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hold) = state.hold_after
            && state.fetches.len() >= hold
        {
            state.waiting = true;
            let started = Instant::now();
            while !state.released {
                if started.elapsed() > DEADLINE {
                    return Err(Error::io("test broker: the hold was never released"));
                }
                let (next, _) = released
                    .wait_timeout(state, StdDuration::from_millis(20))
                    .unwrap_or_else(|p| p.into_inner());
                state = next;
            }
            state.waiting = false;
        }
        let thread = std::thread::current().name().unwrap_or("").to_string();
        state.fetches.push((offset, thread));
        let watermark = state.partition.len() as u64;
        let batches = match state.partition.get(offset as usize) {
            Some(event) => {
                let record = Record::from_any_event(event, PayloadCodec::CanonicalJson)?;
                let mut batch = Batch::new(
                    MessageType::Data,
                    1,
                    1,
                    PayloadCodec::CanonicalJson,
                    vec![record],
                )?;
                batch.base_offset = offset;
                to_hex(&batch.encode()?)
            }
            None => String::new(),
        };
        Ok(Response::Fetch(FetchResponse::new(
            stream, partition, watermark, 0, batches,
        )?))
    }
}

impl FabricTransport for Broker {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> Result<Response> {
        match request {
            Request::Fetch(fetch) => self.answer(fetch.offset, fetch.partition, fetch.stream),
            other => Err(Error::invalid(format!(
                "test broker: the control consumer asked for {:?}, which it has no reason to",
                other.route()
            ))),
        }
    }
}

// --- the node's side ------------------------------------------------------

struct Node {
    thread: ControlThread,
    barrier: CaughtUp,
    handoff: Handoff,
    refusals: Refusals,
    metrics: Arc<Metrics>,
}

fn start(broker: &Broker, clock: Arc<ManualClock>) -> Result<Node> {
    let consumer = Consumer::new(ConsumerConfig {
        transport: Box::new(broker.clone()),
        stream: CONTROL_STREAM.to_string(),
        partition: 0,
        group_id: format!("control-{CELL}"),
        retry_policy: RetryPolicy::default(),
        breaker_policy: BreakerPolicy::default(),
        clock: clock.clone(),
        sleeper: Arc::new(ThreadSleeper),
        retry_seed: 1,
        breaker_seed: 1,
        timeouts: Timeouts::default(),
        fetch_credit_bytes: 64 * 1024,
    })?;
    let (sender, handoff) = Handoff::bounded(16)?;
    let (refusal_sender, refusals) = Refusals::bounded(16)?;
    let (signal, barrier) = CaughtUp::new();
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let telemetry = Arc::new(OutboxTelemetry::new(metrics.clone()));
    let control = ControlConsumer::new(
        ControlConfig {
            cell: CELL.to_string(),
            key: KEY.to_vec(),
            grant_memory: DEFAULT_GRANT_MEMORY,
            clock,
        },
        consumer,
        sender,
        refusal_sender,
        signal,
        telemetry,
    )?;
    let thread = control.spawn(Arc::new(ThreadSleeper), Duration::from_millis(2))?;
    Ok(Node {
        thread,
        barrier,
        handoff,
        refusals,
        metrics,
    })
}

impl Node {
    /// Wait for the consumer's counters as of the turn that fired the
    /// barrier; they are published just after it.
    fn settled_stats(&self) -> qip_edge_node::event_fabric::control::ControlStats {
        let started = Instant::now();
        loop {
            let stats = self.thread.stats();
            if stats.caught_up {
                return stats;
            }
            assert!(
                started.elapsed() < DEADLINE,
                "the consumer never published caught-up counters: {stats:?}"
            );
            std::thread::sleep(StdDuration::from_millis(1));
        }
    }

    fn refused_by_reason(&self) -> BTreeMap<String, u64> {
        self.metrics
            .snapshot()
            .series
            .iter()
            .filter(|series| series.name == names::EDGE_EVENT_FABRIC_CONTROL_REFUSED)
            .filter_map(|series| match series.value {
                MetricValue::Counter(count) => Some((series.labels.get("reason")?.clone(), count)),
                _ => None,
            })
            .collect()
    }
}

fn clock_at(secs: i64) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(t(secs)))
}

// --- the tests --------------------------------------------------------------

#[test]
fn a_p0_grant_fetched_twice_is_verified_off_the_decision_thread_and_delivered_once() -> Result<()> {
    // FABRIC-007. The broker is at-least-once, and the same grant can sit at
    // two offsets — a producer retry past the dedup window, or a republish.
    // Two frames with two event ids carry one authority, and a consumer that
    // handed both across would renew the cell twice and record two
    // applications of one grant. The downlink's grant memory, keyed on the
    // signature rather than on anything the wire chose, is what stops it; a
    // consumer that built a fresh downlink per record would have none.
    let renewal = frame(
        GrantBody(grant(dec!("2000"), Some(KEY))?),
        "EVT-GRANT",
        t(20),
    )?;
    let broker = Broker::with(vec![renewal.clone(), renewal]);
    let clock = clock_at(60);
    let mut node = start(&broker, clock.clone())?;

    assert_eq!(
        node.barrier.wait(DEADLINE),
        Barrier::CaughtUp,
        "the consumer never caught up with a two-record partition"
    );
    let stats = node.settled_stats();

    // Premise: the frame really was fetched twice, at both offsets, and by
    // the consumer's own thread — not the one the test (the decision
    // thread) runs on.
    let fetched = broker.fetches();
    let offsets: Vec<u64> = fetched.iter().map(|(offset, _)| *offset).collect();
    assert!(
        offsets.starts_with(&[0, 1]),
        "the premise is a grant fetched at offsets 0 and 1: {fetched:?}"
    );
    let decision_thread = std::thread::current().name().unwrap_or("").to_string();
    assert_ne!(decision_thread, CONTROL_THREAD);
    for (offset, thread) in &fetched {
        assert_eq!(
            thread, CONTROL_THREAD,
            "offset {offset} was fetched, and so verified, on {thread:?} rather than the \
             control consumer's own thread"
        );
    }

    let mut cell = funded_cell()?;
    let boundary = node.handoff.boundary(&mut cell, None, clock.now());
    assert_eq!(
        boundary.applied,
        vec![Applied {
            kind: ControlKind::Envelope,
            provenance: Provenance::Fabric(position(0, "EVT-GRANT")),
            outcome: Outcome::Applied,
        }],
        "the grant fetched twice did not cross exactly once, at its first position"
    );
    assert_eq!(
        (stats.delivered, stats.duplicates, stats.refused),
        (1, 1, 0),
        "the second fetch was not recognised as the grant already delivered: {stats:?}"
    );
    assert!(
        node.refusals.drain().refused.is_empty(),
        "a redelivered genuine grant is a duplicate, not a refusal"
    );
    Ok(())
}

#[test]
fn an_unsigned_or_wrong_key_control_frame_is_refused_counted_and_never_crosses_as_a_value()
-> Result<()> {
    // FABRIC-081. The fabric authenticates nobody, and its CRCs prove only
    // that bytes were not torn. A consumer that trusted either — or decoded
    // a grant itself instead of going through the downlinks' verify — is the
    // second verification path by which an unsigned grant reaches the cell.
    // Each kind of control is tried with a bad signature after a genuine
    // policy, so "the prior policy stays" is observed on a cell that had one.
    let partition = vec![
        frame(PolicyBody(policy(1, KEY)?), "EVT-P1", t(10))?,
        frame(PolicyBody(policy(2, OTHER_KEY)?), "EVT-P2-FORGED", t(11))?,
        frame(
            GrantBody(grant(dec!("5000"), None)?),
            "EVT-G-UNSIGNED",
            t(12),
        )?,
        frame(
            HaltBody(HaltCommand::new(CELL, t(13), "forged halt").signed(OTHER_KEY)?),
            "EVT-H-FORGED",
            t(13),
        )?,
    ];
    let broker = Broker::with(partition);
    let clock = clock_at(60);
    let mut node = start(&broker, clock.clone())?;

    assert_eq!(node.barrier.wait(DEADLINE), Barrier::CaughtUp);
    let stats = node.settled_stats();
    assert_eq!(
        stats.frames, 4,
        "the premise is all four records judged: {stats:?}"
    );

    let mut cell = bare_cell()?;
    let boundary = node.handoff.boundary(&mut cell, None, clock.now());
    assert_eq!(
        boundary.applied,
        vec![Applied {
            kind: ControlKind::Policy,
            provenance: Provenance::Fabric(position(0, "EVT-P1")),
            outcome: Outcome::Applied,
        }],
        "something other than the one genuine policy crossed as a value"
    );
    assert_eq!(
        cell.policy_sequence(),
        Some(1),
        "the forged payload moved the cell off its prior policy"
    );
    assert!(!cell.is_halted(), "the forged halt stopped the cell");

    let drained = node.refusals.drain();
    let listed: Vec<(RefusalReason, ControlPosition)> = drained
        .refused
        .iter()
        .map(|refused| (refused.reason, refused.position.clone()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (RefusalReason::Policy, position(1, "EVT-P2-FORGED")),
            (RefusalReason::Grant, position(2, "EVT-G-UNSIGNED")),
            (RefusalReason::Halt, position(3, "EVT-H-FORGED")),
        ],
        "each refused record must cross as a refusal carrying its own position"
    );
    assert_eq!(drained.unlisted, 0);
    assert!(
        drained
            .refused
            .iter()
            .all(|refused| !refused.detail.is_empty()),
        "a refusal must carry the downlink's reason: {:?}",
        drained.refused
    );

    let counted = node.refused_by_reason();
    assert_eq!(
        counted,
        BTreeMap::from([
            (RefusalReason::Grant.as_str().to_string(), 1),
            (RefusalReason::Halt.as_str().to_string(), 1),
            (RefusalReason::Policy.as_str().to_string(), 1),
        ]),
        "every refusal must be counted under its own reason"
    );
    assert_eq!((stats.delivered, stats.refused), (1, 3), "{stats:?}");
    Ok(())
}

#[test]
fn the_control_thread_signals_caught_up_only_after_reading_through_the_start_high_watermark()
-> Result<()> {
    // Red-team major: the first pass raced the control fetch, so which pass
    // a grant landed on depended on scheduling and two runs of one tape
    // diverged. The barrier holds the first pass until the consumer has read
    // through the high watermark it saw at start. Signalling on the first
    // answer instead would let the first pass run with two of three records
    // unread — here, under policy 1 when the centre had already sent 3.
    let partition = vec![
        frame(PolicyBody(policy(1, KEY)?), "EVT-P1", t(10))?,
        frame(PolicyBody(policy(2, KEY)?), "EVT-P2", t(11))?,
        frame(PolicyBody(policy(3, KEY)?), "EVT-P3", t(12))?,
    ];
    let broker = Broker::with(partition);
    broker.hold_after(1);
    let clock = clock_at(60);
    let mut node = start(&broker, clock.clone())?;

    // Premise: the first fetch was answered, carrying the watermark of 3,
    // and the second is parked — the consumer is alive and has read one.
    broker.await_parked();
    assert_eq!(
        broker.fetches().len(),
        1,
        "the premise is exactly one fetch answered: {:?}",
        broker.fetches()
    );
    assert_eq!(
        node.barrier.wait(StdDuration::from_millis(300)),
        Barrier::Waiting,
        "the barrier lifted with two of the three records at start unread"
    );

    broker.release();
    assert_eq!(
        node.barrier.wait(DEADLINE),
        Barrier::CaughtUp,
        "the consumer never caught up once the broker answered"
    );

    // Everything below the start watermark was handed across before the
    // signal, so the first boundary sees all three.
    let mut cell = bare_cell()?;
    let boundary = node.handoff.boundary(&mut cell, None, clock.now());
    assert_eq!(
        boundary.positions(),
        vec![
            position(0, "EVT-P1"),
            position(1, "EVT-P2"),
            position(2, "EVT-P3"),
        ],
        "the barrier lifted before every record below the start watermark crossed"
    );
    assert_eq!(cell.policy_sequence(), Some(3));
    assert_eq!(node.settled_stats().start_watermark, Some(3));
    Ok(())
}
