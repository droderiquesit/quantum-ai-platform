//! The edge cell node.
//!
//! One region's hot execution path, running next to the venues it trades. It
//! decides without asking the central plane — that is what a cell is for — so
//! everything that bounds it has to be present locally before it starts:
//!
//! * A **capital envelope key**. Without it the cell cannot verify a grant,
//!   and a cell that cannot verify capital must not trade. Refusing to start
//!   is the correct behaviour; starting and trading on unverified grants is
//!   not.
//! * A **cell identity**. An envelope is scoped to one cell, and a node that
//!   does not know which cell it is cannot check that scope.
//! * A **venue set**. What this cell may reach, independent of what any
//!   envelope permits; an order clears both or neither.
//!
//! What it keeps, and what it deliberately does not, is settled here too. The
//! journal — every decision and every refusal — is shipped to the configured
//! store, because a cell that dies is a cell whose record is the only account
//! of what it did. Its books, features and stream watermarks are *not*, and
//! that is not an omission: a cell rebuilds its books from the feed, and a
//! book restored from disk is a position nobody has reconciled against the
//! venue. Restoring it would make the cell trade against a picture of the
//! market that stopped being true when the process died.
//!
//! The node cannot reach a language model, and the guarantee is structural
//! rather than checked here: `qip-edge` and this binary do not depend on
//! `qip-ai` directly or transitively, and a workspace architecture test keeps
//! it that way. There is no runtime check because there is nothing to check —
//! the call does not exist to be made.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, SystemClock};
use qip_edge::cell::{Cell, CellConfig, WorkReport};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::mesh::{MeshLink, MeshSettings, PEER_VARIABLE};
use qip_edge_node::mirror::StoreMirror;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_storage::settings::{ROOT_VARIABLE, StorageSettings, TARGET_VARIABLE};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

/// Exit code for a configuration problem, matching `sysexits.h`.
///
/// Distinct from a general failure so an orchestrator can tell "this node was
/// deployed wrong" from "this node broke", and stop restarting the first.
const EX_CONFIG: i32 = 78;

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) if error.message().starts_with("configuration:") => {
            eprintln!("qip-edge-node: {}", error.message());
            std::process::exit(EX_CONFIG);
        }
        Err(error) => {
            eprintln!("qip-edge-node: {}", error.message());
            std::process::exit(1);
        }
    }
}

/// Everything the node needs before it may start.
#[derive(Debug)]
struct NodeConfig {
    cell_id: String,
    region: String,
    venues: Vec<VenueId>,
    envelope_key: Vec<u8>,
    health_port: u16,
    storage: StorageSettings,
    /// Where the central plane is, when there is one. `None` is a cell running
    /// detached, which ADR 0008 makes a legitimate state rather than a
    /// misconfiguration — see `qip_edge_node::mesh`.
    mesh: Option<MeshSettings>,
}

impl NodeConfig {
    /// Read the environment, naming every missing value at once.
    ///
    /// Reporting the first missing variable and exiting makes deploying a new
    /// cell a sequence of restarts. Reporting all of them makes it one.
    fn from_env() -> Result<Self> {
        let mut missing = Vec::new();
        let cell_id = required("QIP_CELL_ID", &mut missing);
        let region = required("QIP_CELL_REGION", &mut missing);
        let key = required("QIP_CAPITAL_ENVELOPE_KEY", &mut missing);
        let venues = required("QIP_VENUES", &mut missing);

        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "configuration: {} must be set",
                missing.join(", ")
            )));
        }

        let venues: Vec<VenueId> = venues
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(VenueId::new)
            .collect();
        if venues.is_empty() {
            return Err(Error::invalid(
                "configuration: QIP_VENUES names no venue; a cell with no venue has nothing to do",
            ));
        }

        let health_port = match std::env::var("QIP_HEALTH_PORT") {
            Ok(value) => value.parse::<u16>().map_err(|_| {
                Error::invalid(format!(
                    "configuration: QIP_HEALTH_PORT is not a port: {value}"
                ))
            })?,
            Err(_) => 8080,
        };

        // `QIP_MIRROR_PATH` used to select the journal's destination on its
        // own, beside the variables every other binary reads. Ignoring it now
        // would leave a cell that was deployed with it writing its journal
        // nowhere while its configuration still claimed a path, so it is
        // refused with the replacement named rather than quietly dropped.
        if std::env::var("QIP_MIRROR_PATH").is_ok_and(|value| !value.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "configuration: QIP_MIRROR_PATH is no longer read. The journal goes to the \
                 store named by {TARGET_VARIABLE} and {ROOT_VARIABLE}, the same two variables \
                 every other binary reads; set {TARGET_VARIABLE}=engine and {ROOT_VARIABLE} to \
                 the path you had here"
            )));
        }

        let storage = StorageSettings::from_env()
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
        let mesh = MeshSettings::from_env(&cell_id, &region)?;

        Ok(Self {
            cell_id,
            region,
            venues,
            envelope_key: key.into_bytes(),
            health_port,
            storage,
            mesh,
        })
    }
}

fn required(name: &str, missing: &mut Vec<String>) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            missing.push(name.to_string());
            String::new()
        }
    }
}

fn run() -> Result<()> {
    let config = NodeConfig::from_env()?;

    // The clock is read once, here, at the boundary. Everything inside the
    // cell takes a timestamp as a parameter, which is what makes a session
    // replayable from its journal.
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let started = clock.now();

    if config.envelope_key.is_empty() {
        return Err(Error::denied(
            "configuration: the capital envelope key is empty; a cell that cannot verify a grant must not trade",
        ));
    }

    let mut cell_config = CellConfig::new(&config.cell_id, &config.region);
    for venue in &config.venues {
        cell_config = cell_config.with_venue(venue.clone());
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(cell_config, features)?;

    // The venue seam. Simulated is the only class this binary can construct —
    // `AdapterClass` has no live variant — and the seed is configuration so a
    // session is replayable: the same seed against the same orders produces
    // the same fills and the same rejection draws.
    let gateway_seed = std::env::var("QIP_GATEWAY_SEED")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1);
    let gateway_venue = config
        .venues
        .first()
        .expect("from_env refuses an empty venue list")
        .clone();
    let gateway = SimulatedGateway::new(gateway_venue, gateway_seed, started)?;

    // The store is opened and proven writable before the health surface binds.
    // A node that started, reported healthy, and only discovered at the first
    // flush that its journal had nowhere to go would have been trading for
    // however long that took with no record anybody could read afterwards.
    config
        .storage
        .preflight()
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let mut mirror = StoreMirror::open(
        config.storage.key_value("cell-journal")?,
        &config.cell_id,
        started,
    )?;
    let retained_sessions = mirror.retained_sessions()?;

    println!(
        "qip-edge-node cell={} region={} venues={} live_capable={} gateway={}({})",
        config.cell_id,
        config.region,
        config.venues.len(),
        cell.autonomy().ceiling().is_live(),
        gateway.class(),
        gateway.venue()
    );

    for line in config.storage.banner_lines(
        &["the cell's decision journal, chained within each session"],
        &[
            "the order books and every feature derived from them",
            "the feed watermarks",
            "the halt state",
        ],
    ) {
        println!("{line}");
    }
    println!("  prior sessions:   {retained_sessions} retained in this store");
    if !config.storage.is_durable() {
        // Not fatal: the cell still records locally and still refuses what it
        // should. Saying so is what keeps it from looking healthy — an
        // operator reading this line knows the record dies with the process.
        eprintln!(
            "qip-edge-node: the journal is held in memory only and will not survive this \
             process; set {TARGET_VARIABLE}=engine and {ROOT_VARIABLE} to keep it"
        );
    }

    // The mesh link, when a peer is configured. Built after the store is proven
    // writable and before the health surface binds, so a node that cannot even
    // parse its peer address fails as a configuration error rather than as a
    // process that reported healthy and then never spoke to the centre.
    let mut link = match &config.mesh {
        Some(settings) => {
            let link = MeshLink::connect(settings, &config.envelope_key, Arc::clone(&clock))?;
            println!("qip-edge-node: mesh peer {}", link.peer());
            Some(link)
        }
        None => None,
    };

    // No venue connectivity is configured: this build has no credential for
    // one, and inventing a feed would be worse than serving without it. The
    // node serves its health surface so an orchestrator can see it, and
    // reports what production would still have to supply.
    for requirement in missing_production_requirements(config.mesh.is_some()) {
        println!("qip-edge-node: awaiting {requirement}");
    }

    serve(
        &config,
        &mut cell,
        &gateway,
        &mut mirror,
        link.as_mut(),
        &clock,
        started,
    )
}

/// What a production cell would need that this build cannot supply.
///
/// Named rather than faked. A synthetic feed that looked like a venue would
/// make the node appear to work and make the gap invisible.
fn missing_production_requirements(has_mesh_peer: bool) -> Vec<String> {
    let mut missing = vec![
        "QIP_VENUE_FEED_ENDPOINT and its multicast group or session credential".to_string(),
        "QIP_VENUE_GATEWAY_ENDPOINT and an order-entry session credential".to_string(),
        "QIP_DROP_COPY_ENDPOINT for the independent fill channel".to_string(),
    ];
    if !has_mesh_peer {
        // Named rather than silently tolerated. A detached cell is a valid
        // state and an invisible one is not: this node will keep trading inside
        // the envelope it holds and will never be granted another.
        missing.push(format!(
            "{PEER_VARIABLE} for the central plane: without it this cell publishes no state and \
             receives no capital, and stops when its current envelope expires"
        ));
    }
    missing
}

/// Serve the health surface until the listener fails, draining the journal as
/// it goes.
///
/// Deliberately tiny and deliberately not the platform's HTTP server: a node
/// whose liveness probe depends on the API crate has coupled the two, and the
/// probe is what tells an orchestrator whether the cell is alive at all.
///
/// The journal is flushed on each accepted connection because this node has no
/// scheduler and the liveness probe is the only periodic event it has. That is
/// a compromise and worth naming: it ties the record's durability to how often
/// something asks whether the cell is alive, so a node nobody probes keeps its
/// decisions in memory. A production cell drains on a timer instead. The flush
/// happens before the answer is written so the numbers reported are the ones
/// already shipped, rather than a count the next crash would take back.
fn serve(
    config: &NodeConfig,
    cell: &mut Cell,
    gateway: &SimulatedGateway,
    mirror: &mut StoreMirror,
    mut link: Option<&mut MeshLink>,
    clock: &Arc<dyn Clock>,
    started: qip_core::Timestamp,
) -> Result<()> {
    let address = format!("0.0.0.0:{}", config.health_port);
    let listener = TcpListener::bind(&address)
        .map_err(|error| Error::io(format!("cannot bind {address}: {error}")))?;
    println!("qip-edge-node: health on {address}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                // A journal that cannot be shipped is reported and the node
                // keeps serving: the entries are still held locally and still
                // chained, so the next flush ships them. Exiting here would
                // turn a storage outage into a trading outage.
                let now = clock.now();
                if let Err(error) = cell.flush(mirror, now) {
                    eprintln!(
                        "qip-edge-node: the journal could not be shipped: {}",
                        error.message()
                    );
                }
                // One exchange with the central plane per probe, for the same
                // reason and with the same caveat as the flush above: this node
                // has no scheduler, so the liveness probe is the only periodic
                // event it has. A production cell runs this on a timer, because
                // tying capital renewal to how often something asks whether the
                // cell is alive is a compromise, not a design.
                //
                // The work report is empty because no venue feed is configured
                // in this build, so the delta reports the cell's authority and
                // halt state rather than its trading.
                if let Some(link) = link.as_deref_mut() {
                    let tick = link.exchange(cell, &WorkReport::default(), now);
                    if !tick.is_quiet() {
                        eprintln!("qip-edge-node: mesh exchange: {tick:?}");
                    }
                }
                let health = link.as_deref().map(MeshLink::health);
                if let Err(error) = answer(stream, config, cell, gateway, mirror, health, started) {
                    eprintln!("qip-edge-node: health request failed: {}", error.message());
                }
            }
            // One bad connection is not a reason to stop trading.
            Err(error) => eprintln!("qip-edge-node: accept failed: {error}"),
        }
    }
    Ok(())
}

fn answer(
    mut stream: TcpStream,
    config: &NodeConfig,
    cell: &Cell,
    gateway: &SimulatedGateway,
    mirror: &StoreMirror,
    mesh: Option<qip_edge_node::mesh::MeshHealth>,
    started: qip_core::Timestamp,
) -> Result<()> {
    let mut buffer = [0u8; 1024];
    // The content is irrelevant — any request gets the same answer — but the
    // read has to happen or the client sees a reset instead of a response.
    let _ = stream.read(&mut buffer);

    // The mesh block is `null` when no peer is configured, rather than absent
    // or zeroed. A probe that could not tell "detached" from "connected and
    // quiet" would report the two most different states identically.
    let mesh = match mesh {
        Some(health) => format!(
            r#"{{"peer_circuit":"{}","deltas_delivered":{},"deltas_dead_lettered":{},"circuit_refusals":{},"grants_verified":{},"grants_refused":{},"grants_duplicate":{}}}"#,
            health.circuit.as_str(),
            health.uplink.delivered,
            health.uplink.dead_lettered,
            health.uplink.circuit_refusals,
            health.downlink.verified,
            health.downlink.refused,
            health.downlink.duplicates,
        ),
        None => "null".to_string(),
    };
    let body = format!(
        r#"{{"cell":"{}","region":"{}","halted":{},"live_capable":{},"venues":{},"strategies":{},"journal_entries":{},"journal_shipped":{},"storage":"{}","durable":{},"gateway":{{"class":"{}","venue":"{}","submitted":{},"rejected":{}}},"mesh":{mesh},"started_at":{}}}"#,
        config.cell_id,
        config.region,
        cell.is_halted(),
        cell.autonomy().ceiling().is_live(),
        config.venues.len(),
        cell.deployed_strategies().len(),
        cell.journal().len(),
        // Reported beside the journal's own length so a probe can see the two
        // diverge. A cell whose entries climb while nothing ships is a cell
        // whose record is not leaving the process, and that is invisible if
        // only one of the two numbers is published.
        mirror.shipped_entries(),
        config.storage.target().as_str(),
        config.storage.is_durable(),
        gateway.class(),
        gateway.venue(),
        gateway.submitted_count(),
        gateway.rejected_count(),
        started.as_secs()
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| Error::io(format!("cannot write the health response: {error}")))?;
    stream
        .flush()
        .map_err(|error| Error::io(format!("cannot flush the health response: {error}")))
}
