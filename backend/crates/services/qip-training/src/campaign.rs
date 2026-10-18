//! §23's `TrainingCampaign`: the record of *why* a model was retrained.
//!
//! Drift-triggered retraining already ran — the deep brain's learning round
//! measures every standing model against the window it was fitted on and
//! fits again — but what it left behind was a registration. Nothing named
//! which drifted model the fit answered, how far it had drifted, which data
//! the successor was fitted on, or which model's lineage it inherited, so a
//! reviewer asking "why does this model exist" read a version number.
//!
//! A [`TrainingCampaign`] is that answer. It is built only by
//! [`TrainingCampaign::open`], which refuses a campaign no drift justifies: a
//! model that had not passed its own threshold has no business being the
//! stated reason for a retrain, and a record that admitted one would let any
//! refit read as a response to a measurement.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;

/// What a campaign is opened with. Every field is required; a blank one is a
/// refusal that names it.
#[derive(Clone, Debug, PartialEq)]
pub struct CampaignDraft {
    /// The instrument the retrained model predicts.
    pub subject: String,
    /// The registry reference of the drifted model this campaign answers.
    pub replaces: String,
    /// The drift score the model measured at, against its own threshold.
    pub drift_score: f64,
    pub drift_threshold: f64,
    /// The feature that produced the score.
    pub worst_feature: String,
    /// SHA-256 over the window the successor was fitted on.
    pub dataset_digest: String,
    /// What the successor inherits: the model it replaces and the data and
    /// code that model was fitted from, as `kind:value` entries.
    pub teacher_lineage: Vec<String>,
    /// The registry reference the retraining fit registered.
    pub successor: String,
    pub opened_at: Timestamp,
}

/// A validated retraining campaign.
#[derive(Clone, Debug, PartialEq)]
pub struct TrainingCampaign {
    id: String,
    draft: CampaignDraft,
}

fn named(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!(
            "a training campaign needs `{field}`; name what the retraining answered"
        )));
    }
    Ok(())
}

impl TrainingCampaign {
    /// Validate a draft into a campaign, refusing one its own numbers do not
    /// justify.
    pub fn open(draft: CampaignDraft) -> Result<Self> {
        named("subject", &draft.subject)?;
        named("replaces", &draft.replaces)?;
        named("worst_feature", &draft.worst_feature)?;
        named("successor", &draft.successor)?;
        if draft.replaces == draft.successor {
            return Err(Error::invalid(format!(
                "campaign successor `{}` is the model it replaces; a retrain that registered \
                 nothing new answered nothing",
                draft.successor
            )));
        }
        if !draft.drift_score.is_finite() || !draft.drift_threshold.is_finite() {
            return Err(Error::invalid(
                "a campaign's drift score and threshold must be finite numbers",
            ));
        }
        if draft.drift_score <= draft.drift_threshold {
            return Err(Error::invalid(format!(
                "drift {} has not passed the threshold {}; a model within its bound is not a \
                 reason to retrain, so no campaign opens for it",
                draft.drift_score, draft.drift_threshold
            )));
        }
        let digest = draft.dataset_digest.as_str();
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::invalid(format!(
                "dataset digest `{digest}` is not a SHA-256 hex digest; a campaign that cannot \
                 name its data cannot be reproduced"
            )));
        }
        if draft.teacher_lineage.is_empty()
            || draft.teacher_lineage.iter().any(|e| e.trim().is_empty())
        {
            return Err(Error::invalid(
                "a campaign needs a teacher lineage with no blank entry; name the model and the \
                 data it was fitted from",
            ));
        }
        let id = format!(
            "training-{}",
            &sha256_hex(
                format!(
                    "{}|{}|{}|{}",
                    draft.subject, draft.replaces, draft.successor, draft.dataset_digest
                )
                .as_bytes()
            )[..16]
        );
        Ok(Self { id, draft })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn subject(&self) -> &str {
        &self.draft.subject
    }

    pub fn replaces(&self) -> &str {
        &self.draft.replaces
    }

    pub fn successor(&self) -> &str {
        &self.draft.successor
    }

    pub fn drift_score(&self) -> f64 {
        self.draft.drift_score
    }

    pub fn drift_threshold(&self) -> f64 {
        self.draft.drift_threshold
    }

    pub fn worst_feature(&self) -> &str {
        &self.draft.worst_feature
    }

    pub fn dataset_digest(&self) -> &str {
        &self.draft.dataset_digest
    }

    pub fn teacher_lineage(&self) -> &[String] {
        &self.draft.teacher_lineage
    }

    pub const fn opened_at(&self) -> Timestamp {
        self.draft.opened_at
    }

    /// One line for the cycle line an operator reads.
    pub fn describe(&self) -> String {
        format!(
            "retrained {} -> {} (drift {:.3} > {:.3} on {}; data {})",
            self.draft.replaces,
            self.draft.successor,
            self.draft.drift_score,
            self.draft.drift_threshold,
            self.draft.worst_feature,
            &self.draft.dataset_digest[..12]
        )
    }
}
