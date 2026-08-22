//! The point-in-time logic behind every adapter, written once.
//!
//! The state types are public only because the adapter type aliases in
//! [`crate::adapters`] name them. They carry no public methods: everything a
//! caller can do goes through a port trait. They do serialise, which is what a
//! file-backed store persists and what somebody auditing one reads.
//!
//! Both the in-memory and the file-backed implementations of every port
//! delegate here. They differ only in whether the state is flushed to disk
//! afterwards, so a bug in the as-of filtering cannot be fixed in one adapter
//! and left in the other — which is the failure mode a second implementation
//! of the same rules always eventually has.

use crate::ports::{
    Aggregation, ColumnFilter, Edge, EvidenceReceipt, Row, TableVersion, decimal_of,
};
use qip_contracts::time::Stamped;
use qip_core::error::{Error, Result};
use qip_core::hash::{from_hex, to_hex};
use qip_core::{Decimal, Duration, Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Stamp an answer with the times of the facts that produced it.
///
/// Known-time is the latest of the contributing rows', so the answer says how
/// current it is. An answer with no contributing rows is stamped at the as-of:
/// "nothing was knowable then" is a fact about the read, and pretending it has
/// an earlier known-time would understate how much the caller is missing.
///
/// The maximum known-time is never earlier than the maximum valid-time — the
/// row holding the latest valid-time knows at or after it — so this never
/// trips [`Stamped::new`]'s clamp.
pub(crate) fn stamp<T>(value: T, times: &[(Timestamp, Timestamp)], as_of: Timestamp) -> Stamped<T> {
    let valid = times.iter().map(|(v, _)| *v).max();
    let known = times.iter().map(|(_, k)| *k).max();
    match (valid, known) {
        (Some(valid), Some(known)) => Stamped::new(value, valid, known),
        _ => Stamped::immediate(value, as_of),
    }
}

fn times_of<T>(facts: &[&Stamped<T>]) -> Vec<(Timestamp, Timestamp)> {
    facts.iter().map(|f| (f.valid_at(), f.known_at())).collect()
}

// --- lakehouse --------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LakehouseState {
    tables: BTreeMap<String, TableState>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TableState {
    rows: Vec<Stamped<Row>>,
    versions: Vec<TableVersion>,
}

impl LakehouseState {
    pub(crate) fn append(
        &mut self,
        table: &str,
        rows: Vec<Stamped<Row>>,
        at: Timestamp,
    ) -> Result<TableVersion> {
        if table.trim().is_empty() {
            return Err(Error::invalid("a lakehouse table must be named"));
        }
        if rows.is_empty() {
            return Err(Error::invalid(format!(
                "an empty batch would commit a version of {table} that changed nothing; a \
                 version nobody can point at a change in is noise in the history"
            )));
        }
        // A batch cannot commit a fact the platform did not yet know. Allowing
        // it would make a time-travel read by version disagree with the same
        // read by row, and only one of them would be right.
        for row in &rows {
            if row.known_at() > at {
                return Err(Error::invalid(format!(
                    "a row known at {} cannot be committed to {table} at {at}",
                    row.known_at()
                )));
            }
        }
        let state = self.tables.entry(table.to_string()).or_default();
        if let Some(last) = state.versions.last()
            && at < last.committed_at
        {
            return Err(Error::invalid(format!(
                "version {} of {table} was committed at {} and this batch claims {at}; the \
                 history of a table cannot go backwards",
                last.version, last.committed_at
            )));
        }

        let previous = state
            .versions
            .last()
            .map_or_else(String::new, |v| v.digest.clone());
        let encoded = serde_json::to_vec(&rows)?;
        let digest = sha256_hex(format!("{previous}|{}", to_hex(&sha256_bytes(&encoded))).as_bytes());

        state.rows.extend(rows);
        let version = TableVersion {
            table: table.to_string(),
            version: state.versions.len() as u64 + 1,
            committed_at: at,
            rows: state.rows.len(),
            digest,
        };
        state.versions.push(version.clone());
        Ok(version)
    }

    pub(crate) fn snapshot(&self, table: &str, as_of: Timestamp) -> Result<Stamped<Vec<Row>>> {
        let state = self.existing(table, as_of)?;
        let visible: Vec<&Stamped<Row>> = state
            .rows
            .iter()
            .filter(|r| r.was_known_by(as_of))
            .collect();
        let times = times_of(&visible);
        let rows: Vec<Row> = visible.iter().map(|r| r.value().clone()).collect();
        Ok(stamp(rows, &times, as_of))
    }

    pub(crate) fn versions(
        &self,
        table: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<TableVersion>>> {
        let state = self.existing(table, as_of)?;
        let versions: Vec<TableVersion> = state
            .versions
            .iter()
            .filter(|v| v.committed_at <= as_of)
            .cloned()
            .collect();
        let times: Vec<(Timestamp, Timestamp)> = versions
            .iter()
            .map(|v| (v.committed_at, v.committed_at))
            .collect();
        Ok(stamp(versions, &times, as_of))
    }

    pub(crate) fn tables(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        let mut names = Vec::new();
        let mut times = Vec::new();
        for (name, state) in &self.tables {
            if let Some(first) = state.versions.iter().find(|v| v.committed_at <= as_of) {
                names.push(name.clone());
                times.push((first.committed_at, first.committed_at));
            }
        }
        Ok(stamp(names, &times, as_of))
    }

    /// A table that had no version committed by `as_of` did not exist then.
    ///
    /// Reported as absent rather than empty, and identically to a table that
    /// never existed at all: distinguishing the two would tell a caller
    /// reasoning about the past that something was created afterwards.
    fn existing(&self, table: &str, as_of: Timestamp) -> Result<&TableState> {
        self.tables
            .get(table)
            .filter(|s| s.versions.iter().any(|v| v.committed_at <= as_of))
            .ok_or_else(|| {
                Error::not_found(format!("no lakehouse table `{table}` existed as of {as_of}"))
            })
    }
}

fn sha256_bytes(data: &[u8]) -> Vec<u8> {
    from_hex(&sha256_hex(data)).unwrap_or_default()
}

// --- analytical -------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnalyticalState {
    datasets: BTreeMap<String, Vec<Stamped<Row>>>,
}

impl AnalyticalState {
    pub(crate) fn load(&mut self, dataset: &str, rows: Vec<Stamped<Row>>) -> Result<usize> {
        if dataset.trim().is_empty() {
            return Err(Error::invalid("an analytical dataset must be named"));
        }
        let count = rows.len();
        self.datasets
            .entry(dataset.to_string())
            .or_default()
            .extend(rows);
        Ok(count)
    }

    fn visible(&self, dataset: &str, as_of: Timestamp) -> Result<Vec<&Stamped<Row>>> {
        let rows = self
            .datasets
            .get(dataset)
            .map(|rows| {
                rows.iter()
                    .filter(|r| r.was_known_by(as_of))
                    .collect::<Vec<_>>()
            })
            .filter(|rows: &Vec<&Stamped<Row>>| !rows.is_empty())
            .ok_or_else(|| {
                Error::not_found(format!("no dataset `{dataset}` held anything as of {as_of}"))
            })?;
        Ok(rows)
    }

    pub(crate) fn scan(
        &self,
        dataset: &str,
        projection: &[String],
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Row>>> {
        let matching: Vec<&Stamped<Row>> = self
            .visible(dataset, as_of)?
            .into_iter()
            .filter(|r| filter.matches(r.value()))
            .collect();
        let times = times_of(&matching);
        let rows: Vec<Row> = matching
            .iter()
            .map(|r| project(r.value(), projection))
            .collect();
        Ok(stamp(rows, &times, as_of))
    }

    pub(crate) fn count(
        &self,
        dataset: &str,
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<u64>> {
        let matching: Vec<&Stamped<Row>> = self
            .visible(dataset, as_of)?
            .into_iter()
            .filter(|r| filter.matches(r.value()))
            .collect();
        let times = times_of(&matching);
        Ok(stamp(matching.len() as u64, &times, as_of))
    }

    pub(crate) fn aggregate(
        &self,
        dataset: &str,
        column: &str,
        aggregation: Aggregation,
        filter: &ColumnFilter,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<f64>>> {
        let matching: Vec<&Stamped<Row>> = self
            .visible(dataset, as_of)?
            .into_iter()
            .filter(|r| filter.matches(r.value()))
            .collect();
        let times = times_of(&matching);
        let values: Vec<f64> = matching
            .iter()
            .filter_map(|r| r.value().get(column))
            .filter_map(decimal_of)
            .map(Decimal::to_f64)
            .collect();
        let result = if values.is_empty() {
            None
        } else {
            Some(match aggregation {
                Aggregation::Sum => values.iter().sum(),
                Aggregation::Mean => values.iter().sum::<f64>() / values.len() as f64,
                Aggregation::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
                Aggregation::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            })
        };
        Ok(stamp(result, &times, as_of))
    }

    pub(crate) fn datasets(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        let mut names = Vec::new();
        let mut times = Vec::new();
        for (name, rows) in &self.datasets {
            if let Some(first) = rows.iter().find(|r| r.was_known_by(as_of)) {
                names.push(name.clone());
                times.push((first.valid_at(), first.known_at()));
            }
        }
        Ok(stamp(names, &times, as_of))
    }
}

/// Keep only the requested columns. An empty projection keeps everything,
/// because narrowing to nothing is a mistake and silently returning empty
/// objects would hide it.
fn project(row: &Row, projection: &[String]) -> Row {
    if projection.is_empty() {
        return row.clone();
    }
    let mut out = serde_json::Map::new();
    for column in projection {
        if let Some(value) = row.get(column) {
            out.insert(column.clone(), value.clone());
        }
    }
    Row::Object(out)
}

// --- hot series -------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeriesState {
    retention: Duration,
    series: BTreeMap<String, Vec<Stamped<Decimal>>>,
}

impl SeriesState {
    pub(crate) fn new(retention: Duration) -> Self {
        Self {
            retention,
            series: BTreeMap::new(),
        }
    }

    pub(crate) fn retention(&self) -> Duration {
        self.retention
    }

    /// Record a point and drop anything that has aged out.
    ///
    /// Eviction is relative to the newest point in the series rather than to a
    /// wall clock, because there is no wall clock here: a replay must evict
    /// exactly the same points as the original run.
    pub(crate) fn record(&mut self, series: &str, point: Stamped<Decimal>) -> Result<()> {
        if series.trim().is_empty() {
            return Err(Error::invalid("a series must be named"));
        }
        let points = self.series.entry(series.to_string()).or_default();
        points.push(point);
        points.sort_by_key(|p| (p.valid_at().as_nanos(), p.known_at().as_nanos()));
        if let Some(newest) = points.last().map(Stamped::valid_at) {
            let horizon = newest.saturating_sub(self.retention);
            points.retain(|p| p.valid_at() >= horizon);
        }
        Ok(())
    }

    pub(crate) fn latest_as_of(
        &self,
        series: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<Decimal>>> {
        let points = self.existing(series, as_of)?;
        let latest = points
            .iter()
            .filter(|p| p.was_known_by(as_of))
            .max_by_key(|p| (p.valid_at().as_nanos(), p.known_at().as_nanos()));
        Ok(match latest {
            Some(point) => Stamped::new(Some(*point.value()), point.valid_at(), point.known_at()),
            None => Stamped::immediate(None, as_of),
        })
    }

    pub(crate) fn window(
        &self,
        series: &str,
        from: Timestamp,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<(Timestamp, Decimal)>>> {
        let points = self.existing(series, as_of)?;
        let visible: Vec<&Stamped<Decimal>> = points
            .iter()
            .filter(|p| p.was_known_by(as_of) && p.valid_at() >= from)
            .collect();
        let times = times_of(&visible);
        let values: Vec<(Timestamp, Decimal)> = visible
            .iter()
            .map(|p| (p.valid_at(), *p.value()))
            .collect();
        Ok(stamp(values, &times, as_of))
    }

    pub(crate) fn series(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        let mut names = Vec::new();
        let mut times = Vec::new();
        for (name, points) in &self.series {
            if let Some(first) = points.iter().find(|p| p.was_known_by(as_of)) {
                names.push(name.clone());
                times.push((first.valid_at(), first.known_at()));
            }
        }
        Ok(stamp(names, &times, as_of))
    }

    /// A series with no point knowable by `as_of` did not exist then.
    ///
    /// Reported identically to one that never existed, so a caller reasoning
    /// about the past cannot learn from an error message that a series was
    /// created later.
    fn existing(&self, series: &str, as_of: Timestamp) -> Result<&Vec<Stamped<Decimal>>> {
        self.series
            .get(series)
            .filter(|points| points.iter().any(|p| p.was_known_by(as_of)))
            .ok_or_else(|| {
                Error::not_found(format!("no hot series `{series}` was recorded as of {as_of}"))
            })
    }
}

// --- master data ------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MasterState {
    entities: BTreeMap<String, BTreeMap<String, Vec<Stamped<Row>>>>,
}

impl MasterState {
    pub(crate) fn upsert(&mut self, entity: &str, key: &str, record: Stamped<Row>) -> Result<()> {
        if entity.trim().is_empty() || key.trim().is_empty() {
            return Err(Error::invalid("master data needs both an entity and a key"));
        }
        // A version is added, never replaced. Replacing would make a decision
        // taken last March impossible to re-read against the definition that
        // was in force last March, which is the whole reason this store is
        // bitemporal rather than a map.
        self.entities
            .entry(entity.to_string())
            .or_default()
            .entry(key.to_string())
            .or_default()
            .push(record);
        Ok(())
    }

    pub(crate) fn lookup(
        &self,
        entity: &str,
        key: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<Row>>> {
        let keys = self.existing(entity, as_of)?;
        let in_force = keys.get(key).and_then(|versions| {
            versions
                .iter()
                .filter(|v| v.was_known_by(as_of))
                .max_by_key(|v| (v.valid_at().as_nanos(), v.known_at().as_nanos()))
        });
        Ok(match in_force {
            Some(version) => Stamped::new(
                Some(version.value().clone()),
                version.valid_at(),
                version.known_at(),
            ),
            None => Stamped::immediate(None, as_of),
        })
    }

    pub(crate) fn list(
        &self,
        entity: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<(String, Row)>>> {
        let keys = self.existing(entity, as_of)?;
        let mut out = Vec::new();
        let mut times = Vec::new();
        for (key, versions) in keys {
            if let Some(version) = versions
                .iter()
                .filter(|v| v.was_known_by(as_of))
                .max_by_key(|v| (v.valid_at().as_nanos(), v.known_at().as_nanos()))
            {
                out.push((key.clone(), version.value().clone()));
                times.push((version.valid_at(), version.known_at()));
            }
        }
        Ok(stamp(out, &times, as_of))
    }

    pub(crate) fn history(
        &self,
        entity: &str,
        key: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Row>>> {
        let keys = self.existing(entity, as_of)?;
        let mut versions: Vec<&Stamped<Row>> = keys
            .get(key)
            .map(|v| v.iter().filter(|v| v.was_known_by(as_of)).collect())
            .unwrap_or_default();
        versions.sort_by_key(|v| (v.valid_at().as_nanos(), v.known_at().as_nanos()));
        let times = times_of(&versions);
        let rows: Vec<Row> = versions.iter().map(|v| v.value().clone()).collect();
        Ok(stamp(rows, &times, as_of))
    }

    pub(crate) fn entities(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        let mut names = Vec::new();
        let mut times = Vec::new();
        for (name, keys) in &self.entities {
            if let Some(first) = keys
                .values()
                .flatten()
                .filter(|v| v.was_known_by(as_of))
                .min_by_key(|v| v.known_at().as_nanos())
            {
                names.push(name.clone());
                times.push((first.valid_at(), first.known_at()));
            }
        }
        Ok(stamp(names, &times, as_of))
    }

    fn existing(
        &self,
        entity: &str,
        as_of: Timestamp,
    ) -> Result<&BTreeMap<String, Vec<Stamped<Row>>>> {
        self.entities
            .get(entity)
            .filter(|keys| {
                keys.values()
                    .flatten()
                    .any(|version| version.was_known_by(as_of))
            })
            .ok_or_else(|| {
                Error::not_found(format!("no master entity `{entity}` existed as of {as_of}"))
            })
    }
}

// --- graph ------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GraphState {
    edges: Vec<Stamped<Edge>>,
}

impl GraphState {
    pub(crate) fn add_edge(&mut self, edge: Stamped<Edge>) -> Result<()> {
        if edge.value().from.trim().is_empty() || edge.value().to.trim().is_empty() {
            return Err(Error::invalid("an edge must join two named nodes"));
        }
        if edge.value().kind.trim().is_empty() {
            return Err(Error::invalid(
                "an edge must say what kind of relationship it is; an untyped edge cannot be \
                 traversed selectively and every traversal becomes a whole-graph walk",
            ));
        }
        self.edges.push(edge);
        Ok(())
    }

    pub(crate) fn neighbours(
        &self,
        node: &str,
        kind: Option<&str>,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<Edge>>> {
        let matching: Vec<&Stamped<Edge>> = self
            .edges
            .iter()
            .filter(|e| e.was_known_by(as_of))
            .filter(|e| e.value().from == node)
            .filter(|e| kind.is_none_or(|k| e.value().kind == k))
            .collect();
        let times = times_of(&matching);
        let edges: Vec<Edge> = matching.iter().map(|e| e.value().clone()).collect();
        Ok(stamp(edges, &times, as_of))
    }

    pub(crate) fn reachable(
        &self,
        from: &str,
        kind: Option<&str>,
        max_depth: usize,
        as_of: Timestamp,
    ) -> Result<Stamped<Vec<String>>> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut times = Vec::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        seen.insert(from.to_string());
        queue.push_back((from.to_string(), 0));

        // Breadth-first with a visited set, so a cyclical graph — an issuer
        // that guarantees its own subsidiary, say — terminates instead of
        // walking forever.
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for edge in self
                .edges
                .iter()
                .filter(|e| e.was_known_by(as_of))
                .filter(|e| e.value().from == node)
                .filter(|e| kind.is_none_or(|k| e.value().kind == k))
            {
                times.push((edge.valid_at(), edge.known_at()));
                if seen.insert(edge.value().to.clone()) {
                    queue.push_back((edge.value().to.clone(), depth + 1));
                }
            }
        }
        seen.remove(from);
        Ok(stamp(seen.into_iter().collect(), &times, as_of))
    }

    pub(crate) fn nodes(&self, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        let visible: Vec<&Stamped<Edge>> = self
            .edges
            .iter()
            .filter(|e| e.was_known_by(as_of))
            .collect();
        let times = times_of(&visible);
        let mut nodes: BTreeSet<String> = BTreeSet::new();
        for edge in &visible {
            nodes.insert(edge.value().from.clone());
            nodes.insert(edge.value().to.clone());
        }
        Ok(stamp(nodes.into_iter().collect(), &times, as_of))
    }
}

// --- evidence ---------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvidenceState {
    entries: BTreeMap<String, EvidenceEntry>,
}

/// Bytes are held as hex rather than a JSON byte array so the persisted file
/// stays readable by a person auditing it, which is the point of this layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvidenceEntry {
    receipt: EvidenceReceipt,
    hex: String,
}

impl EvidenceState {
    pub(crate) fn put(
        &mut self,
        key: &str,
        bytes: Vec<u8>,
        at: Timestamp,
    ) -> Result<EvidenceReceipt> {
        if key.trim().is_empty() {
            return Err(Error::invalid("evidence must be written under a key"));
        }
        let digest = sha256_hex(&bytes);
        if let Some(existing) = self.entries.get(key) {
            // Identical content is the retry-after-an-ambiguous-failure case
            // and must succeed, or every writer needs a read-before-write.
            if existing.receipt.digest == digest {
                return Ok(existing.receipt.clone());
            }
            return Err(Error::denied(format!(
                "evidence `{key}` was written at {} with digest {} and cannot be replaced with \
                 digest {}; this layer is write-once, so correct the record by writing a new key \
                 that supersedes it",
                existing.receipt.written_at,
                &existing.receipt.digest[..16],
                &digest[..16]
            )));
        }
        let receipt = EvidenceReceipt {
            key: key.to_string(),
            digest: digest.clone(),
            written_at: at,
            size_bytes: bytes.len(),
        };
        self.entries.insert(
            key.to_string(),
            EvidenceEntry {
                receipt: receipt.clone(),
                hex: to_hex(&bytes),
            },
        );
        Ok(receipt)
    }

    pub(crate) fn get(&self, key: &str, as_of: Timestamp) -> Result<Stamped<Option<Vec<u8>>>> {
        let Some(entry) = self.visible(key, as_of) else {
            return Ok(Stamped::immediate(None, as_of));
        };
        let bytes = from_hex(&entry.hex).ok_or_else(|| {
            Error::io(format!("the stored bytes of evidence `{key}` are not readable"))
        })?;
        // Re-check on read. A content-addressed store that does not verify on
        // the way out is a store that reports whatever it happens to hold.
        if sha256_hex(&bytes) != entry.receipt.digest {
            return Err(Error::guard(format!(
                "evidence `{key}` no longer hashes to the digest it was written under"
            )));
        }
        Ok(Stamped::new(
            Some(bytes),
            entry.receipt.written_at,
            entry.receipt.written_at,
        ))
    }

    pub(crate) fn receipt(
        &self,
        key: &str,
        as_of: Timestamp,
    ) -> Result<Stamped<Option<EvidenceReceipt>>> {
        Ok(match self.visible(key, as_of) {
            Some(entry) => Stamped::new(
                Some(entry.receipt.clone()),
                entry.receipt.written_at,
                entry.receipt.written_at,
            ),
            None => Stamped::immediate(None, as_of),
        })
    }

    pub(crate) fn keys(&self, prefix: &str, as_of: Timestamp) -> Result<Stamped<Vec<String>>> {
        let mut keys = Vec::new();
        let mut times = Vec::new();
        for (key, entry) in &self.entries {
            if key.starts_with(prefix) && entry.receipt.written_at <= as_of {
                keys.push(key.clone());
                times.push((entry.receipt.written_at, entry.receipt.written_at));
            }
        }
        Ok(stamp(keys, &times, as_of))
    }

    fn visible(&self, key: &str, as_of: Timestamp) -> Option<&EvidenceEntry> {
        self.entries
            .get(key)
            .filter(|e| e.receipt.written_at <= as_of)
    }
}
