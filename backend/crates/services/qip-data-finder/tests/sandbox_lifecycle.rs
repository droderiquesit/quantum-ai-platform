//! Source sandbox lifecycle (DATA-034) — what the finder actually does.
//!
//! DATA-034 asks that a newly registered source's adapter run only in a
//! sandbox before promotion, and that its output reach knowledge stores only
//! after a recorded promotion event. **That behaviour does not exist.**
//! `LifecycleStage::Sandbox` and `LifecycleStage::Promote` are declared in
//! `decision.rs` and recorded by nothing in `src/`: `DataFinder::assess`
//! goes from `Register` straight to a `RegisteredSource` in the registry, and
//! the one automatic promotion it performs (§7.6.3, a deep-web adapter in an
//! approved category) is recorded under `Route`. This is a production gap.
//!
//! The previous version of this file recorded `Sandbox` and `Promote` into a
//! `Reasoning` by hand and asserted they had been recorded — it tested a
//! `Vec`, and passed with the finder deleted. The first test below drives the
//! real finder and pins what it does today, so that the day a sandbox stage
//! lands this test fails and is rewritten to the requirement rather than
//! silently continuing to describe an absence.

mod common;

use common::{AGENT, candidate, licensed_for, now, permissive_robots, probe_for};
use qip_contracts::governance::Usage;
use qip_data_finder::decision::{LifecycleStage, Reasoning};
use qip_data_finder::finder::{DataFinder, FinderConfig};

const URL: &str = "https://example.com/data/prices.json";

/// GAP PIN (DATA-034). A source that passes legality and scoring is
/// registered — present in the finder's registry, where ingestion reads it —
/// with no sandbox stage and no promotion stage in its reasoning.
///
/// Mutation that makes this fail: in `DataFinder::assess_one` (finder.rs),
/// add `reasoning.record(LifecycleStage::Sandbox, ...)` before the final
/// `Register` record — the `!reached(Sandbox)` assertion fires. That is the
/// intended failure: it means the gap has been closed and this test must be
/// rewritten to assert the sandbox-before-registry ordering instead.
#[test]
fn a_registered_source_reaches_the_registry_with_no_sandbox_or_promotion_stage_recorded() {
    let config = FinderConfig::new(AGENT, Usage::Derive, "market-data", 7).expect("config");
    let mut finder = DataFinder::new(config);
    let mut probe = probe_for(URL, "example.com", permissive_robots());
    let decisions = finder
        .assess(
            vec![
                candidate(
                    "sandbox-feed",
                    URL,
                    licensed_for(&[Usage::Research, Usage::Derive]).expect("posture"),
                    &["EU0001"],
                )
                .expect("candidate"),
            ],
            &mut probe,
            now(),
        )
        .expect("assess");

    // Premise: the source really was registered, through the stages that do
    // exist. A rejected source would pass the absence checks for free.
    assert_eq!(decisions.len(), 1);
    let decision = &decisions[0];
    assert!(
        decision.is_registered(),
        "premise: the candidate must register: {}",
        decision.reasoning().describe()
    );
    assert!(finder.registered("sandbox-feed").is_some());
    let reasoning = decision.reasoning();
    assert!(reasoning.reached(LifecycleStage::AssessLegality));
    assert!(reasoning.reached(LifecycleStage::Register));

    // The current behaviour, stated as the gap it is.
    assert!(
        !reasoning.reached(LifecycleStage::Sandbox),
        "a sandbox stage is now recorded — DATA-034 may be closing; rewrite this test to the \
         requirement: {}",
        reasoning.describe()
    );
    assert!(
        !reasoning.reached(LifecycleStage::Promote),
        "a promote stage is now recorded — DATA-034 may be closing; rewrite this test to the \
         requirement: {}",
        reasoning.describe()
    );
}

/// `reached` answers from what was recorded and nothing else: a reasoning that
/// recorded `Register` has not thereby reached `Sandbox`. This is the property
/// a future sandbox gate would read, so it is worth pinning on its own.
///
/// Mutation: make `Reasoning::reached` compare stage *position* (`>=`) rather
/// than equality — `reached(Sandbox)` after `Register` alone would then be
/// false still, but `reached(Register)` after only `Sandbox` would be true and
/// the second assertion fires.
#[test]
fn reaching_one_lifecycle_stage_does_not_imply_reaching_its_neighbours() {
    let mut only_register = Reasoning::new();
    only_register.record(LifecycleStage::Register, "registered");
    assert!(only_register.reached(LifecycleStage::Register), "premise");
    assert!(!only_register.reached(LifecycleStage::Sandbox));
    assert!(!only_register.reached(LifecycleStage::Promote));

    let mut only_sandbox = Reasoning::new();
    only_sandbox.record(LifecycleStage::Sandbox, "sandboxed");
    assert!(only_sandbox.reached(LifecycleStage::Sandbox), "premise");
    assert!(!only_sandbox.reached(LifecycleStage::Register));
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
