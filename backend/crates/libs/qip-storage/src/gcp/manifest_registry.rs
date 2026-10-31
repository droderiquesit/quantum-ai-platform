//! The Source Manifest Registry: every [`SourceManifest`] the platform holds,
//! queryable by the canonical URI of the original it points at
//! (R12-DATA-8014), on the store ADR 0113 admitted.
//!
//! A manifest used to exist only inside the record that carried it — a
//! `NewsItem`, an intelligence record — so "what do we hold about this
//! document" meant reading every event that might embed one. The registry
//! keeps one row per original, keyed by where the original lives rather than
//! by what its bytes hashed to: a lookup starts from an address a person or a
//! re-fetch has, and a content-hash key would answer only a question whose
//! answer the caller already held.
//!
//! # The canonical URI
//!
//! `source` and `locator` together, written so no two pairs share a key:
//! the source's length, the source, then the locator. A plain separator would
//! let source `a/b` with locator `c` and source `a` with locator `b/c` name one
//! row, and the second write would read as a revision of the first.
//!
//! # A revision is refused, not overwritten
//!
//! Registering the same original twice with the same hash is idempotent.
//! Registering it with a different hash is refused, naming both hashes: the
//! bytes at that address changed, and silently keeping the newer manifest
//! would erase the evidence that the platform once read something else there.
//! Revisions are detected and journaled elsewhere (`SourceRevisionDetected`);
//! the registry holds what was first read until a caller removes it on
//! purpose.

use super::spanner::{Mutation, SpannerKv};
use qip_core::error::{Error, Result};
use qip_financial::manifest::SourceManifest;

/// Every registry row's key starts with this, so the registry can share a
/// table with other users of [`SpannerKv`] without a scan reaching theirs.
pub const KEY_PREFIX: &str = "source-manifest/";

/// The canonical URI of the original `locator` names at `source`, as the
/// registry keys it. Refuses a blank source or locator: a manifest cannot be
/// built with either, so a lookup by one is a caller error.
pub fn canonical_uri(source: &str, locator: &str) -> Result<String> {
    if source.trim().is_empty() || locator.trim().is_empty() {
        return Err(Error::invalid(
            "a canonical URI needs both the source and the locator; a manifest names both",
        ));
    }
    Ok(format!("{KEY_PREFIX}{}:{source}/{locator}", source.len()))
}

/// The registry over one Spanner key-value table.
#[derive(Debug)]
pub struct SourceManifestRegistry {
    kv: SpannerKv,
}

impl SourceManifestRegistry {
    pub fn new(kv: SpannerKv) -> Self {
        Self { kv }
    }

    /// Register `manifest` under its canonical URI. Returns whether a row was
    /// written: `false` when the registry already holds the same manifest.
    /// Refuses a different manifest for the same original, in one read-write
    /// transaction, so two writers cannot both pass the check.
    pub fn register(&self, manifest: &SourceManifest) -> Result<bool> {
        let key = canonical_uri(manifest.source(), manifest.locator())?;
        let value = serde_json::to_string(manifest).map_err(|error| {
            Error::schema(format!("a source manifest did not serialise: {error}"))
        })?;
        let transaction = self.kv.begin()?;
        let outcome = match self.kv.get(Some(&transaction), &key)? {
            Some(held) if held == value => Ok(false),
            Some(held) => {
                let held: SourceManifest = decode(&key, &held)?;
                Err(Error::denied(format!(
                    "the registry holds {} for {} at sha256 {} and this manifest says {}; the \
                     original changed, so record the revision rather than overwrite what was read",
                    manifest.locator(),
                    manifest.source(),
                    held.sha256(),
                    manifest.sha256()
                )))
            }
            None => {
                self.kv
                    .commit(Some(&transaction), &[Mutation::Put { key, value }])?;
                return Ok(true);
            }
        };
        self.kv.rollback(&transaction)?;
        outcome
    }

    /// The manifest the registry holds for the original `locator` names at
    /// `source`, read strongly; `None` when it holds none.
    pub fn lookup(&self, source: &str, locator: &str) -> Result<Option<SourceManifest>> {
        let key = canonical_uri(source, locator)?;
        self.kv
            .get(None, &key)?
            .map(|held| decode(&key, &held))
            .transpose()
    }

    /// Every manifest the registry holds from `source`, in key order.
    pub fn from_source(&self, source: &str) -> Result<Vec<SourceManifest>> {
        if source.trim().is_empty() {
            return Err(Error::invalid("name the source whose manifests to list"));
        }
        let prefix = format!("{KEY_PREFIX}{}:{source}/", source.len());
        self.kv
            .scan_prefix(&prefix)?
            .iter()
            .map(|(key, held)| decode(key, held))
            .collect()
    }
}

fn decode(key: &str, held: &str) -> Result<SourceManifest> {
    serde_json::from_str(held).map_err(|error| {
        Error::schema(format!(
            "the registry row {key} is not a source manifest: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_separator_inside_the_source_cannot_make_two_originals_share_a_key() {
        let one = canonical_uri("a/b", "c").expect("a key");
        let two = canonical_uri("a", "b/c").expect("a key");
        assert_ne!(one, two);
        assert!(canonical_uri(" ", "c").is_err());
        assert!(canonical_uri("a", "").is_err());
    }
}
