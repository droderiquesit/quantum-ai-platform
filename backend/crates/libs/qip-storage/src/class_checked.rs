//! A blob store that checks every write against the class it was opened for
//! (R12-DATA-1041).
//!
//! `classes::StorageClass::holds` was a label nothing compared a write
//! against: a caller handed the knowledge-pack store could put a document
//! body under `knowledge-packs/` and nothing refused it, so "no durable
//! store holds document bodies" rested on every caller behaving. This wrapper
//! is the seam: a class whose contents have a checkable shape has its
//! bytes checked before they reach the inner store. The namespace is the
//! inner store's own (`StorageEngine::blobs(namespace)` prefixes keys), so
//! it is not re-checked here.
//!
//! Only `Holds::KnowledgePacks` has a content check, because it is the only
//! blob class here whose bytes have a schema that bounds text
//! (`KnowledgePack::decode` refuses an unknown shape and any string over its
//! compactness cap). Every other class passes through unchecked, and this module says
//! so rather than implying a check it does not make.

use crate::blob::BlobStore;
use crate::classes::{Holds, StorageClass};
use qip_contracts::knowledge_pack::KnowledgePack;
use qip_core::error::{Error, Result};
use std::sync::Arc;

#[derive(Debug)]
pub struct ClassCheckedStore {
    inner: Arc<dyn BlobStore>,
    class: StorageClass,
}

impl ClassCheckedStore {
    pub fn new(inner: Arc<dyn BlobStore>, class: StorageClass) -> Self {
        Self { inner, class }
    }

    fn check(&self, key: &str, bytes: &[u8]) -> Result<()> {
        if self.class.holds == Holds::KnowledgePacks {
            KnowledgePack::decode(bytes).map_err(|error| {
                Error::denied(format!(
                    "`{key}` is not a knowledge pack ({}); the `{}` class holds compact derived \
                     packs and never source text",
                    error.message(),
                    self.class.name
                ))
            })?;
        }
        Ok(())
    }
}

impl BlobStore for ClassCheckedStore {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.check(key, &bytes)?;
        self.inner.put(key, bytes)
    }

    fn put_if_absent(&self, key: &str, bytes: Vec<u8>) -> Result<bool> {
        self.check(key, &bytes)?;
        self.inner.put_if_absent(key, bytes)
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.get(key)
    }

    fn delete(&self, key: &str) -> Result<bool> {
        self.inner.delete(key)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix)
    }
}
