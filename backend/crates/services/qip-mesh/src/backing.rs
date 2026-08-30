//! Where a store's state lives.
//!
//! Every port in this crate has an in-memory adapter and a file-backed one,
//! and they differ in exactly one respect: whether a write is also flushed to
//! disk. Rather than write each port twice, the point-in-time logic lives in
//! [`crate::state`] and the adapters are generic over this trait.
//!
//! That is not only tidier. Two hand-written implementations of the same
//! as-of filtering drift, and the drift shows up as a backtest that behaves
//! differently depending on which adapter a deployment happened to configure.

use qip_core::error::Result;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Holds one store's state and mediates access to it.
pub trait StateBacking<S>: Send + Sync + std::fmt::Debug {
    /// Read under the lock.
    fn read<T>(&self, f: impl FnOnce(&S) -> T) -> T;

    /// Mutate under the lock, durably where the backing is durable.
    fn write<T>(&self, f: impl FnOnce(&mut S) -> Result<T>) -> Result<T>;
}

/// State in process memory. Simulation, tests, and anything rebuildable.
#[derive(Debug)]
pub struct MemoryBacking<S> {
    state: Mutex<S>,
}

impl<S> MemoryBacking<S> {
    pub fn new(initial: S) -> Self {
        Self {
            state: Mutex::new(initial),
        }
    }
}

impl<S: Send + std::fmt::Debug> StateBacking<S> for MemoryBacking<S> {
    fn read<T>(&self, f: impl FnOnce(&S) -> T) -> T {
        // A poisoned lock means a previous caller panicked mid-write. The
        // state is still structurally valid — every mutation here is a whole
        // replacement — so recovering beats propagating a panic into a store
        // that the platform's shutdown path also needs.
        f(&self.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn write<T>(&self, f: impl FnOnce(&mut S) -> Result<T>) -> Result<T> {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// State in one JSON document, rewritten on change.
///
/// Adequate for local development and single-node deployments, and explicitly
/// not a database — the managed adapters in [`crate::provider`] exist for
/// that, and say what they need.
#[derive(Debug)]
pub struct FileBacking<S> {
    path: PathBuf,
    state: Mutex<S>,
}

impl<S: Serialize + DeserializeOwned> FileBacking<S> {
    /// Load from `path`, or start from `default` if it does not exist yet.
    pub fn open(path: impl AsRef<Path>, default: S) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let state = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            if text.trim().is_empty() {
                default
            } else {
                serde_json::from_str(&text)?
            }
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            default
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write to a temporary file and rename, so a crash mid-write leaves the
    /// previous contents intact rather than a truncated file.
    fn flush(&self, state: &S) -> Result<()> {
        let text = serde_json::to_string_pretty(state)?;
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

impl<S> StateBacking<S> for FileBacking<S>
where
    S: Clone + Serialize + DeserializeOwned + Send + std::fmt::Debug,
{
    fn read<T>(&self, f: impl FnOnce(&S) -> T) -> T {
        f(&self.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Apply to a copy, flush the copy, then commit it.
    ///
    /// The copy costs a clone per write and buys the property that matters: a
    /// write that could not be persisted leaves the in-memory state exactly as
    /// it was, so the store and the file never disagree about what happened.
    fn write<T>(&self, f: impl FnOnce(&mut S) -> Result<T>) -> Result<T> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut candidate = guard.clone();
        let outcome = f(&mut candidate)?;
        self.flush(&candidate)?;
        *guard = candidate;
        Ok(outcome)
    }
}
