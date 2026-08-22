//! The mesh's ports: one narrow trait per access pattern.
//!
//! Six traits rather than one store, because the access patterns have nothing
//! in common except that they hold data. A columnar scan over four years and a
//! sub-millisecond read of the last quote want opposite things from a storage
//! engine, and a single interface covering both would be an interface that
//! serves neither well and hides which one a call site actually needs.
//!
//! # Every read takes an as-of
//!
//! There is no method anywhere in this module that returns a latest value
//! ignoring an as-of. That absence is the control. A `latest(&self, key)`
//! would be the most convenient method here and the one through which every
//! look-ahead bug arrives: it answers "what is true now" to a caller
//! reconstructing what was true then, and the two agree in every test and
//! diverge in every backtest.
//!
//! Reads therefore take `as_of: Timestamp` and return
//! [`qip_contracts::Stamped`], so the answer carries the known-time it was
//! true as of. An empty answer is stamped at the as-of itself, meaning "this
//! is what was knowable then", which is a fact about the read rather than
//! about a value.
//!
//! Writes take the timestamp they happen at for the same reason: nothing in
//! the platform reads an ambient clock, so a replay produces the same store.

use qip_contracts::time::Stamped;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// A record. Opaque JSON, because the mesh is schema-on-read at this layer and
/// the schema belongs to the dataset's owner, not to the transport.
pub type Row = serde_json::Value;

/// One committed version of a lakehouse table.
///
/// `digest` chains: each version commits to its predecessor's digest and its
/// own batch, so a table whose history was edited afterwards is detectable
/// without keeping a second copy. The same idea as `qip_events::EventLog`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableVersion {
    pub table: String,
    /// Monotonic from 1. Version 0 does not exist, so "no version yet" and
    /// "the first version" cannot be confused.
    pub version: u64,
    pub committed_at: Timestamp,
    /// Rows in the table after this commit, not rows in the batch.
    pub rows: usize,
    pub digest: String,
}

/// A predicate over a row's columns.
///
/// Deliberately small and exact. Comparisons are made on [`Decimal`] parsed
/// from the stored value rather than on `f64`, so a filter on a price does not
/// select a different set of rows than the same filter written by hand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColumnFilter {
    /// Matches everything. The identity, so a caller with no predicate does
    /// not have to special-case the filter away.
    Any,
    Equals(String, Row),
    OneOf(String, Vec<Row>),
    /// Column present and at least this value.
    AtLeast(String, Decimal),
    /// Column present and at most this value.
    AtMost(String, Decimal),
    /// Column present and not null.
    Present(String),
    All(Vec<ColumnFilter>),
    Not(Box<ColumnFilter>),
}

impl ColumnFilter {
    /// Whether a row satisfies the predicate.
    ///
    /// A missing column never matches a positive predicate. The alternative —
    /// treating absent as zero — silently includes rows whose value nobody
    /// recorded, which is how a filtered scan quietly widens.
    pub fn matches(&self, row: &Row) -> bool {
        match self {
            Self::Any => true,
            Self::Equals(column, expected) => row.get(column) == Some(expected),
            Self::OneOf(column, expected) => row
                .get(column)
                .is_some_and(|actual| expected.iter().any(|e| e == actual)),
            Self::AtLeast(column, bound) => row
                .get(column)
                .and_then(decimal_of)
                .is_some_and(|value| value >= *bound),
            Self::AtMost(column, bound) => row
                .get(column)
                .and_then(decimal_of)
                .is_some_and(|value| value <= *bound),
            Self::Present(column) => row.get(column).is_some_and(|v| !v.is_null()),
            Self::All(filters) => filters.iter().all(|f| f.matches(row)),
            Self::Not(inner) => !inner.matches(row),
        }
    }
}

/// Read a JSON value as an exact decimal.
///
/// Strings first, because that is how [`Decimal`] serialises and therefore how
/// every price the platform wrote looks on the way back in.
pub fn decimal_of(value: &Row) -> Option<Decimal> {
    match value {
        serde_json::Value::String(s) => Decimal::parse(s),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Some(Decimal::from_int(i)),
            None => n.as_f64().and_then(Decimal::from_f64),
        },
        _ => None,
    }
}

/// A statistic over a column.
///
/// The result is `f64` and named a statistic on purpose: an average is not
/// money, and computing one in fixed point would imply an exactness it does
/// not have. Sums of money go through the lakehouse and stay [`Decimal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Sum,
    Mean,
    Min,
    Max,
}

impl Aggregation {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Mean => "mean",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

/// A relationship between two nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// What kind of relationship: `issues`, `hedges`, `settles_through`,
    /// `pays_off_from`. Kinds are strings because the graph's vocabulary grows
    /// faster than this crate does.
    pub kind: String,
    /// Exact where it is a quantity — a notional, a ratio, a payoff weight.
    pub weight: Option<Decimal>,
}

impl Edge {
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind: kind.into(),
            weight: None,
        }
    }

    pub fn weighing(mut self, weight: Decimal) -> Self {
        self.weight = Some(weight);
        self
    }
}

/// Proof that a piece of evidence was written, and what it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceReceipt {
    pub key: String,
    pub digest: String,
    pub written_at: Timestamp,
    pub size_bytes: usize,
}

/// Canonical versioned tables with time travel.
///
/// The book of record. Everything else in the mesh is derived from it and can
/// be rebuilt; this is the copy that cannot.
pub trait Lakehouse: Send + Sync + std::fmt::Debug {
    /// Commit a batch of stamped rows, returning the version it created.
    fn append(&self, table: &str, rows: Vec<Stamped<Row>>, at: Timestamp) -> Result<TableVersion>;

    /// The table as it was knowable at `as_of` — the time-travel read.
    fn snapshot(&self, table: &str, as_of: Timestamp) -> Result<Stamped<Vec<Row>>>;

    /// Versions committed at or before `as_of`, oldest first.
    fn versions(&self, table: &str, as_of: Timestamp) -> Result<Stamped<Vec<TableVersion>>>;

    /// Tables that existed at `as_of`.
    fn tables(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>>;
}

/// Columnar scans over history.
///
/// Research, attribution and reporting. Loads are bulk, reads are wide and
/// infrequent, and nothing on the trading path depends on it being up.
pub trait AnalyticalStore: Send + Sync + std::fmt::Debug {
    /// Load stamped rows into a dataset. Returns how many were accepted.
    fn load(&self, dataset: &str, rows: Vec<Stamped<Row>>) -> Result<usize>;

    /// Project and filter, as of a moment.
    ///
    /// An empty `projection` means every column: narrowing to nothing is
    /// almost always a mistake and returning nothing would hide it.
    fn scan(
        &self,
        dataset: &str,
        projection: &[String],
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Row>>>;

    fn count(&self, dataset: &str, filter: &ColumnFilter, as_of: Timestamp)
    -> Result<Stamped<u64>>;

    /// A statistic over one column of the matching rows. `None` when nothing
    /// matched — distinct from a zero, which is an answer.
    fn aggregate(
        &self,
        dataset: &str,
        column: &str,
        aggregation: Aggregation,
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<f64>>>;

    fn datasets(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>>;
}

/// Low-latency recent time series.
///
/// Keeps a bounded window and no more — that bound is what makes it hot. A
/// read as of a moment outside the retention window truthfully returns
/// nothing rather than a stale value, and the caller should go to the
/// [`Lakehouse`] for history. Silently answering from beyond the window would
/// make the hot store a second, slowly diverging book of record.
pub trait HotSeries: Send + Sync + std::fmt::Debug {
    fn record(&self, series: &str, point: Stamped<Decimal>) -> Result<()>;

    /// The most recent point knowable at `as_of`, within retention.
    fn latest_as_of(&self, series: &str, as_of: Timestamp) -> Result<Stamped<Option<Decimal>>>;

    /// Points valid from `from` and knowable by `as_of`, oldest first.
    fn window(
        &self,
        series: &str,
        from: Timestamp,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<(Timestamp, Decimal)>>>;

    fn series(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>>;

    /// How far back this store keeps points. Configuration, not data.
    fn retention(&self) -> Duration;
}

/// Instrument master, configuration, and anything else whose current value
/// matters and whose history matters more.
///
/// Bitemporal by construction: an upsert adds a version rather than replacing
/// one, so a decision made last March can be re-read against the instrument
/// definition that was in force last March rather than today's.
pub trait MasterData: Send + Sync + std::fmt::Debug {
    fn upsert(&self, entity: &str, key: &str, record: Stamped<Row>) -> Result<()>;

    /// The version in force at `as_of`.
    fn lookup(&self, entity: &str, key: &str, as_of: Timestamp) -> Result<Stamped<Option<Row>>>;

    /// Every key and its in-force version at `as_of`.
    fn list(&self, entity: &str, as_of: Timestamp) -> Result<Stamped<Vec<(String, Row)>>>;

    /// Every version of one key knowable at `as_of`, oldest first.
    fn history(&self, entity: &str, key: &str, as_of: Timestamp) -> Result<Stamped<Vec<Row>>>;

    fn entities(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>>;
}

/// Relationships: issuer to instrument, instrument to payoff, position to
/// hedge, counterparty to settlement path.
pub trait GraphStore: Send + Sync + std::fmt::Debug {
    fn add_edge(&self, edge: Stamped<Edge>) -> Result<()>;

    /// Edges out of a node that were knowable at `as_of`, optionally of one
    /// kind.
    fn neighbours(
        &self,
        node: &str,
        kind: Option<&str>,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Edge>>>;

    /// Nodes reachable within `max_depth` hops, excluding the start.
    fn reachable(
        &self,
        from: &str,
        kind: Option<&str>,
        max_depth: usize,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<String>>>;

    fn nodes(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>>;
}

/// The L0 immutable evidence layer: write once, read many.
///
/// There is no `delete` and no `overwrite` on this trait, and that absence is
/// the control rather than a gap. An evidence store an operator can correct is
/// an evidence store whose contents prove nothing, and the records
/// requirement it exists to satisfy is precisely that nobody — including the
/// people who run the platform — can revise the record after the fact.
///
/// Writing the same bytes to the same key again succeeds and changes nothing,
/// because a retry after an ambiguous failure must not be an error. Writing
/// *different* bytes to a key that already exists is refused, naming the key
/// and both digests.
pub trait EvidenceStore: Send + Sync + std::fmt::Debug {
    /// Write once. Idempotent for identical content, refused for a conflict.
    fn put(&self, key: &str, bytes: Vec<u8>, at: Timestamp) -> Result<EvidenceReceipt>;

    fn get(&self, key: &str, as_of: Timestamp) -> Result<Stamped<Option<Vec<u8>>>>;

    /// The receipt without the bytes — enough to prove what was written.
    fn receipt(&self, key: &str, as_of: Timestamp) -> Result<Stamped<Option<EvidenceReceipt>>>;

    fn keys(&self, prefix: &str, as_of: Timestamp) -> Result<Stamped<Vec<String>>>;
}
