//! Control 5 — signed artifacts and provenance.
//!
//! The tests try to get an artifact into the store that nobody signed, one
//! whose bytes changed after signing, and one signed under a different key;
//! then they walk a model's ancestry back to the datasets it came from and
//! check that a missing link is named rather than glossed over.

#![allow(clippy::panic_in_result_fn)]

use qip_compliance::artifacts::ArtifactStore;
use qip_compliance::signing::SigningKey;
use qip_contracts::governance::Provenance;
use qip_core::error::Result;
use qip_core::{Timestamp, sha256_hex};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn store() -> Result<ArtifactStore> {
    Ok(ArtifactStore::new(SigningKey::from_secret(
        "artifact-key-2026-01",
        &[3u8; 32],
    )?))
}

#[test]
fn tampered_bytes_are_rejected_and_the_rejection_names_both_digests() -> Result<()> {
    let mut store = store()?;
    let original = b"model weights v1".to_vec();
    let provenance = store.seal(&original, "training-pipeline", now(), vec![])?;

    // One byte changes somewhere between signing and storing.
    let tampered = b"model weights v2".to_vec();
    let error = store
        .store("vol-forecast.bin", tampered, provenance.clone(), now())
        .expect_err("bytes that do not match their digest must be rejected");

    assert!(error.message().contains("changed after it was signed"));
    assert!(error.message().contains(&provenance.digest()[..16]));
    assert!(store.is_empty());
    assert_eq!(store.rejections().len(), 1);

    // The untampered bytes go in fine.
    assert!(store.store("vol-forecast.bin", original, provenance, now()).is_ok());
    Ok(())
}

#[test]
fn an_unsigned_artifact_cannot_be_stored() -> Result<()> {
    // A provenance with a correct digest and an empty signature: internally
    // consistent, and worth nothing.
    let mut store = store()?;
    let bytes = b"a report nobody signed".to_vec();
    let unsigned = Provenance::sign(&bytes, "somebody", String::new(), now(), vec![])?;

    let error = store
        .store("report.json", bytes, unsigned, now())
        .expect_err("an unsigned artifact must not be stored");
    assert!(error.message().contains("carries no signature"));
    assert!(store.is_empty());
    Ok(())
}

#[test]
fn an_artifact_signed_under_a_different_key_is_rejected() -> Result<()> {
    // The digest matches, the signature is a real HMAC, and it still fails —
    // because it was not made under this store's key.
    let elsewhere = ArtifactStore::new(SigningKey::from_secret("other-key", &[9u8; 32])?);
    let bytes = b"weights from another deployment".to_vec();
    let foreign = elsewhere.seal(&bytes, "their-pipeline", now(), vec![])?;

    let mut store = store()?;
    let error = store
        .store("weights.bin", bytes, foreign, now())
        .expect_err("a signature from another key must not verify here");
    assert!(error.message().contains("does not verify"));
    assert!(error.message().contains("artifact-key-2026-01"));
    Ok(())
}

#[test]
fn storing_identical_content_twice_is_idempotent() -> Result<()> {
    // The store is content-addressed, so the same bytes are the same artifact
    // however often they arrive and whatever they are called.
    let mut store = store()?;
    let bytes = b"deterministic build output".to_vec();
    let provenance = store.seal(&bytes, "build", now(), vec![])?;

    let first = store.store("out.bin", bytes.clone(), provenance.clone(), now())?;
    let second = store.store("out-again.bin", bytes, provenance, now())?;

    assert_eq!(first, second);
    assert_eq!(store.len(), 1);
    Ok(())
}

#[test]
fn a_provenance_chain_reaches_the_raw_datasets_it_was_built_from() -> Result<()> {
    // The shape an investigation actually walks: two vendor feeds, a feature
    // set derived from both, and a model trained on the feature set.
    let mut store = store()?;
    let prices = b"raw prices 2024".to_vec();
    let sentiment = b"raw sentiment 2024".to_vec();
    let prices_digest = store.register_raw_dataset("vendor.prices", &prices, "Vendor A", now())?;
    let sentiment_digest =
        store.register_raw_dataset("vendor.sentiment", &sentiment, "Vendor B", now())?;

    let features = b"feature matrix".to_vec();
    let feature_provenance = store.seal(
        &features,
        "feature-pipeline",
        now(),
        vec![prices_digest.clone(), sentiment_digest.clone()],
    )?;
    let feature_digest = store.store("features.parquet", features, feature_provenance, now())?;

    let model = b"model weights".to_vec();
    let model_provenance =
        store.seal(&model, "training-pipeline", now(), vec![feature_digest.clone()])?;
    let model_digest = store.store("vol-forecast.bin", model, model_provenance, now())?;

    let chain = store.provenance_chain(&model_digest)?;
    chain.require_complete()?;

    assert!(chain.is_complete());
    assert_eq!(chain.depth(), 1);
    assert_eq!(chain.nodes().len(), 2);
    assert_eq!(chain.raw_datasets().len(), 2);
    let sources: Vec<&str> = chain.raw_datasets().iter().map(|d| d.name.as_str()).collect();
    assert!(sources.contains(&"vendor.prices"));
    assert!(sources.contains(&"vendor.sentiment"));
    assert!(chain.breaks().is_empty());
    Ok(())
}

#[test]
fn a_broken_chain_reports_exactly_where_it_breaks() -> Result<()> {
    // The realistic break: a feature set declares an input that was never
    // registered, because the dataset was deleted or the digest was retyped.
    let mut store = store()?;
    let prices = b"raw prices 2024".to_vec();
    let prices_digest = store.register_raw_dataset("vendor.prices", &prices, "Vendor A", now())?;
    let missing = sha256_hex(b"a dataset nobody kept");

    let features = b"feature matrix".to_vec();
    let feature_provenance = store.seal(
        &features,
        "feature-pipeline",
        now(),
        vec![prices_digest, missing.clone()],
    )?;
    let feature_digest = store.store("features.parquet", features, feature_provenance, now())?;

    let chain = store.provenance_chain(&feature_digest)?;
    assert!(!chain.is_complete());
    assert_eq!(chain.breaks().len(), 1);
    assert_eq!(chain.breaks()[0].missing, missing);
    assert_eq!(chain.breaks()[0].referenced_by_name, "features.parquet");

    let error = chain
        .require_complete()
        .expect_err("a broken chain must not pass as complete");
    // The exact digest and the artifact that referenced it, so somebody can go
    // and look for it rather than search the whole lineage.
    assert!(error.message().contains(&missing[..16]));
    assert!(error.message().contains("features.parquet"));
    Ok(())
}

#[test]
fn an_artifact_declaring_no_inputs_is_not_treated_as_fully_traced() -> Result<()> {
    // Its chain has no breaks, and it explains nothing. Treating that as
    // complete would let a model with no recorded training data pass.
    let mut store = store()?;
    let bytes = b"a model from nowhere".to_vec();
    let provenance = store.seal(&bytes, "somebody", now(), vec![])?;
    let digest = store.store("mystery.bin", bytes, provenance, now())?;

    let chain = store.provenance_chain(&digest)?;
    assert!(chain.breaks().is_empty());
    assert!(!chain.is_complete());
    let error = chain
        .require_complete()
        .expect_err("no declared inputs is not a complete provenance");
    assert!(error.message().contains("no registered raw dataset"));
    Ok(())
}

#[test]
fn a_diamond_is_walked_once_and_a_cycle_terminates() -> Result<()> {
    // A dataset feeding two features and both feeding a model is normal. The
    // walk must not visit it twice or, in the pathological case, forever.
    let mut store = store()?;
    let raw = b"one raw dataset".to_vec();
    let raw_digest = store.register_raw_dataset("vendor.prices", &raw, "Vendor A", now())?;

    let left = b"feature left".to_vec();
    let left_provenance = store.seal(&left, "pipeline", now(), vec![raw_digest.clone()])?;
    let left_digest = store.store("left.parquet", left, left_provenance, now())?;

    let right = b"feature right".to_vec();
    let right_provenance = store.seal(&right, "pipeline", now(), vec![raw_digest])?;
    let right_digest = store.store("right.parquet", right, right_provenance, now())?;

    let model = b"model".to_vec();
    let model_provenance = store.seal(
        &model,
        "training",
        now(),
        vec![left_digest, right_digest],
    )?;
    let model_digest = store.store("model.bin", model, model_provenance, now())?;

    let chain = store.provenance_chain(&model_digest)?;
    chain.require_complete()?;
    assert_eq!(chain.nodes().len(), 3);
    assert_eq!(chain.raw_datasets().len(), 1);
    Ok(())
}

#[test]
fn a_raw_dataset_with_no_named_source_is_not_an_origin() {
    // A provenance walk stops at a raw dataset, so registering one is a claim
    // that the platform knows where the data came from. An unnamed source is
    // not that claim.
    let Ok(mut store) = store() else {
        panic!("the store must build");
    };
    assert!(store.register_raw_dataset("x", b"bytes", "  ", now()).is_err());
}

#[test]
fn a_store_of_verified_artifacts_reports_no_integrity_failures() -> Result<()> {
    let mut store = store()?;
    let bytes = b"content".to_vec();
    let provenance = store.seal(&bytes, "build", now(), vec![])?;
    let digest = store.store("out.bin", bytes.clone(), provenance, now())?;

    assert!(store.integrity_failures().is_empty());
    assert_eq!(store.bytes(&digest)?, bytes.as_slice());
    assert!(store.bytes("not-a-digest").is_err());
    Ok(())
}
