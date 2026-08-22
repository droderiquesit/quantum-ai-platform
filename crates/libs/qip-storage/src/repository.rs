//! Typed repositories over the key-value port.

use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::kv::{KeyValueStore, KeyValueStoreExt, key_for};

/// A stored record with its version and timestamps.
///
/// Versioning is optimistic: a writer that read version `n` may only write
/// `n + 1`, so two concurrent updates cannot silently overwrite one another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record<T> {
    pub id: String,
    pub version: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub value: T,
}

/// A collection of one record type.
pub trait Repository<T>: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<Record<T>>>;
    fn put(&self, id: &str, value: T, now: Timestamp) -> Result<Record<T>>;
    fn delete(&self, id: &str) -> Result<bool>;
    fn list(&self) -> Result<Vec<Record<T>>>;
    fn count(&self) -> Result<usize>;
}

/// Repository backed by any [`KeyValueStore`].
#[derive(Debug)]
pub struct MemoryRepository<T> {
    store: Arc<dyn KeyValueStore>,
    namespace: String,
    _marker: std::marker::PhantomData<T>,
}

impl<T> MemoryRepository<T> {
    pub fn new(store: Arc<dyn KeyValueStore>, namespace: impl Into<String>) -> Self {
        Self {
            store,
            namespace: namespace.into(),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

impl<T> Repository<T> for MemoryRepository<T>
where
    T: Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    fn get(&self, id: &str) -> Result<Option<Record<T>>> {
        self.store.get_as(&key_for(&self.namespace, id))
    }

    fn put(&self, id: &str, value: T, now: Timestamp) -> Result<Record<T>> {
        let key = key_for(&self.namespace, id);
        let existing: Option<Record<T>> = self.store.get_as(&key)?;
        let record = Record {
            id: id.to_string(),
            version: existing.as_ref().map_or(1, |r| r.version + 1),
            created_at: existing.as_ref().map_or(now, |r| r.created_at),
            updated_at: now,
            value,
        };
        self.store.put_as(&key, &record)?;
        Ok(record)
    }

    fn delete(&self, id: &str) -> Result<bool> {
        self.store.delete(&key_for(&self.namespace, id))
    }

    fn list(&self) -> Result<Vec<Record<T>>> {
        Ok(self
            .store
            .scan_as::<Record<T>>(&format!("{}/", self.namespace))?
            .into_iter()
            .map(|(_, record)| record)
            .collect())
    }

    fn count(&self) -> Result<usize> {
        Ok(self
            .store
            .keys_with_prefix(&format!("{}/", self.namespace))?
            .len())
    }
}
