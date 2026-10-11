//! `KnowledgeDelta` (blueprint v12.0 §27, R12-CONTRACT-7088): one versioned
//! change to shared knowledge — ontology, world model or memory — as one
//! brain or region proposes it to the rest.
//!
//! `knowledge_pack.rs` holds a different thing (the compact artifact derived
//! knowledge is stored in). A delta is the *change*, and the failure it
//! exists to prevent is a lost update: two regions each editing version 7
//! and the second silently overwriting the first. So a delta names the
//! version it applies to and the one it produces, states the conflicts it
//! knows about, and says how a receiver must merge it; a delta that skips a
//! version or claims a merge policy its own conflicts rule out is refused.

use crate::validated::{
    Contract, Validated, basis_points, each_named, listed, named, names, refuse,
};
use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};

/// Which body of knowledge the delta changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeTarget {
    Ontology,
    WorldModel,
    Memory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChangeOp {
    Assert { key: String, value: String },
    Retract { key: String },
}

/// How a receiver must merge the delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergePolicy {
    /// Apply only if the receiver is at exactly `base_version`; otherwise
    /// refuse and fetch. Never applied over a conflict.
    FastForwardOnly,
    /// Hold for a human or arbitration before applying.
    ReviewRequired,
    /// Apply the higher-confidence side of each conflicting key.
    PreferHigherConfidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeDeltaFields {
    pub schema_version: u32,
    pub delta_id: String,
    pub target: KnowledgeTarget,
    /// The version this delta applies on top of.
    pub base_version: u64,
    /// The version it produces; exactly `base_version + 1`.
    pub new_version: u64,
    /// At least one.
    pub changes: Vec<ChangeOp>,
    /// Evidence or episode ids the change rests on; at least one.
    pub provenance: Vec<String>,
    pub confidence_bp: u32,
    /// Ids of deltas this one is known to conflict with; may be empty.
    pub conflicts: Vec<String>,
    pub originating_brain: String,
    pub originating_region: String,
    pub merge_policy: MergePolicy,
    pub created_at: Timestamp,
}

/// The §27 knowledge delta. Built only through [`Validated::new`].
pub type KnowledgeDelta = Validated<KnowledgeDeltaFields>;

impl Contract for KnowledgeDeltaFields {
    const NAME: &'static str = "KnowledgeDelta";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a version step other than one, no change, a blank key, no
    /// provenance, a confidence above one, a blank origin, and a
    /// fast-forward-only delta that lists a known conflict — fast-forward is
    /// the policy that never merges over one, so the two cannot both be true.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "delta_id", &self.delta_id)?;
        if self.base_version.checked_add(1) != Some(self.new_version) {
            return refuse(
                c,
                format!(
                    "goes from version {} to {}; a delta advances exactly one version, so a gap \
                     or a rewind is refused",
                    self.base_version, self.new_version
                ),
            );
        }
        listed(c, "changes", &self.changes)?;
        for change in &self.changes {
            match change {
                ChangeOp::Assert { key, value } => {
                    named(c, "changes.key", key)?;
                    named(c, "changes.value", value)?;
                }
                ChangeOp::Retract { key } => named(c, "changes.key", key)?,
            }
        }
        names(c, "provenance", &self.provenance)?;
        basis_points(c, "confidence_bp", self.confidence_bp)?;
        each_named(c, "conflicts", &self.conflicts)?;
        named(c, "originating_brain", &self.originating_brain)?;
        named(c, "originating_region", &self.originating_region)?;
        if self.merge_policy == MergePolicy::FastForwardOnly && !self.conflicts.is_empty() {
            return refuse(
                c,
                "is fast-forward-only and lists conflicts; choose review or a resolution policy",
            );
        }
        Ok(())
    }
}
