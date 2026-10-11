//! `ControlPackAnnouncement` (blueprint v12.0 §27, R12-CONTRACT-7091): the
//! fabric message that tells regions and nodes a new control artifact —
//! model, strategy, policy, risk pack — exists and when to activate it.
//!
//! The announcement is not the artifact. What it carries is what a receiver
//! needs to decide whether to fetch and activate: an immutable locator and
//! digest (so the bytes fetched are the bytes signed), the signer and its
//! attestation, exactly who it is for, the window it may activate in, the
//! artifact it rolls back to, and how many acknowledgements make it live.
//! An announcement that could be activated by nobody, or by everybody, or
//! that names a mutable locator, is refused.

use crate::validated::{Contract, Validated, after, digest, named, names, refuse};
use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    ModelPack,
    StrategyPack,
    PolicyPack,
    RiskPack,
    CapitalPack,
}

/// The artifact this one replaces on rollback. `None` is stated for the
/// first artifact of its kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RollbackParent {
    None,
    Artifact { digest: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlPackAnnouncementFields {
    pub schema_version: u32,
    pub kind: ArtifactKind,
    /// A content-addressed locator; it must embed `digest`, so the locator
    /// cannot be repointed at other bytes while the announcement stands.
    pub uri: String,
    pub digest: String,
    pub version: u64,
    pub signer: String,
    /// Digest of the attestation (signature bundle) over `digest`.
    pub attestation: String,
    /// Regions the artifact is for; at least one.
    pub target_regions: Vec<String>,
    /// Nodes within those regions; at least one.
    pub target_nodes: Vec<String>,
    pub activate_from: Timestamp,
    pub activate_until: Timestamp,
    /// After this the artifact must not run, activated or not.
    pub expires_at: Timestamp,
    pub rollback_parent: RollbackParent,
    /// Acknowledgements required before the artifact counts as live; at
    /// least one and no more than there are target nodes.
    pub required_acks: u32,
}

/// The §27 control-pack announcement. Built only through [`Validated::new`].
pub type ControlPackAnnouncement = Validated<ControlPackAnnouncementFields>;

impl Contract for ControlPackAnnouncementFields {
    const NAME: &'static str = "ControlPackAnnouncement";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a malformed digest or attestation, a locator that does not
    /// embed the digest, a version of zero, a blank signer, no target region
    /// or node, an empty activation window, an expiry before the window
    /// closes, a rollback to itself, and an acknowledgement quorum of zero or
    /// larger than the node set (which no rollout could ever meet).
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        digest(c, "digest", &self.digest)?;
        named(c, "uri", &self.uri)?;
        if !self.uri.contains(&self.digest) {
            return refuse(
                c,
                "names a locator that does not embed its digest; announce a content-addressed URI",
            );
        }
        if self.version == 0 {
            return refuse(c, "has version 0; versions start at 1");
        }
        named(c, "signer", &self.signer)?;
        digest(c, "attestation", &self.attestation)?;
        names(c, "target_regions", &self.target_regions)?;
        names(c, "target_nodes", &self.target_nodes)?;
        after(
            c,
            "activate_until",
            self.activate_until,
            "activate_from",
            self.activate_from,
        )?;
        if self.expires_at < self.activate_until {
            return refuse(
                c,
                "expires before its activation window closes; the window would admit an expired \
                 artifact",
            );
        }
        if let RollbackParent::Artifact { digest: parent } = &self.rollback_parent {
            digest(c, "rollback_parent.digest", parent)?;
            if parent == &self.digest {
                return refuse(c, "rolls back to itself");
            }
        }
        let nodes = u32::try_from(self.target_nodes.len()).unwrap_or(u32::MAX);
        if self.required_acks == 0 || self.required_acks > nodes {
            return refuse(
                c,
                format!(
                    "requires {} acknowledgements from {nodes} target nodes; state between 1 and \
                     the node count",
                    self.required_acks
                ),
            );
        }
        Ok(())
    }
}
