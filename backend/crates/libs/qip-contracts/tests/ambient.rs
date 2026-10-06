//! The ambient contracts: records, the attention router, the promotion gate.

// Fixture helpers outside a #[test] fn are test code too; expect is the assertion.
#![allow(clippy::expect_used)]

use qip_contracts::ambient::{
    Advisory, AmbientSignal, AttentionRouter, ComputeAllocation, ComputeResourceType, Disposition,
    ModelReputation, Pathway, PromotionCase, RoutingPolicy, SignalClass, Trigger, promote,
};
use qip_core::{Duration, Timestamp};
use std::collections::BTreeSet;

const WINDOW_SECS: i64 = 60;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn signal(id: usize, class: SignalClass, severity: u32, secs: i64) -> AmbientSignal {
    AmbientSignal::new(
        format!("sig-{id}"),
        class,
        "EURUSD",
        severity,
        Trigger::Schedule("every-minute".into()),
        at(secs),
    )
    .expect("a well-formed signal")
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
    );
    assert!(ok.is_ok(), "premise: a valid signal is admitted");
    let unnamed = AmbientSignal::new(
        "s",
        SignalClass::Risk,
        "book",
        1,
        Trigger::Schedule(String::new()),
        at(0),
    );
    assert!(unnamed.is_err());
    let loud = AmbientSignal::new(
        "s",
        SignalClass::Risk,
        "book",
        10_001,
        Trigger::Schedule("t".into()),
        at(0),
    );
    assert!(loud.is_err());
}

#[test]
fn a_deviation_below_materiality_wakes_nothing_and_a_material_one_takes_its_classes_pathway() {
    let mut rng = Lcg(7);
    let expected = |c: SignalClass| match c {
        SignalClass::Surprise => Pathway::SpecialistActivation,
        SignalClass::Opportunity => Pathway::ResearchTask,
        SignalClass::Risk => Pathway::RiskReview,
        SignalClass::Assumption => Pathway::ReflexModelUpdate,
    };
    let (mut quiet, mut material) = (0, 0);
    for i in 0..400usize {
        // A fresh router per case: this property is about one signal, not the budget.
        let mut router = AttentionRouter::new(policy(1_000, 0));
        let class = SignalClass::ALL[(rng.next() % 4) as usize];
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
            let s = signal(n, SignalClass::ALL[(rng.next() % 4) as usize], 9_000, secs);
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
