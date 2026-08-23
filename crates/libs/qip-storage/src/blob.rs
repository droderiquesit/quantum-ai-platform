//! Blob storage for artifacts: simulation outputs, model files, reports.

use qip_core::error::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Large-object storage, addressed by path-like keys.
pub trait BlobStore: Send + Sync + std::fmt::Debug {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn delete(&self, key: &str) -> Result<bool>;
    fn list(&self, prefix: &str) -> Result<Vec<String>>;

    /// Content hash of a stored blob, for integrity checks.
    fn digest(&self, key: &str) -> Result<Option<String>> {
        Ok(self.get(key)?.map(|b| qip_core::hash::sha256_hex(&b)))
    }
}

#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    blobs: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryBlobStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlobStore for MemoryBlobStore {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.blobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), bytes);
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .blobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }

    fn delete(&self, key: &str) -> Result<bool> {
        Ok(self
            .blobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key)
            .is_some())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .blobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// Blob store rooted at a filesystem directory.
///
/// A `put` returns only once the object's bytes have been flushed to the
/// storage device and the directory entry naming them has been flushed too.
/// Blobs are the platform's evidence — simulation outputs, model artifacts,
/// reports someone will later be asked to justify — so an acknowledged write
/// that a power loss can take back is not acceptable here either.
#[derive(Debug)]
pub struct FileBlobStore {
    root: PathBuf,
}

impl FileBlobStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Resolve a key to a path, refusing anything that escapes the root.
    ///
    /// Keys can reach this from an API request, so traversal has to be blocked
    /// rather than assumed away.
    fn path_for(&self, key: &str) -> Result<PathBuf> {
        if key.is_empty()
            || key.starts_with('/')
            || key
                .split('/')
                .any(|part| part == ".." || part == "." || part.is_empty())
        {
            return Err(Error::invalid(format!("unsafe blob key: {key}")));
        }
        Ok(self.root.join(key))
    }
}

impl BlobStore for FileBlobStore {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::fsio::write_atomic(&path, &bytes)
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path_for(key)?;
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(std::fs::read(path)?))
    }

    fn delete(&self, key: &str) -> Result<bool> {
        let path = self.path_for(key)?;
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path)?;
        // An unlink is a directory change like any other, and is no more
        // durable than a create until the directory itself is flushed.
        if let Some(parent) = path.parent() {
            crate::fsio::sync_directory(parent);
        }
        Ok(true)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        collect(&self.root, &self.root, &mut out)?;
        out.retain(|k| k.starts_with(prefix));
        out.sort();
        Ok(out)
    }
}

fn collect(root: &Path, directory: &Path, out: &mut Vec<String>) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            let key = relative.to_string_lossy().replace('\\', "/");
            // A crash between writing a scratch file and renaming it leaves
            // one behind. It is not a stored object and must not be listed as
            // one.
            if !crate::fsio::is_temporary(&key) {
                out.push(key);
            }
        }
    }
    Ok(())
}
