//! The compactor serves the lake and refuses the segment archive (ADR 0156 §2,
//! R12-DATA-9187).
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_storage::blob::{BlobStore, MemoryBlobStore};
use qip_storage::compaction::{CompactionConfig, Compactor};

fn snapshot(store: &MemoryBlobStore) -> Vec<(String, Vec<u8>)> {
    let mut keys = store.list("").unwrap();
    keys.sort();
    keys.into_iter()
        .map(|key| {
            let bytes = store.get(&key).unwrap().unwrap();
            (key, bytes)
        })
        .collect()
}

fn config() -> CompactionConfig {
    CompactionConfig::new()
        .with_min_object_size(1)
        .with_max_segment_size(64 * 1024)
}

/// Compacting the archive prefix is refused and every object and manifest is
/// left as it was. The failure this prevents: merging sealed segments into a
/// content-addressed blob and returning the originals as deletable, after
/// which `ArchiveReader::hydrate` cannot fetch any archived segment by the key
/// its manifest names and the seal chain has an object it cannot walk.
///
/// A lake prefix of small objects still compacts first, so the refusal below
/// is the archive's and not the compactor's being unable to run.
///
/// Mutation: remove the `is_archive_key` refusal from `Compactor::compact`.
/// The archive objects are merged, the store gains a `segments/compacted/`
/// object, and the untouched assertion fails.
#[test]
fn compacting_the_archive_prefix_is_refused_and_leaves_every_object_and_manifest_untouched() {
    let store = MemoryBlobStore::new();
    for i in 0..4u8 {
        store.put(&format!("lake/ticks/{i}"), vec![i; 64]).unwrap();
    }
    let lake = Compactor::new(&store, config())
        .compact("lake/ticks/")
        .unwrap();
    assert_eq!(
        lake.merged_objects.len(),
        4,
        "premise: a lake prefix of small objects still compacts"
    );

    let store = MemoryBlobStore::new();
    for i in 0..4u8 {
        store
            .put(&format!("segments/sha256/{i:064x}"), vec![i; 64])
            .unwrap();
        store
            .put(&format!("manifests/stream/0000000000/{i:020}"), vec![i; 16])
            .unwrap();
        store
            .put(&format!("fabric/segments/sha256/{i:064x}"), vec![i; 64])
            .unwrap();
    }
    store.put("lake/ticks/0", vec![9; 64]).unwrap();
    let before = snapshot(&store);
    assert!(before.len() >= 13, "premise: the archive objects exist");

    for prefix in [
        "segments/sha256/",
        "manifests/",
        "fabric/segments/sha256/",
        // A prefix that merely contains the archive is refused too: the
        // refusal is on what would be merged, not on the spelling asked for.
        "",
        "segments/",
    ] {
        let error = Compactor::new(&store, config())
            .compact(prefix)
            .expect_err("a prefix that lists an archive object is refused");
        assert!(
            error.to_string().contains("segment archive"),
            "{prefix:?}: {error}"
        );
        assert_eq!(
            snapshot(&store),
            before,
            "{prefix:?}: a refused pass left the store exactly as it was"
        );
    }
}

/// `min_object_size` is read: an object smaller than it is left alone and is
/// not handed back as merged, and what is reported merged is exactly what was.
/// The failure this prevents: a stored-and-never-read field, and a result
/// that listed every key under the prefix as deletable, including objects the
/// pass never read into a segment.
///
/// Mutation: drop the `bytes.len() >= self.config.min_object_size` condition.
/// The 8-byte object is merged and the reported list has four entries.
#[test]
fn objects_below_the_minimum_are_left_alone_and_only_merged_objects_are_reported() {
    let store = MemoryBlobStore::new();
    store.put("lake/a", vec![1; 100]).unwrap();
    store.put("lake/b", vec![2; 100]).unwrap();
    store.put("lake/c", vec![3; 100]).unwrap();
    store.put("lake/tiny", vec![4; 8]).unwrap();
    let result = Compactor::new(
        &store,
        CompactionConfig::new()
            .with_min_object_size(50)
            .with_max_segment_size(10_000),
    )
    .compact("lake/")
    .unwrap();
    assert_eq!(
        result.merged_objects,
        vec!["lake/a", "lake/b", "lake/c"],
        "the 8-byte object is under the minimum and is not merged"
    );
    assert_eq!(result.total_bytes_merged, 300);
}
