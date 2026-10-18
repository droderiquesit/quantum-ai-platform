//! `ChallengeRequest` and `VerificationResult` (blueprint v12.0 §27,
//! R12-REASON-9032): the typed pair in which one brain asks critics to
//! attack a forecast or a model and the answer comes back.
//!
//! The failure this prevents is a verdict that disagrees with its own
//! findings. `qip-learning-engine`'s verifier society already attacks every
//! important prediction, but the record it returned was a plain struct that a
//! receiver could not check: a result reading `survived` beside a refuting
//! finding, or `refuted` naming a critic that raised no objection, decodes
//! fine and then decides whether the forecast is held out. Here the verdict
//! is cross-checked against the findings it summarises on construction and on
//! decode alike, and a result is checked against the request it answers.

use crate::validated::{Contract, Validated, listed, named, names, refuse};
use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What the critics are asked to attack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChallengeTarget {
    /// A forecast of a hypothesis about a subject.
    Prediction {
        hypothesis_id: String,
        subject: String,
    },
    /// A model offered for promotion, by registry reference.
    Model { reference: String },
}

impl ChallengeTarget {
    fn check(&self, contract: &str) -> Result<()> {
        match self {
            Self::Prediction {
                hypothesis_id,
                subject,
            } => {
                named(contract, "target.hypothesis_id", hypothesis_id)?;
                named(contract, "target.subject", subject)
            }
            Self::Model { reference } => named(contract, "target.reference", reference),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeRequestFields {
    pub schema_version: u32,
    pub request_id: String,
    pub target: ChallengeTarget,
    /// The critics asked, by name; at least one, none repeated.
    pub critics: Vec<String>,
    pub requested_by: String,
    pub requested_at: Timestamp,
}

/// A request that critics attack a target. Built only through
/// [`Validated::new`].
pub type ChallengeRequest = Validated<ChallengeRequestFields>;

impl Contract for ChallengeRequestFields {
    const NAME: &'static str = "ChallengeRequest";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id or requester, a target naming nothing, and a
    /// request that asks no critic or asks one twice.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "request_id", &self.request_id)?;
        self.target.check(c)?;
        names(c, "critics", &self.critics)?;
        named(c, "requested_by", &self.requested_by)
    }
}

/// What one critic found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CriticOutcome {
    Survived {
        examined: String,
    },
    Refuted {
        reason: String,
    },
    /// The critic could not examine the target and says what was missing.
    Abstained {
        missing: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CriticReport {
    pub critic: String,
    pub outcome: CriticOutcome,
}

/// The society's verdict over the reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChallengeVerdict {
    /// Enough critics examined the target and none refuted it.
    Survived,
    /// At least one critic refuted it; `by` names exactly those critics.
    Refuted { by: Vec<String> },
    /// None refuted it, and too few examined it to call that a survival.
    Unverified { examined: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationResultFields {
    pub schema_version: u32,
    /// The request this answers.
    pub request_id: String,
    pub target: ChallengeTarget,
    pub reports: Vec<CriticReport>,
    pub verdict: ChallengeVerdict,
    pub concluded_at: Timestamp,
}

/// The answer to a [`ChallengeRequest`]. Built only through
/// [`Validated::new`].
pub type VerificationResult = Validated<VerificationResultFields>;

impl Contract for VerificationResultFields {
    const NAME: &'static str = "VerificationResult";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a result with no report, a critic reporting twice, an empty
    /// reason, and a verdict its own reports contradict: `survived` over any
    /// refutation, `refuted` over none or naming a critic that did not
    /// refute, `unverified` over a refutation or over an examined count that
    /// is not the number of critics that examined.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "request_id", &self.request_id)?;
        self.target.check(c)?;
        listed(c, "reports", &self.reports)?;
        let mut critics = BTreeSet::new();
        let mut refuting = BTreeSet::new();
        let mut examined = 0_u32;
        for report in &self.reports {
            named(c, "reports.critic", &report.critic)?;
            if !critics.insert(report.critic.as_str()) {
                return refuse(
                    c,
                    format!(
                        "carries two reports from `{}`; a critic answers once",
                        report.critic
                    ),
                );
            }
            match &report.outcome {
                CriticOutcome::Survived { examined: what } => {
                    named(c, "reports.examined", what)?;
                    examined = examined.saturating_add(1);
                }
                CriticOutcome::Refuted { reason } => {
                    named(c, "reports.reason", reason)?;
                    examined = examined.saturating_add(1);
                    refuting.insert(report.critic.as_str());
                }
                CriticOutcome::Abstained { missing } => named(c, "reports.missing", missing)?,
            }
        }
        match &self.verdict {
            ChallengeVerdict::Survived if !refuting.is_empty() => refuse(
                c,
                format!(
                    "says the target survived while {} refuted it; a verdict that disagrees \
                     with its own findings releases what a critic held back",
                    refuting.iter().copied().collect::<Vec<_>>().join(", ")
                ),
            ),
            ChallengeVerdict::Refuted { by } => {
                names(c, "verdict.by", by)?;
                let named_by: BTreeSet<&str> = by.iter().map(String::as_str).collect();
                if named_by != refuting {
                    return refuse(
                        c,
                        format!(
                            "says it was refuted by {named_by:?} but the reports refuting it are \
                             {refuting:?}; a refutation names exactly the critics that raised one"
                        ),
                    );
                }
                Ok(())
            }
            ChallengeVerdict::Unverified { examined: stated } => {
                if !refuting.is_empty() {
                    return refuse(
                        c,
                        "is unverified over a refutation; a refuted target is refuted",
                    );
                }
                if *stated != examined {
                    return refuse(
                        c,
                        format!(
                            "states {stated} critic(s) examined it but {examined} reported a \
                             survival or a refutation"
                        ),
                    );
                }
                Ok(())
            }
            ChallengeVerdict::Survived => Ok(()),
        }
    }
}

impl VerificationResultFields {
    /// Whether this result answers `request`: same id, same target, and every
    /// critic that reported is one the request asked. A result answering a
    /// different question, or fielded by a critic nobody asked, is refused by
    /// the caller rather than filed under the request.
    pub fn answers(&self, request: &ChallengeRequest) -> Result<()> {
        let c = Self::NAME;
        if self.request_id != request.request_id {
            return refuse(
                c,
                format!(
                    "answers `{}` but the request is `{}`",
                    self.request_id, request.request_id
                ),
            );
        }
        if self.target != request.target {
            return refuse(c, "attacks a different target from the one requested");
        }
        let asked: BTreeSet<&str> = request.critics.iter().map(String::as_str).collect();
        for report in &self.reports {
            if !asked.contains(report.critic.as_str()) {
                return refuse(
                    c,
                    format!(
                        "carries a report from `{}`, whom nobody asked",
                        report.critic
                    ),
                );
            }
        }
        Ok(())
    }
}
