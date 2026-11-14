//! The research memory's episode bound.
//!
//! `ResearchMemory::record_episode` once pushed onto an unbounded `Vec`, so
//! the store grew with every agent run for the life of the process. These
//! tests hold the bound and the per-agent index the eviction must keep
//! consistent with it.

use qip_agents::memory::{Episode, ResearchMemory};
use qip_core::ids::AgentRunId;
use qip_core::time::{Duration, Timestamp};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn episode(n: i64, agent: &str) -> Episode {
    Episode {
        run_id: AgentRunId::from_string(format!("run-{n}")),
        agent_id: agent.to_string(),
        occurred_at: now().saturating_add(Duration::from_secs(n)),
        question: "does credit lead equity".to_string(),
        conclusion: "by two days".to_string(),
        evidence: vec![format!("evidence-{n}")],
        conviction: 0.6,
        outcome: None,
        regime: None,
    }
}

#[test]
fn recording_one_episode_past_capacity_keeps_the_memory_at_capacity_and_gives_up_the_oldest() {
    let capacity = 3;
    let mut memory = ResearchMemory::with_capacity(capacity).expect("a positive capacity");
    for n in 0..capacity as i64 {
        memory.record_episode(episode(n, "macro"));
    }
    // Premise: the memory is full and nothing has been given up yet.
    assert_eq!(memory.episode_count(), capacity);
    assert_eq!(memory.evicted(), 0);

    memory.record_episode(episode(capacity as i64, "macro"));

    assert_eq!(memory.episode_count(), capacity);
    assert_eq!(memory.evicted(), 1);
    let held: Vec<String> = memory
        .episodes()
        .iter()
        .map(|e| e.run_id.to_string())
        .collect();
    assert_eq!(held, ["run-1", "run-2", "run-3"]);
}

#[test]
fn an_agents_episodes_after_eviction_are_exactly_the_held_ones_of_that_agent() {
    let mut memory = ResearchMemory::with_capacity(2).expect("a positive capacity");
    memory.record_episode(episode(0, "macro"));
    memory.record_episode(episode(1, "credit"));
    memory.record_episode(episode(2, "macro"));

    let far = now().saturating_add(Duration::from_days(1));
    // Premise: the first macro episode was the one given up.
    assert_eq!(memory.evicted(), 1);
    let macro_runs: Vec<String> = memory
        .episodes_as_of("macro", far)
        .into_iter()
        .map(|e| e.run_id.to_string())
        .collect();
    assert_eq!(macro_runs, ["run-2"]);
    let credit_runs: Vec<String> = memory
        .episodes_as_of("credit", far)
        .into_iter()
        .map(|e| e.run_id.to_string())
        .collect();
    assert_eq!(credit_runs, ["run-1"]);
}

#[test]
fn an_agent_whose_every_episode_was_given_up_leaves_the_index() {
    let mut memory = ResearchMemory::with_capacity(2).expect("a positive capacity");
    memory.record_episode(episode(0, "credit"));
    memory.record_episode(episode(1, "macro"));
    // Premise: both agents are indexed while each holds an episode.
    assert_eq!(memory.agents().collect::<Vec<_>>(), ["credit", "macro"]);

    memory.record_episode(episode(2, "macro"));

    assert_eq!(memory.agents().collect::<Vec<_>>(), ["macro"]);
}

#[test]
fn a_research_memory_of_capacity_zero_is_refused() {
    assert!(ResearchMemory::with_capacity(0).is_err());
    assert_eq!(
        ResearchMemory::new().capacity(),
        qip_agents::memory::DEFAULT_EPISODE_CAPACITY
    );
}

fn resolved(n: i64, correct: bool) -> Episode {
    let mut e = episode(n, "macro");
    e.outcome = Some(qip_agents::memory::EpisodeOutcome {
        resolved_at: now().saturating_add(Duration::from_days(1)),
        correct,
        realised_bps: None,
        explanation: "the print landed".to_string(),
    });
    e
}

fn run(n: i64) -> qip_core::ids::AgentRunId {
    qip_core::ids::AgentRunId::from_string(format!("run-{n}"))
}

/// R12-COVERAGE-7153: procedural memory exists beside the episodic and the
/// semantic, and a procedure is recorded only on enough episodes that
/// followed it and were right — `MemoryKind::Procedural` was a label with no
/// store behind it. Mutation: recording on a single correct episode admits
/// the two-episode procedure.
#[test]
fn a_procedure_is_recorded_only_after_enough_correct_episodes_and_each_record_raises_its_version() {
    let mut memory = ResearchMemory::new();
    for n in 0..3 {
        memory.record_episode(resolved(n, true));
    }
    memory.record_episode(resolved(3, false));
    memory.record_episode(episode(4, "macro"));
    let later = now().saturating_add(Duration::from_days(2));
    let steps = || {
        vec![
            "read the release".to_string(),
            "compare consensus".to_string(),
        ]
    };

    assert!(
        memory
            .record_procedure("earnings-review", steps(), vec![run(0), run(1)], later)
            .is_err(),
        "two correct episodes recorded a procedure"
    );
    assert!(
        memory
            .record_procedure(
                "earnings-review",
                steps(),
                vec![run(0), run(1), run(3)],
                later
            )
            .is_err(),
        "a wrong episode was cited as support"
    );
    assert!(
        memory
            .record_procedure(
                "earnings-review",
                steps(),
                vec![run(0), run(1), run(4)],
                later
            )
            .is_err(),
        "an unresolved episode was cited as support"
    );
    assert!(
        memory
            .record_procedure(
                "earnings-review",
                steps(),
                vec![run(0), run(0), run(1)],
                later
            )
            .is_err(),
        "a repeated episode counted twice"
    );
    // Premise: nothing was recorded by the refusals.
    assert!(memory.procedure_as_of("earnings-review", later).is_none());

    let support = vec![run(0), run(1), run(2)];
    assert_eq!(
        memory
            .record_procedure("earnings-review", steps(), support.clone(), later)
            .unwrap(),
        1
    );
    assert_eq!(
        memory
            .record_procedure("earnings-review", steps(), support, later)
            .unwrap(),
        2
    );
    assert_eq!(
        memory
            .procedure_as_of("earnings-review", later)
            .map(|p| p.version),
        Some(2)
    );
    assert!(memory.procedure_as_of("earnings-review", now()).is_none());
}

/// R12-AMBIENT-3024: the record per (agent, regime) the Meta Brain weighs a
/// panel by. Point-in-time in both instants — an episode that had not
/// happened, or had not resolved, by `as_of` is not yet a record — and an
/// episode with no regime is left out rather than filed under a regime
/// nobody recorded.
#[test]
fn the_record_by_regime_counts_only_what_had_resolved_by_then_and_only_under_its_own_regime() {
    let mut memory = ResearchMemory::new();
    let tagged = |n: i64, regime: Option<&str>, correct: bool| {
        let mut e = resolved(n, correct);
        e.regime = regime.map(str::to_string);
        e
    };
    memory.record_episode(tagged(1, Some("trending/high"), true));
    memory.record_episode(tagged(2, Some("trending/high"), false));
    memory.record_episode(tagged(3, Some("calm/low"), true));
    memory.record_episode(tagged(4, None, true));
    // Resolved only after the read: not yet a record.
    let mut late = tagged(5, Some("calm/low"), false);
    if let Some(outcome) = late.outcome.as_mut() {
        outcome.resolved_at = now().saturating_add(Duration::from_days(30));
    }
    memory.record_episode(late);

    let as_of = now().saturating_add(Duration::from_days(2));
    let cells = memory.reliability_by_regime(as_of);
    let key = |regime: &str| ("macro".to_string(), regime.to_string());
    assert_eq!(cells.len(), 2, "{cells:?}");
    let trending = cells[&key("trending/high")];
    assert_eq!((trending.correct, trending.resolved), (1, 2));
    assert_eq!(trending.hit_rate(), Some(0.5));
    let calm = cells[&key("calm/low")];
    assert_eq!(
        (calm.correct, calm.resolved),
        (1, 1),
        "the late resolution leaked in"
    );
    // The untagged episode is in no regime's record.
    let counted: usize = cells.values().map(|record| record.resolved).sum();
    assert_eq!(counted, 3, "the untagged or late episode was counted");
}

/// R12-WORLD-3034: a procedure recorded as a method carries rules an agent
/// can execute, and a brief checks a call against its agent's own rule for
/// the brief's subject: followed when the call leans the rule's way, not
/// followed otherwise, and no check where the brief names no such procedure
/// or the rule is for another subject. A rule nobody could follow is refused.
///
/// Mutations (2026-11-12, t50-09): `followed: true` unconditionally in
/// `AgentBrief::check_procedure` (the opposing call reads as followed);
/// skipping the `rules` assignment in `record_method` (the stored procedure
/// holds no rule, so nothing is checked); admitting a lean other than
/// positive or negative (the `neutral` rule is recorded).
#[test]
fn a_brief_checks_a_call_against_its_agents_own_procedure_rule() {
    use qip_agents::finding::{AgentBrief, Direction};
    use qip_agents::memory::{ProcedureRule, procedure_name_for};
    let mut memory = ResearchMemory::new();
    let at = Timestamp::from_secs(1_000_000);
    let runs: Vec<AgentRunId> = (0..3)
        .map(|i| {
            let run_id = AgentRunId::from_string(format!("run-{i}"));
            memory.record_episode(Episode {
                run_id: run_id.clone(),
                agent_id: "equity-analyst".to_string(),
                occurred_at: Timestamp::from_secs(1_000 + i),
                question: "which way".to_string(),
                conclusion: "positive".to_string(),
                evidence: vec!["subject:obj-ACME".to_string()],
                conviction: 0.6,
                regime: None,
                outcome: Some(qip_agents::memory::EpisodeOutcome {
                    resolved_at: Timestamp::from_secs(2_000 + i),
                    correct: true,
                    realised_bps: Some(30.0),
                    explanation: "it moved".to_string(),
                }),
            });
            run_id
        })
        .collect();
    let name = procedure_name_for("equity-analyst");
    let rule = |subject: &str, lean: &str| ProcedureRule {
        subject: subject.to_string(),
        lean: lean.to_string(),
        support: 3,
    };
    for bad in [
        vec![],
        vec![rule(" ", "positive")],
        vec![rule("obj-ACME", "neutral")],
        vec![rule("obj-ACME", "positive"), rule("obj-ACME", "negative")],
        vec![ProcedureRule {
            support: 0,
            ..rule("obj-ACME", "positive")
        }],
    ] {
        assert!(
            memory
                .record_method(&name, bad.clone(), runs.clone(), at)
                .is_err(),
            "an unfollowable method was recorded: {bad:?}"
        );
    }
    assert!(
        memory.procedure_as_of(&name, at).is_none(),
        "premise: nothing recorded"
    );
    assert_eq!(
        memory
            .record_method(&name, vec![rule("obj-ACME", "positive")], runs, at)
            .unwrap(),
        1
    );
    let procedure = memory.procedure_as_of(&name, at).cloned().unwrap();
    assert_eq!(procedure.rules, vec![rule("obj-ACME", "positive")]);
    assert_eq!(
        procedure.steps,
        vec!["on obj-ACME: lean positive (3 correct call(s))"]
    );

    let brief = |subject: &str| {
        AgentBrief::new("q", at, Duration::from_days(1))
            .about_objects(vec![qip_core::ObjectId::from_string(subject)])
            .with_procedures(vec![procedure.clone()])
    };
    let followed = brief("obj-ACME")
        .check_procedure("equity-analyst", Direction::Positive)
        .unwrap();
    assert!(followed.followed, "{followed:?}");
    let departed = brief("obj-ACME")
        .check_procedure("equity-analyst", Direction::Negative)
        .unwrap();
    assert!(!departed.followed, "{departed:?}");
    assert_eq!(
        departed.departure(),
        "departs from procedure:equity-analyst v1: its correct calls on obj-ACME leaned \
         positive, this one leans negative"
    );
    assert!(
        brief("obj-ACME")
            .check_procedure("macro-analyst", Direction::Positive)
            .is_none()
    );
    assert!(
        brief("obj-OTHER")
            .check_procedure("equity-analyst", Direction::Positive)
            .is_none()
    );
}

/// A regrade must not erase the first grade: the episode shows the latest
/// outcome and the replaced one stays on record. Mutation: dropping the
/// push in `resolve` empties the superseded list.
#[test]
fn a_regraded_episode_keeps_the_outcome_it_replaced_on_record() {
    let outcome = |correct: bool, why: &str| qip_agents::memory::EpisodeOutcome {
        resolved_at: now().saturating_add(Duration::from_days(1)),
        correct,
        realised_bps: None,
        explanation: why.to_string(),
    };
    let mut memory = ResearchMemory::new();
    memory.record_episode(episode(1, "macro"));
    assert!(memory.superseded_outcomes(&run(1)).is_empty());
    assert!(memory.resolve(&run(1), outcome(true, "first print")));
    assert!(memory.superseded_outcomes(&run(1)).is_empty());
    assert!(memory.resolve(&run(1), outcome(false, "revised print")));
    let kept = memory.superseded_outcomes(&run(1));
    assert_eq!(kept.len(), 1);
    assert!(kept[0].correct);
    assert_eq!(kept[0].explanation, "first print");
    let current = memory.episodes()[0].outcome.as_ref().map(|o| o.correct);
    assert_eq!(current, Some(false));
}
