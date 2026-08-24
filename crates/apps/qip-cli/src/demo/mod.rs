//! `qip demo --live`: the live path, stood up and watched.
//!
//! # What this is
//!
//! A demonstration. This process binds three loopback servers — a data vendor,
//! a venue and a mesh peer — points the platform's live adapters at them, and
//! walks a complete cycle: observations arriving over a socket, a decision, an
//! order leaving over a socket, and an acknowledgement coming back. Each layer
//! says what it did, in the same register the node banners use.
//!
//! # What this is not
//!
//! **It is not a connection to a market.** Every peer is a script this process
//! wrote and bound a moment ago, on an ephemeral loopback port nobody else can
//! reach. Every fill printed below is fabricated by [`doubles::VenueDouble`].
//! No price here was observed anywhere and no order here reached anybody.
//!
//! That is said at the start of the run and again at the end, deliberately and
//! at length, because a demonstration that could be mistaken for a live run is
//! worse than no demonstration: the mistake is not recoverable and the person
//! making it has no way to tell from the output.
//!
//! # It cannot become a way to reach a real venue
//!
//! `main.rs` records the rule that no subcommand may raise the autonomy level.
//! This one keeps a second rule of the same kind: **it takes no address from
//! anybody.** There is no flag, no environment variable and no configuration
//! file that moves any of these three peers. Every endpoint is read back from a
//! `TcpListener` this process bound on `127.0.0.1:0`, which is the one case
//! where the code genuinely knows what it is talking to — the distinction
//! `qip_brokers::rest` says at length that nothing in the platform can draw for
//! a configured host. Nothing here is an exception to that; it is the one
//! situation the question does not arise in.
//!
//! It also runs at the platform's default autonomy ceiling, and the order below
//! goes through the platform's *own* autonomy controller. An order this
//! demonstration would refuse is an order the normal path refuses, for the same
//! reason, at the same gate.
//!
//! # Bounded and deterministic
//!
//! A [`qip_core::ManualClock`] the run advances itself, a fixed number of
//! cycles ([`DemoSettings::with_cycles`] refuses more than [`MAX_CYCLES`]), a
//! script that is a pure function of the first instant, explicit connect and
//! read timeouts on every adapter, and a sleeper that records the retry ladder
//! rather than spending it. Nothing here waits on wall-clock time and nothing
//! here loops until something happens.
//!
//! # The composition gaps this walk found
//!
//! Recorded here because a demonstration that quietly works around a missing
//! seam is worse than none — and printed at the end of the run, so an operator
//! sees them too rather than having to read this file.
//!
//! * **`Platform` fixes its broker at construction.** There is a
//!   `Platform::set_central` and no `set_broker`, so the kernel's own
//!   `submit_order` can only ever reach the in-process `SimulatedBroker`, even
//!   though `qip_brokers::rest::RestOrderEntryAdapter` implements the same
//!   `Broker` port. Layer 6 therefore builds the same `OrderManager` over the
//!   same `PreTradeChecker` and submits with the platform's own
//!   `AutonomyController` — which is the call `Platform::submit_order` makes
//!   internally, and as close to it as this walk can get without editing the
//!   kernel.
//! * **The composition that gives a cell a live gateway is not reachable.**
//!   `qip_edge_node::gateway::RestGateway` already implements the cell's
//!   `Placer` on top of the REST venue adapter, and `qip_edge_node::venue`
//!   already holds the refusal that decides whether it may be built. Neither is
//!   in `[workspace.dependencies]`, so no other binary can name them. Layer 5
//!   therefore hands the cell no gateway: the cell verifies its grant and
//!   reports its authority, and its own trading is not part of this walk. The
//!   alternative was to copy `RestGateway` into this crate, which would put a
//!   second, untested answer to "may this cell reach a venue" in the tree.
//! * **Nothing decodes a cell delta into the central plane's `CellReport`.**
//!   `qip_mesh::spine` says the composition root is where that decode belongs
//!   and the composition root does not do it. Layer 5 reads the delta the
//!   centre received and prints it; it does not invent the mapping, because a
//!   mapping invented in a demonstration would be a mapping nothing else uses.

pub mod doubles;
mod script;

use crate::demo::doubles::{
    ALTERNATIVE_PATH, DEPTH_SNAPSHOT_PATH, DEPTH_UPDATES_PATH, HEALTH_PATH, Loopback,
    MARKET_DATA_PATH, MeshPeer, NARRATIVE_PATH, ORDERS_PATH, VendorDouble, VenueDouble,
};
use qip_api::http::Handler;
use qip_brokers::adapter::VenueAdapter;
use qip_brokers::credential::{
    RequirementKind, Secret, VenueCredential, requirements_of_kind, standard_requirements,
};
use qip_brokers::rest::{RestOrderEntryAdapter, RestVenueConfig};
use qip_capital::envelope::{EnvelopeIssuer, EnvelopeTerms};
use qip_contracts::governance::Approval;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, WorkReport};
use qip_edge::mesh::{
    CapitalDownlink, CellStateDelta, CellUplink, Dispatch, DownlinkConfig, UplinkConfig,
};
use qip_events::AnyEvent;
use qip_execution_engine::broker::Broker;
use qip_execution_engine::oms::OrderManager;
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_market_ingestion::alternative::{
    AlternativeFeedAdapter, AlternativeFeedConfig, AlternativeSubject,
};
use qip_market_ingestion::depth::{DepthFeedAdapter, DepthFeedConfig, DepthInstrument};
use qip_market_ingestion::narrative::{NarrativeAdapter, NarrativeFeedConfig, NarrativeSubject};
use qip_market_ingestion::rest::{RestFeedConfig, RestInstrument, RestMarketDataAdapter};
use qip_mesh::spine::{
    CapitalDispatch, CapitalDispatcher, CellDeltaReceiver, CellDeltaSink, DispatcherConfig,
};
use qip_observability::Telemetry;
use qip_risk::limits::{LimitSet, RiskState};
use qip_risk_engine::pretrade::PreTradeChecker;
use qip_storage::kv::MemoryKeyValueStore;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use qip_transport::{
    ClientLimits, MemoryDeadLetters, MeshConfig, RecordingSleeper, RetryPolicy, Sleeper,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

// --- the fixture ------------------------------------------------------------

/// The cell this demonstration plays, and the region it sits in.
pub const CELL: &str = "london-1";
pub const REGION: &str = "europe-west2";
/// The venue every order names.
pub const VENUE: &str = "XLON";
/// The strategy the cell deploys under the grant that crosses the mesh.
pub const STRATEGY: &str = "demo-live-book-pressure";
/// The account the venue double books against.
pub const VENUE_ACCOUNT: &str = "qip-demo-book";

/// Most cycles this demonstration will run.
///
/// A bound rather than a default: the point of the command is to be watched,
/// and a run long enough that nobody watches it is a run that should have been
/// a test.
pub const MAX_CYCLES: u64 = 10;

/// The key the cell verifies capital grants with.
///
/// A literal, in the source, in a demonstration. That is safe here and only
/// here: this key signs grants for a cell that exists for the length of one
/// command and can commit capital nowhere. A deployed cell's key is written
/// into Secret Manager by a person and reaches the process through a CSI mount;
/// see `docs/security/credentials.md`.
const ENVELOPE_KEY: &[u8] = b"qip-demo-live-envelope-key-not-a-secret";

/// The vendor credential the adapters send.
///
/// Distinctive on purpose. It travels in a header and never in a URL, and a
/// literal this recognisable is what lets somebody reading the run's output, or
/// a test, check that.
const VENDOR_KEY: &str = "demo-vendor-key-never-in-a-url";

/// The venue session secret, for the same reason.
const VENUE_SECRET: &str = "demo-venue-secret-never-in-a-url";

/// How large an order the demonstration sends.
const ORDER_UNITS: i64 = 100;

/// The instrument every feed and the order all name.
fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{}", script::SYMBOL))
}

/// Transport limits tight enough that an unanswered request fails this run in
/// milliseconds rather than in minutes, and generous enough in bytes for a
/// hundred and twenty bars.
fn http_limits() -> ClientLimits {
    ClientLimits {
        max_body: 1024 * 1024,
        max_headers: 32,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(2_000),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

/// A retry ladder that is short, and jitter-free so the run replays.
fn retry_policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(4),
        multiplier: 2,
        jitter_basis_points: 0,
    }
}

/// A sleeper that records the ladder rather than spending it.
///
/// Against loopback peers that always answer, nothing here should retry at all.
/// If something does, the demonstration's job is to say so — not to spend two
/// seconds of an operator's attention not saying it. What the waits themselves
/// do is `qip-transport`'s property and is proven by its own suite.
fn sleeper() -> Arc<dyn Sleeper> {
    Arc::new(RecordingSleeper::new())
}

fn mesh_config(name: &str, peer: &str) -> MeshConfig {
    MeshConfig::new(name, peer)
        .with_retry(retry_policy())
        .with_limits(http_limits())
}

/// The risk picture `Platform::submit_order` builds for its own control path.
///
/// Reproduced rather than borrowed because `Platform::risk_state` is private.
/// The numbers are the platform's, so the pre-trade check in layer 6 is the
/// check the platform would have run.
fn platform_risk_state() -> RiskState {
    RiskState {
        equity: Decimal::from_int(10_000_000),
        cash: Decimal::from_int(10_000_000),
        ..RiskState::default()
    }
}

/// A universe holding the one instrument this walk is about.
fn universe(at: Timestamp) -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(object(), script::SYMBOL, InstrumentType::CommonStock)
            .venue(VENUE)
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("qip-demo-live", at))
            .build(at)?,
    )?;
    Ok(universe)
}

/// One rule over one feature, compiled by the real compiler.
///
/// The rule is `false` and never fires, which is deliberate rather than lazy:
/// this demonstration gives the cell no gateway (see the module documentation's
/// second gap), so a rule that *did* fire would produce a signal the cell could
/// not act on and the run would print a refusal it had arranged for itself.
/// What the cell is here to show is that a grant which crossed a socket is
/// verified against the cell's own key before anything is deployed under it.
fn compiled_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "stand-down",
            SignalKind::Enter,
            Expr::Flag(false),
            Expr::Exact(dec!("1")),
            Expr::Statistic(0.5),
            100,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A credential carrying the resolved secret, which is the only shape a
/// transport can use.
fn venue_credential() -> Result<VenueCredential> {
    let venue = VenueId::new(VENUE);
    let enforced = requirements_of_kind(
        &standard_requirements(&venue),
        &[RequirementKind::Account, RequirementKind::SessionCredential],
    );
    let name = enforced
        .iter()
        .find(|requirement| requirement.kind == RequirementKind::SessionCredential)
        .map(|requirement| requirement.name.clone())
        .ok_or_else(|| {
            Error::not_found("the standard requirement list names no session credential")
        })?;
    Ok(
        VenueCredential::satisfying(VENUE, VENUE_ACCOUNT, &enforced)?.with_secret(
            name,
            format!("QIP_{VENUE}_CREDENTIAL"),
            Secret::new(VENUE_SECRET),
        ),
    )
}

// --- what the operator is told ----------------------------------------------

/// How many of each kind of record arrived, and how many did not.
///
/// The withheld counts are next to the arrived ones on purpose. A record the
/// vendor sent and the adapter would not yet publish is the point-in-time
/// discipline working, and an operator who only sees what arrived cannot tell
/// that from a vendor that sent less.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensedCounts {
    pub bars: usize,
    pub quotes: usize,
    pub trades: usize,
    pub reference: usize,
    pub news: usize,
    pub fundamentals: usize,
    pub macro_observations: usize,
    pub books: usize,
    pub alternative: usize,
    /// Records the vendor sent in *this* cycle that were not yet knowable at
    /// its poll instant. A difference of the adapters' cumulative counters,
    /// because a cumulative number under a per-cycle heading is the kind of
    /// wrong that nobody checks.
    pub withheld: u64,
}

impl SensedCounts {
    /// Count a batch of records by kind.
    pub fn of(records: &[SensedRecord]) -> Self {
        let mut counts = Self::default();
        for record in records {
            match record {
                SensedRecord::Bar(_) => counts.bars += 1,
                SensedRecord::Quote(_) => counts.quotes += 1,
                SensedRecord::Trade(_) | SensedRecord::Tick(_) => counts.trades += 1,
                SensedRecord::ReferenceData(_) | SensedRecord::CorporateAction(_) => {
                    counts.reference += 1;
                }
                SensedRecord::News(_) => counts.news += 1,
                SensedRecord::Fundamental(_) => counts.fundamentals += 1,
                SensedRecord::Macro(_) => counts.macro_observations += 1,
                SensedRecord::Book(_) => counts.books += 1,
                SensedRecord::AlternativeData(_) => counts.alternative += 1,
            }
        }
        counts
    }

    pub fn total(&self) -> usize {
        self.bars
            + self.quotes
            + self.trades
            + self.reference
            + self.news
            + self.fundamentals
            + self.macro_observations
            + self.books
            + self.alternative
    }

    /// The kinds that are non-zero, so a line does not list six zeroes.
    fn describe(&self) -> String {
        let parts = [
            (self.bars, "bar"),
            (self.quotes, "quote"),
            (self.trades, "trade"),
            (self.reference, "reference change"),
            (self.news, "news item"),
            (self.fundamentals, "fundamental"),
            (self.macro_observations, "macro release"),
            (self.books, "book"),
            (self.alternative, "alternative reading"),
        ];
        let listed: Vec<String> = parts
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, name)| format!("{count} {name}(s)"))
            .collect();
        if listed.is_empty() {
            "nothing".to_string()
        } else {
            listed.join(", ")
        }
    }
}

/// What the mesh did in one cycle, both directions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshOutcome {
    /// What became of the grant the centre sent down: `delivered`, `held` or
    /// `rejected`.
    pub dispatch: String,
    /// Strategies whose grant the cell verified against its own key.
    pub verified: Vec<String>,
    /// Grants the cell would not take, and why.
    pub refused: Vec<String>,
    /// Grants recognised as ones already applied.
    pub duplicates: usize,
    /// What became of the delta the cell sent up.
    pub delta: String,
    /// Deltas the centre absorbed, and frames on its inbox that were not one.
    pub absorbed: usize,
    pub ignored: usize,
}

/// What happened to the one order this cycle sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueOutcome {
    pub order_id: String,
    pub accepted: bool,
    pub refusal: Option<String>,
    /// What the venue said it filled, against what was asked for.
    pub requested: Decimal,
    pub filled: Decimal,
    /// Orders whose state nobody knows. The number to alert on in a deployment,
    /// and printed here for the same reason.
    pub unknown: usize,
    /// The broker's own answer, never this command's configuration.
    pub simulated: bool,
    /// Submits that actually reached the venue's socket.
    pub submits: u64,
}

impl VenueOutcome {
    pub fn shortfall(&self) -> Decimal {
        self.requested - self.filled
    }
}

/// The platform's own record, after the cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordOutcome {
    pub events: usize,
    pub chain: String,
    pub autonomy: String,
    pub live_capable: bool,
}

/// Everything one cycle of the demonstration did.
///
/// Returned rather than printed so a test can assert on the same values an
/// operator reads. [`Self::lines`] is the only place the two diverge, and it
/// diverges only in formatting.
#[derive(Clone, Debug)]
pub struct CycleOutcome {
    pub cycle: u64,
    pub of: u64,
    pub at: Timestamp,
    pub sensed: SensedCounts,
    /// Requests that reached the vendor's five feeds during this cycle, and
    /// since the run began. The number that separates "the adapter decoded
    /// something" from "the adapter opened a socket".
    pub vendor_requests: u64,
    pub vendor_requests_total: u64,
    pub absorbed: usize,
    /// `CycleReport::summarise`, verbatim.
    pub loop_summary: String,
    pub opportunities: usize,
    /// The grant the centre signed this cycle.
    pub granted: Decimal,
    pub grant_expires: Timestamp,
    pub mesh: MeshOutcome,
    /// The delta the centre received, if it received one.
    pub delta: Option<CellStateDelta>,
    pub venue: VenueOutcome,
    pub record: RecordOutcome,
}

impl CycleOutcome {
    /// The cycle, as an operator reads it.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "cycle {} of {} at {}",
            self.cycle,
            self.of,
            self.at.to_rfc3339()
        )];
        lines.push(format!(
            "  layer 1  vendor    {} record(s) from {} request(s) over sockets ({} since the \
             run began): {}",
            self.sensed.total(),
            self.vendor_requests,
            self.vendor_requests_total,
            self.sensed.describe()
        ));
        lines.push(format!(
            "           withheld  {} record(s) the vendor sent this cycle that are not yet \
             knowable, and were counted rather than dropped",
            self.sensed.withheld
        ));
        lines.push(format!(
            "  layer 2  platform  {} record(s) absorbed",
            self.absorbed
        ));
        lines.push(format!(
            "  layer 3  loop      {} opportunity(ies) in the queue after this pass",
            self.opportunities
        ));
        for line in self.loop_summary.lines() {
            lines.push(format!("           {line}"));
        }
        lines.push(format!(
            "  layer 4  capital   {} granted to {CELL} at {VENUE}, expiring {}; the centre's \
             dispatch was {}",
            self.granted,
            self.grant_expires.to_rfc3339(),
            self.mesh.dispatch
        ));
        lines.push(format!(
            "  layer 5  cell      {} grant(s) verified against the cell's own key{}",
            self.mesh.verified.len(),
            if self.mesh.verified.is_empty() {
                String::new()
            } else {
                format!(": {}", self.mesh.verified.join(", "))
            }
        ));
        for refusal in &self.mesh.refused {
            lines.push(format!("           !         {refusal}"));
        }
        lines.push(format!(
            "           mesh      delta {}; the centre absorbed {} and ignored {}",
            self.mesh.delta, self.mesh.absorbed, self.mesh.ignored
        ));
        if let Some(delta) = &self.delta {
            lines.push(format!(
                "           delta     sequence {}, halted {}, {} strategy authority(ies), \
                 {} break(s)",
                delta.sequence,
                delta.halted,
                delta.utilisation.len(),
                delta.reconciliation_breaks.len()
            ));
        }
        lines.push(format!(
            "  layer 6  venue     {} {}; {} submit(s) reached the venue's socket",
            self.venue.order_id,
            if self.venue.accepted {
                "accepted by the control path".to_string()
            } else {
                format!(
                    "REFUSED: {}",
                    self.venue
                        .refusal
                        .clone()
                        .unwrap_or_else(|| "no reason recorded".to_string())
                )
            },
            self.venue.submits
        ));
        lines.push(format!(
            "           fill      the venue reported {} of {} filled, {} short; {} order(s) in \
             an unknown state",
            self.venue.filled,
            self.venue.requested,
            self.venue.shortfall(),
            self.venue.unknown
        ));
        lines.push(format!(
            "           source    fabricated by this process's venue double. \
             broker.is_simulated() = {}",
            self.venue.simulated
        ));
        lines.push(format!(
            "  layer 7  record    {} event(s), log chain {}, autonomy {}, live {}",
            self.record.events,
            self.record.chain,
            self.record.autonomy,
            if self.record.live_capable {
                "REACHABLE"
            } else {
                "unreachable in this deployment"
            }
        ));
        lines
    }
}

// --- how the run is configured ----------------------------------------------

/// Everything the run decides before it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoSettings {
    /// How many cycles to run. Bounded by [`MAX_CYCLES`].
    pub cycles: u64,
    /// The first instant. Every fixture is derived from it, so moving it moves
    /// the whole script and changes nothing else.
    pub start: Timestamp,
    /// How far the manual clock advances between cycles.
    pub interval: Duration,
}

impl Default for DemoSettings {
    fn default() -> Self {
        Self {
            // Two, because one cycle cannot show the thing the second cycle
            // shows: the same vendor response, unchanged, yielding one more
            // record because the clock passed a bar's close.
            cycles: 2,
            // 15:00 on a Monday in August 2026 — mid-session, so nothing here
            // turns on a session boundary.
            start: Timestamp::from_secs(1_787_583_600),
            interval: Duration::from_hours(24),
        }
    }
}

impl DemoSettings {
    /// The same settings with a different cycle count, refused if it is not a
    /// number somebody could sit and watch.
    pub fn with_cycles(mut self, cycles: u64) -> Result<Self> {
        if cycles == 0 || cycles > MAX_CYCLES {
            return Err(Error::invalid(format!(
                "run between 1 and {MAX_CYCLES} cycles; this is a demonstration to be watched, \
                 and a run nobody watches should have been a test"
            )));
        }
        self.cycles = cycles;
        Ok(self)
    }
}

// --- the centre's consumer --------------------------------------------------

/// What the centre does with the deltas it drains.
///
/// It keeps them. See the module documentation's third gap: nothing in the
/// workspace turns a `CellStateDelta` into the central plane's `CellReport`, and
/// this demonstration will not be the first thing to invent that mapping.
#[derive(Debug, Default)]
struct DeltaLedger {
    received: Vec<CellStateDelta>,
}

impl CellDeltaSink for DeltaLedger {
    fn absorb(&mut self, frame: &AnyEvent) -> Result<()> {
        let delta: CellStateDelta =
            serde_json::from_value(frame.payload.clone()).map_err(|error| {
                Error::schema(format!(
                    "the centre received a frame on the cell-delta topic that is not a delta: \
                     {error}"
                ))
            })?;
        self.received.push(delta);
        Ok(())
    }
}

// --- the demonstration ------------------------------------------------------

/// The live path, stood up: three peers, the adapters pointed at them, and the
/// platform behind those.
///
/// Standing it up is separate from running it so that a failure to bind, to
/// configure an adapter or to bring a venue session up is reported before
/// anything claims to have run — and so that a test can stand one up and assert
/// on the seams without running a cycle at all.
pub struct LiveDemo {
    settings: DemoSettings,
    completed: u64,
    /// The adapters' cumulative counters as of the end of the previous cycle,
    /// so this one can report its own.
    withheld_before: u64,
    vendor_requests_before: u64,

    vendor: Loopback,
    venue_peer: Loopback,
    mesh_peer: Loopback,
    vendor_script: Arc<VendorDouble>,
    venue_script: Arc<VenueDouble>,

    clock: Arc<ManualClock>,
    platform: Platform,

    prices: RestMarketDataAdapter,
    documents: NarrativeAdapter,
    depth: DepthFeedAdapter,
    alternative: AlternativeFeedAdapter,

    venue: RestOrderEntryAdapter,
    orders: OrderManager,

    cell: Cell,
    issuer: EnvelopeIssuer,
    dispatcher: CapitalDispatcher,
    receiver: CellDeltaReceiver,
    uplink: CellUplink,
    downlink: CapitalDownlink,
    ledger: DeltaLedger,
    /// Taken the first time a grant verifies. A strategy is deployed once and
    /// renewed after that.
    undeployed: Option<(CompiledStrategy, Program)>,
}

/// Written by hand rather than derived: a derived one would print a platform, a
/// cell and four adapters, which is several thousand lines of nothing anybody
/// asked for. What identifies a run is which peers it bound.
impl std::fmt::Debug for LiveDemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDemo")
            .field("vendor", &self.vendor.url())
            .field("venue", &self.venue_peer.url())
            .field("mesh_peer", &self.mesh_peer.url())
            .field("cycles", &self.settings.cycles)
            .field("completed", &self.completed)
            .finish_non_exhaustive()
    }
}

impl LiveDemo {
    /// Bind the peers, point everything at them, and prove the venue session.
    pub fn stand_up(settings: DemoSettings) -> Result<Self> {
        let start = settings.start;

        // Each script is held twice: once by the server that answers with it,
        // and once here, so the run can ask it afterwards what it was asked
        // for. The typed bindings are what perform the unsizing coercion; a
        // bare `Arc::clone` would infer the trait object and clone the wrong
        // handle.
        let vendor_script = Arc::new(VendorDouble::new(start));
        let vendor_handler: Arc<dyn Handler> = vendor_script.clone();
        let vendor = Loopback::spawn("vendor", vendor_handler)?;
        let venue_script = Arc::new(VenueDouble::new());
        let venue_handler: Arc<dyn Handler> = venue_script.clone();
        let venue_peer = Loopback::spawn("venue", venue_handler)?;

        // The centre's inbox is the mesh peer, and both directions use it: the
        // cell publishes deltas into it and polls grants out of it. That is a
        // shared inbox, which `qip_edge::mesh` documents as a deployment's
        // choice — a frame on a topic a consumer does not handle is counted,
        // not refused. The cycle below orders the four mesh steps so that
        // neither consumer acknowledges past a frame the other has not read.
        let receiver = CellDeltaReceiver::with_defaults("qip-demo-centre-inbox", 64)?;
        let mesh_peer = Loopback::spawn(
            "mesh-peer",
            Arc::new(MeshPeer::new(receiver.endpoint().clone())),
        )?;

        let clock = Arc::new(ManualClock::new(start));
        let clock_port: Arc<dyn Clock> = clock.clone();
        let config = PlatformConfig::default();
        let context = Context::new(Arc::clone(&clock_port), config.seed);
        let platform = Platform::new(
            config,
            context,
            Telemetry::silent(),
            universe(start)?,
            LimitSet::conservative_default(),
        )?;

        let base = vendor.url().to_string();
        let prices = RestMarketDataAdapter::new(
            price_config(&base),
            vec![RestInstrument::new(object(), script::SYMBOL, VENUE)],
        )?;
        let documents = NarrativeAdapter::new(
            document_config(&base),
            vec![NarrativeSubject::new(script::ENTITY, script::ISSUER)],
            vec![script::SERIES.to_string()],
        )?;
        let depth = DepthFeedAdapter::new(
            depth_config(&base),
            vec![DepthInstrument::new(
                object(),
                script::SYMBOL,
                VENUE,
                "depth-a",
            )],
        )?;
        let alternative = AlternativeFeedAdapter::new(
            alternative_config(&base),
            vec![script::DATASET.to_string()],
            vec![AlternativeSubject::new(script::ENTITY, script::SUBJECT)],
        )?;

        // Connect, log on and heartbeat: three real requests to the venue,
        // before anything claims a session exists.
        let mut venue =
            RestOrderEntryAdapter::new(VenueId::new(VENUE), venue_config(venue_peer.url()), start)?;
        venue.bring_up(&venue_credential()?, start)?;

        let cell = Cell::new(
            CellConfig::new(CELL, REGION).with_venue(VenueId::new(VENUE)),
            FeatureEngine::new(MarketState::default(), Duration::from_secs(30)),
        )?;

        let issuer = EnvelopeIssuer::new(ENVELOPE_KEY.to_vec(), "qip-demo-capital-key")?;
        let dispatcher = CapitalDispatcher::open(
            DispatcherConfig::new(CELL, mesh_config("centre-capital", mesh_peer.url())),
            // In memory: this demonstration's spool exists for the length of
            // one command and is not a store anybody should recover from.
            Arc::new(MemoryKeyValueStore::new()),
            Arc::clone(&clock_port),
            sleeper(),
            Box::new(MemoryDeadLetters::new(16)),
        )?;
        let uplink = CellUplink::connect(
            UplinkConfig::new(CELL, REGION, mesh_config("uplink", mesh_peer.url())),
            Arc::clone(&clock_port),
            sleeper(),
            Box::new(MemoryDeadLetters::new(16)),
        )?;
        let downlink = CapitalDownlink::connect(
            DownlinkConfig::new(CELL, mesh_config("downlink", mesh_peer.url())),
            ENVELOPE_KEY,
            Arc::clone(&clock_port),
            sleeper(),
        )?;

        Ok(Self {
            settings,
            completed: 0,
            withheld_before: 0,
            vendor_requests_before: 0,
            vendor,
            venue_peer,
            mesh_peer,
            vendor_script,
            venue_script,
            clock,
            platform,
            prices,
            documents,
            depth,
            alternative,
            venue,
            orders: OrderManager::new(PreTradeChecker::new(LimitSet::conservative_default())),
            cell,
            issuer,
            dispatcher,
            receiver,
            uplink,
            downlink,
            ledger: DeltaLedger::default(),
            undeployed: Some(compiled_strategy()?),
        })
    }

    /// The three peers, so a caller can name them.
    pub fn peers(&self) -> [&Loopback; 3] {
        [&self.vendor, &self.venue_peer, &self.mesh_peer]
    }

    pub fn settings(&self) -> DemoSettings {
        self.settings
    }

    /// How many cycles have finished.
    pub fn completed(&self) -> u64 {
        self.completed
    }

    /// The instant cycle `n` runs at. Cycle 1 is the first instant.
    fn instant(&self, cycle: u64) -> Timestamp {
        let steps = i64::try_from(cycle.saturating_sub(1)).unwrap_or(i64::MAX);
        self.settings.start.saturating_add(Duration::from_nanos(
            self.settings.interval.as_nanos().saturating_mul(steps),
        ))
    }

    /// Run the next cycle.
    ///
    /// Returns what each layer did. Every failure is returned rather than
    /// printed: a demonstration that swallowed an error would be a
    /// demonstration whose green output meant nothing.
    pub fn cycle(&mut self) -> Result<CycleOutcome> {
        let cycle = self.completed + 1;
        if cycle > self.settings.cycles {
            return Err(Error::invalid(format!(
                "this run is {} cycle(s) long and all of them have been run",
                self.settings.cycles
            )));
        }
        let now = self.instant(cycle);
        // The platform's context reads this clock, so setting it here is what
        // makes every id and every stamp inside the cycle agree with the
        // instant the adapters were polled at.
        self.clock.set(now);

        // --- layer 1: four feeds, four sockets ---------------------------
        let mut records = Vec::new();
        records.extend(self.prices.poll(now)?);
        records.extend(self.documents.poll(now)?);
        records.extend(self.depth.poll(now)?);
        records.extend(self.alternative.poll(now)?);
        let mut sensed = SensedCounts::of(&records);
        let withheld = self.prices.stats().withheld
            + self.documents.stats().withheld
            + self.alternative.stats().withheld
            + self.depth.stats().withheld_late;
        sensed.withheld = withheld.saturating_sub(self.withheld_before);
        self.withheld_before = withheld;
        let vendor_requests_total: u64 = [
            MARKET_DATA_PATH,
            NARRATIVE_PATH,
            DEPTH_SNAPSHOT_PATH,
            DEPTH_UPDATES_PATH,
            ALTERNATIVE_PATH,
        ]
        .iter()
        .map(|path| self.vendor_script.hits(path))
        .sum();
        let vendor_requests = vendor_requests_total.saturating_sub(self.vendor_requests_before);
        self.vendor_requests_before = vendor_requests_total;

        // --- layer 2: the platform absorbs -------------------------------
        let absorbed = self.platform.observe(records);

        // --- layer 3: the loop decides -----------------------------------
        let report = self.platform.run_cycle(now);
        let loop_summary = report.summarise();
        let opportunities = self.platform.queue().len();

        // --- layer 4: the centre signs a grant and sends it down ---------
        let terms = EnvelopeTerms {
            strategy: StrategyId::new(STRATEGY),
            cell: CELL.to_string(),
            gross_limit: dec!("1000000"),
            order_fraction: dec!("0.05"),
            loss_fraction: dec!("0.02"),
            venues: vec![VenueId::new(VENUE)],
            validity: Duration::from_hours(8),
        };
        let approval = Approval::new(
            "capital grant for the live demonstration",
            "demo.operator",
            now,
            "a grant for a cell that exists for the length of one command",
        )?
        .countersigned_by("demo.reviewer")?;
        let envelope = self.issuer.issue(&terms, &approval, now)?;
        let granted = envelope.gross_limit();
        let grant_expires = envelope.expires_at();
        let dispatch = self.dispatcher.dispatch(envelope, now)?;

        // --- layer 5: the cell verifies it, then reports itself ----------
        //
        // Order matters, and it is the order `qip_edge_node::mesh` keeps for
        // its own reasons: take capital first, so the delta the centre receives
        // already reflects the grant installed in the same tick.
        let mut mesh = MeshOutcome {
            dispatch: describe_dispatch(&dispatch),
            ..MeshOutcome::default()
        };
        let batch = self.downlink.poll(now)?;
        mesh.duplicates = batch.duplicates.len();
        for refusal in batch.refused {
            mesh.refused
                .push(format!("{}: {}", refusal.event_id, refusal.reason));
        }
        for verified in batch.verified {
            let strategy = verified.strategy().as_str().to_string();
            let outcome = match self.undeployed.take() {
                Some((compiled, program)) => self.cell.deploy(compiled, program, verified),
                None => self.cell.renew_capital(verified, now),
            };
            match outcome {
                Ok(()) => mesh.verified.push(strategy),
                Err(error) => mesh
                    .refused
                    .push(format!("{strategy}: {}", error.message())),
            }
        }

        // The cell is given no gateway by this demonstration, so its report is
        // empty by construction and its delta is a statement about authority
        // and halt state rather than about trading. See the module
        // documentation's second gap.
        let delta = self.cell.state_delta(&WorkReport::default(), now);
        mesh.delta = match self.uplink.publish(delta, now) {
            Ok(Dispatch::Delivered(_)) => "delivered".to_string(),
            Ok(Dispatch::CircuitOpen(_)) => {
                "not sent; the circuit to the centre is open".to_string()
            }
            Ok(Dispatch::DeadLettered { .. }) => "dead-lettered".to_string(),
            Err(error) => format!("refused by the transport: {}", error.message()),
        };
        let drained = self.receiver.drain(now, 32, &mut self.ledger)?;
        mesh.absorbed = drained.absorbed;
        mesh.ignored = drained.ignored;
        let received = self.ledger.received.last().cloned();

        // --- layer 6: one order, over a socket ---------------------------
        //
        // Nothing here runs a scheduler, so the venue session goes stale
        // between one cycle and the next and `ready` refuses a session last
        // confirmed at some unknown time. Re-proving it costs a round trip on
        // the latency-sensitive path, which is named rather than hidden: a
        // production cell heartbeats on a timer and this is then a no-op.
        self.venue.heartbeat(now)?;
        let price = Decimal::parse(&format!("{:.2}", script::last_close())).unwrap_or(dec!("100"));
        let order = self.platform.order_from(
            object(),
            Side::Buy,
            Decimal::from_int(ORDER_UNITS),
            price,
            "prop-demo-live",
            vec![format!("the live demonstration's cycle {cycle}")],
            now,
        );
        let order_id = order.order_id.as_str().to_string();
        let requested = order.quantity;
        let result = self.orders.submit(
            order,
            &mut self.venue as &mut dyn Broker,
            self.platform.autonomy(),
            &platform_risk_state(),
            BTreeMap::new(),
            None,
            now,
        );
        let venue = VenueOutcome {
            order_id,
            accepted: result.accepted,
            refusal: result.refusal.as_ref().map(|reason| reason.describe()),
            requested,
            filled: result.filled_quantity(),
            unknown: self.venue.unknown_orders().len(),
            simulated: result.simulated,
            submits: self.venue_script.hits(ORDERS_PATH),
        };

        // --- layer 7: the record -----------------------------------------
        let record = RecordOutcome {
            events: self.platform.event_log().len(),
            chain: match self.platform.event_log().verify_chain() {
                Ok(()) => "intact".to_string(),
                Err(sequence) => format!("BROKEN at sequence {sequence}"),
            },
            autonomy: self.platform.autonomy().level().to_string(),
            live_capable: self.platform.is_live_capable(),
        };

        self.completed = cycle;
        Ok(CycleOutcome {
            cycle,
            of: self.settings.cycles,
            at: now,
            sensed,
            vendor_requests,
            vendor_requests_total,
            absorbed,
            loop_summary,
            opportunities,
            granted,
            grant_expires,
            mesh,
            delta: received,
            venue,
            record,
        })
    }

    /// What this run is, before any of it happens.
    pub fn banner_lines(&self) -> Vec<String> {
        let controller = self.platform.autonomy();
        vec![
            "qip demo --live: A DEMONSTRATION AGAINST LOOPBACK SERVERS, NOT A MARKET".to_string(),
            format!(
                "  vendor:     {}  prices, documents, book depth and alternative data, all \
                 scripted by this process",
                self.vendor.url()
            ),
            format!(
                "  venue:      {}  EVERY FILL BELOW IS FABRICATED HERE. There is no book, no \
                 counterparty and no money",
                self.venue_peer.url()
            ),
            format!(
                "  mesh peer:  {}  the central plane's inbox, played by this process",
                self.mesh_peer.url()
            ),
            "  addresses:  read back from listeners this process bound on 127.0.0.1:0. No flag \
             and no variable moves any of them"
                .to_string(),
            format!(
                "  autonomy:   {} (ceiling {}); live trading {}",
                controller.level(),
                controller.ceiling(),
                if self.platform.is_live_capable() {
                    "REACHABLE"
                } else {
                    "unreachable in this deployment"
                }
            ),
            format!(
                "  clock:      manual, from {}, +{}h per cycle",
                self.settings.start.to_rfc3339(),
                self.settings.interval.as_nanos() / 3_600_000_000_000
            ),
            format!(
                "  run:        {} cycle(s), then exit. Nothing is written to any store",
                self.settings.cycles
            ),
            "  NOT production-grade; no capital decision may rest on it".to_string(),
        ]
    }

    /// What the run was, after all of it has happened.
    ///
    /// Deliberately not a summary of the numbers — those were printed as they
    /// happened. This is the second half of the sentence the banner started,
    /// and it is here because the top of a long scroll is the part nobody
    /// re-reads.
    pub fn closing_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "qip demo --live finished: {} cycle(s) against {} loopback peer(s)",
                self.completed,
                self.peers().len()
            ),
            format!(
                "  vendor:     {} request(s) served by a script in this process",
                self.vendor.served()
            ),
            format!(
                "  venue:      {} request(s), of which {} were submits. Every fill this run \
                 reported was made up by {}",
                self.venue_peer.served(),
                self.venue_script.hits(ORDERS_PATH),
                self.venue_peer.url()
            ),
            format!(
                "  mesh peer:  {} request(s); the centre absorbed {} cell delta(s)",
                self.mesh_peer.served(),
                self.receiver.stats().absorbed
            ),
            format!(
                "  autonomy:   {} throughout; the run never raised it and has no way to",
                self.platform.autonomy().level()
            ),
            "  NOT production-grade; no capital decision may rest on it".to_string(),
            String::new(),
            "  What this did NOT show:".to_string(),
            "    * a market. Every peer above answers immediately and correctly because this \
             process wrote what it would say."
                .to_string(),
            "    * TLS, and therefore any real vendor: qip_transport::http has no TLS stack and \
             refuses https by name."
                .to_string(),
            "    * what any adapter does with a peer that stalls, truncates or lies. That is \
             each adapter's own suite."
                .to_string(),
            String::new(),
            "  Composition gaps this walk ran into, reported rather than bridged:".to_string(),
        ];
        for gap in GAPS {
            lines.push(format!("    * {gap}"));
        }
        lines
    }
}

/// The seams that are not there, in the words the run prints.
///
/// A constant so that the operator's output and this crate's tests quote the
/// same list, and so that closing one means deleting a line here rather than
/// hoping somebody notices the prose.
pub const GAPS: [&str; 3] = [
    "Platform fixes its broker at construction. There is a set_central and no set_broker, so the \
     kernel's own submit_order can never reach a live venue adapter. Layer 6 submits through the \
     same OrderManager, PreTradeChecker and AutonomyController the platform uses internally.",
    "qip-edge-node's RestGateway already bridges the cell's Placer to the REST venue adapter, and \
     its venue module already holds the refusal that decides whether it may be built. Neither is \
     in [workspace.dependencies], so no other binary can name them; this cell therefore has no \
     gateway and does not trade.",
    "Nothing decodes a cell delta into the central plane's CellReport. qip_mesh::spine says the \
     composition root is where that belongs and the composition root does not do it, so layer 5 \
     prints the delta the centre received rather than inventing the mapping.",
];

fn describe_dispatch(dispatch: &CapitalDispatch) -> String {
    match dispatch {
        CapitalDispatch::Delivered { .. } => "delivered".to_string(),
        CapitalDispatch::Held { .. } => "held; persisted and not yet taken by the cell".to_string(),
        CapitalDispatch::Rejected { last_error, .. } => format!("rejected: {last_error}"),
    }
}

// --- adapter configuration --------------------------------------------------
//
// Each of these points one adapter at the vendor and states the two things the
// adapter cannot infer: what the records may be used for, and how long after an
// event the vendor publishes it. `publication_delay` is zero throughout, so what
// decides whether a record is knowable is the record's own instants rather than
// a constant every line of output would have to carry.

fn price_config(base: &str) -> RestFeedConfig {
    RestFeedConfig {
        name: "demo-vendor-prices".into(),
        provider: "a loopback REST market-data vendor".into(),
        base_url: Some(base.to_string()),
        path: MARKET_DATA_PATH.into(),
        api_key: Some(VENDOR_KEY.into()),
        api_key_header: "x-api-key".into(),
        licensing: LicensingClass::Licensed,
        publication_delay: Duration::ZERO,
        window: Duration::from_days(200),
        max_records: 200,
        http: http_limits(),
    }
}

fn document_config(base: &str) -> NarrativeFeedConfig {
    NarrativeFeedConfig {
        name: "demo-vendor-documents".into(),
        provider: "a loopback document and macro-release vendor".into(),
        base_url: Some(base.to_string()),
        path: NARRATIVE_PATH.into(),
        api_key: Some(VENDOR_KEY.into()),
        api_key_header: "x-api-key".into(),
        // None, not `Internal`: every document in the script states its own
        // terms, and a feed-wide default would override them.
        licensing: None,
        publication_delay: Duration::ZERO,
        window: Duration::from_days(120),
        max_records: 100,
        max_document_bytes: 4096,
        http: http_limits(),
    }
}

fn depth_config(base: &str) -> DepthFeedConfig {
    DepthFeedConfig {
        name: "demo-vendor-depth".into(),
        provider: "a loopback depth vendor".into(),
        base_url: Some(base.to_string()),
        snapshot_path: DEPTH_SNAPSHOT_PATH.into(),
        updates_path: DEPTH_UPDATES_PATH.into(),
        api_key: Some(VENDOR_KEY.into()),
        api_key_header: "x-api-key".into(),
        licensing: LicensingClass::Licensed,
        publication_delay: Duration::ZERO,
        http: http_limits(),
        ..DepthFeedConfig::default()
    }
}

fn alternative_config(base: &str) -> AlternativeFeedConfig {
    AlternativeFeedConfig {
        name: "demo-vendor-alternative".into(),
        provider: "a loopback alternative-data vendor".into(),
        base_url: Some(base.to_string()),
        path: ALTERNATIVE_PATH.into(),
        api_key: Some(VENDOR_KEY.into()),
        api_key_header: "x-api-key".into(),
        // The reading states its own class, which for alternative data is the
        // shape a vendor actually has.
        licensing: None,
        publication_delay: Duration::ZERO,
        window: Duration::from_days(30),
        http: http_limits(),
        ..AlternativeFeedConfig::default()
    }
}

fn venue_config(base: &str) -> RestVenueConfig {
    RestVenueConfig {
        base_url: Some(base.to_string()),
        orders_path: ORDERS_PATH.into(),
        health_path: HEALTH_PATH.into(),
        http: http_limits(),
        ..RestVenueConfig::default()
    }
}
