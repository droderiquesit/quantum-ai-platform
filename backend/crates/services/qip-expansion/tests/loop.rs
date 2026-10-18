//! The expansion loop end to end as a library: detectors over readings, the
//! desk's admission and lifecycle, the plan and derivation each item carries,
//! and the promotion road (EXPAND-011, 025, 046..051, 063..069; REASON-3061,
//! 3062).
//!
//! Every test asserts its premise before its property: a detector test first
//! shows the reading would be read, then that the gap was raised.

use qip_agents::tools::{ToolKind, ToolRegistry};
use qip_contracts::expansion::{CapabilityVerb, GapClass};
use qip_core::Decimal;
use qip_expansion::campaigns::{self, TickCampaignDetector, TickCampaignTrigger};
use qip_expansion::curriculum::{Budget, State, TaskKind};
use qip_expansion::desk::{Admission, CloseReason, DeskConfig, ExpansionDesk, question_for};
use qip_expansion::detectors::{
    Decline, Origin, PersistenceTracker, Reading, Refusal, Residual, anomaly_clusters,
    contradictions, coverage, incompetent_specialists, low_confidence, refusal_clusters,
    repeated_vetoes, split_by_wanting, unexplained_residuals,
};
use qip_expansion::gap::{GapTrigger, Observation, Wanting, classify, raise};
use qip_expansion::plan::{AcquisitionAction, Environment, ResearchPlan, Step};
use qip_expansion::promotion::{
    Criterion, EvaluationRecord, Finding, Provenance, ResearchCampaign,
};
use std::collections::{BTreeMap, BTreeSet};

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

fn rates(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

fn class_of(observation: &Observation) -> GapClass {
    match classify(&observation.wanting) {
        Ok(class) => class,
        Err(e) => panic!("a detector produced an unclassifiable observation: {e}"),
    }
}

fn origin_of(observation: &Observation) -> &str {
    observation.evidence.first().map_or("", String::as_str)
}

// --- the six gap kinds of EXPAND-011 ----------------------------------------

#[test]
fn a_belief_class_resolving_below_the_floor_is_a_low_confidence_model_gap() {
    let read = rates(&[("momentum", 0.30), ("carry", 0.70)]);
    let raised = low_confidence(&read, 40, 20, 0.5).unwrap_or_default();
    assert_eq!(
        raised.len(),
        1,
        "only the class below the floor: {raised:?}"
    );
    assert_eq!(raised[0].trigger, GapTrigger::LowConfidence);
    assert_eq!(class_of(&raised[0]), GapClass::Model);
    assert_eq!(origin_of(&raised[0]), Origin::LowConfidence.as_str());
    // Too few resolutions to say anything: nothing raised.
    assert!(
        low_confidence(&read, 5, 20, 0.5)
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn an_agent_below_the_floor_is_a_capability_with_no_competent_specialist() {
    let read = rates(&[("macro-agent", 0.20), ("value-agent", 0.80)]);
    let raised = incompetent_specialists(&read, 50, 20, 0.5).unwrap_or_default();
    assert_eq!(raised.len(), 1, "{raised:?}");
    assert_eq!(class_of(&raised[0]), GapClass::Specialist);
    assert!(raised[0].observation.contains("`macro-agent`"));
}

#[test]
fn a_twin_missing_fills_past_its_threshold_is_an_unexplained_residual() {
    let residuals = vec![
        Residual {
            subject: "XNYS".into(),
            error_bps: 40.0,
        },
        Residual {
            subject: "XNYS".into(),
            error_bps: -60.0,
        },
        Residual {
            subject: "XLON".into(),
            error_bps: 2.0,
        },
    ];
    let raised = unexplained_residuals(&residuals, 25.0).unwrap_or_default();
    assert_eq!(
        raised.len(),
        1,
        "only XNYS's mean miss of 50 bps: {raised:?}"
    );
    assert_eq!(raised[0].trigger, GapTrigger::UnexplainedPnl);
    assert!(raised[0].observation.contains("`XNYS`"));
}

#[test]
fn two_sources_reading_one_subject_in_opposite_directions_is_a_missing_causal_link() {
    let readings = vec![
        Reading {
            subject: "ACME".into(),
            source: "return".into(),
            up: true,
        },
        Reading {
            subject: "ACME".into(),
            source: "break".into(),
            up: false,
        },
        Reading {
            subject: "BETA".into(),
            source: "return".into(),
            up: true,
        },
    ];
    let raised = contradictions(&readings);
    assert_eq!(raised.len(), 1, "BETA is read one way only: {raised:?}");
    assert_eq!(raised[0].trigger, GapTrigger::Contradiction);
    assert_eq!(class_of(&raised[0]), GapClass::Causal);
    // An even split is the strongest contradiction.
    assert!((raised[0].severity - 1.0).abs() < 1e-12);
}

#[test]
fn an_asset_class_with_no_history_is_raised_as_unsupported_and_a_covered_one_is_not() {
    let universe: BTreeMap<String, BTreeSet<String>> = BTreeMap::from([
        ("equity".to_string(), set(&["A", "B"])),
        ("rates".to_string(), set(&["R1", "R2"])),
    ]);
    let covered = set(&["A", "B"]);
    let raised = coverage(&universe, &covered, 0.5).unwrap_or_default();
    assert_eq!(raised.len(), 1, "{raised:?}");
    assert!(raised[0].observation.contains("`rates` is unsupported"));
    assert_eq!(class_of(&raised[0]), GapClass::Data);
    assert!((raised[0].severity - 1.0).abs() < 1e-12);
}

// --- failure-, opportunity- and anomaly-driven curriculum -------------------

#[test]
fn a_gate_whose_declines_repeatedly_would_have_earned_becomes_one_learning_objective() {
    let declines = vec![
        Decline {
            gate: "max_position".into(),
            regret: true,
        },
        Decline {
            gate: "max_position".into(),
            regret: true,
        },
        Decline {
            gate: "max_position".into(),
            regret: false,
        },
        Decline {
            gate: "liquidity".into(),
            regret: true,
        },
    ];
    let raised = repeated_vetoes(&declines, 2).unwrap_or_default();
    assert_eq!(raised.len(), 1, "{raised:?}");
    assert_eq!(origin_of(&raised[0]), Origin::RepeatedVeto.as_str());
    assert!((raised[0].severity - 2.0 / 3.0).abs() < 1e-12);
}

#[test]
fn a_venue_refusing_one_constraint_repeatedly_becomes_an_execution_gap() {
    let refusals = vec![
        Refusal {
            venue: "XNYS".into(),
            constraint: "lot_size".into(),
        },
        Refusal {
            venue: "XNYS".into(),
            constraint: "lot_size".into(),
        },
        Refusal {
            venue: "XLON".into(),
            constraint: "tick".into(),
        },
    ];
    let raised = refusal_clusters(&refusals, 2).unwrap_or_default();
    assert_eq!(raised.len(), 1);
    assert_eq!(class_of(&raised[0]), GapClass::Execution);
}

#[test]
fn a_discrepancy_persisting_for_the_stated_sweeps_raises_exactly_once_and_resets_on_absence() {
    let mut tracker = match PersistenceTracker::new(3) {
        Ok(t) => t,
        Err(e) => panic!("{e}"),
    };
    let acme = set(&["ACME"]);
    assert!(tracker.observe(&acme).is_empty());
    assert!(tracker.observe(&acme).is_empty());
    let third = tracker.observe(&acme);
    assert_eq!(third.len(), 1, "raised on the third consecutive sweep");
    assert_eq!(origin_of(&third[0]), Origin::PersistentDiscrepancy.as_str());
    assert!(
        tracker.observe(&acme).is_empty(),
        "a standing discrepancy is one task"
    );
    assert!(tracker.observe(&set(&[])).is_empty());
    assert_eq!(tracker.tracked(), 0, "absence resets the run");
}

#[test]
fn several_subjects_showing_one_symptom_surface_one_investigation_nobody_asked_for() {
    let symptom = |subject: &str| Observation {
        trigger: GapTrigger::Surprise,
        wanting: vec![Wanting::Unknown],
        observation: format!("price move in {subject}"),
        affected_domains: vec!["market".into()],
        evidence: vec!["price_move_detector".into()],
        severity: 0.6,
        economic_value: Decimal::ZERO,
    };
    let members: Vec<(String, Observation)> = ["A", "B", "C"]
        .iter()
        .map(|s| ((*s).to_string(), symptom(s)))
        .collect();
    // Premise: alone, each member cannot be classified.
    assert!(classify(&members[0].1.wanting).is_err());
    let clusters = anomaly_clusters(&members, 3).unwrap_or_default();
    assert_eq!(clusters.len(), 1, "{clusters:?}");
    assert_eq!(class_of(&clusters[0]), GapClass::Causal);
    assert_eq!(origin_of(&clusters[0]), Origin::AnomalyCluster.as_str());
    assert!(
        anomaly_clusters(&members[..2], 3)
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn an_observation_naming_two_lacks_becomes_two_classifiable_observations() {
    let mixed = Observation {
        trigger: GapTrigger::Surprise,
        wanting: vec![Wanting::CausalLink, Wanting::Source],
        observation: "unexplained move".into(),
        affected_domains: vec!["market".into()],
        evidence: vec!["d".into()],
        severity: 0.5,
        economic_value: Decimal::ZERO,
    };
    assert!(
        classify(&mixed.wanting).is_err(),
        "premise: mixed is refused"
    );
    let split = split_by_wanting(mixed);
    let classes: Vec<GapClass> = split.iter().map(class_of).collect();
    assert_eq!(classes, vec![GapClass::Data, GapClass::Causal]);
}

#[test]
fn lead_lag_and_reaction_residuals_open_a_campaign_past_their_threshold() {
    let mut detector = match TickCampaignDetector::new() {
        Ok(d) => d,
        Err(e) => panic!("{e}"),
    };
    let threshold = Decimal::from_int(5);
    let below = detector.observe_residual(
        TickCampaignTrigger::AbnormalLeadLag,
        Decimal::from_int(5),
        threshold,
        None,
        set(&["A"]),
    );
    assert!(matches!(below, Ok(None)));
    let above = detector.observe_residual(
        TickCampaignTrigger::WorldEventReaction,
        Decimal::from_int(-6),
        threshold,
        Some("XNYS"),
        set(&["A"]),
    );
    match above {
        Ok(Some(c)) => assert_eq!(c.trigger, TickCampaignTrigger::WorldEventReaction),
        other => panic!("expected a campaign, got {other:?}"),
    }
    assert!(
        detector
            .observe_residual(
                TickCampaignTrigger::NewVenue,
                Decimal::ONE,
                threshold,
                None,
                set(&[])
            )
            .is_err()
    );
}

/// RM-P9-08. A residual between what the twin replayed and what the venue
/// filled — "live" read as paper, as the roadmap reads it — past its
/// threshold opens a tick campaign, and the campaign becomes a curriculum
/// item the desk queues with the tick-replay plan rather than a number on a
/// counter nobody reads. Asserted through `raise` and `ExpansionDesk::offer`,
/// the road every other observation takes, so the item is the desk's and not
/// a list kept on the side.
///
/// Mutation: make `campaigns::observations` state nothing, so the campaign is
/// opened and dropped — the count fails and nothing is queued.
#[test]
fn a_residual_above_threshold_between_replay_and_live_opens_a_curriculum_item() {
    let mut detector = match TickCampaignDetector::new() {
        Ok(d) => d,
        Err(e) => panic!("{e}"),
    };
    let threshold = Decimal::from_int(25);
    // Premise: at its threshold the residual opens nothing, so the item below
    // is the residual's doing and not the detector's default.
    assert!(matches!(
        detector.observe_residual(
            TickCampaignTrigger::FillPredictionError,
            threshold,
            threshold,
            Some("XNYS"),
            set(&["AAPL"]),
        ),
        Ok(None)
    ));
    let campaign = match detector.observe_residual(
        TickCampaignTrigger::FillPredictionError,
        Decimal::from_int(40),
        threshold,
        Some("XNYS"),
        set(&["AAPL"]),
    ) {
        Ok(Some(c)) => c,
        other => panic!("premise: 40 past 25 opens a campaign: {other:?}"),
    };

    let mut stated = match campaigns::observations(std::slice::from_ref(&campaign)) {
        Ok(stated) => stated,
        Err(e) => panic!("{e}"),
    };
    assert_eq!(stated.len(), 1, "one campaign, one observation: {stated:?}");
    let raised = match raise(stated.remove(0)) {
        Ok(r) => r,
        Err(e) => panic!("a stated campaign must classify: {e}"),
    };
    let question = question_for(&raised);

    // The tools are registered so the admission is decided by the item, not
    // by a missing tool; that path has its own test below.
    let mut desk = desk_with(&[
        ("tick-replay", ToolKind::Simulator),
        ("venue-adapter", ToolKind::VenueAdapter),
    ]);
    assert!(desk.queue().is_empty(), "premise: an empty desk");
    assert_eq!(
        desk.offer(raised, &set(&[]), &set(&["US"])),
        Admission::Queued
    );
    let entry = desk
        .queue()
        .entry(&question)
        .unwrap_or_else(|| panic!("the campaign's question is queued: {question}"));
    assert_eq!(entry.kind(), TaskKind::Campaign);
    let lineage = desk
        .lineage(&question)
        .unwrap_or_else(|| panic!("a queued item keeps its lineage"));
    assert_eq!(lineage.class, GapClass::Execution);
    assert_eq!(lineage.action, AcquisitionAction::ReplayTicks);
    assert_eq!(lineage.derivation.rule, campaigns::ORIGIN);
    let cited = format!("tick_campaign:{}:40/25", campaign.campaign_id);
    assert!(
        lineage.derivation.evidence.contains(&cited),
        "the item cites the campaign that opened it: {:?}",
        lineage.derivation.evidence
    );
}

// --- the desk ---------------------------------------------------------------

fn desk_with(tools: &[(&str, ToolKind)]) -> ExpansionDesk {
    let mut registry = ToolRegistry::new();
    for (name, kind) in tools {
        if let Err(e) = registry.register(name, *kind) {
            panic!("{e}");
        }
    }
    match ExpansionDesk::new(
        DeskConfig {
            quiet_sweeps: 2,
            max_started_sweeps: 5,
            jurisdiction: "US".into(),
            owner: "test".into(),
        },
        registry,
    ) {
        Ok(d) => d,
        Err(e) => panic!("{e}"),
    }
}

fn low_confidence_gap() -> qip_expansion::gap::RaisedGap {
    let read = rates(&[("momentum", 0.2)]);
    let observation = low_confidence(&read, 40, 20, 0.5)
        .unwrap_or_default()
        .pop()
        .unwrap_or_else(|| panic!("premise: the reading raises a gap"));
    match raise(observation) {
        Ok(r) => r,
        Err(e) => panic!("{e}"),
    }
}

fn budget(units: i64) -> Budget {
    Budget {
        compute: Decimal::from_int(units),
        capital: Decimal::from_int(units),
    }
}

#[test]
fn a_raised_gap_is_queued_once_with_its_derivation_and_plan_and_a_repeat_is_a_duplicate() {
    let mut desk = desk_with(&[("model-fit", ToolKind::ModelEndpoint)]);
    let us = set(&["US"]);
    let raised = low_confidence_gap();
    let question = question_for(&raised);
    assert_eq!(
        desk.offer(raised.clone(), &set(&[]), &us),
        Admission::Queued
    );
    assert_eq!(desk.offer(raised, &set(&[]), &us), Admission::Duplicate);
    assert_eq!(desk.queue().len(), 1);
    let lineage = desk
        .lineage(&question)
        .unwrap_or_else(|| panic!("a queued item keeps its lineage"));
    assert_eq!(lineage.derivation.rule, Origin::LowConfidence.as_str());
    assert_eq!(lineage.derivation.violated_assumptions.len(), 1);
    assert_eq!(
        lineage.plan.steps.first(),
        Some(&Step::Acquire {
            action: lineage.action
        })
    );
}

#[test]
fn a_gap_in_an_ineligible_jurisdiction_is_refused_rather_than_ranked_low() {
    let mut desk = desk_with(&[("model-fit", ToolKind::ModelEndpoint)]);
    let admission = desk.offer(low_confidence_gap(), &set(&[]), &set(&["EU"]));
    assert!(
        matches!(admission, Admission::Refused { .. }),
        "{admission:?}"
    );
    assert!(desk.queue().is_empty());
}

#[test]
fn a_gap_whose_tool_is_missing_is_blocked_and_the_tool_becomes_a_build_task_of_its_own() {
    let mut desk = desk_with(&[]);
    let admission = desk.offer(low_confidence_gap(), &set(&[]), &set(&["US"]));
    assert_eq!(
        admission,
        Admission::Blocked {
            tools: vec!["model-fit".to_string()]
        }
    );
    let kinds: Vec<TaskKind> = desk.queue().ranked().iter().map(|e| e.kind()).collect();
    assert!(kinds.contains(&TaskKind::ToolBuild), "{kinds:?}");
    assert_eq!(desk.queue().proposals().len(), 1);
}

#[test]
fn a_started_item_whose_gap_goes_quiet_closes_and_an_unstarted_one_is_withdrawn() {
    let mut desk = desk_with(&[("model-fit", ToolKind::ModelEndpoint)]);
    let us = set(&["US"]);
    let raised = low_confidence_gap();
    let question = question_for(&raised);
    desk.offer(raised, &set(&[]), &us);
    let first = desk.finish_sweep(budget(10)).unwrap_or_default();
    assert_eq!(first.started, vec![question.clone()]);
    // Two sweeps with nothing raised.
    let second = desk.finish_sweep(budget(10)).unwrap_or_default();
    assert!(second.closed.is_empty(), "one quiet sweep is not two");
    let third = desk.finish_sweep(budget(10)).unwrap_or_default();
    assert_eq!(third.closed.len(), 1, "{third:?}");
    assert_eq!(third.closed[0].reason, CloseReason::GapQuiet);
    assert!(desk.queue().is_empty());

    // Unstarted: a zero budget starts nothing, and quiet withdraws it.
    let mut desk = desk_with(&[("model-fit", ToolKind::ModelEndpoint)]);
    desk.offer(low_confidence_gap(), &set(&[]), &us);
    for _ in 0..2 {
        let _ = desk.finish_sweep(budget(0));
    }
    let r = desk.finish_sweep(budget(0)).unwrap_or_default();
    assert_eq!(r.withdrawn.len(), 1, "{r:?}");
    assert!(desk.queue().is_empty());
}

// --- plan, campaign, evaluation, promotion ----------------------------------

#[test]
fn a_synthesised_plan_builds_a_missing_tool_first_and_runs_the_four_environments_in_order() {
    let plan = ResearchPlan::synthesise("q", GapClass::Execution, false);
    assert!(matches!(plan.steps.first(), Some(Step::BuildTool { tool }) if tool == "tick-replay"));
    let environments: Vec<Environment> = plan
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Run { environment } => Some(*environment),
            _ => None,
        })
        .collect();
    assert_eq!(environments, Environment::ORDER.to_vec());
    assert_eq!(plan.steps.last(), Some(&Step::Decide));
    let ready = ResearchPlan::synthesise("q", GapClass::Execution, true);
    assert!(matches!(ready.steps.first(), Some(Step::Acquire { .. })));
}

fn provenance() -> Provenance {
    Provenance {
        dataset_digest: "sha256:abc".into(),
        lineage: vec!["tape:1".into()],
        seed: 7,
    }
}

fn complete_campaign(question: &str) -> ResearchCampaign {
    let mut c = ResearchCampaign::open(question).unwrap_or_else(|e| panic!("{e}"));
    for env in Environment::ORDER {
        c.record(env, provenance())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    c
}

fn evaluation(question: &str, failing: Option<Criterion>) -> EvaluationRecord {
    let findings = Criterion::ALL
        .iter()
        .map(|c| {
            (
                *c,
                Finding {
                    passed: Some(*c) != failing,
                    evidence: format!("eval:{c:?}"),
                },
            )
        })
        .collect();
    EvaluationRecord::new(question, findings).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn a_campaign_refuses_a_stage_out_of_order_and_one_with_no_provenance() {
    let mut c = ResearchCampaign::open("q").unwrap_or_else(|e| panic!("{e}"));
    assert!(c.record(Environment::Paper, provenance()).is_err());
    let blank = Provenance {
        dataset_digest: String::new(),
        ..provenance()
    };
    assert!(c.record(Environment::Historical, blank).is_err());
    assert!(c.record(Environment::Historical, provenance()).is_ok());
    assert!(!c.is_complete());
}

#[test]
fn an_evaluation_missing_any_of_the_nine_criteria_is_refused_by_name() {
    let mut findings: BTreeMap<Criterion, Finding> = Criterion::ALL
        .iter()
        .map(|c| {
            (
                *c,
                Finding {
                    passed: true,
                    evidence: "e".into(),
                },
            )
        })
        .collect();
    assert!(
        EvaluationRecord::new("q", findings.clone()).is_ok(),
        "premise: nine is whole"
    );
    findings.remove(&Criterion::Leakage);
    match EvaluationRecord::new("q", findings) {
        Err(e) => assert!(e.to_string().contains("Leakage"), "{e}"),
        Ok(r) => panic!("an eight-criterion evaluation was admitted: {r:?}"),
    }
}

#[test]
fn proven_research_becomes_a_capability_and_failed_research_records_its_null() {
    let mut desk = desk_with(&[("model-fit", ToolKind::ModelEndpoint)]);
    let us = set(&["US"]);
    let raised = low_confidence_gap();
    let question = question_for(&raised);
    desk.offer(raised.clone(), &set(&[]), &us);
    let _ = desk.finish_sweep(budget(10));
    assert_eq!(
        desk.queue().entry(&question).map(|e| e.state().clone()),
        Some(State::Started)
    );
    let capability = desk
        .conclude(&complete_campaign(&question), &evaluation(&question, None))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(capability.verb, CapabilityVerb::Reason);
    assert!(desk.queue().is_empty());

    // Failure: the null is recorded and the same question is refused next time.
    let mut desk = desk_with(&[("model-fit", ToolKind::ModelEndpoint)]);
    desk.offer(raised.clone(), &set(&[]), &us);
    let _ = desk.finish_sweep(budget(10));
    let refused = desk.conclude(
        &complete_campaign(&question),
        &evaluation(&question, Some(Criterion::Safety)),
    );
    assert!(refused.is_err());
    assert!(matches!(
        desk.offer(raised, &set(&[]), &us),
        Admission::Refused { .. }
    ));
}

fn quantum_research_gap() -> qip_expansion::gap::RaisedGap {
    let observation = Observation {
        trigger: GapTrigger::UnsupportedRequest,
        wanting: vec![Wanting::Compute],
        observation: "a 40-asset allocation the classical solver bounds but does not close".into(),
        affected_domains: vec!["optimization".into()],
        evidence: vec!["solver-gap:portfolio-40".into()],
        severity: 0.6,
        economic_value: Decimal::from_int(5),
    };
    let raised = raise(observation).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        raised.signal.gap_class,
        GapClass::ComputeQuantumResearch,
        "premise: this is quantum research"
    );
    raised
}

/// R12-COVERAGE-7175: quantum-assisted research stays a research path. The
/// only road from a quantum-research gap to a capability is the desk's
/// `conclude`, which is `promotion::promote`: all four environments in
/// order, all nine criteria — `IncrementalValue` being the one that asks
/// whether it beat its classical baseline — and even then the capability is
/// `Simulate`, paper and shadow only, never a verb that moves money.
///
/// The failure prevented: a "we used a quantum computer" result that skipped
/// shadow, or that lost to the classical baseline, arriving as capability
/// because the class was exotic enough that nobody wired the gate to it.
///
/// Mutation: in `promote`, drop the `campaign.is_complete()` check and the
/// three-environment campaign is promoted; drop the failed-criteria check and
/// the run that lost to its baseline is promoted.
#[test]
fn quantum_research_reaches_capability_only_through_every_environment_and_criterion() {
    let us = set(&["US"]);
    let started = |raised: &qip_expansion::gap::RaisedGap| {
        let mut desk = desk_with(&[
            ("quantum-backend", ToolKind::QuantumBackend),
            ("classical-solver", ToolKind::Solver),
        ]);
        let admission = desk.offer(raised.clone(), &set(&[]), &us);
        assert!(
            !matches!(admission, Admission::Refused { .. }),
            "premise: the desk admits quantum research: {admission:?}"
        );
        let _ = desk.finish_sweep(budget(10));
        assert_eq!(
            desk.queue()
                .entry(&question_for(raised))
                .map(|e| e.state().clone()),
            Some(State::Started),
            "premise: the research ran"
        );
        desk
    };
    let raised = quantum_research_gap();
    let question = question_for(&raised);

    // Stopped before shadow: refused, whatever the evaluation says.
    let mut three = ResearchCampaign::open(&question).unwrap_or_else(|e| panic!("{e}"));
    for env in &Environment::ORDER[..3] {
        three
            .record(*env, provenance())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let mut desk = started(&raised);
    match desk.conclude(&three, &evaluation(&question, None)) {
        Err(e) => assert!(e.to_string().contains("3 of 4"), "{e}"),
        Ok(c) => panic!("quantum research promoted without shadow: {c:?}"),
    }

    // Lost to the classical baseline: refused, and the null is recorded.
    let mut desk = started(&raised);
    match desk.conclude(
        &complete_campaign(&question),
        &evaluation(&question, Some(Criterion::IncrementalValue)),
    ) {
        Err(e) => assert!(e.to_string().contains("IncrementalValue"), "{e}"),
        Ok(c) => panic!("quantum research that lost to its baseline was promoted: {c:?}"),
    }

    // All four environments and all nine criteria: a simulation capability,
    // paper and shadow only.
    let mut desk = started(&raised);
    let capability = desk
        .conclude(&complete_campaign(&question), &evaluation(&question, None))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(capability.verb, CapabilityVerb::Simulate);
    assert_eq!(capability.eligibility, "paper and shadow only");
}

fn model_gap(trigger: GapTrigger, observation: &str) -> qip_expansion::gap::RaisedGap {
    raise(Observation {
        trigger,
        wanting: vec![Wanting::Model],
        observation: observation.into(),
        affected_domains: vec!["equities".into()],
        evidence: vec![format!("reading:{observation}")],
        severity: 0.5,
        // Priority multiplies by value per cost, so a zero here zeroes both
        // gaps and the comparison below would be between two zeros.
        economic_value: Decimal::from_int(5),
    })
    .unwrap_or_else(|e| panic!("{e}"))
}

/// R12-COVERAGE-7169: the expansion engine ranks what it learns from failure
/// above what merely surprised it. Two model gaps identical in severity,
/// value and reach, one raised by a model drifting (a failure the platform
/// is running) and one by a surprise, are ranked failure first — and the
/// failure is offered second, so it wins while carrying the coverage
/// handicap of being the class's second item.
///
/// A mutation that scored every gap as a non-failure (`criteria_for` with
/// `failure = false`) survived every test in this crate and in the deep
/// brain before this one: ranking from live failure signals was asserted
/// nowhere. Mutation: that one, and the drift ranks below the surprise.
#[test]
fn a_failure_signal_outranks_an_otherwise_identical_surprise_on_the_desk() {
    let surprise = model_gap(GapTrigger::Surprise, "momentum surprised the forecast");
    let drift = model_gap(GapTrigger::ModelDrift, "momentum model drifted");
    assert_eq!(
        surprise.signal.gap_class, drift.signal.gap_class,
        "premise: one class, so only the trigger differs"
    );
    let mut desk = desk_with(&[("model-fit", ToolKind::ModelEndpoint)]);
    let us = set(&["US"]);
    for raised in [surprise.clone(), drift.clone()] {
        let admission = desk.offer(raised, &set(&[]), &us);
        assert!(
            !matches!(admission, Admission::Refused { .. } | Admission::Duplicate),
            "premise: both are queued: {admission:?}"
        );
    }
    let ranked: Vec<&str> = desk
        .queue()
        .ranked()
        .iter()
        .map(|e| e.item().research_question.as_str())
        .collect();
    assert_eq!(ranked.len(), 2, "{ranked:?}");
    assert_eq!(
        ranked[0],
        question_for(&drift),
        "the drifting model was not ranked first: {ranked:?}"
    );
    let priority = |raised: &qip_expansion::gap::RaisedGap| {
        desk.queue()
            .entry(&question_for(raised))
            .map(|e| e.score().priority())
            .unwrap_or_else(|| panic!("queued"))
    };
    assert!(
        priority(&surprise) > 0.0,
        "premise: the surprise has a priority to be outranked"
    );
    assert!(priority(&drift) > priority(&surprise));
}
