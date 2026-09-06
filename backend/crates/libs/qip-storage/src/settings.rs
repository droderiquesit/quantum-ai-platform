//! Storage configuration, resolved from an environment the caller supplies,
//! and the refusals that keep a misconfigured deployment from looking healthy.
//!
//! Four binaries need the same three things at start-up: which
//! [`StorageTarget`] to use, where its root is, and a *loud* failure when
//! those two do not describe a store this build can actually write to. Four
//! copies of that would be four chances to get the refusals subtly different,
//! and the refusals are the whole value: [`StorageProvider`] already declines
//! a managed target rather than falling back, and this module is what makes
//! that decline happen at start-up rather than at the first write.
//!
//! # This module never reads the process environment
//!
//! Every constructor here takes the environment as a lookup the composition
//! root passes in — [`StorageSettings::from_env`] is `from_env(&|name|
//! std::env::var(name).ok())` in a binary and `from_env(&|name|
//! map.get(name).cloned())` in a test. A library that reached for
//! `std::env` itself could not be tested without mutating a process-global
//! that every other test in the binary shares, and could not be deployed
//! twice with different settings; more to the point, the two credentials a
//! managed target needs once lived in this crate as bare `std::env::var`
//! reads, which meant a deployment could not mount either one as a file the
//! way it mounts every other secret. They now go through
//! [`qip_core::secret::resolve_from`], with the `_FILE` indirection and the
//! both-set refusal that rule carries, and the acceptance suite refuses any
//! `std::env` outside a composition root.
//!
//! # The failure this module exists to prevent
//!
//! A deployment sets a root path, expects durability, and gets memory —
//! because the target variable was misspelled, or unset, or the path was
//! unwritable in a way nothing noticed until the first audit record. Every
//! smoke test passes: the process starts, serves, answers, and reports
//! healthy. The loss is discovered at the restart, which is the one moment
//! nobody is reading logs.
//!
//! So three things are refused rather than defaulted:
//!
//! * An **unrecognised target name**. [`StorageTarget::parse`] names the valid
//!   set instead of falling through to the default.
//! * A **durable target with no root**. There is no sensible default directory
//!   for state a deployment cannot afford to lose; a guess would put it in
//!   whatever directory the process happened to start in.
//! * A **root set alongside the memory target**. This one is not an error in
//!   any mechanical sense — memory ignores the root — and it is refused
//!   anyway, because an operator who supplied a path believes the process
//!   persists. That belief is exactly the one that costs the data.
//!
//! # Why start-up, and not first write
//!
//! [`StorageSettings::preflight`] builds a store and round-trips a value
//! through it. A root that does not exist, is read-only, is a regular file, or
//! sits on a filesystem the container cannot write is indistinguishable from a
//! working one until something writes. Finding out during the first cycle
//! means the process is already serving and already believed.

use crate::kv::KeyValueStore;
use crate::provider::{StorageProvider, StorageTarget};
use qip_core::error::{Error, Result};
use std::path::{Path, PathBuf};

pub use crate::managed::{Environment, ManagedSettings};

/// The namespace [`StorageSettings::preflight`] writes its probe into.
///
/// Named rather than anonymous so an operator who finds it on disk can tell
/// what wrote it. It is deleted on the way out of a successful preflight; a
/// leftover means the process died between the write and the delete, which is
/// itself worth being able to see.
pub const PREFLIGHT_NAMESPACE: &str = "preflight";

/// The environment variable naming the storage target.
pub const TARGET_VARIABLE: &str = "QIP_STORAGE_TARGET";

/// The environment variable naming the storage root.
pub const ROOT_VARIABLE: &str = "QIP_STORAGE_ROOT";

/// Which store a process uses, where, and what a managed target was given.
///
/// Constructed by a composition root through [`StorageSettings::from_env`],
/// which takes the environment as a lookup rather than reading it, or from
/// the target and root alone by [`StorageSettings::from_values`]. Both are
/// testable without touching the process environment, which every test in a
/// binary shares and which the 2024 edition makes `unsafe` to mutate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageSettings {
    target: StorageTarget,
    root: PathBuf,
    managed: ManagedSettings,
}

impl StorageSettings {
    /// Resolve [`TARGET_VARIABLE`], [`ROOT_VARIABLE`] and — for a managed
    /// target — what [`ManagedSettings::from_env`] reads, from `env`.
    ///
    /// `env` is the composition root's view of its environment: in a binary
    /// `&|name| std::env::var(name).ok()`, in a test a map. Nothing here reads
    /// the process environment, so a deployment's storage location and
    /// credentials are properties of the deployment rather than of the build,
    /// and so a credential may arrive as a mounted file through the `_FILE`
    /// rule every other secret in this platform is read by.
    pub fn from_env(env: &Environment<'_>) -> Result<Self> {
        let settings = Self::from_values(
            env(TARGET_VARIABLE).as_deref(),
            env(ROOT_VARIABLE).as_deref(),
        )?;
        let managed = ManagedSettings::from_env(settings.target, env)?;
        Ok(settings.with_managed(managed))
    }

    /// The target and root, from explicit values, with nothing resolved for
    /// a managed target — the provider built from this refuses one, naming
    /// what it needs, exactly as it does for a deployment that set nothing.
    ///
    /// An empty or whitespace-only variable is treated as unset. A deployment
    /// template that expands a missing value to `""` is common enough that
    /// treating it as "the operator asked for the empty path" would turn a
    /// templating mistake into a directory named nothing.
    pub fn from_values(target: Option<&str>, root: Option<&str>) -> Result<Self> {
        let target = match target.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => StorageTarget::parse(value)?,
            // Memory is the default because it is the only one that cannot be
            // wrong about itself. A default of `file` would make an unset
            // variable produce a store whose location nobody chose.
            None => StorageTarget::Memory,
        };
        let root = root.map(str::trim).filter(|value| !value.is_empty());

        match target {
            // The silent-memory trap, and the only place a root is refused
            // outright. It is not an error in any mechanical sense — memory
            // ignores the root — but an operator who supplied a path believes
            // this process persists, and that belief is what costs the data.
            StorageTarget::Memory => match root {
                None => Ok(Self {
                    target,
                    root: PathBuf::new(),
                    managed: ManagedSettings::none(),
                }),
                Some(root) => Err(Error::invalid(format!(
                    "{ROOT_VARIABLE} is set to {root:?} but {TARGET_VARIABLE} is memory, which \
                     writes nothing to it. An operator who configured a path expects this \
                     process to persist; starting in memory instead would pass every smoke test \
                     and lose everything at the restart. Set {TARGET_VARIABLE}=engine for a \
                     durable store, or unset {ROOT_VARIABLE} to run in memory deliberately"
                ))),
            },
            StorageTarget::File | StorageTarget::Engine => match root {
                Some(root) => Ok(Self {
                    target,
                    root: PathBuf::from(root),
                    managed: ManagedSettings::none(),
                }),
                None => Err(Error::invalid(format!(
                    "{TARGET_VARIABLE} is {} but {ROOT_VARIABLE} is unset; there is no default \
                     directory for state a deployment cannot afford to lose, and guessing one \
                     would write it wherever this process happened to start",
                    target.as_str()
                ))),
            },
            // A managed target is addressed by project and instance rather
            // than by path, so a root here is neither required nor harmful.
            // Refusing it would report a missing or surplus *path* when the
            // real problem is a missing *credential*, and send the operator to
            // change the wrong variable. Preflight states the real one.
            _ => Ok(Self {
                target,
                root: root.map(PathBuf::from).unwrap_or_default(),
                managed: ManagedSettings::none(),
            }),
        }
    }

    /// Settings that persist nothing, for a simulation or an embedder that has
    /// decided in its own code rather than by configuration.
    pub fn in_memory() -> Self {
        Self {
            target: StorageTarget::Memory,
            root: PathBuf::new(),
            managed: ManagedSettings::none(),
        }
    }

    /// Carry what a managed target was given. See [`ManagedSettings`].
    pub fn with_managed(mut self, managed: ManagedSettings) -> Self {
        self.managed = managed;
        self
    }

    /// What was resolved for a managed target; empty for every other target.
    pub fn managed(&self) -> &ManagedSettings {
        &self.managed
    }

    pub fn target(&self) -> StorageTarget {
        self.target
    }

    /// The configured root. Empty for targets that do not use one.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether a write acknowledged by this configuration survives a restart.
    pub fn is_durable(&self) -> bool {
        self.target.is_crash_safe()
    }

    /// The provider these settings resolve to.
    pub fn provider(&self) -> StorageProvider {
        StorageProvider::new(self.target, self.root.clone()).with_managed(self.managed.clone())
    }

    /// Build a store, failing here rather than at the first write.
    pub fn key_value(&self, namespace: &str) -> Result<std::sync::Arc<dyn KeyValueStore>> {
        self.provider().key_value(namespace)
    }

    /// Prove the configuration describes a store this process can write to.
    ///
    /// Constructing a store is not enough on its own: the file adapter creates
    /// its document lazily and the engine opens a directory it may not be able
    /// to extend, so a read-only mount or a root that is a regular file both
    /// construct cleanly and fail on the first real write — by which time the
    /// process is serving and is believed. The round trip is what converts
    /// that into a start-up failure.
    ///
    /// A managed target fails here with the provider's own message naming the
    /// credential it needs, which is the behaviour that must never be softened
    /// into a fallback.
    pub fn preflight(&self) -> Result<()> {
        let store = self.provider().key_value(PREFLIGHT_NAMESPACE)?;
        let key = "storage-preflight";
        let probe = serde_json::json!({
            "target": self.target.as_str(),
            "root": self.root.display().to_string(),
        });
        store.put(key, probe.clone())?;
        let read_back = store.get(key)?;
        if read_back.as_ref() != Some(&probe) {
            return Err(Error::io(format!(
                "the storage preflight wrote a value to {} at {} and read back {read_back:?}; \
                 a store that does not return what it was given cannot hold an audit trail",
                self.target.as_str(),
                self.root.display()
            )));
        }
        store.delete(key)?;
        Ok(())
    }

    /// The start-up banner's storage lines.
    ///
    /// An operator reading these should be able to answer three questions
    /// without opening the code: does this process persist, where, and what
    /// does a restart take away. The last one is why `lost_on_restart` is a
    /// parameter — what a binary deliberately keeps in memory differs per
    /// binary, and a line that omitted it would let the reader assume the
    /// absence of a warning meant the absence of loss.
    pub fn banner_lines(&self, persists: &[&str], lost_on_restart: &[&str]) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(match writes_to_a_local_root(self.target) {
            true => format!(
                "  storage:          {} at {}",
                self.target.as_str(),
                self.root.display()
            ),
            false => format!("  storage:          {}", self.target.as_str()),
        });
        lines.push(format!(
            "  durability:       {}",
            if self.is_durable() {
                "an acknowledged write survives a restart of this process"
            } else {
                "NOTHING SURVIVES A RESTART of this process"
            }
        ));
        lines.push(format!("  rationale:        {}", self.target.rationale()));
        lines.push(format!(
            "  persists:         {}",
            if persists.is_empty() || !self.is_durable() {
                "nothing".to_string()
            } else {
                persists.join(", ")
            }
        ));
        lines.push(format!(
            "  lost on restart:  {}",
            if self.is_durable() {
                if lost_on_restart.is_empty() {
                    "nothing this process holds".to_string()
                } else {
                    lost_on_restart.join(", ")
                }
            } else {
                "everything this process holds".to_string()
            }
        ));
        lines
    }
}

/// Whether this target reads and writes a directory on the local filesystem.
///
/// Memory has no root and the managed targets are addressed by project and
/// instance rather than by path, so requiring a root for either would demand a
/// value that means nothing — and, worse, would report a missing path when the
/// real problem is a missing credential.
fn writes_to_a_local_root(target: StorageTarget) -> bool {
    matches!(target, StorageTarget::File | StorageTarget::Engine)
}
