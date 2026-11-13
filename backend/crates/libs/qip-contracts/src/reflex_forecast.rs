//! `ReflexForecast` (ADR 0133 decision 2, ADR 0506; R12-WORLD-3084): the
//! microsecond-to-millisecond row of the §9.7 forecast lattice, issued by the
//! reflex cell itself and promoted to nothing but the local reflex.
//!
//! The central lattice (`qip-world-model::lattice`) refuses to issue this row
//! from the central lane and refuses to admit a model there aimed at anything
//! but `LocalReflexOnly`. Both of those are checks on the *central* side. This
//! contract is the reflex side: what a cell's microstructure models say about
//! the next microseconds — fill probability, queue movement, adverse selection
//! and executable edge — in a shape that both lanes link without the node
//! linking the world model.
//!
//! Three refusals carry the row's meaning, and each closes a failure:
//!
//! * **The horizon is under a second.** A "microsecond forecast" stated over
//!   two seconds belongs to the seconds-minutes row, whose promotion target is
//!   different; carrying it here would launder it past that row's checks.
//!   The bound is [`MAX_REFLEX_HORIZON_US`], the same edge
//!   `LatticeHorizon::of` draws.
//! * **The promotion target is a type with one value.** [`ReflexPromotion`]
//!   has a single variant, so a record claiming any other target does not
//!   decode — it is not a field a producer can set wrongly and a consumer
//!   must remember to check. A label checked at admission is a label; a type
//!   that cannot name the alternative is the gate.
//! * **The distribution matches the target.** Fill probability and adverse
//!   selection are events — they happen within the horizon or they do not —
//!   and are stated as a two-outcome categorical over [`OCCURRED`] and
//!   [`NOT_OCCURRED`]. Queue movement and executable edge are magnitudes and
//!   are stated as quantiles. A fill probability stated as quantiles cannot
//!   be scored by the rule that scores fill probabilities, so it is refused
//!   rather than guessed at.

use crate::forecast::{Calibration, OutcomeDistribution};
use crate::validated::{Contract, Validated, after, named, refuse};
use crate::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// The longest horizon this row states, in microseconds: just under one
/// second, the boundary `LatticeHorizon::of` puts between the microsecond row
/// and the seconds-minutes row.
pub const MAX_REFLEX_HORIZON_US: u64 = 999_999;

/// The outcome an event target names when the event happened in the horizon.
pub const OCCURRED: &str = "occurred";
/// The outcome an event target names when it did not.
pub const NOT_OCCURRED: &str = "not_occurred";

/// What a reflex forecast is about: the four outputs §9.7 names for the
/// microsecond row's reflex and microstructure models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflexTarget {
    /// Whether a resting order is filled within the horizon. An event.
    FillProbability,
    /// How far the order's queue position advances within the horizon, in
    /// the venue's quantity units. A magnitude.
    QueueMovement,
    /// Whether the price moves against a fill within the horizon after it.
    /// An event.
    AdverseSelection,
    /// The edge left after costs if the order executes now, in basis points.
    /// A magnitude.
    ExecutableEdge,
}

impl ReflexTarget {
    pub const ALL: [Self; 4] = [
        Self::FillProbability,
        Self::QueueMovement,
        Self::AdverseSelection,
        Self::ExecutableEdge,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FillProbability => "fill_probability",
            Self::QueueMovement => "queue_movement",
            Self::AdverseSelection => "adverse_selection",
            Self::ExecutableEdge => "executable_edge",
        }
    }

    /// Whether the target is an event stated as a categorical over
    /// [`OCCURRED`] and [`NOT_OCCURRED`]; otherwise it is a magnitude stated
    /// as quantiles.
    pub const fn is_event(self) -> bool {
        matches!(self, Self::FillProbability | Self::AdverseSelection)
    }
}

/// Where a reflex forecast may be promoted. One variant on purpose: the
/// microsecond row's §9.7 promotion target is the local reflex and nothing
/// else, and a type with no second value cannot carry a second answer. A
/// record naming any other target is refused at decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflexPromotion {
    LocalReflexOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflexForecastFields {
    pub schema_version: u32,
    pub forecast_id: String,
    /// The cell that issued it. A reflex forecast is the node's own; no other
    /// party issues one.
    pub cell: String,
    pub venue: VenueId,
    pub instrument: String,
    pub target: ReflexTarget,
    /// How far ahead the forecast speaks, in microseconds; 1 to
    /// [`MAX_REFLEX_HORIZON_US`].
    pub horizon_us: u64,
    /// On the wire as integer nanoseconds, not RFC 3339: `Timestamp`'s text
    /// form keeps milliseconds, and a 500 µs forecast issued and expiring in
    /// one millisecond would decode with its expiry equal to its issue and be
    /// refused. `Timestamp` already reads the integer form.
    #[serde(serialize_with = "nanos")]
    pub issued_at: Timestamp,
    /// After this instant the forecast is not read. No later than
    /// `issued_at + horizon_us`: a forecast read after its horizon has passed
    /// is a forecast of the past. Integer nanoseconds on the wire, as
    /// `issued_at`.
    #[serde(serialize_with = "nanos")]
    pub expires_at: Timestamp,
    pub distribution: OutcomeDistribution,
    /// How the issuing model's resolved forecasts scored, as the node
    /// measured it. The centre re-scores them after the fact from outcomes
    /// that reach it, so the claim here is checked by a party that did not
    /// make it (ADR 0133).
    pub calibration: Calibration,
    /// The model that issued it, by name and version.
    pub model: String,
    pub promotion: ReflexPromotion,
}

fn nanos<S: serde::Serializer>(t: &Timestamp, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_i64(t.as_nanos())
}

/// The reflex lane's microsecond forecast. Built only through
/// [`Validated::new`].
pub type ReflexForecast = Validated<ReflexForecastFields>;

impl ReflexForecastFields {
    /// The stated probability, in basis points, that an event target occurs
    /// within the horizon; `None` for a magnitude target.
    pub fn occurred_bp(&self) -> Option<u32> {
        if !self.target.is_event() {
            return None;
        }
        match &self.distribution {
            OutcomeDistribution::Categorical { outcomes } => outcomes
                .iter()
                .find(|o| o.outcome == OCCURRED)
                .map(|o| o.probability_bp),
            OutcomeDistribution::Quantiles { .. } => None,
        }
    }
}

impl Contract for ReflexForecastFields {
    const NAME: &'static str = "ReflexForecast";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id, cell, venue, instrument or model; a horizon of
    /// zero or of a second and over; an expiry not after issue or beyond the
    /// horizon; a malformed distribution or calibration; and a distribution
    /// whose shape does not match the target.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "forecast_id", &self.forecast_id)?;
        named(c, "cell", &self.cell)?;
        named(c, "venue", self.venue.as_str())?;
        named(c, "instrument", &self.instrument)?;
        named(c, "model", &self.model)?;
        if self.horizon_us == 0 || self.horizon_us > MAX_REFLEX_HORIZON_US {
            return refuse(
                c,
                format!(
                    "states a horizon of {} µs; the microsecond row speaks from 1 to \
                     {MAX_REFLEX_HORIZON_US} µs, and a longer one belongs to the seconds-minutes \
                     row, whose promotion target is not the local reflex",
                    self.horizon_us
                ),
            );
        }
        after(
            c,
            "expires_at",
            self.expires_at,
            "issued_at",
            self.issued_at,
        )?;
        // Bounded above to 999,999 µs, so the conversion always succeeds; the
        // fallback is the refusal-preserving maximum rather than a panic.
        let horizon = Duration::from_micros(i64::try_from(self.horizon_us).unwrap_or(i64::MAX));
        if self.expires_at > self.issued_at.saturating_add(horizon) {
            return refuse(
                c,
                "expires after its own horizon has passed; a forecast read after the instant it \
                 speaks about is a forecast of the past",
            );
        }
        self.distribution.check(c)?;
        self.calibration.check(c)?;
        match (&self.distribution, self.target.is_event()) {
            (OutcomeDistribution::Categorical { outcomes }, true) => {
                let mut names: Vec<&str> = outcomes.iter().map(|o| o.outcome.as_str()).collect();
                names.sort_unstable();
                if names != [NOT_OCCURRED, OCCURRED] {
                    return refuse(
                        c,
                        format!(
                            "states {} as outcomes {names:?}; an event target is stated over \
                             exactly `{OCCURRED}` and `{NOT_OCCURRED}`",
                            self.target.as_str()
                        ),
                    );
                }
                Ok(())
            }
            (OutcomeDistribution::Quantiles { .. }, false) => Ok(()),
            (_, true) => refuse(
                c,
                format!(
                    "states the event {} as quantiles; state it as a categorical over `{OCCURRED}` \
                     and `{NOT_OCCURRED}` so it can be scored as an event",
                    self.target.as_str()
                ),
            ),
            (_, false) => refuse(
                c,
                format!(
                    "states the magnitude {} as a categorical; state it as quantiles",
                    self.target.as_str()
                ),
            ),
        }
    }
}
