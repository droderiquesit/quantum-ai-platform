//! Why a panel or an endpoint has nothing to show.
//!
//! Every reason the console and the API give for having no data is written
//! here, once. Two things follow from keeping them together.
//!
//! The first is that a page and the JSON behind it cannot drift into
//! disagreeing about why a number is missing, which is the sort of
//! disagreement an operator notices at the worst possible moment.
//!
//! The second is that this module is a readable inventory of what the platform
//! does not yet expose. Each constant names a subsystem the console was asked
//! to show and the specific reason it cannot: either the data is behind a
//! control the console will not reach past, or the subsystem is not wired into
//! this process. Both are honest answers; neither is a zero.

/// The desk's facilities — market state, the book, the risk state, the
/// reference universe — are behind capability gates.
///
/// An HTTP handler holds no agent identity, so it cannot pass one. Reaching
/// past the gate to serve a page would defeat the control for every agent that
/// respects it, which is a bad trade for a number on a screen.
pub const DESK_GATED: &str = "the desk holds this behind a capability gate. \
    This process serves HTTP requests under no agent identity, so it cannot \
    pass one, and it does not reach past a control to fill in a panel. Read it \
    through an agent run.";

/// No cell has reported to this process.
pub const NO_CELL_REPORTS: &str = "no edge cell has reported to this process. \
    The central plane's view of a cell comes from a CellReport the cell pushes; \
    until one arrives there is no book, no utilisation and no latency to show. \
    This is a silent feed, not a flat one.";

/// The arbitrage engine is not part of the assembled platform.
pub const NO_ARBITRAGE_ENGINE: &str = "no arbitrage engine is wired into this \
    process. The kernel assembles the intelligence loop and the central plane; \
    path construction and hedging run in an edge cell, which reports positions \
    rather than paths.";

/// Nothing registers data sources here.
pub const NO_DATA_FINDER: &str = "no data-source registry is wired into this \
    process. Discovery, approval and rejection are recorded by the data finder \
    service, which this deployment does not run, so there is no source list to \
    show — not an empty one.";

/// Nothing records training runs here.
pub const NO_TRAINING_SERVICE: &str = "no training service is wired into this \
    process. Training status is reported by the learning pipeline, which this \
    deployment does not run.";

/// No quantum job record is kept.
pub const NO_QUANTUM_JOBS: &str = "no quantum job record is reachable. The \
    compute router that would submit one is owned by the portfolio constructor \
    and keeps no externally readable job log, so there is nothing to compare \
    against a classical run — and a quantum result without that comparison is \
    not shown at all.";

/// There is no registry of which models are attached.
pub const NO_MODEL_REGISTRY: &str = "the platform attaches a language model to \
    the agent organisation but exposes no registry of which models are \
    attached, nor any contextual reputation record. What the agents actually \
    spent is on the agent-call panel, which is a record of use rather than a \
    roster.";

/// Attribution is not exposed.
pub const NO_ATTRIBUTION: &str = "profit, loss and realised alpha are computed \
    by the attributor inside the cycle and are not exposed by the platform; the \
    book they would be measured against is behind the desk's capability gate. \
    Showing a zero here would be the difference between a flat book and an \
    unmeasured one.";

/// No limit utilisation is readable.
pub const NO_LIMIT_UTILISATION: &str = "the configured limit set is held by the \
    risk monitor and its live utilisation by the desk's capability-gated risk \
    view. Neither is reachable from an HTTP handler, so this shows no limits \
    rather than showing every limit at zero per cent — which would read as \
    headroom the platform has not measured.";

/// No service registry reports to this process.
pub const NO_SERVICE_REGISTRY: &str = "no service registry reports here. This \
    process knows the health of its own components, which are listed, and \
    nothing about any other process.";

/// No transport health is reported.
pub const NO_TRANSPORT_HEALTH: &str = "no transport reports its health to this \
    process. The in-tree HTTP server is the only transport it owns, and it \
    publishes no health record.";

/// No cluster membership is reported.
pub const NO_CLUSTER_HEALTH: &str = "no cluster membership or mesh health is \
    reported to this process. It runs as a single node and knows of no others.";

/// No regime classifier keeps state in this process.
///
/// The kernel does label a routing decision with a market and volatility
/// regime — `qip_cost_router::Conditions` is keyed on it — but the label is
/// computed on demand from the surprise series when an opportunity is routed,
/// carries no confidence, and is kept nowhere. Serving it per instrument would
/// present a routing default (`quiet`/`normal` below the evidence floor) as a
/// classification, and nothing in the cycle publishes `regime.changed`.
pub const NO_REGIME_CLASSIFIER: &str = "no regime classifier runs in this \
    process. The kernel labels each routing decision with a market and \
    volatility regime computed on demand from its surprise series, with no \
    confidence and no state kept between decisions, and the label below the \
    evidence floor is a default rather than a finding. Nothing in the cycle \
    publishes `regime.changed`: the topic is declared on /stream/signals so a \
    subscriber's filter admits it, and the stream carries none until an \
    UNDERSTAND-stage classifier records one to the event log. That classifier \
    is what would produce this view.";

/// No narrative adapter feeds this process.
pub const NO_NARRATIVE_ADAPTER: &str = "no narrative adapter is configured \
    in this process. The kernel absorbs a news item only when an ingestion \
    adapter hands it one, and this composition attaches none — the API's \
    SENSE stage reads no vendor feed — so there is no headline, no entity \
    and no sentiment to show, not a quiet tape.";

/// No calibration has been computed yet.
pub const NO_CALIBRATION: &str = "no calibration has been computed. The LEARN \
    stage grades a claim only once its horizon has passed and the platform's \
    own series can settle it informatively; until one has, the platform has \
    written down confidences and has not yet learned whether they held.";

/// Too few instruments carry enough closes for a correlation to be estimated.
pub const TOO_FEW_SERIES: &str = "fewer than two instruments have enough \
    closes for a correlation to be estimated. A coefficient over a handful of \
    prints is a number with no evidence behind it, so the view refuses below \
    the stated minimum rather than reporting one; the instruments the platform \
    has seen, and how many closes each holds, are listed so the shortfall is \
    a fact rather than a blank.";

/// The mesh backbone is not configured in this process.
pub const MESH_NOT_SERVED: &str = "this process serves no mesh. QIP_MESH_CELLS \
    is not set, so no cell has an address to publish state deltas to or to poll \
    capital from, and nothing here drains an inbox or dispatches an envelope. \
    Cells pointed at this deployment are effectively partitioned: they keep \
    trading inside the envelopes they already hold and stop when those expire.";
