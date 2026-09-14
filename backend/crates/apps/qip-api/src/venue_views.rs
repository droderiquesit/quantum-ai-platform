//! The venue-withdrawal surface: which venues the platform stopped using on
//! feasibility evidence, and the signature route that puts one back
//! (blueprint §12.3's fourth row; ADR 0062).
//!
//! ADR 0062 shipped the withdrawal, the record and `Platform::reinstate_venue`
//! and said plainly that no HTTP route exposed the last of them. That is a
//! control whose *recovery* needed a person with direct access to the kernel:
//! the platform can stop trading its only venue on its own evidence — the
//! record says that is the intended, fail-closed answer to ten off-grid
//! orders in a row — and until this module the desk's way back was to restart
//! the process on a log it had edited. A safety control that cannot be
//! recovered from through the platform's own audited path invites recovery
//! through one that is not audited at all.
//!
//! Two things this module holds the line on, both cloned from the promotion
//! and recalibration signatures because they are the same kind of act. The
//! body of a signature is a rationale and nothing else — no approver, because
//! the approver is the authenticated session, and an approval a caller can
//! name is not an approval. And the subject is a venue *the platform
//! withdrew*: the kernel refuses one that is not withdrawn as not found, so
//! no signature can name a venue into existence. Reinstatement removes a name
//! from a subtractive set and can never add one; what a venue goes back to
//! being permitted to do is whatever `QIP_VENUES`, the arbitrage policy's
//! venue map and the grant already said.

use qip_kernel::platform::Platform;
use qip_kernel::venue_review::{VenueReinstatementEntry, VenueWithdrawal};
use serde::Serialize;

/// What `GET /venues/withdrawals` answers: one row per withdrawn venue.
#[derive(Clone, Debug, Serialize)]
pub struct WithdrawnVenuesView {
    /// The venues withdrawn right now, in name order. Empty is the normal
    /// answer and is an observed zero rather than a gap: the set is read
    /// from the platform this process assembled, which resumed it from the
    /// log at start-up.
    pub withdrawn: Vec<WithdrawnVenueRow>,
    /// How many withdrawal records the log holds in all, including venues
    /// since reinstated — so a reader can tell "never withdrawn anything"
    /// from "withdrew one and put it back".
    pub withdrawals_recorded: usize,
    /// The path a signature is posted to, with `:venue` as the route table
    /// spells it. Served rather than left to a runbook because the operator
    /// reading this list is the one about to call it.
    ///
    /// A `String` rather than a `&'static str` because it is *composed* — the
    /// version prefix the router strips and the pattern the router matches,
    /// both read from `crate::routes` — and this workspace has no const string
    /// concatenation without a dependency. The cost of the allocation is one
    /// per call of a route an operator reads; the cost of the third hand-typed
    /// copy it replaces was a 404 in the middle of a recovery.
    pub reinstatement_path: String,
}

/// One withdrawn venue and where its reinstatement stands.
#[derive(Clone, Debug, Serialize)]
pub struct WithdrawnVenueRow {
    pub venue: String,
    /// The withdrawal record, with the evidence the review made it on: the
    /// dominating constraint, the count, the sample and the seams that
    /// contributed. `None` only if the log no longer holds the record the
    /// set was resumed from, which is a fact about retention and is reported
    /// rather than filled in with a guess.
    pub withdrawal: Option<VenueWithdrawal>,
    /// Whether a first signature is standing and waiting for a second
    /// person.
    ///
    /// A boolean and not a name. Who signed is on the event log, at the
    /// authority that reads the log; this list is served to a viewer, and a
    /// viewer credential's whole authority is reading what the platform
    /// decided — not which operator decided it. The same split `GET
    /// /registrations` and `/registrations/slots` already make.
    pub awaiting_countersignature: bool,
}

/// The view, or the reason the log could not be read.
pub fn withdrawals(platform: &Platform) -> Result<WithdrawnVenuesView, String> {
    let records = platform
        .venue_withdrawals()
        .map_err(|error| error.message().to_string())?;
    let withdrawn = platform
        .withdrawn_venues()
        .iter()
        .map(|venue| WithdrawnVenueRow {
            venue: venue.clone(),
            // The newest record for this venue: a venue withdrawn, reinstated
            // and withdrawn again is standing on the second withdrawal's
            // evidence, and showing the first would describe a cluster that
            // has already been answered.
            withdrawal: records
                .iter()
                .rev()
                .find(|record| &record.venue == venue)
                .cloned(),
            awaiting_countersignature: platform.pending_venue_reinstatement(venue).is_some(),
        })
        .collect();
    Ok(WithdrawnVenuesView {
        withdrawn,
        withdrawals_recorded: records.len(),
        reinstatement_path: reinstatement_path(),
    })
}

/// The full path a reinstatement is signed at, composed from `crate::routes`
/// rather than written out here.
///
/// This module used to declare its own
/// `REINSTATEMENT_PATTERN = "/api/v1/venues/:venue/reinstatements"` under a
/// comment claiming it was the one constant the route table read. It was not,
/// and the claim is gone rather than restated: see
/// [`crate::routes::REINSTATEMENT_PATTERN`] for which copies can share a
/// constant, which cannot and why, and for the test that holds all of them
/// together by driving this string through the router instead of comparing it
/// to another copy of itself.
pub fn reinstatement_path() -> String {
    format!(
        "{}{}",
        crate::routes::VERSION_PREFIX,
        crate::routes::REINSTATEMENT_PATTERN
    )
}

/// What a reinstatement signature answers with: the entry the kernel
/// journaled, and nothing this layer invented.
pub fn rendered(entry: &VenueReinstatementEntry) -> Result<String, String> {
    serde_json::to_string(entry).map_err(|error| error.to_string())
}

// --- the signature ------------------------------------------------------------

/// The body of a reinstatement signature: a rationale and nothing else.
///
/// Cloned from `PromotionApprovalRequest`, and for the same reason it has no
/// approver field: the approver is the authenticated session's subject, and a
/// body that could name one would turn an approval into a claim to have been
/// approved. It has no venue field either — the venue is the path segment,
/// and the kernel refuses one it did not withdraw. An unknown key is refused
/// rather than ignored, so a caller who believed they were naming an approver
/// is told they were not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueReinstatementRequest {
    pub rationale: String,
}

impl VenueReinstatementRequest {
    const FIELDS: [&'static str; 1] = ["rationale"];

    /// The longest rationale the record will hold; the same bound the
    /// promotion and recalibration signatures keep, for the same reason — it
    /// reaches the hash-chained event log, and an unbounded field is an
    /// unbounded record with no erase path.
    const MAX_RATIONALE: usize = 512;

    pub fn parse(body: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            "the body is not JSON; send {\"rationale\": \"<why this venue should be traded at \
             again>\"}"
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
                 `rationale` only. In particular the approver cannot be sent: it is taken from \
                 the authenticated session, because an approval a caller can name is not an \
                 approval; nor can the venue, which is the path segment and must be one the \
                 platform itself withdrew",
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
        // The kernel's `Approval::new` holds a floor of its own on the
        // rationale. This is the ceiling; the floor is deliberately not
        // repeated here, because one authority on what a reviewable rationale
        // is beats two that can drift apart.
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
