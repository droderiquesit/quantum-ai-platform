//! Which backing service serves which access pattern.
//!
//! The architecture names managed services for each port — a BigLake/Iceberg
//! lakehouse, BigQuery for analytics, Bigtable for hot series, Spanner for the
//! instrument master, Spanner Graph for relationships, Cloud Storage with
//! object lock for evidence. This build has no GCP project and no credentials,
//! so those adapters are declared here and return
//! [`qip_core::Error::Unavailable`] naming exactly what is missing.
//!
//! # No silent fallback
//!
//! A provider configured for a managed target never quietly returns a local
//! store. `qip-storage`'s documentation calls that hazard out and this crate
//! honours it: a deployment pointed at BigQuery that fell back to a JSON file
//! would pass its smoke tests, serve stale answers, and lose every write when
//! the pod restarted. Failing at construction means a misconfiguration is
//! found by whoever deployed it rather than by whoever is reconciling
//! positions a week later.

use crate::adapters::{
    FileAnalytics, FileEvidence, FileGraph, FileHotSeries, FileLakehouse, FileMasterData,
    MemoryAnalytics, MemoryEvidence, MemoryGraph, MemoryHotSeries, MemoryLakehouse,
    MemoryMasterData,
};
use crate::ports::{AnalyticalStore, EvidenceStore, GraphStore, HotSeries, Lakehouse, MasterData};
use qip_core::error::{Error, Result};
use qip_core::Duration;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// One access pattern in the mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshPort {
    Lakehouse,
    Analytical,
    HotSeries,
    MasterData,
    Graph,
    Evidence,
}

impl MeshPort {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Lakehouse => "lakehouse",
            Self::Analytical => "analytical",
            Self::HotSeries => "hot_series",
            Self::MasterData => "master_data",
            Self::Graph => "graph",
            Self::Evidence => "evidence",
        }
    }

    pub const fn all() -> [Self; 6] {
        [
            Self::Lakehouse,
            Self::Analytical,
            Self::HotSeries,
            Self::MasterData,
            Self::Graph,
            Self::Evidence,
        ]
    }

    /// The managed service the architecture names for this port.
    pub const fn managed_target(&self) -> MeshTarget {
        match self {
            Self::Lakehouse => MeshTarget::BigLakeIceberg,
            Self::Analytical => MeshTarget::BigQuery,
            Self::HotSeries => MeshTarget::Bigtable,
            Self::MasterData => MeshTarget::Spanner,
            Self::Graph => MeshTarget::SpannerGraph,
            Self::Evidence => MeshTarget::CloudStorageWorm,
        }
    }
}

/// A backing service the mesh can be configured to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshTarget {
    /// Process memory. Simulation, tests, and anything rebuildable.
    Memory,
    /// Local JSON documents. Development and single-node deployments.
    File,
    /// Iceberg tables over object storage, queried through BigLake.
    BigLakeIceberg,
    /// Columnar analytics over research history.
    BigQuery,
    /// Wide-column store for high-throughput recent time series.
    Bigtable,
    /// Globally consistent relational store for the instrument master.
    Spanner,
    /// Property-graph queries over the same Spanner database.
    SpannerGraph,
    /// Object storage with object lock: write-once, read-many evidence.
    CloudStorageWorm,
}

impl MeshTarget {
    /// Whether an adapter for this target exists in this build.
    pub const fn is_implemented(&self) -> bool {
        matches!(self, Self::Memory | Self::File)
    }

    /// What the target is for, and why it was chosen over the alternatives.
    pub const fn rationale(&self) -> &'static str {
        match self {
            Self::Memory => "deterministic simulation and tests; nothing survives a restart",
            Self::File => "single-node durability for local development",
            Self::BigLakeIceberg => {
                "table format with snapshot isolation and time travel, so the canonical history \
                 is versioned by the store rather than by convention"
            }
            Self::BigQuery => {
                "columnar scans over years of history; deliberately not on the trading path"
            }
            Self::Bigtable => {
                "sustained high-throughput writes keyed by instrument and time, with a bounded \
                 hot window"
            }
            Self::Spanner => {
                "the instrument master must be consistent across regions; a stale definition in \
                 one region prices the wrong contract"
            }
            Self::SpannerGraph => {
                "relationship traversal in the same transaction as the master data it describes, \
                 so the graph cannot disagree with the records"
            }
            Self::CloudStorageWorm => {
                "object lock makes the evidence layer immutable to the people who run the \
                 platform, which is the only version of that guarantee worth having"
            }
        }
    }

    /// The credential or configuration a deployment must supply.
    pub const fn required_configuration(&self) -> Option<&'static str> {
        match self {
            Self::Memory | Self::File => None,
            Self::BigLakeIceberg => Some(
                "GCP project, a BigLake metastore catalog, a Cloud Storage warehouse bucket, and \
                 a service account with BigLake Admin and Storage Object Admin",
            ),
            Self::BigQuery => Some(
                "GCP project, dataset, and a service account with BigQuery Data Editor and \
                 BigQuery Job User",
            ),
            Self::Bigtable => Some(
                "GCP project, Bigtable instance and table, and a service account with Bigtable \
                 User",
            ),
            Self::Spanner => Some(
                "GCP project, Spanner instance and database, and a service account with Cloud \
                 Spanner Database User",
            ),
            Self::SpannerGraph => Some(
                "the same Spanner instance and database with a property graph defined, and a \
                 service account with Cloud Spanner Database User",
            ),
            Self::CloudStorageWorm => Some(
                "GCP project, a bucket with a locked retention policy, and a service account \
                 with Storage Object Creator only — deliberately not Object Admin, since a \
                 credential that can delete evidence defeats the layer",
            ),
        }
    }

    /// The error a managed target returns at first use.
    fn unavailable(self, port: MeshPort) -> Error {
        Error::unavailable(format!(
            "the {self:?} adapter for the {} port is not built into this binary. It requires: {}. \
             This deployment will not fall back to local storage, because a mesh that silently \
             served local files would pass its smoke tests and lose every write. See \
             docs/operations/external-dependencies.md",
            port.as_str(),
            self.required_configuration()
                .unwrap_or("additional configuration")
        ))
    }
}

/// Resolves a target into a working adapter, or an error naming what is
/// missing.
#[derive(Debug, Clone)]
pub struct MeshProvider {
    target: MeshTarget,
    root: PathBuf,
    /// How much history the hot-series adapter keeps.
    hot_retention: Duration,
}

impl MeshProvider {
    pub fn new(target: MeshTarget, root: impl Into<PathBuf>, hot_retention: Duration) -> Self {
        Self {
            target,
            root: root.into(),
            hot_retention,
        }
    }

    /// A provider backed entirely by process memory.
    pub fn in_memory(hot_retention: Duration) -> Self {
        Self::new(MeshTarget::Memory, PathBuf::new(), hot_retention)
    }

    pub fn target(&self) -> MeshTarget {
        self.target
    }

    pub fn hot_retention(&self) -> Duration {
        self.hot_retention
    }

    pub fn lakehouse(&self) -> Result<Arc<dyn Lakehouse>> {
        match self.target {
            MeshTarget::Memory => Ok(Arc::new(MemoryLakehouse::new())),
            MeshTarget::File => Ok(Arc::new(FileLakehouse::open(
                self.root.join("lakehouse.json"),
            )?)),
            other => Err(other.unavailable(MeshPort::Lakehouse)),
        }
    }

    pub fn analytics(&self) -> Result<Arc<dyn AnalyticalStore>> {
        match self.target {
            MeshTarget::Memory => Ok(Arc::new(MemoryAnalytics::new())),
            MeshTarget::File => Ok(Arc::new(FileAnalytics::open(
                self.root.join("analytics.json"),
            )?)),
            other => Err(other.unavailable(MeshPort::Analytical)),
        }
    }

    pub fn hot_series(&self) -> Result<Arc<dyn HotSeries>> {
        match self.target {
            MeshTarget::Memory => Ok(Arc::new(MemoryHotSeries::new(self.hot_retention))),
            MeshTarget::File => Ok(Arc::new(FileHotSeries::open(
                self.root.join("hot-series.json"),
                self.hot_retention,
            )?)),
            other => Err(other.unavailable(MeshPort::HotSeries)),
        }
    }

    pub fn master_data(&self) -> Result<Arc<dyn MasterData>> {
        match self.target {
            MeshTarget::Memory => Ok(Arc::new(MemoryMasterData::new())),
            MeshTarget::File => Ok(Arc::new(FileMasterData::open(
                self.root.join("master.json"),
            )?)),
            other => Err(other.unavailable(MeshPort::MasterData)),
        }
    }

    pub fn graph(&self) -> Result<Arc<dyn GraphStore>> {
        match self.target {
            MeshTarget::Memory => Ok(Arc::new(MemoryGraph::new())),
            MeshTarget::File => Ok(Arc::new(FileGraph::open(self.root.join("graph.json"))?)),
            other => Err(other.unavailable(MeshPort::Graph)),
        }
    }

    pub fn evidence(&self) -> Result<Arc<dyn EvidenceStore>> {
        match self.target {
            MeshTarget::Memory => Ok(Arc::new(MemoryEvidence::new())),
            MeshTarget::File => Ok(Arc::new(FileEvidence::open(
                self.root.join("evidence.json"),
            )?)),
            other => Err(other.unavailable(MeshPort::Evidence)),
        }
    }

    /// Which port would be served by what, and what it still needs.
    ///
    /// For a start-up log line, so an operator can see the whole mapping
    /// rather than discovering it one failed call at a time.
    pub fn describe(&self) -> Vec<(MeshPort, MeshTarget, Option<&'static str>)> {
        MeshPort::all()
            .into_iter()
            .map(|port| {
                (
                    port,
                    self.target,
                    if self.target.is_implemented() {
                        None
                    } else {
                        self.target.required_configuration()
                    },
                )
            })
            .collect()
    }
}
