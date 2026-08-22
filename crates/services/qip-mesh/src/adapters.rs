//! The local adapters: one implementation of each port, over either backing.
//!
//! Each port has exactly one implementation here, generic over
//! [`StateBacking`], with type aliases naming the in-memory and file-backed
//! instantiations. The as-of filtering therefore exists once, in
//! [`crate::state`], and both adapters are the same store with different
//! durability.

use crate::backing::{FileBacking, MemoryBacking, StateBacking};
use crate::ports::{
    Aggregation, AnalyticalStore, ColumnFilter, Edge, EvidenceReceipt, EvidenceStore, GraphStore,
    HotSeries, Lakehouse, MasterData, Row, TableVersion,
};
use crate::state::{
    AnalyticalState, EvidenceState, GraphState, LakehouseState, MasterState, SeriesState,
};
use qip_contracts::time::Stamped;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use std::path::Path;

/// Canonical versioned tables with time travel.
#[derive(Debug)]
pub struct MeshLakehouse<B> {
    backing: B,
}

impl<B> MeshLakehouse<B> {
    pub fn with_backing(backing: B) -> Self {
        Self { backing }
    }
}

/// The in-memory lakehouse. Nothing survives a restart.
pub type MemoryLakehouse = MeshLakehouse<MemoryBacking<LakehouseState>>;
/// The file-backed lakehouse. One JSON document, rewritten on commit.
pub type FileLakehouse = MeshLakehouse<FileBacking<LakehouseState>>;

impl MemoryLakehouse {
    pub fn new() -> Self {
        Self::with_backing(MemoryBacking::new(LakehouseState::default()))
    }
}

impl Default for MemoryLakehouse {
    fn default() -> Self {
        Self::new()
    }
}

impl FileLakehouse {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::with_backing(FileBacking::open(
            path,
            LakehouseState::default(),
        )?))
    }
}

impl<B: StateBacking<LakehouseState>> Lakehouse for MeshLakehouse<B> {
    fn append(
        &self,
        table: &str,
        rows: Vec<Stamped<Row>>,
        at: Timestamp,
    ) -> Result<TableVersion> {
        self.backing.write(|state| state.append(table, rows, at))
    }

    fn snapshot(&self, table: &str, as_of: Timestamp) -> Result<Stamped<Vec<Row>>> {
        self.backing.read(|state| state.snapshot(table, as_of))
    }

    fn versions(&self, table: &str, as_of: Timestamp) -> Result<Stamped<Vec<TableVersion>>> {
        self.backing.read(|state| state.versions(table, as_of))
    }

    fn tables(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        self.backing.read(|state| state.tables(as_of))
    }
}

/// Columnar scans over history.
#[derive(Debug)]
pub struct MeshAnalytics<B> {
    backing: B,
}

impl<B> MeshAnalytics<B> {
    pub fn with_backing(backing: B) -> Self {
        Self { backing }
    }
}

/// The in-memory analytical store.
pub type MemoryAnalytics = MeshAnalytics<MemoryBacking<AnalyticalState>>;
/// The file-backed analytical store.
pub type FileAnalytics = MeshAnalytics<FileBacking<AnalyticalState>>;

impl MemoryAnalytics {
    pub fn new() -> Self {
        Self::with_backing(MemoryBacking::new(AnalyticalState::default()))
    }
}

impl Default for MemoryAnalytics {
    fn default() -> Self {
        Self::new()
    }
}

impl FileAnalytics {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::with_backing(FileBacking::open(
            path,
            AnalyticalState::default(),
        )?))
    }
}

impl<B: StateBacking<AnalyticalState>> AnalyticalStore for MeshAnalytics<B> {
    fn load(&self, dataset: &str, rows: Vec<Stamped<Row>>) -> Result<usize> {
        self.backing.write(|state| state.load(dataset, rows))
    }

    fn scan(
        &self,
        dataset: &str,
        projection: &[String],
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Row>>> {
        self.backing
            .read(|state| state.scan(dataset, projection, filter, as_of))
    }

    fn count(
        &self,
        dataset: &str,
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<u64>> {
        self.backing
            .read(|state| state.count(dataset, filter, as_of))
    }

    fn aggregate(
        &self,
        dataset: &str,
        column: &str,
        aggregation: Aggregation,
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<f64>>> {
        self.backing
            .read(|state| state.aggregate(dataset, column, aggregation, filter, as_of))
    }

    fn datasets(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        self.backing.read(|state| state.datasets(as_of))
    }
}

/// Low-latency recent time series.
#[derive(Debug)]
pub struct MeshHotSeries<B> {
    backing: B,
}

impl<B> MeshHotSeries<B> {
    pub fn with_backing(backing: B) -> Self {
        Self { backing }
    }
}

/// The in-memory hot series store.
pub type MemoryHotSeries = MeshHotSeries<MemoryBacking<SeriesState>>;
/// The file-backed hot series store.
pub type FileHotSeries = MeshHotSeries<FileBacking<SeriesState>>;

impl MemoryHotSeries {
    /// Keep points valid within `retention` of the newest point in a series.
    pub fn new(retention: Duration) -> Self {
        Self::with_backing(MemoryBacking::new(SeriesState::new(retention)))
    }
}

impl FileHotSeries {
    pub fn open(path: impl AsRef<Path>, retention: Duration) -> Result<Self> {
        Ok(Self::with_backing(FileBacking::open(
            path,
            SeriesState::new(retention),
        )?))
    }
}

impl<B: StateBacking<SeriesState>> HotSeries for MeshHotSeries<B> {
    fn record(&self, series: &str, point: Stamped<Decimal>) -> Result<()> {
        self.backing.write(|state| state.record(series, point))
    }

    fn latest_as_of(&self, series: &str, as_of: Timestamp) -> Result<Stamped<Option<Decimal>>> {
        self.backing.read(|state| state.latest_as_of(series, as_of))
    }

    fn window(
        &self,
        series: &str,
        from: Timestamp,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<(Timestamp, Decimal)>>> {
        self.backing
            .read(|state| state.window(series, from, as_of))
    }

    fn series(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        self.backing.read(|state| state.series(as_of))
    }

    fn retention(&self) -> Duration {
        self.backing.read(SeriesState::retention)
    }
}

/// Instrument master and configuration, versioned.
#[derive(Debug)]
pub struct MeshMasterData<B> {
    backing: B,
}

impl<B> MeshMasterData<B> {
    pub fn with_backing(backing: B) -> Self {
        Self { backing }
    }
}

/// The in-memory master data store.
pub type MemoryMasterData = MeshMasterData<MemoryBacking<MasterState>>;
/// The file-backed master data store.
pub type FileMasterData = MeshMasterData<FileBacking<MasterState>>;

impl MemoryMasterData {
    pub fn new() -> Self {
        Self::with_backing(MemoryBacking::new(MasterState::default()))
    }
}

impl Default for MemoryMasterData {
    fn default() -> Self {
        Self::new()
    }
}

impl FileMasterData {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::with_backing(FileBacking::open(
            path,
            MasterState::default(),
        )?))
    }
}

impl<B: StateBacking<MasterState>> MasterData for MeshMasterData<B> {
    fn upsert(&self, entity: &str, key: &str, record: Stamped<Row>) -> Result<()> {
        self.backing
            .write(|state| state.upsert(entity, key, record))
    }

    fn lookup(
        &self,
        entity: &str,
        key: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<Row>>> {
        self.backing.read(|state| state.lookup(entity, key, as_of))
    }

    fn list(&self, entity: &str, as_of: Timestamp) -> Result<Stamped<Vec<(String, Row)>>> {
        self.backing.read(|state| state.list(entity, as_of))
    }

    fn history(
        &self,
        entity: &str,
        key: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Row>>> {
        self.backing.read(|state| state.history(entity, key, as_of))
    }

    fn entities(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        self.backing.read(|state| state.entities(as_of))
    }
}

/// Relationships and the payoff graph.
#[derive(Debug)]
pub struct MeshGraph<B> {
    backing: B,
}

impl<B> MeshGraph<B> {
    pub fn with_backing(backing: B) -> Self {
        Self { backing }
    }
}

/// The in-memory graph store.
pub type MemoryGraph = MeshGraph<MemoryBacking<GraphState>>;
/// The file-backed graph store.
pub type FileGraph = MeshGraph<FileBacking<GraphState>>;

impl MemoryGraph {
    pub fn new() -> Self {
        Self::with_backing(MemoryBacking::new(GraphState::default()))
    }
}

impl Default for MemoryGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl FileGraph {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::with_backing(FileBacking::open(
            path,
            GraphState::default(),
        )?))
    }
}

impl<B: StateBacking<GraphState>> GraphStore for MeshGraph<B> {
    fn add_edge(&self, edge: Stamped<Edge>) -> Result<()> {
        self.backing.write(|state| state.add_edge(edge))
    }

    fn neighbours(
        &self,
        node: &str,
        kind: Option<&str>,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Edge>>> {
        self.backing
            .read(|state| state.neighbours(node, kind, as_of))
    }

    fn reachable(
        &self,
        from: &str,
        kind: Option<&str>,
        max_depth: usize,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<String>>> {
        self.backing
            .read(|state| state.reachable(from, kind, max_depth, as_of))
    }

    fn nodes(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        self.backing.read(|state| state.nodes(as_of))
    }
}

/// The L0 immutable evidence layer.
#[derive(Debug)]
pub struct MeshEvidence<B> {
    backing: B,
}

impl<B> MeshEvidence<B> {
    pub fn with_backing(backing: B) -> Self {
        Self { backing }
    }
}

/// The in-memory evidence store. Write-once for as long as the process lives.
pub type MemoryEvidence = MeshEvidence<MemoryBacking<EvidenceState>>;
/// The file-backed evidence store.
///
/// Write-once as far as this crate is concerned. Making that true of the file
/// itself is the deployment's job: object-lock or a retention policy on the
/// bucket, and no operator credential that can delete an object. See
/// [`crate::provider::MeshTarget::CloudStorageWorm`].
pub type FileEvidence = MeshEvidence<FileBacking<EvidenceState>>;

impl MemoryEvidence {
    pub fn new() -> Self {
        Self::with_backing(MemoryBacking::new(EvidenceState::default()))
    }
}

impl Default for MemoryEvidence {
    fn default() -> Self {
        Self::new()
    }
}

impl FileEvidence {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::with_backing(FileBacking::open(
            path,
            EvidenceState::default(),
        )?))
    }
}

impl<B: StateBacking<EvidenceState>> EvidenceStore for MeshEvidence<B> {
    fn put(&self, key: &str, bytes: Vec<u8>, at: Timestamp) -> Result<EvidenceReceipt> {
        self.backing.write(|state| state.put(key, bytes, at))
    }

    fn get(&self, key: &str, as_of: Timestamp) -> Result<Stamped<Option<Vec<u8>>>> {
        self.backing.read(|state| state.get(key, as_of))
    }

    fn receipt(
        &self,
        key: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<EvidenceReceipt>>> {
        self.backing.read(|state| state.receipt(key, as_of))
    }

    fn keys(&self, prefix: &str, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        self.backing.read(|state| state.keys(prefix, as_of))
    }
}
