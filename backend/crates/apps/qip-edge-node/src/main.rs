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
//! * A **region allocation**. The one amount the whole cell may commit,
//!   across every strategy it runs. Each envelope bounds one strategy and
//!   nothing on any wire bounds their sum, so a cell without this can spend
//!   every envelope in full while partitioned from the centre — see
//!   `qip_edge_node::allocation` for why it is refused rather than defaulted.
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

use qip_arbitrage::SearchSettings;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::deployment::{COMPUTE_ENGINE_PRODUCT_FILE, Deployment, DeploymentMarkers};
use qip_core::error::{Error, Result};
use qip_core::{Clock, Decimal, Duration, SystemClock};
use qip_edge::cell::{CellConfig, Placer, PricingPolicy};
use qip_edge::fencing::Fence;
use qip_edge::telemetry::CellMetrics;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::arbitrage::{
    ArbitrageInstaller, MAX_LEGS_VARIABLE, STRATEGY_VARIABLE, parse_max_legs,
};
use qip_edge_node::control::{Handoff as ControlHandoff, PlanCompiler, Poller};
use qip_edge_node::cross_region::{CrossRegionMirror, MIRROR_VARIABLE, no_declaration_line};
use qip_edge_node::dark::DarkRegionWire;
use qip_edge_node::event_fabric::drain::{Drain, DrainConfig, LakeSink, Sink};
use qip_edge_node::event_fabric::inputs::RecordedInputs;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::placement::{
    Placement, WRITER_CPU_VARIABLE, current_placement, parse_writer_cpu, pin_current_thread,
};
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::relay::{JournalRelay, RelayConfig};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{self, SpoolWriter, WriterConfig};
use qip_edge_node::feed::{FEED_VARIABLE, FeedChoice, SIMULATED_FEED, SimulatedFeed};
use qip_edge_node::gateway::NodeGateway;
use qip_edge_node::halt::{FLAG_VARIABLE, HaltFlag};
use qip_edge_node::health::{self, HealthSnapshot};
use qip_edge_node::mesh::MeshWorker;
use qip_edge_node::mesh::{MeshLink, MeshSettings, PEER_VARIABLE};
use qip_edge_node::mirror::StoreMirror;
use qip_edge_node::outbox::{
    FABRIC_URL_VARIABLE, FabricSettings, PASS_INTERVAL, SPOOL_BUDGET_BYTES, journal_bound,
    read_fabric, spool_dir, spool_survives_the_machine,
};
use qip_edge_node::pass::PassMeter;
use qip_edge_node::quote_limits::{QUOTE_LIMITS_VARIABLE, VenueQuoteLimits};
use qip_edge_node::reprice::{REPRICE_VARIABLE, Requoter, parse_reprice};
use qip_edge_node::risk::{CellRiskLimits, RISK_LIMITS_VARIABLE};
use qip_edge_node::rollback::{ROLLBACK_VARIABLE, RollbackRequest, RollbackWire};
use qip_edge_node::scheduler::{DecisionLoop, HealthFacts, InstantClock, MarkerFacts, Outbox};
use qip_edge_node::share::RegionShareStatus;
use qip_edge_node::standby::{
    FileLeaseStore, LEASE_VARIABLE, LeaseKeeper, LeaseThread, LeaseTiming, NAME_VARIABLE,
    ROLE_VARIABLE, SimulatedSessions, StandbySettings,
};
use qip_edge_node::strategies::{
    MARKETABLE, PLAN_VARIABLE, PRICING_VARIABLE, REST_AT_MID_PREFIX, StrategyInstaller,
    parse_pricing,
};
use qip_edge_node::trust::{TRUST_BUNDLE_VARIABLE, choose_root};
use qip_edge_node::venue::{ACKNOWLEDGEMENT_VARIABLE, ADAPTER_VARIABLE, VenueChoice};
use qip_edge_node::{NodeAssembly, assemble_with_risk};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::Metrics;
use qip_peer_link::{CLOCK_BOUND_VARIABLE, LISTEN_VARIABLE, PEERS_VARIABLE, PeerSettings};
use qip_routing::reprice::RepricePolicy;
use qip_storage::lake::LAKE_NAMESPACE;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_storage::settings::{ROOT_VARIABLE, StorageSettings, TARGET_VARIABLE};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

/// Exit code for a configuration problem, matching `sysexits.h`.
///
/// Distinct from a general failure so an orchestrator can tell "this node was
/// deployed wrong" from "this node broke", and stop restarting the first.
const EX_CONFIG: i32 = 78;

/// How long one health request may take to arrive, and its answer to be
/// written, before the node gives up on it.
///
/// The health surface has its own thread now (`qip_edge_node::health`), so a
/// client holding a socket open no longer stops the cell deciding; it still
/// stops every later probe on that single-threaded server, which reads as a
/// dead node to the orchestrator. Two seconds is far longer than a loopback
/// probe or the Ops Agent's scrape needs, and far shorter than either one's
/// own timeout.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// How old the spool writer's heartbeat may be before the gauge reads the
/// writer as stopped and halts new exposure. Forty polls of the writer.
const HEARTBEAT_BOUND: Duration = Duration::from_secs(2);

/// The spool's segment size. Small enough that the drain's seal cadence,
/// not the size trigger, decides when a quiet cell's records retire.
const SPOOL_ROLL_BYTES: u64 = 1 << 20;

/// Handoffs the decision thread may have in flight to the relay, and the
/// relay to the writer. A full channel is a refused flush, never a wait.
const HANDOFF_CAPACITY: usize = 1_024;

/// How long the writer and relay wait on an empty channel before beating
/// or checking for a stop.
const WRITER_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// The most handoffs the writer groups into one fsync.
const WRITER_GROUP: usize = 256;

/// Compiled plans waiting for a pass boundary.
const PLAN_HANDOFF_CAPACITY: usize = 16;

/// How often the halt flag and region wire are read, and how long a reading
/// is believed — past it, the wire reads as engaged (§46.2).
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
const POLL_BOUND: std::time::Duration = std::time::Duration::from_secs(2);

/// Mesh polls waiting for a pass boundary.
const MESH_CAPACITY: usize = 16;

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
    /// ADR 0110 decision 5 and ADR 0112: which deployment this node is in,
    /// named and never inferred. Decides whether the shared key may still
    /// stand after its removal date and whether the fabric link may be
    /// plaintext — `local` only, for both.
    deployment: Deployment,
    /// The committed trust bundle, when the node verifies under ADR 0110.
    trust_bundle: Option<String>,
    /// The shared HMAC key, for the release that keeps it. Never both this
    /// and `trust_bundle`; `qip_edge_node::trust::choose_root` refuses the
    /// pair.
    shared_key: Option<Vec<u8>>,
    /// The capital the whole cell may commit. Required, and a `RegionCapital`
    /// rather than a `Decimal` so the only value that can reach the cell is
    /// one that passed `RegionCapital::read` — see `qip_edge_node::allocation`.
    region_allocation: RegionCapital,
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
    /// §36.3's region wire, beside the halt flag and under no variable of its
    /// own — see `qip_edge_node::dark` for why the same mount carries both.
    /// `None` is a node with no halt flag either, which can never be told
    /// that a peer region has gone dark.
    region_wire: Option<DarkRegionWire>,
    /// The operator's rollback request (REFLEX-8071), when the deployment
    /// names one. `None` is a node whose resident previous payload can be
    /// returned to by nobody — see `qip_edge_node::rollback`.
    rollback: Option<RollbackRequest>,
    /// The strategy whose grant funds the arbitrage desk, when this node
    /// runs one. `None` installs no desk and is named in the production
    /// requirements — see `qip_edge_node::arbitrage`.
    arbitrage_strategy: Option<StrategyId>,
    /// What the desk searches under, the maximum leg count in force above
    /// all (MESH-010). The engine's defaults when `QIP_ARBITRAGE_MAX_LEGS`
    /// is unset; a value outside two to twenty stops the process — see
    /// `qip_edge_node::arbitrage::parse_max_legs`.
    arbitrage_search: SearchSettings,
    /// The feed the pass loop prices from. `None` is a node that runs no
    /// pass at all — announced, never defaulted — and the only `Some` is the
    /// simulator; see `qip_edge_node::feed` for why a live feed is refused
    /// at start rather than read.
    feed: Option<FeedChoice>,
    /// How every strategy the plan deploys is priced at the venue, until
    /// the plan slot carries a policy of its own. `None` deploys nothing
    /// and is named in the production requirements; a value that is not
    /// one of the two forms stops the process — see
    /// `qip_edge_node::strategies`.
    pricing: Option<PricingPolicy>,
    /// Where the compiled plan the payload names by digest is read from.
    /// `None` deploys nothing and is named in the production requirements.
    plan_path: Option<PathBuf>,
    /// When a resting order is stale enough to withdraw and re-send at the
    /// touch. `None` leaves every resting order where it was sent until its
    /// time to live and is named in the production requirements; a value
    /// with no feed to run passes is refused, because a policy nothing
    /// consults is a control that reads as one — see
    /// `qip_edge_node::reprice`.
    reprice: Option<RepricePolicy>,
    /// Which of this node's venues are abroad and under what discipline this
    /// region mirrors each instrument (§31.1). `None` is a cell all of whose
    /// venues are at home — every node deployed so far — and is announced at
    /// start-up rather than assumed; see `qip_edge_node::cross_region`.
    mirror: Option<CrossRegionMirror>,
    /// Each venue's own message rate and message-to-trade ratio (EXEC-004).
    /// Empty is every venue on the cell's fallback ceiling, announced venue
    /// by venue at start-up; an entry that is malformed, or names a venue
    /// outside `QIP_VENUES`, stops the process — see
    /// `qip_edge_node::quote_limits`.
    quote_limits: VenueQuoteLimits,
    /// The fabric partition the spool drains to, when one is configured —
    /// see `qip_edge_node::outbox`. Taken once, because it holds the token.
    fabric: Option<FabricSettings>,
    /// The limits the cell's deterministic Risk Gate evaluates. Never
    /// absent: unset is the shipped set, and a named file that cannot be
    /// read or names a limit the cell cannot evaluate stops the process —
    /// see `qip_edge_node::risk`.
    risk_limits: CellRiskLimits,
    /// The fencing lease this process holds or stands by for, when it runs
    /// as one of a primary/standby pair. `None` is a node with no standby,
    /// announced — see `qip_edge_node::standby`.
    standby: Option<StandbySettings>,
    /// The peer Reflex Mesh, when this cell has peers. `None` is a cell that
    /// coordinates with no other cell directly, announced — see
    /// `qip_peer_link`.
    peers: Option<PeerSettings>,
    /// The core the spool writer is pinned to, when the deployment names
    /// one — see `qip_edge_node::event_fabric::placement`.
    writer_cpu: Option<usize>,
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
        // The shared key is optional now: a node verifying under the trust
        // bundle must not hold it, and `choose_root` refuses a node with
        // neither. Read here, through the `_FILE` indirection, so the refusal
        // names the variable rather than a path.
        let shared_key = qip_core::secret::from_environment("QIP_CAPITAL_ENVELOPE_KEY")
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?
            .filter(|value| !value.trim().is_empty())
            .map(String::into_bytes);
        let trust_bundle = std::env::var(TRUST_BUNDLE_VARIABLE)
            .ok()
            .filter(|value| !value.trim().is_empty());
        let deployment_name = required("QIP_DEPLOYMENT", &mut missing);
        let venues = required("QIP_VENUES", &mut missing);
        // Spelled as a literal beside the other four rather than through
        // `allocation::ALLOCATION_VARIABLE`: the acceptance walk in
        // `manifest_wiring.rs` learns what this binary refuses to start
        // without by reading the quoted name in each `required` call, and a
        // name it could not see there is a crash loop it could not warn a
        // deployment about. (That walk reads comments too, so this one must
        // not spell the call with a placeholder name inside the quotes.)
        let region_allocation = required("QIP_REGION_ALLOCATION", &mut missing);

        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "configuration: {} must be set",
                missing.join(", ")
            )));
        }

        // Checked before anything below opens a port or a store, and refused
        // outright: a node whose allocation is unparseable or zero must never
        // reach the cell with a number it made up in place of one.
        let region_allocation = RegionCapital::read(Some(&region_allocation))?;
        // `local` is refused on a Compute Engine guest (the DMI product
        // name) or under Cloud Run or Kubernetes: `add-metadata` setting
        // local+plaintext must not re-admit the seed key and a plaintext
        // fabric link on a deployed node.
        let deployment = Deployment::parse_observed(
            Some(&deployment_name),
            &DeploymentMarkers::observe(
                &|name| std::env::var(name).ok(),
                std::fs::read_to_string(COMPUTE_ENGINE_PRODUCT_FILE)
                    .ok()
                    .as_deref(),
            ),
        )
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

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

        let storage = StorageSettings::from_env(&|name| std::env::var(name).ok())
            .and_then(StorageSettings::require_authoritative)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
        let mesh = MeshSettings::from_env(&cell_id, &region)?;
        // Set but unusable is refused; unset is a node without the second
        // wire, which is allowed and announced.
        let halt_flag = match std::env::var(FLAG_VARIABLE) {
            Ok(value) if !value.trim().is_empty() => Some(HaltFlag::at(value.trim())?),
            _ => None,
        };
        // Derived from the flag rather than read on its own: one mount, one
        // operator's hand, and therefore no second variable a deployment can
        // set inconsistently with the first.
        let region_wire = halt_flag
            .as_ref()
            .and_then(|flag| DarkRegionWire::beside(flag.path()));
        // Set but unusable is refused, as the halt flag is; unset is a node
        // with no rollback control, which is how every node ran before it.
        let rollback = match std::env::var(ROLLBACK_VARIABLE) {
            Ok(value) if !value.trim().is_empty() => Some(RollbackRequest::at(value.trim())?),
            Ok(value) if !value.is_empty() => {
                return Err(Error::invalid(format!(
                    "configuration: {ROLLBACK_VARIABLE} is whitespace; name an absolute path, \
                     or unset it to run with no rollback control"
                )));
            }
            _ => None,
        };

        let arbitrage_strategy = match std::env::var(STRATEGY_VARIABLE) {
            Ok(value) if !value.trim().is_empty() => Some(StrategyId::new(value.trim())),
            _ => None,
        };
        let max_legs = std::env::var(MAX_LEGS_VARIABLE).ok();
        let arbitrage_search = parse_max_legs(max_legs.as_deref())?;
        if max_legs.is_some_and(|value| !value.trim().is_empty()) && arbitrage_strategy.is_none() {
            return Err(Error::invalid(format!(
                "configuration: {MAX_LEGS_VARIABLE} is set and {STRATEGY_VARIABLE} is not; a \
                 node with no desk scans no cycle, and a leg limit nothing consults reads as a \
                 control. Set {STRATEGY_VARIABLE} or unset {MAX_LEGS_VARIABLE}"
            )));
        }
        let feed = FeedChoice::from_env()?;
        let pricing = parse_pricing(std::env::var(PRICING_VARIABLE).ok().as_deref())?;
        let plan_path = match std::env::var(PLAN_VARIABLE) {
            Ok(value) if !value.trim().is_empty() => Some(PathBuf::from(value.trim())),
            _ => None,
        };
        let reprice = parse_reprice(std::env::var(REPRICE_VARIABLE).ok().as_deref())?;
        // Read against the venue list and the region above, because both
        // refusals it carries need them: a venue placed abroad that this cell
        // may not trade, and a region that is the cell's own. Read here rather
        // than after the store is opened, so a declaration nobody can satisfy
        // stops the process before anything is bound.
        let mirror = CrossRegionMirror::read(
            std::env::var(MIRROR_VARIABLE).ok().as_deref(),
            &region,
            &venues,
        )?;
        // Read against the venue list for the same reason the mirror is: a
        // limit stated for a venue this cell may not trade binds nothing.
        let quote_limits = VenueQuoteLimits::read(
            std::env::var(QUOTE_LIMITS_VARIABLE).ok().as_deref(),
            &venues,
        )?;
        if reprice.is_some() && feed.is_none() {
            return Err(Error::invalid(format!(
                "configuration: {REPRICE_VARIABLE} is set and {FEED_VARIABLE} is not; a node \
                 that runs no pass consults no requote policy, and a policy nothing consults \
                 reads as a control. Set {FEED_VARIABLE}={SIMULATED_FEED} or unset \
                 {REPRICE_VARIABLE}"
            )));
        }

        let fabric = read_fabric(&|name| std::env::var(name).ok())?;
        let writer_cpu = parse_writer_cpu(std::env::var(WRITER_CPU_VARIABLE).ok().as_deref())?;
        // ADR 0112: the peer mesh is mutual TLS with this node's workload
        // certificate, and plaintext only where the deployment is `local`
        // and says `plaintext` — and then on loopback alone. The certificate
        // is loaded only when a peer mesh is configured, so a cell with no
        // peers needs none.
        let peers = PeerSettings::read(
            &cell_id,
            std::env::var(LISTEN_VARIABLE).ok().as_deref(),
            std::env::var(PEERS_VARIABLE).ok().as_deref(),
            std::env::var(CLOCK_BOUND_VARIABLE).ok().as_deref(),
            &|| {
                qip_transport::tls::LinkSecurity::from_lookup(deployment, &|name| {
                    std::env::var(name).ok()
                })
            },
        )?;
        let standby = StandbySettings::read(
            std::env::var(LEASE_VARIABLE).ok().as_deref(),
            std::env::var(ROLE_VARIABLE).ok().as_deref(),
            std::env::var(NAME_VARIABLE).ok().as_deref(),
            deployment,
        )?;
        let risk_limits = CellRiskLimits::read(
            std::env::var(RISK_LIMITS_VARIABLE).ok().as_deref(),
            |path| std::fs::read_to_string(path),
        )?;

        Ok(Self {
            cell_id,
            region,
            venues,
            deployment,
            trust_bundle,
            shared_key,
            region_allocation,
            health_port,
            storage,
            mesh,
            halt_flag,
            region_wire,
            rollback,
            arbitrage_strategy,
            arbitrage_search,
            feed,
            pricing,
            plan_path,
            reprice,
            mirror,
            quote_limits,
            fabric,
            risk_limits,
            standby,
            peers,
            writer_cpu,
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
fn run() -> Result<()> {
    let mut config = NodeConfig::from_env()?;

    // The clock is read once, here, at the boundary. Everything inside the
    // cell takes a timestamp as a parameter, which is what makes a session
    // replayable from its journal.
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let started = clock.now();

    // What every grant, payload and halt is verified against (ADR 0110):
    // the trust bundle's public keys, or the shared key for its last
    // release. Chosen before anything is bound, and refused when there is
    // neither — a cell that cannot verify a grant must not trade.
    let trust = choose_root(
        config.trust_bundle.as_deref(),
        config.shared_key.as_deref(),
        config.deployment,
        started,
    )?;
    println!("qip-edge-node: trust root {}", trust.describe());

    let mut cell_config = CellConfig::new(&config.cell_id, &config.region);
    for venue in &config.venues {
        cell_config = cell_config.with_venue(venue.clone());
    }
    // EXEC-004. Until this line every node ran one default budget on every
    // venue, so a venue's own rate and ratio were found out when the venue
    // enforced them. Stated limits refuse before the gateway is called; a
    // venue with none keeps the fallback ceiling, and that is said by name
    // rather than left to be assumed.
    let cell_config = config.quote_limits.apply(cell_config);
    // W0.9 and ADR 0100 §6: the journal drops what the relay has taken, so
    // memory holds what has not shipped rather than everything since start,
    // and the cell reads the journal-pressure wire the decision loop feeds
    // every turn. A wired cell nobody feeds stays halted, which is why the
    // wire is armed here and nowhere a spool is not.
    let cell_config = cell_config
        .with_journal_trimmed_on_ship()
        .with_journal_wire();
    // Recorded on every pass marker, so a replay can refuse to call a pass
    // reproduced under a configuration it was not run under.
    let config_digest = qip_core::sha256_hex(format!("{cell_config:?}").as_bytes());
    for line in config.quote_limits.banner_lines() {
        println!("{line}");
    }
    for venue in config.quote_limits.unstated(&config.venues) {
        println!(
            "qip-edge-node: awaiting {QUOTE_LIMITS_VARIABLE} for {venue}: without it this venue \
             runs the cell's default message ceiling, which is no venue's stated limit, and \
             its message-to-trade ratio narrows quoting without refusing it"
        );
    }
    // Built empty, and not left that way: which instruments this cell will
    // trade is the plan's to say, so `StrategyInstaller` registers the
    // standard suite for each strategy's subject as it deploys it
    // (`Cell::register_features`). An engine nothing registered into was
    // what every node ran until then.
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));

    // The cell, the mesh series and the registry the scrape serves are wired
    // together in the library, where a test can prove they are one registry.
    // Assembled piecewise here, that property was held by a source check on
    // this file, which a second `Telemetry` inserted between the lines passed.
    // The region allocation goes in by the same door, for the same reason:
    // the builder that installs it was called by no composition root for as
    // long as calling it was optional.
    let NodeAssembly {
        telemetry,
        mut cell,
        mesh_series,
    } = assemble_with_risk(
        cell_config,
        features,
        Arc::clone(&clock),
        config.region_allocation,
        config.mirror.clone(),
        &config.risk_limits,
    )?;
    // Said at start, because which limits the reflex path refuses on is the
    // first thing an operator reading a refusal under `risk_gate` needs.
    println!("qip-edge-node: {}", config.risk_limits.describe());
    // Said before the venue banner, because which venues are abroad changes
    // what the venue line below means: two venues in one region compose a
    // transport edge and two across a boundary compose a mirror edge, and an
    // operator reading only the venue list cannot tell which this node builds.
    match &config.mirror {
        Some(mirror) => println!("{}", mirror.banner_line()),
        None => println!("{}", no_declaration_line(&config.region)),
    }
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
    //
    // `from_env` already refuses an empty venue list, so this arm is not
    // expected to run; it is a refusal rather than an `expect` because a
    // composition root refuses invalid configuration by name and never
    // panics on it (`.claude/rules/architecture/00-boundaries.md`). A panic
    // here would be the one exit from this binary that named no variable to
    // set, and it was the one `expect` outside tests in the workspace.
    let gateway_venue = config.venues.first().cloned().ok_or_else(|| {
        Error::invalid(
            "configuration: QIP_VENUES names no venue, so there is no venue to open a \
             gateway to; set QIP_VENUES to a comma-separated venue list",
        )
    })?;
    let ceiling = cell.autonomy().ceiling();
    let choice = VenueChoice::from_env(&gateway_venue, ceiling.is_live())?;
    for line in choice.banner_lines(ceiling.as_str()) {
        println!("{line}");
    }
    let mut gateway = NodeGateway::open(&choice, gateway_venue.clone(), started)?;

    // The fencing lease, before the feed and before anything is served: a
    // standby that reported healthy before its fence was installed would be
    // a second owner for as long as that took. Refused on any adapter but
    // the simulator, because the venue-session check a promotion requires
    // exists only for the simulator, whose session is its process.
    let lease_thread = match &config.standby {
        Some(settings) => {
            if !matches!(choice, VenueChoice::Simulated { .. }) {
                return Err(Error::invalid(format!(
                    "configuration: {LEASE_VARIABLE} is set on a node whose venue adapter is not                      the simulator; a standby promotes only once every venue reports the old                      holder's session ended, and no adapter but the simulator can answer that                      yet"
                )));
            }
            let fence = Fence::new();
            cell.install_fence(fence.clone());
            let store = FileLeaseStore::at(&settings.path)?;
            let keeper = LeaseKeeper::new(
                &settings.name,
                settings.role,
                LeaseTiming::deployed()?,
                Arc::new(store),
                fence,
                config.venues.clone(),
                Box::new(SimulatedSessions),
            )?;
            println!(
                "qip-edge-node: fencing lease at {} as {} `{}`: ttl {:?}, margin {:?}; this cell                  places nothing until it holds the epoch. The file store is correct on one host                  only: a standby in another zone needs a store with a conditional write, which                  this tree does not have",
                settings.path.display(),
                settings.role.as_str(),
                settings.name,
                keeper.timing().ttl,
                keeper.timing().margin,
            );
            Some(LeaseThread::spawn(keeper, Arc::clone(&clock))?)
        }
        None => None,
    };

    // The feed, bound to the cell before anything is served. Refused here,
    // and not at the first pass, if the gateway it would price is not the
    // simulator: a simulated feed on a live gateway would send real orders
    // priced off a book nobody trades, and the node must not come up to
    // find that out.
    let feed = match config.feed {
        Some(FeedChoice::Simulated) => {
            gateway.simulated_for_feed()?;
            let feed = SimulatedFeed::new(gateway_venue);
            feed.attach(&mut cell)?;
            Some(feed)
        }
        None => None,
    };
    // The requoter, recording into the registry the scrape serves — the
    // same handle the cell was given, so its one series sits beside the
    // cell's. `from_env` has already refused a policy with no feed.
    let requoter = match &config.reprice {
        Some(policy) => {
            let requoter = Requoter::new(
                policy.clone(),
                CellMetrics::new(Arc::clone(metrics), &config.cell_id, &config.region),
            )?;
            println!(
                "qip-edge-node: requote policy tick={} stale at {} tick(s) or {} bps, \
                 {} requotes per order, {} per instrument per {:.0}s",
                policy.tick,
                policy.max_drift_ticks,
                policy.max_drift_bps_f64,
                policy.max_requotes_per_order,
                policy.max_requotes_per_instrument,
                policy.per_instrument_window.as_secs_f64()
            );
            Some(requoter)
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
    let mirror = StoreMirror::open(
        config.storage.key_value("cell-journal")?,
        &config.cell_id,
        started,
    )?;
    let retained_sessions = mirror.retained_sessions()?;
    // §36.3's node-crash row: "that region dark; reconciles against every
    // venue before resuming". A store that already holds a session is a
    // previous run of this cell, and this process knows nothing about the
    // orders that run left resting — it rebuilds its books from the feed and
    // chains its journal onto genesis. So the cell forms no order until every
    // venue's own account has agreed with its record.
    //
    // Armed here rather than inside the cell, and only on the evidence of a
    // prior session: a cell that armed itself on every start would stop the
    // genuinely first start of every node for ever, waiting for an account of
    // a venue it has never sent anything to, which is a gate that cannot open
    // rather than a control that fires.
    if retained_sessions > 0 {
        cell.require_reconciliation_before_resuming(
            format!(
                "this journal store already holds {retained_sessions} session(s), so this \
                 process is a restart of a cell that has run before"
            ),
            started,
        )?;
        println!(
            "qip-edge-node: reconciling before resuming: {} venue(s) must answer with what they \
             hold open for this cell before it forms an order (§36.3)",
            config.venues.len()
        );
    }

    // The table is printed as the cell holds it, not as configuration read
    // it, so the banner is a claim about the cell and not about a variable —
    // the two were allowed to differ for as long as no root installed the
    // value it read. Both numbers, because they are different facts: the
    // ceiling is the operator's and the bound is the centre's share, which
    // is nothing until a payload names one (ADR 0039).
    let share = RegionShareStatus::of(&cell);
    println!(
        "qip-edge-node cell={} region={} venues={} region_ceiling={} region_bound={} \
         live_capable={} gateway={}({}) adapter={} reaches_a_socket={}",
        config.cell_id,
        config.region,
        config.venues.len(),
        share
            .ceiling
            .map_or_else(|| "none".to_string(), |ceiling| ceiling.to_string()),
        share
            .bound
            .map_or_else(|| "none".to_string(), |bound| bound.to_string()),
        ceiling.is_live(),
        gateway.class(),
        gateway.venue(),
        choice.selector(),
        gateway.reaches_a_socket()
    );
    if let Some(why) = &share.why {
        println!("qip-edge-node: region share: {why}");
    }

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
    let link = match &config.mesh {
        Some(settings) => {
            let link = MeshLink::connect(settings, trust.clone(), Arc::clone(&clock))?;
            println!("qip-edge-node: mesh peer {}", link.peer());
            Some(link)
        }
        None => None,
    };

    // The desk's installer, when the node is told which strategy funds it.
    // It holds nothing until a grant arrives over the mesh and installs
    // nothing until a whitelist does, so a node with no peer can never grow
    // a desk — which is right, since neither input can reach it.
    let installer = config.arbitrage_strategy.clone().map(|strategy| {
        ArbitrageInstaller::new(strategy, config.venues.clone())
            .with_search(config.arbitrage_search)
    });
    // The plan's installer, always: it holds the grants for strategies the
    // plan will name and deploys them once a fresh, verified payload names
    // a plan whose bytes are at the configured path. With no pricing or no
    // path it deploys nothing and says which on every tick, so a node
    // running no strategy is a node whose log says why.
    let strategies = StrategyInstaller::new(config.plan_path.clone(), config.pricing);

    for requirement in missing_production_requirements(
        config.mesh.is_some(),
        config.halt_flag.is_some(),
        installer.is_some(),
        config.feed,
        &gateway,
        &strategies,
        requoter.is_some(),
        lease_thread.is_some(),
    ) {
        println!("qip-edge-node: awaiting {requirement}");
    }
    // The maximum leg count in force, said out loud when there is a desk to
    // hold it to: a limit an operator has to infer from the absence of long
    // cycles is a limit nobody can check against what they configured.
    if let Some(installer) = &installer {
        println!(
            "qip-edge-node: arbitrage cycles of at most {} legs ({MAX_LEGS_VARIABLE}); a longer \
             one is refused under arbitrage_scan_length",
            installer.max_legs()
        );
    }
    if let Some(flag) = &config.halt_flag {
        println!(
            "qip-edge-node: polled halt flag at {}",
            flag.path().display()
        );
    }
    // §36.3's region wire, beside the halt flag and under no variable of its
    // own — see `qip_edge_node::dark` for why the same mount carries both.
    // Announced in both directions: a node with no wire can never be told
    // that a peer has gone dark, and an operator reading a cell that kept
    // mirroring through another region's outage needs to know that is the
    // reason rather than a gate that failed.
    match &config.region_wire {
        Some(wire) => println!(
            "qip-edge-node: region availability wire at {} (absent file: no region dark)",
            wire.path().display()
        ),
        None => println!(
            "qip-edge-node: no region availability wire ({FLAG_VARIABLE} unset): no peer region \
             can be declared dark on this node, so a cross-region mirror is gated only by \
             §31.1's reference window"
        ),
    }

    // ---- the outbox: spool, writer, relay, drain (ADR 0100 §1, §6) --------
    //
    // Built after the store is proven writable and before anything is
    // served, for the reason the store is: a node that reported healthy and
    // then found its spool unopenable would be deciding with no record.
    let outbox_telemetry = Arc::new(OutboxTelemetry::new(Arc::clone(metrics)));
    // The gauge's clock is monotonic, never the wall clock: a heartbeat
    // judged on a clock NTP can step would halt a healthy cell.
    let gauge_clock: Arc<dyn Clock> = Arc::new(InstantClock::anchored_at(started));
    let thresholds = Thresholds::new(SPOOL_BUDGET_BYTES, fraction("0.5")?, fraction("0.9")?)?;
    let (gauge, spool_publisher, drain_publisher) =
        PressureGauge::new(thresholds, Arc::clone(&gauge_clock), HEARTBEAT_BOUND)?;
    let spool_dir = spool_dir(&config.cell_id, &config.storage);
    let spool = Arc::new(
        SegmentLog::open(
            &spool_dir,
            SegmentLogConfig::new(Arc::clone(&gauge_clock)).with_roll_after_bytes(SPOOL_ROLL_BYTES),
        )
        .map_err(|error| {
            Error::invalid(format!(
                "configuration: the spool at {} cannot be opened: {}",
                spool_dir.display(),
                error.message()
            ))
        })?,
    );
    println!(
        "qip-edge-node: spool at {} (budget {} bytes, narrowing at half, halting at nine \
         tenths); journal bound {} unshipped entries",
        spool_dir.display(),
        SPOOL_BUDGET_BYTES,
        journal_bound()?.exhaust_at()
    );
    if !spool_survives_the_machine(&config.storage) {
        eprintln!(
            "qip-edge-node: the spool is in a temporary directory because the storage target \
             has no local root; it does not survive this machine. Set {TARGET_VARIABLE}=engine \
             and {ROOT_VARIABLE} to keep it"
        );
    }
    let (to_relay, from_mirror) = writer::channel(HANDOFF_CAPACITY)?;
    let (to_writer, writer_inbox) = writer::channel(HANDOFF_CAPACITY)?;
    let spool_writer = SpoolWriter::open(
        Arc::clone(&spool),
        writer_inbox,
        spool_publisher,
        Arc::clone(&outbox_telemetry),
        WriterConfig::new(&config.cell_id, started, WRITER_POLL, WRITER_GROUP)?,
    )?;
    let session = spool_writer.session();
    // Before the first pass: the cell numbers orders from one at every start,
    // so the id of a decision chain is scoped by the session this writer just
    // committed, and the writer derives the same id for every record of it.
    cell.set_chain_epoch(session)?;
    // The writer's placement is settled on its own thread before it takes
    // a handoff, and reported back here before anything is served: pinned
    // when a core is named, and a node that cannot have the core it was
    // told to use stops rather than running on whatever the scheduler chose.
    let (placed, placement) = std::sync::mpsc::sync_channel::<Result<Placement>>(1);
    let writer_cpu = config.writer_cpu;
    std::thread::Builder::new()
        .name("qip-spool-writer".to_string())
        .spawn(move || {
            let outcome = match writer_cpu {
                Some(cpu) => pin_current_thread(cpu),
                None => current_placement(),
            };
            let pinned = outcome.is_ok() || writer_cpu.is_none();
            let _ = placed.send(outcome);
            if !pinned {
                return;
            }
            // Returns only once every sender has gone and the spool holds
            // what it received, or with the write the spool refused; either
            // way its heartbeat has stopped and the gauge halts new exposure.
            if let Err(error) = spool_writer.run() {
                eprintln!(
                    "qip-edge-node: the spool writer stopped: {}; new exposure halts on the \
                     stale heartbeat",
                    error.message()
                );
            }
        })
        .map_err(|error| Error::io(format!("cannot start the spool writer: {error}")))?;
    match placement.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(placement)) => println!(
            "qip-edge-node: spool writer thread {} {} (allowed {:?}, last ran on CPU {})",
            placement.tid,
            match config.writer_cpu {
                Some(cpu) => format!("pinned to CPU {cpu}"),
                None => format!(
                    "not pinned ({WRITER_CPU_VARIABLE} unset): the scheduler may run it on the \
                     decision thread's core"
                ),
            },
            placement.allowed,
            placement.last_cpu
        ),
        // Unpinned, a placement that cannot be read is said and not fatal:
        // nothing was asked of it.
        Ok(Err(error)) if config.writer_cpu.is_none() => eprintln!(
            "qip-edge-node: the spool writer's placement could not be read: {}",
            error.message()
        ),
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(Error::io(
                "the spool writer did not report its placement within five seconds",
            ));
        }
    }
    let relay = JournalRelay::spawn(
        from_mirror,
        to_writer,
        mirror,
        RelayConfig::new(
            WRITER_POLL,
            std::time::Duration::from_millis(100),
            std::time::Duration::from_secs(5),
        )?,
    )?;
    // What the lake may keep is the retention policy's (R12-DATA-1039): the
    // simulator's own depth by default, and every licensed dataset only as
    // QIP_TICK_RETENTION grants it. The codec is identity unless
    // QIP_TICK_LAKE_CODEC says otherwise (ADR 0113 §3). Both refuse a value
    // they cannot read, here, before the drain starts.
    let retention = qip_edge_node::event_fabric::drain::parse_retention(
        std::env::var(qip_edge_node::event_fabric::drain::RETENTION_VARIABLE)
            .ok()
            .as_deref(),
    )?;
    let lake_codec = qip_storage::lake_codec::SegmentCodec::parse(
        &std::env::var(qip_edge_node::event_fabric::drain::CODEC_VARIABLE).unwrap_or_default(),
    )?;
    println!(
        "qip-edge-node: tick lake keeps {} dataset(s) [{}], segments sealed {:?}",
        retention.len(),
        retention
            .iter()
            .map(|(dataset, right)| format!(
                "{dataset}:{}{}",
                if right.canonical { "canonical" } else { "none" },
                if right.raw { "+raw" } else { "" }
            ))
            .collect::<Vec<_>>()
            .join(", "),
        lake_codec
    );
    let mut sinks: Vec<Box<dyn Sink>> = vec![Box::new(LakeSink::new(
        config.storage.blobs(LAKE_NAMESPACE)?,
        &config.cell_id,
        retention,
        lake_codec,
    )?)];
    // The fabric link's issued certificate under `cas`, held for the life
    // of `run` — which never returns while the node runs — because dropping
    // it stops its renewal loop.
    let mut issued: Option<qip_transport::cas::CasIdentity> = None;
    match config.fabric.take() {
        Some(fabric) => {
            // ADR 0112: the fabric is a link between Compute Engine
            // processes, so it is mutual TLS with this node's workload
            // certificate unless the deployment is `local` and said
            // `plaintext`. No fallback: hand-placed files that will not load
            // stop the node here; under `cas` the certificate is requested
            // here and renewed on its own thread, off the decision thread,
            // and a CA that cannot be reached leaves the sink's handshakes
            // refused — outcomes wait in the spool — never a plaintext link.
            use qip_transport::tls::{LinkSecurity, TlsSource, WorkloadIdentity};
            let source = qip_transport::tls::link_source(config.deployment, &|name| {
                std::env::var(name).ok()
            })
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
            let link = match &source {
                Some(TlsSource::Files(paths)) => {
                    LinkSecurity::MutualTls(Arc::new(WorkloadIdentity::from_files(paths)?))
                }
                Some(TlsSource::Issued(settings)) => {
                    let started = qip_storage::gcp::workload_certificate::workload_certificate(
                        settings,
                        "qip-edge-node",
                        Arc::clone(&clock),
                    )?;
                    if let Some(error) = started.status().last_error() {
                        eprintln!(
                            "qip-edge-node: no workload certificate yet; the fabric link refuses \
                             until one arrives. The CA said: {error}"
                        );
                    }
                    let link = LinkSecurity::issued(started.identity());
                    issued = Some(started);
                    link
                }
                None => LinkSecurity::Plaintext,
            };
            println!(
                "qip-edge-node: fabric at {} over {}",
                fabric.url,
                link.describe()
            );
            sinks.push(Box::new(fabric.sink(
                &config.cell_id,
                &config.region,
                Arc::clone(&clock),
                &link,
            )?));
        }
        None => println!(
            "qip-edge-node: awaiting {FABRIC_URL_VARIABLE}: the spool drains to the tick lake \
             only, and the journal reaches the configured store through the relay; no fabric \
             partition receives this cell's outcomes"
        ),
    }
    let drain = Drain::open(
        Arc::clone(&spool),
        sinks,
        drain_publisher,
        Arc::clone(&outbox_telemetry),
        DrainConfig::new(
            std::time::Duration::from_millis(100),
            std::time::Duration::from_secs(1),
            64,
        )?,
    )?
    .spawn()?;
    // The writer beats on its first loop. Waiting for that beat here is
    // what keeps the cell's first turn from reading a never-beaten gauge as
    // exhausted and journaling a halt nobody caused; a writer that cannot
    // beat within the bound is refused at start, not discovered as a halt.
    let waited = std::time::Instant::now();
    while gauge.read().generation == 0 {
        if waited.elapsed() > std::time::Duration::from_secs(5) {
            return Err(Error::io(
                "the spool writer did not beat within five seconds of starting; a node whose \
                 journal writer is not running must not decide",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let outbox = Outbox {
        mirror: FabricMirror::new(&config.cell_id, to_relay.clone())?,
        inputs: RecordedInputs::new(to_relay, thresholds, Arc::clone(&outbox_telemetry)),
        gauge,
        session,
    };

    let gateway_seed = match &choice {
        VenueChoice::Simulated { seed } => *seed,
        // A node on the REST adapter runs no pass — the feed refused it at
        // start — so no marker ever carries this value.
        VenueChoice::Live(_) => 0,
    };
    let health_facts = HealthFacts {
        cell: config.cell_id.clone(),
        region: config.region.clone(),
        venues: config.venues.len(),
        halt_flag_path: config
            .halt_flag
            .as_ref()
            .map(|flag| flag.path().display().to_string()),
        storage_target: config.storage.target().as_str().to_string(),
        durable: config.storage.is_durable(),
        started_secs: started.as_secs(),
    };
    let certificate = issued.as_ref().map(qip_transport::cas::CasIdentity::status);
    let snapshot = HealthSnapshot::new(r#"{"turns":0}"#);
    let mut decisions = DecisionLoop::new(
        cell,
        gateway,
        outbox,
        journal_bound()?,
        Arc::clone(&outbox_telemetry),
        MarkerFacts {
            cell: config.cell_id.clone(),
            config_digest,
            gateway_seed,
            binary_version: env!("CARGO_PKG_VERSION").to_string(),
        },
    )?
    .with_threads(Some(relay), Some(drain))
    .with_health(snapshot.clone(), health_facts)
    .with_meter(PassMeter::new(Arc::clone(metrics), PASS_INTERVAL)?);
    if let Some(status) = certificate {
        decisions = decisions.with_certificate(status);
    }
    if let Some(feed) = feed {
        decisions = decisions.with_feed(feed, requoter);
    }
    if let Some(installer) = installer {
        decisions = decisions.with_desk(installer);
    }
    // The peer mesh (ADR 0116): the session and the wire run on threads of
    // their own, and the decision loop holds only the port, which offers and
    // never waits. This is the one node module that may name the link beyond
    // `qip_peer_link::boundary`, because it is the one that spawns it.
    match &config.peers {
        Some(settings) => {
            let (port, local) = qip_peer_link::spawn(settings, Arc::clone(&clock))?;
            println!(
                "qip-edge-node: peer mesh listening on {local} for {} peer(s) over {}; joins an \
                 epoch only with every counterparty's clock measured within {} ms",
                settings.peers.len(),
                settings.security.describe(),
                settings.clock_bound.as_millis()
            );
            decisions = decisions.with_peers(port);
        }
        None => println!(
            "qip-edge-node: no peer mesh ({LISTEN_VARIABLE} and {PEERS_VARIABLE} unset): this \
             cell coordinates with no other cell directly, and a cross-region cycle reaches it \
             only through the centre"
        ),
    }
    let plans = match config.plan_path.clone() {
        Some(path) => {
            let (sender, handoff) = ControlHandoff::bounded(PLAN_HANDOFF_CAPACITY)?;
            Some((PlanCompiler::spawn(path, sender)?, handoff))
        }
        None => None,
    };
    decisions = decisions.with_strategies(strategies, plans);
    if let Some(flag) = config.halt_flag.clone() {
        let poller = Poller::spawn(
            flag,
            config.region_wire.clone(),
            POLL_INTERVAL,
            POLL_BOUND,
            Arc::clone(&clock),
        )?;
        // One interval for the first reading, so the first turn applies a
        // reading rather than "not yet published", which reads as engaged.
        // A flag on a hung mount still starts the node — halted, which is
        // the answer for a wire whose state is unknown.
        let waited = std::time::Instant::now();
        while poller.latest().halt.read_at.is_none() && waited.elapsed() < POLL_BOUND {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        decisions = decisions.with_poller(poller);
    }
    match config.rollback.clone() {
        Some(request) => {
            println!(
                "qip-edge-node: policy rollback request at {} ({ROLLBACK_VARIABLE}; write the \
                 sequence to roll away from)",
                request.path().display()
            );
            decisions = decisions.with_rollback(RollbackWire::spawn(request, POLL_INTERVAL)?);
        }
        None => println!(
            "qip-edge-node: no policy rollback control ({ROLLBACK_VARIABLE} unset): the previous \
             payload stays resident and only the centre can move the cell off the current one"
        ),
    }
    if let Some(link) = link {
        decisions = decisions.with_mesh(MeshWorker::spawn(
            link,
            Some(mesh_series),
            Arc::clone(&clock),
            PASS_INTERVAL,
            MESH_CAPACITY,
        )?);
    }

    // One turn before the health surface binds, so the first probe reads a
    // body the decision thread wrote rather than a placeholder.
    decisions.turn(clock.now())?;
    let address = format!("0.0.0.0:{}", config.health_port);
    let listener = TcpListener::bind(&address)
        .map_err(|error| Error::io(format!("cannot bind {address}: {error}")))?;
    health::spawn(listener, Arc::clone(metrics), snapshot, REQUEST_TIMEOUT)?;
    println!(
        "qip-edge-node: health on {address}; a turn every {:?}",
        PASS_INTERVAL
    );

    // The decision thread. Paced by its own timer, so how often the cell
    // decides is a configured number rather than how often something asks
    // whether it is alive.
    loop {
        let began = std::time::Instant::now();
        if let Err(error) = decisions.turn(clock.now()) {
            // A turn that failed is in the journal where the cell refused;
            // the loop keeps turning so the wires and the flush keep running.
            eprintln!("qip-edge-node: a turn failed: {}", error.message());
        }
        if let Some(rest) = PASS_INTERVAL.checked_sub(began.elapsed()) {
            std::thread::sleep(rest);
        }
    }
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
    strategies: &StrategyInstaller,
    has_requoter: bool,
    has_lease: bool,
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
    if strategies.pricing().is_none() {
        // Unset is allowed and deploys nothing: the cell refuses an unpriced
        // deploy, and the alternative — defaulting to one of the two forms —
        // would have this node deciding whether to cross the spread on a
        // strategy nobody priced.
        missing.push(format!(
            "{PRICING_VARIABLE} for the plan's strategies: without it the plan the payload names \
             deploys nothing; set `{MARKETABLE}` or `{REST_AT_MID_PREFIX}<seconds>`"
        ));
    }
    if strategies.plan_path().is_none() {
        missing.push(format!(
            "{PLAN_VARIABLE} for the compiled plan: the payload names the plan by digest, and \
             without a file to read the digest names nothing this node can deploy"
        ));
    }
    if !has_requoter {
        // Unset is allowed and requotes nothing: a resting order stays at
        // the price it was sent until its time to live, however far the
        // touch moves. Said here because a cell whose orders never fill and
        // never move reads as a quiet market rather than a missing policy.
        missing.push(format!(
            "{REPRICE_VARIABLE} for the requote threshold: without it a resting order stays where \
             it was sent until its time to live however far the touch moves; set \
             `<tick>:<ticks>:<bps>`"
        ));
    }
    if !has_lease {
        // A node with no lease is the only owner of its session by
        // assumption, and a standby started beside it with no lease would
        // be a second owner; said here so nobody adds one without a lease.
        missing.push(format!(
            "{LEASE_VARIABLE} for a fenced standby: without it this node runs alone, a restart \
             is its only recovery, and a second node for this cell must not be started"
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

/// A literal fraction, refused by name if it does not parse rather than
/// unwrapped: a composition root never panics on its own constants.
fn fraction(literal: &str) -> Result<Decimal> {
    Decimal::parse(literal)
        .ok_or_else(|| Error::invalid(format!("the fraction {literal} does not parse")))
}
