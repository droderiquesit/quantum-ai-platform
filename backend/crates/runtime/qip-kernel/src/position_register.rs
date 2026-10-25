//! What the Asset Brain holds for each position the desk owns (blueprint
//! §12, R12-ASSET-4066..4069), kept by the kernel at the seams where each
//! fact becomes known.
//!
//! Before this module the facts existed and nobody joined them to a
//! holding. `construct_from` wrote a target weight onto every proposal leg
//! and forgot it the moment the proposal left; `Portfolio::flag_position`
//! had no kernel caller; and `qip_financial::position_record` could build a
//! nine-field record nothing ever built. So the desk could hold a position
//! and be unable to say under which mandate, toward what target, or that it
//! could not say — the last being the dangerous one, because a holding with
//! no stated purpose reads exactly like a holding with one.
//!
//! The types here are what the kernel records; the composition lives on
//! [`crate::platform::Platform`], because the inputs live there.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

/// The mandate a held object was sized under and the weight it was sized
/// toward, as the last construction that touched it left them
/// (R12-ASSET-4066).
///
/// Written by `Platform::construct_from` and nothing else, once the
/// proposal's capital is held — a target the platform never committed to is
/// not a target. Read every cycle by the LEARN stage's mandate review, which
/// flags through `Portfolio::flag_position` every held position that has no
/// such record, or whose recorded mandate the ledger no longer holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionMandate {
    mandate: String,
    target_weight: f64,
    proposal: String,
    sized_at: Timestamp,
}

impl PositionMandate {
    pub(crate) fn new(
        mandate: impl Into<String>,
        target_weight: f64,
        proposal: impl Into<String>,
        sized_at: Timestamp,
    ) -> Self {
        Self {
            mandate: mandate.into(),
            target_weight,
            proposal: proposal.into(),
            sized_at,
        }
    }

    /// The user the ledger holds the governing mandate under — the desk's,
    /// until a mandate registry routes desk proposals to anyone else.
    pub fn mandate(&self) -> &str {
        &self.mandate
    }

    /// Statistic: the leg's target weight as a fraction of equity, as the
    /// constructor computed it. `f64` because the constructor's weights are;
    /// no money is derived from it here.
    pub fn target_weight(&self) -> f64 {
        self.target_weight
    }

    /// The proposal that sized it, so the target can be traced to the
    /// construction in the log.
    pub fn proposal(&self) -> &str {
        &self.proposal
    }

    pub fn sized_at(&self) -> Timestamp {
        self.sized_at
    }
}
