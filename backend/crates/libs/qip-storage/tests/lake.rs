//! The Tick/Internal Lake: what it accepts, what it seals, and what it proves.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_storage::lake::{Entitlement, Lake, Manifest, Partition, RecordClass};
use qip_storage::{BlobStore, MemoryBlobStore};

fn ent(id: &str, raw: bool) -> Entitlement {
    Entitlement {
        id: id.into(),
        raw_retention_permitted: raw,
    }
}

fn part(instrument: &str, entitlement: Entitlement) -> Result<Partition> {
    Partition::new(
        RecordClass::Market,
        "XNAS",
        "2026-10-04",
        instrument,
        entitlement,
    )
}

#[test]
fn a_world_source_document_is_refused_by_name_and_the_lake_has_only_two_classes() -> Result<()> {
    assert_eq!(RecordClass::parse("market")?, RecordClass::Market);
    assert_eq!(RecordClass::parse("internal")?, RecordClass::Internal);
    for world in ["news", "world", "document", "fundamental", "Market"] {
        let refusal = RecordClass::parse(world).expect_err("a world class must be refused");
        assert!(
            refusal.to_string().contains(world),
            "the refusal names what was refused: {refusal}"
        );
    }
    Ok(())
}

#[test]
fn a_partition_without_an_entitlement_cannot_be_formed() -> Result<()> {
    assert!(part("AAPL", ent("", false)).is_err());
    assert!(part("AAPL", ent("nasdaq-l2", false)).is_ok(), "premise");
    assert!(
        part("AA/PL", ent("nasdaq-l2", false)).is_err(),
        "path escape"
    );
    assert!(
        Partition::new(
            RecordClass::Market,
            "XNAS",
            "2026-13-4x",
            "AAPL",
            ent("e", false)
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn a_written_segments_path_encodes_its_partition_keys() -> Result<()> {
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    let record = lake.write_segment(
        &part("AAPL", ent("nasdaq-l2", false))?,
        "seg-1",
        b"c".to_vec(),
        None,
    )?;
    for fragment in [
        "class=market",
        "venue=XNAS",
        "date=2026-10-04",
        "instrument=AAPL",
        "entitlement=nasdaq-l2",
    ] {
        assert!(
            record.key.contains(fragment),
            "{} lacks {fragment}",
            record.key
        );
    }
    Ok(())
}

#[test]
fn a_sealed_segment_cannot_be_overwritten_or_deleted_through_the_writer() -> Result<()> {
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    let p = part("AAPL", ent("nasdaq-l2", false))?;
    let record = lake.write_segment(&p, "seg-1", b"original".to_vec(), None)?;

    assert!(
        lake.write_segment(&p, "seg-1", b"rewritten".to_vec(), None)
            .is_err()
    );
    assert_eq!(
        store.get(&record.key)?,
        Some(b"original".to_vec()),
        "the original survived"
    );
    assert!(lake.delete_segment(&record.key).is_err());
    assert!(store.get(&record.key)?.is_some());
    // A correction is a new segment beside it.
    assert!(
        lake.write_segment(&p, "seg-1-correction", b"fix".to_vec(), None)
            .is_ok()
    );
    Ok(())
}

#[test]
fn raw_bytes_are_kept_only_where_the_entitlement_permits_and_the_hash_is_kept_either_way()
-> Result<()> {
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    let raw = b"8=FIX.4.4|35=W|".to_vec();
    let expected = sha256_hex(&raw);

    let denied = lake.write_segment(
        &part("AAPL", ent("no-raw", false))?,
        "s",
        b"canon".to_vec(),
        Some(raw.clone()),
    )?;
    assert_eq!(denied.raw_sha256.as_deref(), Some(expected.as_str()));
    assert!(denied.raw_key.is_none());
    let stored = store.list("lake/")?;
    assert!(
        stored.iter().all(|k| !k.ends_with(".raw")),
        "no raw bytes under a raw-retention-not-permitted entitlement: {stored:?}"
    );

    let allowed = lake.write_segment(
        &part("MSFT", ent("raw-ok", true))?,
        "s",
        b"canon".to_vec(),
        Some(raw),
    )?;
    let raw_key = allowed.raw_key.expect("raw is retained where permitted");
    let kept = store.get(&raw_key)?.expect("the raw object exists");
    assert_eq!(
        sha256_hex(&kept),
        allowed.raw_sha256.expect("hash recorded")
    );
    assert_eq!(sha256_hex(&kept), expected);
    Ok(())
}

fn two_entitlement_manifest(lake: &Lake<'_>) -> Result<Manifest> {
    let a = lake.write_segment(&part("AAPL", ent("A", false))?, "s", b"aaa".to_vec(), None)?;
    let b = lake.write_segment(&part("MSFT", ent("B", false))?, "s", b"bbb".to_vec(), None)?;
    let manifest = Manifest {
        name: "train-1".into(),
        segments: vec![a, b],
    };
    lake.write_manifest(&manifest)?;
    Ok(manifest)
}

#[test]
fn a_job_granted_one_entitlement_reads_only_that_partition_and_reports_it() -> Result<()> {
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    let manifest = two_entitlement_manifest(&lake)?;
    assert_eq!(manifest.segments.len(), 2, "premise: two partitions held");

    let read = lake.materialize("train-1", &["A"])?;
    assert_eq!(read.segments.len(), 1);
    assert_eq!(read.segments[0].1, b"aaa");
    assert_eq!(read.entitlements_read.iter().collect::<Vec<_>>(), ["A"]);
    assert_eq!(read.withheld.len(), 1);
    assert!(read.withheld[0].contains("entitlement=B"));
    Ok(())
}

#[test]
fn re_materializing_a_manifest_matches_its_checksums_and_a_corrupt_byte_names_the_segment()
-> Result<()> {
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    let manifest = two_entitlement_manifest(&lake)?;

    let whole = lake.materialize("train-1", &["A", "B"])?;
    for (record, bytes) in &whole.segments {
        assert_eq!(sha256_hex(bytes), record.canonical_sha256);
    }
    assert!(
        lake.verify("train-1")?.is_empty(),
        "premise: intact lake verifies clean"
    );

    // Corrupt one byte of the second segment behind the writer's back.
    let victim = manifest.segments[1].key.clone();
    store.put(&victim, b"bbc".to_vec())?;

    assert_eq!(lake.verify("train-1")?, std::slice::from_ref(&victim));
    let failure = lake
        .materialize("train-1", &["A", "B"])
        .expect_err("corruption must fail the read");
    assert!(failure.to_string().contains(&victim), "{failure}");
    Ok(())
}

/// A store whose reads see nothing: what two writers racing on one segment
/// id each saw, having both read before either wrote. Writes reach the real
/// store underneath.
#[derive(Debug, Default)]
struct RacedReads {
    inner: MemoryBlobStore,
}

impl BlobStore for RacedReads {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.inner.put(key, bytes)
    }
    fn put_if_absent(&self, key: &str, bytes: Vec<u8>) -> Result<bool> {
        self.inner.put_if_absent(key, bytes)
    }
    fn get(&self, _key: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn delete(&self, key: &str) -> Result<bool> {
        self.inner.delete(key)
    }
    fn list(&self, _prefix: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// R12-TICK-2055: the existence check is a read followed by a write, so two
/// writers racing on one segment id both passed it and the second silently
/// replaced the first. Here both writers' reads see nothing, as in the race;
/// exactly one seals, the other is refused, and the sealed bytes and the raw
/// bytes are both the winner's. Mutation: sealing with `put` (reporting the
/// write as created) lets the second writer replace the first.
#[test]
fn two_writers_racing_one_segment_id_seal_exactly_one() -> Result<()> {
    let store = RacedReads::default();
    let lake = Lake::new(&store);
    let p = part("AAPL", ent("nasdaq-l2", true))?;

    let first = lake.write_segment(&p, "seg-1", b"first".to_vec(), Some(b"raw-1".to_vec()))?;
    // Premise: the second writer's existence check passes, as it did in the
    // race — the store's reads show nothing sealed.
    assert_eq!(store.get(&first.key)?, None);
    let second = lake.write_segment(&p, "seg-1", b"second".to_vec(), Some(b"raw-2".to_vec()));

    assert!(second.is_err(), "both writers sealed one segment id");
    assert_eq!(store.inner.get(&first.key)?, Some(b"first".to_vec()));
    let raw_key = first.raw_key.clone().expect("raw retention was permitted");
    assert_eq!(store.inner.get(&raw_key)?, Some(b"raw-1".to_vec()));
    Ok(())
}

/// The file adapter's create-if-absent publishes by hard link, which fails
/// when the name exists: the second call writes nothing and reports so.
/// Mutation: publishing by `write_atomic`'s rename replaces the first bytes.
#[test]
fn a_file_store_creates_an_object_once_and_leaves_an_existing_one_untouched() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "qip-lake-create-once-{}-{}",
        std::process::id(),
        sha256_hex(b"create-once")
    ));
    let _ = std::fs::remove_dir_all(&root);
    let store = qip_storage::FileBlobStore::open(&root)?;
    assert!(store.put_if_absent("lake/a/seg-1.seg", b"first".to_vec())?);
    assert!(!store.put_if_absent("lake/a/seg-1.seg", b"second".to_vec())?);
    assert_eq!(store.get("lake/a/seg-1.seg")?, Some(b"first".to_vec()));
    assert_eq!(
        store.list("")?,
        ["lake/a/seg-1.seg"],
        "no scratch file left"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
