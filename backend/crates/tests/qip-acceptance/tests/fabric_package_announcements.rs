#![allow(clippy::unwrap_used)]
//! FABRIC-053: Package announcement schema validation.
//!
//! The fabric carries package announcements, never the packages themselves.
//! Every package-topic schema must carry only:
//! - version: the package format version
//! - digest: the content digest of the package artifact
//! - availability: whether the package is available for use
//! - activation: indication of package activation/readiness
//!
//! Packages (model weights, compiled strategies) live in signed artifact storage,
//! verified by digest before use. No package bytes travel the fabric.

use qip_contracts::policy::ModelManifest;
use serde_json::json;
use std::collections::BTreeMap;

/// The maximum size a package announcement record may occupy on the wire.
/// Ensures backpressure mechanisms work predictably and prevent OOM scenarios.
const MAX_ANNOUNCEMENT_SIZE_BYTES: usize = 8_192;

/// ModelManifest must carry only model digests, never weights.
/// This is enforced by the structure itself: no binary field for weights.
#[test]
fn model_manifest_carries_only_digests_never_weights() {
    let mut models = BTreeMap::new();
    models.insert("llm-v7".to_string(), "sha256:abc123".to_string());
    models.insert("qa-model".to_string(), "sha256:def456".to_string());

    let manifest = ModelManifest { models };

    // Serialize to JSON to verify the wire format
    let json = serde_json::to_value(&manifest).expect("serialize");

    // Must not contain any array/binary field that could hold weights
    let obj = json.as_object().expect("is object");
    assert!(obj.contains_key("models"), "must have models field");

    for (name, digest) in obj["models"].as_object().expect("models is object") {
        // Each value must be a string (digest), not an object or array
        assert!(digest.is_string(), "model {} digest must be string", name);

        let digest_str = digest.as_str().expect("digest is string");
        // Digests should be in format algo:hash
        assert!(
            digest_str.contains(':'),
            "digest {} should be in format 'algo:hash'",
            digest_str
        );
    }
}

/// ModelManifest must serialize to a bounded size for backpressure to work.
#[test]
fn model_manifest_serializes_to_bounded_size() {
    let mut models = BTreeMap::new();
    // Create a manifest with typical number of models that fits in 8KB
    for i in 0..50 {
        models.insert(format!("model-{:03}", i), format!("sha256:{:064x}", i));
    }

    let manifest = ModelManifest { models };
    let json = serde_json::to_vec(&manifest).expect("serialize");

    assert!(
        json.len() < MAX_ANNOUNCEMENT_SIZE_BYTES,
        "manifest size {} exceeds limit {}",
        json.len(),
        MAX_ANNOUNCEMENT_SIZE_BYTES
    );
}

/// Package announcement structure must not leak artifact storage details.
/// The platform fetches and verifies artifacts by digest; the announcement
/// carries only the digest, not the storage path or URL.
#[test]
fn package_announcement_digest_is_verifiable_by_construction() {
    let mut models = BTreeMap::new();
    models.insert(
        "model-name".to_string(),
        "sha256:0123456789abcdef".to_string(),
    );

    let manifest = ModelManifest { models };
    let serialized = serde_json::to_string(&manifest).expect("serialize");

    // Verify no storage paths or URLs leak into the announcement
    assert!(!serialized.contains("s3://"), "no S3 URLs");
    assert!(!serialized.contains("gs://"), "no GCS URLs");
    assert!(!serialized.contains("/models/"), "no local paths");
    assert!(!serialized.contains("http"), "no HTTP URLs");
}

/// Mutation test: removing the deny_unknown_fields gate should fail.
/// This ensures the schema is validated at deserialization time.
#[test]
fn model_manifest_refuses_unknown_fields() {
    let json = json!({
        "models": {
            "test-model": "sha256:abc123"
        },
        "unknown_field": "this should be rejected"
    });

    let result = serde_json::from_value::<ModelManifest>(json);
    assert!(
        result.is_err(),
        "should reject unknown fields by deny_unknown_fields"
    );
}

/// Serialized ModelManifest must be deterministic for digest consistency.
/// Same input produces same bytes across runs, enabling stable digesting.
#[test]
fn model_manifest_serialization_is_deterministic() {
    let mut models1 = BTreeMap::new();
    models1.insert("a-model".to_string(), "sha256:aaa".to_string());
    models1.insert("b-model".to_string(), "sha256:bbb".to_string());

    let mut models2 = BTreeMap::new();
    models2.insert("a-model".to_string(), "sha256:aaa".to_string());
    models2.insert("b-model".to_string(), "sha256:bbb".to_string());

    let manifest1 = ModelManifest { models: models1 };
    let manifest2 = ModelManifest { models: models2 };

    let json1 = serde_json::to_string(&manifest1).expect("serialize");
    let json2 = serde_json::to_string(&manifest2).expect("serialize");

    assert_eq!(
        json1, json2,
        "identical manifests must serialize identically"
    );
}
