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

use qip_core::{Decimal, Duration, Error, Result, Timestamp};
use qip_financial::cashflow::Commitment;
use qip_financial::position_record::Declared;
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

/// A held position's hedge relationships, pending corporate actions and
/// capital commitments, each either present or declared not to apply with
/// the reason (R12-ASSET-4068).
///
/// [`Declared`] rather than three collections, because "no hedge" and "no
/// commitment" are real states that must be *stated*: an empty list nobody
/// filled in and a list the platform looked for and found empty read
/// identically, and only the second is an answer. Built by
/// `Platform::position_obligations` from the declared hedge policies, the
/// corporate actions awaiting their ex-date and the commitment book — the
/// records the platform already holds, keyed by the same object id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionObligations {
    hedges: Declared<Vec<String>>,
    corporate_actions: Declared<Vec<String>>,
    commitments: Declared<Commitment>,
}

impl PositionObligations {
    pub(crate) fn new(
        hedges: Declared<Vec<String>>,
        corporate_actions: Declared<Vec<String>>,
        commitments: Declared<Commitment>,
    ) -> Self {
        Self {
            hedges,
            corporate_actions,
            commitments,
        }
    }

    /// Every declared hedge policy relating to the position, in words.
    pub fn hedges(&self) -> &Declared<Vec<String>> {
        &self.hedges
    }

    /// The idempotency keys of the corporate actions announced on the
    /// instrument and not yet past their ex-date on the tape.
    pub fn corporate_actions(&self) -> &Declared<Vec<String>> {
        &self.corporate_actions
    }

    /// The capital commitment the book holds on the instrument.
    pub fn commitments(&self) -> &Declared<Commitment> {
        &self.commitments
    }
}

/// A held position's valuation confidence, investment horizon and expected
/// liquidity, in one record that refuses to exist without all three
/// (R12-ASSET-4067).
///
/// Built only by `Platform::position_holding`, which reads each from the
/// figure the platform already decides with; see that method for where each
/// comes from and what refuses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionHolding {
    valuation_confidence: Decimal,
    hypothesis: String,
    horizon: Duration,
    expected_exit_days: f64,
}

impl PositionHolding {
    /// Refuses a confidence outside `(0, 1]`, a horizon that is not
    /// positive, and an expected exit that is not a finite, non-negative
    /// number of days — each a figure no reader could act on, which a record
    /// carrying it would present as though one could.
    pub(crate) fn new(
        valuation_confidence: Decimal,
        hypothesis: impl Into<String>,
        horizon: Duration,
        expected_exit_days: f64,
    ) -> Result<Self> {
        let hypothesis = hypothesis.into();
        if !valuation_confidence.is_positive() || valuation_confidence > Decimal::ONE {
            return Err(Error::invalid(format!(
                "a valuation confidence of {valuation_confidence} is outside (0, 1]; the record \
                 carries the sizing confidence as computed, so correct its source"
            )));
        }
        if horizon.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "hypothesis {hypothesis} states a horizon that is not positive; a position held \
                 on it has no investment horizon to record"
            )));
        }
        if !expected_exit_days.is_finite() || expected_exit_days < 0.0 {
            return Err(Error::invalid(format!(
                "an expected exit of {expected_exit_days} days is not a number of days; correct \
                 the liquidity record"
            )));
        }
        Ok(Self {
            valuation_confidence,
            hypothesis,
            horizon,
            expected_exit_days,
        })
    }

    /// The fraction in `(0, 1]` sizing narrows this position by.
    pub fn valuation_confidence(&self) -> Decimal {
        self.valuation_confidence
    }

    /// The hypothesis the position was last sized on.
    pub fn hypothesis(&self) -> &str {
        &self.hypothesis
    }

    /// That hypothesis's own horizon.
    pub fn horizon(&self) -> Duration {
        self.horizon
    }

    /// Statistic: days to exit, never money.
    pub fn expected_exit_days(&self) -> f64 {
        self.expected_exit_days
    }
}
