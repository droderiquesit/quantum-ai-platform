//! Serving the operator interface.
//!
//! The web surfaces sit alongside the API on the same server, under paths that
//! do not start with `/api`. They are read-only and require the `Viewer` role,
//! the same as the equivalent API endpoints — a page is not a lower bar than
//! the JSON it renders.
//!
//! The view model is assembled here, from a platform read under a lock that is
//! released before rendering. A rendering path that held the platform lock
//! could stall a trading loop behind an HTML page, which is a bad trade.

use crate::auth::{Authenticator, RateLimiter, Role};
use crate::cells::{CellRegistry, describe_age};
use crate::http::{Handler, Method, Request, Response, StreamDecision};
use qip_contracts::policy::PolicyItem;
use qip_core::time::Timestamp;
use qip_events::{EventBody, EventFilter};
use qip_kernel::Platform;
use qip_kernel::central::WhitelistIssue;
use qip_observability::Snapshot;
use qip_observability::metrics::{Labels, labels, names};
use qip_web::pages::{Surface, render};
use qip_web::panel::Panel;
use qip_web::view::{
    AgentRow, EdgeCellRow, Fact, FactRow, GovernanceRow, LimitRow, OpportunityRow, OrderRow,
    ProposalRow, ShippedPolicyRow, StageRow, UniverseExclusionRow, UniverseView, ViewModel,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// What the API holds beside the platform, lent to the interface per request.
///
/// The interface owns none of this. The cell registry and the mesh backbone
/// are the API's — reviewed stores, named in the boundary suite — and the
/// router lends them for the duration of one render so a page can say which
/// cells reported and what each last said about itself. Borrowed rather than
/// held because a `Web` built with its own copies would be a `Web` whose
/// copies nothing feeds: the stage overview rendered empty for the life of
/// the process once, for exactly that reason. A `Web` served without a
/// router gets [`Feeds::default`], and its pages say so in words.
#[derive(Clone, Copy, Debug, Default)]
pub struct Feeds<'a> {
    pub cells: Option<&'a CellRegistry>,
    pub mesh: Option<&'a Mutex<crate::mesh::MeshBackbone>>,
}

/// Serves the operator interface.
pub struct Web {
    platform: Arc<Mutex<Platform>>,
    authenticator: Arc<Authenticator>,
    rate_limiter: Arc<RateLimiter>,
    clock: Arc<dyn qip_core::Clock>,
    /// The last cycle's stages, so the overview has something to show between
    /// cycles. Shared with whatever runs cycles — see [`CycleOverview`].
    overview: Arc<CycleOverview>,
}

/// The stages of the last cycle, as the overview page shows them.
///
/// Kept outside the platform because it is a display concern the platform
/// should not carry, and outside [`Web`] because `Web` never runs a cycle: the
/// API's `POST /cycle` does. For the process lifetime this store was private
/// to `Web` and nothing wrote to it, so the stage overview rendered empty after
/// every cycle the process ran — indistinguishable, to an operator, from a
/// process that had never cycled. The handle is shared so the route that runs
/// the cycle is the one that records it.
#[derive(Debug, Default)]
pub struct CycleOverview {
    rows: Mutex<Vec<StageRow>>,
}

impl CycleOverview {
    /// Record a cycle's stages for the overview.
    ///
    /// A poisoned lock leaves the previous rows in place rather than failing
    /// the cycle: the cycle already ran, and a page is not allowed to fail it.
    pub fn record(&self, report: &qip_kernel::CycleReport) {
        if let Ok(mut rows) = self.rows.lock() {
            *rows = report
                .stages
                .iter()
                .map(|outcome| StageRow {
                    stage: outcome.stage.as_str().to_string(),
                    ran: outcome.ran,
                    produced: outcome.produced,
                    detail: outcome.detail.clone(),
                })
                .collect();
        }
    }

    /// The rows of the last recorded cycle; empty until one is recorded, and
    /// empty on a poisoned lock rather than a guess about what was there.
    pub fn rows(&self) -> Vec<StageRow> {
        self.rows
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for Web {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Web")
            .field("surfaces", &Surface::all().len())
            .finish_non_exhaustive()
    }
}

impl Web {
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
            overview: Arc::new(CycleOverview::default()),
        }
    }

    /// The store the overview page reads its stages from.
    ///
    /// Hand this to [`crate::routes::Api::with_cycle_overview`]; a `Web`
    /// whose overview nothing feeds shows no stages however many cycles run.
    pub fn cycle_overview(&self) -> Arc<CycleOverview> {
        self.overview.clone()
    }

    /// Build the view model from the platform.
    ///
    /// The lock is taken, everything needed is copied out, and it is released
    /// before rendering. Holding it across rendering would let an HTML page
    /// stall a trading loop.
    fn model(&self, feeds: Feeds<'_>) -> ViewModel {
        let now = self.clock.now();
        // The registry's lock is taken and released before the platform's.
        // Nothing here nests the two, and the platform-then-mesh order the
        // cycle route documents is kept by reading the mesh last, after the
        // platform lock has been dropped.
        let observations = feeds
            .cells
            .map(|cells| (cells.observations(), cells.freshness_bound()));
        let Ok(platform) = self.platform.lock() else {
            // A poisoned lock means a thread panicked while holding the
            // platform. Rendering the safe defaults says nothing untrue.
            return ViewModel {
                halted: true,
                halt_reason: "the platform is in an inconsistent state after an internal failure"
                    .to_string(),
                ..ViewModel::default()
            };
        };

        let controller = platform.autonomy();
        let switch = controller.kill_switch();
        let snapshot = platform.telemetry().metrics.snapshot();
        let policy_halt_flag = switch.is_globally_tripped();
        // Assembled under the lock from the switch and the observations, then
        // finished after it with the mesh's standings, so the two locks are
        // never held together.
        let mut cell_rows: Vec<EdgeCellRow> = observations
            .as_ref()
            .map(|(observations, bound)| {
                observations
                    .iter()
                    .map(|observation| EdgeCellRow {
                        cell: observation.cell.clone(),
                        reported_at: observation.at.to_rfc3339(),
                        age: describe_age(observation.age(now)),
                        stale: observation.is_stale(now, *bound),
                        positions: observation.positions,
                        strategies: observation.strategies,
                        breaks_shipped: observation.reconciliation_breaks,
                        orders_sent: Fact::not_recorded(NO_PER_CELL_SETTLEMENT),
                        fills_confirmed: Fact::not_recorded(NO_PER_CELL_SETTLEMENT),
                        halted_by_centre: switch.is_halted(&observation.cell),
                        policy_halt_flag,
                        cell_reports_halted: Fact::not_recorded(NO_MESH),
                        polled_halt_flag: Fact::not_recorded(POLLED_FLAG_STAYS_ON_THE_NODE),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let shipped_policy = shipped_policy(&platform, now);
        let universe = universe(&platform, now);
        let settlement = settlement_rows(&snapshot);

        let model = ViewModel {
            autonomy_level: controller.level().as_str().to_string(),
            autonomy_ceiling: controller.ceiling().as_str().to_string(),
            live: controller.is_live(),
            halted: switch.is_globally_tripped(),
            halt_reason: switch
                .global_trip()
                .map(|trip| format!("{} ({})", trip.reason, trip.tripped_by))
                .unwrap_or_default(),
            cycle: platform.cycle_count(),
            correlation_id: "see the event log".to_string(),
            events_logged: platform.event_log().len(),
            chain_intact: platform.event_log().verify_chain().is_ok(),
            rendered_at: now.to_string(),

            equity: "10,000,000".to_string(),
            position_count: 0,
            gross_exposure: 0.0,
            net_exposure: 0.0,
            paper_only: !platform.orders().has_live_fills(),

            stages: self.overview.rows(),
            opportunities: platform
                .queue()
                .iter()
                .map(|opportunity| OpportunityRow {
                    id: opportunity.opportunity_id.as_str().to_string(),
                    headline: opportunity.headline.clone(),
                    score: opportunity.rank.score,
                    confidence: opportunity.rank.confidence,
                    detectors: opportunity.detectors.clone(),
                })
                .collect(),
            theses: Vec::new(),
            proposals: platform
                .proposals()
                .iter()
                .map(|proposal| ProposalRow {
                    id: proposal.proposal_id.as_str().to_string(),
                    status: proposal.status().as_str().to_string(),
                    legs: proposal.len(),
                    rationale: proposal.rationale.clone(),
                })
                .collect(),
            orders: platform
                .orders()
                .orders()
                .map(|order| OrderRow {
                    id: order.order_id.as_str().to_string(),
                    instrument: order.object_id.as_str().to_string(),
                    side: order.side.as_str().to_string(),
                    quantity: order.quantity.to_string(),
                    state: order.state.as_str().to_string(),
                    simulated: order.is_paper() || order.fills.is_empty(),
                })
                .collect(),
            refusals: platform
                .orders()
                .refusals()
                .iter()
                .filter_map(|refusal| refusal.refusal.as_ref().map(|reason| reason.describe()))
                .collect(),
            limits: limit_rows(&platform),
            agents: platform
                .organisation()
                .roster()
                .iter()
                .map(|manifest| AgentRow {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    role: manifest.role.as_str().to_string(),
                    owner: manifest.owner.clone(),
                    purpose: manifest.purpose.clone(),
                    capabilities: manifest
                        .capabilities
                        .iter()
                        .map(|capability| capability.as_str().to_string())
                        .collect(),
                })
                .collect(),
            governance: platform
                .review_governance(now)
                .iter()
                .map(|finding| GovernanceRow {
                    severity: match finding.severity {
                        qip_agents::governance::Severity::Error => "error".to_string(),
                        qip_agents::governance::Severity::Warning => "warning".to_string(),
                    },
                    rule: finding.rule.clone(),
                    detail: finding.detail.clone(),
                })
                .collect(),
            cells: Panel::default(),
            settlement,
            shipped_policy,
            universe,
        };
        drop(platform);

        // The cell's own account of itself travels only on its delta, which
        // only the mesh decodes. Read after the platform lock is released.
        if let Some(mesh) = feeds.mesh {
            let standings: BTreeMap<String, (bool, u64)> = mesh
                .lock()
                .map(|mesh| {
                    mesh.status()
                        .standings
                        .into_iter()
                        .map(|standing| (standing.cell, (standing.halted, standing.sequence)))
                        .collect()
                })
                .unwrap_or_default();
            for row in &mut cell_rows {
                row.cell_reports_halted = match standings.get(&row.cell) {
                    Some((halted, sequence)) => Fact::recorded(format!(
                        "{} (delta {sequence})",
                        if *halted { "yes" } else { "no" }
                    )),
                    None => Fact::not_recorded(
                        "the mesh has decoded no delta from this cell, so the cell has not \
                         said whether it stopped itself",
                    ),
                };
            }
        }
        let cells = match observations {
            None => Panel::absent(
                "no cell registry is lent to this interface; served through the router beside \
                 the API it reads the API's registry, and served alone it has none",
            ),
            Some((observations, _)) if observations.is_empty() => {
                Panel::absent(crate::missing::NO_CELL_REPORTS)
            }
            Some(_) => Panel::current(cell_rows, now.to_rfc3339()),
        };
        ViewModel { cells, ..model }
    }

    /// Render one surface with what the API lends for this request.
    ///
    /// [`Handler::handle`] is this with nothing lent, for a `Web` served on
    /// its own; the router calls this directly with the API's registry and
    /// mesh so the deployed pages read what the deployed API holds.
    pub fn serve(&self, request: &Request, feeds: Feeds<'_>) -> Response {
        let now = self.clock.now();

        if request.method != Method::Get {
            return Response::text(405, "the operator interface is read-only");
        }
        let Some(surface) = Surface::from_path(&request.path) else {
            return Response::html(404, render(Surface::Overview, &ViewModel::default()));
        };

        let principal = match self
            .authenticator
            .authenticate(request.header("authorization"), now)
        {
            Ok(principal) => principal,
            Err(_) => {
                return Response::text(401, "authentication required")
                    .with_header("www-authenticate", "Bearer");
            }
        };
        if !self.rate_limiter.permit(&principal.subject, now) {
            return Response::text(429, "rate limit exceeded");
        }
        // A page is not a lower bar than the JSON it renders.
        if principal.require(Role::Viewer).is_err() {
            return Response::text(403, "the viewer role is required");
        }

        // Every surface shows the same view. Role-dependent redaction would
        // mean two readers of the same page disagreeing about what the
        // platform did, which is worse than one bar for the whole interface.
        Response::html(200, render(surface, &self.model(feeds)))
    }
}

/// Why a per-cell settlement figure is not on the page.
const NO_PER_CELL_SETTLEMENT: &str = "the platform counts orders sent and fill shares \
    attributed across every cell, under qip_central_orders_sent_total and \
    qip_central_fills_attributed_total, and retains no per-cell settlement: the last \
    ingestion's Settlement is returned to the drain and kept nowhere";

/// Why a cell's own halted flag is not on the page.
const NO_MESH: &str = "this process serves no mesh, so no delta from the cell has reached it; \
    the cell's own halted flag travels only on its delta";

/// Why the polled halt flag is never on this page.
const POLLED_FLAG_STAYS_ON_THE_NODE: &str = "the polled halt flag is a file the node reads off \
    its own filesystem; it is never shipped to the centre, and the centre cannot see it";

/// Why the catalogue's identity is not on the page.
const MANIFEST_NOT_ON_THE_PLATFORM: &str = "the catalogue manifest is recorded by the \
    composition root in the universe key-value store, and the universe itself sits behind \
    the desk's capability gate; neither is readable from the platform this page is \
    rendered from";

/// One series from the platform's registry, as a fact.
///
/// A series the registry holds is rendered as its value. A series it does
/// not hold is rendered as the reason: a counter that never incremented has
/// no value, and `0` would be a claim the platform did not make. The same
/// rule the scrape surface follows, applied to the page.
fn counter_fact(snapshot: &Snapshot, name: &str, series_labels: &Labels) -> Fact {
    match snapshot.get(name, series_labels) {
        Some(qip_observability::MetricValue::Counter(value)) => Fact::recorded(value.to_string()),
        _ => Fact::not_recorded(format!(
            "the platform has recorded no {name}{} series; a counter that never incremented \
             has no value, and zero would be a claim",
            describe_labels(series_labels)
        )),
    }
}

/// A series summed over every label set it was recorded under.
fn counter_total_fact(snapshot: &Snapshot, name: &str) -> Fact {
    if snapshot.series.iter().any(|series| series.name == name) {
        Fact::recorded(snapshot.counter_total(name).to_string())
    } else {
        Fact::not_recorded(format!(
            "the platform has recorded no {name} series; a counter that never incremented has \
             no value, and zero would be a claim"
        ))
    }
}

fn describe_labels(series_labels: &Labels) -> String {
    if series_labels.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = series_labels
        .iter()
        .map(|(key, value)| format!("{key}=\"{value}\""))
        .collect();
    format!("{{{}}}", pairs.join(","))
}

/// What the central plane recorded settling every cell's reports.
///
/// Read off the platform's own registry rather than recomputed from state,
/// for the reason `/metrics` gives: two claims about one fact will disagree.
fn settlement_rows(snapshot: &Snapshot) -> Vec<FactRow> {
    let mut rows = vec![
        FactRow::new(
            "central_orders_sent",
            "Orders sent",
            counter_total_fact(snapshot, names::CENTRAL_ORDERS_SENT),
        ),
        FactRow::new(
            "central_fills_attributed",
            "Fill shares attributed",
            counter_total_fact(snapshot, names::CENTRAL_FILLS_ATTRIBUTED),
        ),
        FactRow::new(
            "central_crosses_settled",
            "Crosses settled",
            counter_total_fact(snapshot, names::CENTRAL_CROSSES_SETTLED),
        ),
        FactRow::new(
            "central_settlements_refused",
            "Settlements refused",
            counter_total_fact(snapshot, names::CENTRAL_SETTLEMENTS_REFUSED),
        ),
    ];
    for direction in [
        qip_kernel::central::BreakDirection::CellOverVenue,
        qip_kernel::central::BreakDirection::VenueOverCell,
        qip_kernel::central::BreakDirection::DetailOnly,
        qip_kernel::central::BreakDirection::UnsentFill,
    ] {
        rows.push(FactRow::new(
            format!("central_breaks_{}", direction.as_str()),
            format!("Reconciliation breaks: {}", direction.as_str()),
            counter_fact(
                snapshot,
                names::CENTRAL_RECONCILIATION_BREAKS,
                &labels([("direction", direction.as_str())]),
            ),
        ));
    }
    rows.push(FactRow::new(
        "central_cell_halts_reconciliation",
        "Cell halts: reconciliation",
        counter_fact(
            snapshot,
            names::CENTRAL_CELL_HALTS,
            &labels([("cause", "reconciliation")]),
        ),
    ));
    rows
}

/// The last cycle whitelist the platform journaled for each cell.
///
/// The one slot of the twelve the platform records as it produces it. The
/// other eleven are assembled at the shipping seam from platform facts and
/// are not journaled, so they are rendered as not recorded rather than as
/// produced: the page attests what the journal holds.
fn shipped_policy(platform: &Platform, now: Timestamp) -> Panel<ShippedPolicyRow> {
    let mut latest: BTreeMap<String, WhitelistIssue> = BTreeMap::new();
    // Read through the journal rather than off the event log's records.
    //
    // The log stores what `StreamEnvelope::to_frame` produced, so a record's
    // payload is the sealed envelope and not the body inside it; decoding a
    // record straight into the body fails on the first field it cannot find
    // ("missing field `cell`"), which is a silent empty panel when the
    // failure is swallowed. `replay_journal` is the seam that unwraps, and it
    // is what this panel's own promise — that it attests what the journal
    // holds — actually names.
    let replayed = match platform.replay_journal(&EventFilter::new().topic(WhitelistIssue::TOPIC)) {
        Ok(replayed) => replayed,
        Err(error) => {
            return Panel::absent(format!(
                "the journal could not be replayed for policy issues, so this page cannot say \
                 what was shipped: {error}"
            ));
        }
    };
    for envelope in replayed {
        let Ok(envelope) = envelope.decode::<WhitelistIssue>() else {
            continue;
        };
        let issue = envelope.body;
        let newer = latest
            .get(&issue.cell)
            .is_none_or(|held| issue.issued_at >= held.issued_at);
        if newer {
            latest.insert(issue.cell.clone(), issue);
        }
    }
    if latest.is_empty() {
        return Panel::absent(
            "the platform has journaled no policy issue: no cycle has shipped policy to a cell \
             from this process, or no mesh is served",
        );
    }
    let rows = latest
        .into_values()
        .map(|issue| {
            let issued_at = issue.issued_at.to_rfc3339();
            let slots = PolicyItem::all()
                .into_iter()
                .map(|item| {
                    let fact = if item == PolicyItem::CycleWhitelist {
                        Fact::recorded(format!("produced at {issued_at}"))
                    } else {
                        Fact::not_recorded(
                            "the platform journals slot 8 (cycle_whitelist) as it issues it and \
                             records no other slot; the shipping seam assembles this one \
                             without a journal entry",
                        )
                    };
                    FactRow::new(item.as_str(), item.as_str(), fact)
                })
                .collect();
            ShippedPolicyRow {
                cell: issue.cell.clone(),
                issued_at,
                sequence: Fact::not_recorded(
                    "the payload's sequence is assigned at the shipping seam as the issue \
                     instant in nanoseconds and is not journaled; the journal holds the issue \
                     instant",
                ),
                whitelist: issue.describe(),
                slots,
            }
        })
        .collect();
    Panel::current(rows, now.to_rfc3339())
}

/// The universe the platform assembled, as far as the platform can attest.
///
/// The not-decision-grade list is the platform's own, taken at assembly. The
/// catalogue's version, hash and size are not: the manifest is recorded by
/// the composition root and the universe sits behind the desk's gate, so
/// both are rendered as the reason rather than as a figure read past a
/// control.
fn universe(platform: &Platform, now: Timestamp) -> UniverseView {
    let rows = platform
        .universe_not_decision_grade()
        .iter()
        .map(|(object, reason)| UniverseExclusionRow {
            object: object.clone(),
            reason: reason.clone(),
        })
        .collect();
    UniverseView {
        version: Fact::not_recorded(MANIFEST_NOT_ON_THE_PLATFORM),
        sha256: Fact::not_recorded(MANIFEST_NOT_ON_THE_PLATFORM),
        instruments: Fact::not_recorded(MANIFEST_NOT_ON_THE_PLATFORM),
        not_decision_grade: Panel::current(rows, now.to_rfc3339()),
    }
}

/// The limits the platform runs under, with their rationales.
fn limit_rows(_platform: &Platform) -> Vec<LimitRow> {
    qip_risk::limits::LimitSet::conservative_default()
        .limits
        .iter()
        .map(|limit| LimitRow {
            name: limit.name.clone(),
            observed: 0.0,
            bound: 0.0,
            utilisation: 0.0,
            breached: false,
            rationale: limit.rationale.clone(),
        })
        .collect()
}

impl Handler for Web {
    /// A `Web` served without the router lends itself nothing; its pages say
    /// which panels that leaves unreported.
    fn handle(&self, request: &Request) -> Response {
        self.serve(request, Feeds::default())
    }
}

/// Routes between the API, the operator interface and the operator console on
/// one server.
///
/// One process serves all three because they are one deployment and one
/// credential set: an operator who can read the console can read the JSON
/// behind it, and splitting them would mean two things to authenticate against
/// and two chances to configure the authority differently.
pub struct Router {
    api: Arc<crate::routes::Api>,
    web: Arc<Web>,
    /// The console, where one is mounted.
    ///
    /// Optional because the console needs a cell registry to say how old a
    /// cell's book is, and a caller assembling a router without one should get
    /// a refusal on `/console` rather than a console that cannot answer the
    /// question it exists to answer.
    console: Option<Arc<crate::console::Console>>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router").finish_non_exhaustive()
    }
}

impl Router {
    pub fn new(api: Arc<crate::routes::Api>, web: Arc<Web>) -> Self {
        Self {
            api,
            web,
            console: None,
        }
    }

    /// Serve the operator console under `/console`.
    pub fn with_console(mut self, console: Arc<crate::console::Console>) -> Self {
        self.console = Some(console);
        self
    }
}

impl Handler for Router {
    fn handle(&self, request: &Request) -> Response {
        if request.path.starts_with("/api") {
            return self.api.handle(request);
        }
        if crate::console::Console::owns(&request.path) {
            return match &self.console {
                Some(console) => console.handle(request),
                // Not a 404 that pretends the path does not exist, and not the
                // overview page the surfaces answer an unknown path with: the
                // console is a thing this build has and this process was not
                // given, and an operator looking for it should be told that.
                None => Response::text(503, "the operator console is not mounted in this process"),
            };
        }
        // The API's registry and mesh are lent for this one render, so the
        // pages read the same stores the JSON routes serve from.
        self.web.serve(
            request,
            Feeds {
                cells: Some(self.api.cells()),
                mesh: self.api.mesh(),
            },
        )
    }

    /// Forward the streaming decision to the API.
    ///
    /// Only the API has streams. Without this the router would answer every
    /// stream request with `NotAStream` and the server would fall back to the
    /// buffered path, which serves the stream's descriptor — a correct-looking
    /// `200` that never sends a second event. That is the failure this
    /// forwarding prevents, and it is invisible in every test that calls the
    /// API directly rather than through the router.
    fn stream(&self, request: &Request) -> StreamDecision {
        if request.path.starts_with("/api") {
            return self.api.stream(request);
        }
        StreamDecision::NotAStream
    }
}
