//! `qip-storage` — persistence ports and their local adapters.
//!
//! The platform talks to storage through narrow traits. Two adapter families
//! implement them here: an in-memory one for tests and simulation, and a
//! file-backed one that makes local development durable across restarts.
//!
//! The managed-service adapters (BigQuery, Spanner, Bigtable, Cloud Storage)
//! are declared in [`provider`] but not implemented — they need credentials and
//! a project this build has no access to. Each returns a
//! [`qip_core::Error::Unavailable`] naming exactly what is missing, so a
//! misconfigured deployment fails loudly at start-up rather than silently
//! writing nowhere. See `docs/operations/external-dependencies.md`.

pub mod blob;
pub mod kv;
pub mod provider;
pub mod repository;

pub use blob::{BlobStore, FileBlobStore, MemoryBlobStore};
pub use kv::{FileKeyValueStore, KeyValueStore, KeyValueStoreExt, MemoryKeyValueStore};
pub use provider::{StorageProvider, StorageTarget};
pub use repository::{MemoryRepository, Record, Repository};
