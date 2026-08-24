//! The key-value persistence port.
//!
//! # Why the port is declared here and not beside its adapters
//!
//! Every adapter that satisfies this trait lives in `qip-storage` — memory,
//! one-JSON-document-per-namespace files, the embedded engine. The obvious
//! home for the trait is therefore `qip-storage` too, and that is where it
//! used to be.
//!
//! It moved because a *port* and an *adapter* have different dependency
//! directions, and putting them in one crate hides that. `qip-transport`'s
//! durable spool needs somewhere to persist unsent capital envelopes, so it
//! needs the port; it has no business knowing which adapter a deployment
//! chose. With the trait in `qip-storage`, needing the port meant depending on
//! the whole adapter crate, and the edge `qip-transport -> qip-storage` was
//! the result. That edge is what made the reverse edge impossible: a storage
//! adapter that speaks a REST API over `qip_transport::HttpClient` would have
//! closed a cycle cargo refuses to build.
//!
//! Declaring the port in the substrate both sides already depend on removes
//! the choice between them. `qip-transport` states what it needs from
//! persistence; `qip-storage` supplies it and is free to depend on the
//! transport in turn.
//!
//! # What this module does not provide
//!
//! No adapter, and no behaviour beyond the two default methods. This is the
//! contract only — the smallest set of operations every consumer in the
//! platform actually needs. It deliberately does not promise transactions,
//! compare-and-swap, TTLs, secondary indexes, or any ordering other than the
//! lexicographic key order [`KeyValueStore::keys_with_prefix`] names. A caller
//! needing atomic multi-key writes wants `qip_storage::engine::DurableStore`
//! and its `WriteBatch`, which is a concrete type precisely because that
//! guarantee is not one every adapter here can honour.
//!
//! It says nothing about durability either. Whether an acknowledged `put`
//! survives loss of power is the adapter's promise, not the port's, and the
//! adapters differ: the in-memory one loses everything on restart by design.
//! Code that needs the guarantee must name the adapter, not the trait.

use crate::error::Result;

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
