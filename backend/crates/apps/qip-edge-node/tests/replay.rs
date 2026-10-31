//! ADR 0100 §8, SLICE-39: a recorded node session is re-driven from its
//! recorded exogenous inputs alone — the pass markers and applied tape
//! events the spool holds, the control frames at the positions those markers
//! name, the clock instants they carry — against a rebuilt cell and a
//! re-seeded simulated venue, and the replayed journal equals the recorded
//! one entry for entry (R12-TICK-10184).
//!
//! The recorded run is the node as `main.rs` composes it: `DecisionLoop::turn`
//! over a real spool with the writer and relay threads, control arriving
//! through the pass-boundary handoff with fabric positions, the simulated
//! feed attached to the cell. Nothing in the replay reads the recorded cell;
//! it reads the spool.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, HaltCommand, PolicyPayload, Slot};
use qip_contracts::reflex::Decision;
use qip_contracts::replay::ControlPosition;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, CorrelationId, Decimal, EventId, Id, Lineage, ManualClock, dec};
use qip_edge::cell::{Cell, CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{HaltTopic, PolicyPayloadTopic};
use qip_edge::trust::TrustRoot;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::control::{Delivery, Handoff as ControlHandoff};
use qip_edge_node::event_fabric::control::CONTROL_STREAM;
use qip_edge_node::event_fabric::inputs::RecordedInputs;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::relay::{JournalRelay, RelayConfig};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{self, SpoolWriter, WriterConfig};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::{NodeGateway, SimulatedGateway, widen_config_digest};
use qip_edge_node::mirror::StoreMirror;
use qip_edge_node::replay::{
    ArchivedControl, Divergence, FixtureControl, Rebuilt, RecordedSession, ReplayTrust, Verdict,
    replay,
};
use qip_edge_node::scheduler::{DecisionLoop, InstantClock, JournalBound, MarkerFacts, Outbox};
use qip_edge_node::strategies::StrategyInstaller;
use qip_events::event_fabric::bindings::{REFLEX_JOURNAL_RECORDED, REFLEX_PASS_MARKED};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_storage::blob::MemoryBlobStore;
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_storage::segment::archive::{ArchiveReader, Archiver};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const KEY: &[u8] = b"replay-test-envelope-key";
const GATEWAY_SEED: u64 = 7;
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
        "qip-replay-test-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory can be created");
    dir
}

// --- the frames the centre sends -----------------------------------------

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

fn position(offset: u64, event_id: &str) -> ControlPosition {
    ControlPosition::new(CONTROL_STREAM, 0, offset, EventId::from_string(event_id))
}

// --- the cell, as main.rs assembles it ------------------------------------

/// A strategy whose one rule always holds, so every pass decides something.
fn firing_strategy() -> Result<(CompiledStrategy, Program)> {
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
    Ok((compiled, compiler.into_program()))
}

fn unsigned_grant() -> Result<CapitalEnvelope> {
    let unsigned = CapitalEnvelope::new(
        StrategyId::new(STRATEGY),
        CELL,
        dec!("1000000"),
        dec!("100000"),
        dec!("50000"),
        vec![venue()],
        t(0),
        t(36_000),
        "alice@example.com",
        "",
    )?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        Decimal::ONE,
        capital,
    )?)?;
    let loss_limit = unsigned.loss_limit();
    unsigned.with_drawdown(qip_contracts::DrawdownBounds::bounded(
        loss_limit, None, None,
    )?)
}

fn grant() -> Result<VerifiedEnvelope> {
    let unsigned = unsigned_grant()?;
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), KEY, CELL, t(1))
}

/// The payload the centre ships once it has partitioned the region's grant:
/// the one grant the cell holds, named in the live slot.
fn share_payload(sequence: u64, issued_at: Timestamp) -> Result<PolicyPayload> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![grant()?.signature().to_string()],
        },
        issued_at,
    );
    payload.signed(KEY)
}

fn permitting_to_seed(gateway: SimulatedGateway) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for action in [
        CreationAction::Create,
        CreationAction::List,
        CreationAction::Seed,
    ] {
        let request = CreationRequest::new(action, venue.as_str(), object().as_str(), "paper");
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request, "test fixture")?;
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}

struct Node {
    cell: Cell,
    gateway: SimulatedGateway,
    feed: SimulatedFeed,
    config_digest: String,
}

/// The node's pieces, built the same way twice: once to record, once to
/// replay. Every step here is what `main.rs` does with the same inputs, so
/// the recorded `config_digest` is the rebuilt one.
fn node(clock: Arc<dyn Clock>) -> Result<Node> {
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_journal_trimmed_on_ship()
        .with_journal_wire();
    let config_digest = widen_config_digest(
        &qip_core::sha256_hex(format!("{config:?}").as_bytes()),
        &SimulatedGateway::default_settings(),
    );
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut assembly = assemble(config, features, clock, allocation, None)?;
    let mut gateway = permitting_to_seed(SimulatedGateway::new(venue(), GATEWAY_SEED, t(0))?)?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut assembly.cell)?;
    let (compiled, program) = firing_strategy()?;
    assembly
        .cell
        .deploy_with_pricing(compiled, program, grant()?, PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    Ok(Node {
        cell: assembly.cell,
        gateway,
        feed,
        config_digest,
    })
}

// --- the recorded run -----------------------------------------------------

struct Recorded {
    spool: Arc<SegmentLog>,
    control: FixtureControl,
    /// The frames again, as the fabric would hold them on its control
    /// stream: one record per batch, at the offset the position names.
    control_frames: Vec<(ControlPosition, AnyEvent)>,
    /// Kept alive so the writer and relay threads keep running while the
    /// spool is read back.
    _decisions: DecisionLoop,
}

/// The share is applied at pass 2's boundary and a halt at pass 5's, both
/// through the fabric-position handoff, so the markers of those passes name
/// a control position each.
const SHARE_POSITION: u64 = 0;
const HALT_POSITION: u64 = 1;
const PASSES: u64 = 6;

/// Run the node for six passes over a real spool, with the control frames
/// arriving at fabric positions, and hand back the spool and the frames.
fn record() -> Result<Recorded> {
    let mut node = node(Arc::new(ManualClock::new(t(0))))?;
    let metrics = Arc::new(qip_observability::metrics::Metrics::new("qip-edge-node"));
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
        Arc::clone(&spool),
        writer_inbox,
        spool_publisher,
        Arc::clone(&telemetry),
        WriterConfig::new(CELL, t(0), std::time::Duration::from_millis(5), 64)?,
    )?;
    let session = spool_writer.session();
    // As `main.rs` seats it: the chain epoch is the writer's session.
    node.cell.set_chain_epoch(session)?;
    std::thread::spawn(move || {
        let _ = spool_writer.run();
    });
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());
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
    let (control_sender, control_handoff) = ControlHandoff::bounded(16)?;
    let mut decisions = DecisionLoop::new(
        node.cell,
        NodeGateway::Simulated(node.gateway),
        outbox,
        JournalBound::new(1_000)?,
        Arc::clone(&telemetry),
        MarkerFacts {
            cell: CELL.to_string(),
            config_digest: node.config_digest,
            gateway_seed: GATEWAY_SEED,
            binary_version: "test".to_string(),
        },
    )?
    .with_feed(node.feed, None)
    .with_strategies(
        StrategyInstaller::new(None, None),
        Some((None, control_handoff)),
    )
    .with_threads(Some(relay), None);

    // The frames, as the fabric control consumer would deliver them: verified
    // once here for the recorded run, and held as the frames the replay must
    // verify for itself.
    let mut control = FixtureControl::new();
    let share = share_payload(1, t(15))?;
    let share_frame = frame(PolicyBody(share.clone()), "ctl-share", t(15))?;
    control.insert(&position(SHARE_POSITION, "ctl-share"), share_frame.clone())?;
    let halt = HaltCommand::new(CELL, t(45), "operator drill").signed(KEY)?;
    let halt_frame = frame(HaltBody(halt.clone()), "ctl-halt", t(45))?;
    control.insert(&position(HALT_POSITION, "ctl-halt"), halt_frame.clone())?;
    let control_frames = vec![
        (position(SHARE_POSITION, "ctl-share"), share_frame),
        (position(HALT_POSITION, "ctl-halt"), halt_frame),
    ];

    for pass in 1..=PASSES {
        let now = t(10 * pass as i64);
        if pass == 2 {
            let verified = qip_edge::policy::VerifiedPolicy::verify(share.clone(), KEY, CELL, now)?;
            control_sender.send(Delivery::policy(
                position(SHARE_POSITION, "ctl-share"),
                verified,
            ))?;
        }
        if pass == 5 {
            let verified = qip_edge::policy::VerifiedHalt::verify(halt.clone(), KEY, CELL, now)?;
            control_sender.send(Delivery::halt(
                position(HALT_POSITION, "ctl-halt"),
                verified,
            ))?;
        }
        // A pass that errs is tolerated, as the node tolerates it: the journal
        // holds the refusal and the loop keeps turning. What the recording
        // must show is that the inputs reached the spool on every turn.
        let turn = decisions.turn(now)?;
        assert!(
            turn.recorded.is_some(),
            "pass {pass}'s inputs were not recorded"
        );
    }

    // Wait for every marker to reach the spool: the writer is a thread.
    let waited = std::time::Instant::now();
    loop {
        let markers = writer::records(&spool)?
            .into_iter()
            .filter(|record| record.event.topic == REFLEX_PASS_MARKED.topic)
            .count() as u64;
        if markers >= PASSES {
            break;
        }
        assert!(
            waited.elapsed() < PATIENCE,
            "only {markers} of {PASSES} pass markers reached the spool"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // And the journal entries the last pass wrote, which trail the marker
    // through the relay: wait until the spool's journal stops growing.
    let waited = std::time::Instant::now();
    let mut seen = usize::MAX;
    loop {
        let entries = writer::records(&spool)?
            .iter()
            .filter(|record| writer::journal_entry(&record.event).is_ok())
            .count();
        if entries == seen {
            break;
        }
        seen = entries;
        assert!(
            waited.elapsed() < PATIENCE,
            "the spool's journal never settled"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(Recorded {
        spool,
        control,
        control_frames,
        _decisions: decisions,
    })
}

fn rebuilt(clock: Arc<ManualClock>, session: &RecordedSession) -> Result<Rebuilt> {
    let mut node = node(clock)?;
    node.cell.set_chain_epoch(
        session
            .session()
            .parse()
            .expect("the writer's session counter"),
    )?;
    Ok(Rebuilt {
        cell: node.cell,
        gateway: node.gateway,
        feed: node.feed,
        requoter: None,
        strategies: Some(StrategyInstaller::new(None, None)),
        config_digest: node.config_digest,
        gateway_seed: GATEWAY_SEED,
    })
}

fn trust() -> Result<ReplayTrust> {
    Ok(ReplayTrust {
        trust: TrustRoot::shared_key(KEY)?,
        grant_memory: 64,
    })
}

#[test]
fn a_recorded_node_session_replayed_reproduces_every_journal_entry_fill_and_flag() -> Result<()> {
    let recorded = record()?;
    let session = RecordedSession::from_spool(&recorded.spool)?;

    // The premise: six recorded passes, the share and the halt recorded as
    // control positions on the passes that consumed them, and a recorded
    // journal holding at least one fill and one halt flag after the first
    // pass instant — so that what the replay reproduces includes both.
    assert_eq!(session.passes().len() as u64, PASSES);
    let positions: Vec<(u64, usize)> = session
        .passes()
        .iter()
        .map(|pass| (pass.marker.pass, pass.marker.control.len()))
        .collect();
    assert_eq!(
        positions,
        vec![(1, 0), (2, 1), (3, 0), (4, 0), (5, 1), (6, 0)],
        "the markers do not name the control positions their boundaries consumed"
    );
    let from = session.first_instant().expect("a recorded pass");
    let recorded_journal: Vec<_> = session
        .journal()
        .iter()
        .filter(|entry| entry.at >= from)
        .collect();
    let fills = recorded_journal
        .iter()
        .filter(|entry| matches!(entry.decision, Decision::Filled { .. }))
        .count();
    let halts = recorded_journal
        .iter()
        .filter(|entry| matches!(entry.decision, Decision::HaltChanged { halted: true, .. }))
        .count();
    assert!(
        fills >= 1,
        "the recorded run filled nothing: {recorded_journal:?}"
    );
    assert!(
        halts >= 1,
        "the recorded run was never halted: {recorded_journal:?}"
    );
    // The feed publishes differences, so a quiet pass applies no tape and
    // its marker hashes an empty one; what the premise needs is that some
    // pass applied tape, so the digest comparison has a non-trivial case.
    assert!(
        session.passes().iter().any(|pass| !pass.applied.is_empty()),
        "no pass applied any tape; the replay would compare only empty tapes"
    );

    let clock = Arc::new(ManualClock::new(t(0)));
    let report = replay(
        &session,
        rebuilt(Arc::clone(&clock), &session)?,
        &recorded.control,
        &trust()?,
        &clock,
    )?;
    assert_eq!(report.verdict, Verdict::Reproduced, "{}", report.describe());
    assert_eq!(report.passes, PASSES);
    assert_eq!(report.control_frames, 2);
    assert_eq!(report.control_refused, 0);
    assert_eq!(report.journal_entries as usize, recorded_journal.len());
    assert_eq!(report.fills as usize, fills);
    assert!(report.halt_flags >= 1);
    assert!(report.halted_passes >= 1, "pass 6 ran under the halt");
    Ok(())
}

#[test]
fn a_replay_missing_one_recorded_control_frame_names_the_pass_and_position_rather_than_reproducing()
-> Result<()> {
    let mut recorded = record()?;
    let session = RecordedSession::from_spool(&recorded.spool)?;
    assert_eq!(
        recorded.control.len(),
        2,
        "the premise is two recorded frames"
    );
    assert!(
        recorded
            .control
            .remove(&position(HALT_POSITION, "ctl-halt"))
    );

    let clock = Arc::new(ManualClock::new(t(0)));
    let report = replay(
        &session,
        rebuilt(Arc::clone(&clock), &session)?,
        &recorded.control,
        &trust()?,
        &clock,
    )?;
    assert_eq!(
        report.verdict,
        Verdict::Diverged(Divergence::ControlFrame {
            pass: 5,
            position: position(HALT_POSITION, "ctl-halt"),
        }),
        "{}",
        report.describe()
    );
    // Four passes re-drove and agreed before the missing frame stopped it.
    assert_eq!(report.passes, 4);
    Ok(())
}

#[test]
fn a_rebuild_under_another_configuration_is_refused_before_any_pass_is_re_driven() -> Result<()> {
    let recorded = record()?;
    let session = RecordedSession::from_spool(&recorded.spool)?;
    let clock = Arc::new(ManualClock::new(t(0)));
    let mut other = rebuilt(Arc::clone(&clock), &session)?;
    other.config_digest = format!("{}-other", other.config_digest);
    let report = replay(&session, other, &recorded.control, &trust()?, &clock)?;
    assert!(
        matches!(
            report.verdict,
            Verdict::Diverged(Divergence::Configuration { .. })
        ),
        "{}",
        report.describe()
    );
    assert_eq!(report.passes, 0);
    Ok(())
}

/// The archive key the node's spool is archived under in this test; the
/// fabric names the stream it relayed to, and the reader only needs the two
/// to agree.
const SPOOL_STREAM: &str = "reflex.london-1.spool";

/// Seal and archive every segment of `log` under `stream` into `store`.
fn archive_all(log: &SegmentLog, stream: &str, store: Arc<MemoryBlobStore>) -> Result<usize> {
    log.seal_active()?;
    let archiver = Archiver::new(store).with_manifest_index();
    let mut archived = 0;
    for segment in log.segments() {
        if !segment.sealed {
            continue;
        }
        archiver.archive(log, segment.start_offset, stream, 0, &BTreeSet::new())?;
        archived += 1;
    }
    Ok(archived)
}

/// ADR 0100 §8: the replay reads "the spool or archived segments". Once the
/// spool is retained away, the session comes back from the fabric's indexed
/// archive — the same records, so the same verdict — and the control frames
/// come from the archived control stream at the positions the markers name,
/// through `ArchiveReader` exactly as the fabric wrote them.
#[test]
fn a_session_read_from_the_archive_with_control_from_the_archived_control_stream_reproduces()
-> Result<()> {
    let recorded = record()?;
    let from_spool = RecordedSession::from_spool(&recorded.spool)?;

    // The spool, archived: every sealed segment under one stream key.
    let store = Arc::new(MemoryBlobStore::new());
    let segments = archive_all(&recorded.spool, SPOOL_STREAM, Arc::clone(&store))?;
    assert!(
        segments >= 1,
        "the premise is at least one archived segment"
    );

    // The control stream, archived: the fabric holds one record per offset
    // at the position each marker names, so the batch's base offset is the
    // position's offset.
    let control_log = SegmentLog::open(
        temp_dir("control"),
        SegmentLogConfig::new(Arc::new(ManualClock::new(t(0)))),
    )?;
    for (position, event) in &recorded.control_frames {
        let record = Record::from_any_event(event, PayloadCodec::CanonicalJson)?;
        let mut batch = Batch::new(
            MessageType::Data,
            1,
            1,
            PayloadCodec::CanonicalJson,
            vec![record],
        )?;
        batch.base_offset = position.offset;
        let offset = control_log.append(&batch)?;
        assert_eq!(
            offset, position.offset,
            "the premise is a record per position"
        );
    }
    let control_store = Arc::new(MemoryBlobStore::new());
    archive_all(&control_log, CONTROL_STREAM, Arc::clone(&control_store))?;
    let control = ArchivedControl::new(ArchiveReader::new(control_store));

    let from_archive = RecordedSession::from_archive(&ArchiveReader::new(store), SPOOL_STREAM, 0)?;
    assert_eq!(
        from_archive, from_spool,
        "the archive did not give back the session the spool holds"
    );

    let clock = Arc::new(ManualClock::new(t(0)));
    let report = replay(
        &from_archive,
        rebuilt(Arc::clone(&clock), &from_archive)?,
        &control,
        &trust()?,
        &clock,
    )?;
    assert_eq!(report.verdict, Verdict::Reproduced, "{}", report.describe());
    assert_eq!(report.control_frames, 2);
    assert!(report.fills >= 1 && report.halt_flags >= 1);
    Ok(())
}

/// The journal comparison is what makes the verdict about decisions and not
/// only about inputs: a spool whose recorded journal disagrees with what its
/// own inputs reproduce is reported on the journal, at the first entry that
/// differs, after every pass has applied its recorded tape. Here one
/// recorded `SignalRaised` entry's conviction is altered in the records the
/// session is read from; the inputs are untouched, so every tape digest
/// agrees and only the journal can catch it.
///
/// Mutation: make the entry comparison in `replay` accept any pair, and the
/// verdict reads reproduced over a journal that was edited.
#[test]
fn a_recorded_journal_that_disagrees_with_its_own_inputs_is_caught_at_the_first_differing_entry()
-> Result<()> {
    let recorded = record()?;
    let mut records = writer::records(&recorded.spool)?;
    let altered = records
        .iter_mut()
        .filter(|record| record.event.topic == REFLEX_JOURNAL_RECORDED.topic)
        .find(|record| {
            record.event.payload["decision"]["SignalRaised"]["conviction_shrunk_f64"].is_number()
        })
        .expect("the premise is a recorded SignalRaised entry");
    altered.event.payload["decision"]["SignalRaised"]["conviction_shrunk_f64"] =
        serde_json::json!(0.25);
    let altered_sequence = altered.event.payload["sequence"]
        .as_u64()
        .expect("a journal entry carries its sequence");
    let session = RecordedSession::from_records(&records)?;
    let from = session.first_instant().expect("a recorded pass");
    let index = session
        .journal()
        .iter()
        .filter(|entry| entry.at >= from)
        .position(|entry| entry.sequence == altered_sequence)
        .expect("the altered entry is after the first pass");

    let clock = Arc::new(ManualClock::new(t(0)));
    let report = replay(
        &session,
        rebuilt(Arc::clone(&clock), &session)?,
        &recorded.control,
        &trust()?,
        &clock,
    )?;
    assert_eq!(
        report.passes, PASSES,
        "every pass applied its recorded tape first"
    );
    match &report.verdict {
        Verdict::Diverged(Divergence::Journal {
            index: at,
            recorded: Some(recorded),
            replayed: Some(replayed),
        }) => {
            assert_eq!(*at, index, "{}", report.describe());
            assert!(recorded.contains("0.25"), "{recorded}");
            assert!(replayed.contains("0.5"), "{replayed}");
        }
        other => panic!("expected a journal divergence, got {other:?}"),
    }
    assert_eq!(report.journal_entries as usize, index);
    Ok(())
}

/// Every byte of every file under `directory`, hashed in path order: what
/// "left as it was found" means for a spool.
fn directory_digest(directory: &std::path::Path) -> String {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("the directory lists") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut all = Vec::new();
    for path in paths {
        all.extend_from_slice(path.to_string_lossy().as_bytes());
        all.extend_from_slice(&std::fs::read(&path).expect("the file reads"));
    }
    qip_core::sha256_hex(&all)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("the copy's directory");
    for entry in std::fs::read_dir(from).expect("the directory lists") {
        let entry = entry.expect("an entry");
        let kind = entry.file_type().expect("a file type");
        if kind.is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()));
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).expect("the file copies");
        }
    }
}

/// The re-drive reads the spool directory through a copy, as the manifests
/// do: `SegmentLog::open` is the writer's open and truncates a torn tail,
/// and on a running node's spool that tail is the batch its writer is
/// appending. Here the recorded spool is copied, its active segment is given
/// a torn tail, and the session read from that directory is the recorded one
/// while every byte of the directory is as it was before the read.
///
/// Mutation: make `RecordedSession::from_spool_directory` open the directory
/// in place; recovery truncates the torn tail and the digest moves.
#[test]
fn a_replay_reads_a_copy_of_the_spool_and_leaves_a_torn_tail_where_it_found_it() -> Result<()> {
    let recorded = record()?;
    let expected = RecordedSession::from_spool(&recorded.spool)?;
    let torn = temp_dir("torn-spool");
    copy_dir(recorded.spool.directory(), &torn);
    let mut segments: Vec<PathBuf> = std::fs::read_dir(&torn)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("segment."))
        })
        .collect();
    segments.sort();
    let active = segments
        .last()
        .expect("the premise is a spool with a segment")
        .clone();
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&active)?;
        file.write_all(&[0xFF; 7])?;
    }
    let before = directory_digest(&torn);

    let read = RecordedSession::from_spool_directory(&torn)?;
    assert_eq!(
        read, expected,
        "the copy did not read back the recorded session"
    );
    assert_eq!(
        directory_digest(&torn),
        before,
        "reading the spool changed the directory: the torn tail was recovered in place"
    );
    Ok(())
}

/// R12-REFLEX-8066: the timing mode's two halves over the fixture spool.
/// Every re-driven pass is timed by the monotonic clock outside the cell,
/// the summary's percentiles are latencies a pass actually took, and the
/// timing changes nothing the replay reproduces. Mutation: drop the
/// `pass_latencies` push in `replay`, or summarise an empty set as zeros.
#[test]
fn a_timed_replay_times_every_pass_it_re_drives_and_still_reproduces() -> Result<()> {
    use qip_edge_node::replay::LatencySummary;
    let recorded = record()?;
    let session = RecordedSession::from_spool(&recorded.spool)?;
    let clock = Arc::new(ManualClock::new(t(0)));
    let report = replay(
        &session,
        rebuilt(Arc::clone(&clock), &session)?,
        &recorded.control,
        &trust()?,
        &clock,
    )?;
    assert_eq!(report.verdict, Verdict::Reproduced, "{}", report.describe());
    assert_eq!(
        report.pass_latencies.len() as u64,
        report.passes,
        "every re-driven pass is timed, once"
    );
    let summary = LatencySummary::of(&report.pass_latencies).expect("passes were timed");
    assert_eq!(summary.samples as u64, PASSES);
    assert!(summary.p50 <= summary.p90 && summary.p90 <= summary.p99 && summary.p99 <= summary.max);
    assert!(
        report.pass_latencies.contains(&summary.p50)
            && report.pass_latencies.contains(&summary.max),
        "a percentile is not a latency any pass took"
    );
    assert_eq!(
        LatencySummary::of(&[]),
        None,
        "no pass timed is no distribution"
    );
    Ok(())
}

/// The host tuning a timing run is labelled with is read from the kernel
/// command line, word by word, and an absent one says so. Mutation: match
/// by substring and `nohz_full=3` is read for an `isolcpus` lookup's
/// neighbour, or `hugepagesz=2M` for `hugepages`.
#[test]
fn the_host_tuning_is_read_whole_word_from_the_kernel_command_line() {
    use qip_edge_node::replay::host_tuning;
    let tuned = host_tuning(
        "BOOT_IMAGE=/vmlinuz ro hugepagesz=2M hugepages=512 isolcpus=2-3 nohz_full=2-3 quiet",
    );
    assert_eq!(
        tuned,
        vec![
            ("isolcpus", "2-3".to_string()),
            ("nohz_full", "2-3".to_string()),
            ("hugepages", "512".to_string()),
        ]
    );
    let untuned = host_tuning("BOOT_IMAGE=/vmlinuz ro hugepagesz=2M quiet");
    assert!(
        untuned.iter().all(|(_, value)| value == "unset"),
        "{untuned:?}"
    );
}
