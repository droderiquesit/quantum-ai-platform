//! Integration tests for source sandbox lifecycle (DATA-034).
//!
//! A newly registered source's adapter runs only in sandbox before promotion
//! to production. Its output reaches knowledge stores only after a recorded
//! promotion event.

use qip_data_finder::decision::{LifecycleStage, Reasoning};

/// A newly registered source enters sandbox stage before promotion.
#[test]
fn a_newly_registered_source_enters_sandbox_after_registration() {
    let mut reasoning = Reasoning::new();
    reasoning.record(LifecycleStage::Register, "source registered for testing");
    reasoning.record(
        LifecycleStage::Sandbox,
        "adapter instantiated in sandbox environment only",
    );

    assert!(reasoning.reached(LifecycleStage::Register));
    assert!(reasoning.reached(LifecycleStage::Sandbox));
    assert!(!reasoning.reached(LifecycleStage::Promote));

    let register_findings = reasoning.at(LifecycleStage::Register);
    assert_eq!(register_findings.len(), 1);
    assert_eq!(register_findings[0], "source registered for testing");

    let sandbox_findings = reasoning.at(LifecycleStage::Sandbox);
    assert_eq!(sandbox_findings.len(), 1);
    assert_eq!(
        sandbox_findings[0],
        "adapter instantiated in sandbox environment only"
    );
}

/// A source moves from sandbox to production only through a recorded promote stage.
#[test]
fn a_source_moves_from_sandbox_to_production_through_promotion() {
    let mut reasoning = Reasoning::new();
    reasoning.record(LifecycleStage::Register, "source registered");
    reasoning.record(LifecycleStage::Sandbox, "adapter running in sandbox");
    reasoning.record(
        LifecycleStage::Promote,
        "sandbox verification passed; adapter approved for production",
    );

    assert!(reasoning.reached(LifecycleStage::Register));
    assert!(reasoning.reached(LifecycleStage::Sandbox));
    assert!(reasoning.reached(LifecycleStage::Promote));

    let promote_findings = reasoning.at(LifecycleStage::Promote);
    assert_eq!(promote_findings.len(), 1);
    assert_eq!(
        promote_findings[0],
        "sandbox verification passed; adapter approved for production"
    );
}

/// A source without a promote stage has not reached production.
#[test]
fn a_source_without_promotion_is_still_in_sandbox() {
    let mut reasoning = Reasoning::new();
    reasoning.record(LifecycleStage::Register, "source registered");
    reasoning.record(LifecycleStage::Sandbox, "running tests in sandbox");

    assert!(reasoning.reached(LifecycleStage::Sandbox));
    assert!(!reasoning.reached(LifecycleStage::Promote));

    let stages_seen = LifecycleStage::ORDER
        .iter()
        .filter(|s| reasoning.reached(**s))
        .map(|s| s.as_str())
        .collect::<Vec<_>>();

    assert_eq!(stages_seen, vec!["register", "sandbox"]);
}

/// The lifecycle order enforces sandbox before promotion.
#[test]
fn lifecycle_order_has_sandbox_before_promote_before_monitor() {
    let order = LifecycleStage::ORDER;

    let sandbox_idx = order
        .iter()
        .position(|s| *s == LifecycleStage::Sandbox)
        .expect("Sandbox stage must exist");
    let promote_idx = order
        .iter()
        .position(|s| *s == LifecycleStage::Promote)
        .expect("Promote stage must exist");
    let register_idx = order
        .iter()
        .position(|s| *s == LifecycleStage::Register)
        .expect("Register stage must exist");
    let monitor_idx = order
        .iter()
        .position(|s| *s == LifecycleStage::Monitor)
        .expect("Monitor stage must exist");

    assert!(
        register_idx < sandbox_idx,
        "Register must come before Sandbox"
    );
    assert!(
        sandbox_idx < promote_idx,
        "Sandbox must come before Promote"
    );
    assert!(
        promote_idx < monitor_idx,
        "Promote must come before Monitor"
    );
}

/// The lifecycle order includes all 12 expected stages.
#[test]
fn lifecycle_order_contains_all_twelve_stages() {
    let order = LifecycleStage::ORDER;
    assert_eq!(order.len(), 12, "Must have exactly 12 lifecycle stages");

    let expected_stages = vec![
        LifecycleStage::Discover,
        LifecycleStage::Classify,
        LifecycleStage::Probe,
        LifecycleStage::AssessLegality,
        LifecycleStage::Score,
        LifecycleStage::Route,
        LifecycleStage::Register,
        LifecycleStage::Sandbox,
        LifecycleStage::Promote,
        LifecycleStage::Monitor,
        LifecycleStage::DetectDrift,
        LifecycleStage::FindReplacement,
    ];

    assert_eq!(order.to_vec(), expected_stages);
}

/// Sandbox and Promote stages serialize and deserialize correctly.
#[test]
fn sandbox_and_promote_stages_roundtrip_through_serialization() {
    let mut reasoning = Reasoning::new();
    reasoning.record(LifecycleStage::Sandbox, "test sandbox stage");
    reasoning.record(LifecycleStage::Promote, "test promote stage");

    let json = serde_json::to_string(&reasoning).expect("serialization failed");

    let deserialized: Reasoning = serde_json::from_str(&json).expect("deserialization failed");

    assert!(deserialized.reached(LifecycleStage::Sandbox));
    assert!(deserialized.reached(LifecycleStage::Promote));
    assert_eq!(
        deserialized.at(LifecycleStage::Sandbox),
        vec!["test sandbox stage"]
    );
    assert_eq!(
        deserialized.at(LifecycleStage::Promote),
        vec!["test promote stage"]
    );
}
