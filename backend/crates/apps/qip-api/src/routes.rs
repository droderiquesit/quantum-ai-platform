//! The versioned REST API.
//!
//! Every path is under `/api/v1`. Versioning the whole surface rather than
//! individual resources means a breaking change is one decision rather than a
//! negotiation per endpoint, and a client can pin to a version it understands.
//!
//! Authorisation is per route and stated in the table, not scattered through
//! the handlers. That makes it possible to read what the API permits without
//! reading what it does — which is the question a security review asks.

use crate::auth::{Authenticator, Principal, RateLimiter, Role};
use crate::cells::{CellRegistry, describe_age};
use crate::http::{Handler, Method, Request, Response, StreamDecision};
use crate::json;
use crate::stream::{
    EventSource, EventStream, HealthPulse, LAST_EVENT_ID, LoggedEvents, PlatformHealth, StreamKind,
    StreamLimits,
};
use qip_core::time::Timestamp;
use qip_kernel::Platform;
use qip_storage::ChainArchive;
use std::sync::{Arc, Mutex};

/// The prefix every route in the table sits under.
///
/// A constant rather than a literal in three places: the router, the discovery
/// endpoint and the generated OpenAPI document all have to agree about where
/// the API lives, and two of them agreeing is not enough.
pub const VERSION_PREFIX: &str = "/api/v1";

/// Where the unauthenticated route listing is served.
pub const DISCOVERY_PATH: &str = VERSION_PREFIX;

/// Where the generated OpenAPI document is served.
pub const OPENAPI_PATH: &str = "/api/v1/openapi.json";

/// The one route that answers in Prometheus text exposition rather than JSON.
///
/// Named once so the handler and the generated OpenAPI document cannot come to
/// different conclusions about the media type. A document that promises JSON
/// where the server sends `# HELP` produces a client that reports the endpoint
/// broken, and the endpoint is the one an operator reaches for when they
/// already believe something is broken.
pub const SCRAPE_PATH: &str = "/metrics";

/// One route's declaration.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    pub method: Method,
    /// Path pattern under `/api/v1`, with `:name` for a parameter.
    pub pattern: &'static str,
    /// The least authority that may call it.
    pub required_role: Role,
    /// What it does, for the discovery endpoint and for a review.
    pub summary: &'static str,
    /// The status a successful call returns.
    ///
    /// In the table rather than in the OpenAPI generator, because the
    /// generator reads this table and nothing else. A second list of status
    /// codes kept beside it is a list that will disagree with the handlers
    /// one release from now, and a document that says `200` where the handler
    /// answers `202` is worse than one that says nothing.
    pub success: u16,
}

/// Every route the API exposes.
///
/// Written out so the whole surface, and what each part requires, can be read
/// in one place. Anything not in this table is a 404.
pub const ROUTES: &[Route] = &[
    Route {
        method: Method::Get,
        pattern: "/health",
        required_role: Role::Monitor,
        summary: "liveness, and whether the platform is halted",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/system/status",
        required_role: Role::Viewer,
        summary: "autonomy level, kill switch, cycle count",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/system/metrics",
        required_role: Role::Monitor,
        summary: "counters and gauges",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/system/governance",
        required_role: Role::Viewer,
        summary: "the agent roster and its governance findings",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/mesh",
        required_role: Role::Viewer,
        summary: "the mesh backbone: cells served, deltas absorbed, envelopes dispatched, inbox depth",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/portfolio",
        required_role: Role::Viewer,
        summary: "positions, exposures and equity",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/opportunities",
        required_role: Role::Viewer,
        summary: "the current opportunity queue",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/proposals",
        required_role: Role::Viewer,
        summary: "proposals and their status",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/orders",
        required_role: Role::Viewer,
        summary: "orders, fills, refusals and any venue/book disagreement",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/agents",
        required_role: Role::Viewer,
        summary: "the agent roster and each agent's manifest",
        success: 200,
    },
    Route {
        method: Method::Post,
        pattern: "/cycle",
        required_role: Role::Analyst,
        summary: "run one cycle of the intelligence loop",
        success: 202,
    },
    Route {
        method: Method::Post,
        pattern: "/kill-switch",
        required_role: Role::Operator,
        summary: "halt the platform",
        success: 200,
    },
    Route {
        method: Method::Delete,
        pattern: "/kill-switch",
        required_role: Role::Operator,
        summary: "clear a halt",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/autonomy",
        required_role: Role::Viewer,
        summary: "the current autonomy level and ceiling",
        success: 200,
    },
    // --- the console's read surface -----------------------------------------
    //
    // One route per panel group the operator console renders, so anything the
    // console shows can also be read as JSON by something that is not a
    // browser. Several of them have nothing behind them in this deployment and
    // say so in the response rather than returning a fabricated shape; which
    // ones, and why, is written down once in `crate::missing`.
    Route {
        method: Method::Get,
        pattern: "/system",
        required_role: Role::Viewer,
        summary: "autonomy, halt state, cycle count and the integrity of the event log",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: SCRAPE_PATH,
        required_role: Role::Monitor,
        summary: "what the platform recorded, in Prometheus text exposition, for a scrape that \
                  holds no portfolio authority",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/regions",
        required_role: Role::Viewer,
        summary: "every edge cell that has reported, its book and how old the report is",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/markets",
        required_role: Role::Viewer,
        summary: "market state, where the desk's capability gate permits reading it",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/assets",
        required_role: Role::Viewer,
        summary: "the reference universe, where the desk's capability gate permits reading it",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/arbitrage",
        required_role: Role::Viewer,
        summary: "active three-arm and N-leg paths, their capital and their hedge state",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/strategies",
        required_role: Role::Viewer,
        summary: "registered strategies, their stage on the ladder and whether they hold capital",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/models",
        required_role: Role::Viewer,
        summary: "models in use, their cost, and what the agents actually spent",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/capital",
        required_role: Role::Viewer,
        summary: "allocation bounds, issued envelopes and outstanding recalls",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/risk",
        required_role: Role::Viewer,
        summary: "exposure, concentration, the kill switch, and what is not measurable here",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/fills",
        required_role: Role::Viewer,
        summary: "every fill, and whether it came from a simulated venue",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/pnl",
        required_role: Role::Viewer,
        summary: "profit, loss and realised against expected alpha",
        success: 200,
    },
    // The summary says what the handler returns and not what a data-source
    // surface would return, because the handler matches one expression and
    // answers `available: false` unconditionally: no deployment of this
    // process has ever served a source list, a health record, a freshness
    // instant or a quality score. The previous summary — "discovered,
    // approved and rejected data sources with health and licensing" —
    // described an intent, and a console built against the route table was
    // promising four fields no code path here can produce.
    Route {
        method: Method::Get,
        pattern: "/data-sources",
        required_role: Role::Viewer,
        summary: "why no source list is served: discovery, approval and rejection are the data \
                  finder's and this deployment runs none, so the body is that refusal with the \
                  reason and never an empty list. What each venue demands before it is read is \
                  on /registrations",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/training",
        required_role: Role::Viewer,
        summary: "training runs and their status",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/quantum",
        required_role: Role::Viewer,
        summary: "submitted quantum jobs and the classical run each is compared against",
        success: 200,
    },
    // --- the research read surface -------------------------------------------
    //
    // What the REASON and LEARN stages hold and the lifecycle ledger records,
    // for the portal pages that render them. The same rule as above: a route
    // is served from the platform's own state or it says which subsystem is
    // absent, and a computed figure carries the window and the cycle it was
    // computed at so it can be reproduced against the process that served it.
    Route {
        method: Method::Get,
        pattern: "/predictions",
        required_role: Role::Viewer,
        summary: "every falsifiable claim the platform holds, per instrument, with the belief \
                  calibration LEARN has computed",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/regimes",
        required_role: Role::Viewer,
        summary: "the regime classification per instrument, where a classifier runs",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/correlation",
        required_role: Role::Viewer,
        summary: "pairwise return correlation over the platform's own price tape, with the \
                  window and cycle it was computed at",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/backtests",
        required_role: Role::Viewer,
        summary: "per-strategy holdout evidence, gate findings and the band each admission \
                  produced, as the lifecycle ledger recorded them",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/news",
        required_role: Role::Viewer,
        summary: "narrative items the platform absorbed, where an adapter feeds it",
        success: 200,
    },
    // --- the cognition read surface ------------------------------------------
    //
    // What the LEARN stage has measured of the platform's own components and
    // the precedent REASON recorded beside each hypothesis. Read-only at the
    // viewer role: a self-estimate is portfolio reasoning, not liveness, and
    // a monitor credential holds no authority over it. An accuracy appears
    // only where the engine computed one; below its minimum sample the body
    // says `null`, never a number. The shapes are `crate::self_model_views`
    // and `ROUTES-COGNITION.md`.
    Route {
        method: Method::Get,
        pattern: "/cognition/self-model",
        required_role: Role::Viewer,
        summary: "every component the platform has graded (detector, analyst, rung or \
                  strategy) with its sample count and, where the sample reaches the stated \
                  minimum, the engine's estimated accuracy",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/cognition/precedents",
        required_role: Role::Viewer,
        summary: "the precedent recorded beside each hypothesis: the nearest resolved \
                  episodes REASON recalled and how their outcomes sat against the claim, \
                  oldest first",
        success: 200,
    },
    // --- the treasury read surface -------------------------------------------
    //
    // The blueprint's per-account entitlements and wallet, corridor and
    // transfer-gate views, read-only, and one operator route beside them.
    // ADR 0021 permits the deterministic half of the treasury and refuses the
    // path by which capital leaves, so there is no route here that could
    // submit, approve, sign or move anything; `api_boundary.rs` pins the
    // whole mutating set, and the one addition here — an eligibility
    // decision — records a precondition the ledger checks *before* a
    // funding, which is the opposite of a transfer. Each body is read off
    // the kernel's fabric journal — the wallet
    // it last assembled, the registries it holds, the newest gate
    // assessment — and what the journal does not yet hold (a wallet, before
    // a statement and a cycle) is stated in the body, not zero-filled. The
    // shapes are `crate::ledger_views` and `ROUTES-LEDGER.md`.
    //
    // `/ledger/users` is the one route of the four that requires an analyst.
    // Its body carries every enrolled user's mandate, balances and inflow
    // references, and the portal grants the viewer role to anyone who
    // completes self-registration on the public front door — so at viewer
    // the route would hand every user's capital to whoever could sign up.
    // The other three carry no per-user datum (the desk's wallet, the
    // registries, the gate's checks and the kill switch) and stay readable
    // by a viewer.
    Route {
        method: Method::Get,
        pattern: "/ledger/users",
        required_role: Role::Analyst,
        summary: "every user in the mandate registry: mandate, the per-strategy balances the \
                  pro-rata booking moved with expected inflows kept apart, and the viewer-role \
                  entitlement evaluation, in which withdrawal is never granted",
        success: 200,
    },
    // The one route on this surface that changes anything, and an operator's.
    // Until it existed, an eligibility could be decided only from the
    // deployment's committed configuration or from a test, so the gate the
    // ledger runs before every funding could be moved by nobody a running
    // process could name. It raises the kernel's typed intent
    // `Platform::decide_eligibility`, which takes the same `OperatorIdentity`
    // an autonomy change does, journals the decision before the registry
    // adopts it, and refuses a credential older than the kernel accepts. It
    // moves no capital: an eligibility is a precondition of a funding.
    Route {
        method: Method::Post,
        pattern: "/ledger/users/:user/eligibility",
        required_role: Role::Operator,
        summary: "record the authenticated operator's eligibility decision for one user — \
                  granted on stated terms, or revoked with a reason — journalled before the \
                  registry adopts it and answered with that user's updated ledger row",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/wallet",
        required_role: Role::Viewer,
        summary: "the wallet the fabric journal last assembled and its reconciliation \
                  outcomes, or that none is assembled yet",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/corridors",
        required_role: Role::Viewer,
        summary: "the corridor registry and destination allowlist the fabric journal holds, \
                  as records with stage and caps",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/transfer-gate",
        required_role: Role::Viewer,
        summary: "the transfer gate's seven checks in order, the newest assessment the \
                  fabric journal holds (or null) and the kill switch its seventh check reads",
        success: 200,
    },
    // --- venue registrations ------------------------------------------------
    //
    // The platform does everything about a venue registration except the
    // part that must be a person's. The list names, per catalogued source,
    // what the venue demands, where the source stands and the terms to read;
    // the approval records that an operator did those things, under the
    // operator's own authenticated subject. The fourth mutating route, and
    // the typed intent it raises is `Platform::approve_registration`, which
    // takes the same `OperatorIdentity` an autonomy change does and refuses
    // a body whose secret looks like a key before the kernel sees it.
    // Nothing a caller sends becomes a credential: the body carries a
    // variable name, screened by the manifest's own shape rule, and the
    // value stays in Secret Manager where the operator put it. The shapes
    // are `crate::registration_views` and `ROUTES-REGISTRATIONS.md`.
    //
    // Two read routes rather than one whose body varies by caller. The slot
    // a credential is read under, and the `gcloud` line that writes one, are
    // facts about this deployment's secret store rather than about a venue,
    // and they were being served to a viewer credential — reproduced on the
    // wire before this split existed. Splitting them onto their own
    // operator-role route keeps this table a true statement of what the API
    // permits: a `Role::Viewer` row that served operator material to an
    // operator would mean a security review had to read the handlers, which
    // is exactly what the table exists to avoid.
    Route {
        method: Method::Get,
        pattern: "/registrations",
        required_role: Role::Viewer,
        summary: "every catalogued source with its registration requirement, standing \
                  (keyless, registered by whom, or pending and why) and the terms to read — \
                  and no credential slot: the deployment variable and the Secret Manager \
                  command that fills it are on /registrations/slots, at the operator role",
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/registrations/slots",
        required_role: Role::Viewer,
        summary: "the same list with the credential slots beside each source: the deployment \
                  variable the connector manifest reads the credential under, the one Secret \
                  Manager command that puts a version behind it, any companion variable, and \
                  the variable the registration record itself names — names only, never a \
                  value",
        success: 200,
    },
    Route {
        method: Method::Post,
        pattern: "/registrations/:source/approve",
        required_role: Role::Operator,
        summary: "record that the authenticated operator registered with the venue, read the \
                  terms cited in the body and put the credential in Secret Manager under the \
                  variable the body names; journalled before it stands, answered with the \
                  source's new standing",
        success: 200,
    },
    // --- the live surface ---------------------------------------------------
    //
    // In the same table as everything else, so a security review reads one
    // list rather than two. A stream is held open for minutes and carries the
    // same data the equivalent REST route serves, so it requires the same
    // authority: `/stream/orders` is `/orders` over time and is not a lower
    // bar than it. What each stream carries, where it comes from and what a
    // reconnect recovers is in `crate::stream::StreamKind`.
    Route {
        method: Method::Get,
        pattern: "/stream/market",
        required_role: Role::Viewer,
        summary: StreamKind::Market.summary(),
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/stream/signals",
        required_role: Role::Viewer,
        summary: StreamKind::Signals.summary(),
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/stream/orders",
        required_role: Role::Viewer,
        summary: StreamKind::Orders.summary(),
        success: 200,
    },
    Route {
        method: Method::Get,
        pattern: "/stream/positions",
        required_role: Role::Viewer,
        summary: StreamKind::Positions.summary(),
        success: 200,
    },
    // The one stream a monitoring credential may read, matching `/health`:
    // liveness and the halt state are what a monitor is for, and they carry
    // no portfolio detail.
    Route {
        method: Method::Get,
        pattern: "/stream/health",
        required_role: Role::Monitor,
        summary: StreamKind::Health.summary(),
        success: 200,
    },
];

/// The API surface.
pub struct Api {
    platform: Arc<Mutex<Platform>>,
    authenticator: Arc<Authenticator>,
    rate_limiter: Arc<RateLimiter>,
    /// The clock the API reads. Injected so a test and a deployment differ
    /// only in which clock they pass.
    clock: Arc<dyn qip_core::Clock>,
    /// When each edge cell last reported.
    ///
    /// The central plane absorbs a cell report into its aggregate and keeps no
    /// arrival time, so without this `/regions` could only say what the cells
    /// hold and never how old that is. Shared with the operator console
    /// through [`Api::with_cells`] so the page and the JSON behind it cannot
    /// disagree about which cells are stale.
    cells: Arc<CellRegistry>,
    /// Where the event log's hash chain is kept across restarts.
    ///
    /// `None` means this instance archives nothing. That is not a fallback the
    /// server can reach: `qip-api`'s `main` resolves a store from the
    /// environment and refuses to start if the configuration does not describe
    /// one it can write to, so a running deployment always has an archive. The
    /// `None` case exists for tests and embedders that assemble an `Api`
    /// directly and have decided in their own code, rather than by
    /// configuration, that nothing should persist.
    archive: Option<Arc<ChainArchive>>,
    /// The mesh backbone's central half, where one is configured.
    ///
    /// `None` means this process serves no mesh, and `/mesh` says so in the
    /// words of `crate::missing` rather than with an empty status. Behind a
    /// mutex of its own because the drain needs the platform and the mesh
    /// together while dispatch must run with the platform lock released —
    /// the lock order is always platform first, then mesh, and no path takes
    /// them the other way around.
    mesh: Option<Arc<Mutex<crate::mesh::MeshBackbone>>>,
    /// Where the operator interface reads the last cycle's stages from.
    ///
    /// `None` when no interface is mounted alongside this API. When one is,
    /// this is the only writer: the interface never runs a cycle itself, and
    /// an API assembled without the handle leaves its overview empty for the
    /// life of the process — the defect this field exists to close.
    overview: Option<Arc<crate::web::CycleOverview>>,
    /// The source `POST /cycle` senses before it runs the loop.
    ///
    /// `None` is a process that senses nothing — the shipped state, and the
    /// cycle report carries no `sense` key for it rather than a zero. Behind
    /// a mutex of its own because a tape advances and a connector polls, and
    /// the lock order is always platform first, then feed; no path takes
    /// them the other way around. Records in transit from a source to the
    /// platform, not state the API keeps.
    feed: Option<Arc<Mutex<crate::feed::ApiFeed>>>,
    /// Where a connector credential slot is looked up when a registration is
    /// approved at runtime, or `None` to read the process environment at the
    /// instant of the lookup.
    ///
    /// `None` is what a deployment runs: reading the environment then rather
    /// than at start-up is what lets a secret mounted after the process came
    /// up be seen at all. The map exists because a test cannot put a
    /// `_FILE` variable into the process it runs in — `std::env::set_var` is
    /// unsafe in the 2024 edition and `unsafe` is forbidden at the workspace
    /// root — which is the same reason `FeedSettings::parse` takes a map
    /// rather than reading the environment. It holds variable *names* the
    /// caller supplied; no credential value is ever stored on this type.
    credential_variables: Option<std::collections::BTreeMap<String, String>>,
    /// The bounds every live connection runs under.
    ///
    /// Held here rather than read from a constant so a test can open a stream
    /// that ends in milliseconds instead of minutes. A test that had to wait
    /// out the production lifetime bound to prove the connection ends is a
    /// test nobody runs.
    stream_limits: StreamLimits,
    /// The health reading every subscriber to `/stream/health` shares.
    ///
    /// Shared so two dashboards watching the same process agree about which
    /// transition they are looking at; see [`HealthPulse`].
    pulse: Arc<HealthPulse>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("routes", &ROUTES.len())
            .field("credentials", &self.authenticator.credential_count())
            .finish_non_exhaustive()
    }
}

impl Api {
    pub fn new(
        platform: Arc<Mutex<Platform>>,
        authenticator: Arc<Authenticator>,
        rate_limiter: Arc<RateLimiter>,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Self {
        Self {
            platform,
            authenticator,
            rate_limiter,
            clock,
            cells: Arc::new(CellRegistry::default()),
            archive: None,
            mesh: None,
            overview: None,
            feed: None,
            credential_variables: None,
            stream_limits: StreamLimits::default(),
            pulse: Arc::new(HealthPulse::default()),
        }
    }

    /// Run live connections under `limits` rather than the defaults.
    ///
    /// The defaults are chosen for a browser behind a reverse proxy. A test
    /// needs a connection that heartbeats and closes in milliseconds, and an
    /// embedder behind a proxy with a shorter idle timeout than the usual
    /// sixty seconds needs a shorter heartbeat than the usual ten.
    pub fn with_stream_limits(mut self, limits: StreamLimits) -> Self {
        self.stream_limits = limits;
        self
    }

    /// Serve the mesh backbone this process assembled.
    ///
    /// With it, `POST /cycle` drains the cell-delta inbox into the platform
    /// and dispatches the plane's capital envelopes after each cycle, and
    /// `/mesh` reports what the backbone has done. Without it the API still
    /// serves everything else and says honestly that no mesh is served.
    pub fn with_mesh(mut self, mesh: Arc<Mutex<crate::mesh::MeshBackbone>>) -> Self {
        self.mesh = Some(mesh);
        self
    }

    /// Sense `feed` into the platform at the start of every `POST /cycle`.
    ///
    /// Without this the loop runs over a platform nothing observes into,
    /// which is what every cycle this process ever ran did until the
    /// composition root began choosing a source. The platform must have been
    /// assembled on the feed's own clock where it owns one — the root does
    /// that, and a feed handed here after a platform built on the wall clock
    /// would observe last year while reasoning about today.
    pub fn with_feed(mut self, feed: Arc<Mutex<crate::feed::ApiFeed>>) -> Self {
        self.feed = Some(feed);
        self
    }

    /// Look a connector credential slot up in `variables` rather than in the
    /// process environment.
    ///
    /// For a test, which cannot write the environment of the process it runs
    /// in: `std::env::set_var` is unsafe in the 2024 edition and this
    /// workspace forbids `unsafe`, so the only way to exercise the
    /// re-admission's `_FILE` indirection is to hand it the variables. A
    /// deployment never calls this and reads the environment, which is what
    /// lets a secret mounted after start-up be found.
    pub fn with_credential_variables(
        mut self,
        variables: std::collections::BTreeMap<String, String>,
    ) -> Self {
        self.credential_variables = Some(variables);
        self
    }

    /// The two places a credential slot can be set: the variable itself and
    /// its `_FILE` projection.
    ///
    /// Read here and resolved in [`crate::feed::credential_is_readable`], so
    /// the `_FILE` rule has exactly one implementation and this only decides
    /// *where* to look.
    fn credential_sources(&self, slot: &str) -> (Option<String>, Option<String>) {
        let file = format!("{slot}{}", qip_core::secret::FILE_SUFFIX);
        match &self.credential_variables {
            Some(variables) => (variables.get(slot).cloned(), variables.get(&file).cloned()),
            None => (std::env::var(slot).ok(), std::env::var(&file).ok()),
        }
    }

    /// Re-open this process's connector on a registration just approved.
    ///
    /// The failure this closes: the feed's admission gate runs once, at
    /// start-up, against the registry the deployment's configuration stood
    /// for. An operator who approved a registration through the API moved
    /// the registry and the event log and nothing else — the process went on
    /// refusing the source it had refused at boot, and the answer said the
    /// source was registered, which is true and was read as "and is now
    /// being read", which was not.
    ///
    /// `None` is "there is nothing here to re-open": no feed, a tape, or a
    /// connector for some other source. That is not a failure and is not
    /// reported as one.
    ///
    /// The feed lock is taken with the platform lock already held, which is
    /// the order `POST /cycle` takes them in and the only order any path in
    /// this file takes them in. Re-opening touches a socket under both, as
    /// the cycle's own SENSE does; what it must never do is leave the
    /// process with no feed, so the replacement happens inside
    /// [`crate::feed::ApiFeed::readmit`] only after the new connector is
    /// built.
    fn readmit_connector(
        &self,
        platform: &Platform,
        source_id: &str,
        slot: &str,
        now: Timestamp,
    ) -> Option<crate::registration_views::ConnectorAdmissionView> {
        use crate::registration_views::ConnectorAdmissionView as Admission;
        let feed = self.feed.as_ref()?;
        let Ok(mut feed) = feed.lock() else {
            return Some(Admission::refused(
                "the feed is in an inconsistent state, so the connector was not re-opened. The \
                 registration stands and is in the log; restart the process to sense through it",
            ));
        };
        if feed.connector_source() != Some(source_id) {
            return None;
        }
        // Asked before the gate, because a source admitted onto a feed whose
        // credential this process cannot read would fail at the vendor with
        // an error naming neither the variable nor the record.
        let (direct, path) = self.credential_sources(slot);
        if let Err(refusal) = crate::feed::credential_is_readable(slot, direct, path) {
            return Some(Admission::refused(refusal.message()));
        }
        match feed.readmit(platform.registrations(), now) {
            Ok(()) => Some(Admission::admitted(feed.describe())),
            Err(refusal) => Some(Admission::refused(refusal.message())),
        }
    }

    /// Read cell reports from a registry shared with the operator console.
    ///
    /// Without this the API has its own empty registry and `/regions` answers
    /// honestly that no cell has reported to it — which is true of an API that
    /// nothing feeds, and is why the default is an empty registry rather than
    /// a missing one.
    pub fn with_cells(mut self, cells: Arc<CellRegistry>) -> Self {
        self.cells = cells;
        self
    }

    /// Record each cycle's stages where the operator interface reads them.
    ///
    /// Without this the interface's stage overview stays empty after every
    /// cycle, which an operator cannot tell from a process that never cycled.
    pub fn with_cycle_overview(mut self, overview: Arc<crate::web::CycleOverview>) -> Self {
        self.overview = Some(overview);
        self
    }

    /// Archive the event log's hash chain into `archive` as cycles run.
    ///
    /// The platform's own log is in memory and begins again at every start, so
    /// without this the audit trail is only ever as long as the current
    /// process has been up — and the one question an incident review asks is
    /// what happened before the restart.
    pub fn with_archive(mut self, archive: Arc<ChainArchive>) -> Self {
        self.archive = Some(archive);
        self
    }

    /// Hand the platform's log to the archive, once, at a cycle boundary.
    ///
    /// The hand-over happens here rather than inside the log's append, so a
    /// disk never sits on the path of an individual event. What that costs is
    /// the events of a cycle that was interrupted; what it buys is a decision
    /// loop whose latency is not a storage system's problem. `None` is a
    /// process with no archive configured, which the cycle response reports
    /// as such rather than as zero records written.
    ///
    /// A failure is reported — in the response and on stderr — and never
    /// propagated: the cycle already happened, so failing the request would be
    /// a lie about what the platform did, and an archive that stopped
    /// accepting records is an incident that a silent failure would hide.
    fn archive_from(&self, platform: &Platform) -> Option<std::result::Result<usize, String>> {
        let archived = self.archive.as_ref().map(|archive| {
            archive
                .absorb(platform.event_log().records())
                .map_err(|error| error.message().to_string())
        });
        if let Some(Err(reason)) = &archived {
            eprintln!("qip-api: the event chain could not be archived: {reason}");
        }
        archived
    }

    /// The registry of cell reports this API serves `/regions` from.
    ///
    /// Exposed so the router can hand the same registry to the operator
    /// interface: a page and the JSON behind it must not disagree about which
    /// cells have reported.
    pub fn cells(&self) -> &CellRegistry {
        &self.cells
    }

    /// The mesh backbone, where one is served.
    ///
    /// For the router to pass to the operator interface, which reads each
    /// cell's last standing off it. `None` is answered on the page in words,
    /// the same as `/mesh` answers it.
    pub fn mesh(&self) -> Option<&Mutex<crate::mesh::MeshBackbone>> {
        self.mesh.as_deref()
    }

    /// The mesh backbone's status as JSON, `None` when none is configured.
    ///
    /// A poisoned backbone lock still answers — with the fact of its own
    /// inconsistency — because `/mesh` claiming "not served" about a mesh
    /// that is bound and broken would send an operator to the wrong page.
    fn mesh_status_json(&self) -> Option<String> {
        let mesh = self.mesh.as_ref()?;
        Some(match mesh.lock() {
            Ok(mesh) => serde_json::to_string(&mesh.status()).unwrap_or_else(|error| {
                format!(r#"{{"error":{}}}"#, json::string(&error.to_string()))
            }),
            Err(_) => r#"{"served":true,"error":"the mesh backbone is in an inconsistent state"}"#
                .to_string(),
        })
    }

    /// Find the route matching a request, if any.
    pub fn route_for(method: Method, path: &str) -> Option<&'static Route> {
        let suffix = path.strip_prefix(VERSION_PREFIX)?;
        let suffix = if suffix.is_empty() { "/" } else { suffix };
        ROUTES
            .iter()
            .find(|route| route.method == method && matches_pattern(route.pattern, suffix))
    }

    /// Authenticate, charge the allowance, and check the role.
    ///
    /// One ladder, called from both the buffered path and the streaming one,
    /// so a stream cannot end up authorised by a check the REST surface does
    /// not make — or, worse, by no check at all. Returns the refusal rather
    /// than a bare error so each caller writes the same body a caller of the
    /// equivalent REST route would receive.
    fn admit(&self, request: &Request, route: &Route) -> std::result::Result<Principal, Response> {
        let now = self.clock.now();
        let principal = match self
            .authenticator
            .authenticate(request.header("authorization"), now)
        {
            Ok(principal) => principal,
            Err(error) => {
                return Err(Response::json(
                    401,
                    format!(r#"{{"error":{}}}"#, json::string(error.message())),
                )
                .with_header("www-authenticate", "Bearer"));
            }
        };

        if !self.rate_limiter.permit(&principal.subject, now) {
            return Err(Response::json(429, r#"{"error":"rate limit exceeded"}"#));
        }

        if let Err(error) = principal.require(route.required_role) {
            return Err(Response::json(
                403,
                format!(r#"{{"error":{}}}"#, json::string(error.message())),
            ));
        }
        Ok(principal)
    }

    fn dispatch(&self, request: &Request, principal: &Principal, route: &Route) -> Response {
        let now = self.clock.now();
        let Ok(mut platform) = self.platform.lock() else {
            // A poisoned lock means a thread panicked while holding the
            // platform. Serving from it could produce inconsistent state, so
            // the honest answer is that the service is unavailable.
            return Response::json(
                503,
                r#"{"error":"the platform is in an inconsistent state and is not serving"}"#,
            );
        };

        match (route.method, route.pattern) {
            (Method::Get, "/health") => Response::json(200, health(&platform)),
            (Method::Get, "/system/status") => {
                let mesh = self.mesh_status_json();
                Response::json(
                    200,
                    status(&platform, self.archive.as_deref(), mesh.as_deref()),
                )
            }
            (Method::Get, "/mesh") => match self.mesh_status_json() {
                Some(mesh) => Response::json(200, mesh),
                None => Response::json(200, unavailable("mesh", crate::missing::MESH_NOT_SERVED)),
            },
            (Method::Get, "/system/metrics") => Response::json(200, metrics(&platform)),
            (Method::Get, "/system/governance") => Response::json(200, governance(&platform, now)),
            (Method::Get, "/portfolio") => Response::json(200, portfolio(&platform)),
            (Method::Get, "/opportunities") => Response::json(200, opportunities(&platform)),
            (Method::Get, "/proposals") => Response::json(200, proposals(&platform)),
            (Method::Get, "/orders") => Response::json(200, orders(&platform)),
            (Method::Get, "/agents") => Response::json(200, agents(&platform)),
            (Method::Get, "/autonomy") => Response::json(200, autonomy(&platform)),
            (Method::Get, "/system") => Response::json(200, system(&platform)),
            (Method::Get, "/metrics") => Response::text(200, scrape(&platform)),
            (Method::Get, "/regions") => {
                Response::json(200, regions(&platform, self.cells.as_ref(), now))
            }
            (Method::Get, "/markets") => {
                Response::json(200, unavailable("markets", crate::missing::DESK_GATED))
            }
            (Method::Get, "/assets") => {
                Response::json(200, unavailable("assets", crate::missing::DESK_GATED))
            }
            (Method::Get, "/arbitrage") => Response::json(
                200,
                unavailable("paths", crate::missing::NO_ARBITRAGE_ENGINE),
            ),
            (Method::Get, "/strategies") => Response::json(200, strategies(&platform)),
            (Method::Get, "/models") => Response::json(200, models(&platform)),
            (Method::Get, "/capital") => Response::json(200, capital(&platform, now)),
            (Method::Get, "/risk") => Response::json(200, risk(&platform, self.cells.as_ref())),
            (Method::Get, "/fills") => Response::json(200, fills(&platform)),
            (Method::Get, "/pnl") => Response::json(
                200,
                unavailable("attribution", crate::missing::NO_ATTRIBUTION),
            ),
            (Method::Get, "/data-sources") => {
                Response::json(200, unavailable("sources", crate::missing::NO_DATA_FINDER))
            }
            (Method::Get, "/training") => Response::json(
                200,
                unavailable("runs", crate::missing::NO_TRAINING_SERVICE),
            ),
            (Method::Get, "/quantum") => Response::json(200, quantum(&platform)),
            (Method::Get, "/predictions") => Response::json(200, predictions(&platform)),
            (Method::Get, "/regimes") => Response::json(200, regimes()),
            (Method::Get, "/correlation") => Response::json(200, correlation(&platform)),
            (Method::Get, "/backtests") => Response::json(200, backtests(&platform)),
            (Method::Get, "/news") => Response::json(
                200,
                unavailable("news", crate::missing::NO_NARRATIVE_ADAPTER),
            ),
            // The cognition read surface, rendered the same way as the
            // treasury below. The self-model view is the one that can refuse
            // — its stated minimum sample is checked against the engine's
            // behaviour on every row — and a refusal is a 500 naming the
            // drift, not a body a page would misexplain.
            (Method::Get, "/cognition/self-model") => {
                match crate::self_model_views::self_model(&platform) {
                    Ok(view) => {
                        let (status, body) = crate::ledger_views::render(&view);
                        Response::json(status, body)
                    }
                    Err(reason) => {
                        Response::json(500, format!(r#"{{"error":{}}}"#, json::string(&reason)))
                    }
                }
            }
            (Method::Get, "/cognition/precedents") => {
                let (status, body) =
                    crate::ledger_views::render(&crate::self_model_views::precedents(&platform));
                Response::json(status, body)
            }
            // The treasury read surface. Each body is a `Serialize` view
            // built from what the platform holds at this instant; a view
            // that does not serialise answers 500 with the reason rather
            // than panicking under the lock every other route waits on.
            (Method::Get, "/ledger/users") => {
                let (status, body) = crate::ledger_views::render_fallible(
                    crate::ledger_views::ledger_users(&platform, now),
                );
                Response::json(status, body)
            }
            (Method::Post, "/ledger/users/:user/eligibility") => {
                // The user is the path's third segment under the prefix; the
                // route matched, so it is present.
                let Some(user) = path_segment(&request.path, 2) else {
                    return Response::json(404, r#"{"error":"no such route"}"#);
                };
                let decided = match request
                    .body_as_str()
                    .map_err(|error| error.message().to_string())
                    .and_then(crate::ledger_views::EligibilityRequest::parse)
                {
                    Ok(decided) => decided,
                    Err(reason) => {
                        return Response::json(400, crate::registration_views::refusal(&reason));
                    }
                };
                // The user is taken from the mandate registry rather than
                // built from the path. Eligibility is a statement about a
                // mandate holder — the ledger's own first refusal says so —
                // and a decision recorded against a user nobody enrolled
                // would be a record `/ledger/users` never shows and nobody
                // would find again.
                let Some(held) = platform
                    .user_ledger()
                    .mandates()
                    .keys()
                    .find(|held| held.as_str() == user)
                    .cloned()
                else {
                    return Response::json(
                        404,
                        crate::registration_views::refusal(&format!(
                            "no mandate is registered for `{user}`, and eligibility is a \
                             statement about a mandate holder; enrol the mandate first"
                        )),
                    );
                };
                let decision = match decided.decision() {
                    Ok(decision) => decision,
                    Err(reason) => {
                        return Response::json(400, crate::registration_views::refusal(&reason));
                    }
                };
                // The operator is the authenticated principal, as clearing
                // the kill switch establishes one — and dated at the instant
                // the *credential* was issued rather than at `now`. The
                // kernel refuses a decision taken on a credential older than
                // fifteen minutes, and an identity stamped `now` is fresh by
                // construction: the check would then be a control that
                // cannot fire, which reads as protection and is not. What it
                // costs is that a long-lived deployment token stops being
                // able to decide eligibility fifteen minutes after the
                // process started, which is the kernel's rule actually
                // applied rather than defeated.
                let operator = qip_risk_engine::autonomy::OperatorIdentity::verified(
                    principal.subject.clone(),
                    "api-bearer-token",
                    principal.issued_at,
                );
                match platform.decide_eligibility(&held, decision, &operator, decided.reason(), now)
                {
                    Ok(_) => {
                        // The row is read back from the ledger, not built
                        // from the request: what an operator sees is what the
                        // registry adopted.
                        let (status, body) = crate::ledger_views::render_fallible(
                            crate::ledger_views::decided_eligibility(&platform, user, now),
                        );
                        Response::json(status, body)
                    }
                    // The same mapping the venue approval answers with, from
                    // the same place, so the two operator routes refuse in
                    // one grammar: the caller's to fix is 400, an identity
                    // the kernel will not act on is 409.
                    Err(error) => Response::json(
                        crate::registration_views::refusal_status(&error),
                        crate::registration_views::refusal(error.message()),
                    ),
                }
            }
            (Method::Get, "/wallet") => {
                let (status, body) = crate::ledger_views::render_fallible(
                    crate::ledger_views::wallet(&platform, now),
                );
                Response::json(status, body)
            }
            (Method::Get, "/corridors") => {
                let (status, body) = crate::ledger_views::render_fallible(
                    crate::ledger_views::corridors(&platform, now),
                );
                Response::json(status, body)
            }
            (Method::Get, "/transfer-gate") => {
                let (status, body) = crate::ledger_views::render_fallible(
                    crate::ledger_views::transfer_gate(&platform, now),
                );
                Response::json(status, body)
            }
            // The viewer's list: requirement, standing, terms. The slot a
            // credential is read under is not on it — see
            // `/registrations/slots` below and the module comment in
            // `registration_views`.
            (Method::Get, "/registrations") => {
                let (status, body) = crate::ledger_views::render_fallible(
                    crate::registration_views::standings(&platform, now),
                );
                Response::json(status, body)
            }
            // The operator's list: the same rows with the deployment
            // variables and the commands that fill them.
            (Method::Get, "/registrations/slots") => {
                let (status, body) = crate::ledger_views::render_fallible(
                    crate::registration_views::registrations(&platform, now),
                );
                Response::json(status, body)
            }
            (Method::Post, "/registrations/:source/approve") => {
                // The source is the path's second segment under the prefix;
                // the route matched, so it is present.
                let Some(source_id) = path_segment(&request.path, 1) else {
                    return Response::json(404, r#"{"error":"no such route"}"#);
                };
                // The body is screened before the kernel sees it, and the
                // screen never repeats what it refused: the one thing this
                // route exists to refuse is a pasted key where a variable
                // name belongs.
                let body = match request
                    .body_as_str()
                    .map_err(|error| error.message().to_string())
                    .and_then(crate::registration_views::ApprovalRequest::parse)
                {
                    Ok(body) => body,
                    Err(reason) => {
                        return Response::json(400, crate::registration_views::refusal(&reason));
                    }
                };
                // The operator is the authenticated principal, exactly as
                // clearing the kill switch establishes one; the kernel takes
                // the record's operator from this identity and from nothing
                // the body says.
                //
                // `authenticated_at` is when the session was issued, never
                // `now`. Passing `now` here made `is_fresh(now,
                // REGISTRATION_CREDENTIAL_AGE)` compute an age of zero on
                // every call, so the kernel's fifteen-minute gate was a
                // control that could not fire — the `MaxExpectedShortfall`
                // shape the risk rules name by example. The remedy for a
                // session older than the window is that the operator signs in
                // again, not that the window grows to fit the session.
                let operator = qip_risk_engine::autonomy::OperatorIdentity::verified(
                    principal.subject.clone(),
                    "api-bearer-token",
                    principal.issued_at,
                );
                match platform.approve_registration(
                    source_id,
                    &operator,
                    &body.terms,
                    body.secret,
                    now,
                ) {
                    Ok(record) => {
                        // The slot is the record the kernel just adopted,
                        // not the body that proposed it: one claim about
                        // which variable holds this venue's credential,
                        // taken from the side that will be replayed.
                        let connector = self.readmit_connector(
                            &platform,
                            source_id,
                            record.secret().variable(),
                            now,
                        );
                        let (status, body) =
                            crate::ledger_views::render(&crate::registration_views::approved(
                                &platform, source_id, now, connector,
                            ));
                        Response::json(status, body)
                    }
                    Err(error) => Response::json(
                        crate::registration_views::refusal_status(&error),
                        crate::registration_views::refusal(error.message()),
                    ),
                }
            }
            // A stream asked for through the handler rather than over a socket
            // — by a test, an embedder, or a client library that buffers a
            // whole response before returning it. Answering with the stream's
            // contract is more use than a refusal: it names the path, the
            // media type, the topics it carries and exactly what a reconnect
            // does and does not recover.
            (Method::Get, pattern) if StreamKind::from_pattern(pattern).is_some() => {
                match StreamKind::from_pattern(pattern) {
                    Some(kind) => Response::json(200, kind.descriptor()),
                    // Unreachable: the guard above just resolved it. Answered
                    // rather than asserted, because a panic on a path that
                    // cannot be taken is still a panic in a request handler.
                    None => Response::json(404, r#"{"error":"no such stream"}"#),
                }
            }
            (Method::Post, "/cycle") => {
                // SENSE, before the loop: the feed's records go into the
                // platform and the cycle runs at the instant the feed says —
                // tape time for a tape, which is why `now` is rebound here
                // rather than read again from the wall clock. A source that
                // fails to answer is a refusal to cycle, not a cycle over
                // stale state with a note attached: every stage after SENSE
                // would otherwise reason as if it were current.
                let (now, sensed) = match self.feed.as_ref() {
                    None => (now, None),
                    Some(feed) => {
                        let Ok(mut feed) = feed.lock() else {
                            return Response::json(
                                503,
                                r#"{"error":"the feed is in an inconsistent state and is not serving"}"#,
                            );
                        };
                        if feed.is_exhausted() {
                            return Response::json(
                                409,
                                format!(
                                    r#"{{"error":{}}}"#,
                                    json::string(
                                        "the tape is spent: every period has been released, \
                                         so there is no next instant to cycle at. Restart the \
                                         process to replay it"
                                    )
                                ),
                            );
                        }
                        let mut sensed = match feed.sense(now) {
                            Ok(sensed) => sensed,
                            Err(error) => {
                                eprintln!("qip-api: the feed did not answer: {}", error.message());
                                return Response::json(
                                    503,
                                    format!(
                                        r#"{{"error":{},"source":{}}}"#,
                                        json::string(error.message()),
                                        json::string(&feed.descriptor().name)
                                    ),
                                );
                            }
                        };
                        let released = sensed.records.len();
                        let observed = platform.observe(std::mem::take(&mut sensed.records));
                        (
                            sensed.at,
                            Some(SenseSummary {
                                source: sensed.source,
                                at: sensed.at,
                                released,
                                observed,
                                rejections: sensed.rejections,
                            }),
                        )
                    }
                };
                let report = platform.run_cycle(now);
                // Recorded before anything that can fail below, so the page
                // shows the cycle even when the archive or the mesh does not.
                if let Some(overview) = &self.overview {
                    overview.record(&report);
                }
                // The mesh backbone rides this same rhythm: drain the cell
                // deltas into the platform while its lock is held, snapshot
                // the plane's live envelopes, then release the lock before
                // any dispatch socket is touched — a retry ladder must never
                // run under the lock every other request waits on.
                //
                // The archive hand-over sits *after* the whitelist is issued
                // and *before* the lock is released, and the order is the
                // point. It used to run first, straight after the cycle, and
                // `pending_policy` below then journaled the cycle whitelist
                // into a log the archive had already read — so the record of
                // what this cycle shipped reached the store only when the
                // next cycle archived, and the last cycle's never did: this
                // process has no signal handler, and a `qip-api` stopped
                // between two cycles left its final `policy_distributed`
                // record in memory while the cell it had been sent to was
                // already acting on it. A whitelist shipped without its record
                // is a permission reproducible from nothing, which is exactly
                // what journaling it was meant to prevent.
                let (archived, mesh) = match self.mesh.as_ref() {
                    None => (self.archive_from(&platform), None),
                    Some(mesh) => match mesh.lock() {
                        Ok(mut mesh) => {
                            let drained = mesh.drain_into(&mut platform, self.cells.as_ref(), now);
                            let pending = crate::mesh::pending_capital(&platform, now);
                            // Policy is built under the same lock so its grant
                            // manifest and the dispatched grants describe one
                            // instant, and sent after it for the same reason
                            // capital is. The cycle whitelist is issued and
                            // journaled here too, under the same lock, which
                            // is why the platform is borrowed mutably.
                            let cells: Vec<String> = mesh.cells().collect();
                            let policy_pending = crate::mesh::pending_policy(
                                &mut platform,
                                cells.into_iter(),
                                mesh.regions(),
                                now,
                            );
                            let archived = self.archive_from(&platform);
                            drop(platform);
                            // Said on stderr as well as in the response: an
                            // operator asking why a desk never installs reads
                            // the answer where the policy was shipped from,
                            // whichever they reach first.
                            for line in policy_pending
                                .whitelist
                                .iter()
                                .chain(policy_pending.shares.iter())
                                .chain(policy_pending.episodic.iter())
                            {
                                eprintln!("qip-api: {line}");
                            }
                            let dispatched = mesh.dispatch(pending, now);
                            let policy = mesh.dispatch_policy(policy_pending, now);
                            (
                                archived,
                                Some(crate::mesh::exchange_json(&drained, &dispatched, &policy)),
                            )
                        }
                        Err(_) => (
                            self.archive_from(&platform),
                            Some(
                                r#"{"error":"the mesh backbone is in an inconsistent state"}"#
                                    .to_string(),
                            ),
                        ),
                    },
                };
                Response::json(
                    202,
                    cycle_report(&report, archived.as_ref(), mesh.as_deref(), sensed.as_ref()),
                )
            }
            (Method::Post, "/kill-switch") => {
                let reason = request
                    .query_param("reason")
                    .unwrap_or("halted through the API");
                platform.autonomy_mut().kill_switch_mut().trip_global(
                    now,
                    format!("api:{}", principal.subject),
                    reason,
                );
                // The central switch is tripped either way; the broadcast is
                // what makes the same action reach the regions, closing the
                // gap where an operator's halt stopped the centre and left
                // every cell trading. Best effort, and the counts say what
                // happened — the payload's own halted flag re-carries the
                // state for a cell that missed it.
                drop(platform);
                let broadcast = self.mesh.as_ref().and_then(|mesh| {
                    let mut mesh = mesh.lock().ok()?;
                    Some(mesh.broadcast_halt(reason, now))
                });
                let broadcast_json = broadcast
                    .and_then(|summary| serde_json::to_string(&summary).ok())
                    .unwrap_or_else(|| "null".to_string());
                Response::json(
                    200,
                    format!(
                        r#"{{"halted":true,"by":{},"reason":{},"broadcast":{}}}"#,
                        json::string(&principal.subject),
                        json::string(reason),
                        broadcast_json
                    ),
                )
            }
            (Method::Delete, "/kill-switch") => {
                // Clearing a halt needs an operator identity, which is what
                // the Operator role on this route establishes. As on the two
                // routes above, `authenticated_at` is when the session was
                // issued and not `now`: an identity that claims to have
                // authenticated at the instant it is used is fresh by
                // construction, and any caller downstream that asks how old
                // this credential is gets the answer zero forever.
                let operator = qip_risk_engine::autonomy::OperatorIdentity::verified(
                    principal.subject.clone(),
                    "api-bearer-token",
                    principal.issued_at,
                );
                match platform
                    .autonomy_mut()
                    .kill_switch_mut()
                    .clear_global(&operator, now)
                {
                    Ok(()) => Response::json(
                        200,
                        format!(
                            r#"{{"halted":false,"cleared_by":{}}}"#,
                            json::string(&principal.subject)
                        ),
                    ),
                    Err(error) => Response::json(
                        409,
                        format!(r#"{{"error":{}}}"#, json::string(error.message())),
                    ),
                }
            }
            _ => Response::json(404, r#"{"error":"no such route"}"#),
        }
    }
}

impl Handler for Api {
    fn handle(&self, request: &Request) -> Response {
        // Discovery is unauthenticated on purpose: a client needs to know the
        // API version before it can present a credential correctly, and the
        // route table is not a secret.
        if request.method == Method::Get && request.path == DISCOVERY_PATH {
            return Response::json(200, discovery());
        }
        // The same information in a form a generated client can read. Served
        // rather than checked in, so it is derived from `ROUTES` at the moment
        // it is asked for and cannot drift from the table the way a committed
        // file would. Unauthenticated for the same reason discovery is.
        if request.method == Method::Get && request.path == OPENAPI_PATH {
            return Response::json(200, crate::openapi::document());
        }

        let Some(route) = Api::route_for(request.method, &request.path) else {
            // A path that exists under a different method gets a 405 rather
            // than a 404, since hiding that would only confuse a legitimate
            // client without hiding anything from anyone else.
            let exists_under_another_method = ROUTES.iter().any(|candidate| {
                request
                    .path
                    .strip_prefix(VERSION_PREFIX)
                    .is_some_and(|suffix| matches_pattern(candidate.pattern, suffix))
            });
            return if exists_under_another_method {
                Response::json(405, r#"{"error":"that method is not allowed here"}"#)
            } else {
                Response::json(404, r#"{"error":"no such route"}"#)
            };
        };

        match self.admit(request, route) {
            Ok(principal) => self.dispatch(request, &principal, route),
            Err(refusal) => refusal,
        }
    }

    fn stream(&self, request: &Request) -> StreamDecision {
        // Streams are read-only and live under one prefix. Anything else is
        // not a stream request and is answered the ordinary way.
        if request.method != Method::Get {
            return StreamDecision::NotAStream;
        }
        let Some(suffix) = request.path.strip_prefix(VERSION_PREFIX) else {
            return StreamDecision::NotAStream;
        };
        let Some(kind) = StreamKind::from_pattern(suffix) else {
            return StreamDecision::NotAStream;
        };
        let Some(route) = Api::route_for(Method::Get, &request.path) else {
            return StreamDecision::NotAStream;
        };

        // The same ladder every other route goes through, run once. A refusal
        // is returned as a response rather than as "not a stream", because the
        // head of an accepted stream is a 200 that cannot be withdrawn once
        // written.
        let principal = match self.admit(request, route) {
            Ok(principal) => principal,
            Err(refusal) => return StreamDecision::Refused(refusal),
        };
        let _ = principal;

        let source: Box<dyn EventSource> = match kind {
            StreamKind::Health => Box::new(PlatformHealth::new(
                self.platform.clone(),
                self.cells.clone(),
                self.pulse.clone(),
                self.clock.clone(),
            )),
            log_backed => Box::new(LoggedEvents::new(
                self.platform.clone(),
                log_backed.topics(),
                self.stream_limits.backlog,
            )),
        };
        StreamDecision::Accepted(Box::new(EventStream::open(
            kind,
            source,
            self.clock.clone(),
            self.stream_limits,
            request.header(LAST_EVENT_ID),
        )))
    }
}

/// The `index`th segment of a request path under the version prefix, for a
/// route whose pattern names a parameter there.
///
/// Read from the path the router already matched rather than parsed again
/// from the pattern, so the segment a handler acts on is the one the table
/// admitted. `None` only for a path that did not match, which the router
/// has already answered.
fn path_segment(path: &str, index: usize) -> Option<&str> {
    path.strip_prefix(VERSION_PREFIX)?
        .split('/')
        .filter(|segment| !segment.is_empty())
        .nth(index)
}

/// Whether a concrete path matches a pattern with `:name` parameters.
fn matches_pattern(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if pattern_parts.len() != path_parts.len() {
        return false;
    }
    pattern_parts
        .iter()
        .zip(&path_parts)
        .all(|(pattern, actual)| pattern.starts_with(':') || pattern == actual)
}

// --- response bodies --------------------------------------------------------

fn discovery() -> String {
    let routes: Vec<String> = ROUTES
        .iter()
        .map(|route| {
            format!(
                r#"{{"method":{},"path":{},"role":{},"summary":{}}}"#,
                json::string(route.method.as_str()),
                json::string(&format!("{VERSION_PREFIX}{}", route.pattern)),
                json::string(route.required_role.as_str()),
                json::string(route.summary)
            )
        })
        .collect();
    format!(r#"{{"version":"v1","routes":[{}]}}"#, routes.join(","))
}

fn health(platform: &Platform) -> String {
    let halted = platform.autonomy().kill_switch().is_globally_tripped();
    // A book that disagrees with the venue is reported here as well as on
    // `/orders`, because this is the endpoint a monitor polls and that
    // disagreement is the one condition under which none of the platform's own
    // numbers should be believed.
    let breaks = platform.orders().reconciliation_breaks().len();
    let status = if halted {
        "halted"
    } else if breaks > 0 {
        "reconciliation-break"
    } else {
        "ok"
    };
    format!(
        r#"{{"status":{},"halted":{},"autonomy":{},"live_capable":{},"reconciliation_breaks":{}}}"#,
        json::string(status),
        halted,
        json::string(platform.autonomy().level().as_str()),
        platform.is_live_capable(),
        breaks
    )
}

/// The platform's state, and how much of it is being kept.
///
/// `events` counts what this process holds in memory and `archived` counts what
/// survives it. Both are reported because they answer different questions and
/// the first one alone is the misleading half: an operator reading a healthy
/// event count has no way to tell it from a deployment that is keeping none of
/// it. `archived` is `null` when nothing is configured, rather than zero — a
/// zero would read as "configured and empty".
fn status(platform: &Platform, archive: Option<&ChainArchive>, mesh: Option<&str>) -> String {
    let switch = platform.autonomy().kill_switch();
    format!(
        r#"{{"autonomy":{},"configured_autonomy":{},"ceiling":{},"live_capable":{},"halted":{},"halted_scopes":[{}],"cycles":{},"events":{},"archived":{},"mesh":{}}}"#,
        json::string(platform.autonomy().level().as_str()),
        json::string(platform.autonomy().configured_level().as_str()),
        json::string(platform.autonomy().ceiling().as_str()),
        platform.is_live_capable(),
        switch.is_globally_tripped(),
        switch
            .halted_scopes()
            .iter()
            .map(|scope| json::string(scope))
            .collect::<Vec<_>>()
            .join(","),
        platform.cycle_count(),
        platform.event_log().len(),
        match archive {
            Some(archive) => archive.records_archived().to_string(),
            None => "null".to_string(),
        },
        // The status page states the truth about the backbone either way: a
        // served mesh reports its counters, and an unserved one says the
        // deltas have nowhere to land rather than saying nothing.
        match mesh {
            Some(mesh) => mesh.to_string(),
            None => r#"{"served":false}"#.to_string(),
        }
    )
}

/// The console's counts, counted from the platform's state.
///
/// Kept as it was, and deliberately not merged with [`scrape`]. This answers
/// "how big is the book right now" for a person reading a page; that answers
/// "what has this process done since it started" for a scrape. The two are
/// different questions and the second cannot be derived from the first — a
/// refusal that happened and was then superseded is gone from the state and
/// permanent in the counter.
fn metrics(platform: &Platform) -> String {
    format!(
        r#"{{"cycles":{},"events_logged":{},"opportunities_queued":{},"proposals":{},"orders":{},"fills":{},"refusals":{},"live_fills":{}}}"#,
        platform.cycle_count(),
        platform.event_log().len(),
        platform.queue().len(),
        platform.proposals().len(),
        platform.orders().orders().count(),
        platform.orders().fills().len(),
        platform.orders().refusals().len(),
        platform.orders().has_live_fills()
    )
}

/// What the platform actually recorded, in Prometheus text exposition.
///
/// This endpoint used to serve the same eight inferred counts as
/// [`metrics`] — a JSON object recomputed from platform state at the moment of
/// the request. It was a second claim about facts the platform already knew,
/// and the two disagreed the moment anything was superseded: an order refused
/// on cycle four and released on cycle five left `refusals` reading one, then
/// one forever, whatever happened next.
///
/// It is also why nothing could scrape this platform. A scrape needs text
/// exposition, Cloud Monitoring will not create a metric descriptor from a
/// bespoke JSON object, and the four alert policies in
/// `infrastructure/terraform/modules/observability/main.tf` are gated behind
/// `workload_metrics_exist` precisely because no descriptor by their names had
/// ever been ingested. Serving the recorded surface here is what makes a
/// descriptor possible; the scrape configuration and the flag are still
/// separate work, and neither should be turned on before a pod has been seen
/// to scrape.
///
/// Empty until the first cycle records something, and that is correct rather
/// than a gap: a scrape of a process that has done nothing should return
/// nothing, not zeroes it has no evidence for.
fn scrape(platform: &Platform) -> String {
    platform.telemetry().metrics.snapshot().to_prometheus()
}

fn governance(platform: &Platform, now: Timestamp) -> String {
    let findings = platform.review_governance(now);
    let rendered: Vec<String> = findings
        .iter()
        .map(|finding| {
            format!(
                r#"{{"severity":{},"rule":{},"detail":{},"agents":[{}]}}"#,
                json::string(match finding.severity {
                    qip_agents::governance::Severity::Error => "error",
                    qip_agents::governance::Severity::Warning => "warning",
                }),
                json::string(&finding.rule),
                json::string(&finding.detail),
                finding
                    .agents
                    .iter()
                    .map(|agent| json::string(agent))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    format!(
        r#"{{"agents":{},"findings":[{}]}}"#,
        platform.organisation().len(),
        rendered.join(",")
    )
}

fn portfolio(platform: &Platform) -> String {
    // The book is behind the desk's capability gate, so the API reports what
    // it can see without one: the counts and the risk posture. Position-level
    // detail is served through the read model, not by reaching past a control.
    format!(
        r#"{{"proposals":{},"orders":{},"fills":{},"paper_only":{}}}"#,
        platform.proposals().len(),
        platform.orders().orders().count(),
        platform.orders().fills().len(),
        !platform.orders().has_live_fills()
    )
}

fn opportunities(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .queue()
        .iter()
        .map(|opportunity| {
            format!(
                r#"{{"id":{},"headline":{},"score":{},"confidence":{},"detectors":[{}]}}"#,
                json::string(opportunity.opportunity_id.as_str()),
                json::string(&opportunity.headline),
                json::number(opportunity.rank.score),
                json::number(opportunity.rank.confidence),
                opportunity
                    .detectors
                    .iter()
                    .map(|detector| json::string(detector))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    format!(r#"{{"opportunities":[{}]}}"#, rendered.join(","))
}

fn proposals(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .proposals()
        .iter()
        .map(|proposal| {
            format!(
                r#"{{"id":{},"status":{},"legs":{},"gross":{},"turnover":{},"rationale":{}}}"#,
                json::string(proposal.proposal_id.as_str()),
                json::string(proposal.status().as_str()),
                proposal.len(),
                json::number(proposal.target_gross),
                json::number(proposal.turnover),
                json::string(&proposal.rationale)
            )
        })
        .collect();
    format!(r#"{{"proposals":[{}]}}"#, rendered.join(","))
}

fn orders(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .orders()
        .orders()
        .map(|order| {
            format!(
                r#"{{"id":{},"instrument":{},"side":{},"quantity":{},"state":{},"filled":{},"simulated":{}}}"#,
                json::string(order.order_id.as_str()),
                json::string(order.object_id.as_str()),
                json::string(order.side.as_str()),
                json::string(&order.quantity.to_string()),
                json::string(order.state.as_str()),
                json::string(&order.filled_quantity().to_string()),
                order.is_paper()
            )
        })
        .collect();
    let breaks: Vec<String> = platform
        .orders()
        .reconciliation_breaks()
        .iter()
        .map(|reason| json::string(reason))
        .collect();
    format!(
        r#"{{"orders":[{}],"refusals":{},"reconciliation_breaks":[{}]}}"#,
        rendered.join(","),
        platform.orders().refusals().len(),
        breaks.join(",")
    )
}

fn agents(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .organisation()
        .roster()
        .iter()
        .map(|manifest| {
            format!(
                r#"{{"id":{},"name":{},"role":{},"owner":{},"purpose":{},"capabilities":[{}]}}"#,
                json::string(&manifest.id),
                json::string(&manifest.name),
                json::string(manifest.role.as_str()),
                json::string(&manifest.owner),
                json::string(&manifest.purpose),
                manifest
                    .capabilities
                    .iter()
                    .map(|capability| json::string(capability.as_str()))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    format!(r#"{{"agents":[{}]}}"#, rendered.join(","))
}

fn autonomy(platform: &Platform) -> String {
    let controller = platform.autonomy();
    let history: Vec<String> = controller
        .history()
        .iter()
        .map(|change| {
            format!(
                r#"{{"at":{},"from":{},"to":{},"operator":{},"reason":{}}}"#,
                change.at.as_nanos(),
                json::string(change.from.as_str()),
                json::string(change.to.as_str()),
                json::string(&change.operator),
                json::string(&change.reason)
            )
        })
        .collect();
    format!(
        r#"{{"level":{},"ceiling":{},"live":{},"history":[{}]}}"#,
        json::string(controller.level().as_str()),
        json::string(controller.ceiling().as_str()),
        controller.is_live(),
        history.join(",")
    )
}

/// What SENSE did before one cycle, for the cycle's response.
///
/// `released` is what the source handed over and `observed` is what the
/// platform took in; the two are reported separately because a record the
/// platform declined is a gap that would otherwise be invisible in a count
/// that only ever grew.
struct SenseSummary {
    source: String,
    at: Timestamp,
    released: usize,
    observed: usize,
    rejections: Vec<String>,
}

fn cycle_report(
    report: &qip_kernel::CycleReport,
    archived: Option<&std::result::Result<usize, String>>,
    mesh: Option<&str>,
    sensed: Option<&SenseSummary>,
) -> String {
    let stages: Vec<String> = report
        .stages
        .iter()
        .map(|outcome| {
            format!(
                r#"{{"stage":{},"ran":{},"produced":{},"detail":{},"problems":[{}]}}"#,
                json::string(outcome.stage.as_str()),
                outcome.ran,
                outcome.produced,
                json::string(&outcome.detail),
                outcome
                    .problems
                    .iter()
                    .map(|problem| json::string(problem))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    // Reported rather than omitted on failure: a caller that ran a cycle and
    // got a 202 has no other way to learn that the cycle left no durable
    // trace, and "the request succeeded" is not the same claim as "the record
    // was kept".
    let archived = match archived {
        None => r#""archived":null"#.to_string(),
        Some(Ok(count)) => format!(r#""archived":{count}"#),
        Some(Err(reason)) => format!(
            r#""archived":null,"archive_error":{}"#,
            json::string(reason)
        ),
    };
    // Absent rather than null when no mesh is configured: an absent key says
    // the process has no mesh, while `"mesh":null` would read as a mesh that
    // did nothing this cycle.
    let mesh = mesh
        .map(|exchange| format!(r#","mesh":{exchange}"#))
        .unwrap_or_default();
    // Absent for the same reason the mesh is: a process with no feed sensed
    // nothing, and `"observed":0` would read as a source that went quiet.
    let sense = sensed
        .map(|sensed| {
            format!(
                r#","sense":{{"source":{},"at":{},"released":{},"observed":{},"rejected":{},"rejections":[{}]}}"#,
                json::string(&sensed.source),
                json::string(&sensed.at.to_rfc3339()),
                sensed.released,
                sensed.observed,
                sensed.rejections.len(),
                sensed
                    .rejections
                    .iter()
                    .map(|rejection| json::string(rejection))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .unwrap_or_default();
    format!(
        r#"{{"cycle":{},"correlation_id":{},"halted":{},"traversed_every_stage":{},{},"stages":[{}]{}{}}}"#,
        report.cycle,
        json::string(report.correlation_id.as_str()),
        report.halted,
        report.traversed_every_stage(),
        archived,
        stages.join(","),
        mesh,
        sense
    )
}

// --- the console's read surface, as JSON ------------------------------------
//
// One function per route the operator console also renders, so anything on a
// panel can be read by something that is not a browser. They follow the rule
// the console follows and `crate::missing` exists for: where the platform
// holds nothing behind a panel, the body says which subsystem is missing and
// why, rather than returning a shape full of zeroes that a client would plot
// as a flat line.

/// The body for a surface this deployment has nothing behind.
///
/// `available` is stated rather than implied by an absent field, because a
/// client that forgot to check would otherwise read a missing key as an empty
/// collection — which is the same mistake as rendering a zero.
fn unavailable(subject: &str, reason: &str) -> String {
    format!(
        r#"{{"subject":{},"available":false,"reason":{}}}"#,
        json::string(subject),
        json::string(reason)
    )
}

/// Autonomy, halt state, cycle count, and whether the event log verifies.
fn system(platform: &Platform) -> String {
    let switch = platform.autonomy().kill_switch();
    let chain = platform.event_log().verify_chain();
    format!(
        r#"{{"autonomy":{},"ceiling":{},"live":{},"halted":{},"halted_scopes":[{}],"cycles":{},"events_logged":{},"chain_intact":{},"chain_broken_at":{}}}"#,
        json::string(platform.autonomy().level().as_str()),
        json::string(platform.autonomy().ceiling().as_str()),
        platform.autonomy().is_live(),
        switch.is_globally_tripped(),
        switch
            .halted_scopes()
            .iter()
            .map(|scope| json::string(scope))
            .collect::<Vec<_>>()
            .join(","),
        platform.cycle_count(),
        platform.event_log().len(),
        chain.is_ok(),
        // The sequence of the first broken link, or null. Null rather than a
        // sentinel sequence: zero is a record number.
        match chain {
            Ok(()) => "null".to_string(),
            Err(sequence) => sequence.to_string(),
        }
    )
}

/// Every edge cell that has reported here, its book, and how old the report is.
///
/// The aggregate the central plane holds says what the cells have; only the
/// arrival times in [`CellRegistry`] say whether it is current, so both are
/// reported together. With no cell reporting, this is an absence rather than
/// an empty book: the aggregate would show zero gross, and zero gross is what
/// a flat platform looks like too.
fn regions(platform: &Platform, cells: &CellRegistry, now: Timestamp) -> String {
    let observations = cells.observations();
    if observations.is_empty() {
        return unavailable("cells", crate::missing::NO_CELL_REPORTS);
    }
    let bound = cells.freshness_bound();
    let exposure = platform.central().exposure();
    let switch = platform.autonomy().kill_switch();
    let rendered: Vec<String> = observations
        .iter()
        .map(|observation| {
            format!(
                r#"{{"cell":{},"reported_at":{},"age":{},"stale":{},"halted":{},"positions":{},"strategies":{},"reconciliation_breaks":{},"gross":{},"net":{}}}"#,
                json::string(&observation.cell),
                json::string(&observation.at.to_rfc3339()),
                json::string(&describe_age(observation.age(now))),
                observation.is_stale(now, bound),
                switch.is_halted(&observation.cell),
                observation.positions,
                observation.strategies,
                observation.reconciliation_breaks,
                json::string(&exposure.by_cell.gross_of(&observation.cell).to_string()),
                json::string(&exposure.by_cell.net_of(&observation.cell).to_string())
            )
        })
        .collect();
    format!(
        r#"{{"freshness_bound":{},"cells":[{}]}}"#,
        json::string(&describe_age(bound)),
        rendered.join(",")
    )
}

/// Registered strategies, their stage on the ladder, and whether they hold
/// capital.
fn strategies(platform: &Platform) -> String {
    let factory = platform.central().factory();
    let rendered: Vec<String> = factory
        .candidates()
        .map(|candidate| {
            format!(
                r#"{{"id":{},"cell":{},"venue":{},"stage":{},"holds_capital":{},"registered_at":{}}}"#,
                json::string(candidate.strategy().as_str()),
                json::string(candidate.cell()),
                json::string(candidate.venue().as_str()),
                json::string(factory.stage_of(candidate.strategy()).as_str()),
                factory.holds_capital(candidate.strategy()),
                json::string(&candidate.registered_at().to_rfc3339())
            )
        })
        .collect();
    // The factory is held in this process, so an empty list is an observed
    // zero: nothing has been registered.
    format!(r#"{{"strategies":[{}]}}"#, rendered.join(","))
}

/// What the agents spent on language models, and why there is no roster.
///
/// The platform attaches a model to the agent organisation and keeps no
/// registry of which models are attached, so there is no list of models to
/// return. What it does keep is an audit record of every agent run, which is a
/// record of use rather than a roster — reported as such, and separately from
/// the registry that does not exist.
fn models(platform: &Platform) -> String {
    let records = platform.organisation().audit().records();
    let calls: u64 = records
        .iter()
        .map(|record| u64::from(record.spend.language_model_calls))
        .sum();
    let tokens: u64 = records
        .iter()
        .map(|record| u64::from(record.spend.tokens))
        .sum();
    let cost: u64 = records.iter().map(|record| record.spend.cost_micros).sum();
    format!(
        r#"{{"registry":{},"observed_use":{{"agent_runs":{},"model_calls":{},"tokens":{},"cost_micros":{}}}}}"#,
        unavailable("models", crate::missing::NO_MODEL_REGISTRY),
        records.len(),
        calls,
        tokens,
        // Micros, as an integer, because that is the unit the budget is
        // charged in. Dividing it into a float here would report a spend
        // nobody recorded.
        cost
    )
}

/// Allocation bounds, issued envelopes, and outstanding recalls.
fn capital(platform: &Platform, now: Timestamp) -> String {
    let central = platform.central();
    let config = central.config();
    let envelopes: Vec<String> = central
        .factory()
        .candidates()
        .filter_map(|candidate| {
            let envelope = central.envelope(candidate.cell(), candidate.strategy())?;
            // What a cell has committed is only known if the cell said so.
            // Absent that, the field states that it was not reported: a zero
            // would read as an unused grant, which is the opposite of an
            // unknown one.
            let used = match central.utilisation(candidate.cell(), candidate.strategy()) {
                Some(utilisation) => format!(
                    r#"{{"reported":true,"gross_committed":{},"orders_sent":{}}}"#,
                    json::string(&utilisation.gross_committed.to_string()),
                    utilisation.orders_sent
                ),
                None => format!(
                    r#"{{"reported":false,"reason":{}}}"#,
                    json::string(
                        "the cell has not reported what it has committed against this envelope"
                    )
                ),
            };
            Some(format!(
                r#"{{"cell":{},"strategy":{},"gross_limit":{},"expires_at":{},"used":{}}}"#,
                json::string(candidate.cell()),
                json::string(candidate.strategy().as_str()),
                json::string(&envelope.gross_limit().to_string()),
                json::string(&envelope.expires_at().to_rfc3339()),
                used
            ))
        })
        .collect();
    let recalls: Vec<String> = central
        .recalls()
        .outstanding(now)
        .iter()
        .map(|recall| {
            format!(
                r#"{{"cell":{},"strategy":{},"reason":{},"detail":{},"issued_at":{},"acknowledge_by":{},"gross_recalled":{},"backstop_expiry":{}}}"#,
                json::string(&recall.cell),
                json::string(recall.strategy.as_str()),
                json::string(recall.reason.as_str()),
                json::string(&recall.detail),
                json::string(&recall.issued_at.to_rfc3339()),
                json::string(&recall.acknowledge_by.to_rfc3339()),
                json::string(&recall.gross_recalled.to_string()),
                // The one bound that holds without anyone being reachable: an
                // expired envelope admits nothing whatever the cell does.
                json::string(&recall.backstop_expiry.to_rfc3339())
            )
        })
        .collect();
    format!(
        r#"{{"bounds":{{"total_budget":{},"per_strategy":{},"per_cell":{},"per_venue":{}}},"envelopes":[{}],"outstanding_recalls":[{}]}}"#,
        json::string(&config.total_budget.to_string()),
        json::string(&config.per_strategy.to_string()),
        json::string(&config.per_cell.to_string()),
        json::string(&config.per_venue.to_string()),
        envelopes.join(","),
        recalls.join(",")
    )
}

/// Exposure, concentration, the kill switch — and what cannot be measured here.
///
/// The exposure aggregate and the concentration findings are both built from
/// cell reports. With nothing reporting, the aggregate sums to zero and the
/// findings come back empty — which reads as a flat book with no breach in it,
/// and is indistinguishable from the truth, which is that nobody has looked.
/// So both are absences until a cell has reported, and neither is an empty
/// list.
fn risk(platform: &Platform, cells: &CellRegistry) -> String {
    let switch = platform.autonomy().kill_switch();
    let limits = platform.central().concentration_limits();
    let (exposure, concentrations) = if cells.is_empty() {
        (
            unavailable("exposure", crate::missing::NO_CELL_REPORTS),
            unavailable("concentrations", crate::missing::NO_CELL_REPORTS),
        )
    } else {
        let aggregate = platform.central().exposure();
        let buckets: Vec<String> = [
            (
                "instrument",
                &aggregate.by_instrument,
                limits.per_instrument,
            ),
            ("sector", &aggregate.by_sector, limits.per_sector),
            ("venue", &aggregate.by_venue, limits.per_venue),
            ("currency", &aggregate.by_currency, limits.per_currency),
            ("cell", &aggregate.by_cell, limits.per_cell),
        ]
        .into_iter()
        .flat_map(|(axis, view, limit)| {
            let shares = view.shares();
            view.ranked()
                .into_iter()
                .map(|(bucket, gross)| {
                    let share = shares.get(&bucket).copied().unwrap_or(0.0);
                    format!(
                        r#"{{"axis":{},"bucket":{},"gross":{},"net":{},"share":{},"limit":{},"breached":{}}}"#,
                        json::string(axis),
                        json::string(&bucket),
                        json::string(&gross.to_string()),
                        json::string(&view.net_of(&bucket).to_string()),
                        json::number(share),
                        json::number(limit),
                        share > limit
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
        let findings: Vec<String> = aggregate
            .concentrations(&limits)
            .iter()
            .map(|finding| {
                format!(
                    r#"{{"axis":{},"bucket":{},"gross":{},"share":{},"limit":{}}}"#,
                    json::string(finding.axis),
                    json::string(&finding.bucket),
                    json::string(&finding.gross.to_string()),
                    json::number(finding.share),
                    json::number(finding.limit)
                )
            })
            .collect();
        (
            format!(r#"{{"available":true,"buckets":[{}]}}"#, buckets.join(",")),
            format!(
                r#"{{"available":true,"findings":[{}]}}"#,
                findings.join(",")
            ),
        )
    };
    format!(
        r#"{{"exposure":{},"concentrations":{},"kill_switch":{{"halted":{},"halted_scopes":[{}],"tripped_by":{},"reason":{},"clearances":{}}},"limit_utilisation":{},"tail_risk":{}}}"#,
        exposure,
        concentrations,
        switch.is_globally_tripped(),
        switch
            .halted_scopes()
            .iter()
            .map(|scope| json::string(scope))
            .collect::<Vec<_>>()
            .join(","),
        json::string(
            switch
                .global_trip()
                .map_or("", |trip| trip.tripped_by.as_str())
        ),
        json::string(switch.global_trip().map_or("", |trip| trip.reason.as_str())),
        switch.clearances().len(),
        // Two things a risk view is expected to carry that this process cannot
        // measure. Named rather than omitted: a client that finds no limits
        // would otherwise conclude there are none.
        unavailable("limits", crate::missing::NO_LIMIT_UTILISATION),
        unavailable("tail_risk", crate::missing::DESK_GATED)
    )
}

/// Every fill, and whether it came from a simulated venue.
fn fills(platform: &Platform) -> String {
    let manager = platform.orders();
    let rendered: Vec<String> = manager
        .fills()
        .into_iter()
        .map(|fill| {
            let order = manager.order(&fill.order_id);
            format!(
                r#"{{"order":{},"instrument":{},"side":{},"quantity":{},"price":{},"venue":{},"simulated":{}}}"#,
                json::string(fill.order_id.as_str()),
                json::string(
                    order.map_or("unknown", |order| order.object_id.as_str())
                ),
                json::string(order.map_or("unknown", |order| order.side.as_str())),
                json::string(&fill.quantity.to_string()),
                json::string(&fill.price.to_string()),
                json::string(&fill.venue),
                // On every row. A blotter where the reader has to work out
                // which fills were real is a blotter that will be misread.
                fill.simulated
            )
        })
        .collect();
    // The book is this process's own, so no fills is an observed zero.
    format!(
        r#"{{"fills":[{}],"any_live_fill":{}}}"#,
        rendered.join(","),
        manager.has_live_fills()
    )
}

/// Quantum jobs, and the classical run each would be compared against.
///
/// There are no jobs to report: the compute router that submits them keeps no
/// externally readable log. What the process does know is whether a provider
/// is attached at all, which is configuration rather than a result.
fn quantum(platform: &Platform) -> String {
    format!(
        r#"{{"jobs":{},"routing":{{"provider":{},"classical_baseline":"always","note":{}}}}}"#,
        unavailable("jobs", crate::missing::NO_QUANTUM_JOBS),
        json::string(if platform.config().quantum_enabled {
            "attached (simulated)"
        } else {
            "none"
        }),
        json::string(
            "every routed problem is solved classically as well; a quantum result is never \
             reported alone"
        )
    )
}

// --- the research read surface, as JSON ---------------------------------------
//
// What REASON wrote down, what LEARN concluded about it, and what the lifecycle
// ledger recorded about each strategy. Each body is read off the platform's
// own state through its accessors; where a figure is computed here rather than
// read, the body carries the window and the cycle so the figure can be
// recomputed against the same process. Nothing below is estimated in place of
// something the platform did not measure.

/// The fewest closes an instrument must hold before its returns enter the
/// correlation matrix.
///
/// Thirty closes is twenty-nine returns, about the floor below which a
/// Pearson coefficient's standard error is as large as the coefficient. Below
/// it the instrument is listed as excluded with its count rather than
/// correlated, so a portal cannot plot a coefficient over three prints as if
/// it were one over three hundred.
pub const CORRELATION_MINIMUM_CLOSES: usize = 30;

/// Every falsifiable claim the platform holds, grouped by the instrument it is
/// about, with the calibration LEARN has computed over the resolved ones.
///
/// The working set is served whole with its bound, so a reader can tell a
/// process that has made forty claims from one that has evicted a thousand.
/// The instrument comes from the claim where one was written down and from
/// the proposition's metric otherwise — a record written before the claim
/// field existed still names the series it is settled on.
fn predictions(platform: &Platform) -> String {
    let mut by_instrument: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let (mut open, mut resolved) = (0usize, 0usize);
    for prediction in platform.predictions() {
        let metric = prediction
            .proposition
            .criteria
            .metrics()
            .into_iter()
            .next()
            .unwrap_or_default();
        let instrument = prediction
            .claim
            .as_ref()
            .map(|claim| claim.subject.clone())
            .or_else(|| {
                metric
                    .split_once(':')
                    .map(|(_, subject)| subject.to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());
        // Direction as the claim stated it. "unstated" is a value, not a
        // default: a record with no claim carried no direction, and writing
        // "up" for it would be the platform grading itself on a call it
        // never made.
        let direction = match prediction.claim.as_ref().map(|claim| claim.direction) {
            Some(direction) if direction > 0.0 => "up",
            Some(direction) if direction < 0.0 => "down",
            _ => "unstated",
        };
        let state = match prediction.verdict.as_ref() {
            None => "open",
            Some(verdict) if !verdict.is_determined() => "undetermined",
            Some(verdict) if verdict.holds() => "held",
            Some(_) => "failed",
        };
        if prediction.is_open() {
            open += 1;
        } else {
            resolved += 1;
        }
        let horizon = prediction
            .proposition
            .resolves_at
            .since(prediction.recorded_at)
            .as_nanos()
            / 1_000_000_000;
        by_instrument.entry(instrument).or_default().push(format!(
            r#"{{"hypothesis":{},"cycle":{},"statement":{},"metric":{},"direction":{},"confidence":{},"expected_move_bps":{},"horizon_seconds":{},"made_at":{},"resolves_at":{},"state":{},"scored_at":{}}}"#,
            json::string(&prediction.hypothesis),
            prediction.cycle,
            json::string(&prediction.proposition.statement),
            json::string(&metric),
            json::string(direction),
            prediction
                .claim
                .as_ref()
                .map_or_else(|| "null".to_string(), |claim| json::number(claim.confidence)),
            prediction.claim.as_ref().map_or_else(
                || "null".to_string(),
                |claim| json::number(claim.expected_move_bps)
            ),
            horizon,
            json::string(&prediction.recorded_at.to_rfc3339()),
            json::string(&prediction.proposition.resolves_at.to_rfc3339()),
            json::string(state),
            prediction
                .scored_at
                .map_or_else(|| "null".to_string(), |at| json::string(&at.to_rfc3339())),
        ));
    }
    let instruments: Vec<String> = by_instrument
        .iter()
        .map(|(instrument, rendered)| {
            format!(
                r#"{}:{{"predictions":[{}]}}"#,
                json::string(instrument),
                rendered.join(",")
            )
        })
        .collect();
    // The calibration is an absence until a claim has resolved informatively,
    // and an absence with a reason: a Brier score of zero would read as a
    // perfectly calibrated platform rather than an ungraded one.
    let calibration = match platform.calibration() {
        None => unavailable("calibration", crate::missing::NO_CALIBRATION),
        Some(report) => format!(
            r#"{{"available":true,"evaluations_in_window":{},"material":{},"report":{}}}"#,
            platform.evaluations().len(),
            report.is_material(),
            serde_json::to_string(report).unwrap_or_else(|error| {
                format!(r#"{{"error":{}}}"#, json::string(&error.to_string()))
            })
        ),
    };
    format!(
        r#"{{"as_of_cycle":{},"window":{},"held":{},"open":{},"resolved":{},"instruments":{{{}}},"calibration":{}}}"#,
        platform.cycle_count(),
        Platform::prediction_window(),
        platform.predictions().len(),
        open,
        resolved,
        instruments.join(","),
        calibration
    )
}

/// Why there is no regime view, and what the declared stream topic is worth.
///
/// Served as a platform statement rather than left to the portal, and it
/// names the stream topic on purpose: `/stream/signals` declares
/// `regime.changed`, nothing in this composition publishes it, and a client
/// deciding whether to wait for one deserves to be told which.
fn regimes() -> String {
    format!(
        r#"{{"subject":"regimes","available":false,"reason":{},"stream_topic":{{"name":{},"declared_on":{},"published":false}}}}"#,
        json::string(crate::missing::NO_REGIME_CLASSIFIER),
        json::string(qip_events::topic::Topic::RegimeChanged.name()),
        json::string(&format!(
            "{VERSION_PREFIX}{}",
            StreamKind::Signals.pattern()
        ))
    )
}

/// Pairwise Pearson correlation of simple returns over the platform's tape.
///
/// Computed here rather than read, so everything a reader needs to recompute
/// it is in the body: the statistic, the window in closes and in returns, the
/// minimum below which an instrument is excluded, and the cycle. The window is
/// the shortest eligible series, taken from the most recent close backwards,
/// so every coefficient in one matrix is over the same number of returns.
///
/// Two refusals rather than a number. An instrument below the minimum, or
/// one with a non-positive close inside the window, is listed as excluded with
/// its count. A pair where either side's returns have zero variance has no
/// coefficient — the formula divides by that variance — and is written as
/// `null` and named under `undefined`, never as `NaN`, which is not JSON and
/// which a chart library would happily plot as zero.
fn correlation(platform: &Platform) -> String {
    let tape = platform.price_history();
    let mut excluded: Vec<String> = Vec::new();
    let mut eligible: Vec<(&String, &Vec<f64>)> = Vec::new();
    for (instrument, closes) in tape {
        if closes.len() < CORRELATION_MINIMUM_CLOSES {
            excluded.push(format!(
                r#"{{"instrument":{},"closes":{},"reason":{}}}"#,
                json::string(instrument),
                closes.len(),
                json::string(&format!(
                    "fewer than the {CORRELATION_MINIMUM_CLOSES} closes the estimate requires"
                ))
            ));
        } else {
            eligible.push((instrument, closes));
        }
    }
    let observed: Vec<String> = tape
        .iter()
        .map(|(instrument, closes)| {
            format!(
                r#"{{"instrument":{},"closes":{}}}"#,
                json::string(instrument),
                closes.len()
            )
        })
        .collect();
    if eligible.len() < 2 {
        return format!(
            r#"{{"subject":"correlation","available":false,"reason":{},"as_of_cycle":{},"minimum_closes":{},"instruments_observed":[{}]}}"#,
            json::string(crate::missing::TOO_FEW_SERIES),
            platform.cycle_count(),
            CORRELATION_MINIMUM_CLOSES,
            observed.join(",")
        );
    }
    let window = eligible
        .iter()
        .map(|(_, closes)| closes.len())
        .min()
        .unwrap_or(CORRELATION_MINIMUM_CLOSES);
    // Returns over the last `window` closes, or the reason there are none.
    // A non-positive close cannot be divided by, and skipping the step — as
    // the kernel's own return series does for a single instrument — would
    // misalign this series against every other in the matrix.
    let mut series: Vec<(&String, Vec<f64>)> = Vec::new();
    for (instrument, closes) in eligible {
        let recent = &closes[closes.len() - window..];
        if recent.iter().any(|close| *close <= 0.0) {
            excluded.push(format!(
                r#"{{"instrument":{},"closes":{},"reason":{}}}"#,
                json::string(instrument),
                closes.len(),
                json::string(
                    "a non-positive close inside the window; a return cannot be taken from it"
                )
            ));
            continue;
        }
        let returns: Vec<f64> = recent
            .windows(2)
            .map(|pair| (pair[1] - pair[0]) / pair[0])
            .collect();
        series.push((instrument, returns));
    }
    if series.len() < 2 {
        return format!(
            r#"{{"subject":"correlation","available":false,"reason":{},"as_of_cycle":{},"minimum_closes":{},"instruments_observed":[{}],"excluded":[{}]}}"#,
            json::string(crate::missing::TOO_FEW_SERIES),
            platform.cycle_count(),
            CORRELATION_MINIMUM_CLOSES,
            observed.join(","),
            excluded.join(",")
        );
    }
    let moments: Vec<(f64, f64)> = series
        .iter()
        .map(|(_, returns)| {
            let n = returns.len() as f64;
            let mean = returns.iter().sum::<f64>() / n;
            let deviation = (returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n).sqrt();
            (mean, deviation)
        })
        .collect();
    let mut undefined: Vec<String> = Vec::new();
    let rows: Vec<String> = series
        .iter()
        .enumerate()
        .map(|(i, (a, returns_a))| {
            let cells: Vec<String> = series
                .iter()
                .enumerate()
                .map(|(j, (b, returns_b))| {
                    let (mean_a, sd_a) = moments[i];
                    let (mean_b, sd_b) = moments[j];
                    let coefficient = if sd_a <= 0.0 || sd_b <= 0.0 {
                        if i <= j {
                            undefined.push(format!(
                                r#"{{"a":{},"b":{},"reason":"zero return variance inside the window on at least one side"}}"#,
                                json::string(a),
                                json::string(b)
                            ));
                        }
                        "null".to_string()
                    } else if i == j {
                        // The identity, stated exactly. Computing it would
                        // print 0.9999999999999998 for some series and a
                        // reader would wonder what the platform was unsure of.
                        json::number(1.0)
                    } else {
                        let n = returns_a.len() as f64;
                        let covariance = returns_a
                            .iter()
                            .zip(returns_b)
                            .map(|(ra, rb)| (ra - mean_a) * (rb - mean_b))
                            .sum::<f64>()
                            / n;
                        json::number(covariance / (sd_a * sd_b))
                    };
                    format!("{}:{}", json::string(b), coefficient)
                })
                .collect();
            format!("{}:{{{}}}", json::string(a), cells.join(","))
        })
        .collect();
    let instruments: Vec<String> = series
        .iter()
        .map(|(instrument, _)| json::string(instrument))
        .collect();
    format!(
        r#"{{"available":true,"as_of_cycle":{},"statistic":"pearson correlation of simple returns between consecutive closes","alignment":"by position from the most recent close backwards; the tape keeps closes without their instants, so two series are aligned by count rather than by timestamp","window_closes":{},"window_returns":{},"minimum_closes":{},"instruments":[{}],"matrix":{{{}}},"excluded":[{}],"undefined":[{}]}}"#,
        platform.cycle_count(),
        window,
        window - 1,
        CORRELATION_MINIMUM_CLOSES,
        instruments.join(","),
        rows.join(","),
        excluded.join(","),
        undefined.join(",")
    )
}

/// Per strategy: the holdout evidence submitted, the trial account it was
/// charged under, the band its holdout admission produced, and every move the
/// ledger recorded with the gate findings that admitted it.
///
/// Nothing here is recomputed. The holdout Sharpe is the band's centre — the
/// annualised figure the gate admitted on — and the deflated Sharpe is served
/// as the gate wrote it into its own finding, because the ledger keeps no
/// numeric copy and a second computation here would be a second claim about
/// the same evidence. No equity curve is served because none was kept.
fn backtests(platform: &Platform) -> String {
    let factory = platform.central().factory();
    let ledger = factory.ledger();
    let strategies: Vec<String> = factory
        .candidates()
        .map(|candidate| {
            let id = candidate.strategy();
            let evidence = candidate.evidence();
            let holdout = match evidence.holdout.as_ref() {
                None => r#"{"submitted":false}"#.to_string(),
                Some(holdout) => format!(
                    r#"{{"submitted":true,"observations":{},"trials_this_run":{},"periods_per_year":{},"cross_validation":{{"folds":{},"observations":{},"purged":{},"embargoed":{}}},"leakage_findings":[{}]}}"#,
                    holdout.holdout_returns.len(),
                    holdout.trials,
                    json::number(holdout.periods_per_year),
                    holdout.cross_validation.folds,
                    holdout.cross_validation.observations,
                    holdout.cross_validation.purged,
                    holdout.cross_validation.embargoed,
                    holdout
                        .leakage
                        .findings()
                        .iter()
                        .map(|finding| json::string(finding))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            };
            // The account on the submitted evidence, and the family's count on
            // the book, are two different facts and are served as two. The
            // gate charges the book directly and does not write the account
            // back onto the factory's copy of the evidence, so on the ordinary
            // path the first is absent while the second has moved — and
            // "unknown" for the first must not be read as "uncharged".
            let account = match evidence.trial_account.as_ref() {
                None => format!(
                    r#"{{"on_evidence":false,"reason":{}}}"#,
                    json::string(
                        "the submitted evidence carries no trial account of its own; the \
                         gate charges the family's trial book directly, and the lifetime \
                         count it deflated against is `family_lifetime_trials`"
                    )
                ),
                Some(account) => format!(
                    r#"{{"on_evidence":true,"lifetime":{},"this_run":{},"prior":{},"charged_at":{}}}"#,
                    account.lifetime(),
                    account.this_run(),
                    account.prior(),
                    json::string(&account.charged_at().to_rfc3339())
                ),
            };
            let family_lifetime_trials = ledger
                .trial_book()
                .and_then(|book| book.lifetime_trials(candidate.family()))
                .map_or_else(|| "null".to_string(), |count| count.to_string());
            let band = match ledger.holdout_band(id) {
                None => format!(
                    r#"{{"present":false,"reason":{}}}"#,
                    json::string(
                        "the strategy has not been admitted through the holdout gate, so no \
                         band was produced and there is no holdout Sharpe on record"
                    )
                ),
                Some(band) => format!(
                    r#"{{"present":true,"sharpe":{},"lower":{},"upper":{},"standard_error":{},"observations":{},"periods_per_year":{},"trials":{},"method":{},"as_of":{}}}"#,
                    json::number(band.centre),
                    json::number(band.lower),
                    json::number(band.upper),
                    json::number(band.standard_error),
                    band.observations,
                    json::number(band.periods_per_year),
                    band.trials,
                    json::string(&band.method.describe()),
                    json::string(&band.as_of.to_rfc3339())
                ),
            };
            let moves: Vec<String> = ledger
                .history(id)
                .iter()
                .map(|entry| {
                    let gate = match entry.outcome.as_ref() {
                        None => "null".to_string(),
                        Some(outcome) => format!(
                            r#"{{"stage":{},"passed":{},"findings":[{}]}}"#,
                            json::string(outcome.stage.as_str()),
                            outcome.passed,
                            outcome
                                .findings
                                .iter()
                                .map(|(check, passed, detail)| {
                                    format!(
                                        r#"{{"check":{},"passed":{},"detail":{}}}"#,
                                        json::string(check),
                                        passed,
                                        json::string(detail)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                    };
                    format!(
                        r#"{{"from":{},"to":{},"at":{},"approver":{},"rationale":{},"gate":{}}}"#,
                        json::string(entry.promotion.from.as_str()),
                        json::string(entry.promotion.to.as_str()),
                        json::string(&entry.promotion.at.to_rfc3339()),
                        entry
                            .promotion
                            .approver
                            .as_deref()
                            .map_or_else(|| "null".to_string(), json::string),
                        json::string(&entry.promotion.rationale),
                        gate
                    )
                })
                .collect();
            format!(
                r#"{{"strategy":{},"family":{},"cell":{},"venue":{},"stage":{},"registered_at":{},"holdout":{},"trial_account":{},"family_lifetime_trials":{},"holdout_band":{},"ledger":[{}]}}"#,
                json::string(id.as_str()),
                json::string(candidate.family().as_str()),
                json::string(candidate.cell()),
                json::string(candidate.venue().as_str()),
                json::string(factory.stage_of(id).as_str()),
                json::string(&candidate.registered_at().to_rfc3339()),
                holdout,
                account,
                family_lifetime_trials,
                band,
                moves.join(",")
            )
        })
        .collect();
    let trial_book = match ledger.trial_book() {
        None => r#"{"attached":false}"#.to_string(),
        Some(book) => format!(
            r#"{{"attached":true,"durable":{},"families":[{}]}}"#,
            book.is_durable(),
            book.families()
                .map(|family| {
                    format!(
                        r#"{{"family":{},"lifetime_trials":{}}}"#,
                        json::string(family.as_str()),
                        book.lifetime_trials(family)
                            .map_or_else(|| "null".to_string(), |count| count.to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    // The factory is held in this process, so an empty list is an observed
    // zero: nothing has been registered.
    format!(
        r#"{{"strategies":[{}],"trial_book":{},"deflated_sharpe":{},"equity_curve":{}}}"#,
        strategies.join(","),
        trial_book,
        format_args!(
            r#"{{"available":false,"reason":{}}}"#,
            json::string(
                "the holdout gate computes the deflated Sharpe at admission and records it in \
                 its `deflated_sharpe_above_selection` finding, served under each ledger \
                 entry's gate; the ledger keeps no numeric copy and this process does not \
                 recompute one. The band's `sharpe` is the annualised holdout Sharpe the \
                 gate admitted on."
            )
        ),
        format_args!(
            r#"{{"available":false,"reason":{}}}"#,
            json::string(
                "no equity curve is kept. The ledger records returns as the evidence a gate \
                 saw and the band it produced, not a path a page could plot; a curve drawn \
                 here would be one nobody computed."
            )
        )
    )
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::error::Result;
    use qip_core::{Context, Timestamp};
    use qip_financial::universe::Universe;
    use qip_kernel::config::PlatformConfig;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Result<Platform> {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            // Not `silent()`: the metric registry inside a silent telemetry is
            // a real one — only the logger is quietened — and this test is
            // about what reaches that registry.
            Telemetry::new("qip-api-test", context_clock()),
            Universe::new(),
            LimitSet::conservative_default(),
        )
    }

    fn context_clock() -> std::sync::Arc<dyn qip_core::Clock> {
        std::sync::Arc::new(qip_core::ManualClock::new(start()))
    }

    #[test]
    fn the_scrape_surface_serves_what_the_platform_recorded_and_not_counts_computed_beside_it()
    -> Result<()> {
        let mut platform = platform()?;

        // The premise, and the defect this endpoint had: a platform that has
        // run nothing serves nothing it ran. It used to serve eight counts
        // recomputed from state, so it was never empty and never evidence of
        // anything — a scrape of a process that had done nothing looked
        // identical to a scrape of one that had. Since `78026e2` assembly
        // itself records one fact — the count of instruments the universe
        // may not trade on — so the honest state before the first cycle is
        // exactly that gauge and no cycle series, not an empty page.
        let before = scrape(&platform);
        assert!(
            before.contains("\nqip_universe_not_decision_grade 0\n"),
            "assembly records the unfit-instrument gauge, and the scrape must carry it: {before}"
        );
        assert!(
            !before.contains("qip_cycles_total"),
            "a process that has run no cycle must not serve a cycle count: {before}"
        );
        assert_eq!(
            before.matches("# TYPE ").count(),
            1,
            "before the first cycle the only series is the assembly gauge: {before}"
        );

        platform.run_cycle(start());
        let text = scrape(&platform);
        assert!(
            text.contains("# TYPE qip_cycles_total counter"),
            "the scrape surface carries no type declaration; a scraper cannot ingest it: {text}"
        );
        assert!(
            text.contains("\nqip_cycles_total 1\n"),
            "the cycle the platform ran did not reach the scrape surface: {text}"
        );
        Ok(())
    }

    #[test]
    fn the_scrape_surface_and_the_console_counts_answer_two_different_questions() -> Result<()> {
        // Both are served, and deliberately: `/system/metrics` is how big the
        // book is now, `/metrics` is what this process has done since it
        // started. Serving the first at the second's path is what made this
        // platform unscrapeable, so the test is that they are not the same
        // answer wearing two names.
        let mut platform = platform()?;
        platform.run_cycle(start());

        let console = metrics(&platform);
        let scraped = scrape(&platform);
        assert!(
            console.starts_with('{'),
            "the console surface stopped being JSON: {console}"
        );
        assert!(
            !scraped.starts_with('{'),
            "the scrape surface is JSON again: {scraped}"
        );
        assert!(
            !scraped.is_empty() && scraped != console,
            "the two surfaces returned the same body"
        );
        Ok(())
    }

    #[test]
    fn the_generated_document_says_the_scrape_surface_is_text_and_the_rest_is_json() {
        // A generated client told `/metrics` is JSON parses `# HELP` and
        // reports the endpoint broken — and it is the endpoint an operator
        // reaches for when they already believe something is broken.
        let document = crate::openapi::document();
        let scrape_path = format!("{VERSION_PREFIX}{SCRAPE_PATH}");
        assert!(
            document.contains(&scrape_path),
            "the scrape route is missing from the document"
        );

        // Split at the scrape path so the media type asserted is the one on
        // that operation rather than one anywhere in a document that mentions
        // both. A `contains("text/plain")` over the whole document would pass
        // however wrongly the route was described.
        let (_, after) = document
            .split_once(&format!("{}:", json::string(&scrape_path)))
            .expect("the path key is present");
        let operation = after
            .split_once("},\"/api/v1/")
            .map_or(after, |(operation, _)| operation);
        assert!(
            operation.contains("text/plain"),
            "the scrape operation is not declared as text: {operation}"
        );
        assert!(
            !operation.contains("application/json"),
            "the scrape operation is still declared as JSON: {operation}"
        );

        let (_, health) = document
            .split_once(&format!("{}:", json::string("/api/v1/health")))
            .expect("the health path is present");
        let health = health
            .split_once("},\"/api/v1/")
            .map_or(health, |(operation, _)| operation);
        assert!(
            health.contains("application/json"),
            "an ordinary route stopped being JSON: {health}"
        );
    }
}
