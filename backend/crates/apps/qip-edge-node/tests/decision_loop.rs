//! The decision loop with the outbox wired as `main.rs` wires it: the cell
//! flushes through `FabricMirror`, the relay forwards to the spool writer and
//! ships to the journal store, and the drain delivers the spool to the tick
//! lake (ADR 0100 §1, §6; W0.9, W1.2, W1.3, W1.13).
//!
//! Every test drives `DecisionLoop::turn` — the call the binary's timer
//! makes — against a real `SegmentLog` spool and a real `StoreMirror`, so
//! what is asserted is what the deployed composition does, not what one of
//! its parts does on its own.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Decimal, SystemClock, dec};
use qip_core::{CorrelationId, Id, Lineage};
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::PolicyPayloadTopic;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::pressure::{Exhaustion, JournalPressure};
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::event_fabric::drain::{Drain, DrainConfig, LakeSink, Sink};
use qip_edge_node::event_fabric::inputs::RecordedInputs;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{DrainPublisher, PressureGauge, Thresholds};
use qip_edge_node::event_fabric::relay::{JournalRelay, RelayConfig};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{self, SpoolWriter, WriterConfig};
use qip_edge_node::feed::{FEED_NAME, SimulatedFeed};
use qip_edge_node::gateway::{NodeGateway, SimulatedGateway};
use qip_edge_node::mesh::{MeshLink, MeshSettings, MeshWorker};
use qip_edge_node::mirror::{StoreMirror, batches};
use qip_edge_node::scheduler::{DecisionLoop, InstantClock, JournalBound, MarkerFacts, Outbox};
use qip_events::event_fabric::bindings::REFLEX_PASS_MARKED;
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_peer_link::boundary::ReflexPort;
use qip_storage::BlobStore;
use qip_storage::blob::MemoryBlobStore;
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use qip_transport::{Admission, MeshEndpoint, MeshInbox, MeshMessage, Method, RecordingSleeper};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"decision-loop-envelope-key";

/// How long a test waits for something another thread does before calling
/// it a failure. Far longer than any healthy step; short enough that a
/// regression fails rather than hangs.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-decision-loop-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory can be created");
    dir
}

/// A strategy whose one rule always holds, so every pass decides something
/// and the journal has entries to ship on every turn.
fn firing_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("1")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn grant() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            t(0),
            t(36_000),
            "alice@example.com",
            signature,
        )
    };
    // Bounded as the centre's issuer bounds every grant (R12-CAPITAL-4088):
    // the cell `assemble` builds refuses an envelope without exposure bounds
    // at `envelope_unbounded`, and until 2026-10-18 this rig's grant had
    // none, so no turn in this file ever placed an order and a test timing
    // "a venue event to a local order" timed a refusal instead.
    let unsigned = build("unsigned")?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        qip_core::Decimal::ONE,
        capital,
    )?)?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, CELL, t(1))
}

fn share_policy(grants: Vec<String>) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(5))
}

/// A key-value store whose writes wait while it is stalled — the journal
/// store on a hung disk or an unreachable managed endpoint.
#[derive(Debug, Default)]
struct StallingStore {
    inner: MemoryKeyValueStore,
    stalled: AtomicBool,
    attempts: AtomicU64,
}

impl StallingStore {
    fn stalled() -> Arc<Self> {
        let store = Arc::new(Self::default());
        store.stalled.store(true, Ordering::Release);
        store
    }

    fn release(&self) {
        self.stalled.store(false, Ordering::Release);
    }

    fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Acquire)
    }
}

impl KeyValueStore for StallingStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.inner.get(key)
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.attempts.fetch_add(1, Ordering::AcqRel);
        while self.stalled.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.inner.put(key, value)
    }

    fn delete(&self, key: &str) -> Result<bool> {
        self.inner.delete(key)
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.keys_with_prefix(prefix)
    }

    fn len(&self) -> Result<usize> {
        self.inner.len()
    }
}

/// Everything the tests reach after the loop has taken the cell.
struct Rig {
    decisions: DecisionLoop,
    spool: Arc<SegmentLog>,
    drain_publisher: Option<DrainPublisher>,
    telemetry: Arc<OutboxTelemetry>,
}

/// The node as `main.rs` composes it: an assembled cell with the journal
/// wire and trimming armed, a firing strategy under a funded share, the
/// simulated venue with a two-sided touch, and the outbox — spool, writer
/// thread, relay thread into a `StoreMirror` over `store` — under `bound`.
fn rig(store: Arc<dyn KeyValueStore>, bound: usize, handoff_capacity: usize) -> Result<Rig> {
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_journal_trimmed_on_ship()
        .with_journal_wire();
    rig_with(config, store, bound, handoff_capacity)
}

/// [`rig`] with the cell's configuration supplied, so a test can build the
/// node the way `main.rs` must not.
fn rig_with(
    config: CellConfig,
    store: Arc<dyn KeyValueStore>,
    bound: usize,
    handoff_capacity: usize,
) -> Result<Rig> {
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    // The one instrument the firing strategy trades and the touch below
    // seeds: the venue lists it on demand, and seeds it, only once permitted
    // to do each.
    let mut gateway = permitting_to_seed(SimulatedGateway::new(venue(), 7, t(0))?, &[&object()])?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell.deploy_with_pricing(
        compiled,
        program,
        grant()?,
        PricingPolicy::rest_at_mid(Duration::from_secs(5))?,
    )?;
    let named = grant()?.signature().to_string();
    node.cell.apply_policy(share_policy(vec![named])?, t(5))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;

    let metrics = Arc::clone(node.scrape_registry());
    let telemetry = Arc::new(OutboxTelemetry::new(metrics));
    let clock: Arc<dyn Clock> = Arc::new(InstantClock::anchored_at(t(0)));
    let thresholds = Thresholds::new(
        1 << 30,
        Decimal::parse("0.5").expect("a literal fraction parses"),
        Decimal::parse("0.9").expect("a literal fraction parses"),
    )?;
    let (gauge, spool_publisher, drain_publisher) =
        PressureGauge::new(thresholds, Arc::clone(&clock), Duration::from_secs(5))?;
    let spool = Arc::new(SegmentLog::open(
        temp_dir("spool"),
        SegmentLogConfig::new(Arc::clone(&clock)),
    )?);
    let (to_relay, from_mirror) = writer::channel(handoff_capacity)?;
    let (to_writer, writer_inbox) = writer::channel(handoff_capacity)?;
    let spool_writer = SpoolWriter::open(
        Arc::clone(&spool),
        writer_inbox,
        spool_publisher,
        Arc::clone(&telemetry),
        WriterConfig::new(CELL, t(0), std::time::Duration::from_millis(5), 64)?,
    )?;
    let session = spool_writer.session();
    std::thread::spawn(move || {
        let _ = spool_writer.run();
    });
    let relay = JournalRelay::spawn(
        from_mirror,
        to_writer,
        StoreMirror::open(store, CELL, t(0))?,
        RelayConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(20),
        )?,
    )?;
    let waited = std::time::Instant::now();
    while gauge.read().generation == 0 {
        assert!(waited.elapsed() < PATIENCE, "the spool writer never beat");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let outbox = Outbox {
        mirror: FabricMirror::new(CELL, to_relay.clone())?,
        inputs: RecordedInputs::new(to_relay, thresholds, Arc::clone(&telemetry)),
        gauge,
        session,
    };
    let decisions = DecisionLoop::new(
        node.cell,
        NodeGateway::Simulated(gateway),
        outbox,
        JournalBound::new(bound)?,
        Arc::clone(&telemetry),
        MarkerFacts {
            cell: CELL.to_string(),
            config_digest: "test".to_string(),
            gateway_seed: 7,
            binary_version: "test".to_string(),
        },
    )?
    .with_feed(feed, None)
    .with_threads(Some(relay), None);
    Ok(Rig {
        decisions,
        spool,
        drain_publisher: Some(drain_publisher),
        telemetry,
    })
}

/// A deadline on the calling thread. The cell is not `Send` — its decoders
/// and feature definitions are trait objects that do not promise it — so
/// the turns run here and the deadline runs beside them: a turn that blocks
/// never returns, and a test that never returns reports nothing, so past
/// the deadline the watchdog says why and aborts the test binary.
struct Watchdog {
    done: mpsc::Sender<()>,
}

impl Watchdog {
    fn arm(what: &'static str) -> Self {
        let (done, finished) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = finished.recv_timeout(PATIENCE) {
                eprintln!("watchdog: {what} did not finish within {PATIENCE:?}");
                std::process::abort();
            }
        });
        Self { done }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.done.send(());
    }
}

/// Run `turns` turns starting at `from`, under a watchdog, and return the
/// longest single turn and each turn's retained count and pressure.
fn drive(
    decisions: &mut DecisionLoop,
    from: i64,
    turns: i64,
) -> (std::time::Duration, Vec<usize>, Vec<JournalPressure>) {
    let _watchdog = Watchdog::arm(
        "the decision thread's turns — a turn blocked, which is the stall the outbox exists to \
         keep off this thread",
    );
    let mut longest = std::time::Duration::ZERO;
    let mut retained = Vec::new();
    let mut pressures = Vec::new();
    for i in 0..turns {
        let began = std::time::Instant::now();
        let turn = decisions.turn(t(from + i)).expect("a turn completes");
        longest = longest.max(began.elapsed());
        retained.push(turn.retained);
        pressures.push(turn.pressure);
    }
    (longest, retained, pressures)
}

#[test]
fn a_stalled_journal_store_never_holds_the_decision_thread() -> Result<()> {
    // The failure this prevents: `main.rs` called `Cell::flush` into the
    // store on the thread that runs `Cell::work`, so a store write that hung
    // stopped the cell deciding for as long as it hung (REFLEX-5033,
    // ARCH-8021). Here the store never answers until released.
    let store = StallingStore::stalled();
    let mut rig = rig(Arc::clone(&store) as Arc<dyn KeyValueStore>, 1 << 16, 4)?;
    let decisions = &mut rig.decisions;

    let (longest, retained, _) = drive(decisions, 10, 30);

    // The premise: the relay really is parked inside a store write, so the
    // turns above ran beside a stalled store and not merely a slow one.
    let waited = std::time::Instant::now();
    while store.attempts() == 0 {
        assert!(
            waited.elapsed() < PATIENCE,
            "the relay never reached the store"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        decisions.stats().passes > 0,
        "the premise is a loop that ran passes, and none ran"
    );
    assert!(
        !decisions.cell().journal().is_empty(),
        "the premise is a journal with entries to ship"
    );
    // Thirty turns against a store that never answered all returned, and
    // none took long: the decision thread offered and moved on.
    assert!(
        longest < std::time::Duration::from_millis(500),
        "a turn took {longest:?} beside a stalled store"
    );
    // The stall is visible where it belongs: the handoff filled behind the
    // parked relay, the mirror refused, and the entries wait in memory.
    assert!(
        decisions.refused_flushes() > 0,
        "the mirror never refused, so the stall was never felt and the test proves nothing"
    );
    assert!(
        retained.last().copied().unwrap_or(0) > 0,
        "nothing waited unshipped while the store was stalled"
    );

    // Released, the record arrives — chained, in order, nothing lost.
    store.release();
    let _watchdog = Watchdog::arm("the released store taking the journal");
    let waited = std::time::Instant::now();
    let mut second: i64 = 40;
    loop {
        // Turning, because only a flush hands the waiting entries across
        // again; the relay drains what it already holds meanwhile.
        decisions.turn(t(second))?;
        second += 1;
        let shipped: usize = batches(&*store)?.iter().map(|b| b.entries.len()).sum();
        if shipped > 0 && decisions.cell().journal().retained() == 0 {
            break;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "the journal did not reach the store once it was released"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let shipped = batches(&*store)?;
    for pair in shipped.windows(2) {
        pair[1].verify_against(&pair[0].tail_digest())?;
    }
    Ok(())
}

#[test]
fn the_journal_in_memory_stays_under_its_bound_when_healthy_and_halts_new_exposure_at_it_when_stalled()
-> Result<()> {
    const BOUND: usize = 16;

    // Healthy: the store answers, so every flush ships and trims. The
    // journal records far more than the bound over the run while what it
    // holds in memory never reaches it — the unbounded growth W0.9 names
    // ("red-team m6: the default grows without bound on a cell that runs
    // for days") is what a journal that does not trim on ship does here.
    let mut healthy = rig(Arc::new(MemoryKeyValueStore::new()), BOUND, 1_024)?;
    let decisions = &mut healthy.decisions;
    let (_, retained, pressures) = drive(decisions, 10, 40);
    let recorded = decisions.cell().journal().len();
    assert!(
        recorded > 2 * BOUND,
        "the premise is a run that recorded well past the bound, and it recorded {recorded}"
    );
    assert!(
        retained.iter().all(|held| *held < BOUND),
        "the journal held {retained:?} in memory against a bound of {BOUND} while every flush \
         shipped"
    );
    assert!(
        pressures.iter().all(|p| *p == JournalPressure::Normal),
        "a healthy outbox read as {pressures:?}"
    );

    // Stalled: nothing ships, so what the journal holds grows — and the
    // bound narrows and then halts new exposure before it is far exceeded.
    let store = StallingStore::stalled();
    let mut stalled = rig(Arc::clone(&store) as Arc<dyn KeyValueStore>, BOUND, 2)?;
    let decisions = &mut stalled.decisions;
    let (_, retained, pressures) = drive(decisions, 10, 60);
    assert!(
        pressures
            .iter()
            .any(|p| matches!(p, JournalPressure::Narrow(_))),
        "the bound halted without narrowing first: {pressures:?}"
    );
    assert!(
        pressures.contains(&JournalPressure::Exhausted(Exhaustion::OverBudget)),
        "the bound never halted new exposure: {pressures:?}"
    );
    assert!(
        decisions.cell().is_halted(),
        "the cell trades on at the bound"
    );
    // Once halted the cell sends nothing, so what it still journals is
    // bounded by what one turn's halt bookkeeping adds, not by uptime.
    let at_halt = retained
        .iter()
        .zip(&pressures)
        .find(|(_, p)| p.halts())
        .map(|(held, _)| *held)
        .expect("a halting turn exists");
    let last = retained.last().copied().unwrap_or(0);
    assert!(
        last <= at_halt + BOUND,
        "the journal kept growing after the halt: {at_halt} at the halt, {last} at the end"
    );
    store.release();
    Ok(())
}

#[test]
fn market_events_the_feed_applied_reach_the_tick_lake_with_exchange_time_capture_time_and_sequence()
-> Result<()> {
    // W1.13's edge half: until this the node's ticks reached no
    // `qip_storage::lake` writer at all, and had they, the sequence number
    // the venue's publisher wrote was nowhere in the record.
    let mut rig = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 1_024)?;
    let lake: Arc<MemoryBlobStore> = Arc::new(MemoryBlobStore::new());
    let sinks: Vec<Box<dyn Sink>> = vec![Box::new(LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        CELL,
        qip_edge_node::event_fabric::drain::default_retention(),
        qip_storage::lake_codec::SegmentCodec::Identity,
    )?)];
    let mut drain = Drain::open(
        Arc::clone(&rig.spool),
        sinks,
        rig.drain_publisher
            .take()
            .expect("the publisher is handed once"),
        Arc::clone(&rig.telemetry),
        DrainConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
            64,
        )?,
    )?;

    let turn = rig.decisions.turn(t(10))?;
    assert_eq!(
        turn.captured, 2,
        "the premise is a pass that published the two seeded levels"
    );

    // The pass that applied them reaches the spool beside them, so a replay
    // has the marker the ticks belong to. Read before the drain runs: the
    // drain retires what every sink has taken, which is the point of it.
    let waited = std::time::Instant::now();
    loop {
        let markers = writer::records(&rig.spool)?
            .into_iter()
            .filter(|record| record.event.topic == REFLEX_PASS_MARKED.topic)
            .count();
        if markers >= 1 {
            break;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "no pass marker reached the spool"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let waited = std::time::Instant::now();
    let keys = loop {
        drain.step()?;
        let keys = segment_keys(lake.as_ref())?;
        if !keys.is_empty() {
            break keys;
        }
        assert!(waited.elapsed() < PATIENCE, "no tick reached the lake");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(
        keys.iter().all(|key| key.contains("/venue=XLON/")
            && key.contains("/instrument=obj-ACME/")
            && key.contains("/entitlement=qip-simulated-depth/")),
        "a tick was partitioned somewhere other than its venue, instrument and licence: {keys:?}"
    );
    let mut sequences = Vec::new();
    for key in &keys {
        let body = lake.get(key)?.expect("a listed segment is readable");
        for line in String::from_utf8(body).expect("canonical JSON").lines() {
            let tick: serde_json::Value = serde_json::from_str(line)?;
            assert_eq!(tick["feed"], FEED_NAME);
            assert!(
                tick["exchange_time_ns"].as_i64().is_some()
                    && tick["capture_time_ns"].as_i64() == Some(t(10).as_nanos()),
                "a tick lost its exchange or capture instant: {tick}"
            );
            sequences.push(tick["sequence"].as_u64().expect("a sequence number"));
        }
    }
    sequences.sort_unstable();
    assert_eq!(
        sequences,
        vec![1, 2],
        "the lake does not carry the feed's own sequence for each tick"
    );
    Ok(())
}

/// A pass with a peer configured publishes through the port, and a peer mesh
/// whose every peer is unreachable holds neither the decision thread nor the
/// journal (R12-MESH-5050, ADR 0116): the turns stay short, passes run, the
/// port takes the cell's offers, the journal reaches its store, and a policy
/// the centre ships still applies — while the link counts the peer it could
/// not reach.
///
/// The policy travels the way production's does: the centre puts a signed
/// payload on its mesh inbox, the node's `MeshWorker` polls it on its own
/// thread, and `DecisionLoop::turn` applies it in `apply_mesh` — with the
/// peer link down the whole time. Until 2026-10-11 this test called
/// `cell_mut().apply_policy` itself, which bypassed the loop, so a loop that
/// gated control distribution on peer health would still have passed it.
///
/// Mutation: make `ReflexPort::try_publish` a blocking `send` and stop the
/// session thread from draining the queue — the fifth turn blocks and the
/// drive's watchdog aborts the test. Mutation: drop the `try_publish` call
/// from `DecisionLoop::turn` — the session never turns on an offer and the
/// wire never tries the peer, so the `send_failures` premise times out.
/// Mutation (run): make `DecisionLoop::apply_mesh` return early while the
/// peer port reports no frame sent — the policy never applies and the wait
/// for sequence 2 times out.
#[test]
fn an_unreachable_peer_mesh_holds_neither_the_decision_thread_nor_the_journal() -> Result<()> {
    use qip_peer_link::{DEFAULT_CLOCK_BOUND, PeerSettings};
    use qip_transport::tls::LinkSecurity;
    use std::collections::BTreeMap;
    let free = || {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|probe| probe.local_addr())
            .expect("a loopback port")
    };
    let (port, _) = qip_peer_link::spawn(
        &PeerSettings {
            node: CELL.to_string(),
            listen: free(),
            // Nothing listens here: every connect is refused.
            peers: BTreeMap::from([("tokyo-1".to_string(), free())]),
            security: LinkSecurity::plaintext_for_tests(),
            clock_bound: DEFAULT_CLOCK_BOUND,
        },
        Arc::new(qip_core::SystemClock),
    )?;
    let store = Arc::new(MemoryKeyValueStore::new());
    let rig = rig(Arc::clone(&store) as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    // The centre, on a real socket, with nothing on its inbox yet.
    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let centre = CentreServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    let link = MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: centre.url(),
            seed: 3,
        },
        ENVELOPE_KEY,
        Arc::new(SystemClock),
        Arc::new(RecordingSleeper::new()),
    )?;
    let worker = MeshWorker::spawn(
        link,
        None,
        Arc::new(SystemClock),
        std::time::Duration::from_millis(5),
        8,
    )?;
    let mut decisions = rig.decisions.with_peers(port).with_mesh(worker);

    let (longest, _, _) = drive(&mut decisions, 10, 20);
    assert!(
        decisions.stats().passes > 0,
        "the premise is a loop that ran passes"
    );
    // The wire's thread connects asynchronously; give it the time to find
    // the peer unreachable before reading the premise.
    let waited = std::time::Instant::now();
    while decisions
        .peers()
        .expect("the loop holds the port")
        .status()
        .send_failures
        == 0
    {
        assert!(
            waited.elapsed() < PATIENCE,
            "premise: the wire never tried the unreachable peer"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let wire = decisions.peers().expect("the loop holds the port").status();
    assert_eq!(wire.sent, 0, "premise: nothing reached a peer: {wire:?}");
    assert!(
        longest < std::time::Duration::from_millis(500),
        "a turn took {longest:?} beside an unreachable peer"
    );

    let watchdog = Watchdog::arm("the journal reaching its store beside an unreachable peer");
    let waited = std::time::Instant::now();
    let mut second: i64 = 40;
    loop {
        decisions.turn(t(second))?;
        second += 1;
        let shipped: usize = batches(&*store)?.iter().map(|b| b.entries.len()).sum();
        if shipped > 0 {
            break;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "the journal did not ship while the peer mesh was down"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Control distribution does not ride the peer mesh either: a fresh
    // policy the centre publishes reaches the cell through the loop while
    // every peer is still down.
    drop(watchdog);
    let _watchdog =
        Watchdog::arm("the centre's policy reaching the cell beside an unreachable peer");
    assert_eq!(
        decisions.cell().policy_sequence(),
        Some(1),
        "premise: the rig applied sequence 1 and nothing newer"
    );
    let named = grant()?.signature().to_string();
    let mut payload = PolicyPayload::unproduced(2, CELL, t(second));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![named],
        },
        t(second),
    );
    let frame = policy_frame(payload.signed(ENVELOPE_KEY)?, t(second))?;
    let admission = inbox.accept(&MeshMessage {
        key: frame.event_id.as_str().to_string(),
        attempt: 1,
        frame,
    });
    assert!(
        matches!(admission, Admission::Accepted(_)),
        "premise: the centre's inbox took the policy: {admission:?}"
    );
    let waited = std::time::Instant::now();
    while decisions.cell().policy_sequence() != Some(2) {
        decisions.turn(t(second))?;
        second += 1;
        assert!(
            waited.elapsed() < PATIENCE / 2,
            "the centre's policy did not reach the cell through the loop while the peer mesh \
             was down: {:?}",
            decisions.peers().map(ReflexPort::status)
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let wire = decisions.peers().expect("the loop holds the port").status();
    assert_eq!(
        wire.sent, 0,
        "premise: still nothing reached a peer: {wire:?}"
    );
    assert!(centre.served() > 0, "premise: the worker polled the centre");
    Ok(())
}

/// The centre's policy frame, written from the vocabulary alone, as the
/// node's own mesh tests write it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
struct PolicyBody(qip_contracts::policy::PolicyPayload);

impl EventBody for PolicyBody {
    const TOPIC: Topic = PolicyPayloadTopic::TOPIC;
    const SCHEMA_VERSION: u32 = PolicyPayloadTopic::SCHEMA_VERSION;
}

fn policy_frame(payload: qip_contracts::policy::PolicyPayload, at: Timestamp) -> Result<AnyEvent> {
    let id = format!("EVT-POLICY-{}", payload.sequence);
    Envelope::new(
        Id::from_string(id.clone()),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR{id}")),
            "decision-loop-tests",
        ),
        PolicyBody(payload),
    )
    .erase()
}

/// A mesh endpoint on a real loopback port, served on its own thread: the
/// centre as the node's `MeshWorker` reaches it.
struct CentreServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl CentreServer {
    fn spawn(endpoint: MeshEndpoint) -> Result<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|error| qip_core::error::Error::io(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| qip_core::error::Error::io(error.to_string()))?
            .to_string();
        listener
            .set_nonblocking(true)
            .map_err(|error| qip_core::error::Error::io(error.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicU64::new(0));
        let (thread_stop, thread_served) = (Arc::clone(&stop), Arc::clone(&served));
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        thread_served.fetch_add(1, Ordering::SeqCst);
                        serve_one(stream, &endpoint);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            stop,
            served,
            handle: Some(handle),
        })
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn served(&self) -> u64 {
        self.served.load(Ordering::SeqCst)
    }
}

impl Drop for CentreServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_one(mut stream: std::net::TcpStream, endpoint: &MeshEndpoint) {
    use std::io::{BufRead, BufReader, Read, Write};
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let Some(method) = parts.next().and_then(Method::parse) else {
        return;
    };
    let Some(target) = parts.next().map(str::to_string) else {
        return;
    };
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let response = endpoint.handle(method, &target, &body);
    let mut out = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.status,
        qip_transport::EndpointResponse::CONTENT_TYPE,
        response.body.len()
    )
    .into_bytes();
    out.extend_from_slice(&response.body);
    let _ = stream.write_all(&out);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// The cell's acknowledgement of a policy it applied reaches the fabric's
/// spool after the turn that flushed it, on the journal stream the drain
/// delivers to `reflex-journal.local` (the policy-ack half of
/// R12-REFLEX-5034; telemetry is not yet a fabric record).
///
/// Mutation: drop `Decision::PolicyApplied` entries in `SpoolWriter`'s
/// journal handling (or stop flushing in `DecisionLoop::turn`) and the
/// acknowledgement never reaches the spool.
/// A CA certificate `openssl req -x509` made for this test; its key was
/// deleted unread. Public material, enough for an identity that holds roots
/// and no certificate.
const FIXTURE_ROOT: &str = "-----BEGIN CERTIFICATE-----
MIIBqzCCAVOgAwIBAgIUYT43nsbEcR277bc6CUkHAo5ZbjowCgYIKoZIzj0EAwIw
IzEhMB8GA1UEAwwYcWlwIHJlYWRpbmVzcyBmaXh0dXJlIENBMCAXDTI2MTAxMTEx
MDEwOFoYDzIxMjYwOTE3MTEwMTA4WjAjMSEwHwYDVQQDDBhxaXAgcmVhZGluZXNz
IGZpeHR1cmUgQ0EwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAATQZpXAff4IZ68a
48wN/d71sb6foGUd8b+/60TuL8tLKMU3W8WOGutofeSFvdbum2VcgSIKSG5iAW4B
oZz5lLOuo2MwYTAdBgNVHQ4EFgQUW/eoZJFiL/VbokTiODWFqzpfZx8wHwYDVR0j
BBgwFoAUW/eoZJFiL/VbokTiODWFqzpfZx8wDwYDVR0TAQH/BAUwAwEB/zAOBgNV
HQ8BAf8EBAMCAQYwCgYIKoZIzj0EAwIDRgAwQwIfAxe48XHniN3HVZAeq+yZXgUP
y+tWW22AN6xQlVPo3gIgJH4TQbQ5DbBEaFi8NBD1YPhsi93qd8LFwl7M9NuVuYo=
-----END CERTIFICATE-----
";

/// ADR 0112's fail-closed rule, seen from outside the node: a fabric link
/// on CA Service with no current certificate is refused at every handshake,
/// and the health body a probe reads says so. Mutation: drop the
/// `workload_certificate` field from `health_body` (or never apply
/// `with_certificate`) and the parsed body has no such object; this fails.
#[test]
fn a_node_whose_fabric_certificate_is_not_current_says_so_in_its_health_body() -> Result<()> {
    use qip_edge_node::health::HealthSnapshot;
    use qip_edge_node::scheduler::HealthFacts;
    let store = Arc::new(MemoryKeyValueStore::new());
    let rig = rig(store as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    let identity = qip_transport::tls::WorkloadIdentity::awaiting_issuance(
        FIXTURE_ROOT.as_bytes(),
        Arc::new(SystemClock),
    )?;
    let snapshot = HealthSnapshot::new(r#"{"turns":0}"#);
    let mut decisions = rig
        .decisions
        .with_health(
            snapshot.clone(),
            HealthFacts {
                cell: CELL.to_string(),
                region: REGION.to_string(),
                venues: 1,
                halt_flag_path: None,
                storage_target: "memory".to_string(),
                durable: false,
                started_secs: 0,
            },
        )
        .with_certificate(Arc::new(qip_transport::cas::RenewalStatus::new(Arc::new(
            identity,
        ))));
    decisions.turn(t(10))?;
    let body: serde_json::Value = serde_json::from_str(&snapshot.read())
        .map_err(|error| qip_core::error::Error::invalid(error.to_string()))?;
    // Premise: the body is the decision thread's, not the placeholder.
    assert!(body.get("cell").is_some(), "{body}");
    assert_eq!(body["workload_certificate"]["current"], false, "{body}");
    assert_eq!(
        body["workload_certificate"]["source"],
        "certificate_authority_service"
    );
    Ok(())
}

#[test]
fn a_policy_the_cell_applied_is_acknowledged_on_the_fabric_spool_after_the_turn() -> Result<()> {
    use qip_contracts::reflex::Decision as Recorded;
    use qip_events::event_fabric::bindings::REFLEX_JOURNAL_RECORDED;
    use qip_events::event_fabric::policy::QosClass;
    let store = Arc::new(MemoryKeyValueStore::new());
    let mut rig = rig(Arc::clone(&store) as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    let acknowledged = |spool: &SegmentLog| -> Result<Option<QosClass>> {
        Ok(writer::records(spool)?
            .into_iter()
            .filter(|record| record.event.topic == REFLEX_JOURNAL_RECORDED.topic)
            .find(|record| {
                writer::journal_entry(&record.event).is_ok_and(|entry| {
                    matches!(entry.decision, Recorded::PolicyApplied { sequence: 7, .. })
                })
            })
            .map(|record| record.stream))
    };
    // Premise: nothing has acknowledged sequence 7 before it is applied.
    drive(&mut rig.decisions, 10, 2);
    assert_eq!(acknowledged(&rig.spool)?, None);

    let named = grant()?.signature().to_string();
    let mut payload = PolicyPayload::unproduced(7, CELL, t(20));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![named],
        },
        t(20),
    );
    let policy = VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(20))?;
    rig.decisions.cell_mut().apply_policy(policy, t(20))?;

    let _watchdog = Watchdog::arm("the policy acknowledgement reaching the spool");
    let waited = std::time::Instant::now();
    let mut second = 21;
    let stream = loop {
        rig.decisions.turn(t(second))?;
        second += 1;
        if let Some(stream) = acknowledged(&rig.spool)? {
            break stream;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "the policy acknowledgement never reached the fabric spool"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(stream, QosClass::P2MarketJournal);
    Ok(())
}

/// The decision loop's offer from a running, funded cell authorises, at each
/// book it prices, a buy at the ask and a sell at the bid, sized so that all
/// of them together fit inside what the region has free; once the cell is
/// halted the same loop authorises nothing (MESH-5046, MESH-5062). These are
/// the only legs the peer link's session may reserve or send.
///
/// Mutation: drop the `is_halted` check from `leg_authorities` and the halted
/// offer still authorises; size each authority to the whole free balance
/// and the notional sum exceeds it.
#[test]
fn a_running_funded_cell_authorises_legs_within_its_free_capital_and_a_halted_one_none()
-> Result<()> {
    use qip_edge::cell::PolledHalt;
    use qip_edge_node::edge_quotes::cell_offer;
    use qip_peer_link::boundary::Side as LegSide;
    let store = Arc::new(MemoryKeyValueStore::new());
    let mut rig = rig(Arc::clone(&store) as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    rig.decisions.turn(t(10))?;
    let cell = rig.decisions.cell();
    let free = cell
        .region_allocation_free()
        .expect("the rig's cell holds an allocation");
    assert!(
        free.is_positive(),
        "premise: the rig's share left capital free"
    );
    let offer = cell_offer(cell, t(11));
    assert_eq!(offer.quotes.len(), 1, "premise: the rig prices one book");
    assert_eq!(offer.legs.len(), 2, "a buy and a sell: {:?}", offer.legs);
    let quote = &offer.quotes[0];
    let mut notional = Decimal::ZERO;
    for leg in &offer.legs {
        match leg.side {
            LegSide::Buy => {
                assert_eq!(leg.limit_price, quote.ask);
                assert_eq!(leg.touch_size, quote.ask_size);
            }
            LegSide::Sell => {
                assert_eq!(leg.limit_price, quote.bid);
                assert_eq!(leg.touch_size, quote.bid_size);
            }
        }
        assert_eq!(leg.unwind_per_unit, quote.ask - quote.bid, "the spread");
        assert!(leg.valid_until_ns > t(11).as_nanos());
        notional += leg.max_size * leg.limit_price;
    }
    assert!(
        notional <= free,
        "the authorities together could commit {notional} of {free} free"
    );

    rig.decisions
        .cell_mut()
        .apply_polled_halt(PolledHalt::Engaged("drill".to_string()), t(12));
    assert!(
        rig.decisions.cell().is_halted(),
        "premise: the cell is halted"
    );
    let halted = cell_offer(rig.decisions.cell(), t(13));
    assert_eq!(
        halted.quotes.len(),
        1,
        "a halted cell still shows its touch"
    );
    assert!(
        halted.legs.is_empty(),
        "a halted cell authorised {:?}",
        halted.legs
    );
    Ok(())
}

/// REFLEX-8071: the previous payload is resident in the cell, and the node's
/// rollback request is what reaches it. Driven through `DecisionLoop::turn`,
/// the call the binary's timer makes, with the wire `main.rs` builds from
/// `QIP_POLICY_ROLLBACK_PATH` reading a real file on its own thread.
///
/// Mutation: drop the `apply_rollback` call from `DecisionLoop::turn` and the
/// cell is still serving sequence 2 after the turn.
#[test]
fn an_operator_rollback_request_returns_the_cell_to_its_previous_payload_on_the_next_turn()
-> Result<()> {
    use qip_edge_node::rollback::{RollbackReading, RollbackRequest, RollbackWire};
    let store = Arc::new(MemoryKeyValueStore::new());
    let Rig { decisions, .. } = rig(store as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    let dir = temp_dir("rollback");
    let path = dir.join("rollback");
    let wire = RollbackWire::spawn(
        RollbackRequest::at(&path)?,
        std::time::Duration::from_millis(5),
    )?;
    let mut decisions = decisions.with_rollback(wire);

    let named = grant()?.signature().to_string();
    let mut payload = PolicyPayload::unproduced(2, CELL, t(20));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![named],
        },
        t(20),
    );
    let policy = VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(20))?;
    decisions.cell_mut().apply_policy(policy, t(20))?;
    // Premise: with no request on disk, a turn leaves the cell where it is.
    drive(&mut decisions, 21, 2);
    assert_eq!(decisions.cell().policy_sequence(), Some(2));

    std::fs::write(&path, "2\n").expect("the request can be written");
    let rollback = decisions_wire_reading(&path);
    assert_eq!(rollback, RollbackReading::AwayFrom(2));
    let waited = std::time::Instant::now();
    while decisions.cell().policy_sequence() == Some(2) {
        assert!(
            waited.elapsed() < PATIENCE,
            "the rollback request never reached the cell through the decision loop"
        );
        decisions.turn(t(30))?;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        decisions.cell().policy_sequence(),
        Some(1),
        "the cell left sequence 2 for something other than its previous payload"
    );
    // The request stays on disk and acts once: further turns keep sequence 1.
    drive(&mut decisions, 31, 3);
    assert_eq!(decisions.cell().policy_sequence(), Some(1));
    Ok(())
}

/// What the request file reads as, through the same type the wire reads it
/// with — so the premise above is about the file the reader thread sees.
fn decisions_wire_reading(path: &std::path::Path) -> qip_edge_node::rollback::RollbackReading {
    qip_edge_node::rollback::RollbackRequest::at(path)
        .expect("an absolute path is accepted")
        .read()
}

#[test]
fn a_decision_loop_refuses_a_cell_built_without_the_journal_wire() -> Result<()> {
    // RES-10124: on fabric loss the cell spools boundedly and narrows on
    // pressure only if the composition root arms the journal wire it feeds.
    // `main.rs` arms it, and what keeps it armed is that the loop refuses a
    // cell without it — a node whose spool fills while the cell trades at
    // full size is the failure this wire exists to stop. Premise: the same
    // rig with the wire armed assembles.
    let wired = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 64);
    assert!(wired.is_ok(), "premise: the wired rig assembles");
    let unwired = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_journal_trimmed_on_ship();
    match rig_with(unwired, Arc::new(MemoryKeyValueStore::new()), 1 << 16, 64) {
        Ok(_) => panic!("a decision loop accepted a cell with no journal wire"),
        Err(error) => assert!(
            error.message().contains("built without the journal wire"),
            "refused for the wrong reason: {}",
            error.message()
        ),
    }
    Ok(())
}

#[test]
fn a_decision_loop_refuses_a_cell_that_keeps_shipped_journal_entries() -> Result<()> {
    // REFLEX-5032: the journal is append-only and hash-chained, and it is
    // bounded in memory only because `Cell::flush` drops what it shipped.
    // `main.rs` arms the trim; what keeps it armed is that the loop refuses
    // a cell without it, so a node whose journal grows with uptime cannot be
    // composed. Premise: the same rig with the trim armed assembles.
    let wired = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 64);
    assert!(wired.is_ok(), "premise: the trimmed rig assembles");
    let untrimmed = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_journal_wire();
    match rig_with(untrimmed, Arc::new(MemoryKeyValueStore::new()), 1 << 16, 64) {
        Ok(_) => panic!("a decision loop accepted a cell that keeps shipped journal entries"),
        Err(error) => assert!(
            error
                .message()
                .contains("keeps every shipped entry in memory"),
            "refused for the wrong reason: {}",
            error.message()
        ),
    }
    Ok(())
}

/// A peer that accepts every connection and never says a word: the hanging
/// centre or region, as against the refusing one. Counts what it accepted
/// and holds each stream open until the process ends.
fn silent_peer() -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let address = listener.local_addr().expect("a bound address");
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&accepted);
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            held.push(stream);
        }
    });
    (address, accepted)
}

#[test]
fn a_silent_centre_a_silent_peer_and_a_stalled_journal_store_together_hold_no_turn() -> Result<()> {
    // ARCH-10176: no synchronous call to warm services or another region
    // between a venue event and a local order. The refusing peer above is
    // the easy half: a refused connect returns at once wherever it runs. A
    // centre that accepts and then hangs is the half that would hold a turn
    // for the transport's whole timeout if the exchange ran inline, so both
    // the centre and the one peer here accept and stay silent.
    use qip_peer_link::{DEFAULT_CLOCK_BOUND, PeerSettings};
    use qip_transport::tls::LinkSecurity;
    use std::collections::BTreeMap;
    let (centre, centre_accepted) = silent_peer();
    let (peer, peer_accepted) = silent_peer();
    let listen = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|probe| probe.local_addr())
        .expect("a loopback port");
    let (port, _) = qip_peer_link::spawn(
        &PeerSettings {
            node: CELL.to_string(),
            listen,
            peers: BTreeMap::from([("tokyo-1".to_string(), peer)]),
            security: LinkSecurity::plaintext_for_tests(),
            clock_bound: DEFAULT_CLOCK_BOUND,
        },
        Arc::new(qip_core::SystemClock),
    )?;
    // The journal's store hangs too, so the centre, the region and the
    // durable record are all stalled at once.
    let store = StallingStore::stalled();
    let rig = rig(Arc::clone(&store) as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    let link = MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: format!("http://{centre}"),
            seed: 3,
        },
        ENVELOPE_KEY,
        Arc::new(SystemClock),
        Arc::new(RecordingSleeper::new()),
    )?;
    let worker = MeshWorker::spawn(
        link,
        None,
        Arc::new(SystemClock),
        std::time::Duration::from_millis(5),
        8,
    )?;
    let mut decisions = rig.decisions.with_peers(port).with_mesh(worker);

    // Premise: the worker is inside a request the centre will never answer
    // before the turns are timed, or the timing proves nothing about a hang.
    let waited = std::time::Instant::now();
    while centre_accepted.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(
            waited.elapsed() < PATIENCE,
            "premise: the mesh worker never reached the silent centre"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let hanging_since = std::time::Instant::now();
    let (longest, _, _) = drive(&mut decisions, 10, 40);
    let waited = std::time::Instant::now();
    while store.attempts() == 0 {
        assert!(
            waited.elapsed() < PATIENCE,
            "premise: the relay never reached the stalled store"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let during = hanging_since.elapsed();
    assert!(
        decisions.stats().passes >= 20,
        "premise: the forty turns ran passes: {:?}",
        decisions.stats()
    );
    // The venue event became a local order with every remote stalled.
    assert!(
        decisions.stats().orders > 0,
        "the venue's touch produced no order beside the stalled dependencies: {:?}",
        decisions.stats()
    );
    assert!(
        longest < std::time::Duration::from_millis(250),
        "a turn took {longest:?} while the centre held a request open"
    );
    // Forty turns inside the window the centre was hanging: a turn that
    // waited on it would have taken at least the transport's timeout.
    assert!(
        during < std::time::Duration::from_secs(5),
        "forty turns took {during:?} beside a silent centre"
    );
    // The peer wire dials on its own thread and schedule, so its reaching
    // the silent peer is not required here; the centre half carries the
    // hang. Whatever it did, it held no turn.
    drop(peer_accepted);
    store.release();
    Ok(())
}

/// The turn feeds the peer link's measured round trip to the scrape
/// registry: two linked nodes, the first held by a decision loop that was
/// given a `PeerSeries`, and the gauge appears from turns alone. Without the
/// per-turn read a node could measure its peer and chart nothing, which is
/// the state R12-MESH-1190 closes.
///
/// Mutation: drop the `series.observe` call from `DecisionLoop::turn` and the
/// wait times out with the gauge absent.
#[test]
fn the_decision_loop_publishes_the_peer_round_trip_it_reads_from_the_port() -> Result<()> {
    use qip_observability::metrics::{Metrics, labels, names};
    use qip_peer_link::{DEFAULT_CLOCK_BOUND, PeerSettings};
    use qip_transport::tls::LinkSecurity;
    use std::collections::BTreeMap;
    let free = || {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|probe| probe.local_addr())
            .expect("a loopback port")
    };
    let (a_at, b_at) = (free(), free());
    let spawn = |node: &str, at, peer: &str, peer_at| {
        qip_peer_link::spawn(
            &PeerSettings {
                node: node.to_string(),
                listen: at,
                peers: BTreeMap::from([(peer.to_string(), peer_at)]),
                security: LinkSecurity::plaintext_for_tests(),
                clock_bound: DEFAULT_CLOCK_BOUND,
            },
            Arc::new(SystemClock),
        )
        .map(|(port, _)| port)
    };
    let port = spawn(CELL, a_at, "tokyo-1", b_at)?;
    let _peer = spawn("tokyo-1", b_at, CELL, a_at)?;
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let series = qip_edge_node::telemetry::PeerSeries::new(Arc::clone(&metrics), CELL, REGION);
    let store = Arc::new(MemoryKeyValueStore::new());
    let rig = rig(store as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    let mut decisions = rig.decisions.with_peers(port).with_peer_series(series);
    let key = labels([("cell", CELL), ("region", REGION)]);
    assert!(
        metrics
            .snapshot()
            .gauge(names::EDGE_PEER_ROUND_TRIP_MS, &key)
            .is_none(),
        "premise: nothing is recorded before a turn has read a measured clock"
    );
    let waited = std::time::Instant::now();
    let mut second: i64 = 10;
    loop {
        decisions.turn(t(second))?;
        second += 1;
        if let Some(ms) = metrics
            .snapshot()
            .gauge(names::EDGE_PEER_ROUND_TRIP_MS, &key)
        {
            assert!(ms > 0.0, "a measured round trip of {ms} ms");
            return Ok(());
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "no peer round trip reached the registry"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// R12-RES-7095: when the peer reflex mesh degrades, local trading goes on
/// and a cycle needing coordination is refused rather than waited for.
///
/// The node coordinates (`london-1` sorts before `tokyo-1`) a cross-region
/// cycle its own decision loop authorised through `DecisionLoop::turn`,
/// priced against the touch tokyo showed. tokyo authorises no leg of its
/// own, so it can never reserve and FIRE is impossible whatever the timing;
/// then tokyo's port is dropped, which ends its session thread and closes
/// its listener: the peer is gone mid-epoch. The loop then turns past the
/// epoch's TTL, and both halves of the requirement are asserted on the same
/// passes: the originated cycle lapsed unfired and is counted as such
/// (`PeerStatus::cycles_expired`), no FIRE was declared and no leg was sent,
/// and the single-venue strategy kept placing paper orders at the simulated
/// venue throughout.
///
/// Until the counter existed `Saga::forget_gone` dropped such a cycle in
/// silence, and a peer lost mid-epoch read in the status exactly like a
/// cycle never offered: the property had no assertion to hang on.
///
/// Both session threads run on one `InstantClock` anchored at the rig's
/// instants, and every turn is stamped from it. The leg authorities the loop
/// publishes are stamped with the turn's instant and the session judges them
/// on its own clock, so turns on the test's fixed calendar beside a session
/// on the wall clock would see every authority as long expired and
/// originate nothing.
///
/// Mutation (run): drop the `lapsed += 1` from `Saga::forget_gone`, and the
/// wait for `cycles_expired` times out with the epoch long gone.
/// Mutation (run): drop the `cycles_expired` line from `Host::record`, and
/// the same wait times out.
#[test]
fn a_cycle_whose_peer_is_lost_mid_epoch_is_refused_at_its_ttl_while_the_single_venue_strategy_keeps_trading()
-> Result<()> {
    use qip_peer_link::boundary::{EdgeQuote, Offer, PeerStatus};
    use qip_peer_link::{DEFAULT_CLOCK_BOUND, PeerSettings};
    use qip_transport::tls::LinkSecurity;
    use std::collections::BTreeMap;
    /// Longer than this file's `PATIENCE`: a first connect that finds no
    /// listener backs off for a second, clocks are probed every 250 ms, and
    /// a saturated workspace run shares the cores with hundreds of threads.
    const MESH_PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);
    /// Real time between turns. The simulated venue allows fifty orders a
    /// second and the strategy enters on every pass, so turns closer than
    /// twenty milliseconds make the venue's rate limit, not the peer loss,
    /// the reason a pass placed nothing.
    const TURN_GAP_MS: u64 = 25;
    let free = || {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|probe| probe.local_addr())
            .expect("a loopback port")
    };
    let clock: Arc<InstantClock> = Arc::new(InstantClock::anchored_at(t(10)));
    let spawn = |node: &str, at, peer: &str, peer_at| {
        qip_peer_link::spawn(
            &PeerSettings {
                node: node.to_string(),
                listen: at,
                peers: BTreeMap::from([(peer.to_string(), peer_at)]),
                security: LinkSecurity::plaintext_for_tests(),
                clock_bound: DEFAULT_CLOCK_BOUND,
            },
            Arc::clone(&clock) as Arc<dyn Clock>,
        )
        .map(|(port, _)| port)
    };
    // A port taken between the probe and the bind is an `Err` from `spawn`,
    // not a finding; a fresh pair turns that race into a retry.
    let mut pair = None;
    for _ in 0..10 {
        let (london_at, tokyo_at) = (free(), free());
        if let (Ok(london), Ok(tokyo)) = (
            spawn(CELL, london_at, "tokyo-1", tokyo_at),
            spawn("tokyo-1", tokyo_at, CELL, london_at),
        ) {
            pair = Some((london, tokyo));
            break;
        }
    }
    let (london, tokyo) = pair.expect("two loopback listeners within ten attempts");
    // tokyo bids 103 for what this cell offers at 101: two hundred basis
    // points, far past `MIN_EDGE_BPS`. Its touch only, and no leg authority,
    // so whatever reaches tokyo it can only decline.
    let tokyo_touch = |at: Timestamp| Offer {
        quotes: vec![EdgeQuote {
            venue: "XTKS".into(),
            instrument: object().as_str().to_string(),
            bid: dec!("103"),
            bid_size: dec!("50"),
            ask: dec!("104"),
            ask_size: dec!("50"),
            as_of_ns: at.as_nanos(),
        }],
        legs: Vec::new(),
        withheld: 0,
    };

    let store = Arc::new(MemoryKeyValueStore::new());
    let rig = rig(store as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    let mut decisions = rig.decisions.with_peers(london);
    let status = |decisions: &DecisionLoop| -> PeerStatus {
        decisions.peers().expect("the loop holds the port").status()
    };

    // The cycle opens: the loop turns on the shared clock, publishing the
    // cell's touch and legs, while tokyo shows its touch.
    let began = std::time::Instant::now();
    loop {
        let now = clock.now();
        tokyo.try_publish(tokyo_touch(now));
        decisions.turn(now)?;
        if status(&decisions).cycles_originated >= 1 {
            break;
        }
        assert!(
            began.elapsed() < MESH_PATIENCE,
            "premise: the node never originated a cycle against tokyo's touch: {:?}",
            status(&decisions)
        );
        std::thread::sleep(std::time::Duration::from_millis(TURN_GAP_MS));
    }
    let opened = status(&decisions);
    assert_eq!(
        opened.cycles_expired, 0,
        "premise: nothing had lapsed when the cycle opened: {opened:?}"
    );
    assert_eq!(
        opened.fires, 0,
        "premise: tokyo authorised nothing: {opened:?}"
    );

    // The peer is lost mid-epoch.
    drop(tokyo);
    let orders_at_loss = decisions.stats().orders;
    let passes_at_loss = decisions.stats().passes;

    // Turn on, past the epoch's TTL, until the node has let the cycle go.
    let began = std::time::Instant::now();
    loop {
        decisions.turn(clock.now())?;
        if status(&decisions).cycles_expired >= 1 {
            break;
        }
        assert!(
            began.elapsed() < MESH_PATIENCE,
            "the cycle the lost peer never reserved was not counted as lapsed: {:?}",
            status(&decisions)
        );
        std::thread::sleep(std::time::Duration::from_millis(TURN_GAP_MS));
    }
    let after = status(&decisions);
    assert!(
        after.cycles_expired <= after.cycles_originated,
        "more cycles lapsed than were opened: {after:?}"
    );
    assert_eq!(
        after.fires, 0,
        "a cycle whose peer was lost was fired: {after:?}"
    );
    assert_eq!(after.legs_sent, 0, "a leg was sent without FIRE: {after:?}");

    // The same passes traded locally: the strategy kept placing paper
    // orders at the simulated venue while the cycle waited and lapsed.
    let stats = decisions.stats();
    assert!(
        orders_at_loss > 0,
        "premise: the single-venue strategy was trading before the loss: {stats:?}"
    );
    assert!(
        stats.passes > passes_at_loss,
        "premise: passes ran after the peer was lost: {stats:?}"
    );
    assert!(
        stats.orders > orders_at_loss,
        "the single-venue strategy placed nothing after the peer was lost: {orders_at_loss} \
         before, {stats:?} after"
    );
    Ok(())
}

#[test]
fn a_halt_the_cell_applied_is_acknowledged_on_the_fabric_spool_after_the_turn() -> Result<()> {
    // FABRIC-10076: control acknowledgments reach the regional Fabric, and a
    // halt is the control whose acknowledgment matters most — the centre
    // must be able to read that the cell stopped, not infer it from
    // silence. The policy half is the test above; this is the halt half,
    // through the same journal relay and spool writer the node runs.
    use qip_contracts::reflex::Decision as Recorded;
    use qip_edge::cell::PolledHalt;
    use qip_events::event_fabric::bindings::REFLEX_JOURNAL_RECORDED;
    let store = Arc::new(MemoryKeyValueStore::new());
    let mut rig = rig(Arc::clone(&store) as Arc<dyn KeyValueStore>, 1 << 16, 64)?;
    let acknowledged = |spool: &SegmentLog| -> Result<bool> {
        Ok(writer::records(spool)?
            .into_iter()
            .filter(|record| record.event.topic == REFLEX_JOURNAL_RECORDED.topic)
            .any(|record| {
                writer::journal_entry(&record.event).is_ok_and(|entry| {
                    matches!(entry.decision, Recorded::HaltChanged { halted: true, .. })
                })
            }))
    };
    // Premise: a running cell whose spool holds no halt yet.
    drive(&mut rig.decisions, 10, 2);
    assert!(!rig.decisions.cell().is_halted(), "premise: a running cell");
    assert!(!acknowledged(&rig.spool)?, "premise: no halt on the spool");

    rig.decisions.cell_mut().apply_polled_halt(
        PolledHalt::Unreadable("the operator's halt flag".to_string()),
        t(20),
    );
    assert!(rig.decisions.cell().is_halted(), "premise: the halt held");

    let _watchdog = Watchdog::arm("the halt acknowledgement reaching the spool");
    let waited = std::time::Instant::now();
    let mut second = 21;
    while !acknowledged(&rig.spool)? {
        rig.decisions.turn(t(second))?;
        second += 1;
        assert!(
            waited.elapsed() < PATIENCE,
            "the halt acknowledgement never reached the fabric spool"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(())
}

#[test]
fn a_cell_whose_centre_has_gone_silent_keeps_journaling_its_orders_to_the_fabric_spool()
-> Result<()> {
    // RES-10126: with the regional warm services lost the cell keeps
    // executing locally and the Fabric keeps journaling. The centre here
    // accepts and never answers, so no new policy or capital reaches the
    // cell, and the orders its passes place still reach the spool the drain
    // ships from — the outbox does not wait on the centre and is not routed
    // through it.
    use qip_contracts::reflex::Decision as Recorded;
    use qip_events::event_fabric::bindings::REFLEX_JOURNAL_RECORDED;
    let (centre, centre_accepted) = silent_peer();
    let rig = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 64)?;
    let link = MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: format!("http://{centre}"),
            seed: 3,
        },
        ENVELOPE_KEY,
        Arc::new(SystemClock),
        Arc::new(RecordingSleeper::new()),
    )?;
    let worker = MeshWorker::spawn(
        link,
        None,
        Arc::new(SystemClock),
        std::time::Duration::from_millis(5),
        8,
    )?;
    let mut decisions = rig.decisions.with_mesh(worker);
    let spool = rig.spool;
    let sent_on_spool = || -> Result<usize> {
        Ok(writer::records(&spool)?
            .into_iter()
            .filter(|record| record.event.topic == REFLEX_JOURNAL_RECORDED.topic)
            .filter(|record| {
                writer::journal_entry(&record.event)
                    .is_ok_and(|entry| matches!(entry.decision, Recorded::OrderSent { .. }))
            })
            .count())
    };
    let waited = std::time::Instant::now();
    while centre_accepted.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(
            waited.elapsed() < PATIENCE,
            "premise: the mesh worker never reached the silent centre"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(sent_on_spool()?, 0, "premise: nothing journaled yet");
    let policy_before = decisions.cell().policy_sequence();

    let _watchdog = Watchdog::arm("orders reaching the spool beside a silent centre");
    let mut second = 10;
    while sent_on_spool()? == 0 {
        decisions.turn(t(second))?;
        second += 1;
        assert!(
            waited.elapsed() < PATIENCE,
            "no order reached the fabric spool while the centre was silent: {:?}",
            decisions.stats()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        decisions.stats().orders > 0,
        "premise: the cell placed orders"
    );
    assert_eq!(
        decisions.cell().policy_sequence(),
        policy_before,
        "a silent centre delivered new authority"
    );
    Ok(())
}

#[test]
fn a_policy_the_centre_publishes_while_the_fabric_is_fenced_is_held_and_not_applied() -> Result<()>
{
    // RES-7113: on fabric loss new control changes stop. A fenced producer
    // reads exhausted, so the spool cannot journal the PolicyApplied entry
    // an application would write, and a control change the event log cannot
    // replay is one the desk could never attribute afterwards. The centre's
    // policy reaches the loop through the real worker and is held; the cell
    // keeps the sequence it already had and keeps deciding under it.
    let mut rig = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 64)?;
    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let centre = CentreServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    let link = MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: centre.url(),
            seed: 3,
        },
        ENVELOPE_KEY,
        Arc::new(SystemClock),
        Arc::new(RecordingSleeper::new()),
    )?;
    let worker = MeshWorker::spawn(
        link,
        None,
        Arc::new(SystemClock),
        std::time::Duration::from_millis(5),
        8,
    )?;
    rig.drain_publisher
        .as_mut()
        .expect("the rig hands back the drain's publisher")
        .mark_fenced();
    let mut decisions = rig.decisions.with_mesh(worker);
    assert_eq!(
        decisions.cell().policy_sequence(),
        Some(1),
        "premise: the rig applied sequence 1 and nothing newer"
    );

    let named = grant()?.signature().to_string();
    let mut payload = PolicyPayload::unproduced(2, CELL, t(10));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![named],
        },
        t(10),
    );
    let frame = policy_frame(payload.signed(ENVELOPE_KEY)?, t(10))?;
    let admission = inbox.accept(&MeshMessage {
        key: frame.event_id.as_str().to_string(),
        attempt: 1,
        frame,
    });
    assert!(
        matches!(admission, Admission::Accepted(_)),
        "premise: the centre's inbox took the policy: {admission:?}"
    );

    let _watchdog = Watchdog::arm("the held policy reaching the loop");
    let waited = std::time::Instant::now();
    let mut second = 10;
    // Premise: the worker has polled the centre that holds the policy.
    while centre.served() == 0 {
        decisions.turn(t(second))?;
        second += 1;
        assert!(
            waited.elapsed() < PATIENCE / 2,
            "the worker's polls never reached the fenced loop"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Long enough that an unheld policy would have applied: the outage test
    // above applies one within a handful of turns.
    for _ in 0..40 {
        decisions.turn(t(second))?;
        second += 1;
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        decisions.cell().policy_sequence(),
        Some(1),
        "the cell applied a policy the fenced fabric could not journal"
    );
    assert!(
        decisions.held_control() > 0,
        "the loop let go of what it held"
    );
    Ok(())
}

#[test]
fn a_normalized_time_and_uncertainty_on_the_lake_line_come_from_the_disciplined_clock() -> Result<()>
{
    // TICK-1122, TICK-2051, TICK-2052, TICK-2089: capture-time clock
    // discipline on a production path. Every tick the node's passes apply
    // reaches the tick lake through the drain's LakeSink, which runs it
    // through qip_sequencing's CaptureStream: the line carries the
    // normalized instant and its uncertainty as fields of their own, and
    // every correction the discipline applied to reach them. The proof is
    // an independent discipline over the same ticks in the same order: it
    // must reproduce every line's clock block exactly, and once it holds its
    // minimum samples the lines must carry its offset estimate.
    use qip_contracts::market_event::MarketEvent;
    use qip_sequencing::{CaptureStream, DisciplineSettings};
    let mut rig = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 1_024)?;
    let lake: Arc<MemoryBlobStore> = Arc::new(MemoryBlobStore::new());
    let sinks: Vec<Box<dyn Sink>> = vec![Box::new(LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        CELL,
        qip_edge_node::event_fabric::drain::default_retention(),
        qip_storage::lake_codec::SegmentCodec::Identity,
    )?)];
    let mut drain = Drain::open(
        Arc::clone(&rig.spool),
        sinks,
        rig.drain_publisher
            .take()
            .expect("the publisher is handed once"),
        Arc::clone(&rig.telemetry),
        DrainConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
            64,
        )?,
    )?;

    // Forty turns, each with a moved touch so every pass publishes.
    let mut captured = 0usize;
    for second in 10..50 {
        let simulated = rig
            .decisions
            .gateway_mut()
            .simulated_mut()
            .expect("the rig's gateway is simulated");
        let bid = dec!("99") - Decimal::from(second % 3);
        simulated.seed_touch(&object(), Side::Buy, bid, dec!("500"), t(second))?;
        captured += rig.decisions.turn(t(second))?.captured;
    }
    let settings = DisciplineSettings::default();
    assert!(
        captured > settings.min_samples,
        "premise: the passes applied enough ticks to warm the discipline ({captured})"
    );

    let waited = std::time::Instant::now();
    let mut lines: Vec<serde_json::Value> = Vec::new();
    while lines.len() < captured {
        drain.step()?;
        lines.clear();
        for key in segment_keys(lake.as_ref())? {
            let body = lake.get(&key)?.expect("a listed segment is readable");
            for line in String::from_utf8(body).expect("canonical JSON").lines() {
                lines.push(serde_json::from_str(line)?);
            }
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "only {} of {captured} ticks reached the lake",
            lines.len()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // One stream (one venue feed partition), so its sequence is the order
    // the drain disciplined it in.
    lines.sort_by_key(|line| line["sequence"].as_u64());

    let mut independent = CaptureStream::new(settings)?;
    let mut estimated = 0usize;
    for line in &lines {
        let event: MarketEvent = serde_json::from_value(line["event"].clone())?;
        let expected = independent.capture(&event, None)?;
        let clock = &line["clock"];
        assert_eq!(
            clock["normalized_time_ns"].as_i64(),
            Some(expected.normalized_time.as_nanos()),
            "a lake line's normalized instant is not the discipline's: {line}"
        );
        assert_eq!(
            clock["uncertainty_ns"].as_i64(),
            Some(expected.uncertainty.as_nanos()),
            "a lake line's uncertainty is not the discipline's: {line}"
        );
        assert_eq!(
            clock["corrections"],
            serde_json::to_value(&expected.corrections)?,
            "a lake line's correction lineage is not the discipline's: {line}"
        );
        if expected
            .corrections
            .iter()
            .any(|correction| correction.kind == "offset_estimate")
        {
            estimated += 1;
        }
    }
    assert!(
        estimated > 0,
        "no lake line carries the discipline's offset estimate after {} ticks",
        lines.len()
    );
    Ok(())
}

/// `gateway`, permitted to list each of `objects` on demand and to seed
/// liquidity into it: a recorded venue, product and jurisdiction permission
/// for each action on each instrument (R12-COVERAGE-5107). `Seed` as well as
/// `List`, because the tests here seed the touch they trade against and
/// permission to list is not permission to seed. A fixture; the refusals
/// without either are proven in qip-edge-node's tests/gateway.rs.
fn permitting_to_seed(
    gateway: SimulatedGateway,
    objects: &[&ObjectId],
) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for object in objects {
        for action in [
            CreationAction::Create,
            CreationAction::List,
            CreationAction::Seed,
        ] {
            let request = CreationRequest::new(action, venue.as_str(), object.as_str(), "paper");
            for authority in CreationAuthority::ALL {
                permissions.record(authority, &request, "test fixture")?;
            }
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}

/// R12-TICK-1201: what the node's passes applied goes through the drain's
/// lake sink as sealed segments, and the lake alone is enough to rebuild the
/// book the venue published. The segments are pinned and read back through
/// `qip_storage::lake_history` (the reader every replay job uses, checksums
/// verified), replayed with `qip_orderbook::replay`, and the rebuilt ladder
/// must equal the simulated venue's own depth after the last pass: an
/// independent truth, not a second copy of the feed.
///
/// Mutations: `LakeSink::deliver` skips every even-sequenced tick — the
/// drain wait fails ("only 37 of 73 ticks reached the lake"), bounded by
/// `PATIENCE`; the feed captures each level a tick above the price it
/// published — the bids rebuilt from the lake differ from the venue's.
#[test]
fn the_lake_alone_rebuilds_the_book_the_venue_published() -> Result<()> {
    use qip_contracts::venue::VenueStatus;
    use qip_orderbook::venue::VenueState;
    use qip_storage::lake::Lake;
    use qip_storage::lake_history::HistoryQuery;
    use std::collections::BTreeSet;

    let mut rig = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 1_024)?;
    let lake: Arc<MemoryBlobStore> = Arc::new(MemoryBlobStore::new());
    let sinks: Vec<Box<dyn Sink>> = vec![Box::new(LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        CELL,
        qip_edge_node::event_fabric::drain::default_retention(),
        qip_storage::lake_codec::SegmentCodec::Identity,
    )?)];
    let mut drain = Drain::open(
        Arc::clone(&rig.spool),
        sinks,
        rig.drain_publisher
            .take()
            .expect("the publisher is handed once"),
        Arc::clone(&rig.telemetry),
        DrainConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
            64,
        )?,
    )?;

    // Thirty turns, the touch moving every turn and a second level coming
    // and going, so the book the lake must rebuild is not the first one.
    let mut captured = 0usize;
    for second in 10..40 {
        let simulated = rig
            .decisions
            .gateway_mut()
            .simulated_mut()
            .expect("the rig's gateway is simulated");
        let bid = dec!("99") - Decimal::from(second % 3);
        simulated.seed_touch(&object(), Side::Buy, bid, dec!("500"), t(second))?;
        if second % 4 == 0 {
            simulated.seed_touch(&object(), Side::Sell, dec!("101"), dec!("300"), t(second))?;
        }
        captured += rig.decisions.turn(t(second))?.captured;
    }
    assert!(
        captured > 10,
        "premise: the passes applied ticks ({captured})"
    );

    let waited = std::time::Instant::now();
    loop {
        drain.step()?;
        let lines: usize = segment_keys(lake.as_ref())?
            .iter()
            .map(|key| {
                lake.get(key).ok().flatten().map_or(0, |body| {
                    body.split(|byte| *byte == b'\n')
                        .filter(|l| !l.is_empty())
                        .count()
                })
            })
            .sum();
        if lines >= captured {
            break;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "only {lines} of {captured} ticks reached the lake"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let granted: BTreeSet<String> =
        BTreeSet::from([qip_edge_node::feed::SIMULATED_DATASET.to_string()]);
    let reader = Lake::new(lake.as_ref());
    let listing = reader.pin_market_history(
        "tick-1201",
        &HistoryQuery {
            from_date: "2000-01-01".to_string(),
            to_date: "2100-01-01".to_string(),
            venues: BTreeSet::new(),
            instruments: BTreeSet::new(),
            granted: granted.clone(),
            max_segments: 10_000,
            max_ticks: 100_000,
        },
    )?;
    assert!(
        !listing.segments.is_empty(),
        "premise: segments were pinned"
    );
    let history = reader.read_market_history("tick-1201", &granted, 100_000)?;
    assert_eq!(history.unparsed, 0);
    assert_eq!(
        history.ticks.len(),
        captured,
        "the lake does not hold every tick the passes applied"
    );

    let mut messages: Vec<_> = history
        .ticks
        .into_iter()
        .filter(|tick| tick.message.object_id == object())
        .map(|tick| tick.message)
        .collect();
    messages.sort_by_key(|message| message.origin.sequence);
    let mut rebuilt = VenueState::aggregated(object(), VenueId::new(VENUE), VenueStatus::Open);
    let output = qip_orderbook::replay::replay(&mut rebuilt, &messages)?;
    assert!(
        output.is_clean(),
        "the lake's sequence has a hole: {:?}",
        output.periods
    );

    let venue = rig
        .decisions
        .gateway_mut()
        .simulated_mut()
        .expect("the rig's gateway is simulated")
        .quotes()
        .into_iter()
        .find(|depth| depth.object_id == object())
        .expect("the venue quotes the instrument");
    let book = rebuilt.snapshot().book;
    let ladder = |levels: &[qip_orderbook::view::Level]| -> Vec<(Decimal, Decimal)> {
        levels
            .iter()
            .map(|level| (level.price, level.size))
            .collect()
    };
    assert!(!venue.bids.is_empty(), "premise: the venue holds bids");
    assert_eq!(
        ladder(&book.bids),
        venue
            .bids
            .iter()
            .map(|level| (level.price, level.size))
            .collect::<Vec<_>>(),
        "bids rebuilt from the lake"
    );
    assert_eq!(
        ladder(&book.asks),
        venue
            .asks
            .iter()
            .map(|level| (level.price, level.size))
            .collect::<Vec<_>>(),
        "asks rebuilt from the lake"
    );
    Ok(())
}

/// R12-TICK-2044: a circuit breaker the simulated venue declares reaches the
/// cell as a typed status message on the feed's own wire, and the tick lake
/// seals it as that typed record. The feed refuses a pass whose frame the
/// cell decoded short (`decoded != tick.messages`), so the pass succeeding
/// is the cell having decoded the status line; the lake line is what the
/// capture path kept of it.
///
/// Mutation: in `SimulatedFeed::publish`, skip the status line
/// (`wire.status(..)`) — no `StatusChange` reaches the lake and the first
/// assertion fails.
#[test]
fn a_circuit_breaker_the_venue_declares_reaches_the_cell_as_a_typed_status_and_is_sealed_in_the_lake()
-> Result<()> {
    use qip_contracts::message::MessageBody;
    use qip_contracts::venue::VenueStatus;

    let mut rig = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 1_024)?;
    let lake: Arc<MemoryBlobStore> = Arc::new(MemoryBlobStore::new());
    let sinks: Vec<Box<dyn Sink>> = vec![Box::new(LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        CELL,
        qip_edge_node::event_fabric::drain::default_retention(),
        qip_storage::lake_codec::SegmentCodec::Identity,
    )?)];
    let mut drain = Drain::open(
        Arc::clone(&rig.spool),
        sinks,
        rig.drain_publisher
            .take()
            .expect("the publisher is handed once"),
        Arc::clone(&rig.telemetry),
        DrainConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
            64,
        )?,
    )?;

    let mut captured = 0usize;
    for second in 10..16 {
        let simulated = rig
            .decisions
            .gateway_mut()
            .simulated_mut()
            .expect("the rig's gateway is simulated");
        simulated.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(second))?;
        match second {
            12 => simulated.declare_status(&object(), VenueStatus::CircuitBreaker)?,
            14 => simulated.declare_status(&object(), VenueStatus::Open)?,
            _ => {}
        }
        captured += rig.decisions.turn(t(second))?.captured;
    }

    let statuses = |lake: &MemoryBlobStore| -> Result<Vec<VenueStatus>> {
        let mut found: Vec<(u64, VenueStatus)> = Vec::new();
        for key in segment_keys(lake)? {
            let body = lake.get(&key)?.expect("a listed segment is readable");
            for line in String::from_utf8(body).expect("canonical JSON").lines() {
                let value: serde_json::Value = serde_json::from_str(line)?;
                let event: qip_contracts::market_event::MarketEvent =
                    serde_json::from_value(value["event"].clone())?;
                if let MessageBody::StatusChange { status } = event.payload().body {
                    found.push((event.origin().sequence, status));
                }
            }
        }
        found.sort();
        Ok(found.into_iter().map(|(_, status)| status).collect())
    };
    let waited = std::time::Instant::now();
    let mut lines = 0usize;
    while lines < captured {
        drain.step()?;
        lines = segment_keys(lake.as_ref())?
            .iter()
            .filter_map(|key| lake.get(key).ok().flatten())
            .map(|body| {
                body.split(|byte| *byte == b'\n')
                    .filter(|l| !l.is_empty())
                    .count()
            })
            .sum();
        assert!(
            waited.elapsed() < PATIENCE,
            "only {lines} of {captured} ticks reached the lake"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        statuses(&lake)?,
        vec![VenueStatus::CircuitBreaker, VenueStatus::Open],
        "the venue's breaker and its reopening are not sealed as typed statuses"
    );
    Ok(())
}

/// The lake's sealed market segments, without the raw slots beside them
/// (`.raw`, ADR 0181): a segment holds canonical lines, a raw slot holds the
/// venue's bytes, and a reader of lines must not parse bytes as JSON.
fn segment_keys(lake: &MemoryBlobStore) -> Result<Vec<String>> {
    Ok(lake
        .list("lake/class=market/")?
        .into_iter()
        .filter(|key| !key.ends_with(".raw"))
        .collect())
}

/// R12-TICK-2042 on the node's own path: the passes' ticks go through the
/// spool and the drain thread's `LakeSink`, as on the default path, and
/// every sealed segment carries a raw slot whose lines are, one for one and
/// in order, the bytes the venue published for the segment's canonical
/// lines — each hashing to the native hash the feed recorded at publication.
///
/// Mutation: in `LakeSink::deliver`, stop collecting raw bytes for the
/// partition (`raw_by_partition` never filled) — no raw slot is sealed and
/// the first assertion fails.
#[test]
fn every_drained_lake_segment_carries_the_venues_published_bytes_line_for_line() -> Result<()> {
    let mut rig = rig(Arc::new(MemoryKeyValueStore::new()), 1 << 16, 1_024)?;
    let lake: Arc<MemoryBlobStore> = Arc::new(MemoryBlobStore::new());
    let sinks: Vec<Box<dyn Sink>> = vec![Box::new(LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        CELL,
        qip_edge_node::event_fabric::drain::default_retention(),
        qip_storage::lake_codec::SegmentCodec::Identity,
    )?)];
    let mut drain = Drain::open(
        Arc::clone(&rig.spool),
        sinks,
        rig.drain_publisher
            .take()
            .expect("the publisher is handed once"),
        Arc::clone(&rig.telemetry),
        DrainConfig::new(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
            64,
        )?,
    )?;
    let mut captured = 0usize;
    for second in 10..30 {
        let simulated = rig
            .decisions
            .gateway_mut()
            .simulated_mut()
            .expect("the rig's gateway is simulated");
        let bid = dec!("99") - Decimal::from(second % 3);
        simulated.seed_touch(&object(), Side::Buy, bid, dec!("500"), t(second))?;
        captured += rig.decisions.turn(t(second))?.captured;
    }
    assert!(
        captured > 10,
        "premise: the passes applied ticks ({captured})"
    );
    let waited = std::time::Instant::now();
    loop {
        drain.step()?;
        let lines: usize = segment_keys(lake.as_ref())?
            .iter()
            .filter_map(|key| lake.get(key).ok().flatten())
            .map(|body| {
                body.split(|byte| *byte == b'\n')
                    .filter(|l| !l.is_empty())
                    .count()
            })
            .sum();
        if lines >= captured {
            break;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "only {lines} of {captured} ticks reached the lake"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let mut checked = 0usize;
    for key in segment_keys(lake.as_ref())? {
        let raw_key = key.replace(".canonical", ".raw");
        let raw = lake
            .get(&raw_key)?
            .unwrap_or_else(|| panic!("segment {key} was sealed without the venue's bytes"));
        let raw = String::from_utf8(raw).expect("the simulated wire is text");
        let body = String::from_utf8(lake.get(&key)?.expect("listed")).expect("canonical JSON");
        let canonical: Vec<&str> = body.lines().collect();
        let raw_lines: Vec<&str> = raw.split_inclusive('\n').collect();
        assert_eq!(
            raw_lines.len(),
            canonical.len(),
            "{key}: the raw slot does not hold one published line per sealed tick"
        );
        for (line, published) in canonical.iter().zip(&raw_lines) {
            let value: serde_json::Value = serde_json::from_str(line)?;
            let event: qip_contracts::market_event::MarketEvent =
                serde_json::from_value(value["event"].clone())?;
            assert_eq!(
                Some(qip_core::hash::sha256_hex(published.as_bytes()).as_str()),
                event.native_hash(),
                "{key}: a raw line is not the bytes published for its tick"
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked, captured,
        "every applied tick's published bytes are in the lake"
    );
    Ok(())
}
