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
