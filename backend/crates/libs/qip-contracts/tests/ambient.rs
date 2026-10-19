//! The ambient contracts: records, the attention router, the promotion gate.

// Fixture helpers outside a #[test] fn are test code too; expect is the assertion.
#![allow(clippy::expect_used)]

use qip_contracts::ambient::{
    AcquisitionCriteria, AcquisitionStatus, ActivationReason, Advisory, AmbientSignal,
    AttentionBudget, AttentionEvent, AttentionRouter, AuthorityBoundary, ComputeAllocation,
    ComputeResourceType, Detection, Disposition, InfoRequest, ModelReputation, Pathway,
    PromotionCase, RoutingPolicy, SignalClass, Trigger, promote,
};
use qip_core::{Duration, Timestamp, dec};
use std::collections::BTreeSet;

const WINDOW_SECS: i64 = 60;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

/// A complete CONTRACT-025 detection made at `secs`, live for two hours.
fn detection(secs: i64) -> Detection {
    Detection {
        observed_deviation: dec!("0.05"),
        expected_baseline: dec!("1.02"),
        horizon: Duration::from_secs(3_600),
        novelty_bp: 500,
        affected_entities: vec!["EURUSD".into()],
        urgency_bp: 2_000,
        wake_targets: vec![Pathway::SpecialistActivation],
        evidence_ids: vec!["ev-1".into()],
        expiry: at(secs + 7_200),
    }
}

fn signal_with(
    id: usize,
    class: SignalClass,
    severity: u32,
    secs: i64,
    detection: Detection,
) -> AmbientSignal {
    AmbientSignal::new(
        format!("sig-{id}"),
        class,
        "EURUSD",
        severity,
        Trigger::Schedule("every-minute".into()),
        at(secs),
        detection,
    )
    .expect("a well-formed signal")
}

fn signal(id: usize, class: SignalClass, severity: u32, secs: i64) -> AmbientSignal {
    signal_with(id, class, severity, secs, detection(secs))
}

fn policy(budget: u32, defer: usize) -> RoutingPolicy {
    RoutingPolicy::standard(4_000, budget, Duration::from_secs(WINDOW_SECS), defer)
        .expect("a valid policy")
}

/// A small deterministic generator; the repository has no property-test crate
/// and may not add one.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

#[test]
fn a_signal_without_a_named_trigger_or_with_an_out_of_range_severity_is_refused() {
    // Premise: the same call with valid parts succeeds.
    let ok = AmbientSignal::new(
        "s",
        SignalClass::Risk,
        "book",
        10_000,
        Trigger::Event("world-model-updated".into()),
        at(0),
        detection(0),
    );
    assert!(ok.is_ok(), "premise: a valid signal is admitted");
    let unnamed = AmbientSignal::new(
        "s",
        SignalClass::Risk,
        "book",
        1,
        Trigger::Schedule(String::new()),
        at(0),
        detection(0),
    );
    assert!(unnamed.is_err());
    let loud = AmbientSignal::new(
        "s",
        SignalClass::Risk,
        "book",
        10_001,
        Trigger::Schedule("t".into()),
        at(0),
        detection(0),
    );
    assert!(loud.is_err());
}

/// R12-AMBIENT-3065: `Pathway::CapitalPreparation` has a route. With four
/// classes it had none, so the pathway the blueprint names for "prepare
/// capital" was a label no signal could reach.
///
/// Mutation: route `SignalClass::Capital` to `Pathway::ResearchTask` in
/// `RoutingPolicy::standard`.
#[test]
fn a_capital_signal_is_routed_to_capital_preparation_and_no_other_class_is() {
    assert!(
        SignalClass::ALL.contains(&SignalClass::Capital),
        "premise: capital is a class"
    );
    let mut reached = Vec::new();
    for class in SignalClass::ALL {
        let mut router = AttentionRouter::new(policy(1_000, 0));
        let events = router
            .route(&signal(1, class, 9_000, 0), at(0))
            .expect("routes");
        assert_eq!(events.len(), 1, "{class:?}");
        if events[0].pathway == Pathway::CapitalPreparation {
            reached.push(class);
        }
    }
    assert_eq!(reached, vec![SignalClass::Capital]);
}

#[test]
fn a_deviation_below_materiality_wakes_nothing_and_a_material_one_takes_its_classes_pathway() {
    let mut rng = Lcg(7);
    let expected = |c: SignalClass| match c {
        SignalClass::Surprise => Pathway::SpecialistActivation,
        SignalClass::Opportunity => Pathway::ResearchTask,
        SignalClass::Risk => Pathway::RiskReview,
        SignalClass::Assumption => Pathway::ReflexModelUpdate,
        SignalClass::Capital => Pathway::CapitalPreparation,
    };
    let (mut quiet, mut material) = (0, 0);
    for i in 0..400usize {
        // A fresh router per case: this property is about one signal, not the budget.
        let mut router = AttentionRouter::new(policy(1_000, 0));
        let class = SignalClass::ALL[(rng.next() % SignalClass::ALL.len() as u64) as usize];
        let severity = (rng.next() % 10_001) as u32;
        let events = router
            .route(&signal(i, class, severity, 0), at(0))
            .expect("routes");
        if severity < 4_000 {
            quiet += 1;
            assert!(events.is_empty(), "severity {severity} is below the bar");
        } else {
            material += 1;
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].pathway, expected(class));
            assert_eq!(events[0].signal_id, format!("sig-{i}"));
            assert_eq!(events[0].disposition, Disposition::Activated);
        }
    }
    // Premise: the generator exercised both sides of the bar.
    assert!(
        quiet > 50 && material > 50,
        "{quiet} quiet, {material} material"
    );
}

#[test]
fn a_storm_never_exceeds_the_attention_budget_and_every_excess_signal_is_deferred_or_shed() {
    let (budget, defer, windows) = (3u32, 5usize, 12i64);
    let mut router = AttentionRouter::new(policy(budget, defer));
    let mut rng = Lcg(42);
    let mut events = Vec::new();
    let mut material_ids = BTreeSet::new();
    let mut n = 0usize;
    for w in 0..windows {
        // The first call of a window lands on its boundary so windows align.
        let calls = 1 + (rng.next() % 40) as usize;
        for c in 0..calls {
            let secs = w * WINDOW_SECS + if c == 0 { 0 } else { (rng.next() % 59) as i64 };
            let secs = secs.max(w * WINDOW_SECS);
            let s = signal(
                n,
                SignalClass::ALL[(rng.next() % SignalClass::ALL.len() as u64) as usize],
                9_000,
                secs,
            );
            material_ids.insert(s.id().to_string());
            n += 1;
            events.extend(router.route(&s, at(secs)).expect("routes"));
        }
    }
    let mut per_window = vec![0u32; windows as usize];
    for e in events
        .iter()
        .filter(|e| e.disposition == Disposition::Activated)
    {
        per_window[(e.at.as_secs() / WINDOW_SECS) as usize] += 1;
    }
    // Premise: the storm was a storm — some window demanded more than it was given.
    assert!(events.iter().any(|e| e.disposition == Disposition::Shed));
    assert!(
        events
            .iter()
            .any(|e| e.disposition == Disposition::Deferred)
    );
    assert!(per_window.contains(&budget));
    for (w, a) in per_window.iter().enumerate() {
        assert!(*a <= budget, "window {w} activated {a} > budget {budget}");
    }
    // Nothing silently lost: each signal ended activated, shed, or still held.
    let activated: BTreeSet<_> = events
        .iter()
        .filter(|e| e.disposition == Disposition::Activated)
        .map(|e| e.signal_id.clone())
        .collect();
    let shed: BTreeSet<_> = events
        .iter()
        .filter(|e| e.disposition == Disposition::Shed)
        .map(|e| e.signal_id.clone())
        .collect();
    assert_eq!(
        activated.len() + shed.len() + router.deferred_len(),
        material_ids.len()
    );
}

#[test]
fn a_clock_that_steps_back_cannot_refill_a_spent_budget() {
    let mut router = AttentionRouter::new(policy(1, 0));
    router
        .route(&signal(1, SignalClass::Risk, 9_000, 600), at(600))
        .expect("first call opens the window");
    let back = router.route(&signal(2, SignalClass::Risk, 9_000, 10), at(10));
    assert!(back.is_err());
}

#[test]
fn a_routing_policy_that_leaves_a_class_unrouted_or_the_budget_zero_is_refused() {
    use std::collections::BTreeMap;
    let w = Duration::from_secs(60);
    assert!(RoutingPolicy::standard(1, 1, w, 0).is_ok(), "premise");
    assert!(RoutingPolicy::new(1, BTreeMap::new(), 1, w, 0).is_err());
    assert!(RoutingPolicy::standard(1, 0, w, 0).is_err());
}

#[test]
fn an_ambient_output_without_evidence_a_pass_an_approver_and_held_controls_is_refused() {
    let good = || PromotionCase {
        evidence: vec!["holdout-run-17".into()],
        evaluation_passed: true,
        approver: "risk-desk".into(),
        deterministic_controls_held: true,
    };
    let advisory = || Advisory::new("pre-position 2m cash", "sig-9");
    // Premise: the complete case promotes, so each refusal below is its own field.
    let promoted = promote(advisory(), &good()).expect("a complete case promotes");
    assert_eq!(promoted.into_inner(), "pre-position 2m cash");
    let cases = [
        PromotionCase {
            evidence: vec![],
            ..good()
        },
        PromotionCase {
            evaluation_passed: false,
            ..good()
        },
        PromotionCase {
            approver: String::new(),
            ..good()
        },
        PromotionCase {
            deterministic_controls_held: false,
            ..good()
        },
    ];
    for case in cases {
        let refused = promote(advisory(), &case).expect_err("must refuse");
        assert!(refused.message().contains("advisory until promoted"));
    }
}

// ---- CONTRACT-025: the fields an ambient signal carries, and its expiry.

fn new_with(d: Detection) -> qip_core::Result<AmbientSignal> {
    AmbientSignal::new(
        "s",
        SignalClass::Risk,
        "book",
        5_000,
        Trigger::Event("world-model-updated".into()),
        at(100),
        d,
    )
}

#[test]
fn an_ambient_signal_missing_any_contract_field_is_refused_naming_that_field() {
    // Premise: the unaltered detection is admitted, so each refusal below is
    // caused by the one field it alters and not by the fixture.
    assert!(
        new_with(detection(100)).is_ok(),
        "premise: a complete detection is admitted"
    );

    let cases: [(&str, Detection, &str); 9] = [
        (
            "no affected entity",
            Detection {
                affected_entities: vec![],
                ..detection(100)
            },
            "entities it affects",
        ),
        (
            "a blank affected entity",
            Detection {
                affected_entities: vec!["EURUSD".into(), String::new()],
                ..detection(100)
            },
            "entities it affects",
        ),
        (
            "no wake target",
            Detection {
                wake_targets: vec![],
                ..detection(100)
            },
            "pathway to wake",
        ),
        (
            "no evidence lineage",
            Detection {
                evidence_ids: vec![],
                ..detection(100)
            },
            "evidence lineage",
        ),
        (
            "a blank evidence id",
            Detection {
                evidence_ids: vec!["ev-1".into(), String::new()],
                ..detection(100)
            },
            "evidence lineage",
        ),
        (
            "novelty above the scale",
            Detection {
                novelty_bp: 10_001,
                ..detection(100)
            },
            "novelty 10001 bp",
        ),
        (
            "urgency above the scale",
            Detection {
                urgency_bp: 10_001,
                ..detection(100)
            },
            "urgency 10001 bp",
        ),
        (
            "a negative horizon",
            Detection {
                horizon: Duration::from_secs(-1),
                ..detection(100)
            },
            "negative horizon",
        ),
        (
            "an expiry before detection",
            Detection {
                expiry: at(99),
                ..detection(100)
            },
            "expiry before detection",
        ),
    ];
    for (what, d, names) in cases {
        match new_with(d) {
            Ok(_) => panic!("{what} was admitted"),
            Err(e) => assert!(
                e.message().contains(names),
                "{what} was refused for the wrong reason: {}",
                e.message()
            ),
        }
    }
}

#[test]
fn an_ambient_signal_with_a_zero_horizon_or_an_expiry_at_detection_is_admitted() {
    // The boundaries of the two ordering checks: zero is "matters now", and an
    // expiry equal to detection is a signal live for one instant.
    let d = Detection {
        horizon: Duration::ZERO,
        expiry: at(100),
        ..detection(100)
    };
    assert!(new_with(d).is_ok());
}

#[test]
fn an_ambient_signal_carries_every_contract_field_it_was_given() {
    let d = Detection {
        observed_deviation: dec!("0.15"),
        expected_baseline: dec!("1.05"),
        horizon: Duration::from_secs(7_200),
        novelty_bp: 2_500,
        affected_entities: vec!["EURUSD".into(), "GBPUSD".into()],
        urgency_bp: 8_000,
        wake_targets: vec![Pathway::ResearchTask, Pathway::RiskReview],
        evidence_ids: vec!["ev-1".into(), "ev-2".into()],
        expiry: at(300),
    };
    let sig = new_with(d).expect("valid signal");
    assert_eq!(sig.class(), SignalClass::Risk);
    assert_eq!(sig.observed_deviation(), dec!("0.15"));
    assert_eq!(sig.expected_baseline(), dec!("1.05"));
    assert_eq!(sig.horizon(), Duration::from_secs(7_200));
    assert_eq!(sig.novelty_bp(), 2_500);
    assert_eq!(sig.affected_entities(), ["EURUSD", "GBPUSD"]);
    assert_eq!(sig.urgency_bp(), 8_000);
    assert_eq!(
        sig.wake_targets(),
        [Pathway::ResearchTask, Pathway::RiskReview]
    );
    assert_eq!(sig.evidence_ids(), ["ev-1", "ev-2"]);
    assert_eq!(sig.expiry(), at(300));
}

#[test]
fn a_material_signal_consumed_after_its_expiry_wakes_nothing() {
    let short = |secs| Detection {
        expiry: at(secs + 10),
        ..detection(secs)
    };
    // Premise: at its expiry instant the same signal still activates.
    let mut router = AttentionRouter::new(policy(5, 0));
    let live = router
        .route(
            &signal_with(1, SignalClass::Risk, 9_000, 0, short(0)),
            at(10),
        )
        .expect("routes");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].disposition, Disposition::Activated);

    let mut router = AttentionRouter::new(policy(5, 0));
    let late = router
        .route(
            &signal_with(2, SignalClass::Risk, 9_000, 0, short(0)),
            at(11),
        )
        .expect("routes");
    assert_eq!(late.len(), 1, "an expired signal is recorded, not silent");
    assert_eq!(late[0].signal_id, "sig-2");
    assert_eq!(late[0].disposition, Disposition::Expired);
    assert!(
        late.iter().all(|e| e.disposition != Disposition::Activated),
        "an expired signal woke something"
    );
}

#[test]
fn a_held_signal_that_expires_while_deferred_wakes_nothing_and_spends_no_budget() {
    // Budget one per window, room to hold one.
    let mut router = AttentionRouter::new(policy(1, 1));
    let first = router
        .route(&signal(1, SignalClass::Risk, 9_000, 0), at(0))
        .expect("routes");
    assert_eq!(first[0].disposition, Disposition::Activated);
    // Live for 30s, so stale by the next window at 60s.
    let short = Detection {
        expiry: at(30),
        ..detection(1)
    };
    let held = router
        .route(&signal_with(2, SignalClass::Risk, 9_000, 1, short), at(1))
        .expect("routes");
    assert_eq!(held[0].disposition, Disposition::Deferred);
    assert_eq!(router.deferred_len(), 1, "premise: sig-2 is held");

    // The next window drains the hold queue: sig-2 is expired, so it is
    // recorded Expired and the window's one slot still goes to sig-3.
    let next = router
        .route(&signal(3, SignalClass::Risk, 9_000, 60), at(60))
        .expect("routes");
    let dispositions: Vec<_> = next
        .iter()
        .map(|e| (e.signal_id.as_str(), e.disposition))
        .collect();
    assert_eq!(
        dispositions,
        [
            ("sig-2", Disposition::Expired),
            ("sig-3", Disposition::Activated)
        ]
    );
    assert_eq!(router.deferred_len(), 0);
}

// ---- CONTRACT-029: evidence acquisition ranking and deadline-aware halting.

fn criteria(
    econ_bp: u32,
    uncertainty_bp: u32,
    urgency_bp: u32,
    cost_bp: u32,
) -> AcquisitionCriteria {
    AcquisitionCriteria::new(econ_bp, uncertainty_bp, urgency_bp, cost_bp).expect("valid criteria")
}

#[test]
fn acquisition_criteria_refuses_basis_points_above_the_maximum() {
    // Premise: valid criteria are admitted.
    let ok = AcquisitionCriteria::new(5_000, 5_000, 5_000, 5_000);
    assert!(ok.is_ok(), "premise: valid criteria are admitted");

    // Economic significance over the limit.
    let econ = AcquisitionCriteria::new(10_001, 5_000, 5_000, 5_000);
    assert!(econ.is_err(), "economic significance > 10000 is refused");

    // Uncertainty reduction over the limit.
    let uncertainty = AcquisitionCriteria::new(5_000, 10_001, 5_000, 5_000);
    assert!(
        uncertainty.is_err(),
        "uncertainty reduction > 10000 is refused"
    );

    // Urgency over the limit.
    let urgency = AcquisitionCriteria::new(5_000, 5_000, 10_001, 5_000);
    assert!(urgency.is_err(), "urgency > 10000 is refused");

    // Cost over the limit.
    let cost = AcquisitionCriteria::new(5_000, 5_000, 5_000, 10_001);
    assert!(cost.is_err(), "cost > 10000 is refused");
}

/// R12-AMBIENT-3106 (§9.10): the rank is a product, so a zero in any of
/// significance, uncertainty reduction or urgency zeroes it. The additive
/// form this replaced scored `criteria(10_000, 0, 10_000, 0)` — evidence that
/// would reduce no uncertainty — at 6,000 bp, and this test asserted that
/// form for as long as it stood.
///
/// Mutation: restore the weighted sum in `net_value_bp` — the zero-reduction
/// case scores 6,000 and the first loop fails.
#[test]
fn a_zero_in_any_curiosity_factor_zeroes_the_acquisition_rank() {
    let full = criteria(10_000, 10_000, 10_000, 0);
    assert_eq!(
        full.net_value_bp(),
        10_000,
        "premise: every factor at its maximum and free evidence ranks at the maximum"
    );
    for (name, zeroed) in [
        ("significance", criteria(0, 10_000, 10_000, 0)),
        ("uncertainty reduction", criteria(10_000, 0, 10_000, 0)),
        ("urgency", criteria(10_000, 10_000, 0, 0)),
    ] {
        assert_eq!(
            zeroed.net_value_bp(),
            0,
            "zero {name} must rank zero, not be carried by the other factors"
        );
    }
    // Halving one factor halves the rank: multiplicative, not additive.
    assert_eq!(criteria(5_000, 10_000, 10_000, 0).net_value_bp(), 5_000);
    assert_eq!(criteria(10_000, 5_000, 10_000, 0).net_value_bp(), 5_000);
    assert_eq!(criteria(10_000, 10_000, 5_000, 0).net_value_bp(), 5_000);
    // 0.5 * 0.5 * 0.5 = 0.125 of the scale.
    assert_eq!(criteria(5_000, 5_000, 5_000, 0).net_value_bp(), 1_250);
}

/// Cost is a multiplicative penalty in the share of decision value it
/// consumes: free evidence keeps its rank, evidence costing the whole
/// decision value ranks zero, and the rank falls strictly as cost rises.
///
/// Mutation: drop the `(1 − cost)` factor — the full-cost case then ranks
/// 10,000 and the second assertion fails.
#[test]
fn acquisition_cost_scales_the_rank_down_and_a_cost_of_the_whole_decision_value_zeroes_it() {
    assert_eq!(criteria(10_000, 10_000, 10_000, 0).net_value_bp(), 10_000);
    assert_eq!(criteria(10_000, 10_000, 10_000, 10_000).net_value_bp(), 0);
    assert_eq!(
        criteria(10_000, 10_000, 10_000, 2_500).net_value_bp(),
        7_500
    );
    let mut previous = u32::MAX;
    for cost in (0..=10_000).step_by(1_000) {
        let rank = criteria(8_000, 7_000, 9_000, cost).net_value_bp();
        assert!(
            rank < previous,
            "rank {rank} at cost {cost} did not fall from {previous}"
        );
        previous = rank;
    }
}

/// The Curiosity Engine's ordering uses the rank: highest first, ties by id,
/// and a request with a zero factor or one already halted is not ranked.
///
/// Mutation: drop the `net_value_bp() > 0` filter in `rank_acquisitions` —
/// `no-reduction` is then ranked and the id list differs.
#[test]
fn the_curiosity_engine_ranks_open_requests_by_the_multiplicative_score_and_skips_worthless_ones() {
    let request = |id: &str, c: AcquisitionCriteria| {
        InfoRequest::new(id, "decision-1", c, at(100), at(0)).expect("valid request")
    };
    let mut halted = request("halted", criteria(10_000, 10_000, 10_000, 0));
    assert!(halted.halt(), "premise: a requested request halts");
    let requests = vec![
        request("b-mid", criteria(8_000, 8_000, 8_000, 0)),
        request("no-reduction", criteria(10_000, 0, 10_000, 0)),
        request("a-mid", criteria(8_000, 8_000, 8_000, 0)),
        request("top", criteria(10_000, 9_000, 10_000, 0)),
        halted,
    ];
    assert!(
        requests
            .iter()
            .all(|r| r.criteria().net_value_bp() > 0 || r.id() == "no-reduction"),
        "premise: only the zero-reduction request ranks zero"
    );
    let ranked: Vec<&str> = qip_contracts::ambient::rank_acquisitions(&requests)
        .into_iter()
        .map(InfoRequest::id)
        .collect();
    assert_eq!(ranked, ["top", "a-mid", "b-mid"]);
}
#[test]
fn info_request_refuses_empty_or_future_mismatched_fields() {
    // Premise: a valid request is admitted.
    let ok = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    );
    assert!(ok.is_ok(), "premise: valid request is admitted");

    // Empty request ID.
    let empty_req_id = InfoRequest::new(
        "",
        "decision-1",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    );
    assert!(empty_req_id.is_err(), "empty request ID is refused");

    // Empty decision ID.
    let empty_dec_id = InfoRequest::new(
        "req-1",
        "",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    );
    assert!(empty_dec_id.is_err(), "empty decision ID is refused");

    // Deadline before request time.
    let deadline_before = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(50),
        at(100),
    );
    assert!(
        deadline_before.is_err(),
        "deadline before request time is refused"
    );

    // Deadline equal to request time is allowed (decision must be made instantly).
    let deadline_equal = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(100),
    );
    assert!(
        deadline_equal.is_ok(),
        "deadline equal to request time is allowed"
    );
}

#[test]
fn info_request_status_transitions_are_ordered() {
    let mut req = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");

    // Initially Requested.
    assert_eq!(
        req.status(),
        AcquisitionStatus::Requested,
        "initial status is Requested"
    );

    // Record acquisition transitions to InProgress.
    req.record_acquisition(500).expect("valid cost");
    assert_eq!(
        req.status(),
        AcquisitionStatus::InProgress,
        "status is InProgress after acquisition"
    );

    // Complete transitions to Complete.
    req.complete().expect("valid complete");
    assert_eq!(
        req.status(),
        AcquisitionStatus::Complete,
        "status is Complete after completion"
    );

    // Attempting to complete again is refused.
    let already_complete = req.complete();
    assert!(
        already_complete.is_err(),
        "completing a complete request is refused"
    );

    // Attempting to record acquisition after completion is refused.
    let after_complete = req.record_acquisition(500);
    assert!(
        after_complete.is_err(),
        "recording acquisition after completion is refused"
    );
}

#[test]
fn info_request_cumulative_cost_accumulates_and_refuses_zero() {
    let mut req = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");

    assert_eq!(req.cumulative_cost_bp(), 0, "initial cost is 0");

    // Record a 300 bp cost.
    req.record_acquisition(300).expect("valid cost");
    assert_eq!(req.cumulative_cost_bp(), 300, "cost accumulates");

    // Record another 200 bp cost.
    req.record_acquisition(200).expect("valid cost");
    assert_eq!(
        req.cumulative_cost_bp(),
        500,
        "cumulative cost sums acquisitions"
    );

    // Zero cost is refused.
    let zero = req.record_acquisition(0);
    assert!(zero.is_err(), "zero-cost acquisition is refused");
    assert_eq!(
        req.cumulative_cost_bp(),
        500,
        "cost unchanged after refused acquisition"
    );
}

#[test]
fn info_request_should_halt_checks_deadline_and_minimum_viable_value() {
    let req = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(10_000, 9_000, 8_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");
    assert_eq!(
        req.criteria().net_value_bp(),
        6_480,
        "premise: 1.0 * 0.9 * 0.8 * (1 - 0.1) of the scale"
    );

    // At time 50, before the deadline, with min_viable_value = 2000,
    // and net value = 6480, should not halt.
    assert!(
        !req.should_halt(at(50), 2_000),
        "should_halt is false when before deadline and value > minimum"
    );

    // At time 100 (the deadline), should halt even if value is high.
    assert!(
        req.should_halt(at(100), 2_000),
        "should_halt is true when at deadline"
    );

    // After the deadline, should halt regardless of value.
    assert!(
        req.should_halt(at(101), 2_000),
        "should_halt is true when past deadline"
    );

    // If value falls below minimum, should halt even before deadline.
    let req2 = InfoRequest::new(
        "req-2",
        "decision-2",
        criteria(1_000, 1_000, 1_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");
    let net_value = req2.criteria().net_value_bp();
    assert!(net_value < 1_000, "premise: net value is below minimum");
    assert!(
        req2.should_halt(at(50), 1_000),
        "should_halt is true when net value < minimum, even before deadline"
    );
}

#[test]
fn info_request_halt_transitions_requested_or_in_progress_to_halted() {
    // A Requested request can be halted.
    let mut req1 = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");
    assert!(req1.halt(), "halt returns true for Requested status");
    assert_eq!(
        req1.status(),
        AcquisitionStatus::Halted,
        "Requested halts to Halted"
    );

    // An InProgress request can be halted.
    let mut req2 = InfoRequest::new(
        "req-2",
        "decision-2",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");
    req2.record_acquisition(500).expect("valid cost");
    assert!(req2.halt(), "halt returns true for InProgress status");
    assert_eq!(
        req2.status(),
        AcquisitionStatus::Halted,
        "InProgress halts to Halted"
    );

    // A Complete request cannot be halted.
    let mut req3 = InfoRequest::new(
        "req-3",
        "decision-3",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");
    req3.record_acquisition(500).expect("valid cost");
    req3.complete().expect("valid complete");
    assert!(!req3.halt(), "halt returns false for Complete status");
    assert_eq!(
        req3.status(),
        AcquisitionStatus::Complete,
        "Complete does not transition"
    );

    // A Halted request that halts again remains halted.
    let mut req4 = InfoRequest::new(
        "req-4",
        "decision-4",
        criteria(5_000, 5_000, 5_000, 1_000),
        at(100),
        at(0),
    )
    .expect("valid request");
    req4.halt();
    assert!(!req4.halt(), "halt returns false for Halted status");
    assert_eq!(
        req4.status(),
        AcquisitionStatus::Halted,
        "Halted does not transition"
    );
}
#[test]
fn a_model_reputation_requires_all_three_dimensions_and_all_metrics_within_bounds() {
    let good = || ModelReputation::new("equities", "1h", "stable", 8_000, 7_500, 8_500);
    good().expect("a well-formed reputation");

    let bad_cases = [
        (
            "empty domain",
            ModelReputation::new("", "1h", "stable", 8_000, 7_500, 8_500),
        ),
        (
            "empty horizon",
            ModelReputation::new("equities", "", "stable", 8_000, 7_500, 8_500),
        ),
        (
            "empty regime",
            ModelReputation::new("equities", "1h", "", 8_000, 7_500, 8_500),
        ),
        (
            "accuracy out of range",
            ModelReputation::new("equities", "1h", "stable", 10_001, 7_500, 8_500),
        ),
        (
            "quality out of range",
            ModelReputation::new("equities", "1h", "stable", 8_000, 10_001, 8_500),
        ),
        (
            "skill out of range",
            ModelReputation::new("equities", "1h", "stable", 8_000, 7_500, 10_001),
        ),
    ];
    for (name, result) in bad_cases {
        let refused = result.expect_err(&format!("{name} is refused"));
        assert!(refused.message().contains("must not") || refused.message().contains("exceeds"));
    }
}

#[test]
fn a_model_reputation_overall_score_is_weighted_average() {
    // 40% accuracy, 30% quality, 30% skill
    let rep = ModelReputation::new("equities", "1h", "stable", 10_000, 10_000, 10_000)
        .expect("valid reputation");
    assert_eq!(rep.overall_bp(), 10_000);

    let rep = ModelReputation::new("equities", "1h", "stable", 5_000, 5_000, 5_000)
        .expect("valid reputation");
    assert_eq!(rep.overall_bp(), 5_000);

    // (8000 * 40 + 6000 * 30 + 7000 * 30) / 100 = (320000 + 180000 + 210000) / 100 = 7100
    let rep = ModelReputation::new("equities", "1h", "stable", 8_000, 6_000, 7_000)
        .expect("valid reputation");
    assert_eq!(rep.overall_bp(), 7_100);
}

#[test]
fn a_compute_allocation_requires_nonempty_fields_and_positive_units() {
    let good_rep = || {
        ModelReputation::new("equities", "1h", "stable", 8_000, 7_500, 8_500)
            .expect("valid reputation")
    };

    let good = || {
        ComputeAllocation::new(
            "work-123",
            ComputeResourceType::Cpu,
            100,
            at(100),
            good_rep(),
            "high expected value",
        )
    };
    good().expect("a well-formed allocation");

    let bad_cases = [
        (
            "empty work_id",
            ComputeAllocation::new(
                "",
                ComputeResourceType::Cpu,
                100,
                at(100),
                good_rep(),
                "high value",
            ),
        ),
        (
            "empty rationale",
            ComputeAllocation::new(
                "work-123",
                ComputeResourceType::Cpu,
                100,
                at(100),
                good_rep(),
                "",
            ),
        ),
        (
            "zero units",
            ComputeAllocation::new(
                "work-123",
                ComputeResourceType::Cpu,
                0,
                at(100),
                good_rep(),
                "high value",
            ),
        ),
    ];
    for (name, result) in bad_cases {
        let refused = result.expect_err(&format!("{name} is refused"));
        assert!(refused.message().contains("must not") || refused.message().contains("positive"));
    }
}

#[test]
fn a_compute_allocation_records_all_metadata_for_auditability() {
    let rep = ModelReputation::new("equities", "1h", "stable", 8_000, 7_500, 8_500)
        .expect("valid reputation");
    let alloc = ComputeAllocation::new(
        "work-789",
        ComputeResourceType::Gpu,
        500,
        at(200),
        rep.clone(),
        "strong forecast accuracy needed",
    )
    .expect("valid allocation");

    assert_eq!(alloc.work_id(), "work-789");
    assert_eq!(alloc.resource_type(), ComputeResourceType::Gpu);
    assert_eq!(alloc.units(), 500);
    assert_eq!(alloc.model_reputation().domain(), "equities");
    assert_eq!(alloc.model_reputation().overall_bp(), 8_000); // (8000*40 + 7500*30 + 8500*30)/100
    assert_eq!(alloc.rationale(), "strong forecast accuracy needed");
}

/// R12-CONTRACT-7079: the event the router produces carries the six §27
/// AttentionEvent fields, each with the value the decision actually had, and
/// survives a round trip unchanged.
///
/// The failure this prevents: an event that said only "signal X woke pathway
/// Y" — what the type carried until 2026-10-11 — cannot say what budget the
/// wake was charged to, whether it could have carried authority, or where its
/// outcome is filed, so a replay could count activations and explain none.
#[test]
fn an_attention_event_carries_its_reason_origin_budget_priority_authority_and_feedback_link() {
    let mut router = AttentionRouter::new(policy(2, 1));
    let first = router
        .route(&signal(1, SignalClass::Risk, 7_500, 0), at(0))
        .expect("routes");
    let second = router
        .route(&signal(2, SignalClass::Surprise, 6_000, 1), at(1))
        .expect("routes");
    assert_eq!(
        (first.len(), second.len()),
        (1, 1),
        "premise: both material"
    );
    let (a, b) = (&first[0], &second[0]);

    assert_eq!(
        a.reason,
        ActivationReason {
            class: SignalClass::Risk,
            trigger: Trigger::Schedule("every-minute".into()),
        }
    );
    assert_eq!(b.reason.class, SignalClass::Surprise);
    assert_eq!(a.originating_models, vec!["every-minute".to_string()]);
    // The budget as each decision left it: one spent, then two.
    assert_eq!(
        a.budget,
        AttentionBudget {
            per_window: 2,
            spent: 1
        }
    );
    assert_eq!(
        b.budget,
        AttentionBudget {
            per_window: 2,
            spent: 2
        }
    );
    assert_eq!((a.priority_bp, b.priority_bp), (7_500, 6_000));
    assert_eq!(a.authority, AuthorityBoundary::AdvisoryOnly);
    assert_eq!(a.feedback_link, "sig-1");
    // The link joins to the advisory an activation produces.
    let advisory = Advisory::new("risk review", a.signal_id.clone());
    assert_eq!(advisory.origin_signal(), a.feedback_link);

    let encoded = serde_json::to_string(a).expect("encodes");
    let decoded: AttentionEvent = serde_json::from_str(&encoded).expect("decodes");
    assert_eq!(&decoded, a, "a round trip preserves every field");
}

#[test]
fn an_attention_event_missing_any_contract_field_is_refused_on_decode() {
    let mut router = AttentionRouter::new(policy(1, 0));
    let events = router
        .route(&signal(1, SignalClass::Opportunity, 9_000, 0), at(0))
        .expect("routes");
    let full = serde_json::to_value(&events[0]).expect("encodes");
    let fields = [
        "reason",
        "originating_models",
        "budget",
        "priority_bp",
        "authority",
        "feedback_link",
    ];
    for field in fields {
        // Premise: the field is present in the full record, so its removal is
        // what the decode below refuses.
        assert!(full.get(field).is_some(), "{field} absent from {full}");
        let mut partial = full.clone();
        partial.as_object_mut().expect("an object").remove(field);
        let refused = serde_json::from_value::<AttentionEvent>(partial);
        assert!(
            refused.is_err(),
            "an attention event without {field} decoded: {refused:?}"
        );
    }
    // An authority boundary nobody defined is refused, not read as advisory.
    let mut granted = full.clone();
    granted["authority"] = serde_json::json!("execution");
    assert!(serde_json::from_value::<AttentionEvent>(granted).is_err());
}

/// Mutation: remove the `uncertainty_reduction_bp` shrinkage in
/// `InfoRequest::record_acquisition` — the request then stays viable however
/// much has been bought, and the final assertion fails.
#[test]
fn a_request_viable_at_creation_halts_once_its_acquisitions_have_consumed_its_value() {
    let mut req = InfoRequest::new(
        "req-1",
        "decision-1",
        criteria(10_000, 9_000, 8_000, 1_000),
        at(1_000),
        at(0),
    )
    .expect("valid request");
    let floor = 2_000;
    let initial = req.criteria().net_value_bp();
    assert_eq!(
        initial, 6_480,
        "premise: 1.0 * 0.9 * 0.8 * 0.9 of the scale"
    );
    assert!(
        !req.should_halt(at(1), floor),
        "premise: the request is viable at creation"
    );

    // One purchase of 1,000 bp keeps nine tenths of the reduction on offer.
    req.record_acquisition(1_000).expect("valid cost");
    assert_eq!(req.criteria().uncertainty_reduction_bp, 8_100);
    assert!(req.criteria().net_value_bp() < initial);

    let mut purchases = 1;
    while !req.should_halt(at(1), floor) {
        let before = req.criteria().net_value_bp();
        req.record_acquisition(1_000).expect("valid cost");
        purchases += 1;
        assert!(
            req.criteria().net_value_bp() < before,
            "a purchase did not lower the marginal value"
        );
        assert!(purchases < 100, "the request never halted");
    }
    assert!(req.criteria().net_value_bp() < floor);
    assert_eq!(req.cumulative_cost_bp(), purchases * 1_000);

    // An acquisition that costs the whole decision value leaves nothing to buy.
    let mut all = InfoRequest::new(
        "req-2",
        "decision-2",
        criteria(10_000, 10_000, 10_000, 0),
        at(1_000),
        at(0),
    )
    .expect("valid request");
    all.record_acquisition(10_000).expect("valid cost");
    assert_eq!(all.criteria().net_value_bp(), 0);
    assert!(all.should_halt(at(1), 1));
}

/// Mutation: restore `self.deferred.pop_front()` in `AttentionRouter::route`'s
/// window drain — the earlier, lower-value signal is activated first.
#[test]
fn a_higher_value_deferred_signal_is_activated_before_an_earlier_lower_one() {
    let mut router = AttentionRouter::new(policy(1, 8));
    let valued = |id: usize, severity: u32, economic: Option<u32>, secs: i64| {
        let built = signal(id, SignalClass::Surprise, severity, secs);
        match economic {
            Some(bp) => built.with_economic_value_bp(bp).expect("within the scale"),
            None => built,
        }
    };
    // Window one: the first signal takes the whole budget and the rest are
    // held in arrival order. Values (severity x economic value): 2 is 4.1M,
    // 3 is 60M, 4 is 4.5M, 5 and 6 tie at 32M and 5 arrived first.
    let arrivals = [
        valued(1, 9_000, None, 0),
        valued(2, 4_100, Some(1_000), 1),
        valued(3, 6_000, Some(10_000), 2),
        valued(4, 9_000, Some(500), 3),
        valued(5, 4_000, Some(8_000), 4),
        valued(6, 8_000, Some(4_000), 5),
    ];
    let mut activated: Vec<String> = Vec::new();
    for s in &arrivals {
        for event in router
            .route(s, at(s.detected_at().as_secs()))
            .expect("routes")
        {
            if event.disposition == Disposition::Activated {
                activated.push(event.signal_id.clone());
            }
        }
    }
    assert_eq!(
        activated,
        ["sig-1"],
        "premise: only the first signal fit the window"
    );
    assert_eq!(
        router.deferred_len(),
        5,
        "premise: the other five are held, so the drain has a choice to make"
    );

    // Each later window admits one held signal. A signal below materiality
    // opens the window without taking a slot of its own.
    for window in 1..=5_i64 {
        let tick = signal(
            100 + window as usize,
            SignalClass::Surprise,
            100,
            window * WINDOW_SECS,
        );
        for event in router
            .route(&tick, at(window * WINDOW_SECS))
            .expect("routes")
        {
            if event.disposition == Disposition::Activated {
                activated.push(event.signal_id.clone());
            }
        }
    }
    assert_eq!(
        activated,
        ["sig-1", "sig-3", "sig-5", "sig-6", "sig-4", "sig-2"],
        "held signals must drain by severity x economic value, the older first among equals"
    );
    assert_eq!(router.deferred_len(), 0);
}

#[test]
fn an_economic_value_is_refused_above_the_scale_and_an_unstated_one_does_not_change_the_encoding() {
    let plain = signal(1, SignalClass::Surprise, 5_000, 0);
    assert_eq!(plain.economic_value_bp(), 10_000);
    assert!(plain.clone().with_economic_value_bp(10_001).is_err());
    let text = serde_json::to_string(&plain).expect("encodes");
    assert!(
        !text.contains("economic_value_bp"),
        "an unstated value must not change the encoding of a recorded signal"
    );
    let stated = plain
        .with_economic_value_bp(2_500)
        .expect("within the scale");
    let text = serde_json::to_string(&stated).expect("encodes");
    assert!(text.contains("\"economic_value_bp\":2500"));
    let back: AmbientSignal = serde_json::from_str(&text).expect("decodes");
    assert_eq!(back, stated);
    assert_eq!(stated.attention_value(), 5_000 * 2_500);
}
