//! The deployable's mesh seam, against a real socket.
//!
//! `qip-edge` proves the cell's uplink and downlink in isolation. What this
//! file is for is the thing only the node can be wrong about: the *order* of a
//! tick, and what a tick does when the centre is not there. A cell that
//! published its state before installing the capital that arrived in the same
//! tick would leave the centre one full cycle behind on the one fact it needs
//! to decide whether to re-issue — and that mistake is invisible in a test of
//! either half alone.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{
    Clock, CorrelationId, Decimal, Duration, Id, Lineage, ManualClock, ObjectId, Timestamp, dec,
};
use qip_edge::cell::{Cell, CellConfig, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{CapitalGrantTopic, CellStateDelta};
use qip_edge_node::mesh::{MeshLink, MeshSettings, PEER_VARIABLE};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_transport::{
    ClientLimits, MemoryDeadLetters, MeshConfig, MeshEndpoint, MeshInbox, MeshPublisher, Method,
    RecordingSleeper, RetryPolicy, Sleeper,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const CELL: &str = "london-1";
const REGION: &str = "eu-west";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

// --- a loopback server ------------------------------------------------------

struct MeshServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MeshServer {
    fn spawn(endpoint: MeshEndpoint) -> Result<Self> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|error| Error::io(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| Error::io(error.to_string()))?
            .to_string();
        listener
            .set_nonblocking(true)
            .map_err(|error| Error::io(error.to_string()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicUsize::new(0));
        let thread_stop = Arc::clone(&stop);
        let thread_served = Arc::clone(&served);
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

    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

impl Drop for MeshServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_one(mut stream: TcpStream, endpoint: &MeshEndpoint) {
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

/// An address nothing is listening on: bound, then dropped.
fn dead_peer() -> Result<String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|error| Error::io(error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| Error::io(error.to_string()))?
        .to_string();
    drop(listener);
    Ok(format!("http://{address}"))
}

// --- fixtures ---------------------------------------------------------------

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(t(0)))
}

fn sleeper() -> Arc<dyn Sleeper> {
    Arc::new(RecordingSleeper::new())
}

fn settings(peer: &str) -> MeshSettings {
    MeshSettings {
        cell: CELL.to_string(),
        region: REGION.to_string(),
        peer: peer.to_string(),
        seed: 3,
    }
}

fn link(peer: &str) -> Result<MeshLink> {
    MeshLink::connect_with(&settings(peer), KEY, clock(), sleeper())
}

fn signed_envelope(strategy: &str, gross: &str, expires: i64) -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            Decimal::parse(gross).unwrap_or(Decimal::ZERO),
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(expires),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    build(&sign_payload(KEY, &unsigned.signing_payload()))
}

/// The centre's frame, written from the vocabulary alone.
///
/// `qip-mesh` is the crate that builds this in production; it cannot be named
/// from the cell side, and the fact that a payload built independently here is
/// understood by the node is the wire contract being tested.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct GrantFrame(CapitalEnvelope);

impl EventBody for GrantFrame {
    const TOPIC: Topic = CapitalGrantTopic::TOPIC;
    const SCHEMA_VERSION: u32 = CapitalGrantTopic::SCHEMA_VERSION;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}|{}|{}",
            self.0.cell(),
            self.0.strategy().as_str(),
            self.0.signature()
        ))
    }
}

fn grant_frame(envelope: &CapitalEnvelope, event_id: &str, at: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(event_id.to_string()),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "qip-edge-node-tests",
        ),
        GrantFrame(envelope.clone()),
    )
    .erase()
}

fn centre_publisher(peer: &str) -> Result<MeshPublisher> {
    MeshPublisher::new(
        MeshConfig::new("central-plane", peer)
            .with_retry(RetryPolicy {
                max_attempts: 2,
                initial_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(4),
                multiplier: 2,
                jitter_basis_points: 0,
            })
            .with_limits(ClientLimits {
                read_timeout: std::time::Duration::from_millis(500),
                connect_timeout: std::time::Duration::from_millis(500),
                ..ClientLimits::default()
            }),
        clock(),
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )
}

fn trivial_strategy() -> Result<(
    qip_strategy::compile::CompiledStrategy,
    qip_strategy::program::Program,
)> {
    use qip_contracts::signal::SignalKind;
    use qip_strategy::catalogue::FeatureCatalogue;
    use qip_strategy::compile::StrategyCompiler;
    use qip_strategy::ir::{Expr, Rule, StrategySpec};

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(
        StrategyId::new("mean-reversion-1"),
        object("ACME"),
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
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn deployed_cell(expires: i64) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new("XLON"));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    let (strategy, program) = trivial_strategy()?;
    let grant = VerifiedEnvelope::verify(
        signed_envelope("mean-reversion-1", "1000", expires)?,
        KEY,
        CELL,
        t(10),
    )?;
    cell.deploy(strategy, program, grant)?;
    Ok(cell)
}

// --- the round trip ---------------------------------------------------------

#[test]
fn one_tick_installs_the_capital_that_arrived_and_reports_it_in_the_same_delta() -> Result<()> {
    // The order is the property. The delta the centre reads back carries the
    // expiry of the grant it just issued, which is how it knows the envelope
    // landed. Publishing first would make that confirmation a cycle late.
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    let mut centre = centre_publisher(&server.url())?;
    let renewal = signed_envelope("mean-reversion-1", "2000", 7_200)?;
    centre.publish_frame(grant_frame(&renewal, "EVT-RENEWAL", t(20))?, t(20))?;

    let mut cell = deployed_cell(3_600)?;
    let mut node_link = link(&server.url())?;
    let tick = node_link.exchange(&mut cell, &WorkReport::default(), t(30));

    assert_eq!(tick.renewed, vec!["mean-reversion-1".to_string()]);
    assert_eq!(tick.delta.as_deref(), Some("delivered"), "{tick:?}");
    assert!(
        tick.is_quiet(),
        "a clean tick reported something to look at: {tick:?}"
    );

    // The grant frame the centre published is still in the inbox alongside the
    // delta, so the delta is found by topic rather than by position.
    let response = inbox.read(0, Timestamp::MAX, 16);
    let delta = response
        .frames
        .iter()
        .find_map(|entry| entry.frame.decode::<CellStateDelta>().ok())
        .expect("the node published no state delta")
        .body;
    assert_eq!(delta.cell, CELL);
    assert_eq!(
        delta.utilisation[0].envelope_expires_at,
        t(7_200),
        "the delta reported the envelope the cell had before the tick, so the centre cannot tell \
         whether the grant it issued was installed"
    );
    assert_eq!(
        cell.journal().tally().get("capital_renewed"),
        Some(&1),
        "the change in the cell's authority was not journalled"
    );
    assert!(server.served() >= 3, "the tick did not use the socket");
    Ok(())
}

#[test]
fn a_tick_against_an_unreachable_centre_reports_it_and_leaves_the_cell_running() -> Result<()> {
    // ADR 0008's whole point, at the deployable: losing the central plane is
    // not an outage for the cell. It keeps its authority until the envelope it
    // already holds expires, and the tick says plainly that the centre is gone
    // rather than presenting as a quiet cell.
    let mut cell = deployed_cell(3_600)?;
    let mut node_link = link(&dead_peer()?)?;
    let tick = node_link.exchange(&mut cell, &WorkReport::default(), t(30));

    assert!(
        tick.poll_error.is_some(),
        "an unreachable centre polled fine"
    );
    assert_eq!(tick.delta.as_deref(), Some("dead_lettered"), "{tick:?}");
    assert!(!tick.is_quiet());
    assert!(!cell.is_halted(), "losing the centre stopped the cell");
    assert_eq!(cell.deployed_strategies(), vec!["mean-reversion-1"]);
    assert_eq!(node_link.health().uplink.dead_lettered, 1);
    Ok(())
}

#[test]
fn a_grant_for_a_strategy_the_node_does_not_run_is_reported_rather_than_deployed() -> Result<()> {
    // A cell receives authority; it never promotes itself into using it. The
    // node's answer to a grant it cannot place is to say so, not to start
    // running something the approval ladder never sent it.
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox))?;
    let mut centre = centre_publisher(&server.url())?;
    let stranger = signed_envelope("momentum-9", "1000", 7_200)?;
    centre.publish_frame(grant_frame(&stranger, "EVT-STRANGER", t(20))?, t(20))?;

    let mut cell = deployed_cell(3_600)?;
    let mut node_link = link(&server.url())?;
    let tick = node_link.exchange(&mut cell, &WorkReport::default(), t(30));

    assert!(tick.renewed.is_empty(), "{tick:?}");
    assert_eq!(tick.refused.len(), 1, "{tick:?}");
    assert!(
        tick.refused[0].contains("momentum-9"),
        "the refusal did not name the strategy: {}",
        tick.refused[0]
    );
    assert_eq!(
        cell.deployed_strategies(),
        vec!["mean-reversion-1"],
        "capital arriving for a strategy deployed it"
    );
    Ok(())
}

#[test]
fn a_grant_for_the_desks_strategy_is_held_by_the_installer_rather_than_refused() -> Result<()> {
    // The same frame as the stranger's, addressed to the strategy the node
    // was told funds its desk. `renew_capital` would refuse it — no desk is
    // deployed, and a cell does not deploy one because capital arrived —
    // so the exchange hands it to the installer, which holds exactly one
    // and waits for a whitelist. Nothing is deployed by the grant alone.
    use qip_edge_node::arbitrage::ArbitrageInstaller;
    let inbox = MeshInbox::new("central", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox))?;
    let mut centre = centre_publisher(&server.url())?;
    let grant = signed_envelope("arbitrage-desk", "1000", 7_200)?;
    centre.publish_frame(grant_frame(&grant, "EVT-DESK", t(20))?, t(20))?;

    let mut cell = deployed_cell(3_600)?;
    let mut installer = ArbitrageInstaller::new(
        StrategyId::new("arbitrage-desk"),
        vec![VenueId::new("XLON")],
    );
    let mut node_link = link(&server.url())?;
    let tick = node_link.exchange_with(
        &mut cell,
        &WorkReport::default(),
        t(30),
        Some(&mut installer),
    );

    assert!(
        tick.refused.is_empty(),
        "the desk's grant was refused: {tick:?}"
    );
    assert!(
        tick.renewed
            .iter()
            .any(|entry| entry.starts_with("arbitrage-desk")),
        "the tick does not report the grant as held: {tick:?}"
    );
    assert!(
        installer.holds_envelope(),
        "the installer did not hold the desk's grant"
    );
    assert_eq!(
        tick.desk.as_deref(),
        Some("no fresh cycle whitelist applied"),
        "the installer's outcome is not reported: {tick:?}"
    );
    assert!(cell.arbitrage().is_none(), "a grant alone installed a desk");
    assert_eq!(
        cell.deployed_strategies(),
        vec!["mean-reversion-1"],
        "capital arriving for the desk deployed something"
    );
    Ok(())
}

#[test]
fn a_node_without_a_configured_peer_runs_detached_rather_than_refusing_to_start() -> Result<()> {
    // Asserting its own premise first: this is a test about the *absent*
    // variable, and a suite that happened to run with it set would pass while
    // proving nothing. The workspace forbids unsafe code, so the environment is
    // read rather than staged.
    assert!(
        std::env::var(PEER_VARIABLE).is_err(),
        "{PEER_VARIABLE} is set in this environment, so this test cannot check the absent case"
    );
    assert_eq!(
        MeshSettings::from_env(CELL, REGION)?,
        None,
        "a cell with no central plane was treated as a misconfiguration rather than as detached"
    );
    Ok(())
}
