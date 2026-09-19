//! `qip-edge` — a source-adjacent edge cell.
//!
//! The hot execution path, assembled: bytes arrive on a venue feed and leave
//! as orders without a network hop to the central plane anywhere in between.
//! That is what the cell is for, and it is why every safety property here has
//! to hold locally — there is nobody to ask.
//!
//! What makes deciding alone safe is that a cell never decides *how much* it
//! may risk. It receives a [`VerifiedEnvelope`]: signed, bounded, venue-scoped
//! and expiring. The worst a cell cut off from the centre can do is spend an
//! amount somebody already approved, for as long as the envelope has left to
//! run. See `docs/adr/0008-edge-cells-decide-alone.md`.
//!
//! Four things are worth knowing before reading further:
//!
//! * **The hot path does no I/O.** [`Cell::on_bytes`] and [`Cell::work`] touch
//!   memory and arithmetic only. The journal is drained to durable storage by
//!   [`Cell::flush`], which is the one call that may block.
//! * **A stale book trades nothing.** After a sequence gap the book is marked
//!   stale and both the pricer and the router refuse it, so a price from
//!   before the gap cannot reach an order.
//! * **Refusals are recorded like decisions.** A cell must answer "why did
//!   nothing trade" as precisely as "why did this trade".
//! * **Nothing here can reach a language model.** `qip-edge` does not depend
//!   on `qip-ai`, directly or transitively, and the workspace architecture
//!   tests keep it that way.
//! * **A figure the cell cannot evaluate refuses; it never abstains.** The
//!   centre carries that discipline in `RiskState::unevaluated`, which nothing
//!   here imports and nothing here should — every figure [`Cell::work`] sizes
//!   against is measured from this cell's own book on this pass or refused
//!   under a named gate before an intent exists. The argument, the table of
//!   gates and what would overturn it are in
//!   `docs/architecture/edge-fail-closed-figures.md`, enforced by
//!   `tests/unevaluated.rs`.

pub mod arbitrage;
pub mod cell;
/// §32.1's size decomposition: what a cycle does after a leg fills short.
pub mod decomposition;
pub mod dispersion;
pub mod dropcopy;
pub mod envelope;
pub mod feasibility;
pub mod journal;
pub mod mesh;
pub mod mirror;
pub mod policy;
pub mod quoting;
/// Which other regions have gone dark, and what a cell does about it (§36.3).
pub mod region;
pub mod reservation;
/// What a cell must be shown before it resumes after a crash (§36.3).
pub mod resume;
pub mod seam;
pub mod telemetry;

pub use arbitrage::ArbitrageDesk;
pub use cell::{
    Cell, CellConfig, ConfirmedFill, CrossingInterval, ExecutionReport,
    GATE_AWAITING_RECONCILIATION, GATE_DARK_REGION, GATE_FILL_DISPERSION, GATE_LIVE_VENUE,
    GATE_MASS_CANCEL, GATE_PATH_EXTENSION, GATE_PATH_ROUTER, GATE_QUOTE_BUDGET, MAX_OPEN_ORDERS,
    OpenOrder, PlacedOrder, Placer, PolledHalt, PricingPolicy, RoutedCycle, WorkReport,
};
pub use decomposition::{
    Completion, DEFAULT_MINIMUM_VIABLE_FRACTION, Decomposition, DecompositionPolicy, LegSize,
};
pub use dispersion::{DispersionPolicy, DispersionVerdict, FillTimes, VenueFillTimeState};
pub use dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
pub use envelope::{VerifiedEnvelope, sign_payload};
pub use feasibility::{Granularity, Infeasible, VenueModel};
pub use journal::{Decision, FileMirror, Journal, JournalEntry, MemoryMirror, Mirror, MirrorBatch};
pub use mesh::{
    CapitalDownlink, CapitalGrantTopic, CellStateDelta, CellUplink, DeltaOrder, DeltaRefusal,
    Dispatch, DownlinkBatch, DownlinkConfig, DownlinkStats, HaltTopic, PolicyBatch, PolicyDownlink,
    PolicyDownlinkStats, PolicyPayloadTopic, RefusedGrant, RefusedPolicy, StrategyUtilisation,
    UplinkConfig, UplinkStats,
};
pub use mirror::{MirrorArrangement, MirroredInstrument};
pub use policy::{VerifiedHalt, VerifiedPolicy};
pub use quoting::{
    Admission, Depletion, MessageKind, QuoteBudget, REQUOTE_MESSAGES, RateLimits, VenueBudgetState,
};
pub use region::{DarkSource, RegionOutlook};
pub use reservation::{Rebase, RegionAllocation, RegionTable};
pub use resume::{ResumeDiscipline, VenueAccount};
pub use seam::{CellLiquidity, value_kind, value_type};
pub use telemetry::CellMetrics;
