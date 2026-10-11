//! World-model contracts: `WorldModelState` (§27, R12-CONTRACT-7075),
//! `WorldBranch` (§24.9, R12-CONTRACT-6110) and `ModelDisagreement` (§27,
//! R12-CONTRACT-7076).
//!
//! The world model, its branches and the disputes between models existed as
//! service types in `qip-world-model` and `qip-prediction`, each carrying part
//! of what the blueprint lists — a branch with a parent but no probability, a
//! disagreement with views but no unresolved assumptions. A part is the
//! dangerous amount: enough to pass between brains, not enough for the
//! receiver to know what it was not told. These are the whole records.

use crate::forecast::{Calibration, OutcomeDistribution};
use crate::validated::{
    Contract, Validated, after, basis_points, each_named, listed, named, names, positive_duration,
    refuse,
};
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Where a state or branch came from. `Root` is stated, never assumed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum BranchParent {
    Root,
    Branch { id: String },
}

impl BranchParent {
    fn check(&self, contract: &str, own_id: &str) -> Result<()> {
        if let Self::Branch { id } = self {
            named(contract, "parent.id", id)?;
            if id == own_id {
                return refuse(
                    contract,
                    "names itself as its parent; a branch cannot fork from itself",
                );
            }
        }
        Ok(())
    }
}

/// One directed causal edge, `cause -> effect`, with its signed strength.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalEdge {
    pub cause: String,
    pub effect: String,
    /// Signed strength in basis points of a unit effect, within ±10,000.
    pub strength_bp: i32,
}

fn check_edges(contract: &str, field: &str, edges: &[CausalEdge]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for e in edges {
        named(contract, field, &e.cause)?;
        named(contract, field, &e.effect)?;
        if e.cause == e.effect {
            return refuse(
                contract,
                format!("has a causal edge from `{}` to itself", e.cause),
            );
        }
        basis_points(contract, field, e.strength_bp.unsigned_abs())?;
        if !seen.insert((e.cause.as_str(), e.effect.as_str())) {
            return refuse(
                contract,
                format!("states the edge `{}` -> `{}` twice", e.cause, e.effect),
            );
        }
    }
    Ok(())
}

/// One state variable's distribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateVariable {
    pub name: String,
    pub distribution: OutcomeDistribution,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldModelStateFields {
    pub schema_version: u32,
    /// The model's identity and version.
    pub model: String,
    /// What the model covers (asset class, region, domain).
    pub scope: String,
    pub horizon: Duration,
    /// At least one.
    pub state: Vec<StateVariable>,
    /// The causal graph; may be empty for a purely statistical model, which
    /// is then a statement that it claims no causal structure.
    pub causal_structure: Vec<CausalEdge>,
    pub evidence_lineage: Vec<String>,
    /// Stated assumptions; at least one. A model that assumes nothing has
    /// not been asked what it assumes.
    pub assumptions: Vec<String>,
    pub calibration: Calibration,
    pub branch_parent: BranchParent,
    pub updated_at: Timestamp,
    pub expires_at: Timestamp,
}

/// The §27 world-model state. Built only through [`Validated::new`].
pub type WorldModelState = Validated<WorldModelStateFields>;

impl Contract for WorldModelStateFields {
    const NAME: &'static str = "WorldModelState";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank model or scope, a non-positive horizon, no state
    /// variable or one named twice, any distribution or calibration defect, a
    /// self-loop or repeated causal edge, no evidence or assumption, a parent
    /// that is itself, and an expiry not after the update.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "model", &self.model)?;
        named(c, "scope", &self.scope)?;
        positive_duration(c, "horizon", self.horizon)?;
        listed(c, "state", &self.state)?;
        let mut seen = BTreeSet::new();
        for v in &self.state {
            named(c, "state.name", &v.name)?;
            if !seen.insert(v.name.as_str()) {
                return refuse(c, format!("states variable `{}` twice", v.name));
            }
            v.distribution.check(c)?;
        }
        check_edges(c, "causal_structure", &self.causal_structure)?;
        names(c, "evidence_lineage", &self.evidence_lineage)?;
        names(c, "assumptions", &self.assumptions)?;
        self.calibration.check(c)?;
        self.branch_parent.check(c, &self.model)?;
        after(
            c,
            "expires_at",
            self.expires_at,
            "updated_at",
            self.updated_at,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldBranchFields {
    pub schema_version: u32,
    pub branch_id: String,
    pub parent: BranchParent,
    /// What must be true for this future; at least one.
    pub assumptions: Vec<String>,
    /// The branch's probability, in basis points.
    pub probability_bp: u32,
    /// How the causal structure differs from the parent's; at least one —
    /// a branch identical to its parent is not a different future.
    pub causal_deltas: Vec<CausalEdge>,
    /// Evidence that would confirm or kill the branch, not yet in; may be
    /// empty once everything is in.
    pub unresolved_evidence: Vec<String>,
}

/// The §24.9 world branch: one plausible future. Built only through
/// [`Validated::new`].
pub type WorldBranch = Validated<WorldBranchFields>;

impl Contract for WorldBranchFields {
    const NAME: &'static str = "WorldBranch";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id, a parent that is itself, no assumption, a
    /// probability above one, no causal delta or a malformed one, and a
    /// blank or repeated unresolved-evidence id.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "branch_id", &self.branch_id)?;
        self.parent.check(c, &self.branch_id)?;
        names(c, "assumptions", &self.assumptions)?;
        basis_points(c, "probability_bp", self.probability_bp)?;
        listed(c, "causal_deltas", &self.causal_deltas)?;
        check_edges(c, "causal_deltas", &self.causal_deltas)?;
        each_named(c, "unresolved_evidence", &self.unresolved_evidence)
    }
}

/// Where an arbitration between disagreeing models stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ArbitrationStatus {
    Open,
    /// Further evidence was requested; the request's id.
    EvidenceRequested {
        request_id: String,
    },
    /// One side was preferred, with the reason.
    Resolved {
        preferred: String,
        reason: String,
    },
    /// Both views are kept live as separate branches.
    Branched,
}

/// One side of a dispute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisputingView {
    /// The model or hypothesis id.
    pub id: String,
    pub supporting_evidence: Vec<String>,
    pub calibration: Calibration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDisagreementFields {
    pub schema_version: u32,
    pub disagreement_id: String,
    /// At least two.
    pub views: Vec<DisputingView>,
    /// The propositions the views disagree on; at least one.
    pub disputed_propositions: Vec<String>,
    /// Assumptions neither side has tested; may be empty.
    pub unresolved_assumptions: Vec<String>,
    pub status: ArbitrationStatus,
    pub raised_at: Timestamp,
}

/// The §27 model disagreement. Built only through [`Validated::new`].
pub type ModelDisagreement = Validated<ModelDisagreementFields>;

impl Contract for ModelDisagreementFields {
    const NAME: &'static str = "ModelDisagreement";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses fewer than two views or a view listed twice, a view with no
    /// supporting evidence (a side that cites nothing is an opinion), no
    /// disputed proposition, and a resolution that prefers a view that is not
    /// party to the dispute.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "disagreement_id", &self.disagreement_id)?;
        if self.views.len() < 2 {
            return refuse(
                c,
                "needs at least two views; one model does not disagree with itself",
            );
        }
        let mut ids = BTreeSet::new();
        for v in &self.views {
            named(c, "views.id", &v.id)?;
            if !ids.insert(v.id.as_str()) {
                return refuse(c, format!("lists view `{}` twice", v.id));
            }
            names(c, "views.supporting_evidence", &v.supporting_evidence)?;
            v.calibration.check(c)?;
        }
        names(c, "disputed_propositions", &self.disputed_propositions)?;
        each_named(c, "unresolved_assumptions", &self.unresolved_assumptions)?;
        match &self.status {
            ArbitrationStatus::EvidenceRequested { request_id } => {
                named(c, "status.request_id", request_id)
            }
            ArbitrationStatus::Resolved { preferred, reason } => {
                named(c, "status.reason", reason)?;
                if !ids.contains(preferred.as_str()) {
                    return refuse(
                        c,
                        format!(
                            "resolves in favour of `{preferred}`, which is not one of its views"
                        ),
                    );
                }
                Ok(())
            }
            ArbitrationStatus::Open | ArbitrationStatus::Branched => Ok(()),
        }
    }
}
