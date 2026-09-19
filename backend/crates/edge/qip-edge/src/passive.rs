//! §32.1's second mechanism: passive-first on thin venues.
//!
//! The section's own words are *"rest on the slow venue, cross the fast ones
//! on its fill"*, and the effect it claims is *"removes it from the exposure
//! window"*. That is the whole argument. A cycle whose legs go to the venues
//! all at once is exposed from the instant the first leg fills until the last
//! one does, and the length of that window is set by the slowest venue in the
//! set. Sending the slow leg **alone** and waiting for the venue's own answer
//! before the fast ones go out does not shorten the window — it removes the
//! slow venue from it, because by the time the fast legs are sent the slow
//! one is already a fact rather than a hope.
//!
//! "Thin" in the section's heading and "slow" in its description name the same
//! venue: a venue with little resting depth is the one an order sits on, and
//! sitting is what the cell measures. The cell measures fill time and nothing
//! else about a venue's depth over time, so this module chooses on fill time
//! and says so, rather than asserting a thinness it never computed.
//!
//! # What this module decides, and what it deliberately does not
//!
//! It decides **which leg rests**, from the medians [`crate::dispersion`]
//! already keeps, and nothing else. It holds no clock, no timer and no
//! schedule. That boundary is not tidiness: the section's *first* mechanism,
//! latency-equalised dispatch, needs a timer wheel on a dispatch thread and a
//! send delayed by a measured interval, and this process has neither — ADR
//! 0001 and ADR 0011 rule out the async runtime it would take, and
//! [`crate::cell::Cell`] is handed `now` rather than reading a clock precisely
//! so a replay of the same inputs produces the same orders. Passive-first
//! needs neither, which is why it is buildable here and equalisation is not.
//!
//! # Why a unique slowest venue and not merely a slowest one
//!
//! Two venues whose medians are equal offer no answer to "which one do we wait
//! for". Resting on either is a coin toss, and the fast legs would still be
//! crossed while a venue just as slow was outstanding — the exposure window is
//! unchanged and the cycle has been delayed a pass for nothing. So a tie at the
//! top declines the mechanism and the cycle goes out whole, which is what the
//! cell did before this existed.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use qip_contracts::VenueId;

/// Which leg of a cycle rests first, or why none does.
///
/// Two arms and not an `Option`, because "no leg rests" has three completely
/// different causes and an operator reading the series needs to know which
/// one: the cycle is at one venue and has no slow side, the cell has not
/// measured enough of its venues to have an opinion, or it has measured them
/// and they are equally slow. The first is the mechanism not applying, the
/// second is the mechanism having no evidence, and the third is the mechanism
/// declining on the evidence it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PassiveChoice {
    /// The leg at `position` in plan order goes out alone and rests; the rest
    /// of the cycle waits for the venue's answer on it.
    Rest {
        position: usize,
        venue: String,
        /// The measured median fill time that made this venue the slowest.
        /// Carried for the journal: "why is this cycle resting" is answered
        /// by the number, never by the fact that it rested.
        median: Duration,
    },
    /// Every leg goes out this pass, as the cell did before this mechanism
    /// existed.
    Whole(WholeReason),
}

/// Why a cycle went out whole rather than resting a leg first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WholeReason {
    /// Every leg is at one venue. There is no fast side to cross on the slow
    /// one's fill, and two legs at one venue arrive together.
    SingleVenue,
    /// Fewer than two of the cycle's venues have produced enough fills for a
    /// median. The cell has no basis for calling one of them slow, and
    /// picking one anyway would be a number nobody computed.
    Unmeasured,
    /// Two or more venues share the longest median. See the module
    /// documentation for why a tie declines.
    NoSlowest,
}

impl WholeReason {
    /// The label this reason is counted under: one source-file literal per
    /// arm, so the series is bounded by this enum and never by a venue name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SingleVenue => "single_venue",
            Self::Unmeasured => "unmeasured",
            Self::NoSlowest => "no_slowest",
        }
    }
}

/// What became of a cycle the mechanism took an interest in.
///
/// Counted rather than inferred from the journal, and `Whole` is in the
/// enumeration on purpose: without it a cell that never rested a leg and a
/// cell that never ran a cycle read identically on the series, which is the
/// same failure `qip_edge_fill_time_unmeasured_venues` exists to close beside
/// the dispersion gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveOutcome {
    /// A leg was sent alone and is resting. The rest of the cycle has not
    /// been sent and no capital is at risk beyond that one leg.
    Rested,
    /// The resting leg was answered and the rest of the cycle went out behind
    /// it, at whatever fraction it completed.
    Completed,
    /// The resting leg was withdrawn without filling anything, so the cycle
    /// never became a position at all. This is the outcome the mechanism
    /// exists to produce: under the all-at-once discipline the fast legs
    /// would already have been crossed.
    Abandoned,
    /// The cycle went out whole, the mechanism declining for one of
    /// [`WholeReason`]'s three causes.
    Whole,
}

impl PassiveOutcome {
    /// The label this outcome is counted under: one source-file literal per
    /// arm, so the series is bounded by this enum.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rested => "rested",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
            Self::Whole => "whole",
        }
    }
}

/// Which leg of `legs` — venues in plan order — should rest first, given what
/// each venue's fill time has been measured at.
///
/// `medians` holds an entry only for a venue the cell has measured; a venue
/// absent from it is unmeasured, which is not the same as fast. That
/// distinction is the whole reason [`WholeReason::Unmeasured`] exists: a cell
/// that has filled nothing anywhere would otherwise rest every cycle on
/// whichever venue happened to sort first.
///
/// The position returned is the **first** leg at the slowest venue, so a
/// cycle with two legs at one slow venue rests the earlier of them. Plan
/// order is least reversible first, and resting the later leg would leave the
/// more reversible one outstanding while the cell waited.
pub fn choose(legs: &[VenueId], medians: &BTreeMap<String, Duration>) -> PassiveChoice {
    let distinct: BTreeSet<&str> = legs.iter().map(VenueId::as_str).collect();
    if distinct.len() < 2 {
        return PassiveChoice::Whole(WholeReason::SingleVenue);
    }
    // Measured venues only, in a deterministic order: `distinct` is a
    // `BTreeSet`, so two venues with identical medians are compared in the
    // same order on every replay and the tie below is found rather than
    // resolved by whichever arrived first.
    let measured: Vec<(&str, Duration)> = distinct
        .iter()
        .filter_map(|venue| medians.get(*venue).map(|median| (*venue, *median)))
        .collect();
    if measured.len() < 2 {
        return PassiveChoice::Whole(WholeReason::Unmeasured);
    }
    let Some((slowest, median)) = measured
        .iter()
        .copied()
        .max_by_key(|(_, median)| median.as_nanos())
    else {
        // Unreachable: `measured` holds at least two entries above. Stated as
        // a decline rather than an unwrap, because this function is on the
        // order path and the crate forbids the panic either way.
        return PassiveChoice::Whole(WholeReason::Unmeasured);
    };
    let tied = measured
        .iter()
        .filter(|(_, other)| *other == median)
        .count();
    if tied > 1 {
        return PassiveChoice::Whole(WholeReason::NoSlowest);
    }
    let Some(position) = legs.iter().position(|leg| leg.as_str() == slowest) else {
        // Also unreachable: `slowest` came from `legs`.
        return PassiveChoice::Whole(WholeReason::Unmeasured);
    };
    PassiveChoice::Rest {
        position,
        venue: slowest.to_string(),
        median,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venue(name: &str) -> VenueId {
        VenueId::new(name)
    }

    fn medians(pairs: &[(&str, u64)]) -> BTreeMap<String, Duration> {
        pairs
            .iter()
            .map(|(name, millis)| ((*name).to_string(), Duration::from_millis(*millis)))
            .collect()
    }

    #[test]
    fn a_cycle_whose_legs_are_all_at_one_venue_rests_nothing() {
        let legs = vec![venue("alpha"), venue("alpha")];
        assert_eq!(
            choose(&legs, &medians(&[("alpha", 40)])),
            PassiveChoice::Whole(WholeReason::SingleVenue)
        );
    }

    #[test]
    fn a_cycle_with_one_measured_venue_rests_nothing_because_it_has_no_comparison() {
        let legs = vec![venue("alpha"), venue("beta")];
        // The premise: the two venues really are distinct, so the decline is
        // about measurement and not about the cycle's shape.
        assert_ne!(legs[0].as_str(), legs[1].as_str());
        assert_eq!(
            choose(&legs, &medians(&[("alpha", 40)])),
            PassiveChoice::Whole(WholeReason::Unmeasured)
        );
    }

    #[test]
    fn the_leg_at_the_slowest_measured_venue_is_the_one_that_rests() {
        let legs = vec![venue("alpha"), venue("beta"), venue("gamma")];
        assert_eq!(
            choose(
                &legs,
                &medians(&[("alpha", 40), ("beta", 90), ("gamma", 12)])
            ),
            PassiveChoice::Rest {
                position: 1,
                venue: "beta".to_string(),
                median: Duration::from_millis(90),
            }
        );
    }

    #[test]
    fn two_venues_that_are_equally_slow_leave_the_cycle_going_out_whole() {
        let legs = vec![venue("alpha"), venue("beta"), venue("gamma")];
        // The premise: all three are measured, so the decline is the tie and
        // not a missing median.
        let table = medians(&[("alpha", 90), ("beta", 90), ("gamma", 12)]);
        assert_eq!(table.len(), 3);
        assert_eq!(
            choose(&legs, &table),
            PassiveChoice::Whole(WholeReason::NoSlowest)
        );
    }

    #[test]
    fn the_earlier_of_two_legs_at_the_slow_venue_is_the_one_that_rests() {
        let legs = vec![venue("beta"), venue("alpha"), venue("beta")];
        assert_eq!(
            choose(&legs, &medians(&[("alpha", 40), ("beta", 90)])),
            PassiveChoice::Rest {
                position: 0,
                venue: "beta".to_string(),
                median: Duration::from_millis(90),
            }
        );
    }

    #[test]
    fn an_unmeasured_venue_is_never_read_as_a_fast_one() {
        // `gamma` has no median. If absence read as zero it would be the
        // fastest venue and `beta` would still rest; the property here is
        // that the cell declines instead, because two of three measured is
        // not a measurement of the third.
        let legs = vec![venue("alpha"), venue("gamma")];
        assert_eq!(
            choose(&legs, &medians(&[("alpha", 40), ("beta", 90)])),
            PassiveChoice::Whole(WholeReason::Unmeasured)
        );
    }
}
