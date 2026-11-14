//! `qip reflex-package`: the steps the two-person `model-pack` publish takes
//! to turn a reflex package draft the deep brain wrote into a package a cell
//! loads (ADR 0131 decision 1, ADR 0535, R12-REASON-7203).
//!
//! The deep brain holds no package signer: the owner's 2026-10-31 ruling is
//! that no running process signs a reflex package. So the draft leaves the
//! deep brain unsigned, and signing is split where the `model-pack` key
//! sits, exactly as `qip model-pack` splits it:
//!
//! - `signing-message <draft.json> --key-version <name> --author <a>
//!   --approver <b> --approved-at <rfc3339>` prints, as hex, the message
//!   KMS `asymmetricSign` signs: the package digest and the publication,
//!   under the `model-pack` class;
//! - `attach-signature <draft.json> --key-version <name> --author <a>
//!   --approver <b> --approved-at <rfc3339> --der <hex>` prints the
//!   published package, refusing a one-person publication;
//! - `verify <published.json> --trust-bundle <path> [--now <rfc3339>]` runs
//!   [`PublishedReflexPackage::load`] and exits [`INVALID`] on any refusal.
//!
//! The publication is named on both signing steps because it is inside the
//! signed bytes: a package attached under other names than the message was
//! printed for does not verify.

use qip_contracts::reflex_package::{PublishedReflexPackage, ReflexPackageDraft};
use qip_contracts::trust::{ArtefactClass, ArtefactSignature, Publication, signing_message};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};

/// The exit code of a package that does not verify, distinct from a refusal
/// to run at all, as `qip model-pack` keeps it.
pub const INVALID: u8 = 3;

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

fn time(text: &str, name: &str) -> Result<Timestamp> {
    Timestamp::parse_rfc3339(text)
        .ok_or_else(|| Error::invalid(format!("{name} {text} is not an RFC 3339 time")))
}

fn read(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| Error::io(format!("cannot read {path}: {error}")))
}

fn publication(arguments: &[String]) -> Result<Publication> {
    let publication = Publication {
        author: required(arguments, "--author")?.to_string(),
        approver: required(arguments, "--approver")?.to_string(),
        approved_at: time(required(arguments, "--approved-at")?, "--approved-at")?,
    };
    // Refused before a message is printed, so nobody signs a one-person
    // publication and finds out at attach time.
    publication.validate()?;
    Ok(publication)
}

/// The hex message KMS signs for `draft` under `key_version` and
/// `publication`.
pub fn signing_message_hex(
    draft: &ReflexPackageDraft,
    key_version: &str,
    publication: &Publication,
) -> Result<String> {
    ArtefactSignature::new(key_version, vec![0x30; 8])?;
    Ok(qip_core::hash::to_hex(&signing_message(
        ArtefactClass::ModelPack,
        key_version,
        &draft.signing_canonical(publication),
    )))
}

/// Run one `qip reflex-package` step: the lines to print and the exit code.
pub fn run(arguments: &[String], now: Timestamp) -> Result<(Vec<String>, u8)> {
    let step = arguments.first().map(String::as_str).unwrap_or("");
    let path = || {
        arguments
            .get(1)
            .map(String::as_str)
            .ok_or_else(|| Error::invalid(format!("qip reflex-package {step} <file.json> ...")))
    };
    match step {
        "signing-message" => {
            let draft = ReflexPackageDraft::decode(&read(path()?)?)?;
            let hex = signing_message_hex(
                &draft,
                required(arguments, "--key-version")?,
                &publication(arguments)?,
            )?;
            Ok((vec![hex], 0))
        }
        "attach-signature" => {
            let draft = ReflexPackageDraft::decode(&read(path()?)?)?;
            let der = qip_core::hash::from_hex(required(arguments, "--der")?.trim()).ok_or_else(
                || Error::invalid("--der is not hex; pass the DER signature KMS returned"),
            )?;
            let signature =
                ArtefactSignature::new(required(arguments, "--key-version")?, der)?.encode();
            let published = draft.publish(publication(arguments)?, signature)?;
            let json = serde_json::to_string_pretty(&published)
                .map_err(|error| Error::invalid(format!("the published package: {error}")))?;
            Ok((vec![json], 0))
        }
        "verify" => {
            let bytes = read(path()?)?;
            let bundle = required(arguments, "--trust-bundle")?;
            let verifier =
                qip_transport::trust::BundleVerifier::from_file(std::path::Path::new(bundle))?;
            let now = match flag(arguments, "--now") {
                Some(text) => time(text, "--now")?,
                None => now,
            };
            match PublishedReflexPackage::load(&bytes, &verifier, now) {
                Ok(loaded) => Ok((
                    vec![format!(
                        "reflex package {}: verified; it loads",
                        loaded.package().package_id
                    )],
                    0,
                )),
                Err(error) => Ok((
                    vec![format!("reflex package: refused, {}", error.message())],
                    INVALID,
                )),
            }
        }
        other => Err(Error::invalid(format!(
            "qip reflex-package has no step {other:?}; the steps are signing-message, \
             attach-signature and verify"
        ))),
    }
}
