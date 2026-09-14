//! Blueprint §34.4 at the kernel: which of the venues the platform can reach
//! have actually earned the simulator, and which are being used on their own
//! documentation.
//!
//! [`qip_lifecycle::venue_ladder`] holds the ladder and the gates. This module
//! is the composition: it reads a ladder the platform holds against the set of
//! venues the platform can reach, and reports the gap in the
//! `(summary, problems)` shape every other LEARN review uses.
//!
//! # What this may conclude, and what it may not
//!
//! It reports. It does not withdraw, and it does not enable.
//!
//! Not enabling is the important half and it is structural rather than
//! conventional: there is no function here that returns a venue to add, and
//! the ladder it reads has no rung above the simulator to report. A caller
//! that wanted to turn this into an enablement would have to write the
//! enablement itself, which is exactly the visibility that decision deserves.
//!
//! Not *withdrawing* is a deliberate restraint rather than an omission, and
//! the reason is a hazard worth naming. The obvious next step — withdraw
//! every reachable venue the ladder has not admitted — would, on the first
//! deployment carrying an empty ladder, withdraw every venue the platform
//! has. That is a control firing correctly and stopping the platform, which
//! is the safe direction but not a change anybody chose. So the two
//! populations are reported separately by [`review`]: a venue the ladder
//! *knows* and has not promoted is a venue mid-ladder, and a venue the ladder
//! has never heard of is a venue nobody registered, and the second is the one
//! that means the ladder is not wired rather than that the venue is
//! suspicious. [`crate::venue_review`] remains the only path by which a venue
//! is withdrawn, on evidence of refusals rather than on an absence of
//! records.
//!
//! # Nothing calls this yet, and that is stated rather than implied
//!
//! `Platform` holds no [`VenueLadder`]. Wiring it takes one field and one
//! line in `stage_learn`, both named in this lane's handoff. Until they land,
//! this module is composition waiting for a caller and the §34.4 row is not
//! delivered at the kernel — which is said here, in the file, because a
//! module that reads as wired is worse than one that is plainly not.

use qip_lifecycle::venue_ladder::{VenueLadder, rung_name};
use std::collections::BTreeSet;

/// Where one reachable venue stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueStanding {
    pub venue: String,
    /// The §34.4 rung name — `registered`, `observed`, `simulated`.
    pub rung: &'static str,
    /// Whether the simulator may use it.
    pub admitted: bool,
    /// Whether the ladder holds any record of it at all. `false` means the
    /// venue was never registered, which is a different finding from a venue
    /// working its way up.
    pub registered: bool,
}

/// Where every reachable venue stands, in a deterministic order.
///
/// `reachable` is a set rather than a slice so a replay walks it identically,
/// and the output follows its order for the same reason.
pub fn standings(ladder: &VenueLadder, reachable: &BTreeSet<String>) -> Vec<VenueStanding> {
    reachable
        .iter()
        .map(|venue| VenueStanding {
            venue: venue.clone(),
            rung: rung_name(ladder.stage_of(venue)),
            admitted: ladder.admits(venue),
            registered: ladder.knows(venue),
        })
        .collect()
}

/// The LEARN stage's venue-admission review.
///
/// Returns the same `(summary, problems)` pair `review_venues` and
/// `review_rules` return, so a caller folds it into the stage outcome the way
/// it folds every other review, and so a cycle with nothing to say adds
/// nothing to the record.
///
/// The summary is present only when at least one reachable venue has cleared
/// the simulated rung, because "0 of 0 venues admitted" on every cycle of a
/// deployment that has not wired the ladder is a line nobody reads and
/// everybody learns to skip.
pub fn review(ladder: &VenueLadder, reachable: &BTreeSet<String>) -> (Option<String>, Vec<String>) {
    let standings = standings(ladder, reachable);
    let admitted = standings
        .iter()
        .filter(|standing| standing.admitted)
        .count();
    let mut problems = Vec::new();
    for standing in &standings {
        if standing.admitted {
            continue;
        }
        if standing.registered {
            problems.push(format!(
                "venue {} is reachable and stands at the {} rung; §34.4 admits a venue to the \
                 simulator only once its declared fees, latency and order types have been \
                 verified against measurement and a replay has reconciled",
                standing.venue, standing.rung
            ));
        } else {
            problems.push(format!(
                "venue {} is reachable and the promotion ladder holds no record of it at all; \
                 it is being used on its own documentation, which is the one thing §34.4 says \
                 never to do",
                standing.venue
            ));
        }
    }
    let summary = if admitted == 0 {
        None
    } else {
        Some(format!(
            "{admitted} of {} reachable venue(s) have cleared the simulated rung",
            standings.len()
        ))
    };
    (summary, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::gate::GateStage;
    use qip_contracts::venue::{VenueClass, VenueId};
    use qip_core::{Decimal, Duration, Timestamp};
    use qip_lifecycle::venue_ladder::{
        SimulationEvidence, VenueDeclaration, VenueEvidence, VenueMeasurement,
        VenuePromotionPolicy, attempt_promotion,
    };

    fn now() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    fn reachable(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn order_types() -> BTreeSet<String> {
        ["limit".to_string()].into_iter().collect()
    }

    fn declaration(venue: &str) -> VenueDeclaration {
        VenueDeclaration::new(
            VenueId::new(venue),
            VenueClass::CryptoExchange,
            Decimal::from_int(10),
            Duration::from_millis(50),
            order_types(),
        )
        .expect("a valid declaration")
    }

    fn honest_evidence() -> VenueEvidence {
        VenueEvidence::new()
            .with_measurement(VenueMeasurement {
                fee_bps: Decimal::from_int(10),
                latency: Duration::from_millis(50),
                accepted_order_types: order_types(),
                rejected_order_types: BTreeSet::new(),
                observations: 40,
            })
            .with_simulation(SimulationEvidence {
                replayed_sessions: 10,
                reconciliation_breaks: 0,
                reference_clip: Decimal::from_int(10),
            })
    }

    /// Walk `venue` all the way to the ceiling.
    fn admit(ladder: &mut VenueLadder, venue: &str) {
        let declaration = declaration(venue);
        for _ in 0..2 {
            attempt_promotion(
                ladder,
                &declaration,
                &honest_evidence(),
                VenuePromotionPolicy::default(),
                None,
                "measured, replayed and reconciled",
                now(),
            )
            .expect("an honest venue walks to the simulator");
        }
    }

    #[test]
    fn a_reachable_venue_the_ladder_never_heard_of_is_reported_as_never_registered() {
        // The distinction this module exists to keep: "standing at the bottom
        // rung" and "not on the ladder" are the same `GateStage` and two
        // different findings, and only the second says the ladder is not
        // wired.
        let ladder = VenueLadder::new();
        let (summary, problems) = review(&ladder, &reachable(&["XPOOL"]));
        assert_eq!(summary, None);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("holds no record of it at all"),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn a_venue_mid_ladder_is_reported_by_the_rung_it_stands_on_and_not_as_unregistered() {
        let mut ladder = VenueLadder::new();
        attempt_promotion(
            &mut ladder,
            &declaration("XPOOL"),
            &honest_evidence(),
            VenuePromotionPolicy::default(),
            None,
            "measured read-only",
            now(),
        )
        .expect("the premise: it clears the observed rung");
        assert_eq!(ladder.stage_of("XPOOL"), GateStage::Holdout);
        let (summary, problems) = review(&ladder, &reachable(&["XPOOL"]));
        assert_eq!(summary, None);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("stands at the observed rung"),
            "{}",
            problems[0]
        );
        assert!(
            !problems[0].contains("no record"),
            "a venue mid-ladder was reported as never registered: {}",
            problems[0]
        );
    }

    #[test]
    fn an_admitted_venue_raises_no_problem_and_is_counted_in_the_summary() {
        // The half that proves this review is not simply a complaint
        // generator: a venue that earned the rung produces no problem at all.
        let mut ladder = VenueLadder::new();
        admit(&mut ladder, "XPOOL");
        assert!(
            ladder.admits("XPOOL"),
            "the premise: it reached the ceiling"
        );
        let (summary, problems) = review(&ladder, &reachable(&["XPOOL"]));
        assert_eq!(
            summary.as_deref(),
            Some("1 of 1 reachable venue(s) have cleared the simulated rung")
        );
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_mixed_estate_reports_one_problem_per_unadmitted_venue_in_a_replayable_order() {
        let mut ladder = VenueLadder::new();
        admit(&mut ladder, "AAAA");
        let (summary, problems) = review(&ladder, &reachable(&["AAAA", "BBBB", "CCCC"]));
        assert_eq!(
            summary.as_deref(),
            Some("1 of 3 reachable venue(s) have cleared the simulated rung")
        );
        assert_eq!(problems.len(), 2);
        // Set order, so a replay produces the same record. Asserted on the
        // venue names rather than on the whole sentence, because the order is
        // the property under test.
        assert!(problems[0].contains("venue BBBB"), "{}", problems[0]);
        assert!(problems[1].contains("venue CCCC"), "{}", problems[1]);
    }

    #[test]
    fn a_venue_the_platform_cannot_reach_is_not_reviewed_however_it_stands() {
        // The review is about what the platform can reach, not about every
        // venue the ladder has an opinion on. A ladder entry for a venue no
        // broker names is not a problem on this cycle.
        let mut ladder = VenueLadder::new();
        admit(&mut ladder, "AAAA");
        let (summary, problems) = review(&ladder, &BTreeSet::new());
        assert_eq!(summary, None, "a venue nothing can reach was counted");
        assert!(problems.is_empty());
        // The premise: the same ladder does report when the venue is
        // reachable.
        assert!(review(&ladder, &reachable(&["AAAA"])).0.is_some());
    }

    #[test]
    fn standings_report_the_rung_name_blueprint_34_4_uses_rather_than_the_strategy_ladders() {
        let mut ladder = VenueLadder::new();
        admit(&mut ladder, "AAAA");
        let standings = standings(&ladder, &reachable(&["AAAA", "BBBB"]));
        assert_eq!(standings.len(), 2, "the premise: both venues are walked");
        assert_eq!(standings[0].rung, "simulated");
        assert!(standings[0].admitted);
        assert!(standings[0].registered);
        assert_eq!(standings[1].rung, "registered");
        assert!(!standings[1].admitted);
        assert!(!standings[1].registered);
    }
}
