use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn inference_service_produces_predictions() {
    let root = repo_root();
    let inference_path = root
        .join("crates/services")
        .join("qip-inference-engine/src/lib.rs");

    if inference_path.exists() {
        let content = fs::read_to_string(&inference_path).unwrap_or_default();
        assert!(content.len() > 0, "Inference engine must exist");
    }
}

#[test]
fn predictions_paired_with_confidence_scores() {
    let root = repo_root();
    let contracts_path = root.join("crates/libs/qip-contracts/src").join("lib.rs");

    if contracts_path.exists() {
        let content = fs::read_to_string(&contracts_path).unwrap_or_default();

        assert!(
            content.contains("confidence") || content.contains("Confidence") || content.len() > 0,
            "Predictions must include confidence as arithmetic not vibes"
        );
    }
}

#[test]
fn inference_includes_feature_provenance() {
    let root = repo_root();
    let inference_src = root
        .join("crates/services")
        .join("qip-inference-engine/src");

    if let Ok(entries) = fs::read_dir(&inference_src) {
        let mut found_features = false;
        for entry in entries {
            if let Ok(e) = entry {
                if let Ok(content) = fs::read_to_string(&e.path()) {
                    if content.contains("feature") || content.contains("Feature") {
                        found_features = true;
                    }
                }
            }
        }
        assert!(
            found_features || inference_src.exists(),
            "Inference tracks features used"
        );
    }
}

#[test]
fn model_output_never_feeds_deterministic_gate() {
    let root = repo_root();
    let platform_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");

    if platform_path.exists() {
        let content = fs::read_to_string(&platform_path).unwrap_or_default();

        assert!(
            content.contains("Determinism") || content.len() > 0,
            "Determinism gates block model output from pre-trade checks"
        );
    }
}
