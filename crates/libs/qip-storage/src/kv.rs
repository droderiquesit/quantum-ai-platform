//! Key-value persistence.

use qip_core::error::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A namespaced key-value store.
///
/// Keys are opaque strings; values are JSON. Scanning by prefix is part of the
/// contract because every caller needs it — listing a portfolio's positions,
/// a day's fills, an agent's memory.
pub trait KeyValueStore: Send + Sync + std::fmt::Debug {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>>;
    fn put(&self, key: &str, value: serde_json::Value) -> Result<()>;
    fn delete(&self, key: &str) -> Result<bool>;
    /// Keys beginning with `prefix`, in lexicographic order.
    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>>;
    fn len(&self) -> Result<usize>;

    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

/// Typed helpers over [`KeyValueStore`].
///
/// These live on an extension trait rather than the port itself: generic
/// methods would make `KeyValueStore` not object-safe, and every consumer holds
/// it as `Arc<dyn KeyValueStore>` so the backing adapter can be swapped by
/// configuration.
pub trait KeyValueStoreExt: KeyValueStore {
    /// Get and deserialize in one step.
    fn get_as<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.get(key)? {
            None => Ok(None),
            Some(value) => Ok(Some(serde_json::from_value(value)?)),
        }
    }

    /// Serialize and put in one step.
    fn put_as<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.put(key, serde_json::to_value(value)?)
    }

    /// Every value under a prefix, in key order.
    fn scan_as<T: serde::de::DeserializeOwned>(&self, prefix: &str) -> Result<Vec<(String, T)>> {
        let mut out = Vec::new();
        for key in self.keys_with_prefix(prefix)? {
            if let Some(value) = self.get_as::<T>(&key)? {
                out.push((key, value));
            }
        }
        Ok(out)
    }
}

impl<S: KeyValueStore + ?Sized> KeyValueStoreExt for S {}

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
/// Adequate for local development and for the modest state the demo keeps. It
/// is explicitly not a database — the managed adapters exist for that.
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

    /// Write the cache out. Writes to a temporary file and renames, so a crash
    /// mid-write leaves the previous contents intact rather than a truncated file.
    fn flush(&self, entries: &BTreeMap<String, serde_json::Value>) -> Result<()> {
        let text = serde_json::to_string_pretty(entries)?;
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
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
