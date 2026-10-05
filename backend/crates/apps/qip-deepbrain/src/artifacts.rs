//! Where a promoted model's artifact is written and read back (ADR 0083 §5,
//! MODEL-052).
//!
//! A blob store, not a key-value store. `StorageProvider::key_value` refuses
//! the Cloud Storage target for exactly this content and directs callers to
//! `blobs`, so an artifact written through the key-value port worked under
//! the memory and file targets and would have failed under the one a
//! deployment ships to. The read half is the same object fetched by its
//! digest-name and checked against it: a package manager that fetched bytes
//! it could not check would be trusting the bucket.

use qip_ai::registry::PublishedArtifact;
use qip_ai::serving::ModelArtifact;
use qip_core::error::{Error, Result};
use qip_storage::BlobStore;

/// The blob namespace a promoted model's artifact is written under.
pub const MODEL_ARTIFACTS_NAMESPACE: &str = "model-artifacts";

/// Write a promoted artifact under its digest-named key, as the JSON the
/// registry rendered it in.
pub fn write_model_artifact(store: &dyn BlobStore, published: &PublishedArtifact) -> Result<()> {
    // Parsed before it is stored: a registry that rendered something this
    // reader would refuse is caught at the write, not at the first fetch.
    ModelArtifact::from_json(&published.contents)?;
    store.put(
        &published.file_name,
        published.contents.clone().into_bytes(),
    )
}

/// Fetch an artifact by the digest that names it, refusing bytes that are not
/// the artifact the name promises.
pub fn read_model_artifact(store: &dyn BlobStore, digest: &str) -> Result<ModelArtifact> {
    let key = format!("{digest}.json");
    let bytes = store.get(&key)?.ok_or_else(|| {
        Error::not_found(format!(
            "no model artifact {key} is stored; it is written when its promotion is journalled, \
             so a missing one means the write failed or the store was replaced"
        ))
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Error::invalid(format!(
            "model artifact {key} is stored as text that is not UTF-8"
        ))
    })?;
    let artifact = ModelArtifact::from_json(&text)?;
    artifact.verify_digest()?;
    if artifact.digest != digest {
        return Err(Error::denied(format!(
            "{key} holds an artifact whose digest is {}; the name and the contents disagree, so \
             this is not the package that was asked for",
            artifact.digest
        )));
    }
    Ok(artifact)
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry};
    use qip_core::{ModelId, Timestamp};
    use qip_storage::MemoryBlobStore;
    use qip_training::dataset::TrainingDataset;
    use qip_training::job::TrainingSpec;
    use qip_training::local::{LocalTrainer, ModelFamily};
    use qip_training::serve::InTreeProvider;
    use std::collections::BTreeMap;

    fn promoted(version: &str, slope: f64) -> Result<(PublishedArtifact, String)> {
        let now = Timestamp::from_secs(1_760_000_000);
        let rows: Vec<Vec<f64>> = (0..60).map(|i| vec![(i as f64 * 0.3).sin()]).collect();
        let targets = rows.iter().map(|row| slope * row[0] + 0.5).collect();
        let times = (0..60)
            .map(|i| Timestamp::from_secs(1_760_000_000 + i))
            .collect();
        let data = TrainingDataset::new("series", vec!["x".to_string()], rows, targets, times)?;
        let spec = TrainingSpec::new(
            "artifact-test",
            version,
            "tests",
            "series",
            ModelFamily::Linear { ridge: 1e-6 },
        );
        let teacher = LocalTrainer::new().fit(&spec, &data, now)?;
        let artifact = InTreeProvider::pack(&teacher)?;
        let mut card = ModelCard::new(
            ModelId::from_string(artifact.reference.clone()),
            "artifact-test",
            version,
            "tests",
            now,
        )
        .with_features(vec!["x".to_string()]);
        card.evaluations.push(EvaluationRecord {
            evaluated_at: now,
            dataset: "series".to_string(),
            metrics: BTreeMap::new(),
            passed: true,
        });
        let mut registry = ModelRegistry::new();
        registry.register(card);
        let published = registry.promote_artifact(&artifact, now)?;
        Ok((published, artifact.digest))
    }

    #[test]
    fn a_written_artifact_is_fetched_back_by_its_digest_and_a_tampered_one_is_refused() -> Result<()>
    {
        let (published, digest) = promoted("0.1.0", 2.0)?;
        let store = MemoryBlobStore::new();
        // Premise: nothing is stored yet, so the read below can only succeed
        // by finding what the write put there.
        assert!(read_model_artifact(&store, &digest).is_err());

        write_model_artifact(&store, &published)?;
        let fetched = read_model_artifact(&store, &digest)?;
        assert_eq!(fetched.digest, digest);

        // A payload edited after the digest was computed: refused as not the
        // artifact that was promoted.
        let flipped = if digest.starts_with('a') { "b" } else { "a" };
        let edited = published
            .contents
            .replacen(&digest, &format!("{flipped}{}", &digest[1..]), 1);
        assert_ne!(edited, published.contents, "premise: the bytes changed");
        store.put(&published.file_name, edited.into_bytes())?;
        assert!(read_model_artifact(&store, &digest).is_err());

        // A different, internally consistent artifact stored under this name:
        // the name no longer vouches for the contents.
        let (other, other_digest) = promoted("0.2.0", -3.0)?;
        assert_ne!(other_digest, digest, "premise: two different artifacts");
        store.put(&published.file_name, other.contents.into_bytes())?;
        let refused = read_model_artifact(&store, &digest).unwrap_err();
        assert!(refused.message().contains("disagree"), "{refused}");
        Ok(())
    }
}
