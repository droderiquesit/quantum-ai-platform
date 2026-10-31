//! The Source Manifest Registry over Spanner (R12-DATA-8014), driven through
//! `SpannerKv` against an in-process REST double: the requests the adapter
//! writes are the bytes the double reads.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

mod support;

use qip_core::Timestamp;
use qip_financial::manifest::SourceManifest;
use qip_storage::gcp::manifest_registry::{KEY_PREFIX, SourceManifestRegistry, canonical_uri};
use support::spanner_double::Double;

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// A manifest written to the registry is read back, field for field, by the
/// canonical URI of its original, and the row the store holds is keyed by
/// that URI.
///
/// The failure prevented: a manifest that lived only inside the record that
/// carried it, so "what do we hold about this document" had no lookup at
/// all; and a registry keyed by content hash, which answers only a caller who
/// already holds the bytes' hash.
///
/// Mutation: key `register` by `manifest.sha256()` instead of
/// `canonical_uri(..)` — the lookup by URI finds nothing and the round-trip
/// assertion fails.
#[test]
fn a_registry_write_and_lookup_by_canonical_uri_round_trip_through_spanner() {
    let double = Double::start();
    let registry = SourceManifestRegistry::new(double.kv());
    let filing = SourceManifest::of("edgar", "https://sec.example/10-K/1", at(), b"filing text")
        .expect("a manifest");
    let wire = SourceManifest::of("wire", "https://wire.example/story/9", at(), b"story text")
        .expect("a manifest");

    assert!(registry.register(&filing).expect("the write commits"));
    assert!(registry.register(&wire).expect("the write commits"));
    let mut expected = vec![
        canonical_uri("edgar", "https://sec.example/10-K/1").unwrap(),
        canonical_uri("wire", "https://wire.example/story/9").unwrap(),
    ];
    expected.sort();
    assert_eq!(
        double.keys(),
        expected,
        "the store is keyed by canonical URI"
    );
    assert!(double.keys().iter().all(|key| key.starts_with(KEY_PREFIX)));

    assert_eq!(
        registry
            .lookup("edgar", "https://sec.example/10-K/1")
            .expect("the read answers"),
        Some(filing.clone()),
        "the manifest did not round-trip by its canonical URI"
    );
    assert_eq!(
        registry
            .lookup("edgar", "https://sec.example/10-K/2")
            .expect("the read answers"),
        None
    );
    assert_eq!(
        registry.from_source("edgar").expect("the scan answers"),
        vec![filing.clone()],
        "the source's listing holds its one manifest and not the other source's"
    );
    assert!(
        !registry.register(&filing).expect("the read answers"),
        "registering the same manifest twice is idempotent"
    );
}

/// The same original read with different bytes is refused rather than
/// overwritten, so the registry never erases what the platform first read
/// there.
///
/// Mutation: in `register`, treat any held row as idempotent (`Some(_) =>
/// Ok(false)`) — the revision is accepted silently and this fails.
#[test]
fn a_changed_original_is_refused_naming_both_hashes_and_the_first_manifest_stands() {
    let double = Double::start();
    let registry = SourceManifestRegistry::new(double.kv());
    let first = SourceManifest::of("edgar", "https://sec.example/10-K/1", at(), b"filing text")
        .expect("a manifest");
    let edited = SourceManifest::of(
        "edgar",
        "https://sec.example/10-K/1",
        at(),
        b"filing text, edited",
    )
    .expect("a manifest");
    registry.register(&first).expect("the first write commits");

    let refusal = registry
        .register(&edited)
        .expect_err("a different manifest for the same original is refused");
    assert!(
        refusal.message().contains(first.sha256()) && refusal.message().contains(edited.sha256()),
        "{}",
        refusal.message()
    );
    assert_eq!(
        registry
            .lookup("edgar", "https://sec.example/10-K/1")
            .unwrap(),
        Some(first)
    );
}
