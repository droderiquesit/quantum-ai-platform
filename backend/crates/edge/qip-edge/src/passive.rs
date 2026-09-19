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
//!
//! # Resting is not free, and the threshold is the operator's own number
//!
//! Waiting costs a pass. A cycle that would have gone out in one now goes out
//! in two, and an arbitrage is an opportunity with a shelf life — the price
//! that made it worth taking may not survive the wait. So the mechanism must
//! be worth its cost, and on two venues a millisecond apart it is not: it
//! would trade a whole pass to remove a millisecond from the exposure window.
//! That is not a hypothetical. This module engaged unconditionally on any
//! measured difference for exactly one commit, and
//! `two_venues_that_answer_together_keep_trading_and_their_fill_times_are_published`
//! — a test written for the dispersion gate, not for this — caught it, because
//! its two venues are one and two milliseconds apart under a five-millisecond
//! bound and it stopped sending the cycle in one pass.
//!
//! The threshold is [`crate::dispersion::DispersionPolicy::bound`] and
//! deliberately not a number of this module's own. The bound is already the
//! operator's statement of how far apart a cycle's legs may arrive and still
//! be one position; a venue whose *own* median answer takes longer than that
//! is, in the operator's own units, a venue that cannot be part of a
//! simultaneous set. That is what "thin" means here, and it is measured rather
//! than declared. A second threshold would be a second opinion about the same
//! question, and the two would disagree.

use std::collections::{BTreeMap, BTreeSet};

use qip_contracts::VenueId;
use qip_core::Duration;

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
    /// The slowest venue answers inside the dispersion bound, so there is
    /// nothing worth removing from the exposure window and a pass spent
    /// waiting would cost more than it saved.
    WithinBound,
}

impl WholeReason {
    /// The label this reason is counted under: one source-file literal per
    /// arm, so the series is bounded by this enum and never by a venue name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SingleVenue => "single_venue",
            Self::Unmeasured => "unmeasured",
            Self::NoSlowest => "no_slowest",
            Self::WithinBound => "within_bound",
        }
    }
}

/// What became of a cycle the mechanism took an interest in.
///
/// The declining arm carries its [`WholeReason`] rather than flattening to
/// one `whole` label, and that is the difference between a series that can be
/// acted on and one that only proves the code ran. A cell that never rests a
/// cycle because it is a single-venue cell needs nothing done about it; a cell
/// that never rests one because two of its venues have been equally slow for a
/// week is a cell whose fill-time measurement has stopped discriminating. Both
/// read as `whole` on a flattened series, and an operator would have to go to
/// the journal to tell them apart — which is the thing a series exists to
/// save.
///
/// The declining arms are also the denominator. Without them a cell that has
/// never rested a leg and a cell that has never run a cycle are the same empty
/// series, which is the failure `qip_edge_fill_time_unmeasured_venues` exists
/// to close beside the dispersion gate.
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
    /// The cycle went out whole, the mechanism having declined for this
    /// reason.
    Whole(WholeReason),
}

impl PassiveOutcome {
    /// The label this outcome is counted under: one source-file literal per
    /// arm here and per arm of [`WholeReason`], six in all, so the series is
    /// bounded by the two enums and never by a venue name or a cycle id.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rested => "rested",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
            Self::Whole(reason) => reason.as_str(),
        }
    }
}

/// Which leg of `legs` — venues in plan order — should rest first, given what
/// each venue's fill time has been measured at and how far apart `bound` says
/// a cycle's legs may arrive.
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
pub fn choose(
    legs: &[VenueId],
    medians: &BTreeMap<String, Duration>,
    bound: Duration,
) -> PassiveChoice {
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
    // Strictly slower than the bound, never merely at it. A venue that
    // answers in exactly the time the operator says legs may be apart is
    // inside what they permitted, and refusing to trade in one pass at the
    // boundary would make the bound mean one thing here and another in
    // `FillTimes::assess`, which also compares strictly.
    if median <= bound {
        return PassiveChoice::Whole(WholeReason::WithinBound);
    }
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

    fn medians(pairs: &[(&str, i64)]) -> BTreeMap<String, Duration> {
        pairs
            .iter()
            .map(|(name, millis)| ((*name).to_string(), Duration::from_millis(*millis)))
            .collect()
    }

    /// Small enough that every venue in these fixtures is over it, except
    /// where a test is about the bound itself.
    fn bound() -> Duration {
        Duration::from_millis(5)
    }

    #[test]
    fn a_cycle_whose_legs_are_all_at_one_venue_rests_nothing() {
        let legs = vec![venue("alpha"), venue("alpha")];
        assert_eq!(
            choose(&legs, &medians(&[("alpha", 40)]), bound()),
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
            choose(&legs, &medians(&[("alpha", 40)]), bound()),
            PassiveChoice::Whole(WholeReason::Unmeasured)
        );
    }

    #[test]
    fn the_leg_at_the_slowest_measured_venue_is_the_one_that_rests() {
        let legs = vec![venue("alpha"), venue("beta"), venue("gamma")];
        assert_eq!(
            choose(
                &legs,
                &medians(&[("alpha", 40), ("beta", 90), ("gamma", 12)]),
                bound()
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
            choose(&legs, &table, bound()),
            PassiveChoice::Whole(WholeReason::NoSlowest)
        );
    }

    #[test]
    fn the_earlier_of_two_legs_at_the_slow_venue_is_the_one_that_rests() {
        let legs = vec![venue("beta"), venue("alpha"), venue("beta")];
        assert_eq!(
            choose(&legs, &medians(&[("alpha", 40), ("beta", 90)]), bound()),
            PassiveChoice::Rest {
                position: 0,
                venue: "beta".to_string(),
                median: Duration::from_millis(90),
            }
        );
    }

    #[test]
    fn two_venues_a_millisecond_apart_are_not_worth_a_pass_of_waiting() {
        // The regression this threshold exists for. Both venues are measured
        // and one is strictly slower, so every other condition says rest —
        // and resting here would spend a whole pass to remove one millisecond
        // from the exposure window. `dispersion.rs`'s
        // `two_venues_that_answer_together_keep_trading_...` failed on
        // exactly this shape before the bound was consulted.
        let legs = vec![venue("alpha"), venue("beta")];
        let table = medians(&[("alpha", 1), ("beta", 2)]);
        assert_eq!(table.len(), 2, "the premise failed: both venues measured");
        assert!(
            table["beta"] > table["alpha"],
            "the premise failed: beta is not the slower venue"
        );
        assert_eq!(
            choose(&legs, &table, Duration::from_millis(5)),
            PassiveChoice::Whole(WholeReason::WithinBound)
        );
    }

    #[test]
    fn a_venue_slower_on_its_own_than_the_bound_allows_between_legs_is_rested_on() {
        // The other half: same shape, same bound, and the slow venue's own
        // median is past it. Without this the test above would be satisfied
        // by a mechanism that never engages at all.
        let legs = vec![venue("alpha"), venue("beta")];
        assert_eq!(
            choose(
                &legs,
                &medians(&[("alpha", 1), ("beta", 9)]),
                Duration::from_millis(5)
            ),
            PassiveChoice::Rest {
                position: 1,
                venue: "beta".to_string(),
                median: Duration::from_millis(9),
            }
        );
    }

    #[test]
    fn a_venue_exactly_at_the_bound_is_inside_what_the_operator_permitted() {
        let legs = vec![venue("alpha"), venue("beta")];
        assert_eq!(
            choose(
                &legs,
                &medians(&[("alpha", 1), ("beta", 5)]),
                Duration::from_millis(5)
            ),
            PassiveChoice::Whole(WholeReason::WithinBound)
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
            choose(&legs, &medians(&[("alpha", 40), ("beta", 90)]), bound()),
            PassiveChoice::Whole(WholeReason::Unmeasured)
        );
    }
}
