//! Blueprint §12.3's last row, executed-order half: "alternative sizing
//! consistently better… the counterfactual says exactly where", read from
//! the sizes the twin priced on the orders a venue actually filled.
//!
//! ADR 0055 built the declined-path half — a multiplier on sizing
//! confidence that can only narrow. This module reads the other half, the
//! [`FillScore`]s the LEARN stage writes for every fill, and produces two
//! things that differ by how much they may change:
//!
//! * A **cap** ([`cap_multiplier`]) on one instrument's weight bound, in
//!   `(0, 1]`, armed when at least three in four of a sample of at least ten
//!   scored fills on that instrument would have done better *smaller*. A
//!   bound rather than a second budget multiplier, because a budget cannot
//!   name the instrument and a bound can — and the proposal's own
//!   `compromises` then say which name was narrowed and by how much (ADR
//!   0063). No branch returns more than one.
//! * A **finding** ([`larger_size_finding`]) when the same bars are cleared
//!   in the other direction — most fills would have done better *larger*.
//!   A record, never a number: §12.4's guardrail forbids loosening on
//!   counterfactual evidence, and this function's return type cannot carry
//!   a multiplier at all.
//!
//! The bars are ADR 0055's by reference, for the reason `rule_review` and
//! `venue_review` give: the platform has one answer to how much evidence
//! makes a pattern a finding.

use crate::platform::{
    COUNTERFACTUAL_SIZING_MIN_SAMPLE, COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION, FillScore,
};
use qip_core::Decimal;
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

/// Minimum scored fills on one instrument before either finding is trusted.
/// [`COUNTERFACTUAL_SIZING_MIN_SAMPLE`], by reference.
pub const SIZING_CAP_MIN_SAMPLE: usize = COUNTERFACTUAL_SIZING_MIN_SAMPLE;

/// The fraction of an instrument's scored fills that must favour one size
/// direction before it is a finding. Three in four, by reference to
/// [`COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION`].
pub const SIZING_CAP_FAVOUR_FRACTION: f64 = COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION;

/// The one cap [`cap_multiplier`] may return below one: the weight bound
/// halved. A single auditable number, as ADR 0055's discount is, so what
/// the finding changes is something a person can name rather than a curve
/// fitted after the fact.
pub const SIZING_CAP_MULTIPLIER: Decimal = Decimal::from_raw(500_000_000);

/// What one instrument's scored fills add up to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizeRegret {
    pub sample: usize,
    pub smaller_favoured: usize,
    pub larger_favoured: usize,
}

/// Accumulate one instrument's fill scores.
pub fn size_regret(scores: &[FillScore], object_id: &str) -> SizeRegret {
    let mut regret = SizeRegret {
        sample: 0,
        smaller_favoured: 0,
        larger_favoured: 0,
    };
    for score in scores
        .iter()
        .filter(|score| score.object_id.as_str() == object_id)
    {
        regret.sample += 1;
        if score.smaller_favoured {
            regret.smaller_favoured += 1;
        }
        if score.larger_favoured {
            regret.larger_favoured += 1;
        }
    }
    regret
}

impl SizeRegret {
    /// usize → f64: a ratio of counts, in the statistics lane.
    fn fraction(count: usize, sample: usize) -> f64 {
        if sample == 0 {
            0.0
        } else {
            count as f64 / sample as f64
        }
    }

    pub fn smaller_fraction(&self) -> f64 {
        Self::fraction(self.smaller_favoured, self.sample)
    }

    pub fn larger_fraction(&self) -> f64 {
        Self::fraction(self.larger_favoured, self.sample)
    }

    fn clears(&self, fraction: f64) -> bool {
        self.sample >= SIZING_CAP_MIN_SAMPLE && fraction >= SIZING_CAP_FAVOUR_FRACTION
    }
}

/// The cap on `object_id`'s weight bound, in `(0, 1]`.
///
/// [`SIZING_CAP_MULTIPLIER`] when the sample clears both bars on the
/// smaller side; one otherwise, including below the sample and including
/// when the *larger* side clears them. There is no branch above one, which
/// is how §12.4's guardrail is held structurally rather than by care at the
/// call site.
pub fn cap_multiplier(scores: &[FillScore], object_id: &str) -> Decimal {
    let regret = size_regret(scores, object_id);
    if regret.clears(regret.smaller_fraction()) {
        SIZING_CAP_MULTIPLIER
    } else {
        Decimal::ONE
    }
}

/// A pattern of fills that would have done better larger: a finding, with
/// no multiplier on it, because the only shape the loosening direction may
/// take is a proposal a person reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LargerSizeFinding {
    pub sample: usize,
    pub larger_favoured: usize,
}

/// The finding on `object_id`'s fills, if the larger side clears the bars.
pub fn larger_size_finding(scores: &[FillScore], object_id: &str) -> Option<LargerSizeFinding> {
    let regret = size_regret(scores, object_id);
    regret
        .clears(regret.larger_fraction())
        .then_some(LargerSizeFinding {
            sample: regret.sample,
            larger_favoured: regret.larger_favoured,
        })
}

/// The cap was armed on this instrument this cycle.
pub const SIZING_CAP_ARMED: &str = "armed";
/// The cap was released: the evidence stopped clearing the bar.
pub const SIZING_CAP_RELEASED: &str = "released";

/// A change of state on one instrument's sizing cap, journaled under
/// `learning.sizing_reviewed` when it *changes* and not on every cycle the
/// cap stands, so the log says when a bound moved rather than restating it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SizingCapEntry {
    pub object_id: String,
    pub sample: usize,
    /// The fraction of the sample that favoured the smaller size.
    pub fraction: f64,
    /// The multiplier now in force: [`SIZING_CAP_MULTIPLIER`] when armed,
    /// one when released.
    pub multiplier: Decimal,
    /// [`SIZING_CAP_ARMED`] or [`SIZING_CAP_RELEASED`].
    pub state: String,
    pub cycle: u64,
    pub at: Timestamp,
}

impl EventBody for SizingCapEntry {
    const TOPIC: Topic = Topic::SizingReviewed;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "sizing-cap:{}:{}:{}",
            self.object_id, self.state, self.cycle
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::ids::{ObjectId, OrderId};

    const OBJECT: &str = "obj-AAA";

    fn score(index: usize, smaller: bool, larger: bool) -> FillScore {
        let at = Timestamp::from_secs(1_760_000_000);
        FillScore {
            order_id: OrderId::from_string(format!("ord-fill-{index}")),
            object_id: ObjectId::from_string(OBJECT),
            venue: "simulated-venue".to_string(),
            filled_at: at,
            scored_at: at,
            smaller_favoured: smaller,
            larger_favoured: larger,
            trade_error_bps: None,
        }
    }

    fn scores(count: usize, smaller: bool, larger: bool) -> Vec<FillScore> {
        (0..count).map(|i| score(i, smaller, larger)).collect()
    }

    #[test]
    fn below_the_sample_the_cap_is_one() {
        // Nine fills that every one would have done better smaller: a
        // fraction of one on a sample one short of the bar. Narrowing on
        // nine observations is narrowing on noise, the same discipline ADR
        // 0055 states for the declined half. The tenth makes it a finding —
        // the admitting half, so the bar is a bar and not a function that
        // always answers one.
        let nine = scores(SIZING_CAP_MIN_SAMPLE - 1, true, false);
        assert_eq!(nine.len(), 9, "the premise is nine");
        assert_eq!(cap_multiplier(&nine, OBJECT), Decimal::ONE);

        let ten = scores(SIZING_CAP_MIN_SAMPLE, true, false);
        assert_eq!(cap_multiplier(&ten, OBJECT), SIZING_CAP_MULTIPLIER);
        assert_eq!(
            cap_multiplier(&ten, "obj-BBB"),
            Decimal::ONE,
            "another instrument's fills armed this one's cap"
        );
    }

    #[test]
    fn a_pattern_of_fills_that_would_have_done_better_larger_never_raises_the_cap_above_one() {
        // The mirror of `a_pattern_of_wrongly_declined_paths_never_widens_
        // sizing`: overwhelming evidence that every fill should have been
        // larger, and the cap stays at one. §12.4's guardrail is held by
        // there being no branch that returns more than one — not by care
        // taken to ignore the larger side at the call site. The finding for
        // that pattern is a record, produced beside this and carrying no
        // multiplier.
        let larger = scores(SIZING_CAP_MIN_SAMPLE * 3, false, true);
        assert_eq!(
            cap_multiplier(&larger, OBJECT),
            Decimal::ONE,
            "overwhelming evidence for a larger size moved the cap"
        );
        assert!(cap_multiplier(&larger, OBJECT) <= Decimal::ONE);
        let finding = larger_size_finding(&larger, OBJECT).expect("the larger side clears");
        assert_eq!(finding.sample, 30);
        assert_eq!(finding.larger_favoured, 30);
        // And the smaller side alone produces no larger finding.
        assert_eq!(
            larger_size_finding(&scores(SIZING_CAP_MIN_SAMPLE, true, false), OBJECT),
            None
        );
    }
}
