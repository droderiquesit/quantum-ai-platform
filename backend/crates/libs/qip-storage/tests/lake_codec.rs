//! The tick-lake codec (R12-TICK-2057, ADR 0113 §3): off by default, a
//! measurement that writes nothing, and — when switched on — segments that
//! are smaller than their input, read back exactly, and keep the lineage
//! hash of their uncompressed content.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_storage::lake::{Entitlement, Lake, Manifest, Partition, RecordClass};
use qip_storage::lake_codec::{SegmentCodec, measure_partitions};
use qip_storage::{BlobStore, MemoryBlobStore};

fn partition(instrument: &str, date: &str) -> Result<Partition> {
    Partition::new(
        RecordClass::Market,
        "XLON",
        date,
        instrument,
        Entitlement {
            id: "vendor-depth".into(),
            raw_retention_permitted: false,
        },
    )
}

/// A day of repetitive ticks, the shape a real segment has: the same keys
/// on every line, slowly moving numbers.
fn canonical_ticks(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..lines {
        out.extend_from_slice(
            format!(
                "{{\"capture_time_ns\":{},\"exchange_time_ns\":{},\"feed\":\"itch-a\",\
                 \"partition\":0,\"sequence\":{i},\"event\":{{\"bid\":\"{}.{:02}\"}}}}\n",
                1_700_000_000_000_000_000_i64 + i as i64 * 1_000,
                1_700_000_000_000_000_000_i64 + i as i64 * 1_000 - 7,
                100 + i % 3,
                i % 100
            )
            .as_bytes(),
        );
    }
    out
}

/// Mutation: make `SegmentCodec::encode` return the canonical bytes for
/// `Zstd` (store uncompressed) — the size assertion fails. Make `decode`
/// return the stored bytes — the round-trip assertion fails.
#[test]
fn a_zstd_segment_is_smaller_than_its_input_and_reads_back_byte_for_byte() -> Result<()> {
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store).with_codec(SegmentCodec::zstd(3)?);
    let canonical = canonical_ticks(2_000);
    assert!(
        canonical.len() > 100_000,
        "premise: a segment worth compressing"
    );
    let record = lake.write_segment(
        &partition("obj-ACME", "2026-10-11")?,
        "seg-1",
        canonical.clone(),
        None,
    )?;
    assert!(
        record.key.ends_with(".canonical.zst"),
        "stored under the codec's suffix: {}",
        record.key
    );
    let stored = store.get(&record.key)?.expect("the segment was written");
    assert!(
        stored.len() * 4 < canonical.len(),
        "a compressed segment of repetitive ticks is far smaller than its input: {} vs {}",
        stored.len(),
        canonical.len()
    );
    assert_ne!(
        stored, canonical,
        "what is stored is not the uncompressed input"
    );
    assert_eq!(
        lake.read_canonical(&record)?,
        canonical,
        "it reads back exactly"
    );
    assert_eq!(
        record.canonical_sha256,
        sha256_hex(&canonical),
        "the lineage hash names the uncompressed content, whatever stored it"
    );
    Ok(())
}

/// Mutation: default `Lake::new` to `SegmentCodec::Zstd` — the suffix and
/// byte-identity assertions fail.
#[test]
fn the_codec_is_off_by_default_and_a_default_lake_stores_canonical_bytes_as_written() -> Result<()>
{
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    assert_eq!(lake.codec(), SegmentCodec::Identity);
    let canonical = canonical_ticks(50);
    let record = lake.write_segment(
        &partition("obj-ACME", "2026-10-11")?,
        "seg-1",
        canonical.clone(),
        None,
    )?;
    assert!(record.key.ends_with(".canonical"), "{}", record.key);
    assert_eq!(
        store.get(&record.key)?.as_deref(),
        Some(canonical.as_slice())
    );
    // An identity record serialises exactly as it did before the codec
    // existed, so every pinned manifest still parses.
    let json = serde_json::to_string(&record)?;
    assert!(!json.contains("\"codec\""), "identity is omitted: {json}");
    assert_eq!(SegmentCodec::parse("")?, SegmentCodec::Identity);
    assert!(
        SegmentCodec::parse("zsdt").is_err(),
        "a typo is refused, not read as identity"
    );
    assert!(
        SegmentCodec::parse("zstd:22").is_err(),
        "the ultra range is refused"
    );
    Ok(())
}

/// Mutation: make `Lake::sealed` look only under the current codec's suffix
/// — the second write is accepted and history is shadowed.
#[test]
fn a_sealed_segment_id_cannot_be_rewritten_by_switching_the_codec_on() -> Result<()> {
    let store = MemoryBlobStore::new();
    let p = partition("obj-ACME", "2026-10-11")?;
    Lake::new(&store).write_segment(&p, "seg-1", canonical_ticks(10), None)?;
    let compressed = Lake::new(&store).with_codec(SegmentCodec::zstd(3)?);
    let refusal = compressed
        .write_segment(&p, "seg-1", canonical_ticks(11), None)
        .expect_err("a sealed id is sealed under every codec");
    assert_eq!(refusal.code(), "denied", "{refusal}");
    assert_eq!(
        store.list("lake/")?.len(),
        1,
        "nothing was written beside it"
    );
    Ok(())
}

/// Mutation: key the report by the whole store rather than per partition
/// (e.g. a constant key), or `put` the candidate's bytes — the per-partition
/// and wrote-nothing assertions fail.
#[test]
fn the_measurement_reports_raw_and_compressed_bytes_per_partition_and_writes_nothing() -> Result<()>
{
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    lake.write_segment(
        &partition("obj-ACME", "2026-10-10")?,
        "seg-1",
        canonical_ticks(400),
        None,
    )?;
    lake.write_segment(
        &partition("obj-ACME", "2026-10-11")?,
        "seg-1",
        canonical_ticks(800),
        None,
    )?;
    lake.write_segment(
        &partition("obj-BETA", "2026-10-11")?,
        "seg-1",
        canonical_ticks(200),
        None,
    )?;
    let before = store.list("")?;
    let report = measure_partitions(&store, "lake/class=market/", SegmentCodec::zstd(3)?)?;
    assert_eq!(store.list("")?, before, "a measurement writes nothing");
    assert_eq!(
        report.partitions.len(),
        3,
        "one row per partition: {:?}",
        report.partitions.keys()
    );
    for (prefix, measurement) in &report.partitions {
        assert_eq!(measurement.segments, 1, "{prefix}");
        let ratio = measurement.ratio().expect("bytes were measured");
        assert!(ratio > 0.0 && ratio < 1.0, "{prefix}: ratio {ratio}");
    }
    let by_date = report.by_date();
    assert_eq!(by_date.len(), 2, "two days measured: {:?}", by_date.keys());
    assert_eq!(by_date["2026-10-11"].segments, 2);
    assert_eq!(
        report.total().raw_bytes,
        (canonical_ticks(400).len() + canonical_ticks(800).len() + canonical_ticks(200).len())
            as u64
    );
    assert!(
        measure_partitions(&store, "lake/", SegmentCodec::Identity).is_err(),
        "measuring the identity codec is refused"
    );
    Ok(())
}

/// Mutation: make `Lake::verify` compare the stored (compressed) bytes'
/// hash with `canonical_sha256` — the intact manifest reports every segment
/// as bad.
#[test]
fn a_manifest_over_compressed_segments_verifies_and_a_flipped_byte_fails_it() -> Result<()> {
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store).with_codec(SegmentCodec::zstd(3)?);
    let record = lake.write_segment(
        &partition("obj-ACME", "2026-10-11")?,
        "seg-1",
        canonical_ticks(300),
        None,
    )?;
    lake.write_manifest(&Manifest {
        name: "pinned".into(),
        segments: vec![record.clone()],
    })?;
    assert!(
        lake.verify("pinned")?.is_empty(),
        "an intact compressed segment verifies"
    );
    let read = lake.materialize("pinned", &["vendor-depth"])?;
    assert_eq!(
        read.segments[0].1,
        canonical_ticks(300),
        "materialize hands back canonical bytes"
    );
    let mut stored = store.get(&record.key)?.expect("written");
    let middle = stored.len() / 2;
    stored[middle] ^= 0xFF;
    store.put(&record.key, stored)?;
    assert_eq!(
        lake.verify("pinned")?,
        vec![record.key.clone()],
        "a corrupt frame is named"
    );
    assert!(lake.materialize("pinned", &["vendor-depth"]).is_err());
    Ok(())
}
