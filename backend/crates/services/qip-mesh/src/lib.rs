//! `qip-mesh` — point-in-time data mesh ports.
//!
//! Six narrow traits, one per access pattern, because a columnar scan over
//! four years and a sub-millisecond read of the last quote want opposite
//! things from a storage engine. Each has an in-memory and a file-backed
//! adapter here; the managed services the architecture names are declared in
//! [`provider`] and return [`qip_core::Error::Unavailable`] saying exactly
//! which credential and project they need.
//!
//! Three properties are worth stating plainly, because everything else in the
//! crate follows from them:
//!
//! * **Every read takes an as-of.** There is no `latest(&self, key)` anywhere
//!   in [`ports`]. That method would be the most convenient one here and the
//!   one through which every look-ahead bug arrives, because it answers "what
//!   is true now" to a caller reconstructing what was true then. Reads return
//!   [`qip_contracts::Stamped`], so the answer carries how current it is.
//! * **Evidence is write-once.** [`ports::EvidenceStore`] has no `delete` and
//!   no overwrite. Writing identical bytes again succeeds and changes nothing;
//!   writing different bytes to an existing key is refused, naming the key and
//!   both digests. An evidence layer its own operators can revise proves
//!   nothing.
//! * **No silent fallback.** A provider configured for BigQuery does not
//!   quietly hand back a JSON file. `qip-storage` calls that hazard out and
//!   this crate honours it: the failure happens where somebody can fix it.
//!
//! [`spine`] is the odd one out and is named here so nobody has to discover it:
//! it is not a data-mesh port at all but the central plane's half of ADR 0011's
//! *network* mesh — the wire that carries state deltas up from the regional
//! cells and signed capital envelopes back down. It shares a word with the rest
//! of this crate and nothing else. [`delta`] sits beside it and decodes what
//! the spine absorbed, keeping the delta's absolute and incremental halves in
//! separate types so a receiver cannot accumulate the one it must replace.
//!
//! [`catalog::Catalog`] ties them together — which datasets exist, what
//! produced what, what each may be used for (in
//! [`qip_contracts::Entitlement`], the type `qip-compliance` enforces), and
//! whether each is currently trustworthy.

pub mod adapters;
pub mod backing;
pub mod catalog;
pub mod delta;
pub mod ports;
pub mod provider;
pub mod spine;
pub mod state;

pub use adapters::{
    FileAnalytics, FileEvidence, FileGraph, FileHotSeries, FileLakehouse, FileMasterData,
    MemoryAnalytics, MemoryEvidence, MemoryGraph, MemoryHotSeries, MemoryLakehouse,
    MemoryMasterData, MeshAnalytics, MeshEvidence, MeshGraph, MeshHotSeries, MeshLakehouse,
    MeshMasterData,
};
pub use backing::{FileBacking, MemoryBacking, StateBacking};
pub use catalog::{Catalog, DatasetLineage, DatasetRegistration, LineageBreak, QualityState};
pub use delta::{
    CellInterval, CellStanding, DecodedCellDelta, DeltaOrder, DeltaRefusal, StrategyStanding,
    decode_cell_delta,
};
pub use ports::{
    Aggregation, AnalyticalStore, ColumnFilter, Edge, EvidenceReceipt, EvidenceStore, GraphStore,
    HotSeries, Lakehouse, MasterData, Row, TableVersion,
};
pub use provider::{MeshPort, MeshProvider, MeshTarget};
pub use spine::{
    CELL_DELTA_TOPIC, CapitalDispatch, CapitalDispatcher, CapitalGrantFrame, CellDeltaReceiver,
    CellDeltaSink, DispatcherConfig, DrainHalt, DrainReport, HeldReason, ReceiverStats,
    RecoveryReport, grant_key,
};
