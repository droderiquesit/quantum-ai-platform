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
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
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
use qip_edge_node::mirror::{StoreMirror, batches};
use qip_edge_node::scheduler::{DecisionLoop, InstantClock, JournalBound, MarkerFacts, Outbox};
use qip_events::event_fabric::bindings::REFLEX_PASS_MARKED;
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_storage::BlobStore;
use qip_storage::blob::MemoryBlobStore;
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
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
    let unsigned = build("unsigned")?;
    let signed = build(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, ENVELOPE_KEY, CELL, t(1))
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
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
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
        let keys = lake.list("lake/class=market/")?;
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
