//! The mesh backbone's central half, composed into the serving API.
//!
//! `qip_mesh::spine` built the centre's receiver and dispatcher and
//! `qip-edge-node` built the cell's uplink and downlink, and for a long time
//! the two met only in an acceptance test: a deployed cell published state
//! deltas at a peer where nothing listened, and no recall or reconciliation
//! halt could ever fire because no serving binary drained the inbox they
//! arrive on. This module is the missing composition root. The API serves the
//! wire, drains the deltas into `qip_kernel::Platform::ingest_cell_report`,
//! and dispatches the plane's capital envelopes back down over the durable
//! spool.
//!
//! # Why every cell gets its own listener
//!
//! Two facts of the transport force the topology, and both are worth knowing
//! before changing it:
//!
//! * A mesh poll is a destructive read with a single cursor. The inbox
//!   discards everything at or below the caller's acknowledged position, so
//!   two cells polling one capital inbox would discard each other's grants —
//!   the second cell simply never receives capital, silently.
//! * A publisher and a subscriber address a peer by base URL and then
//!   *replace* the path (`Url::with_path`), so a path prefix cannot carry the
//!   cell's identity, and nothing else on the wire does either.
//!
//! The address is therefore the identity: each configured cell gets one
//! listener at the address its `QIP_MESH_PEER` points at. On it, `publish`
//! feeds the shared delta inbox (deltas name their cell in the payload, so
//! sharing is safe) and `poll` reads that cell's own capital inbox (which is
//! exactly not safe to share). A second, unconfigurable listener per cell
//! binds a loopback port for the [`CapitalDispatcher`] to publish grants
//! into — the dispatcher speaks the real socket protocol like everything
//! else in this workspace, and keeping its ingress off the cell-facing
//! address means no remote peer can inject frames into a capital inbox.
//!
//! # The rhythm
//!
//! Nothing here runs a thread of its own beyond the listeners. The drain and
//! the dispatch ride the API's existing rhythm — `POST /cycle` — with
//! bounded work per pass: at most [`DRAIN_LIMIT`] deltas absorbed, and every
//! socket the dispatcher touches is a loopback with explicit connect, read
//! and write timeouts. A delta published between cycles waits in a bounded
//! inbox; when the inbox fills, the transport's own backpressure answers 503
//! and the cell's breaker backs off, which is the designed behaviour of the
//! wire and not an error in it.
//!
//! # Idempotency, in layers
//!
//! Delivery is at-least-once in both directions, so every layer here absorbs
//! a duplicate rather than trusting the one below to have caught it: the
//! inbox recognises a retried delta within its dedup window, the receiver
//! remembers absorbed keys past that window, and a grant is dispatched once
//! per [`qip_mesh::spine::grant_key`] with the cell's own applied-grant
//! memory as the final backstop. The integration tests prove the property at
//! this seam — a delta delivered twice is one ingestion.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{
    GrantManifest, HaltCommand, PolicyPayload, RiskEnvelopeSnapshot, Slot,
};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, hash};
use qip_events::AnyEvent;
use qip_kernel::Platform;
use qip_kernel::central::{BreakOrigin, CellReport, ReconciliationBreak, RegionMembership};
use qip_mesh::delta::{CellStanding, decode_cell_delta};
use qip_mesh::spine::{
    CapitalDispatch, CapitalDispatcher, CellDeltaReceiver, CellDeltaSink, DispatcherConfig,
    PolicyCourier, PolicySend, ReceiverStats, grant_key,
};
use qip_storage::kv::KeyValueStore;
use qip_transport::mesh::{HEALTH_PATH, POLL_PATH, PUBLISH_PATH};
use qip_transport::retry::ThreadSleeper;
use qip_transport::spool::DurableDeadLetters;
use qip_transport::{ClientLimits, InboxHealth, MeshConfig, MeshEndpoint, MeshInbox, RetryPolicy};
use serde::Serialize;

use crate::cells::CellRegistry;
use crate::http::{Handler, Request, Response, Server, ServerLimits};

/// The gate. Set it to `cell=host:port[,cell=host:port…]` and the mesh is
/// served; leave it unset and no mesh route exists in this process. Each
/// address is the value that cell's `QIP_MESH_PEER` must carry.
pub const CELLS_VARIABLE: &str = "QIP_MESH_CELLS";
/// Frames a mesh inbox holds before the transport's backpressure answers 503.
pub const INBOX_CAPACITY_VARIABLE: &str = "QIP_MESH_INBOX_CAPACITY";
/// Undelivered capital envelopes one cell's durable spool may hold.
pub const SPOOL_CAPACITY_VARIABLE: &str = "QIP_MESH_SPOOL_CAPACITY";
/// The region membership the centre partitions grants by (ADR 0039):
/// `region=grant:cell[,cell…][;region=…]`, every served cell filed under
/// exactly one region. Unset, the `capital_grants` slot ships every live
/// grant to every cell — the one-cell-per-region shape ADR 0039 exists to
/// grow out of — and the cycle says so beside each payload.
pub const REGIONS_VARIABLE: &str = "QIP_MESH_REGIONS";

/// Deltas absorbed per cycle. A bound because the drain runs inside a
/// request: a backlog is worked off across cycles rather than stalling one
/// request for as long as the backlog is deep.
pub const DRAIN_LIMIT: usize = 256;

/// Dispatched grant identities remembered per cell. Past the bound the
/// oldest is forgotten and a still-live envelope would be dispatched again —
/// which the spool, the inbox window and the cell's own applied-grant memory
/// each absorb as the duplicate it is.
const DISPATCH_MEMORY: usize = 4_096;

fn default_inbox_capacity() -> usize {
    1_024
}

fn default_spool_capacity() -> usize {
    1_024
}

// --- configuration ------------------------------------------------------

/// One cell and the address this process serves it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellAddress {
    pub cell: String,
    pub address: String,
}

/// What the environment says the mesh should be.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshSettings {
    pub cells: Vec<CellAddress>,
    pub inbox_capacity: usize,
    pub spool_capacity: usize,
    /// Which region each served cell belongs to and what the region is
    /// granted, or `None` when the deployment declared none. Validated at
    /// parse time to cover every cell in `cells`: a cell the centre serves
    /// but files nowhere would be withheld every share for as long as the
    /// process ran, silently.
    pub regions: Option<RegionMembership>,
}

impl MeshSettings {
    /// Read the mesh configuration, or `None` when this process serves no
    /// mesh. Absence is not a misconfiguration: an API can honestly run
    /// without being anyone's peer, and the banner says so out loud.
    pub fn from_env() -> Result<Option<Self>> {
        let Some(cells) = std::env::var(CELLS_VARIABLE)
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        Self::parse(
            &cells,
            std::env::var(INBOX_CAPACITY_VARIABLE).ok().as_deref(),
            std::env::var(SPOOL_CAPACITY_VARIABLE).ok().as_deref(),
            std::env::var(REGIONS_VARIABLE)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .as_deref(),
        )
        .map(Some)
    }

    /// The parse behind [`Self::from_env`], separated so a test can hand it
    /// strings without touching the process environment.
    pub fn parse(
        cells: &str,
        inbox_capacity: Option<&str>,
        spool_capacity: Option<&str>,
        regions: Option<&str>,
    ) -> Result<Self> {
        let mut parsed = Vec::new();
        let mut names = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        for entry in cells.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let Some((cell, address)) = entry.split_once('=') else {
                return Err(Error::invalid(format!(
                    "configuration: {CELLS_VARIABLE} entry `{entry}` is not `cell=host:port`; \
                     the address is the cell's identity on this transport, so it cannot be \
                     left to a default"
                )));
            };
            let cell = cell.trim();
            let address = address.trim();
            if cell.is_empty() || address.is_empty() {
                return Err(Error::invalid(format!(
                    "configuration: {CELLS_VARIABLE} entry `{entry}` names an empty cell or \
                     address"
                )));
            }
            if !names.insert(cell.to_string()) {
                return Err(Error::invalid(format!(
                    "configuration: {CELLS_VARIABLE} names {cell} twice; two lanes for one \
                     cell would each claim the same capital spool"
                )));
            }
            if !addresses.insert(address.to_string()) {
                return Err(Error::invalid(format!(
                    "configuration: {CELLS_VARIABLE} reuses {address}; the address is the \
                     cell's identity, so two cells on one address are one cell with two names"
                )));
            }
            parsed.push(CellAddress {
                cell: cell.to_string(),
                address: address.to_string(),
            });
        }
        if parsed.is_empty() {
            return Err(Error::invalid(format!(
                "configuration: {CELLS_VARIABLE} is set and names no cell; unset it to serve \
                 no mesh, which is a decision, rather than an empty list, which is a typo"
            )));
        }
        let regions = match regions {
            None => None,
            Some(declaration) => {
                let membership = RegionMembership::parse(declaration).map_err(|error| {
                    Error::invalid(format!(
                        "configuration: {REGIONS_VARIABLE}: {}",
                        error.message()
                    ))
                })?;
                membership
                    .covering(parsed.iter().map(|cell| cell.cell.as_str()))
                    .map_err(|error| {
                        Error::invalid(format!(
                            "configuration: {REGIONS_VARIABLE} against {CELLS_VARIABLE}: {}",
                            error.message()
                        ))
                    })?;
                Some(membership)
            }
        };
        Ok(Self {
            cells: parsed,
            inbox_capacity: parse_capacity(INBOX_CAPACITY_VARIABLE, inbox_capacity)?
                .unwrap_or_else(default_inbox_capacity),
            spool_capacity: parse_capacity(SPOOL_CAPACITY_VARIABLE, spool_capacity)?
                .unwrap_or_else(default_spool_capacity),
            regions,
        })
    }
}

fn parse_capacity(variable: &str, value: Option<&str>) -> Result<Option<usize>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed: usize = value.trim().parse().map_err(|_| {
        Error::invalid(format!(
            "configuration: {variable} is not a number: {value}"
        ))
    })?;
    if parsed == 0 {
        return Err(Error::invalid(format!(
            "configuration: {variable} is zero, which does not disable the mesh — unset \
             {CELLS_VARIABLE} for that — it refuses every message while looking configured"
        )));
    }
    Ok(Some(parsed))
}

/// A stable per-cell seed from the cell's own name, mirroring the edge
/// node's derivation: nine dispatchers that all drew the same jitter would
/// retry in lockstep, and a configured seed per cell would be nine settings
/// nobody rotates.
fn seed_from(cell: &str) -> u64 {
    let digest = hash::sha256_hex(cell.as_bytes());
    u64::from_str_radix(digest.get(..16).unwrap_or("0"), 16).unwrap_or(0)
}

/// Client limits for the dispatcher's loopback publishes. Tight because the
/// peer is this process: a send that needs longer than this is not a slow
/// network, it is this process wedged, and the spool keeps the envelope.
fn dispatch_limits() -> ClientLimits {
    ClientLimits {
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_secs(2),
        write_timeout: StdDuration::from_secs(1),
        ..ClientLimits::default()
    }
}

/// A short ladder for the same reason: the entry is persisted before the
/// first attempt, so giving up quickly costs a redelivery on a later cycle
/// rather than a lost instruction — while a long ladder would hold the
/// `POST /cycle` request that drives it.
fn dispatch_retry() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(400),
        multiplier: 2,
        jitter_basis_points: 2_500,
    }
}

fn mesh_server_limits() -> ServerLimits {
    ServerLimits {
        max_body: 1024 * 1024,
        read_timeout: StdDuration::from_secs(5),
        write_timeout: StdDuration::from_secs(5),
        max_concurrent: 16,
        ..ServerLimits::default()
    }
}

// --- the wire, served ---------------------------------------------------

/// The handler on a cell's configured address.
///
/// `publish` lands in the shared delta inbox — a delta names its cell in the
/// payload, so one inbox serves all publishers and the receiver files each
/// frame under the cell it claims. `poll` reads this cell's own capital
/// inbox, which is single-consumer by the transport's cursor semantics and
/// is why this handler exists per cell rather than once.
#[derive(Debug)]
struct CellFacing {
    cell: String,
    deltas: MeshEndpoint,
    capital: MeshEndpoint,
}

impl Handler for CellFacing {
    fn handle(&self, request: &Request) -> Response {
        let Some(method) = qip_transport::Method::parse(request.method.as_str()) else {
            return Response::json(405, r#"{"error":"the mesh endpoint knows no such method"}"#);
        };
        match request.path.as_str() {
            PUBLISH_PATH => wire(self.deltas.handle(method, PUBLISH_PATH, &request.body)),
            POLL_PATH => wire(self.capital.handle(method, POLL_PATH, &request.body)),
            // Composed rather than one inbox's health, because a probe of
            // this address is asking about the lane and the lane is both
            // directions.
            HEALTH_PATH => Response::json(
                200,
                format!(
                    r#"{{"cell":{},"deltas":{},"capital":{}}}"#,
                    crate::json::string(&self.cell),
                    health_json(&self.deltas.inbox().health()),
                    health_json(&self.capital.inbox().health()),
                ),
            ),
            other => Response::json(
                404,
                format!(r#"{{"error":"{other} is not a mesh endpoint"}}"#),
            ),
        }
    }
}

/// The handler on a cell's loopback feed address, where the dispatcher
/// publishes grants into that cell's capital inbox. It answers `publish` and
/// nothing else: this address exists so capital ingress is not reachable
/// from the cell-facing one, and serving anything more here would erode
/// exactly that.
#[derive(Debug)]
struct CapitalFeed {
    capital: MeshEndpoint,
}

impl Handler for CapitalFeed {
    fn handle(&self, request: &Request) -> Response {
        let Some(method) = qip_transport::Method::parse(request.method.as_str()) else {
            return Response::json(405, r#"{"error":"the mesh endpoint knows no such method"}"#);
        };
        match request.path.as_str() {
            PUBLISH_PATH => wire(self.capital.handle(method, PUBLISH_PATH, &request.body)),
            other => Response::json(
                404,
                format!(
                    r#"{{"error":"{other} is not served here; this address only feeds one \
                     cell's capital inbox"}}"#
                ),
            ),
        }
    }
}

fn wire(answer: qip_transport::EndpointResponse) -> Response {
    Response::new(
        answer.status,
        qip_transport::EndpointResponse::CONTENT_TYPE,
        answer.body,
    )
}

fn health_json(health: &InboxHealth) -> String {
    serde_json::to_string(health).unwrap_or_else(|_| "null".to_string())
}

/// One bound listener, kept so the thread's server is owned by something and
/// the banner can say where the mesh lives.
#[derive(Debug)]
pub struct MeshListener {
    pub role: &'static str,
    pub cell: String,
    pub address: String,
    shutdown: Arc<AtomicBool>,
}

impl Drop for MeshListener {
    fn drop(&mut self) {
        // A request, not a join: the accept loop notices between connections.
        // The process does not wait for it, exactly as the CLI's loopback
        // peers document.
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

fn spawn_listener(
    role: &'static str,
    cell: &str,
    address: &str,
    handler: Arc<dyn Handler>,
) -> Result<MeshListener> {
    let server = Arc::new(Server::bind(address, handler, mesh_server_limits())?);
    let bound = server.local_address()?;
    let shutdown = server.shutdown_handle();
    let serving = Arc::clone(&server);
    std::thread::Builder::new()
        .name(format!("qip-mesh-{role}-{cell}"))
        .spawn(move || {
            // A serving error is the listener going away, which is what
            // shutdown looks like from inside the loop; the visible failure
            // is a peer not getting an answer, which the peer's breaker and
            // dead letters already report better than a message here could.
            let _ = serving.serve();
        })
        .map_err(|error| Error::io(format!("cannot start the {role} listener: {error}")))?;
    Ok(MeshListener {
        role,
        cell: cell.to_string(),
        address: bound,
        shutdown,
    })
}

// --- the backbone -------------------------------------------------------

/// The dispatch path to one cell.
#[derive(Debug)]
struct CapitalLane {
    /// Where the cell polls, for the banner and the status page.
    address: String,
    /// Another handle on the inbox the listeners serve, for the status page
    /// — a clone shares the same state, which is the transport's own design
    /// for exactly this split.
    capital: MeshEndpoint,
    dispatcher: CapitalDispatcher,
    /// The policy sender for this cell. Unspooled where the dispatcher
    /// spools, because policy is state and the newest wins; the courier's own
    /// doc comment carries the argument.
    courier: PolicyCourier,
    /// Grant identities already handed to the dispatcher, so a cycle that
    /// sees the same live envelope again does not push it into the spool
    /// again. Bounded; see [`DISPATCH_MEMORY`].
    dispatched: BTreeSet<String>,
    dispatched_order: VecDeque<String>,
}

impl CapitalLane {
    fn remember(&mut self, key: String) {
        self.dispatched.insert(key.clone());
        self.dispatched_order.push_back(key);
        while self.dispatched_order.len() > DISPATCH_MEMORY {
            if let Some(oldest) = self.dispatched_order.pop_front() {
                self.dispatched.remove(&oldest);
            }
        }
    }
}

/// Counters the receiver does not already keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BackboneCounters {
    /// Cell reports built from deltas and absorbed by the platform.
    pub reports_ingested: u64,
    /// Frames on the delta topic that did not decode as a delta. Skipped
    /// rather than halting the drain — see the sink for why the two failure
    /// classes part ways.
    pub undecodable: u64,
    /// Orders reported *sent* across all deltas — the incremental half,
    /// summed, which is the arithmetic that half is for. Not fills: the
    /// two are separate counts so the status surface cannot restate the
    /// defect where one was read as the other.
    pub orders_reported: u64,
    /// Fills reported across all deltas — what the centre billed.
    pub fills_reported: u64,
    /// Fills the cells said they could not fit on the wire. Each is a
    /// trade the centre never billed, which is why it is a counter of its
    /// own rather than folded into the one above.
    pub fills_omitted: u64,
    /// Refusals reported across all deltas, counting the ones each delta
    /// said it truncated.
    pub refusals_reported: u64,
    /// Ingestions that halted the reporting cell.
    pub cell_halts: u64,
    /// Recall orders the plane issued while ingesting.
    pub recalls_issued: u64,
    /// Envelopes the cells acknowledged.
    pub envelopes_dispatched: u64,
    /// Dispatch outcomes still waiting in a spool.
    pub envelopes_held: u64,
    /// Envelopes a cell answered and refused, now in the dead letters.
    pub envelopes_rejected: u64,
    /// Envelopes the plane holds for cells this process does not serve. A
    /// non-zero value is a configuration gap an operator has to see.
    pub envelopes_unserved: u64,
}

/// The last standing each cell reported, kept for the status surface. The
/// platform holds the aggregate; this holds the per-cell facts the kernel's
/// `CellReport` cannot carry — the halt flag above all.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CellStandingSummary {
    pub cell: String,
    pub region: String,
    pub sequence: u64,
    pub at: Timestamp,
    /// Whether the cell reports having stopped itself.
    pub halted: bool,
    pub strategies: usize,
    pub reconciliation_breaks: usize,
    pub reconciliation_breaks_omitted: u32,
}

/// What one drain pass did, for the cycle response.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DrainSummary {
    pub absorbed: usize,
    pub duplicates: usize,
    pub ignored: usize,
    pub corrupt: usize,
    pub undecodable: usize,
    /// Cells halted by this pass's ingestions.
    pub halted_cells: Vec<String>,
    pub recalls: usize,
    /// Set when a sink refusal stopped the drain; the frame is re-offered
    /// next cycle.
    pub halted_at: Option<String>,
    /// Frames the inbox still holds, including the ones just absorbed until
    /// the next read acknowledges them.
    pub inbox_depth: usize,
}

/// What one dispatch pass did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DispatchSummary {
    pub delivered: usize,
    pub held: usize,
    pub rejected: usize,
    /// Grants not dispatched this pass because older spooled entries for the
    /// same cell are still undelivered: capital instructions to one cell are
    /// ordered, and a fresh grant must not overtake a held one.
    pub deferred: usize,
    /// Cells the plane holds envelopes for and this process does not serve.
    pub unserved_cells: Vec<String>,
    /// Spooled entries re-sent by recovery, delivered or not.
    pub recovered: usize,
}

/// Everything the status surface says about the mesh, serialisable so the
/// route and the banner cannot disagree with the internals they describe.
#[derive(Clone, Debug, Serialize)]
pub struct MeshStatus {
    pub served: bool,
    pub delta_inbox: InboxHealth,
    pub receiver: ReceiverStats,
    pub counters: BackboneCounters,
    pub cells: Vec<MeshCellStatus>,
    pub standings: Vec<CellStandingSummary>,
    /// The most recent frame on the delta topic that would not decode, kept
    /// because a counter says something is wrong and this says what.
    pub last_undecodable: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MeshCellStatus {
    pub cell: String,
    pub address: String,
    pub capital_inbox: InboxHealth,
    /// Envelopes persisted and not yet acknowledged by the cell.
    pub spool_pending: usize,
    pub circuit: String,
}

/// A capital envelope the plane holds, snapshotted for dispatch after the
/// platform lock is released — the dispatcher's sockets must not run under
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingGrant {
    pub cell: String,
    pub envelope: CapitalEnvelope,
}

/// Every live envelope the central plane currently holds.
///
/// Enumerated the same way the `/capital` route enumerates them, so the
/// grants an operator reads there and the grants this process dispatches
/// cannot be two different lists. Expired envelopes are skipped: an expired
/// envelope admits nothing whatever the cell does, so sending one would be
/// traffic with no authority in it.
pub fn pending_capital(platform: &Platform, now: Timestamp) -> Vec<PendingGrant> {
    let central = platform.central();
    central
        .factory()
        .candidates()
        .filter_map(|candidate| {
            let envelope = central.envelope(candidate.cell(), candidate.strategy())?;
            envelope.is_live(now).then(|| PendingGrant {
                cell: candidate.cell().to_string(),
                envelope: envelope.clone(),
            })
        })
        .collect()
}

/// One cycle's policy payloads, and the whitelist lines an operator reads.
///
/// The lines travel beside the payloads rather than being printed inside
/// [`pending_policy`], because that function runs under the platform lock
/// and its caller decides where the account goes — stderr and the cycle
/// response, today — so the one place a whitelist is described is the one
/// place it is shipped from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PendingPolicy {
    pub payloads: Vec<(String, PolicyPayload)>,
    /// One line per cell: what its cycle whitelist carries and why, or why
    /// the slot ships unproduced. `WhitelistIssue::describe` for an issue,
    /// the producer's refusal for a refusal.
    pub whitelist: Vec<String>,
    /// One line per cell: what its `capital_grants` slot carries — a share
    /// of its region's grant, every live grant because no membership is
    /// declared, or why it was withheld (ADR 0039). Beside the whitelist
    /// lines for the same reason they exist: a node that opens unfunded and
    /// never funds is explained where the share was, or was not, shipped
    /// from.
    pub shares: Vec<String>,
}

/// The policy payloads one cycle should ship, one per configured cell.
///
/// Built from what the platform actually has, which today is three of the
/// twelve items: the grant manifest — the signatures of every live envelope
/// for the cell, so a dropped grant becomes visible — the risk envelope, as
/// the limit set the monitor really enforces, and the cycle whitelist, as
/// [`Platform::issue_cycle_whitelist`] produces and journals it. Every other
/// slot ships unproduced and reads as unavailable at the cell, which narrows
/// it; that is the fail-closed design, not an omission. The halted flag
/// mirrors the central kill switch, so a cell that missed the halt broadcast
/// converges at the next payload.
///
/// `&mut` because the whitelist is journaled as it is issued: a whitelist
/// that reached a cell with no record at the centre would be a permission
/// reproducible from nothing. The caller already holds the platform's lock
/// mutably for the drain that precedes this, so the borrow costs nothing it
/// was not already paying.
///
/// The whitelist ships as the producer returned it — empty included, and an
/// empty whitelist is what an unset policy or a missing grant produces,
/// which the cell's installer declines by name. A *refusal* from the
/// producer — a policy venue the desk's grant does not permit, a grant that
/// permits no order — ships the slot unproduced and says why in
/// [`PendingPolicy::whitelist`]: the cell narrows as an unavailable slot
/// narrows it, and never receives a whitelist the centre had to guess at.
///
/// The sequence is the issue instant in nanoseconds: strictly increasing
/// under a monotonic clock, and it survives a restart without persisted
/// state. The assumption that the centre's clock does not step backwards is
/// stated here rather than hidden; a cell refuses a regression either way.
///
/// What an operator sees if that assumption breaks: every payload issued
/// after a backward step carries a sequence at or below the last applied,
/// every cell refuses it, and the cells **narrow to their conservative floor**
/// as their slots age out — smaller sizing, never larger. The failure costs
/// availability and never safety, which is the direction every clock fault
/// here is designed to fall. The symptom in the delta stream is policy
/// refusals climbing with the narrowed set widening; the repair is a fresh
/// payload once the clock is ahead of the last applied instant.
pub fn pending_policy(
    platform: &mut Platform,
    cells: impl Iterator<Item = String>,
    regions: Option<&RegionMembership>,
    now: Timestamp,
) -> PendingPolicy {
    let halted = platform.autonomy().kill_switch().is_globally_tripped();
    let limits = serde_json::to_value(platform.risk_limits()).ok();
    let cells: Vec<String> = cells.collect();
    // With a membership declared, the centre decides every cell's share of
    // its region's grant from the plan it sizes under the same drawdown the
    // envelopes were issued against (ADR 0039); a cell whose share was
    // withheld ships the slot unproduced and the reason travels beside it.
    // Without one, every live grant ships to every cell — the shape the ADR
    // grows out of, said out loud rather than defaulted silently, because
    // two nodes under one grant could each spend it.
    let manifests = regions.map(|membership| {
        platform.central().grant_manifests(
            cells.iter().map(String::as_str),
            membership,
            platform.drawdown(),
            now,
        )
    });
    let mut pending = PendingPolicy::default();
    for cell in cells {
        let sequence = now.as_nanos().max(0) as u64;
        let mut payload = PolicyPayload::unproduced(sequence, &cell, now);
        payload.halted = halted;
        match manifests
            .as_ref()
            .and_then(|decided| decided.for_cell(&cell))
        {
            Some(decision) => {
                pending.shares.push(decision.describe(&cell));
                if let Some(manifest) = decision.manifest() {
                    payload.capital_grants = Slot::produced(manifest, now);
                }
            }
            None => {
                let live_grants: Vec<String> = {
                    let central = platform.central();
                    central
                        .factory()
                        .candidates()
                        .filter(|candidate| candidate.cell() == cell)
                        .filter_map(|candidate| {
                            central.envelope(candidate.cell(), candidate.strategy())
                        })
                        .filter(|envelope| envelope.is_live(now))
                        .map(|envelope| envelope.signature().to_string())
                        .collect()
                };
                pending.shares.push(format!(
                    "region share for {cell}: no {REGIONS_VARIABLE} declared, every live grant \
                     shipped ({} grant(s)); a second cell under one of these grants could spend \
                     it too (ADR 0039)",
                    live_grants.len()
                ));
                payload.capital_grants = Slot::produced(GrantManifest { live_grants }, now);
            }
        }
        if let Some(limits) = limits.clone() {
            payload.risk_envelope = Slot::produced(RiskEnvelopeSnapshot { limits }, now);
        }
        match platform.issue_cycle_whitelist(&cell, now) {
            Ok(issue) => {
                pending.whitelist.push(issue.describe());
                payload.cycle_whitelist = Slot::produced(issue.whitelist, now);
            }
            Err(error) => pending.whitelist.push(format!(
                "cycle whitelist for {cell}: not shipped, {}",
                error.message()
            )),
        }
        pending.payloads.push((cell, payload));
    }
    pending
}

/// The centre's half of the mesh, assembled and serving.
pub struct MeshBackbone {
    receiver: CellDeltaReceiver,
    /// The trust root policy payloads and halts are signed with — the same
    /// key the cells verify grants against. `None` means policy distribution
    /// is off and counted, never silently unsigned: an unsigned payload would
    /// be refused by every cell anyway, and signing with a made-up key would
    /// manufacture a second trust root.
    policy_key: Option<Vec<u8>>,
    lanes: BTreeMap<String, CapitalLane>,
    listeners: Vec<MeshListener>,
    counters: BackboneCounters,
    standings: BTreeMap<String, CellStandingSummary>,
    last_undecodable: Option<String>,
    /// The region membership the deployment declared, carried here so the
    /// cycle that builds payloads reads the same one the settings validated.
    regions: Option<RegionMembership>,
}

// The key is deliberately not in the `Debug` output, matching
// `qip_capital::envelope::EnvelopeIssuer`: a struct that prints its own
// secret gets it into a log the first time anything derives `Debug` on a type
// that contains it, and this is the same trust root `QIP_CAPITAL_ENVELOPE_KEY`
// installs — a `{:?}` on this backbone (a panic message, an error wrapper, a
// `dbg!()` added while debugging a stuck cell) must not become the channel
// that puts it in one.
impl std::fmt::Debug for MeshBackbone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshBackbone")
            .field(
                "policy_key",
                &self.policy_key.as_ref().map(|_| "<redacted>"),
            )
            .field("cells", &self.lanes.len())
            .field("listeners", &self.listeners.len())
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

impl MeshBackbone {
    /// Bind every listener, open every spool, and start serving.
    ///
    /// Serving begins here but consuming does not: deltas queue in the inbox
    /// until the first `POST /cycle` drains them, and a spool left by a
    /// previous process goes out on that same first cycle. Start-up stays
    /// fast and cannot wedge on a peer, which is what the spine's own
    /// open-versus-recover split is for.
    pub fn open(
        settings: &MeshSettings,
        store: Arc<dyn KeyValueStore>,
        clock: Arc<dyn Clock>,
        policy_key: Option<Vec<u8>>,
    ) -> Result<Self> {
        let receiver = CellDeltaReceiver::with_defaults("central", settings.inbox_capacity)?;
        let mut lanes = BTreeMap::new();
        let mut listeners = Vec::new();

        for cell in &settings.cells {
            let capital = MeshEndpoint::new(MeshInbox::new(
                format!("capital:{}", cell.cell),
                settings.inbox_capacity,
                settings.inbox_capacity,
            )?);

            let facing = spawn_listener(
                "cells",
                &cell.cell,
                &cell.address,
                Arc::new(CellFacing {
                    cell: cell.cell.clone(),
                    deltas: receiver.endpoint().clone(),
                    capital: capital.clone(),
                }),
            )?;
            // Loopback and port zero, deliberately: nothing configures this
            // address, nothing remote can reach it, and the dispatcher reads
            // it back from the bound socket.
            let feed = spawn_listener(
                "capital-feed",
                &cell.cell,
                "127.0.0.1:0",
                Arc::new(CapitalFeed {
                    capital: capital.clone(),
                }),
            )?;

            let dispatcher = CapitalDispatcher::open(
                DispatcherConfig::new(
                    &cell.cell,
                    MeshConfig::new(
                        format!("capital:{}", cell.cell),
                        format!("http://{}", feed.address),
                    )
                    .with_retry(dispatch_retry())
                    .with_limits(dispatch_limits())
                    .with_seed(seed_from(&cell.cell)),
                )
                .with_spool_capacity(settings.spool_capacity),
                Arc::clone(&store),
                Arc::clone(&clock),
                Arc::new(ThreadSleeper),
                // Durable, because a rejected capital envelope is a record an
                // operator reads after a restart, and the publisher's own
                // requirements name exactly this wiring as the deployment's
                // job.
                Box::new(DurableDeadLetters::open(
                    Arc::clone(&store),
                    format!("capital:{}", cell.cell),
                )?),
            )?;

            let courier = PolicyCourier::open(
                DispatcherConfig::new(
                    &cell.cell,
                    MeshConfig::new(
                        format!("policy:{}", cell.cell),
                        format!("http://{}", feed.address),
                    )
                    .with_retry(dispatch_retry())
                    .with_limits(dispatch_limits())
                    .with_seed(seed_from(&cell.cell).wrapping_add(1)),
                ),
                Arc::clone(&clock),
                Arc::new(ThreadSleeper),
                Box::new(DurableDeadLetters::open(
                    Arc::clone(&store),
                    format!("policy:{}", cell.cell),
                )?),
            )?;

            lanes.insert(
                cell.cell.clone(),
                CapitalLane {
                    address: facing.address.clone(),
                    capital,
                    dispatcher,
                    courier,
                    dispatched: BTreeSet::new(),
                    dispatched_order: VecDeque::new(),
                },
            );
            listeners.push(facing);
            listeners.push(feed);
        }

        Ok(Self {
            receiver,
            policy_key,
            lanes,
            listeners,
            counters: BackboneCounters::default(),
            standings: BTreeMap::new(),
            last_undecodable: None,
            regions: settings.regions.clone(),
        })
    }

    /// The cells this backbone serves, for building payloads.
    pub fn cells(&self) -> impl Iterator<Item = String> + '_ {
        self.lanes.keys().cloned()
    }

    /// The region membership the deployment declared, for the payload builder.
    pub fn regions(&self) -> Option<&RegionMembership> {
        self.regions.as_ref()
    }

    /// Sign and send one cycle's policy payloads.
    ///
    /// With no trust root configured nothing is sent and the count says so —
    /// an unsigned payload would be refused by every cell, and signing with a
    /// manufactured key would mint a second trust root.
    pub fn dispatch_policy(&mut self, pending: PendingPolicy, now: Timestamp) -> PolicySummary {
        let mut summary = PolicySummary {
            whitelist: pending.whitelist,
            shares: pending.shares,
            ..PolicySummary::default()
        };
        let Some(key) = self.policy_key.clone() else {
            summary.unsigned = pending.payloads.len();
            return summary;
        };
        for (cell, payload) in pending.payloads {
            let Some(lane) = self.lanes.get_mut(&cell) else {
                summary.unserved += 1;
                continue;
            };
            let signed = match payload.signed(&key) {
                Ok(signed) => signed,
                Err(error) => {
                    summary.errors.push(format!("{cell}: {}", error.message()));
                    continue;
                }
            };
            match lane.courier.send_payload(signed, now) {
                Ok(PolicySend::Delivered) => summary.sent += 1,
                Ok(PolicySend::CircuitOpen) => summary.circuit_open += 1,
                Err(error) => summary.errors.push(format!("{cell}: {}", error.message())),
            }
        }
        summary
    }

    /// Sign and broadcast a halt to every cell.
    ///
    /// Best-effort by design and honest about it: the guarantee is that a
    /// cell which can hear the centre halts within one poll, and the payload's
    /// own halted flag re-carries the state for any cell that missed this.
    pub fn broadcast_halt(&mut self, reason: &str, now: Timestamp) -> PolicySummary {
        let mut summary = PolicySummary::default();
        let Some(key) = self.policy_key.clone() else {
            summary.unsigned = self.lanes.len();
            return summary;
        };
        for (cell, lane) in &mut self.lanes {
            let command = match HaltCommand::new(cell.clone(), now, reason).signed(&key) {
                Ok(signed) => signed,
                Err(error) => {
                    summary.errors.push(format!("{cell}: {}", error.message()));
                    continue;
                }
            };
            match lane.courier.send_halt(command, now) {
                Ok(PolicySend::Delivered) => summary.sent += 1,
                Ok(PolicySend::CircuitOpen) => summary.circuit_open += 1,
                Err(error) => summary.errors.push(format!("{cell}: {}", error.message())),
            }
        }
        summary
    }

    /// The bound listeners, for the banner.
    pub fn listeners(&self) -> &[MeshListener] {
        &self.listeners
    }

    /// The address one cell's `QIP_MESH_PEER` must name, once bound.
    pub fn cell_address(&self, cell: &str) -> Option<&str> {
        self.lanes.get(cell).map(|lane| lane.address.as_str())
    }

    pub const fn counters(&self) -> BackboneCounters {
        self.counters
    }

    /// Drain the delta inbox into the platform, one bounded pass.
    ///
    /// Runs under the platform lock because ingestion writes the central
    /// plane; the pass is bounded by [`DRAIN_LIMIT`] so the lock is held for
    /// work proportional to what arrived, never to what could.
    pub fn drain_into(
        &mut self,
        platform: &mut Platform,
        cells: &CellRegistry,
        now: Timestamp,
    ) -> Result<DrainSummary> {
        let Self {
            receiver,
            counters,
            standings,
            last_undecodable,
            ..
        } = self;
        let mut sink = IngestSink {
            platform,
            cells,
            now,
            counters,
            standings,
            last_undecodable,
            undecodable: 0,
            halted_cells: Vec::new(),
            recalls: 0,
        };
        let report = receiver.drain(now, DRAIN_LIMIT, &mut sink)?;
        let summary = DrainSummary {
            // What the platform absorbed, not what the receiver handed over:
            // the receiver counts undecodable frames as absorbed because the
            // sink accepted them, and this summary is read as ingestions.
            absorbed: report.absorbed - sink.undecodable,
            duplicates: report.duplicates.len(),
            ignored: report.ignored,
            corrupt: report.corrupt.len(),
            undecodable: sink.undecodable,
            halted_cells: sink.halted_cells,
            recalls: sink.recalls,
            halted_at: report.halted.map(|halt| halt.reason),
            inbox_depth: report.remaining,
        };
        Ok(summary)
    }

    /// Send the plane's envelopes down, one bounded pass.
    ///
    /// Recovery first, then new grants, and never a new grant past a held
    /// one: the spool is FIFO because capital instructions to one cell are
    /// ordered, and dispatching fresh envelopes while older ones are held
    /// would deliver them out of that order the moment the peer recovered.
    pub fn dispatch(&mut self, pending: Vec<PendingGrant>, now: Timestamp) -> DispatchSummary {
        let mut summary = DispatchSummary::default();

        for lane in self.lanes.values_mut() {
            match lane.dispatcher.recover(now) {
                Ok(recovery) => {
                    summary.recovered += recovery.outcomes.len();
                    for outcome in &recovery.outcomes {
                        Self::count(&mut self.counters, &mut summary, outcome);
                    }
                }
                // A recovery error is a spool that cannot be read. The
                // entries are still persisted; saying so beats failing the
                // cycle that tried.
                Err(error) => {
                    eprintln!(
                        "qip-api: the capital spool for {} could not be recovered: {}",
                        lane.dispatcher.cell(),
                        error.message()
                    );
                }
            }
        }

        for grant in pending {
            let Some(lane) = self.lanes.get_mut(&grant.cell) else {
                self.counters.envelopes_unserved += 1;
                if !summary.unserved_cells.contains(&grant.cell) {
                    summary.unserved_cells.push(grant.cell);
                }
                continue;
            };
            let key = grant_key(&grant.envelope);
            if lane.dispatched.contains(&key) {
                continue;
            }
            match lane.dispatcher.pending() {
                Ok(0) => {}
                Ok(_) => {
                    // Order guard: the recovery above left entries held, so
                    // this grant waits for a later cycle rather than entering
                    // a spool it would then be sent ahead of.
                    summary.deferred += 1;
                    continue;
                }
                Err(error) => {
                    eprintln!(
                        "qip-api: the capital spool for {} is unreadable: {}",
                        grant.cell,
                        error.message()
                    );
                    continue;
                }
            }
            match lane.dispatcher.dispatch(grant.envelope, now) {
                Ok(outcome) => {
                    // Remembered on every spooled outcome, including a held
                    // one: the envelope is persisted now, and recovery — not
                    // re-dispatch — is what retries it.
                    lane.remember(key);
                    Self::count(&mut self.counters, &mut summary, &outcome);
                }
                // Refused before anything was persisted (a full spool). The
                // grant stays pending and next cycle tries again.
                Err(error) => {
                    eprintln!(
                        "qip-api: dispatching capital to {} failed: {}",
                        lane.dispatcher.cell(),
                        error.message()
                    );
                }
            }
        }
        summary
    }

    fn count(
        counters: &mut BackboneCounters,
        summary: &mut DispatchSummary,
        outcome: &CapitalDispatch,
    ) {
        match outcome {
            CapitalDispatch::Delivered { .. } => {
                counters.envelopes_dispatched += 1;
                summary.delivered += 1;
            }
            CapitalDispatch::Held { .. } => {
                counters.envelopes_held += 1;
                summary.held += 1;
            }
            CapitalDispatch::Rejected { .. } => {
                counters.envelopes_rejected += 1;
                summary.rejected += 1;
            }
        }
    }

    /// The whole truth, for `/mesh` and the status line.
    pub fn status(&self) -> MeshStatus {
        MeshStatus {
            served: true,
            delta_inbox: self.receiver.inbox().health(),
            receiver: self.receiver.stats(),
            counters: self.counters,
            cells: self
                .lanes
                .iter()
                .map(|(cell, lane)| MeshCellStatus {
                    cell: cell.clone(),
                    address: lane.address.clone(),
                    capital_inbox: lane.capital.inbox().health(),
                    spool_pending: lane.dispatcher.pending().unwrap_or(0),
                    circuit: lane.dispatcher.circuit().as_str().to_string(),
                })
                .collect(),
            standings: self.standings.values().cloned().collect(),
            last_undecodable: self.last_undecodable.clone(),
        }
    }
}

/// The cycle response's account of one drain-and-dispatch exchange.
///
/// A drain error is rendered rather than propagated: the cycle it rode on
/// already ran, so the response reports what the exchange did and did not
/// manage, exactly as the archive failure on the same route is reported.
/// What became of one cycle's policy sends, or one halt broadcast.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PolicySummary {
    pub sent: usize,
    pub circuit_open: usize,
    /// Payloads not sent because no trust root is configured. Counted loudly:
    /// a deployment that thinks it is shipping policy and is not should read
    /// it here rather than infer it from a narrowed cell.
    pub unsigned: usize,
    pub unserved: usize,
    pub errors: Vec<String>,
    /// What each cell's cycle whitelist carried, or why the slot was not
    /// produced — the same lines the process logs. In the response so the
    /// answer to "why does the desk never install" is readable from the
    /// cycle that shipped the policy, not only from a pod's stderr.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub whitelist: Vec<String>,
    /// What each cell's `capital_grants` slot carried, or why it was withheld
    /// (ADR 0039), for the same reason the whitelist lines are here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<String>,
}

pub fn exchange_json(
    drained: &Result<DrainSummary>,
    dispatched: &DispatchSummary,
    policy: &PolicySummary,
) -> String {
    let drained = match drained {
        Ok(summary) => serde_json::to_string(summary).unwrap_or_else(|_| "null".to_string()),
        Err(error) => format!(r#"{{"error":{}}}"#, crate::json::string(error.message())),
    };
    let dispatched = serde_json::to_string(dispatched).unwrap_or_else(|_| "null".to_string());
    let policy = serde_json::to_string(policy).unwrap_or_else(|_| "null".to_string());
    format!(r#"{{"drained":{drained},"dispatched":{dispatched},"policy":{policy}}}"#)
}

// --- delta to report ----------------------------------------------------

/// Build the kernel's report from a delta's absolute half.
///
/// Only the standing crosses into the platform, because only the standing is
/// the cell as it is: utilisation replaces what the plane holds, and every
/// reconciliation break arrives as a break so that `ingest_cell_report`'s
/// halt actually trips. Two absences are deliberate:
///
/// * **No positions.** The delta is not a position book — the cell says so
///   in its own wire contract — and inventing positions from the interval's
///   orders would hand the aggregate exposure numbers nobody reconciled
///   against a custodian.
/// * **No parsed quantities on a break.** The wire carries the cell's prose;
///   the quantities are stated as zero and the prose travels in `detail`,
///   which is what the kill switch's reason renders. Guessing numbers out of
///   a sentence would put fiction in the one record an incident reader
///   trusts.
fn report_from(standing: &CellStanding) -> CellReport {
    let mut report = CellReport::new(standing.cell.clone(), standing.at).with_utilisation(
        standing
            .utilisation
            .iter()
            .map(|entry| (entry.strategy.clone(), entry.utilisation.clone()))
            .collect(),
    );
    for detail in &standing.reconciliation_breaks {
        report = report.with_break(ReconciliationBreak {
            instrument: "unquantified".to_string(),
            cell_quantity: Decimal::ZERO,
            external_quantity: Decimal::ZERO,
            detail: detail.clone(),
            origin: BreakOrigin::Book,
        });
    }
    if standing.reconciliation_breaks_omitted > 0 {
        // The cell said its list understates. That fact must reach the
        // centre as a break of its own, or a cell that dropped a thousand
        // breaks and retained three would read as an incident of three.
        report = report.with_break(ReconciliationBreak {
            instrument: "unquantified".to_string(),
            cell_quantity: Decimal::ZERO,
            external_quantity: Decimal::ZERO,
            detail: format!(
                "{} further reconciliation break(s) the cell recorded but no longer retains",
                standing.reconciliation_breaks_omitted
            ),
            origin: BreakOrigin::Book,
        });
    }
    report
}

/// The sink the drain hands frames to: decode, then ingest.
///
/// Two failure classes part ways here, and the difference is the point. A
/// frame that does not *decode* will never decode — halting on it would
/// wedge the drain behind one poison frame forever on an unauthenticated
/// wire — so it is counted, remembered for the status page, and skipped. An
/// *ingest* that fails is refused with `Err`, which halts the drain and
/// re-offers the frame next cycle: the platform refusing a well-formed
/// report is a condition to stop on, not to skip past, because continuing
/// would leave an invisible hole in the centre's view of that cell.
#[derive(Debug)]
struct IngestSink<'a> {
    platform: &'a mut Platform,
    cells: &'a CellRegistry,
    now: Timestamp,
    counters: &'a mut BackboneCounters,
    standings: &'a mut BTreeMap<String, CellStandingSummary>,
    last_undecodable: &'a mut Option<String>,
    undecodable: usize,
    halted_cells: Vec<String>,
    recalls: usize,
}

impl CellDeltaSink for IngestSink<'_> {
    fn absorb(&mut self, frame: &AnyEvent) -> Result<()> {
        let decoded = match decode_cell_delta(frame) {
            Ok(decoded) => decoded,
            Err(error) => {
                self.undecodable += 1;
                self.counters.undecodable += 1;
                *self.last_undecodable = Some(format!("{}: {}", frame.event_id, error.message()));
                return Ok(());
            }
        };

        // The interval's orders, fills and crosses ride the report, or the
        // centre attributes no fill and settles no cross: a sink that drops
        // them renders every strategy book flat however much the cell traded.
        // The orders travel as what was sent and the fills as what traded;
        // the plane bills from the second and only registers the first.
        let report = report_from(&decoded.standing)
            .with_orders(decoded.interval.orders.clone())
            .with_fills(decoded.interval.fills.clone())
            .with_crosses(decoded.interval.crosses.clone());
        // Recorded before the ingest so `/regions` knows the cell spoke even
        // when the plane goes on to halt it — a halted cell that looked
        // silent would be the worst possible rendering of the loudest fact.
        self.cells.record(&report);
        let ingestion = self.platform.ingest_cell_report(report, self.now)?;

        self.counters.reports_ingested += 1;
        self.counters.orders_reported += decoded.interval.orders.len() as u64;
        self.counters.fills_reported += decoded.interval.fills.len() as u64;
        self.counters.fills_omitted += u64::from(decoded.interval.fills_omitted);
        self.counters.refusals_reported +=
            decoded.interval.refusals.len() as u64 + u64::from(decoded.interval.refusals_omitted);
        if ingestion.halted.is_some() {
            self.counters.cell_halts += 1;
            self.halted_cells.push(ingestion.cell.clone());
        }
        self.counters.recalls_issued += ingestion.recalls.len() as u64;
        self.recalls += ingestion.recalls.len();

        self.standings.insert(
            decoded.standing.cell.clone(),
            CellStandingSummary {
                cell: decoded.standing.cell.clone(),
                region: decoded.standing.region.clone(),
                sequence: decoded.standing.sequence,
                at: decoded.standing.at,
                halted: decoded.standing.halted,
                strategies: decoded.standing.utilisation.len(),
                reconciliation_breaks: decoded.standing.reconciliation_breaks.len(),
                reconciliation_breaks_omitted: decoded.standing.reconciliation_breaks_omitted,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // In a test the assertion is the deliverable; the workspace denies this
    // lint for production code, where a panic on the capital path is a bug.
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_contracts::capital::Utilisation;
    use qip_contracts::signal::StrategyId;

    /// A membership that files nowhere a cell the centre serves is refused
    /// at parse, naming the cell: shipped no share for as long as the
    /// process ran, that cell would look like a node that never funds
    /// (ADR 0039), and the reason would be in nobody's log.
    #[test]
    fn a_region_membership_must_file_every_served_cell_and_is_carried_when_it_does() -> Result<()> {
        let covered = MeshSettings::parse(
            "london-1=127.0.0.1:9101, tokyo-1=127.0.0.1:9102",
            None,
            None,
            Some("europe-west2=1000:london-1; asia-northeast1=500:tokyo-1"),
        )?;
        let membership = covered
            .regions
            .as_ref()
            .ok_or_else(|| Error::not_found("the parsed membership"))?;
        assert_eq!(membership.region_of("tokyo-1"), Some("asia-northeast1"));

        let error = MeshSettings::parse(
            "london-1=127.0.0.1:9101, tokyo-1=127.0.0.1:9102",
            None,
            None,
            Some("europe-west2=1000:london-1"),
        )
        .err()
        .ok_or_else(|| Error::invalid("a membership missing tokyo-1 was admitted"))?;
        assert!(
            error.message().contains("tokyo-1") && error.message().contains(REGIONS_VARIABLE),
            "the refusal names neither the cell nor the variable: {}",
            error.message()
        );

        let unset = MeshSettings::parse("london-1=127.0.0.1:9101", None, None, None)?;
        assert!(
            unset.regions.is_none(),
            "an undeclared membership was invented"
        );
        Ok(())
    }
    use qip_mesh::delta::StrategyStanding;

    #[test]
    fn the_settings_parse_names_every_cell_with_its_own_address() -> Result<()> {
        let settings = MeshSettings::parse(
            "london-1=127.0.0.1:9101, tokyo-1=127.0.0.1:9102",
            Some("64"),
            None,
            None,
        )?;
        assert_eq!(settings.cells.len(), 2);
        assert_eq!(settings.cells[0].cell, "london-1");
        assert_eq!(settings.cells[0].address, "127.0.0.1:9101");
        assert_eq!(settings.inbox_capacity, 64);
        assert_eq!(
            settings.spool_capacity,
            default_spool_capacity(),
            "an unset capacity takes the default rather than failing"
        );
        Ok(())
    }

    #[test]
    fn a_duplicate_cell_or_a_shared_address_is_refused_at_parse() {
        // Two lanes for one cell would both claim the capital spool; two
        // cells on one address are indistinguishable on this transport, so
        // the second would silently read the first's capital.
        let duplicate = MeshSettings::parse("a=127.0.0.1:1,a=127.0.0.1:2", None, None, None)
            .expect_err("one cell was accepted twice");
        assert!(duplicate.message().contains("twice"));
        let shared = MeshSettings::parse("a=127.0.0.1:1,b=127.0.0.1:1", None, None, None)
            .expect_err("two cells were accepted on one address");
        assert!(shared.message().contains("identity"));
    }

    #[test]
    fn an_entry_without_an_address_is_refused_rather_than_defaulted() {
        let error = MeshSettings::parse("london-1", None, None, None)
            .expect_err("a cell with no address was accepted");
        assert!(
            error.message().contains("cell=host:port"),
            "the refusal does not show the expected shape: {}",
            error.message()
        );
    }

    #[test]
    fn a_zero_capacity_is_refused_because_it_is_not_a_disable_switch() {
        let error = MeshSettings::parse("a=127.0.0.1:1", Some("0"), None, None)
            .expect_err("a zero inbox capacity was accepted");
        assert!(error.message().contains(CELLS_VARIABLE));
    }

    fn standing() -> CellStanding {
        CellStanding {
            cell: "london-1".to_string(),
            region: "eu-west".to_string(),
            sequence: 4,
            at: Timestamp::from_secs(1_000),
            halted: false,
            utilisation: vec![StrategyStanding {
                strategy: StrategyId::new("mean-reversion-1"),
                utilisation: Utilisation {
                    gross_committed: Decimal::from_int(250_000),
                    realised_loss: Decimal::from_int(1_200),
                    orders_sent: 14,
                },
                envelope_expires_at: Timestamp::from_secs(5_000),
            }],
            reconciliation_breaks: Vec::new(),
            reconciliation_breaks_omitted: 0,
        }
    }

    #[test]
    fn a_reconciling_standing_becomes_a_report_that_reconciles() {
        let report = report_from(&standing());
        assert_eq!(report.cell, "london-1");
        assert!(report.reconciles(), "a clean delta must not trip a halt");
        assert_eq!(report.utilisation.len(), 1);
        assert_eq!(report.utilisation[0].1.orders_sent, 14);
        assert!(
            report.positions.is_empty(),
            "the delta carries no position book, so the report must not invent one"
        );
    }

    #[test]
    fn every_break_the_cell_described_reaches_the_report_and_trips_reconciliation() {
        let mut standing = standing();
        standing.reconciliation_breaks =
            vec!["OBJ1: cell holds 100, venue confirms 60".to_string()];
        let report = report_from(&standing);
        assert!(
            !report.reconciles(),
            "a break that does not fail reconciliation can never trip the kill switch"
        );
        assert_eq!(report.reconciliation_breaks.len(), 1);
        assert!(
            report.reconciliation_breaks[0]
                .detail
                .contains("venue confirms 60"),
            "the cell's own description must survive into the incident record"
        );
    }

    #[test]
    fn omitted_breaks_surface_as_a_break_of_their_own() {
        // A cell that dropped a thousand breaks and retained three must not
        // read as an incident of three.
        let mut standing = standing();
        standing.reconciliation_breaks = vec!["OBJ1: gap".to_string()];
        standing.reconciliation_breaks_omitted = 997;
        let report = report_from(&standing);
        assert_eq!(report.reconciliation_breaks.len(), 2);
        assert!(
            report.reconciliation_breaks[1].detail.contains("997"),
            "the omission count did not reach the centre: {}",
            report.reconciliation_breaks[1].detail
        );
    }

    #[test]
    fn two_cells_draw_different_dispatch_jitter_from_their_own_names() {
        // Nine dispatchers retrying a recovering process on the same
        // millisecond is the herd the seed exists to break up.
        assert_ne!(seed_from("london-1"), seed_from("tokyo-2"));
        assert_eq!(seed_from("london-1"), seed_from("london-1"));
    }

    /// The backbone's `Debug` output must never contain the raw trust-root
    /// key it dispatches every capital envelope and policy payload under.
    ///
    /// `EnvelopeIssuer` in `qip_capital::envelope` states the failure this
    /// guards against in its own doc comment: a struct that prints its own
    /// secret gets it into a log the first time anything derives `Debug` on
    /// a type that contains it. `MeshBackbone` held the key in a plain
    /// `#[derive(Debug)]` struct, so a panic message, an error wrapper or a
    /// stray `dbg!()` anywhere this backbone crossed would have put the same
    /// key `QIP_CAPITAL_ENVELOPE_KEY` installs into a log line — the key
    /// every cell verifies a capital grant against.
    #[test]
    fn the_backbones_debug_output_never_carries_the_trust_root_key() -> Result<()> {
        // A recognisable byte pattern rather than the production variable's
        // name, so the assertion below cannot pass by accident on a value
        // that happens to contain common English words. Deliberately not
        // named with the bare identifier a short constant like this would
        // usually get: `qip-acceptance`'s `manifest_wiring` walk resolves a
        // binary's environment reads in part by matching bare identifier
        // names workspace-wide and is not comment- or scope-aware, so a
        // short, common local name here can be mistaken for an unrelated
        // constant of the same name declared in another crate.
        const TEST_TRUST_ROOT_SIGNING_MATERIAL: &str = "unmistakable-signing-key-bytes-3f9a7c21";
        let settings = MeshSettings {
            cells: vec![CellAddress {
                cell: "london-1".to_string(),
                address: "127.0.0.1:0".to_string(),
            }],
            inbox_capacity: 64,
            spool_capacity: 64,
            regions: None,
        };
        let backbone = MeshBackbone::open(
            &settings,
            Arc::new(qip_storage::kv::MemoryKeyValueStore::new()),
            Arc::new(qip_core::ManualClock::new(Timestamp::from_secs(
                1_760_000_000,
            ))) as Arc<dyn Clock>,
            Some(TEST_TRUST_ROOT_SIGNING_MATERIAL.as_bytes().to_vec()),
        )?;

        // Premise: the backbone really was built with the key, or a
        // redaction that hid nothing would prove nothing.
        assert!(
            backbone.policy_key.as_deref() == Some(TEST_TRUST_ROOT_SIGNING_MATERIAL.as_bytes()),
            "the backbone was not assembled with the key this test signs with"
        );

        let rendered = format!("{backbone:?}");
        assert!(
            !rendered.contains(TEST_TRUST_ROOT_SIGNING_MATERIAL),
            "the trust-root signing key leaked through MeshBackbone's Debug output: {rendered}"
        );
        assert!(
            rendered.contains("redacted"),
            "the policy key field is missing from the redacted output rather than \
             genuinely redacted: {rendered}"
        );
        Ok(())
    }
}
