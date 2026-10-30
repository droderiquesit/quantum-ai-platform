//! `qip control-pack`: the release's rollout record for a signed model pack
//! (R12-CONTRACT-7091, ADR 0149 decision 4).
//!
//! The target regions and nodes, the activation window, the expiry, the
//! rollback parent and the acknowledgements a release needs are fields of
//! the reviewed release file, so choosing them is the two publishers'
//! decision and no process invents them. What a process may not be trusted
//! to type by hand is what binds the record to the release: the artifact
//! digest, the signers and the attestation. So:
//!
//! - `announce <pack.json> --rollout <rollout.json>` fills those three from
//!   the signed pack and prints the announcement, refused by the contract's
//!   own checks when the rollout is not one;
//! - `verify <announcement.json> --pack <pack.json> --region <r> --node <n>
//!   [--now <rfc3339>]` says whether the announcement attests to that signed
//!   pack and targets that node now, exiting [`INVALID`] when it does not.
//!
//! Neither step can widen a release: an announcement only narrows which cells
//! are told about a pack, and the centre refuses one that does not attest to
//! the pack it holds.

use qip_contracts::control_pack::{
    ArtifactKind, ControlPackAnnouncement, ControlPackAnnouncementFields, RollbackParent,
    attestation_of,
};
use qip_contracts::policy::ModelPack;
use qip_contracts::validated::Contract;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::Deserialize;

/// The exit code of an announcement that does not attest to the pack or does
/// not target the node, distinct from a refusal to run at all.
pub const INVALID: u8 = 3;

/// The reviewed release file's rollout fields: everything the announcement
/// carries except what binds it to the signed pack.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rollout {
    pub kind: ArtifactKind,
    pub uri: String,
    pub version: u64,
    pub target_regions: Vec<String>,
    pub target_nodes: Vec<String>,
    pub activate_from: Timestamp,
    pub activate_until: Timestamp,
    pub expires_at: Timestamp,
    pub rollback_parent: RollbackParent,
    pub required_acks: u32,
}

/// The announcement of `pack` under `rollout`: the artifact digest, the
/// signers and the attestation taken from the signed pack.
pub fn announce(pack: &ModelPack, rollout: Rollout) -> Result<ControlPackAnnouncement> {
    ControlPackAnnouncement::new(ControlPackAnnouncementFields {
        schema_version: ControlPackAnnouncementFields::SCHEMA_VERSION,
        kind: rollout.kind,
        uri: rollout.uri,
        digest: pack.artifact_digest.clone(),
        version: rollout.version,
        signer: format!(
            "{} and {}",
            pack.publication.author, pack.publication.approver
        ),
        attestation: attestation_of(pack),
        target_regions: rollout.target_regions,
        target_nodes: rollout.target_nodes,
        activate_from: rollout.activate_from,
        activate_until: rollout.activate_until,
        expires_at: rollout.expires_at,
        rollback_parent: rollout.rollback_parent,
        required_acks: rollout.required_acks,
    })
}

/// Why `announcement` does not stand for `pack` at `node` in `region` at
/// `now`, or `None` when it does.
pub fn refusal(
    announcement: &ControlPackAnnouncement,
    pack: &ModelPack,
    region: &str,
    node: &str,
    now: Timestamp,
) -> Option<String> {
    if announcement.digest != pack.artifact_digest {
        return Some(format!(
            "announces artifact {} and the pack is {}",
            announcement.digest, pack.artifact_digest
        ));
    }
    if announcement.attestation != attestation_of(pack) {
        return Some(
            "does not attest to this signed pack; it was made for another signing or none"
                .to_string(),
        );
    }
    if !announcement.targets(region, node, now) {
        return Some(format!(
            "does not target node {node} in {region} at {}",
            now.to_rfc3339()
        ));
    }
    None
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

fn read<T: for<'de> Deserialize<'de>>(path: &str, what: &str) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::io(format!("cannot read the {what} at {path}: {error}")))?;
    serde_json::from_str(&text)
        .map_err(|error| Error::invalid(format!("{path} is not a {what}: {error}")))
}

/// Run one `qip control-pack` step: the lines to print and the exit code.
pub fn run(arguments: &[String], now: Timestamp) -> Result<(Vec<String>, u8)> {
    let step = arguments.first().map(String::as_str).unwrap_or("");
    let subject = || {
        arguments
            .get(1)
            .map(String::as_str)
            .ok_or_else(|| Error::invalid(format!("qip control-pack {step} <file> ...")))
    };
    match step {
        "announce" => {
            let pack: ModelPack = read(subject()?, "model pack")?;
            let rollout: Rollout = read(required(arguments, "--rollout")?, "rollout")?;
            let announcement = announce(&pack, rollout)?;
            let json = serde_json::to_string_pretty(&announcement)
                .map_err(|error| Error::invalid(format!("the announcement: {error}")))?;
            Ok((vec![json], 0))
        }
        "verify" => {
            let announcement: ControlPackAnnouncement =
                read(subject()?, "control-pack announcement")?;
            let pack: ModelPack = read(required(arguments, "--pack")?, "model pack")?;
            let region = required(arguments, "--region")?;
            let node = required(arguments, "--node")?;
            let now = match flag(arguments, "--now") {
                Some(text) => Timestamp::parse_rfc3339(text).ok_or_else(|| {
                    Error::invalid(format!("--now {text} is not an RFC 3339 time"))
                })?,
                None => now,
            };
            Ok(match refusal(&announcement, &pack, region, node, now) {
                None => (
                    vec![format!(
                        "announcement of {}: stands for node {node} in {region}",
                        pack.artifact_digest
                    )],
                    0,
                ),
                Some(reason) => (
                    vec![format!(
                        "announcement of {}: refused, {reason}",
                        announcement.digest
                    )],
                    INVALID,
                ),
            })
        }
        other => Err(Error::invalid(format!(
            "qip control-pack has no step {other:?}; the steps are announce and verify"
        ))),
    }
}
