//! Storage provider selection.
//!
//! Which backing service a workload uses is a deployment decision, and the
//! choice is deliberate rather than "one database for everything" (charter
//! section 7). The mapping below records *why* each target exists; the managed
//! adapters are ports awaiting credentials, and say so.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A backing store the platform can be configured to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageTarget {
    /// Process memory. Simulation, tests, and the Fast Brain's hot state.
    Memory,
    /// Local files, one JSON document per namespace. Development, and small
    /// hand-inspectable state.
    File,
    /// The in-tree embedded engine: write-ahead log, checkpoints, atomic
    /// transactions, crash recovery. The durable choice for a single node.
    Engine,
    /// Analytical warehouse: research queries, attribution, backtest results.
    BigQuery,
    /// Object storage: event log archives, model artifacts, reports.
    CloudStorage,
    /// Relational: entities, portfolios, orders — anything needing transactions.
    AlloyDb,
    /// Globally consistent relational, where cross-region transactions justify it.
    Spanner,
    /// Wide-column: tick and order-book history at high write throughput.
    Bigtable,
    /// In-memory cache: hot quotes, feature values, rate limits.
    Memorystore,
}

impl StorageTarget {
    /// Whether an adapter for this target exists in this build.
    pub fn is_implemented(&self) -> bool {
        matches!(self, Self::Memory | Self::File | Self::Engine)
    }

    /// Whether an acknowledged write to this target survives loss of power.
    ///
    /// Only [`StorageTarget::Engine`] both flushes every write and recovers a
    /// crash mid-write. [`StorageTarget::File`] flushes too, but rewrites the
    /// whole document each time, so it is durable without being an engine.
    pub fn is_crash_safe(&self) -> bool {
        matches!(self, Self::File | Self::Engine)
    }

    /// What the target is for, and why it was chosen over the alternatives.
    pub fn rationale(&self) -> &'static str {
        match self {
            Self::Memory => "hot state and deterministic simulation; nothing survives a restart",
            Self::File => "one readable JSON document per namespace; small, rarely written state",
            Self::Engine => {
                "the in-tree engine: durable, crash-recoverable, transactional, single node"
            }
            Self::BigQuery => "columnar scans over research history; not for transactional writes",
            Self::CloudStorage => "immutable large objects: log archives, model artifacts, reports",
            Self::AlloyDb => {
                "transactional records with foreign keys: entities, orders, portfolios"
            }
            Self::Spanner => {
                "only where a transaction must span regions; otherwise AlloyDB is cheaper"
            }
            Self::Bigtable => "high-throughput time series keyed by instrument and time",
            Self::Memorystore => "sub-millisecond reads of values that can be recomputed if lost",
        }
    }

    /// The credential or configuration a deployment must supply.
    pub fn required_configuration(&self) -> Option<&'static str> {
        match self {
            Self::Memory | Self::File | Self::Engine => None,
            Self::BigQuery => {
                Some("GCP project, dataset, and a service account with BigQuery Data Editor")
            }
            Self::CloudStorage => {
                Some("GCP project, bucket, and a service account with Storage Object Admin")
            }
            Self::AlloyDb => Some("AlloyDB instance, database, and IAM database credentials"),
            Self::Spanner => Some(
                "Spanner instance, database, and a service account with Cloud Spanner Database User",
            ),
            Self::Bigtable => {
                Some("Bigtable instance, table, and a service account with Bigtable User")
            }
            Self::Memorystore => Some("Memorystore instance address and VPC connectivity"),
        }
    }
}

/// Resolves a storage target into a working adapter.
#[derive(Debug, Clone)]
pub struct StorageProvider {
    target: StorageTarget,
    root: std::path::PathBuf,
    clock: std::sync::Arc<dyn qip_core::Clock>,
}

impl StorageProvider {
    /// A provider reading the host wall clock.
    ///
    /// The provider is a composition-root helper, which is the one place a
    /// live [`qip_core::SystemClock`] belongs; a simulation or a replay calls
    /// [`StorageProvider::with_clock`] to inject its own.
    pub fn new(target: StorageTarget, root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            target,
            root: root.into(),
            clock: std::sync::Arc::new(qip_core::SystemClock),
        }
    }

    /// Build stores against an injected clock. Required for deterministic
    /// replay, where the timestamps the engine stamps on commits must be the
    /// simulated ones.
    pub fn with_clock(mut self, clock: std::sync::Arc<dyn qip_core::Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn target(&self) -> StorageTarget {
        self.target
    }

    /// Build a key-value store for `namespace`.
    ///
    /// A managed target returns [`Error::Unavailable`] naming the missing
    /// configuration. Failing at construction is deliberate: a deployment
    /// pointed at BigQuery must not quietly fall back to local files and
    /// appear to work.
    pub fn key_value(&self, namespace: &str) -> Result<std::sync::Arc<dyn crate::KeyValueStore>> {
        match self.target {
            StorageTarget::Memory => Ok(std::sync::Arc::new(crate::MemoryKeyValueStore::new())),
            StorageTarget::File => {
                let path = self.root.join(format!("{namespace}.json"));
                Ok(std::sync::Arc::new(crate::FileKeyValueStore::open(path)?))
            }
            StorageTarget::Engine => Ok(std::sync::Arc::new(crate::DurableStore::open(
                self.root.join(namespace),
                crate::EngineConfig::new(self.clock.clone()),
            )?)),
            other => Err(Error::unavailable(format!(
                "the {other:?} adapter is not built into this binary. It requires: {}. \
                 See docs/operations/external-dependencies.md",
                other
                    .required_configuration()
                    .unwrap_or("additional configuration")
            ))),
        }
    }

    /// Build a blob store, with the same failure semantics.
    pub fn blobs(&self, namespace: &str) -> Result<std::sync::Arc<dyn crate::BlobStore>> {
        match self.target {
            StorageTarget::Memory => Ok(std::sync::Arc::new(crate::MemoryBlobStore::new())),
            // Blobs are whole files either way: the engine's log buys nothing
            // for objects written once and never edited.
            StorageTarget::File | StorageTarget::Engine => Ok(std::sync::Arc::new(
                crate::FileBlobStore::open(self.root.join(namespace))?,
            )),
            other => Err(Error::unavailable(format!(
                "the {other:?} blob adapter is not built into this binary. It requires: {}",
                other
                    .required_configuration()
                    .unwrap_or("additional configuration")
            ))),
        }
    }
}
