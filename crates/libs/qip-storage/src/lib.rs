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
//! * [`redis::RedisKeyValueStore`] — Memorystore, over an in-tree RESP client.
//!   The one managed target whose protocol is small enough to implement here:
//!   RESP is length-prefixed text over a TCP socket. It is a **cache**, and
//!   its module documentation spends as many words on what it does not promise
//!   as on what it does — nothing stored there survives a restart, a failover,
//!   an eviction or an expiry, and none of those is an error.
//!
//! * [`gcp::CloudStorageBlobStore`] — a Cloud Storage bucket, over the JSON
//!   REST API. Object storage is the natural second managed target: log
//!   archives and model artifacts are written once and read rarely, which is
//!   what a bucket is for. It never falls back to local disk.
//! * [`gcp::BigQueryWarehouse`] — the research warehouse: stream rows in, run
//!   a query, over the same transport. Not a [`KeyValueStore`] and not a
//!   [`BlobStore`]; a warehouse is a third shape and is reached directly.
//!
//! The two GCP adapters need what [`gcp`] sets out in full: a TLS-terminating
//! proxy, because [`qip_transport::http`] has no TLS stack and refuses `https`
//! by name, and a bearer token, because minting one means RS256-signing a JWT
//! and ADR 0009 forbids in-tree cryptography. Both are the deployment's to
//! supply, and both are refused loudly rather than worked around.
//!
//! Every file this crate writes goes through [`fsio`], which flushes to the
//! storage device before reporting success. An acknowledged write that a power
//! loss can take back would make the platform's audit trail a story rather
//! than evidence.
//!
//! The remaining managed targets are declared in [`provider`] and left as
//! ports. Bigtable, Spanner and AlloyDB are not blocked on a credential —
//! their obstacle is a protocol this workspace has decided not to implement
//! (gRPC and protobuf, a session-and-transaction data plane, and the
//! PostgreSQL wire protocol respectively), and
//! [`StorageTarget::required_configuration`] says which and how large rather
//! than "needs credentials". Every unbuildable target returns a
//! [`qip_core::Error::Unavailable`] naming exactly what is missing, so a
//! misconfigured deployment fails loudly at start-up rather than silently
//! writing nowhere. See `docs/operations/external-dependencies.md`.

pub mod blob;
pub mod chain;
pub mod engine;
mod fsio;
pub mod gcp;
pub mod kv;
pub mod provider;
pub mod redis;
pub mod repository;
pub mod settings;

pub use blob::{BlobStore, FileBlobStore, MemoryBlobStore};
pub use chain::{ArchivedRecord, ChainArchive};
pub use engine::{
    Durability, DurableStore, EngineConfig, EngineStats, IntegrityReport, RecoveryReport,
    WriteBatch,
};
pub use gcp::{
    AccessToken, BigQueryConfig, BigQueryWarehouse, CloudStorageBlobStore, CloudStorageConfig,
    GcpAccess, InsertOutcome, InsertRow, MetadataServerTokens, QueryPage, QueryParameter,
    QueryRequest, StaticToken, TokenFile, TokenSource,
};
pub use kv::{FileKeyValueStore, KeyValueStore, KeyValueStoreExt, MemoryKeyValueStore};
pub use provider::{StorageProvider, StorageTarget};
pub use redis::{RedisConfig, RedisKeyValueStore, RedisLimits, RespValue};
pub use repository::{MemoryRepository, Record, Repository};
pub use settings::StorageSettings;
