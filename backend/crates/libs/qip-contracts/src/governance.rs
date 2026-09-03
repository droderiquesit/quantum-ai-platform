//! The vocabulary of the cross-cutting governance and safety plane.
//!
//! The six controls that apply to every subsystem rather than to one of them:
//! bitemporal truth, data licensing and entitlements, model risk and
//! explainability, human capital approvals, signed artifacts and provenance,
//! and kill switches with incident response.
//!
//! The types live here so that a subsystem physically cannot produce a value
//! that skips a control — an unlicensed dataset has no way to become a
//! [`Entitlement::Granted`], and an unsigned artifact has no constructor that
//! yields a [`Provenance`] a deployment will accept.

use qip_core::error::{Error, Result};
use qip_core::{Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};

/// The six controls, named so a test can assert each one is enforced
/// somewhere rather than merely described in a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Control {
    BitemporalTruth,
    LicensingAndEntitlements,
    ModelRiskAndExplainability,
    HumanCapitalApproval,
    SignedArtifactsAndProvenance,
    KillSwitchAndIncidentResponse,
}

impl Control {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::BitemporalTruth => "point_in_time_truth",
            Self::LicensingAndEntitlements => "licensing_and_entitlements",
            Self::ModelRiskAndExplainability => "model_risk_and_explainability",
            Self::HumanCapitalApproval => "human_capital_approval",
            Self::SignedArtifactsAndProvenance => "signed_artifacts_and_provenance",
            Self::KillSwitchAndIncidentResponse => "kill_switch_and_incident_response",
        }
    }

    pub const fn all() -> [Self; 6] {
        [
            Self::BitemporalTruth,
            Self::LicensingAndEntitlements,
            Self::ModelRiskAndExplainability,
            Self::HumanCapitalApproval,
            Self::SignedArtifactsAndProvenance,
            Self::KillSwitchAndIncidentResponse,
        ]
    }
}

/// What a dataset may be used for.
///
/// Not a boolean. A feed licensed for research and not for trading is the
/// common case, and collapsing the two is how a licence gets breached by a
/// backtest that was promoted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Usage {
    /// Look at it internally.
    Research,
    /// Derive features and train on it.
    Derive,
    /// Base a live order on it.
    Trade,
    /// Show it to a client or publish a number derived from it.
    Redistribute,
}

impl Usage {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Derive => "derive",
            Self::Trade => "trade",
            Self::Redistribute => "redistribute",
        }
    }
}

/// Whether a use of a dataset is permitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Entitlement {
    Granted {
        dataset: String,
        usage: Usage,
        expires_at: Timestamp,
    },
    Denied {
        dataset: String,
        usage: Usage,
        reason: String,
    },
}

impl Entitlement {
    pub fn is_granted(&self, now: Timestamp) -> bool {
        match self {
            Self::Granted { expires_at, .. } => now < *expires_at,
            Self::Denied { .. } => false,
        }
    }

    /// Whether the grant is active at `now`, given a caller-supplied instant
    /// the agreement is known to have taken effect.
    ///
    /// [`Entitlement::is_granted`] only ever checked the upper bound —
    /// `expires_at` — so a check made about an instant *before* the licence
    /// existed passed as granted, the same gap fixed for
    /// `qip_data_finder::SourceLicense` with an opt-in `effective_from`
    /// field. `Entitlement::Granted`'s fields are public and every crate that
    /// builds one does so as a bare struct literal (`qip-compliance`,
    /// `qip-mesh`, `qip-data-finder`, `qip-acceptance` — grep
    /// `Entitlement::Granted` across the workspace), so retrofitting the same
    /// field here would not compile at any of those call sites; Rust has no
    /// default-value syntax for an enum struct variant's literal. A caller
    /// that knows when its agreement took effect passes it here instead of
    /// `is_granted`, so a check about an instant before the agreement existed
    /// is refused rather than silently granted.
    pub fn is_granted_after(&self, now: Timestamp, effective_from: Timestamp) -> bool {
        self.is_granted(now) && now >= effective_from
    }

    pub fn dataset(&self) -> &str {
        match self {
            Self::Granted { dataset, .. } | Self::Denied { dataset, .. } => dataset,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Granted { dataset, usage, .. } => {
                format!("{dataset} licensed for {}", usage.as_str())
            }
            Self::Denied {
                dataset,
                usage,
                reason,
            } => format!("{dataset} not licensed for {}: {reason}", usage.as_str()),
        }
    }
}

/// An artifact's identity, signature and chain of custody.
///
/// The only constructor computes the digest from the bytes, so a provenance
/// whose digest does not match its content cannot be built in the first place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    digest: String,
    signer: String,
    signature: String,
    built_at: Timestamp,
    /// Digests of the inputs this artifact was produced from — datasets,
    /// parent models, feature definitions.
    inputs: Vec<String>,
}

impl Provenance {
    pub fn sign(
        content: &[u8],
        signer: impl Into<String>,
        signature: impl Into<String>,
        built_at: Timestamp,
        inputs: Vec<String>,
    ) -> Result<Self> {
        let signer = signer.into();
        if signer.trim().is_empty() {
            return Err(Error::denied("an artifact must name its signer"));
        }
        Ok(Self {
            digest: sha256_hex(content),
            signer,
            signature: signature.into(),
            built_at,
            inputs,
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn signer(&self) -> &str {
        &self.signer
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }

    pub fn built_at(&self) -> Timestamp {
        self.built_at
    }

    pub fn inputs(&self) -> &[String] {
        &self.inputs
    }

    /// Whether these bytes are the ones that were signed.
    pub fn matches(&self, content: &[u8]) -> bool {
        sha256_hex(content) == self.digest
    }

    /// A reference of the form `sha256:abcd…`, short enough for a log line.
    pub fn reference(&self) -> String {
        format!("sha256:{}", &self.digest[..16.min(self.digest.len())])
    }
}

/// A named human's approval of something consequential.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    pub subject: String,
    pub approver: String,
    /// A second name where the action requires two. `None` is not "not
    /// required" — the caller decides that; this only records what happened.
    pub second_approver: Option<String>,
    pub at: Timestamp,
    pub rationale: String,
}

impl Approval {
    pub fn new(
        subject: impl Into<String>,
        approver: impl Into<String>,
        at: Timestamp,
        rationale: impl Into<String>,
    ) -> Result<Self> {
        let approver = approver.into();
        let rationale = rationale.into();
        if approver.trim().is_empty() {
            return Err(Error::denied("an approval must name its approver"));
        }
        if rationale.trim().len() < 10 {
            return Err(Error::invalid(
                "an approval must state a rationale somebody can review later",
            ));
        }
        Ok(Self {
            subject: subject.into(),
            approver,
            second_approver: None,
            at,
            rationale,
        })
    }

    /// Add a second approver, refusing a self-approval.
    pub fn countersigned_by(mut self, approver: impl Into<String>) -> Result<Self> {
        let second = approver.into();
        if second == self.approver {
            return Err(Error::denied(
                "a second approver who is the first approver is not a second approver",
            ));
        }
        self.second_approver = Some(second);
        Ok(self)
    }

    pub fn is_dual(&self) -> bool {
        self.second_approver.is_some()
    }
}

/// How severe an incident is, and therefore what it stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// Recorded, nothing stops.
    Observation,
    /// One strategy or instrument stops.
    Scoped,
    /// One cell stops.
    Cell,
    /// Everything stops.
    Global,
}

impl Severity {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Scoped => "scoped",
            Self::Cell => "cell",
            Self::Global => "global",
        }
    }

    pub const fn halts_something(&self) -> bool {
        !matches!(self, Self::Observation)
    }
}
