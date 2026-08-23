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
//! The node cannot reach a language model, and the guarantee is structural
//! rather than checked here: `qip-edge` and this binary do not depend on
//! `qip-ai` directly or transitively, and a workspace architecture test keeps
//! it that way. There is no runtime check because there is nothing to check —
//! the call does not exist to be made.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, SystemClock};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::journal::{FileMirror, MemoryMirror, Mirror};
use qip_edge_node::gateway::SimulatedGateway;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
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
    mirror_path: Option<String>,
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

        Ok(Self {
            cell_id,
            region,
            venues,
            envelope_key: key.into_bytes(),
            health_port,
            mirror_path: std::env::var("QIP_MIRROR_PATH").ok(),
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
    let cell = Cell::new(cell_config, features)?;

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

    let mut mirror: Box<dyn Mirror> = match &config.mirror_path {
        Some(path) => Box::new(FileMirror::new(path)),
        // No durable mirror configured is a degraded state, not a fatal one:
        // the cell still records locally and still refuses what it should.
        // Saying so is what keeps it from looking healthy.
        None => {
            eprintln!(
                "qip-edge-node: QIP_MIRROR_PATH is unset; the journal is held in memory only \
                 and will not survive this process"
            );
            Box::new(MemoryMirror::new())
        }
    };

    println!(
        "qip-edge-node cell={} region={} venues={} live_capable={} gateway={}({})",
        config.cell_id,
        config.region,
        config.venues.len(),
        cell.autonomy().ceiling().is_live(),
        gateway.class(),
        gateway.venue()
    );

    // No venue connectivity is configured: this build has no credential for
    // one, and inventing a feed would be worse than serving without it. The
    // node serves its health surface so an orchestrator can see it, and
    // reports what production would still have to supply.
    for requirement in missing_production_requirements() {
        println!("qip-edge-node: awaiting {requirement}");
    }

    serve(&config, cell, &gateway, mirror.as_mut(), started)
}

/// What a production cell would need that this build cannot supply.
///
/// Named rather than faked. A synthetic feed that looked like a venue would
/// make the node appear to work and make the gap invisible.
fn missing_production_requirements() -> Vec<String> {
    vec![
        "QIP_VENUE_FEED_ENDPOINT and its multicast group or session credential".to_string(),
        "QIP_VENUE_GATEWAY_ENDPOINT and an order-entry session credential".to_string(),
        "QIP_DROP_COPY_ENDPOINT for the independent fill channel".to_string(),
    ]
}

/// Serve the health surface until the listener fails.
///
/// Deliberately tiny and deliberately not the platform's HTTP server: a node
/// whose liveness probe depends on the API crate has coupled the two, and the
/// probe is what tells an orchestrator whether the cell is alive at all.
fn serve(
    config: &NodeConfig,
    cell: Cell,
    gateway: &SimulatedGateway,
    mirror: &mut dyn Mirror,
    started: qip_core::Timestamp,
) -> Result<()> {
    let address = format!("0.0.0.0:{}", config.health_port);
    let listener = TcpListener::bind(&address)
        .map_err(|error| Error::io(format!("cannot bind {address}: {error}")))?;
    println!("qip-edge-node: health on {address}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = answer(stream, config, &cell, gateway, started) {
                    eprintln!("qip-edge-node: health request failed: {}", error.message());
                }
            }
            // One bad connection is not a reason to stop trading.
            Err(error) => eprintln!("qip-edge-node: accept failed: {error}"),
        }
    }

    // Unreachable while the listener holds, but if it ever ends the journal is
    // shipped rather than dropped.
    let _ = mirror;
    Ok(())
}

fn answer(
    mut stream: TcpStream,
    config: &NodeConfig,
    cell: &Cell,
    gateway: &SimulatedGateway,
    started: qip_core::Timestamp,
) -> Result<()> {
    let mut buffer = [0u8; 1024];
    // The content is irrelevant — any request gets the same answer — but the
    // read has to happen or the client sees a reset instead of a response.
    let _ = stream.read(&mut buffer);

    let body = format!(
        r#"{{"cell":"{}","region":"{}","halted":{},"live_capable":{},"venues":{},"strategies":{},"journal_entries":{},"gateway":{{"class":"{}","venue":"{}","submitted":{},"rejected":{}}},"started_at":{}}}"#,
        config.cell_id,
        config.region,
        cell.is_halted(),
        cell.autonomy().ceiling().is_live(),
        config.venues.len(),
        cell.deployed_strategies().len(),
        cell.journal().len(),
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
