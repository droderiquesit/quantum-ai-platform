//! The ledger's store port, and the local implementation of it (ADR 0113 §1).
//!
//! [`crate::store::LedgerStore`] decides what one delivery changes and hands
//! the whole of it to [`LedgerBackend::commit`] as one [`LedgerBatch`]. The
//! port is the ledger's own — three operations, no more — so the decision
//! logic of ADR 0105 and ADR 0106 is the same code whichever store holds the
//! rows, and is tested against both:
//!
//! * [`LocalBackend`] — the in-tree `DurableStore`: one write-ahead-log
//!   record per batch, fsynced. For tests and a single-node development run
//!   only; a deployed ledger refuses it at configuration
//!   ([`crate::config::LedgerConfig::parse`]).
//! * `crate::spanner::SpannerBackend` — the ledger of record: one Spanner
//!   read-write transaction per batch.
//!
//! The failure a port prevents here is two ledgers: a Spanner path that grew
//! its own copy of the posting logic would drift from the local one, and the
//! tests that prove the logic would prove only the copy nobody deploys.

use qip_core::error::Result;
use qip_storage::{DurableStore, EngineConfig, KeyValueStore, WriteBatch};
use serde::Serialize;
use std::path::Path;

/// One change in a [`LedgerBatch`].
#[derive(Clone, Debug, PartialEq)]
pub enum BatchOperation {
    Put {
        key: String,
        value: serde_json::Value,
    },
    Delete {
        key: String,
    },
}

/// Everything one delivery changes, committed together or not at all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedgerBatch {
    operations: Vec<BatchOperation>,
}

impl LedgerBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage a serialisable write. Later operations on one key win.
    pub fn put_as<T: Serialize>(mut self, key: impl Into<String>, value: &T) -> Result<Self> {
        self.operations.push(BatchOperation::Put {
            key: key.into(),
            value: serde_json::to_value(value)?,
        });
        Ok(self)
    }

    /// Stage a delete. Deleting an absent key is not an error.
    #[must_use]
    pub fn delete(mut self, key: impl Into<String>) -> Self {
        self.operations
            .push(BatchOperation::Delete { key: key.into() });
        self
    }

    pub fn operations(&self) -> &[BatchOperation] {
        &self.operations
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

/// Where the ledger's rows live. Implementations commit a batch atomically:
/// after a crash, a failover or a refused commit, either every operation of
/// it is visible or none is.
pub trait LedgerBackend: Send + Sync + std::fmt::Debug {
    /// One row, or `None`.
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>>;
    /// Every row under `prefix`, in key order.
    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, serde_json::Value)>>;
    /// Apply `batch` atomically and durably, or change nothing.
    fn commit(&self, batch: LedgerBatch) -> Result<()>;
    /// `local` or `spanner`, for `/health` and the start-up line.
    fn kind(&self) -> &'static str;
}

/// The ledger on the in-tree `DurableStore`.
#[derive(Debug)]
pub struct LocalBackend {
    store: DurableStore,
}

impl LocalBackend {
    /// Open the store at `directory`, recovering whatever the last process
    /// committed.
    pub fn open(directory: impl AsRef<Path>, config: EngineConfig) -> Result<Self> {
        Ok(Self {
            store: DurableStore::open(directory, config)?,
        })
    }
}

impl LedgerBackend for LocalBackend {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.store.get(key)
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, serde_json::Value)>> {
        self.store.scan_prefix(prefix)
    }

    fn commit(&self, batch: LedgerBatch) -> Result<()> {
        let mut write = WriteBatch::new();
        for operation in batch.operations {
            write = match operation {
                BatchOperation::Put { key, value } => write.put(key, value),
                BatchOperation::Delete { key } => write.delete(key),
            };
        }
        self.store.commit(write).map(|_| ())
    }

    fn kind(&self) -> &'static str {
        "local"
    }
}
