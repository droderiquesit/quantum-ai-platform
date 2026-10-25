//! `qip model-pack`: a draft signed through the three steps the publish
//! workflow takes verifies, and the same pack with its signature changed by
//! one byte does not (R12-CONTRACT-7064).

#![allow(clippy::panic_in_result_fn)]

use qip_cli::model_pack::{self, INVALID};
use qip_contracts::policy::ModelPack;
use qip_contracts::trust::{
    ArtefactClass, ArtefactSignature, ArtefactSigner, Publication, TrustBundle, TrustedKey,
};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_transport::trust::LocalSigner;
use std::collections::BTreeSet;
use std::path::PathBuf;

const NOW: Timestamp = Timestamp::from_secs(1_800_000_000);
const KEY_VERSION: &str = "local/model-pack/1";

fn draft() -> ModelPack {
    ModelPack {
        artifact_digest: "ab".repeat(32),
        signature: "unsigned".to_string(),
        features: vec!["mid".to_string(), "spread".to_string()],
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

fn scratch(name: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("qip-model-pack-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|error| Error::io(error.to_string()))?;
    Ok(dir)
}

fn write(path: &PathBuf, text: &str) -> Result<()> {
    std::fs::write(path, text).map_err(|error| Error::io(error.to_string()))
}

fn json<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::invalid(error.to_string()))
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

#[test]
fn a_model_pack_whose_signature_changes_by_one_byte_is_refused_by_qip_model_pack_verify()
-> Result<()> {
    // The publish workflow signs with a KMS key this process never holds, so
    // the three steps are where a wrong byte could enter unnoticed: the
    // message printed, the signature attached, the verdict read. Before these
    // verbs existed there was no operator path at all from a draft to a pack
    // the kernel would load, and the contract's signature was a field nobody
    // outside the kernel's own tests ever filled.
    let dir = scratch("one-byte")?;
    let signer = LocalSigner::generate(ArtefactClass::ModelPack, KEY_VERSION)?;
    let bundle = TrustBundle::new(
        vec![TrustedKey {
            class: ArtefactClass::ModelPack,
            key_version: KEY_VERSION.to_string(),
            public_key: signer.public_key_hex(),
            not_after: Timestamp::from_secs(4_000_000_000),
        }],
        BTreeSet::new(),
    )?;
    let bundle_path = dir.join("bundle.json");
    write(&bundle_path, &json(&bundle)?)?;
    let draft_path = dir.join("draft.json");
    write(&draft_path, &json(&draft())?)?;
    let draft_arg = draft_path.to_string_lossy().to_string();
    let bundle_arg = bundle_path.to_string_lossy().to_string();

    // Step 1: the message, signed as KMS would sign it.
    let (lines, code) = model_pack::run(
        &args(&["signing-message", &draft_arg, "--key-version", KEY_VERSION]),
        NOW,
    )?;
    assert_eq!(code, 0);
    let message = qip_core::hash::from_hex(&lines[0])
        .ok_or_else(|| Error::invalid("the signing message is not hex"))?;
    let der = signer.sign_message(&message)?;

    // Step 2: attached.
    let (lines, code) = model_pack::run(
        &args(&[
            "attach-signature",
            &draft_arg,
            "--key-version",
            KEY_VERSION,
            "--der",
            &qip_core::hash::to_hex(&der),
        ]),
        NOW,
    )?;
    assert_eq!(code, 0);
    let signed_path = dir.join("signed.json");
    write(&signed_path, &lines[0])?;
    let signed_arg = signed_path.to_string_lossy().to_string();

    // Step 3, the premise: the signed pack verifies.
    let (lines, code) = model_pack::run(
        &args(&["verify", &signed_arg, "--trust-bundle", &bundle_arg]),
        NOW,
    )?;
    assert_eq!(
        code, 0,
        "the premise failed: the signed pack did not verify: {lines:?}"
    );

    // One byte of the DER changed: refused, by the verdict code.
    let mut pack: ModelPack = serde_json::from_str(
        &std::fs::read_to_string(&signed_path).map_err(|error| Error::io(error.to_string()))?,
    )
    .map_err(|error| Error::invalid(error.to_string()))?;
    let parsed = ArtefactSignature::parse(&pack.signature)?;
    let mut tampered = der.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;
    pack.signature = ArtefactSignature::new(parsed.key_version().to_string(), tampered)?.encode();
    let tampered_path = dir.join("tampered.json");
    write(&tampered_path, &json(&pack)?)?;
    let (lines, code) = model_pack::run(
        &args(&[
            "verify",
            &tampered_path.to_string_lossy(),
            "--trust-bundle",
            &bundle_arg,
        ]),
        NOW,
    )?;
    assert_eq!(
        code, INVALID,
        "a pack signed over other bytes verified: {lines:?}"
    );
    assert!(lines[0].contains("refused"), "{lines:?}");

    // And a field changed after signing is refused for the same reason.
    let mut widened: ModelPack = serde_json::from_str(
        &std::fs::read_to_string(&signed_path).map_err(|error| Error::io(error.to_string()))?,
    )
    .map_err(|error| Error::invalid(error.to_string()))?;
    widened.allowed_universes.insert("EVERYTHING".to_string());
    let widened_path = dir.join("widened.json");
    write(&widened_path, &json(&widened)?)?;
    let (_, code) = model_pack::run(
        &args(&[
            "verify",
            &widened_path.to_string_lossy(),
            "--trust-bundle",
            &bundle_arg,
        ]),
        NOW,
    )?;
    assert_eq!(code, INVALID, "a pack widened after signing verified");
    Ok(())
}
