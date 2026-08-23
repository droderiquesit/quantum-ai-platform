//! `qip-storage` — persistence ports and the adapters that satisfy them.
//!
//! The platform talks to storage through narrow traits. Three adapter families
//! implement them here:
//!
//! * [`MemoryKeyValueStore`] / [`MemoryBlobStore`] — process memory, for tests
//!   and deterministic simulation. Nothing survives a restart, by design.
//! * [`FileKeyValueStore`] / [`FileBlobStore`] — one file per document. Small,
//!   inspectable, and durable on every write, but a key-value write costs the
//!   size of the whole document.
//! * [`engine::DurableStore`] — an embedded, crash-safe storage engine:
//!   write-ahead log, checkpoints, atomic multi-key transactions, checksummed
//!   records and crash recovery. This is the adapter a single-node deployment
//!   should use for anything it cannot afford to lose. Its module
//!   documentation states the exact durability guarantee and, at equal length,
//!   what it does not provide.
//!
//! Every file this crate writes goes through [`fsio`], which flushes to the
//! storage device before reporting success. An acknowledged write that a power
//! loss can take back would make the platform's audit trail a story rather
//! than evidence.
//!
//! The managed-service adapters (BigQuery, Spanner, Bigtable, Cloud Storage)
//! are declared in [`provider`] but not implemented — they need credentials and
//! a project this build has no access to. Each returns a
//! [`qip_core::Error::Unavailable`] naming exactly what is missing, so a
//! misconfigured deployment fails loudly at start-up rather than silently
//! writing nowhere. See `docs/operations/external-dependencies.md`.

pub mod blob;
pub mod engine;
mod fsio;
pub mod kv;
pub mod provider;
pub mod repository;

pub use blob::{BlobStore, FileBlobStore, MemoryBlobStore};
pub use engine::{
    Durability, DurableStore, EngineConfig, EngineStats, IntegrityReport, RecoveryReport,
    WriteBatch,
};
pub use kv::{FileKeyValueStore, KeyValueStore, KeyValueStoreExt, MemoryKeyValueStore};
pub use provider::{StorageProvider, StorageTarget};
pub use repository::{MemoryRepository, Record, Repository};
