//! Blueprint §34.4's measurement seam: the production caller
//! [`qip_lifecycle::venue_ladder::attempt_promotion`] never had.
//!
//! The ladder and its gates were built, wired into the LEARN stage's review,
//! and fed nothing. `PlatformConfig` carried no venue declaration and the
//! facts a venue would be verified against — how long it took to answer,
//! which order types it honoured, what it actually charged — lived in the
//! adapter layer with no port to cross. So every reachable venue reported as
//! never registered and no gate in `qip-lifecycle` had ever run against a
//! venue this platform trades with.
//!
//! This module is the composition that closes that. It reads one
//! [`VenueObservation`] from the broker port, turns the declaration half into
//! a [`VenueDeclaration`] and the measurement half into a
//! [`VenueMeasurement`], seats the venue at §34.4's registered rung, and asks
//! the ladder for the rung above.
//!
//! # What it refuses to supply, and why that is the point
//!
//! **It fills in nothing.** Where the adapter reports no figure, the
//! measurement carries [`None`] and the gate refuses the rung by name. It
//! would be one line to default a missing acknowledgement latency to zero and
//! the venue would climb on the first cycle — and it would climb *because*
//! nobody had measured it, since zero is faster than any declaration a venue
//! can make. A ladder fed a fabricated measurement is worse than an empty
//! one: an empty ladder is visibly empty, and a ladder full of venues
//! promoted on defaults reads exactly like a ladder full of venues that
//! earned it.
//!
//! One fact this deployment cannot supply at all, named here rather than
//! papered over. This paragraph said **two** until the session-replay lane,
//! and the second is gone rather than reworded:
//!
//! * **Acknowledgement latency from the desk's own venue.**
//!   `SimulatedBroker` stamps a configured latency on its fills rather than
//!   timing them, so a latency read back out of it is the declaration with
//!   extra steps — a number checking itself, which is the exact fallacy the
//!   observed rung exists to catch. It reports [`None`] and does not clear
//!   the rung. `SimulatedExchange` in `qip-brokers` *does* measure one,
//!   because its round trip carries a jitter drawn from the seed; that
//!   adapter is not what the kernel composes today.
//!
//! **A replayed session now exists, and used to be the second bullet.**
//! §34.4's simulated rung wants recorded sessions traded through the
//! simulator and reconciled; `crate::session_replay` produces the
//! [`SimulationEvidence`] from the sessions `OrderManager` seals, and this
//! module takes it as an argument rather than building one. It still fills in
//! nothing: where the replay reports no evidence, [`None`] arrives here and
//! the simulated rung refuses by name, exactly as an absent latency does at
//! the rung below.
//!
//! [`SimulationEvidence`]: qip_lifecycle::venue_ladder::SimulationEvidence
//!
//! # What it cannot do
//!
//! It cannot promote a venue past the simulator, and that is structural
//! rather than a rule this module follows.
//! [`qip_lifecycle::venue_ladder::attempt_promotion`] computes its target
//! from the rung below and refuses anything above
//! [`qip_lifecycle::venue_ladder::VENUE_PROMOTION_CEILING`]; there is no
//! parameter here or there naming a target, and nothing in this module
//! reaches the order path, an autonomy level or a broker's submit. A venue
//! at the ceiling of this ladder is a venue the *simulator* may use, and the
//! desk's submission ladder refuses a non-simulated broker below a live
//! autonomy level entirely independently of anything recorded here.

use qip_contracts::gate::GateStage;
use qip_core::Timestamp;
use qip_execution_engine::observation::VenueObservation;
use qip_lifecycle::venue_ladder::{
    SimulationEvidence, VenueDeclaration, VenueEvidence, VenueLadder, VenueMeasurement,
    VenuePromotionPolicy, attempt_promotion, rung_name,
};
use std::collections::BTreeSet;

/// What one cycle's measurement pass found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueMeasurementOutcome {
    /// One line for the LEARN stage's record. Never [`None`] when a venue was
    /// looked at: a pass that ran and said nothing is indistinguishable from
    /// a pass nobody wired in.
    pub summary: Option<String>,
    /// Findings an operator can act on. Deliberately short: a venue held
    /// below a rung by a fact this deployment cannot produce is *not* a
    /// problem, because it would be one on every cycle of every deployment
    /// and a control that fires always is how an operator learns that
    /// problems are noise. Those venues come back in [`Self::unmeasured`]
    /// instead and are reported in a summary.
    pub problems: Vec<String>,
    /// Venues whose next rung turns on a fact nothing here can measure.
    pub unmeasured: BTreeSet<String>,
}

/// Read a venue's own report, seat it on the ladder, and ask for one rung.
///
/// One rung per cycle, because that is the ladder's own discipline: a gate is
/// evidence for the rung above where a venue stands and nothing else, and a
/// caller that looped until refusal would be walking a venue up on a single
/// pass of evidence.
pub fn measure(
    ladder: &mut VenueLadder,
    venue: &str,
    observation: Option<VenueObservation>,
    simulation: Option<SimulationEvidence>,
    policy: VenuePromotionPolicy,
    now: Timestamp,
) -> VenueMeasurementOutcome {
    let mut unmeasured = BTreeSet::new();

    let Some(observation) = observation else {
        // The adapter reports no tally at all. Not a problem and not silence:
        // the venue is being used on its own documentation, which is §34.4's
        // opening complaint, and nothing an operator does today changes it.
        unmeasured.insert(venue.to_string());
        return VenueMeasurementOutcome {
            summary: Some(format!(
                "venue {venue} reports no measurement of itself at all, so §34.4's observed rung \
                 cannot be evaluated and it stands at the {} rung on its own documentation",
                rung_name(GateStage::Candidate)
            )),
            problems: Vec::new(),
            unmeasured,
        };
    };

    let name = observation.venue.as_str().to_string();
    let declared = &observation.declared;
    let declaration = match VenueDeclaration::new(
        observation.venue.clone(),
        declared.class,
        declared.fee_bps,
        declared.acknowledgement_latency,
        declared.order_types.clone(),
    ) {
        Ok(declaration) => declaration,
        Err(error) => {
            // A declaration this venue's own adapter could not make is
            // §34.4's registered rung failing, and it is a real problem: an
            // adapter shipping a negative fee or an empty order-type list is
            // somebody's defect rather than a fact of the deployment.
            return VenueMeasurementOutcome {
                summary: None,
                problems: vec![format!(
                    "venue {name} cannot be registered on the promotion ladder: {}",
                    error.message()
                )],
                unmeasured,
            };
        }
    };

    ladder.register(&declaration, now);

    let observed = &observation.observed;
    // Straight through. Every `None` here is an adapter saying it does not
    // hold the fact, and it stays `None` all the way to the gate that refuses
    // on it. Nothing in this function supplies a figure.
    let measurement = VenueMeasurement {
        fee_bps: observed.fee_bps(),
        latency: observed.acknowledgement_latency,
        accepted_order_types: observed.accepted_order_types.clone(),
        rejected_order_types: observed.rejected_order_types.clone(),
        observations: observed.acknowledgements,
    };
    let mut missing: Vec<&'static str> = Vec::new();
    if measurement.latency.is_none() {
        missing.push("the acknowledgement latency, which this venue does not time");
    }
    if measurement.fee_bps.is_none() {
        missing.push(
            "the fee actually charged, which this venue does not itemise over a filled \
                      notional",
        );
    }

    let standing = ladder.stage_of(&name);
    // The simulated rung wants recorded sessions replayed and reconciled.
    // A venue standing at the observed rung with no replay behind it is held
    // there by a corpus the deployment has not produced *yet* rather than by
    // evidence that fell short — a venue nobody has sent an order to, or one
    // whose sessions all filled nothing — and that belongs in the summary
    // rather than in the problems, which is why it is listed beside the two
    // facts the adapter cannot take.
    if standing == GateStage::Holdout && simulation.is_none() {
        missing.push(
            "a recorded session that can be replayed into promotion evidence, which needs \
             sessions in which the venue actually filled something",
        );
    }
    if !missing.is_empty() {
        unmeasured.insert(name.clone());
    }

    let evidence = VenueEvidence::new().with_measurement(measurement);
    // Straight through again. Where the replay produced no evidence this
    // stays absent and the gate refuses by name; nothing here substitutes a
    // zeroed `SimulationEvidence`, which would present a venue nobody
    // replayed as one whose replay found no break.
    let evidence = match simulation {
        Some(simulation) => evidence.with_simulation(simulation),
        None => evidence,
    };
    let outcome = attempt_promotion(
        ladder,
        &declaration,
        &evidence,
        policy,
        // No approver. This pass authorises nothing; the gate decides, and a
        // name here would read as an approval nobody gave.
        None,
        "measured at the broker port during the LEARN stage",
        now,
    );

    match outcome {
        Ok(promotion) => VenueMeasurementOutcome {
            summary: Some(format!(
                "venue {name} cleared the {} rung on measurement taken at the broker port",
                rung_name(promotion.to)
            )),
            problems: Vec::new(),
            unmeasured,
        },
        Err(_) if !missing.is_empty() => VenueMeasurementOutcome {
            summary: Some(format!(
                "venue {name} stands at the {} rung and cannot be measured past it: this \
                 deployment does not produce {}",
                rung_name(standing),
                missing.join("; nor ")
            )),
            problems: Vec::new(),
            unmeasured,
        },
        Err(error) => VenueMeasurementOutcome {
            summary: Some(format!(
                "venue {name} stands at the {} rung",
                rung_name(standing)
            )),
            // Everything measurable was measured and the venue still fell
            // short. That is a finding somebody can act on — send more
            // orders, exercise the untried types, or take the venue's
            // documentation back to the venue.
            problems: vec![error.message().to_string()],
            unmeasured,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::venue::{VenueClass, VenueId};
    use qip_core::{Decimal, Duration};
    use qip_execution_engine::broker::{Broker, SimulatedBroker, SimulationSettings};
    use qip_execution_engine::observation::{DeclaredVenueProfile, ObservedVenueFacts};
    use qip_execution_engine::order::{Order, OrderType, Side};
    use qip_lifecycle::venue_ladder::VENUE_PROMOTION_CEILING;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    fn order_types() -> BTreeSet<String> {
        ["limit".to_string(), "market".to_string()]
            .into_iter()
            .collect()
    }

    /// Evidence of the shape `crate::session_replay` produces from a window
    /// of clean recorded sessions.
    fn clean_simulation() -> qip_lifecycle::venue_ladder::SimulationEvidence {
        qip_lifecycle::venue_ladder::SimulationEvidence {
            replayed_sessions: 8,
            reconciliation_breaks: 0,
            reference_clip: Decimal::from_int(10),
        }
    }

    /// An adapter that reports everything §34.4's observed rung asks for.
    fn complete_observation() -> VenueObservation {
        VenueObservation {
            venue: VenueId::new("XVENUE"),
            declared: DeclaredVenueProfile {
                class: VenueClass::Exchange,
                fee_bps: Decimal::from_int(10),
                acknowledgement_latency: Duration::from_millis(50),
                order_types: order_types(),
            },
            observed: ObservedVenueFacts {
                acknowledgements: 40,
                acknowledgement_latency: Some(Duration::from_millis(50)),
                fees_charged: Some(Decimal::from_int(10)),
                notional_filled: Some(Decimal::from_int(100_000)),
                accepted_order_types: order_types(),
                rejected_order_types: BTreeSet::new(),
            },
        }
    }

    #[test]
    fn a_venue_that_does_not_time_itself_is_held_at_the_registered_rung_and_is_not_a_problem() {
        // The refusal this seam exists to make. The adapter reports
        // everything except the one fact it cannot honestly claim, and the
        // ladder refuses rather than reading the absence as instant.
        let mut observation = complete_observation();
        observation.observed.acknowledgement_latency = None;
        let mut ladder = VenueLadder::new();
        let outcome = measure(
            &mut ladder,
            "XVENUE",
            Some(observation),
            None,
            VenuePromotionPolicy::default(),
            now(),
        );
        assert_eq!(
            ladder.stage_of("XVENUE"),
            GateStage::Candidate,
            "an untimed venue was promoted"
        );
        assert!(
            ladder.knows("XVENUE"),
            "the venue was not even registered, so the refusal above is about the wrong thing"
        );
        assert!(
            outcome.problems.is_empty(),
            "a gap this deployment cannot close was raised as a problem on every cycle: {:?}",
            outcome.problems
        );
        assert!(outcome.unmeasured.contains("XVENUE"));
        let summary = outcome.summary.expect("a pass that ran says so");
        assert!(
            summary.contains("does not time"),
            "the summary does not name the missing fact: {summary}"
        );

        // The premise, and the half that proves this is a gate rather than a
        // refusal generator: the same venue with the one fact supplied moves.
        let mut moved = VenueLadder::new();
        let admitted = measure(
            &mut moved,
            "XVENUE",
            Some(complete_observation()),
            None,
            VenuePromotionPolicy::default(),
            now(),
        );
        assert_eq!(moved.stage_of("XVENUE"), GateStage::Holdout);
        assert!(admitted.problems.is_empty(), "{:?}", admitted.problems);
    }

    #[test]
    fn the_measurement_seam_cannot_walk_a_venue_past_the_simulator_on_any_number_of_cycles() {
        // The ceiling, exercised through the production seam rather than
        // through a bare `attempt_promotion`: a venue reporting perfect
        // evidence, measured on twenty consecutive cycles, never reaches a
        // rung that holds capital. It stops at the observed rung because
        // nothing here replays a session, and even if it did, the ceiling is
        // the simulator.
        let mut ladder = VenueLadder::new();
        for _ in 0..20 {
            measure(
                &mut ladder,
                "XVENUE",
                Some(complete_observation()),
                Some(clean_simulation()),
                VenuePromotionPolicy::default(),
                now(),
            );
        }
        let reached = ladder.stage_of("XVENUE");
        assert!(
            reached <= VENUE_PROMOTION_CEILING,
            "the measurement seam promoted a venue past the simulator: {}",
            rung_name(reached)
        );
        assert!(
            !reached.holds_capital(),
            "the rung this seam reached holds capital: {}",
            rung_name(reached)
        );
        assert!(
            !reached.may_reach_a_venue(),
            "the rung this seam reached may reach a venue: {}",
            rung_name(reached)
        );
        // And the premise: it did move, and it moved all the way to the
        // ceiling. Before the session replay existed this assertion read
        // `Holdout`, because no `SimulationEvidence` was constructed
        // anywhere and the rung below the ceiling was as far as any venue
        // could go — which made the bound above true of a ceiling nothing
        // could reach.
        assert_eq!(reached, VENUE_PROMOTION_CEILING);
    }

    #[test]
    fn a_venue_with_no_replayed_session_is_held_below_the_ceiling_and_is_not_a_problem() {
        // The other side of the seam's classification. A venue that measured
        // as it declared earns the observed rung and then stops, because the
        // rung above turns on a corpus this deployment has not produced for
        // it. That is reported and it is not raised: it would be raised on
        // every cycle until the venue had traded, and a control that fires
        // always teaches an operator to ignore it.
        let mut ladder = VenueLadder::new();
        for _ in 0..20 {
            measure(
                &mut ladder,
                "XVENUE",
                Some(complete_observation()),
                None,
                VenuePromotionPolicy::default(),
                now(),
            );
        }
        assert_eq!(
            ladder.stage_of("XVENUE"),
            GateStage::Holdout,
            "a venue with no replayed session reached the simulated rung"
        );
        let outcome = measure(
            &mut ladder,
            "XVENUE",
            Some(complete_observation()),
            None,
            VenuePromotionPolicy::default(),
            now(),
        );
        assert!(
            outcome.problems.is_empty(),
            "a corpus the deployment has not gathered yet was raised as a problem: {:?}",
            outcome.problems
        );
        assert!(outcome.unmeasured.contains("XVENUE"));
        let summary = outcome.summary.expect("a pass that ran says so");
        assert!(
            summary.contains("recorded session"),
            "the summary does not name the missing fact: {summary}"
        );
    }

    #[test]
    fn the_desks_own_simulated_broker_crosses_the_port_and_is_refused_on_its_untimed_latency() {
        // The whole path in one test, across the crate boundary it spans: a
        // real `SimulatedBroker`, driven through real orders, read through
        // the widened port, judged by `qip-lifecycle`'s gate.
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
        for index in 0..3u32 {
            let order = Order::new(
                qip_core::ids::OrderId::from_string(format!("order-{index}")),
                qip_core::ids::ObjectId::from_string("OBJ"),
                Side::Buy,
                Decimal::from_int(10),
                OrderType::Market,
                Decimal::from_int(100),
                "proposal",
                vec!["hypothesis".to_string()],
                "scope",
                now(),
            );
            broker
                .submit(&order, now())
                .expect("the frictionless venue never refuses");
        }
        let observation = broker
            .observation()
            .expect("the desk broker reports a tally");
        assert_eq!(
            observation.observed.acknowledgements, 3,
            "the premise: the venue answered three instructions"
        );
        assert!(
            observation.observed.accepted_order_types.contains("market"),
            "the venue did not record the type it accepted"
        );
        assert!(
            observation.observed.acknowledgement_latency.is_none(),
            "the desk broker claimed to have timed acknowledgements it only stamps"
        );

        let mut ladder = VenueLadder::new();
        let outcome = measure(
            &mut ladder,
            broker.name(),
            Some(observation),
            None,
            VenuePromotionPolicy::default(),
            now(),
        );
        assert_eq!(
            ladder.stage_of(broker.name()),
            GateStage::Candidate,
            "the desk's own venue was promoted on a latency nobody measured"
        );
        assert!(ladder.knows(broker.name()));
        assert!(outcome.unmeasured.contains(broker.name()));
    }

    #[test]
    fn a_venue_whose_adapter_reports_nothing_is_said_out_loud_rather_than_passed_over() {
        let mut ladder = VenueLadder::new();
        let outcome = measure(
            &mut ladder,
            "XSILENT",
            None,
            None,
            VenuePromotionPolicy::default(),
            now(),
        );
        assert!(
            !ladder.knows("XSILENT"),
            "a venue that declared nothing was seated on the ladder"
        );
        assert!(outcome.problems.is_empty());
        assert!(outcome.unmeasured.contains("XSILENT"));
        let summary = outcome.summary.expect("a silent adapter is reported");
        assert!(
            summary.contains("own documentation"),
            "the summary does not say the venue is being used unverified: {summary}"
        );
    }

    #[test]
    fn a_venue_short_of_evidence_it_could_supply_is_a_problem_rather_than_an_excused_gap() {
        // The other side of the classification, and the reason it is not just
        // "never raise a problem": a venue that reports every fact and has
        // simply not been sent enough orders is something an operator acts
        // on, and it is raised.
        let mut observation = complete_observation();
        observation.observed.acknowledgements = 2;
        let mut ladder = VenueLadder::new();
        let outcome = measure(
            &mut ladder,
            "XVENUE",
            Some(observation),
            None,
            VenuePromotionPolicy::default(),
            now(),
        );
        assert_eq!(ladder.stage_of("XVENUE"), GateStage::Candidate);
        assert!(
            outcome.unmeasured.is_empty(),
            "a venue short of a sample it can still gather was excused as unmeasurable"
        );
        assert_eq!(outcome.problems.len(), 1, "{:?}", outcome.problems);
        assert!(
            outcome.problems[0].contains("sample_sufficient"),
            "the problem does not name the check that refused: {}",
            outcome.problems[0]
        );
    }
}
