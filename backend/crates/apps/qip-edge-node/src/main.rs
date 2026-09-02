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
//! And one thing it must be *told*, because the default is the safe answer and
//! silence has to keep selecting it: **which venue adapter the orders go
//! through**. Absent any configuration the cell places against the in-process
//! matching engine and nothing leaves this process, which is what every
//! deployment of this binary has done until now. Naming the REST order-entry
//! adapter opens a socket to a real venue, and `qip_edge_node::venue` refuses
//! that unless an operator has written the destination out — because nothing
//! in this platform's code can tell a venue's sandbox host from its production
//! host, and the start-up banner says so beside the address it will use.
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

use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, SystemClock};
use qip_edge::cell::PolledHalt;
use qip_edge::cell::{Cell, CellConfig, Placer, WorkReport};
use qip_edge_node::arbitrage::{ArbitrageInstaller, STRATEGY_VARIABLE};
use qip_edge_node::feed::{FEED_VARIABLE, FeedChoice, SIMULATED_FEED, SimulatedFeed};
use qip_edge_node::gateway::NodeGateway;
use qip_edge_node::halt::{FLAG_VARIABLE, HaltFlag};
use qip_edge_node::mesh::{MeshLink, MeshSettings, PEER_VARIABLE};
use qip_edge_node::mirror::StoreMirror;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::telemetry::{MeshSeries, respond};
use qip_edge_node::venue::{ACKNOWLEDGEMENT_VARIABLE, ADAPTER_VARIABLE, VenueChoice};
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::Metrics;
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
    /// The second halt wire, when the deployment has mounted one. `None` is
    /// a node with the broadcast halt alone, which is named in the list of
    /// production requirements rather than silently accepted — see
    /// `qip_edge_node::halt`.
    halt_flag: Option<HaltFlag>,
    /// The strategy whose grant funds the arbitrage desk, when this node
    /// runs one. `None` installs no desk and is named in the production
    /// requirements — see `qip_edge_node::arbitrage`.
    arbitrage_strategy: Option<StrategyId>,
    /// The feed the pass loop prices from. `None` is a node that runs no
    /// pass at all — announced, never defaulted — and the only `Some` is the
    /// simulator; see `qip_edge_node::feed` for why a live feed is refused
    /// at start rather than read.
    feed: Option<FeedChoice>,
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
        let key = required_secret("QIP_CAPITAL_ENVELOPE_KEY", &mut missing)?;
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
        // Set but unusable is refused; unset is a node without the second
        // wire, which is allowed and announced.
        let halt_flag = match std::env::var(FLAG_VARIABLE) {
            Ok(value) if !value.trim().is_empty() => Some(HaltFlag::at(value.trim())?),
            _ => None,
        };

        let arbitrage_strategy = match std::env::var(STRATEGY_VARIABLE) {
            Ok(value) if !value.trim().is_empty() => Some(StrategyId::new(value.trim())),
            _ => None,
        };
        let feed = FeedChoice::from_env()?;

        Ok(Self {
            cell_id,
            region,
            venues,
            envelope_key: key,
            health_port,
            storage,
            mesh,
            halt_flag,
            arbitrage_strategy,
            feed,
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

/// A required credential, from its variable or from the file the deployment
/// mounted it at.
///
/// Separate from [`required`] because a credential has the second source and
/// ordinary configuration does not, and because the two failures are not the
/// same: a variable nobody set belongs in the collected `missing` list, while a
/// file that is named and unreadable is a specific fault whose message names
/// the path. Collapsing the second into "must be set" would send an operator
/// looking for a variable that is, in fact, already set.
fn required_secret(name: &str, missing: &mut Vec<String>) -> Result<Vec<u8>> {
    match qip_core::secret::from_environment(name)? {
        Some(value) if !value.trim().is_empty() => Ok(value.into_bytes()),
        _ => {
            missing.push(name.to_string());
            Ok(Vec::new())
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

    // The cell, the mesh series and the registry the scrape serves are wired
    // together in the library, where a test can prove they are one registry.
    // Assembled piecewise here, that property was held by a source check on
    // this file, which a second `Telemetry` inserted between the lines passed.
    let NodeAssembly {
        telemetry,
        mut cell,
        mesh_series,
    } = assemble(cell_config, features, Arc::clone(&clock))?;
    let metrics: &Arc<Metrics> = &telemetry.metrics;

    // The venue seam, and the one decision in this binary that is not
    // recoverable if it is wrong. Read before anything is opened and announced
    // before anything is sent: the banner is printed first so that an operator
    // watching a node fail to start still learns where it was about to send
    // orders.
    //
    // The ceiling is read from the cell's own autonomy controller rather than
    // from configuration, so "this deployment permits live execution" is
    // decided by the thing that would permit it.
    let gateway_venue = config
        .venues
        .first()
        .expect("from_env refuses an empty venue list")
        .clone();
    let ceiling = cell.autonomy().ceiling();
    let choice = VenueChoice::from_env(&gateway_venue, ceiling.is_live())?;
    for line in choice.banner_lines(ceiling.as_str()) {
        println!("{line}");
    }
    let mut gateway = NodeGateway::open(&choice, gateway_venue.clone(), started)?;

    // The feed, bound to the cell before anything is served. Refused here,
    // and not at the first pass, if the gateway it would price is not the
    // simulator: a simulated feed on a live gateway would send real orders
    // priced off a book nobody trades, and the node must not come up to
    // find that out.
    let mut feed = match config.feed {
        Some(FeedChoice::Simulated) => {
            if gateway.simulated_mut().is_none() {
                return Err(Error::denied(format!(
                    "configuration: {FEED_VARIABLE}={SIMULATED_FEED} prices passes off the \
                     in-process venue, and this node's order entry is {} on {}; a simulated \
                     feed does not drive a live gateway. Unset {ADAPTER_VARIABLE} or unset \
                     {FEED_VARIABLE}",
                    gateway.class(),
                    gateway.venue()
                )));
            }
            let feed = SimulatedFeed::new(gateway_venue);
            feed.attach(&mut cell)?;
            Some(feed)
        }
        None => None,
    };

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
        "qip-edge-node cell={} region={} venues={} live_capable={} gateway={}({}) adapter={} \
         reaches_a_socket={}",
        config.cell_id,
        config.region,
        config.venues.len(),
        ceiling.is_live(),
        gateway.class(),
        gateway.venue(),
        choice.selector(),
        gateway.reaches_a_socket()
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

    // The desk's installer, when the node is told which strategy funds it.
    // It holds nothing until a grant arrives over the mesh and installs
    // nothing until a whitelist does, so a node with no peer can never grow
    // a desk — which is right, since neither input can reach it.
    let mut installer = config
        .arbitrage_strategy
        .clone()
        .map(|strategy| ArbitrageInstaller::new(strategy, config.venues.clone()));

    for requirement in missing_production_requirements(
        config.mesh.is_some(),
        config.halt_flag.is_some(),
        installer.is_some(),
        config.feed,
        &gateway,
    ) {
        println!("qip-edge-node: awaiting {requirement}");
    }
    if let Some(flag) = &config.halt_flag {
        println!(
            "qip-edge-node: polled halt flag at {}",
            flag.path().display()
        );
    }

    serve(
        &config,
        &mut cell,
        &mut gateway,
        feed.as_mut(),
        &mut mirror,
        link.as_mut(),
        installer.as_mut(),
        &clock,
        started,
        metrics,
        mesh_series,
    )
}

/// What a production cell would need that this deployment has not supplied.
///
/// Named rather than faked. A synthetic feed that looked like a venue would
/// make the node appear to work and make the gap invisible.
///
/// The order-entry line is now conditional, because it can genuinely be
/// satisfied: a node that selected the REST adapter has an endpoint and an
/// authenticated session, and repeating "awaiting an order-entry credential"
/// beside a banner naming the venue it just logged on to would train an
/// operator to ignore this list. What replaces it is the adapter's own
/// standing requirements — the ones that hold *even when everything is
/// configured*, the first of which is that nothing in this code can tell a
/// sandbox endpoint from a production one.
fn missing_production_requirements(
    has_mesh_peer: bool,
    has_halt_flag: bool,
    has_arbitrage_strategy: bool,
    feed: Option<FeedChoice>,
    gateway: &NodeGateway,
) -> Vec<String> {
    let mut missing = vec![
        "QIP_VENUE_FEED_ENDPOINT and its multicast group or session credential".to_string(),
        "QIP_DROP_COPY_ENDPOINT for the independent fill channel".to_string(),
    ];
    match feed {
        // Passes run, and what they price off is said in the same breath:
        // the simulator's book holds what rests there and nothing from any
        // market, so an operator reading a non-zero order count knows what
        // it is a count of.
        Some(FeedChoice::Simulated) => missing.push(format!(
            "a market: {FEED_VARIABLE}={SIMULATED_FEED} runs passes priced off the in-process \
             venue's own resting depth, which holds only what this process rested there"
        )),
        None => missing.push(format!(
            "{FEED_VARIABLE}={SIMULATED_FEED} to run passes at all: without it this node polls \
             its halt, ships its journal and exchanges with the centre, and never decides"
        )),
    }
    if gateway.reaches_a_socket() {
        missing.extend(gateway.required_configuration());
    } else {
        missing.push(format!(
            "an order-entry venue: this cell places against the in-process matching engine and \
             nothing it does reaches a market. Set {ADAPTER_VARIABLE}=rest, the venue's own \
             endpoint, credential and account variables, and {ACKNOWLEDGEMENT_VARIABLE} to the \
             endpoint you mean"
        ));
    }
    if !has_mesh_peer {
        // Named rather than silently tolerated. A detached cell is a valid
        // state and an invisible one is not: this node will keep trading inside
        // the envelope it holds and will never be granted another.
        missing.push(format!(
            "{PEER_VARIABLE} for the central plane: without it this cell publishes no state and \
             receives no capital, and stops when its current envelope expires"
        ));
    }
    if !has_arbitrage_strategy {
        missing.push(format!(
            "{STRATEGY_VARIABLE} for the arbitrage desk: without it the cycle whitelist the \
             centre ships is applied and never priced, and this cell runs strategy programs \
             alone"
        ));
    }
    if !has_halt_flag {
        // The broadcast halt shares the mesh's failure. A node with no
        // second wire is a node that a partition leaves unhaltable for as
        // long as its envelope runs, and that is said here rather than left
        // for the incident to discover.
        missing.push(format!(
            "{FLAG_VARIABLE} for the second halt wire: without it the only halt this cell can \
             hear rides the mesh, and a partition that stops the mesh stops the halt with it"
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
    gateway: &mut NodeGateway,
    mut feed: Option<&mut SimulatedFeed>,
    mirror: &mut StoreMirror,
    mut link: Option<&mut MeshLink>,
    mut installer: Option<&mut ArbitrageInstaller>,
    clock: &Arc<dyn Clock>,
    started: qip_core::Timestamp,
    metrics: &Arc<Metrics>,
    mut mesh_series: MeshSeries,
) -> Result<()> {
    let address = format!("0.0.0.0:{}", config.health_port);
    let listener = TcpListener::bind(&address)
        .map_err(|error| Error::io(format!("cannot bind {address}: {error}")))?;
    println!("qip-edge-node: health on {address}");

    // The last reading of the polled halt flag, so a change is printed once
    // rather than on every probe; an unreadable flag is printed every time,
    // because it is an incident for as long as it lasts.
    let mut last_polled: Option<PolledHalt> = None;
    // The last pass's report, carried into the next probe's mesh exchange so
    // the delta the centre receives describes trading that happened rather
    // than an empty report. One probe behind, and said so: the pass runs
    // after the exchange, per the order below.
    let mut last_report = WorkReport::default();
    let mut stats = PassStats::default();
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let now = clock.now();
                // The second halt wire, polled first so that the halt it
                // applies is in the journal the flush below ships and in the
                // delta the exchange below publishes. It reads a file on this
                // machine and touches nothing on the mesh — see
                // `qip_edge_node::halt` for why that is the whole point.
                if let Some(flag) = &config.halt_flag {
                    let reading = flag.poll(cell, now);
                    let unreadable = matches!(reading, PolledHalt::Unreadable(_));
                    if unreadable || last_polled.as_ref() != Some(&reading) {
                        eprintln!(
                            "qip-edge-node: polled halt flag {}; the cell is {}",
                            reading.describe(),
                            if cell.is_halted() {
                                "halted"
                            } else {
                                "running"
                            }
                        );
                    }
                    last_polled = Some(reading);
                }
                // A journal that cannot be shipped is reported and the node
                // keeps serving: the entries are still held locally and still
                // chained, so the next flush ships them. Exiting here would
                // turn a storage outage into a trading outage.
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
                // The report is the previous pass's — empty for a node with
                // no feed, which then reports its authority and halt state
                // rather than its trading, as it always has.
                if let Some(link) = link.as_deref_mut() {
                    let tick =
                        link.exchange_with(cell, &last_report, now, installer.as_deref_mut());
                    if !tick.is_quiet() {
                        eprintln!("qip-edge-node: mesh exchange: {tick:?}");
                    }
                }
                // The pass, after the halt poll and the exchange so it runs
                // under the newest halt and the newest policy this probe
                // could learn. Only the simulated gateway is ever passed:
                // `run_pass` is typed to it, and a live gateway was refused
                // beside the feed at start-up.
                if let (Some(feed), Some(simulated)) =
                    (feed.as_deref_mut(), gateway.simulated_mut())
                {
                    match run_pass(cell, simulated, feed, &mut stats, now) {
                        Ok(PassOutcome::Ran { report, breaks, .. }) => {
                            for detail in &breaks {
                                eprintln!("qip-edge-node: reconciliation break: {detail}");
                            }
                            last_report = report;
                        }
                        Ok(PassOutcome::Halted { .. }) => {
                            last_report = WorkReport::default();
                        }
                        // A pass that failed is a fact the journal already
                        // holds where the cell refused; the loop keeps serving
                        // so the halt poll and the flush keep running.
                        Err(error) => {
                            eprintln!("qip-edge-node: the pass failed: {}", error.message());
                        }
                    }
                }
                let health = link.as_deref().map(MeshLink::health);
                // The link's counters become a series here, at the one place
                // in this node that reads them. Before this they reached the
                // JSON health body and nothing else, so a cell that had
                // stopped talking to the centre was a number that stopped
                // increasing where nothing was looking.
                if let Some(health) = health.as_ref() {
                    mesh_series.observe(health);
                }
                if let Err(error) = answer(
                    stream,
                    config,
                    cell,
                    gateway,
                    feed.as_deref(),
                    &stats,
                    mirror,
                    health,
                    started,
                    metrics,
                ) {
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
    gateway: &NodeGateway,
    feed: Option<&SimulatedFeed>,
    stats: &PassStats,
    mirror: &StoreMirror,
    mesh: Option<qip_edge_node::mesh::MeshHealth>,
    started: qip_core::Timestamp,
    metrics: &Metrics,
) -> Result<()> {
    let mut buffer = [0u8; 1024];
    // The read has to happen or the client sees a reset instead of a response.
    // It is also the only thing that distinguishes a scrape from a probe: this
    // node answered every request identically until it had something a scraper
    // could read, and a `/metrics` path that returned a health blob would be
    // silently unparseable to every collector that asked.
    let read = stream.read(&mut buffer).unwrap_or(0);

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
    // The polled wire's state, as a fixed word: whether a second wire is
    // configured at all, and whether it is what holds the cell. A probe that
    // could see `halted` but not which wire would send an operator to clear
    // the wrong one.
    let halt_flag = match &config.halt_flag {
        Some(flag) => format!(
            r#"{{"path":"{}","engaged":{}}}"#,
            flag.path().display(),
            cell.polled_halt().is_some()
        ),
        None => "null".to_string(),
    };
    // The pass loop's state: which feed, if any, and what its passes have
    // done. `feed` is `null` for a node that runs no pass, so a probe can
    // tell "never decides" from "decides and refuses everything" — the two
    // read identically from the order count alone.
    let pass = match feed {
        Some(feed) => format!(
            r#"{{"feed":"{}","instruments":{},"instruments_omitted":{},"passes":{},"halted_turns":{},"refusals":{},"signals":{},"orders":{},"fills":{},"expired":{},"breaks":{}}}"#,
            SIMULATED_FEED,
            feed.tracked(),
            feed.omitted_total(),
            stats.passes,
            stats.halted,
            stats.refusals,
            stats.signals,
            stats.orders,
            stats.fills,
            stats.expired,
            stats.breaks,
        ),
        None => "null".to_string(),
    };
    let body = format!(
        r#"{{"cell":"{}","region":"{}","halted":{},"halt_flag":{halt_flag},"arbitrage_desk":{},"live_capable":{},"venues":{},"strategies":{},"journal_entries":{},"journal_shipped":{},"storage":"{}","durable":{},"gateway":{{"class":"{}","venue":"{}","submitted":{},"rejected":{},"reaches_a_socket":{},"unknown_orders":{}}},"pass":{pass},"mesh":{mesh},"started_at":{}}}"#,
        config.cell_id,
        config.region,
        cell.is_halted(),
        cell.arbitrage().is_some(),
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
        gateway.reaches_a_socket(),
        // The number to alert on, and zero for a venue that never sends an
        // order. An unknown order is one that may be working at the venue, may
        // have filled, or may never have arrived, and it is published because
        // the only thing worse than having one is not knowing how many.
        gateway.unknown_orders(),
        started.as_secs()
    );
    // Routing lives in the library so it can be tested against the same
    // registry the cell writes to. See `qip_edge_node::telemetry::respond`.
    let (content_type, payload) = respond(&buffer[..read], metrics, &body);
    write_response(stream, content_type, &payload)
}

fn write_response(mut stream: TcpStream, content_type: &str, body: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| Error::io(format!("cannot write the health response: {error}")))?;
    stream
        .flush()
        .map_err(|error| Error::io(format!("cannot flush the health response: {error}")))
}
