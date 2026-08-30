//! Key-value persistence: the adapters, and the port they satisfy.
//!
//! The port itself is [`qip_core::kv::KeyValueStore`]; this module re-exports
//! it and supplies the two adapters that need no configuration to work.

use qip_core::error::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The key-value port, re-exported from where it is declared.
///
/// The trait and its extension moved to [`qip_core::kv`] so that
/// `qip-transport` could depend on the *port* without depending on this crate,
/// which is what freed this crate to depend on `qip-transport` in turn — see
/// [`crate::gcp`]. They are re-exported here, under the path they have always
/// had, because `qip_storage::KeyValueStore` and `qip_storage::kv::KeyValueStore`
/// name the same type as before and every existing `Arc<dyn KeyValueStore>`
/// still coerces. Moving a port should not be visible to the code using it.
pub use qip_core::kv::{KeyValueStore, KeyValueStoreExt};

/// In-memory store. The default for simulation and tests.
#[derive(Debug, Default)]
pub struct MemoryKeyValueStore {
    entries: Mutex<BTreeMap<String, serde_json::Value>>,
}

impl MemoryKeyValueStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyValueStore for MemoryKeyValueStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), value);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<bool> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key)
            .is_some())
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn len(&self) -> Result<usize> {
        Ok(self.entries.lock().unwrap_or_else(|e| e.into_inner()).len())
    }
}

/// File-backed store: one JSON document, rewritten on change.
///
/// Adequate for a handful of keys read at start-up and rewritten rarely —
/// configuration snapshots, a bootstrap manifest. It is explicitly not a
/// database: every `put` re-serialises and rewrites the whole document, so the
/// cost of a write is the size of the dataset, and a large namespace makes
/// that quadratic. [`crate::engine::DurableStore`] is the engine to reach for
/// instead; this type is kept for the small cases where a single
/// human-readable JSON file is the point.
///
/// Writes are durable: the document is written to a scratch file, flushed with
/// `fsync`, renamed over the target, and the directory flushed in turn. When
/// `put` returns the new contents are on the device, and a crash at any point
/// leaves either the previous document or the new one — never a mixture.
#[derive(Debug)]
pub struct FileKeyValueStore {
    path: PathBuf,
    cache: Mutex<BTreeMap<String, serde_json::Value>>,
}

impl FileKeyValueStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let cache = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            if text.trim().is_empty() {
                BTreeMap::new()
            } else {
                serde_json::from_str(&text)?
            }
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            BTreeMap::new()
        };
        Ok(Self {
            path,
            cache: Mutex::new(cache),
        })
    }

    /// Write the cache out atomically and durably.
    ///
    /// The rename gives atomicity; the `fsync` inside [`crate::fsio::write_atomic`]
    /// gives durability. Without the second, a successful `put` would mean only
    /// that the kernel had seen the bytes, and a power loss would silently
    /// discard writes this store had already acknowledged.
    fn flush(&self, entries: &BTreeMap<String, serde_json::Value>) -> Result<()> {
        let text = serde_json::to_string_pretty(entries)?;
        crate::fsio::write_atomic(&self.path, text.as_bytes())
    }
}

impl KeyValueStore for FileKeyValueStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(key.to_string(), value);
        self.flush(&guard)
    }

    fn delete(&self, key: &str) -> Result<bool> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let removed = guard.remove(key).is_some();
        if removed {
            self.flush(&guard)?;
        }
        Ok(removed)
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn len(&self) -> Result<usize> {
        Ok(self.cache.lock().unwrap_or_else(|e| e.into_inner()).len())
    }
}

/// Build the key for a namespaced record: `namespace/id`.
pub fn key_for(namespace: &str, id: &str) -> String {
    format!("{namespace}/{id}")
}

/// Split a key back into namespace and id.
pub fn split_key(key: &str) -> Result<(&str, &str)> {
    key.split_once('/')
        .ok_or_else(|| Error::invalid(format!("malformed storage key: {key}")))
}
