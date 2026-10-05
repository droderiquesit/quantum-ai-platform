#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_agents::research::{ResearchRegistry, Screening, Verdict};
use qip_agents::tools::{ToolKind, ToolPermission, ToolRegistry};

#[test]
fn a_tool_of_each_of_the_ten_kinds_resolves_by_kind_with_its_scope() {
    assert_eq!(ToolKind::ALL.len(), 10);
    let mut registry = ToolRegistry::new();
    for (i, kind) in ToolKind::ALL.into_iter().enumerate() {
        registry.register(&format!("tool-{i}"), kind).unwrap();
    }
    for (i, kind) in ToolKind::ALL.into_iter().enumerate() {
        let found = registry.by_kind(kind);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name(), format!("tool-{i}"));
        assert!(found[0].scope().contains(&ToolPermission::Read));
    }
}

#[test]
fn a_new_tool_is_read_only_and_sandboxed_until_a_promotion_widens_exactly_the_granted_scope() {
    let mut registry = ToolRegistry::new();
    registry.register("parser-a", ToolKind::Parser).unwrap();
    registry
        .authorise("parser-a", ToolPermission::Read)
        .unwrap();
    assert!(
        registry
            .authorise("parser-a", ToolPermission::Write)
            .is_err()
    );
    assert!(
        registry
            .authorise("parser-a", ToolPermission::LeaveSandbox)
            .is_err()
    );

    assert!(
        registry
            .promote("parser-a", ToolPermission::Write, "  ")
            .is_err()
    );
    assert!(
        registry
            .authorise("parser-a", ToolPermission::Write)
            .is_err()
    );

    registry
        .promote("parser-a", ToolPermission::Write, "eval-2026-10-04")
        .unwrap();
    registry
        .authorise("parser-a", ToolPermission::Write)
        .unwrap();
    assert!(
        registry
            .authorise("parser-a", ToolPermission::LeaveSandbox)
            .is_err()
    );
}

#[test]
fn a_repeated_hypothesis_is_matched_to_its_recorded_null_result_before_any_experiment() {
    let mut registry = ResearchRegistry::new();
    assert_eq!(registry.screen("Moon phase predicts oil"), Screening::Novel);
    registry
        .record(
            "moon phase predicts oil",
            Verdict::Null {
                evidence: "exp-17".into(),
            },
        )
        .unwrap();
    match registry.screen("  Moon   Phase predicts OIL ") {
        Screening::PriorNull { evidence, .. } => assert_eq!(evidence, "exp-17"),
        other => panic!("expected a prior null, got {other:?}"),
    }
    assert!(
        registry
            .record(
                "x",
                Verdict::Null {
                    evidence: String::new()
                }
            )
            .is_err()
    );
}
