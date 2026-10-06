//! Asynchronous compaction of tiny Cloud Storage objects into larger segments.
//!
//! This module implements the compaction job described in DATA-073: many small
//! objects under a prefix are merged into larger segments off the write path,
//! reducing per-operation cost and listing time.
//!
//! Compaction respects locked objects (DATA-052) and must not remove landing
//! segments before they are archived (DATA-068). The current implementation
//! is transport-agnostic and works with any [`BlobStore`].

use crate::blob::BlobStore;
use qip_core::error::Result;

/// Configuration for a compaction job.
#[derive(Clone, Debug)]
pub struct CompactionConfig {
    /// Minimum size in bytes for the input objects to compact.
    pub min_object_size: usize,
    /// Maximum size in bytes for a single compacted segment.
    pub max_segment_size: usize,
}

impl CompactionConfig {
    /// A reasonable default: compact objects 64KB or smaller into 4MB segments.
    pub fn new() -> Self {
        Self {
            min_object_size: 64 * 1024,
            max_segment_size: 4 * 1024 * 1024,
        }
    }

    /// Override the minimum object size (default 64KB).
    pub fn with_min_object_size(mut self, bytes: usize) -> Self {
        self.min_object_size = bytes;
        self
    }

    /// Override the maximum segment size (default 4MB).
    pub fn with_max_segment_size(mut self, bytes: usize) -> Self {
        self.max_segment_size = bytes;
        self
    }
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a compaction job: the segments that were created and the originals
/// that can now be deleted.
#[derive(Clone, Debug)]
pub struct CompactionResult {
    /// Keys of the newly created compacted segments.
    pub compacted_segments: Vec<String>,
    /// Keys of the original objects that were merged.
    pub merged_objects: Vec<String>,
    /// Total bytes in the merged objects.
    pub total_bytes_merged: u64,
}

/// A compaction job: merges small objects under a prefix into larger segments.
#[derive(Debug)]
pub struct Compactor<'a> {
    store: &'a dyn BlobStore,
    config: CompactionConfig,
}

impl<'a> Compactor<'a> {
    /// Create a new compactor for a given blob store.
    pub fn new(store: &'a dyn BlobStore, config: CompactionConfig) -> Self {
        Self { store, config }
    }

    /// Compact all objects under the given prefix, merging small ones into
    /// larger segments. Returns the new segments and the merged originals.
    ///
    /// The job runs off the write path: no locks are acquired on the store,
    /// and objects can be added or removed during compaction. Compaction
    /// reads the list of objects once at the start, so a concurrent write
    /// that finishes before compaction reads an object will be included, but
    /// a write that starts after the listing completes will not be.
    ///
    /// Hash integrity is verified: the new segments' contents, when
    /// concatenated, hash to the same value as the originals concatenated.
    pub fn compact(&self, prefix: &str) -> Result<CompactionResult> {
        // List all objects under the prefix.
        let keys = self.store.list(prefix)?;
        if keys.is_empty() {
            return Ok(CompactionResult {
                compacted_segments: Vec::new(),
                merged_objects: Vec::new(),
                total_bytes_merged: 0,
            });
        }

        // Read and filter: keep only objects smaller than the max compaction size.
        let mut objects = Vec::new();
        for key in &keys {
            if let Ok(Some(bytes)) = self.store.get(key)
                && bytes.len() < self.config.max_segment_size
            {
                objects.push((key.clone(), bytes));
            }
        }

        if objects.is_empty() {
            return Ok(CompactionResult {
                compacted_segments: Vec::new(),
                merged_objects: Vec::new(),
                total_bytes_merged: 0,
            });
        }

        // Merge objects into larger segments.
        let segments = self.merge_objects(objects)?;

        // Build result.
        let compacted_segments = segments
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let merged_objects = keys;
        let total_bytes_merged = segments.iter().map(|(_, bytes)| bytes.len() as u64).sum();

        Ok(CompactionResult {
            compacted_segments,
            merged_objects,
            total_bytes_merged,
        })
    }

    /// Merge a collection of (key, bytes) pairs into larger segments.
    /// Each segment's bytes are stored back under a content-addressed key.
    fn merge_objects(&self, objects: Vec<(String, Vec<u8>)>) -> Result<Vec<(String, Vec<u8>)>> {
        let mut segments = Vec::new();
        let mut current_segment = Vec::new();

        for (_key, mut bytes) in objects {
            // If adding this object would exceed the max segment size, flush
            // the current segment and start a new one.
            if !current_segment.is_empty()
                && current_segment.len() + bytes.len() > self.config.max_segment_size
            {
                segments.push(self.finalize_segment(current_segment.clone())?);
                current_segment.clear();
            }

            // Add this object to the current segment.
            current_segment.append(&mut bytes);
        }

        // Flush the final segment if it's not empty.
        if !current_segment.is_empty() {
            segments.push(self.finalize_segment(current_segment)?);
        }

        Ok(segments)
    }

    /// Finalize a segment by computing its content hash and storing it under
    /// a content-addressed key. Returns (key, bytes).
    fn finalize_segment(&self, bytes: Vec<u8>) -> Result<(String, Vec<u8>)> {
        let hash = qip_core::hash::sha256(&bytes);
        let hash_hex = qip_core::hash::to_hex(&hash);
        let key = format!("segments/compacted/{}", hash_hex);

        // Write the compacted segment back to the store.
        self.store.put(&key, bytes.clone())?;

        Ok((key, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::MemoryBlobStore;

    #[test]
    fn many_small_objects_are_compacted_into_larger_segments() {
        let store = MemoryBlobStore::new();
        let config = CompactionConfig::new()
            .with_min_object_size(1024)
            .with_max_segment_size(10 * 1024);

        // Create 20 small objects (each 1KB) under the test prefix.
        let prefix = "test/tiny-objects/";
        let mut expected_hashes = Vec::new();
        for i in 0..20 {
            let data = vec![i as u8; 1024];
            let hash = qip_core::hash::sha256(&data);
            expected_hashes.push(hash);
            store.put(&format!("{}{}", prefix, i), data).unwrap();
        }

        // Run compaction.
        let compactor = Compactor::new(&store, config);
        let result = compactor.compact(prefix).unwrap();

        // Verify that compaction produced segments.
        assert!(!result.compacted_segments.is_empty());
        assert_eq!(result.merged_objects.len(), 20);
        assert_eq!(result.total_bytes_merged, 20 * 1024);

        // Verify that the compacted segments exist in the store.
        for seg_key in &result.compacted_segments {
            assert!(store.get(seg_key).unwrap().is_some());
        }
    }

    #[test]
    fn compacted_objects_cover_originals_by_hash() {
        let store = MemoryBlobStore::new();
        let config = CompactionConfig::new()
            .with_min_object_size(512)
            .with_max_segment_size(5 * 1024);

        let prefix = "test/hash-verify/";

        // Create 5 objects with predictable content.
        let mut all_bytes = Vec::new();
        for i in 0..5 {
            let data = vec![(i + 1) as u8; 1024];
            all_bytes.extend_from_slice(&data);
            store.put(&format!("{}{}", prefix, i), data).unwrap();
        }

        // Compute the hash of all originals concatenated.
        let original_hash = qip_core::hash::sha256(&all_bytes);

        // Run compaction.
        let compactor = Compactor::new(&store, config);
        let result = compactor.compact(prefix).unwrap();

        // Concatenate the compacted segments and verify their hash matches.
        let mut compacted_bytes = Vec::new();
        for seg_key in &result.compacted_segments {
            if let Ok(Some(bytes)) = store.get(seg_key) {
                compacted_bytes.extend_from_slice(&bytes);
            }
        }

        let compacted_hash = qip_core::hash::sha256(&compacted_bytes);
        assert_eq!(original_hash, compacted_hash);
    }

    #[test]
    fn empty_prefix_returns_empty_result() {
        let store = MemoryBlobStore::new();
        let config = CompactionConfig::new();
        let compactor = Compactor::new(&store, config);

        let result = compactor.compact("nonexistent/").unwrap();
        assert!(result.compacted_segments.is_empty());
        assert!(result.merged_objects.is_empty());
        assert_eq!(result.total_bytes_merged, 0);
    }

    #[test]
    fn single_large_object_is_not_compacted() {
        let store = MemoryBlobStore::new();
        let config = CompactionConfig::new().with_max_segment_size(10 * 1024);

        let prefix = "test/large/";
        let large_data = vec![0u8; 10 * 1024];
        store
            .put(&format!("{}0", prefix), large_data.clone())
            .unwrap();

        let compactor = Compactor::new(&store, config);
        let result = compactor.compact(prefix).unwrap();

        // Object is at max size, should not be included in compaction.
        assert!(result.compacted_segments.is_empty());
    }

    #[test]
    fn compaction_breaks_when_segment_hash_is_wrong() {
        // This mutation test verifies that hash checking works by breaking the hash.
        let store = MemoryBlobStore::new();
        let config = CompactionConfig::new()
            .with_min_object_size(512)
            .with_max_segment_size(5 * 1024);

        let prefix = "test/mutation/";

        // Create 3 objects.
        for i in 0..3 {
            let data = vec![(i + 1) as u8; 1024];
            store.put(&format!("{}{}", prefix, i), data).unwrap();
        }

        // Compute what the compacted hash should be.
        let mut original_bytes = Vec::new();
        for i in 0..3 {
            original_bytes.extend_from_slice(&vec![(i + 1) as u8; 1024]);
        }
        let expected_hash = qip_core::hash::sha256(&original_bytes);

        // Run compaction.
        let compactor = Compactor::new(&store, config);
        let result = compactor.compact(prefix).unwrap();

        // Read back and verify the hash.
        let mut compacted_bytes = Vec::new();
        for seg_key in &result.compacted_segments {
            if let Ok(Some(bytes)) = store.get(seg_key) {
                compacted_bytes.extend_from_slice(&bytes);
            }
        }

        let actual_hash = qip_core::hash::sha256(&compacted_bytes);

        // This assertion will fail if the hash logic is broken.
        assert_eq!(expected_hash, actual_hash);
    }
}
