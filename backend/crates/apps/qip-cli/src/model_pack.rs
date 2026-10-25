//! `qip model-pack`: the three steps the publish workflow takes to turn an
//! unsigned draft into a release pack, and the check that decides whether a
//! pack loads (R12-CONTRACT-7064, ADR 0110, ADR 0149).
//!
//! The signing key is a Cloud KMS key the workflow's identity calls; this
//! process never holds it. So signing is split where the key sits:
//!
//! - `signing-message <pack.json> --key-version <name>` prints, as hex, the
//!   exact bytes [`qip_contracts::trust::signing_message`] builds for the
//!   pack under that key version — the message KMS `asymmetricSign` signs;
//! - `attach-signature <pack.json> --key-version <name> --der <hex>` prints
//!   the pack with the DER signature KMS returned attached in the text form
//!   [`qip_contracts::trust::ArtefactSignature`] encodes;
//! - `verify <pack.json> --trust-bundle <path> [--now <rfc3339>]` runs
//!   [`ModelPack::verify`] — shape, two-person publication, expiry and the
//!   signature under a `model-pack` key the bundle trusts — and exits
//!   [`INVALID`] when any arm refuses.
//!
//! Each step is a function returning what it printed and the exit code, so
//! a test asserts on the same values the workflow reads. Nothing here can
//! widen a pack: `attach-signature` changes only the `signature` field, and
//! a pack whose other bytes moved after the message was printed fails
//! `verify` because the signature covers them.

use qip_contracts::policy::ModelPack;
use qip_contracts::trust::{ArtefactClass, ArtefactSignature, ArtefactVerifier, signing_message};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};

/// The exit code of a pack that does not verify: distinct from the refusal
/// to run at all (a missing flag, a file that is not a pack), for the reason
/// `qip replay` keeps its verdict code distinct.
pub const INVALID: u8 = 3;

/// The hex message KMS signs for `pack` under `key_version`.
pub fn signing_message_hex(pack: &ModelPack, key_version: &str) -> Result<String> {
    // Checked through the signature type so the rules for a key-version
    // name live in one place, and a name KMS would reject is refused before
    // anybody signs a message attributed to it.
    ArtefactSignature::new(key_version, vec![0x30; 8])?;
    Ok(qip_core::hash::to_hex(&signing_message(
        ArtefactClass::ModelPack,
        key_version,
        &pack.signing_payload(),
    )))
}

/// `pack` with the DER signature `der_hex` attached under `key_version`.
pub fn attach_signature(pack: ModelPack, key_version: &str, der_hex: &str) -> Result<ModelPack> {
    let der = qip_core::hash::from_hex(der_hex.trim())
        .ok_or_else(|| Error::invalid("--der is not hex; pass the DER signature KMS returned"))?;
    let signature = ArtefactSignature::new(key_version, der)?.encode();
    Ok(ModelPack { signature, ..pack })
}

/// Whether `pack` loads at `now` under `verifier`.
pub fn verify(pack: &ModelPack, verifier: &dyn ArtefactVerifier, now: Timestamp) -> Result<()> {
    pack.verify(verifier, now)
}

fn flag<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
}

fn required<'a>(arguments: &'a [String], name: &str) -> Result<&'a str> {
    flag(arguments, name).ok_or_else(|| Error::invalid(format!("{name} is required")))
}

fn read_pack(path: &str) -> Result<ModelPack> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::io(format!("cannot read the model pack at {path}: {error}")))?;
    serde_json::from_str(&text)
        .map_err(|error| Error::invalid(format!("{path} is not a model pack: {error}")))
}

/// Run one `qip model-pack` step: the lines to print and the exit code.
pub fn run(arguments: &[String], now: Timestamp) -> Result<(Vec<String>, u8)> {
    let step = arguments.first().map(String::as_str).unwrap_or("");
    let pack_path = || {
        arguments
            .get(1)
            .map(String::as_str)
            .ok_or_else(|| Error::invalid(format!("qip model-pack {step} <pack.json> ...")))
    };
    match step {
        "signing-message" => {
            let pack = read_pack(pack_path()?)?;
            let hex = signing_message_hex(&pack, required(arguments, "--key-version")?)?;
            Ok((vec![hex], 0))
        }
        "attach-signature" => {
            let pack = read_pack(pack_path()?)?;
            let signed = attach_signature(
                pack,
                required(arguments, "--key-version")?,
                required(arguments, "--der")?,
            )?;
            let json = serde_json::to_string_pretty(&signed)
                .map_err(|error| Error::invalid(format!("the signed pack: {error}")))?;
            Ok((vec![json], 0))
        }
        "verify" => {
            let pack = read_pack(pack_path()?)?;
            let bundle = required(arguments, "--trust-bundle")?;
            let verifier =
                qip_transport::trust::BundleVerifier::from_file(std::path::Path::new(bundle))?;
            let now = match flag(arguments, "--now") {
                Some(text) => Timestamp::parse_rfc3339(text).ok_or_else(|| {
                    Error::invalid(format!("--now {text} is not an RFC 3339 time"))
                })?,
                None => now,
            };
            match verify(&pack, &verifier, now) {
                Ok(()) => Ok((
                    vec![format!(
                        "model pack {}: verified; it loads",
                        pack.artifact_digest
                    )],
                    0,
                )),
                Err(error) => Ok((
                    vec![format!(
                        "model pack {}: refused, {}",
                        pack.artifact_digest,
                        error.message()
                    )],
                    INVALID,
                )),
            }
        }
        other => Err(Error::invalid(format!(
            "qip model-pack has no step {other:?}; the steps are signing-message, \
             attach-signature and verify"
        ))),
    }
}
