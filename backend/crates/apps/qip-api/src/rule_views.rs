//! The recalibration surface: what the platform has proposed about its own
//! risk rules, and the signature route that turns a proposal into an
//! artefact (blueprint §12.3, first row; ADR 0061).
//!
//! Two things this module holds the line on. The body of a signature is a
//! rationale and nothing else — no approver, because the approver is the
//! authenticated session, and no bound, because the bound is the platform's
//! own proposal and a caller who could name one would be requesting a
//! loosening rather than approving the evidence for one. And what the second
//! signature produces is a *file*: the running set with one bound replaced,
//! for a deployment to commit and mount. The process that served the request
//! keeps the limits it booted with.

use qip_kernel::platform::Platform;
use qip_kernel::rule_review::RecalibrationProposal;
use serde::Serialize;

/// What `GET /risk/recalibrations` answers.
#[derive(Clone, Debug, Serialize)]
pub struct RecalibrationsView {
    /// The limit set this process runs under, by name — so a reader can tell
    /// which file a signed artefact would replace.
    pub limits: String,
    /// Proposals standing open, oldest rule first. A signature lands on one
    /// of these and nothing else.
    pub open: Vec<RecalibrationProposal>,
    /// Every proposal record this kernel has written, oldest first —
    /// proposed, withdrawn and enacted — bounded by what the log retains.
    pub history: Vec<RecalibrationProposal>,
}

/// The view, or the reason the log could not be read.
pub fn recalibrations(platform: &Platform) -> Result<RecalibrationsView, String> {
    let history = platform
        .recalibration_history()
        .map_err(|error| error.message().to_string())?;
    Ok(RecalibrationsView {
        limits: platform.limits().name.clone(),
        open: platform.open_recalibrations().values().cloned().collect(),
        history,
    })
}

// --- the signature ------------------------------------------------------------

/// The body of a recalibration signature: a rationale and nothing else.
///
/// Cloned from `PromotionApprovalRequest`, and for the same reason it has no
/// approver field: the approver is the authenticated session's subject, and a
/// body that could name one would turn an approval into a claim to have been
/// approved. It has no bound field either — the bound is the platform's
/// proposal, and this route exists to sign evidence, not to request a
/// loosening. An unknown key is refused rather than ignored, so a caller who
/// believed they were naming either is told they were not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecalibrationApprovalRequest {
    pub rationale: String,
}

impl RecalibrationApprovalRequest {
    const FIELDS: [&'static str; 1] = ["rationale"];

    /// The longest rationale the record will hold; the same bound the
    /// promotion signature keeps, for the same reason — it reaches the
    /// hash-chained event log, and an unbounded field is an unbounded record
    /// with no erase path.
    const MAX_RATIONALE: usize = 512;

    pub fn parse(body: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            "the body is not JSON; send {\"rationale\": \"<why this bound should be loosened as \
             proposed>\"}"
                .to_string()
        })?;
        let Some(object) = value.as_object() else {
            return Err("the body must be a JSON object with `rationale`".to_string());
        };
        if let Some(position) = object
            .keys()
            .position(|key| !Self::FIELDS.contains(&key.as_str()))
        {
            // Named by position rather than quoted, the discipline every
            // refusal on this API keeps: a refusal that echoes what a caller
            // sent publishes whatever they sent by mistake.
            return Err(format!(
                "the body's key at position {} is not one this route reads; it takes \
                 `rationale` only. In particular neither the approver nor the bound can be sent: \
                 the approver is taken from the authenticated session, because an approval a \
                 caller can name is not an approval, and the bound is the platform's own \
                 proposal, because a bound a caller can name is a request to loosen a control \
                 rather than a signature on the evidence for one",
                position + 1
            ));
        }
        let rationale = object
            .get("rationale")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "the body needs a non-empty `rationale` string".to_string())?;
        if rationale.len() > Self::MAX_RATIONALE {
            return Err(format!(
                "the rationale is {} bytes and the record holds at most {}",
                rationale.len(),
                Self::MAX_RATIONALE
            ));
        }
        Ok(Self { rationale })
    }
}
