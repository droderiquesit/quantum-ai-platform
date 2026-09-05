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

pub mod arbitrage;
pub mod cell;
pub mod dropcopy;
pub mod envelope;
pub mod feasibility;
pub mod journal;
pub mod mesh;
pub mod policy;
pub mod reservation;
pub mod seam;
pub mod telemetry;

pub use arbitrage::ArbitrageDesk;
pub use cell::{
    Cell, CellConfig, ConfirmedFill, CrossingInterval, ExecutionReport, MAX_OPEN_ORDERS, OpenOrder,
    PlacedOrder, Placer, PolledHalt, PricingPolicy, WorkReport,
};
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
pub use policy::{VerifiedHalt, VerifiedPolicy};
pub use reservation::{RegionAllocation, RegionTable};
pub use seam::{CellLiquidity, value_kind, value_type};
pub use telemetry::CellMetrics;
