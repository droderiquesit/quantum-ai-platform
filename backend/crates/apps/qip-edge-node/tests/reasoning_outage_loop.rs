//! A node keeps deciding and executing when the centre stops answering —
//! driven through `DecisionLoop::turn`, the call the binary's timer makes
//! (R12-RES-10130: "Node trades identically with the centre silent").
//!
//! `reasoning_outage.rs` holds the same property one layer down: it calls
//! `run_pass` and `MeshLink::exchange` in turn on the test's own thread. The
//! shipped node calls neither that way. It reaches the centre through a
//! `MeshWorker` on a thread of its own, and the thread that runs `Cell::work`
//! only takes what the worker polled (`DecisionLoop::apply_mesh`) and leaves a
//! delta behind. A loop that gated its pass on what the worker brought back —
//! skipped a turn whose poll had failed, or waited for a poll at all — would
//! pass that test and halt the node in an outage of the slow lane. An auditor
//! refuted the row on exactly that gap, so this file closes it.
//!
//! Two nodes are composed as `main.rs` composes them — an assembled cell with
//! the journal wire, the outbox's spool writer and relay, the simulated
//! venue, the feed, and a `MeshWorker` to a centre on a real loopback socket —
//! and each works the same turns at the same instants. In one the centre
//! answers throughout. In the other the centre's listener is closed part-way
//! and never returns. The pass count, the orders the venue took and the
//! refusals the cell journaled must be the same, pass for pass.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::reflex::Decision;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Decimal, ManualClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::JournalEntry;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::pressure::JournalPressure;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::event_fabric::inputs::RecordedInputs;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::relay::{JournalRelay, RelayConfig};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{self, SpoolWriter, WriterConfig};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::{NodeGateway, SimulatedGateway};
use qip_edge_node::health::HealthSnapshot;
use qip_edge_node::mesh::{MeshLink, MeshSettings, MeshWorker};
use qip_edge_node::mirror::{StoreMirror, batches};
use qip_edge_node::pass::PassStats;
use qip_edge_node::scheduler::{
    DecisionLoop, HealthFacts, InstantClock, JournalBound, MarkerFacts, Outbox,
};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_transport::mesh::{POLL_PATH, PUBLISH_PATH};
use qip_transport::{MeshEndpoint, MeshInbox, Method, RecordingSleeper};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const STRATEGY: &str = "always-enter";
const KEY: &[u8] = b"outage-loop-envelope-key";
/// Turns each run works, and the turn before which the centre is stopped.
const TURNS: usize = 7;
const CENTRE_STOPS_BEFORE: usize = 3;
/// How often the mesh worker ticks. Far shorter than [`PACE`], so the worker
/// has polled — and, after the stop, failed — several times between turns.
const MESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);
/// Wall time left between turns, for the worker's thread and nothing else:
/// every turn is handed its own instant, so this decides how much the worker
/// has done by the next turn and nothing the pass sees.
const PACE: std::time::Duration = std::time::Duration::from_millis(25);
/// How long anything another thread does may take before it is a failure.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-outage-loop-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory can be created");
    dir
}

// --- the centre, on a real socket, counting what it answered ------------------

struct Centre {
    address: String,
    stop: Arc<AtomicBool>,
    /// Set, the centre still accepts every connection and answers none of
    /// them: the hanging centre, as against the closed one.
    silent: Arc<AtomicBool>,
    /// Connections accepted while silent, each held open and never answered.
    held: Arc<AtomicU64>,
    /// Polls and deltas the centre answered with a 2xx, told apart by path.
    polls: Arc<AtomicU64>,
    deltas: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Centre {
    fn start() -> Result<Self> {
        let io = |error: std::io::Error| Error::io(error.to_string());
        let endpoint = MeshEndpoint::new(MeshInbox::new("central", 64, 256)?);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(io)?;
        let address = listener.local_addr().map_err(io)?.to_string();
        listener.set_nonblocking(true).map_err(io)?;
        let stop = Arc::new(AtomicBool::new(false));
        let silent = Arc::new(AtomicBool::new(false));
        let held = Arc::new(AtomicU64::new(0));
        let polls = Arc::new(AtomicU64::new(0));
        let deltas = Arc::new(AtomicU64::new(0));
        let (thread_stop, thread_polls, thread_deltas) =
            (Arc::clone(&stop), Arc::clone(&polls), Arc::clone(&deltas));
        let (thread_silent, thread_held) = (Arc::clone(&silent), Arc::clone(&held));
        let handle = std::thread::spawn(move || {
            let mut unanswered = Vec::new();
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) if thread_silent.load(Ordering::SeqCst) => {
                        thread_held.fetch_add(1, Ordering::SeqCst);
                        unanswered.push(stream);
                    }
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        match serve_one(stream, &endpoint).as_deref() {
                            Some(POLL_PATH) => thread_polls.fetch_add(1, Ordering::SeqCst),
                            Some(PUBLISH_PATH) => thread_deltas.fetch_add(1, Ordering::SeqCst),
                            _ => 0,
                        };
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
            silent,
            held,
            polls,
            deltas,
            handle: Some(handle),
        })
    }

    /// From now on, accept and never answer.
    fn silence(&self) {
        self.silent.store(true, Ordering::SeqCst);
    }

    fn held(&self) -> u64 {
        self.held.load(Ordering::SeqCst)
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn answered(&self) -> (u64, u64) {
        (
            self.polls.load(Ordering::SeqCst),
            self.deltas.load(Ordering::SeqCst),
        )
    }
}

/// Dropping the centre stops it: the thread ends and the listener closes, so
/// every connection made to the address afterwards is refused.
impl Drop for Centre {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Answer one request, returning its path when the answer was a 2xx.
fn serve_one(mut stream: std::net::TcpStream, endpoint: &MeshEndpoint) -> Option<String> {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next().and_then(Method::parse)?;
    let target = parts.next()?.to_string();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
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
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
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
    stream.write_all(&out).ok()?;
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let path = target
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_string();
    (200..300).contains(&response.status).then_some(path)
}

// --- the node, composed as main.rs composes it --------------------------------

/// A grant small enough that the strategy runs out of it inside the run, so
/// the turns after the stop include refusals as well as orders.
fn grant() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1300"),
            dec!("1300"),
            dec!("1300"),
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    // Bounded as the centre's issuer bounds every grant (R12-CAPITAL-4088).
    let unsigned = build("")?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        Decimal::ONE,
        capital,
    )?)?;
    let loss_limit = unsigned.loss_limit();
    let unsigned = unsigned.with_drawdown(qip_contracts::DrawdownBounds::bounded(
        loss_limit, None, None,
    )?)?;
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), KEY, CELL, t(1))
}

fn share_policy(grants: Vec<String>) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, t(5))
}

/// `gateway`, permitted to list each of `objects` on demand
/// (R12-COVERAGE-5107). A fixture; the refusal without one is proven in
/// tests/gateway.rs.
fn permitting(gateway: SimulatedGateway, objects: &[&ObjectId]) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    // List and seed: the run seeds the touch it trades against, and
    // permission to list is not permission to seed (R12-COVERAGE-5107).
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

/// The node: a decision loop over an assembled cell with the journal wire and
/// trimming armed, a firing strategy under a funded share, the simulated
/// venue and its feed, the outbox's writer and relay into `store`, a health
/// snapshot, and a `MeshWorker` to `centre`.
fn node(
    centre: &str,
    store: Arc<dyn KeyValueStore>,
    health: HealthSnapshot,
) -> Result<DecisionLoop> {
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_journal_trimmed_on_ship()
        .with_journal_wire();
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    // A frozen clock, so that nothing the cell stamps for itself can differ
    // between the two runs: every instant a pass sees is the turn's own.
    let cell_clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let mut node = assemble(config, features, cell_clock, allocation, None)?;
    let gateway = permitting(SimulatedGateway::new(venue(), 7, t(0))?, &[&object()])?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    node.cell.deploy_with_pricing(
        compiled,
        compiler.into_program(),
        grant()?,
        PricingPolicy::Marketable,
    )?;
    let named = grant()?.signature().to_string();
    node.cell.apply_policy(share_policy(vec![named])?, t(5))?;

    let metrics = Arc::clone(node.scrape_registry());
    let telemetry = Arc::new(OutboxTelemetry::new(metrics));
    let gauge_clock: Arc<dyn Clock> = Arc::new(InstantClock::anchored_at(t(0)));
    let thresholds = Thresholds::new(
        1 << 30,
        Decimal::parse("0.5").expect("a literal fraction parses"),
        Decimal::parse("0.9").expect("a literal fraction parses"),
    )?;
    let (gauge, spool_publisher, _drain_publisher) =
        PressureGauge::new(thresholds, Arc::clone(&gauge_clock), Duration::from_secs(5))?;
    let spool = Arc::new(SegmentLog::open(
        temp_dir("spool"),
        SegmentLogConfig::new(Arc::clone(&gauge_clock)),
    )?);
    let (to_relay, from_mirror) = writer::channel(64)?;
    let (to_writer, writer_inbox) = writer::channel(64)?;
    let spool_writer = SpoolWriter::open(
        spool,
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

    // The centre link as the binary builds it, on its own thread. The link's
    // clock is frozen too: a breaker that opens on the outage stays open, so
    // the stopped run never half-recovers part-way through.
    let link_clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let link = MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: centre.to_string(),
            seed: 3,
        },
        KEY,
        Arc::clone(&link_clock),
        Arc::new(RecordingSleeper::new()),
    )?;
    let worker = MeshWorker::spawn(link, None, link_clock, MESH_INTERVAL, 16)?;

    Ok(DecisionLoop::new(
        node.cell,
        NodeGateway::Simulated(gateway),
        outbox,
        JournalBound::new(1 << 16)?,
        telemetry,
        MarkerFacts {
            cell: CELL.to_string(),
            config_digest: "test".to_string(),
            gateway_seed: 7,
            binary_version: "test".to_string(),
        },
    )?
    .with_feed(feed, None)
    .with_threads(Some(relay), None)
    .with_health(
        health,
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
    .with_mesh(worker))
}

/// A deadline beside the turns. The cell is not `Send`, so turns run on the
/// test's thread; a turn that blocks never returns, and past the deadline
/// this says why and aborts rather than letting the suite hang.
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

// --- one run -------------------------------------------------------------------

/// What one turn did, in the terms the requirement names.
#[derive(Debug, PartialEq)]
struct TurnRecord {
    ran_pass: bool,
    /// Cumulative pass statistics after the turn: passes, refusals, signals,
    /// orders, fills.
    stats: PassStats,
    /// Orders the venue itself counted, independently of the cell.
    submitted: u64,
    halted: bool,
}

/// What one pass journaled: `(order id, venue, quantity)` for every order
/// sent and `(gate, reason)` for every refusal, keyed by the pass's instant.
#[derive(Debug, Default, PartialEq)]
struct PassJournal {
    orders: Vec<(String, String, String)>,
    refusals: Vec<(String, String)>,
}

#[derive(Debug)]
struct Run {
    turns: Vec<TurnRecord>,
    journal: Vec<JournalEntry>,
    /// What the centre had answered — polls, deltas — when the turn before
    /// the stop completed, and at the end of the run (`None` once stopped).
    answered_before_stop: (u64, u64),
    answered_at_end: Option<(u64, u64)>,
    /// The health body's mesh object after the last turn.
    mesh: serde_json::Value,
    pressures: Vec<JournalPressure>,
    /// The longest any turn from the stop on took, by the wall clock.
    slowest_after_stop: std::time::Duration,
}

/// What happens to the centre before turn [`CENTRE_STOPS_BEFORE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CentreFate {
    /// It answers throughout.
    Answers,
    /// Its listener closes and never returns.
    Stops,
    /// It goes on accepting and answers nothing again.
    FallsSilent,
}

impl Run {
    fn passes(&self) -> BTreeMap<Timestamp, PassJournal> {
        let mut out: BTreeMap<Timestamp, PassJournal> = BTreeMap::new();
        for entry in &self.journal {
            match &entry.decision {
                Decision::OrderSent {
                    order_id,
                    venue,
                    quantity,
                    ..
                } => out.entry(entry.at).or_default().orders.push((
                    order_id.clone(),
                    venue.clone(),
                    quantity.clone(),
                )),
                Decision::Refused { gate, reason } => out
                    .entry(entry.at)
                    .or_default()
                    .refusals
                    .push((gate.clone(), reason.clone())),
                _ => {}
            }
        }
        out
    }
}

/// Instant of turn `turn`. Two seconds apart, so the whole run sits inside
/// the thirty seconds the simulated venue's session lasts with nothing
/// answering its heartbeat.
fn instant(turn: usize) -> Timestamp {
    t(10 + 2 * turn as i64)
}

/// Work [`TURNS`] turns. With `centre_stops` the centre's listener is closed
/// before turn [`CENTRE_STOPS_BEFORE`] and never returns.
fn run(centre_stops: bool) -> Result<Run> {
    run_as(if centre_stops {
        CentreFate::Stops
    } else {
        CentreFate::Answers
    })
}

fn run_as(fate: CentreFate) -> Result<Run> {
    let centre_stops = fate == CentreFate::Stops;
    let store = Arc::new(MemoryKeyValueStore::new());
    let health = HealthSnapshot::new("{}");
    let mut centre = Some(Centre::start()?);
    let url = centre.as_ref().map(Centre::url).unwrap_or_default();
    let address = centre
        .as_ref()
        .map(|c| c.address.clone())
        .unwrap_or_default();
    let mut decisions = node(
        &url,
        Arc::clone(&store) as Arc<dyn KeyValueStore>,
        health.clone(),
    )?;

    let mut turns = Vec::new();
    let mut pressures = Vec::new();
    let mut answered_before_stop = (0, 0);
    let mut slowest_after_stop = std::time::Duration::ZERO;
    for turn in 0..TURNS {
        let now = instant(turn);
        if turn == 1 {
            // The venue lists the instrument from the second turn, so the
            // first refuses for want of a book and the rest have one.
            let venue = decisions
                .gateway_mut()
                .simulated_mut()
                .expect("the node runs the simulated venue");
            venue.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), now)?;
            venue.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), now)?;
        }
        if turn == CENTRE_STOPS_BEFORE {
            // Both runs wait here, the same way, until the centre has
            // answered a poll and taken a delta from this node's worker: the
            // link was live, not merely configured, before one run loses it.
            let waited = std::time::Instant::now();
            let live = centre.as_ref().expect("the centre runs until here");
            while live.answered().0 == 0 || live.answered().1 == 0 {
                assert!(
                    waited.elapsed() < PATIENCE,
                    "premise: the worker never reached the centre: {:?}",
                    live.answered()
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            answered_before_stop = live.answered();
            if centre_stops {
                centre = None;
                let refused = std::net::TcpStream::connect_timeout(
                    &address.parse().expect("a loopback address"),
                    std::time::Duration::from_secs(1),
                );
                assert!(
                    refused.is_err(),
                    "premise: the stopped centre still accepts connections"
                );
                // Give the worker time to fail against the closed port before
                // the first turn without a centre.
                std::thread::sleep(PACE);
            }
            if fate == CentreFate::FallsSilent {
                let live = centre.as_ref().expect("a silent centre still runs");
                live.silence();
                // Premise: the worker is inside a request the centre will
                // never answer before the first silent turn, or the turns
                // below prove nothing about a hang.
                let waited = std::time::Instant::now();
                while live.held() == 0 {
                    assert!(
                        waited.elapsed() < PATIENCE,
                        "premise: the worker never reached the silent centre"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
        let started = std::time::Instant::now();
        let report = {
            let _watchdog = Watchdog::arm(
                "a decision-loop turn — the turn blocked, which is the centre holding the cell",
            );
            decisions.turn(now)?
        };
        if turn >= CENTRE_STOPS_BEFORE {
            slowest_after_stop = slowest_after_stop.max(started.elapsed());
        }
        pressures.push(report.pressure);
        turns.push(TurnRecord {
            ran_pass: report.ran_pass,
            stats: *decisions.stats(),
            submitted: decisions.gateway_mut().submitted_count(),
            halted: decisions.cell().is_halted(),
        });
        std::thread::sleep(PACE);
    }

    // Everything the cell journaled reaches the store; wait for the relay.
    let recorded = decisions.cell().journal().len();
    let waited = std::time::Instant::now();
    let journal = loop {
        let mut entries: Vec<JournalEntry> = batches(&*store)?
            .into_iter()
            .flat_map(|batch| batch.entries)
            .collect();
        entries.sort_by_key(|entry| entry.sequence);
        if entries.len() >= recorded {
            break entries;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "the journal never reached its store: {} of {recorded}",
            entries.len()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let body: serde_json::Value = serde_json::from_str(&health.read())?;
    Ok(Run {
        turns,
        journal,
        answered_before_stop,
        answered_at_end: centre.as_ref().map(Centre::answered),
        mesh: body["mesh"].clone(),
        pressures,
        slowest_after_stop,
    })
}

fn failed_deltas(mesh: &serde_json::Value) -> u64 {
    mesh["deltas_dead_lettered"].as_u64().unwrap_or(0)
        + mesh["circuit_refusals"].as_u64().unwrap_or(0)
}

/// R12-RES-10130's edge half, through the loop the binary runs.
///
/// The failure this prevents: a decision thread that waits on, or is gated
/// by, the mesh worker — a pass skipped because the worker's poll failed, a
/// turn that blocks until the worker hands something across, a halt on an
/// open circuit. Each turns an outage of the services that reason into an
/// outage of the node that trades. `reasoning_outage.rs` cannot see any of
/// them: it never builds a `DecisionLoop`, and an auditor refuted the row
/// for that reason.
///
/// Mutation (run 2026-10-24): make `DecisionLoop::apply_mesh` report whether
/// any inbound it took carried a poll error, and `turn` skip the pass when it
/// did — the stopped run's turn 3 runs no pass (`ran_pass: false`, `passes:
/// 3` against 4) and the turn-for-turn comparison fails at "turn 3 differs
/// with the centre stopped before turn 3".
///
/// What it does not catch, said plainly: a turn that *blocks* until the
/// worker hands a poll across. With the listener gone the worker still
/// hands across a failed poll every interval, so such a loop would slow down
/// and not stop; the watchdog bounds it only once a turn passes `PATIENCE`.
/// `decision_loop.rs`'s silent-centre test is the one aimed at that.
#[test]
fn a_node_driven_by_its_decision_loop_trades_the_same_passes_orders_and_refusals_when_the_centre_stops_part_way()
-> Result<()> {
    let answered = run(false)?;
    let stopped = run(true)?;

    // The premise, both runs: the worker reached the centre — it answered a
    // poll and took a delta — before the stop, so the link was live.
    for (name, run) in [("answered", &answered), ("stopped", &stopped)] {
        let (polls, deltas) = run.answered_before_stop;
        assert!(
            polls > 0 && deltas > 0,
            "premise: in the {name} run the centre answered {polls} polls and {deltas} deltas \
             before the stop"
        );
    }
    // The healthy run kept its centre and kept talking to it after that
    // point, and its worker never failed a delta; the stopped run's worker
    // did fail deltas once the listener went — it really lost the centre.
    let (end_polls, end_deltas) = answered
        .answered_at_end
        .expect("the healthy run's centre runs to the end");
    assert!(
        end_polls > answered.answered_before_stop.0 && end_deltas > answered.answered_before_stop.1,
        "premise: the healthy centre was still answering after the stop point: \
         {:?} then {:?}",
        answered.answered_before_stop,
        answered.answered_at_end
    );
    assert_eq!(failed_deltas(&answered.mesh), 0, "{}", answered.mesh);
    assert!(stopped.answered_at_end.is_none());
    assert!(
        failed_deltas(&stopped.mesh) > 0,
        "premise: the stopped run's worker never failed a delta: {}",
        stopped.mesh
    );
    // Neither run was under journal pressure, so nothing but the centre
    // differs between them.
    for pressures in [&answered.pressures, &stopped.pressures] {
        assert!(
            pressures.iter().all(|p| *p == JournalPressure::Normal),
            "{pressures:?}"
        );
    }

    // And the turns on both sides of the stop are not idle ones: in the
    // healthy run orders were sent and refusals made before and after it,
    // so "identical" below is said of a node deciding and executing.
    assert_eq!(answered.turns.len(), TURNS);
    let passes = answered.passes();
    let stop_at = instant(CENTRE_STOPS_BEFORE);
    for (side, range) in [
        ("before", passes.range(..stop_at)),
        ("after", passes.range(stop_at..)),
    ] {
        let range: Vec<_> = range.collect();
        assert!(
            range.iter().any(|(_, p)| !p.orders.is_empty()),
            "premise: no order was sent {side} the stop: {range:?}"
        );
        assert!(
            range.iter().any(|(_, p)| !p.refusals.is_empty()),
            "premise: nothing was refused {side} the stop: {range:?}"
        );
    }
    assert!(
        answered
            .turns
            .iter()
            .all(|turn| turn.ran_pass && !turn.halted),
        "premise: every healthy turn ran a pass: {:?}",
        answered.turns
    );
    assert_eq!(answered.turns[TURNS - 1].stats.passes, TURNS as u64);

    // Pass count, orders sent and refusals: identical, turn for turn and
    // pass for pass, and the journal the two cells shipped is the same chain.
    for (turn, (a, s)) in answered.turns.iter().zip(&stopped.turns).enumerate() {
        assert_eq!(
            s, a,
            "turn {turn} differs with the centre stopped before turn {CENTRE_STOPS_BEFORE}"
        );
    }
    assert_eq!(stopped.turns.len(), answered.turns.len());
    assert_eq!(stopped.passes(), passes);
    assert_eq!(stopped.journal, answered.journal);
    Ok(())
}

/// R12-REFLEX-5029: the node takes every hot-path decision without waiting
/// on the centre, against the centre that is hardest to survive — one that
/// accepts the connection and never answers.
///
/// The test beside this one closes the centre's listener, so every request
/// the worker makes after the stop is refused at once and a failed poll is
/// handed across every interval; a turn that *waited* for the worker would
/// slow down there and not stop, and that test says so. Here the worker is
/// held inside a request the centre will never answer, so nothing comes
/// across at all, and a turn that waited on the worker would wait for the
/// transport's whole timeout or for ever.
///
/// Mutation: `MeshWorker::take_inbound_at_most`'s `try_recv` made a `recv`,
/// so the turn waits on what the worker brings back.
#[test]
fn a_node_driven_by_its_decision_loop_trades_the_same_when_the_centre_accepts_and_never_answers()
-> Result<()> {
    let answered = run_as(CentreFate::Answers)?;
    let silent = run_as(CentreFate::FallsSilent)?;

    // The premise: the link was live before the silence, and the turns on
    // both sides of it decided something.
    let (polls, deltas) = silent.answered_before_stop;
    assert!(
        polls > 0 && deltas > 0,
        "premise: the centre answered {polls} polls and {deltas} deltas before falling silent"
    );
    let passes = answered.passes();
    let stop_at = instant(CENTRE_STOPS_BEFORE);
    assert!(
        passes.range(stop_at..).any(|(_, p)| !p.orders.is_empty()),
        "premise: no order was sent after the silence: {passes:?}"
    );

    // No turn waited on the silent centre: each took the wall time of a
    // pass, not of a request nobody will answer.
    assert!(
        silent.slowest_after_stop < std::time::Duration::from_secs(1),
        "a turn took {:?} beside a centre that accepts and never answers",
        silent.slowest_after_stop
    );

    // And the node decided and executed the same, turn for turn, pass for
    // pass, down to the chain it shipped.
    assert_eq!(silent.turns.len(), answered.turns.len());
    for (turn, (a, s)) in answered.turns.iter().zip(&silent.turns).enumerate() {
        assert_eq!(
            s, a,
            "turn {turn} differs with the centre silent from turn {CENTRE_STOPS_BEFORE}"
        );
    }
    assert_eq!(silent.passes(), passes);
    assert_eq!(silent.journal, answered.journal);
    Ok(())
}
