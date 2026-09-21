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
//! # What calls this, and what feeds it
//!
//! `Platform` holds a [`VenueLadder`] (`grep -n 'venue_ladder' backend/crates/runtime/qip-kernel/src/platform.rs`)
//! and `stage_learn` calls [`review`] on it every cycle beside `review_venues`.
//! This paragraph said "nothing calls this yet" from the day the module was
//! written until 2026-09-19, by which time the field and the call had both
//! landed; a module that reads as unwired while it runs is the mirror image
//! of the failure the original sentence was written to prevent.
//!
//! The sentence that stood here until 2026-09-21 — that the ladder "is
//! constructed empty and has no production writer", because
//! [`qip_lifecycle::venue_ladder::attempt_promotion`] was reached only from
//! this module's tests — is no longer true either.
//! [`crate::venue_measurement::measure`] is that writer: it reads the desk
//! broker's own [`qip_execution_engine::observation::VenueObservation`],
//! builds the declaration and the measurement from it, seats the venue at the
//! registered rung and asks the ladder for the rung above, once per cycle
//! from `stage_learn` immediately before [`review`] runs.
//!
//! What is still true is narrower and is the reason [`review`] takes an
//! `unmeasured` set: the desk's venue does not time its acknowledgements, so
//! it is held at the registered rung by a fact this deployment cannot
//! produce rather than by evidence that fell short, and the two are reported
//! differently. Nothing is filled in to close that gap — see
//! [`crate::venue_measurement`]'s header for why a defaulted zero would
//! promote the venue *because* nobody had measured it.

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
/// `unmeasured` names the venues whose next rung turns on a fact this
/// deployment cannot produce — [`crate::venue_measurement::measure`] fills it
/// — and they are reported in the summary rather than as problems. The
/// distinction is the same one this module already draws between a venue
/// mid-ladder and a venue nobody registered, applied one level deeper: a
/// venue held below a rung by evidence that fell short is somebody's work,
/// and a venue held there because its adapter does not time itself is a
/// property of the build. Raising the second as a problem would put one on
/// every cycle of every deployment, which is how an operator learns that
/// problems are noise — the failure this module was corrected for once
/// already, on the empty-ladder branch below.
pub fn review(
    ladder: &VenueLadder,
    reachable: &BTreeSet<String>,
    unmeasured: &BTreeSet<String>,
) -> (Option<String>, Vec<String>) {
    let standings = standings(ladder, reachable);
    let admitted = standings
        .iter()
        .filter(|standing| standing.admitted)
        .count();
    let held_unmeasured = standings
        .iter()
        .filter(|standing| !standing.admitted && unmeasured.contains(&standing.venue))
        .count();
    let mut problems = Vec::new();
    for standing in &standings {
        if standing.admitted {
            continue;
        }
        if unmeasured.contains(&standing.venue) {
            continue;
        }
        if standing.registered {
            problems.push(format!(
                "venue {} is reachable and stands at the {} rung; §34.4 admits a venue to the \
                 simulator only once its declared fees, latency and order types have been \
                 verified against measurement and a replay has reconciled",
                standing.venue, standing.rung
            ));
        } else if ladder.is_empty() {
            // An entirely empty ladder is a deployment whose venues cannot
            // declare themselves at all — every adapter reporting no
            // observation across the port — rather than a fault in any one of
            // them. This comment said "nothing feeds `VenueLadder` yet" until
            // `crate::venue_measurement` became that feed; the branch is kept
            // because an adapter that reports nothing still lands here.
            // Reporting it as a *problem* put one on every cycle
            // of every deployment, which is how an operator learns that
            // problems are noise — and both
            // `a_platform_with_no_data_is_legible_rather_than_merely_quiet` and
            // the strategy-retirement test failed on exactly that when this
            // review was first wired. It belongs in the summary below, where it
            // is visible and is not an alarm.
            continue;
        } else {
            // A ladder that holds *something* and not this venue is different
            // in kind: somebody has begun declaring venues and this one was
            // missed, which is a person's oversight rather than an unbuilt
            // feature. That is worth a problem.
            problems.push(format!(
                "venue {} is reachable and the promotion ladder holds no record of it, though it \
                 holds {} other venue(s); §34.4 admits a venue on measurement and this one is \
                 being used on its own documentation",
                standing.venue,
                ladder.len()
            ));
        }
    }
    // Never `None`. A review that says nothing when nothing has been admitted
    // is indistinguishable from a review nobody wired in, and with an empty
    // ladder that silence would be every cycle of every deployment — the
    // failure three modules shipped in one day earlier in this wave.
    let summary = Some(if standings.is_empty() {
        "no venue is reachable, so none was assessed against the promotion ladder".to_string()
    } else if ladder.is_empty() {
        format!(
            // This read "nothing declares a venue to them yet" until the
            // measurement seam landed, at which point the desk's venue was
            // being declared on every cycle and the sentence was false for
            // the one deployment that exists. An empty ladder now means the
            // measurement pass found no venue that could declare itself at
            // all, which is a different and rarer state.
            "{} reachable venue(s) stand against an empty promotion ladder: §34.4's gates are \
             built and no reachable venue has declared itself to them",
            standings.len()
        )
    } else if held_unmeasured > 0 {
        // Said out loud rather than folded into the count, because "0 of 1
        // cleared" and "0 of 1 cleared, and the one is held by a fact nothing
        // here measures" are different states and only the second is
        // permanent. A reader who cannot tell them apart will wait for a
        // number that is never going to move.
        format!(
            "{admitted} of {} reachable venue(s) have cleared the simulated rung; {held_unmeasured} \
             of them stand below a rung this deployment cannot measure them past",
            standings.len()
        )
    } else {
        format!(
            "{admitted} of {} reachable venue(s) have cleared the simulated rung",
            standings.len()
        )
    });
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
                fee_bps: Some(Decimal::from_int(10)),
                latency: Some(Duration::from_millis(50)),
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
        let (summary, problems) = review(&ladder, &reachable(&["XPOOL"]), &BTreeSet::new());
        // This asserted `None` and one problem until the review was wired into
        // `stage_learn`. Both were wrong in the same way: an empty ladder is
        // this platform's configured state — nothing declares a venue to it —
        // so a problem here landed on every cycle of every deployment, and a
        // `None` summary meant the review reached no surface at all in that
        // same state. Two acceptance tests failed on the first and nothing
        // would have caught the second.
        assert!(problems.is_empty(), "{problems:?}");
        let summary = summary.expect("an empty ladder is reported, not passed over");
        assert!(summary.contains("empty promotion ladder"), "{summary}");
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
        let (summary, problems) = review(&ladder, &reachable(&["XPOOL"]), &BTreeSet::new());
        // The summary asserted `None` here because nothing had been *admitted*.
        // Counting zero out of one is the fact, and saying nothing was the
        // defect: it made a review that ran indistinguishable from one nobody
        // called.
        assert_eq!(
            summary.as_deref(),
            Some("0 of 1 reachable venue(s) have cleared the simulated rung")
        );
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
        let (summary, problems) = review(&ladder, &reachable(&["XPOOL"]), &BTreeSet::new());
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
        let (summary, problems) = review(
            &ladder,
            &reachable(&["AAAA", "BBBB", "CCCC"]),
            &BTreeSet::new(),
        );
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
    fn a_venue_held_below_a_rung_nothing_can_measure_is_summarised_and_never_raised_as_a_problem() {
        // The distinction that keeps this review usable now that something
        // feeds the ladder. The desk's own venue is registered and cannot be
        // measured past that rung, on every cycle, for as long as its adapter
        // does not time itself — so reporting it as a problem would raise one
        // on every cycle of every deployment, which is the failure this
        // module was already corrected for once on the empty-ladder branch.
        let mut ladder = VenueLadder::new();
        attempt_promotion(
            &mut ladder,
            &declaration("AAAA"),
            &honest_evidence(),
            VenuePromotionPolicy::default(),
            None,
            "measured read-only",
            now(),
        )
        .expect("the premise: a second venue is genuinely mid-ladder");
        let unmeasured: BTreeSet<String> = ["BBBB".to_string()].into_iter().collect();
        let (summary, problems) = review(&ladder, &reachable(&["AAAA", "BBBB"]), &unmeasured);
        // One problem, not two: the measurable venue that fell short.
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("venue AAAA"), "{}", problems[0]);
        let summary = summary.expect("a review that ran says so");
        assert!(
            summary.contains("cannot measure them past"),
            "the summary does not distinguish a permanent gap from a transient one: {summary}"
        );

        // The premise for the exclusion itself: the same ladder and the same
        // reachable set with nothing excused raises the second problem, so
        // the omission above is the `unmeasured` set and not an accident of
        // the fixture.
        let (_, both) = review(&ladder, &reachable(&["AAAA", "BBBB"]), &BTreeSet::new());
        assert_eq!(both.len(), 2, "{both:?}");
    }

    #[test]
    fn a_venue_the_platform_cannot_reach_is_not_reviewed_however_it_stands() {
        // The review is about what the platform can reach, not about every
        // venue the ladder has an opinion on. A ladder entry for a venue no
        // broker names is not a problem on this cycle.
        let mut ladder = VenueLadder::new();
        admit(&mut ladder, "AAAA");
        let (summary, problems) = review(&ladder, &BTreeSet::new(), &BTreeSet::new());
        assert!(problems.is_empty());
        let summary = summary.expect("a cycle that assessed nothing says so");
        assert!(
            summary.contains("no venue is reachable"),
            "a venue nothing can reach was counted, or the review fell silent: {summary}"
        );
        // The premise: the same ladder does report when the venue is
        // reachable.
        assert!(
            review(&ladder, &reachable(&["AAAA"]), &BTreeSet::new())
                .0
                .is_some()
        );
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
