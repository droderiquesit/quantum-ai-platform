//! `qip control-pack`: the rollout record of a signed model pack is built
//! from the reviewed rollout fields and the pack itself, and stands only for
//! the nodes it targets (R12-CONTRACT-7091, ADR 0149 decision 4).

#![allow(clippy::panic_in_result_fn)]

use qip_cli::control_pack::{self, INVALID};
use qip_contracts::policy::ModelPack;
use qip_contracts::trust::Publication;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use std::path::Path;

const NOW: Timestamp = Timestamp::from_secs(1_800_000_000);

fn pack() -> ModelPack {
    ModelPack {
        artifact_digest: "ab".repeat(32),
        signature: "ecdsa-p256-sha256:local/model-pack/1:3006".to_string(),
        features: vec!["mid".to_string()],
        calibration: 0.8,
        allowed_universes: ["ACME".to_string()].into_iter().collect(),
        budget_microseconds: 500,
        expires_at: 1_900_000_000,
        rollback_parent: String::new(),
        publication: Publication {
            author: "author@example.com".to_string(),
            approver: "approver@example.com".to_string(),
            approved_at: Timestamp::from_secs(1_799_000_000),
        },
        inline_model: None,
    }
}

fn write(dir: &Path, name: &str, text: &str) -> Result<String> {
    let path = dir.join(name);
    std::fs::write(&path, text).map_err(|error| Error::io(error.to_string()))?;
    Ok(path.to_string_lossy().to_string())
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

#[test]
fn an_announcement_stands_only_for_the_node_it_targets_and_only_for_the_pack_it_attests()
-> Result<()> {
    // Nothing outside `qip-contracts` built an announcement, so a release's
    // targets lived in no record anything could check. The announcement is
    // bound to the signed pack by what this verb fills in, and narrowed by
    // what the reviewed rollout states.
    let dir = std::env::temp_dir().join(format!("qip-control-pack-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|error| Error::io(error.to_string()))?;
    let digest = "ab".repeat(32);
    let pack_path = write(
        &dir,
        "pack.json",
        &serde_json::to_string(&pack()).map_err(|error| Error::invalid(error.to_string()))?,
    )?;
    let rollout = serde_json::json!({
        "kind": "model_pack",
        "uri": format!("oci://registry/packs@sha256:{digest}"),
        "version": 1,
        "target_regions": ["europe-west2"],
        "target_nodes": ["london-1"],
        "activate_from": Timestamp::from_secs(1_799_000_000),
        "activate_until": Timestamp::from_secs(1_850_000_000),
        "expires_at": Timestamp::from_secs(1_860_000_000),
        "rollback_parent": "none",
        "required_acks": 1,
    });
    let rollout_path = write(&dir, "rollout.json", &rollout.to_string())?;
    let (lines, code) = control_pack::run(
        &args(&["announce", &pack_path, "--rollout", &rollout_path]),
        NOW,
    )?;
    assert_eq!(code, 0);
    let announcement_path = write(&dir, "announcement.json", &lines[0])?;
    let verify = |pack: &str, region: &str, node: &str| {
        control_pack::run(
            &args(&[
                "verify",
                &announcement_path,
                "--pack",
                pack,
                "--region",
                region,
                "--node",
                node,
            ]),
            NOW,
        )
    };

    // Premise: the targeted node, the signed pack, inside the window.
    let (lines, code) = verify(&pack_path, "europe-west2", "london-1")?;
    assert_eq!(
        code, 0,
        "premise: the announcement did not stand: {lines:?}"
    );

    let (_, code) = verify(&pack_path, "europe-west2", "tokyo-1")?;
    assert_eq!(
        code, INVALID,
        "the announcement stood for a node it does not target"
    );
    let (_, code) = verify(&pack_path, "asia-northeast1", "london-1")?;
    assert_eq!(
        code, INVALID,
        "the announcement stood for a region it does not target"
    );

    let mut resigned = pack();
    resigned.signature = "ecdsa-p256-sha256:local/model-pack/1:3007".to_string();
    let resigned_path = write(
        &dir,
        "resigned.json",
        &serde_json::to_string(&resigned).map_err(|error| Error::invalid(error.to_string()))?,
    )?;
    let (lines, code) = verify(&resigned_path, "europe-west2", "london-1")?;
    assert_eq!(
        code, INVALID,
        "the announcement stood for another signing: {lines:?}"
    );
    Ok(())
}
