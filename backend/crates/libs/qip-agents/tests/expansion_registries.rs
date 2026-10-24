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

/// R12-COVERAGE-7170: the research registry is versioned like the ontology
/// registry. Recording once overwrote the entry, so a null result re-opened
/// later was lost from the record it was screened against. Mutation:
/// accepting a version equal to the latest lets the stale `version 2` in.
#[test]
fn a_research_verdict_that_does_not_raise_the_version_is_refused_and_every_version_is_kept() {
    let mut registry = ResearchRegistry::new();
    let first = registry
        .record(
            "credit leads equity",
            Verdict::Null {
                evidence: "exp-1".into(),
            },
        )
        .unwrap();
    let second = registry
        .record("Credit leads  equity", Verdict::Open)
        .unwrap();
    // Premise: two verdicts, numbered in order, both held.
    assert_eq!((first, second), (1, 2));
    assert_eq!(registry.history("credit leads equity").len(), 2);

    for stale in [0, 1, 2] {
        assert!(
            registry
                .record_version(
                    "credit leads equity",
                    stale,
                    Verdict::Supported {
                        evidence: "exp-2".into(),
                    },
                )
                .is_err(),
            "version {stale} was accepted over latest 2"
        );
    }
    assert_eq!(registry.latest_version("credit leads equity"), Some(2));
    assert_eq!(
        registry.screen("credit leads equity"),
        Screening::AlreadyKnown {
            hypothesis: "Credit leads  equity".into(),
        }
    );
    assert_eq!(
        registry.history("credit leads equity")[0].verdict,
        Verdict::Null {
            evidence: "exp-1".into()
        }
    );
    registry
        .record_version(
            "credit leads equity",
            3,
            Verdict::Supported {
                evidence: "exp-2".into(),
            },
        )
        .unwrap();
    assert_eq!(registry.latest_version("credit leads equity"), Some(3));
}

mod memory_registry {
    use qip_agents::memory::{
        Disposition, Episode, EpisodeOutcome, MemoryRegistry, ResearchMemory,
    };
    use qip_core::ids::AgentRunId;
    use qip_core::time::Timestamp;

    fn at(text: &str) -> Timestamp {
        Timestamp::parse_rfc3339(text).unwrap()
    }

    fn call(run: &str, conviction: f64, correct: bool) -> Episode {
        Episode {
            run_id: AgentRunId::from_string(run.to_string()),
            agent_id: "analyst-a".to_string(),
            occurred_at: at("2026-10-01T00:00:00Z"),
            question: "which way would it move".to_string(),
            conclusion: "positive".to_string(),
            evidence: vec![run.to_string()],
            conviction,
            outcome: Some(EpisodeOutcome {
                resolved_at: at("2026-10-01T00:00:00Z"),
                correct,
                realised_bps: Some(10.0),
                explanation: "moved".to_string(),
            }),
        }
    }

    #[test]
    fn a_below_threshold_episode_is_kept_compressed_and_still_counts_in_the_hit_rate() {
        let mut registry = MemoryRegistry::new(0.3).unwrap();
        let mut memory = ResearchMemory::new();
        // Nine confident calls that came off (surprise 0.1) and one confident
        // call that missed (surprise 0.9): the registry keeps only the miss
        // whole, and a hit rate over what it kept would read 0 rather than 0.9.
        let mut calls: Vec<Episode> = (0..9)
            .map(|n| call(&format!("hit-{n}"), 0.9, true))
            .collect();
        calls.push(call("miss", 0.9, false));
        let mut dispositions = Vec::new();
        for episode in calls {
            dispositions.push(registry.consult(&episode).unwrap());
            memory.record_episode(episode);
        }
        // The premise: both dispositions occurred, so the assertions below
        // are not about a registry that only ever did one thing.
        assert!(dispositions.contains(&Disposition::Retained));
        assert!(dispositions.contains(&Disposition::Compressed));

        assert_eq!(registry.retained_for("analyst-a"), 1);
        let summary = registry.summary_for("analyst-a").unwrap();
        assert_eq!(summary.episodes, 9);
        assert_eq!(summary.correct, 9);
        assert_eq!(memory.episode_count(), 10, "compression drops no episode");
        let rate = memory
            .hit_rate("analyst-a", at("2026-10-02T00:00:00Z"))
            .unwrap();
        assert!((rate - 0.9).abs() < 1e-9, "hit rate was {rate}");
    }

    #[test]
    fn a_registry_refuses_a_threshold_that_could_never_compress_and_an_unresolved_episode() {
        for bad in [0.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
            assert!(MemoryRegistry::new(bad).is_err(), "{bad} was accepted");
        }
        let mut registry = MemoryRegistry::new(1.0).unwrap();
        let mut open = call("open", 0.5, true);
        open.outcome = None;
        assert_eq!(registry.consult(&open), None);
        assert_eq!(registry.retained_for("analyst-a"), 0);
    }
}
