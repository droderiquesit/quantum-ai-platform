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
