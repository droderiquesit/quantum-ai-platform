//! The attention router's scope is covered by what it reported, not by its
//! registration (R12-AMBIENT-3027, ADR 0725).
//!
//! The Brain Registry holds `ambient/attention` over `attention-routing`, and
//! `tests/brains.rs` proves removing that entry removes the scope from the
//! registry. That is a statement about a record. These tests are about the
//! router: the same loop, on the same platform, with and without it.

use qip_contracts::ambient::{Disposition, Pathway};
use qip_core::{Clock, Context, Duration, ManualClock, Timestamp};
use qip_deepbrain::ambient_coverage::{
    ATTENTION_ACTIVATED_FACT, ATTENTION_ROUTING_FACT, attention_finding,
};
use qip_deepbrain::ambient_journal::{AmbientJournal, AmbientRecord, JournalDestination, records};
use qip_deepbrain::brains::AMBIENT_ATTENTION;
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop, ResearchRound};
use qip_deepbrain::status::NodeStatus;
use qip_financial::universe::Universe;
use qip_investment_agents::coverage::{AmbientScope, Scope, ambient_covered_by, covered_by};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("the platform assembles: {e}"))
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

/// The deployed default loop over one asset class with no history: the
/// coverage detector raises one gap, which is one signal to route.
fn research() -> ResearchLoop {
    let coverage: BTreeMap<String, BTreeSet<String>> =
        [("Equity".to_string(), set(&["obj-A", "obj-B"]))].into();
    ResearchLoop::new(ResearchConfig::default(), coverage)
        .unwrap_or_else(|e| panic!("the research loop assembles: {e}"))
}

fn sweep(mut research: ResearchLoop, platform: &Platform) -> ResearchRound {
    research
        .maybe_sweep(platform, 1, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("the deployed default sweeps every cycle"))
}

fn covered_gauge(platform: &Platform) -> Option<f64> {
    platform.telemetry().metrics.snapshot().gauge(
        names::AMBIENT_SCOPE_COVERED,
        &labels([("scope", AmbientScope::AttentionRouting.as_str())]),
    )
}

/// The failure this prevents: the router's scope read as covered because a
/// registry entry said so, so a deep brain whose router was gone still
/// reported attention routing. Mutation: skip the finding composition in
/// `ResearchLoop::sweep_over` (leave `round.attention_finding` at `None`);
/// the with-router half fails on the missing finding.
#[test]
fn removing_the_attention_router_drops_attention_routing_from_covered_scopes() {
    let with_platform = platform();
    let without_platform = platform();
    let with = sweep(research(), &with_platform);
    let without = sweep(research().without_attention_router(), &without_platform);

    // Premise: both loops had the same signals to route, so the difference
    // below is the router and not a quiet sweep.
    assert_eq!(with.signals.len(), 1, "premise: one signal: {with:?}");
    assert_eq!(
        with.signals, without.signals,
        "premise: the same signals reached the routing step in both loops"
    );
    assert_eq!(
        with.attention_events.len(),
        1,
        "premise: the router returned one decision: {:?}",
        with.attention_events
    );
    assert_eq!(
        with.attention_events[0].disposition,
        Disposition::Activated,
        "premise: and it was an activation"
    );

    let finding = with
        .attention_finding
        .as_ref()
        .expect("the sweep composes one finding from its attention events");
    finding
        .validate()
        .unwrap_or_else(|e| panic!("the finding is well formed: {e}"));
    assert_eq!(finding.agent_id, AMBIENT_ATTENTION);
    let event_ids: Vec<String> = with.attention_events.iter().map(|e| e.id.clone()).collect();
    assert_eq!(finding.evidence, event_ids, "it cites exactly the events");
    let routed = finding
        .fact(ATTENTION_ROUTING_FACT)
        .expect("the attention_routing fact");
    assert_eq!(
        routed.value.to_bits(),
        1.0_f64.to_bits(),
        "one decision, counted once"
    );
    let activated = finding
        .fact(ATTENTION_ACTIVATED_FACT)
        .expect("the activated fact");
    assert_eq!(activated.value.to_bits(), 1.0_f64.to_bits());
    assert_eq!(
        with.attention_events[0].pathway,
        Pathway::ReflexModelUpdate,
        "premise: a coverage gap is routed to the reflex-model pathway"
    );
    let woke = finding
        .fact("attention_woke_reflex_model_update")
        .expect("the pathway the activation woke");
    assert_eq!(woke.value.to_bits(), 1.0_f64.to_bits());
    let labels: Vec<&str> = finding.facts.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            ATTENTION_ROUTING_FACT,
            ATTENTION_ACTIVATED_FACT,
            "attention_woke_reflex_model_update"
        ],
        "exactly these facts, and no pathway the router did not wake"
    );

    let all: BTreeSet<AmbientScope> = [AmbientScope::AttentionRouting].into();
    assert_eq!(with.ambient_scopes, all, "covered with the router");
    assert_eq!(covered_gauge(&with_platform), Some(1.0));

    assert_eq!(
        without.attention_events,
        Vec::new(),
        "no router, no decision"
    );
    assert_eq!(without.attention_finding, None, "no decision, no finding");
    assert_eq!(
        without.ambient_scopes,
        BTreeSet::new(),
        "the scope is dropped with the router"
    );
    assert_eq!(
        covered_gauge(&without_platform),
        Some(0.0),
        "and the series says so rather than staying absent"
    );
}

/// The ambient scope and the Desk's scopes never read each other's labels:
/// the attention finding covers no Desk scope, and `Scope::ALL` holds no
/// variant named for it. Mutation: rename the fact the finding carries to
/// `attention_allocated` (the Desk's `AttentionAllocation` label); the first
/// assertion fails because a Desk scope is then covered by a brain with no
/// seat on the roster.
#[test]
fn the_attention_finding_covers_its_ambient_scope_and_no_desk_scope() {
    let round = sweep(research(), &platform());
    let finding = round.attention_finding.expect("premise: a finding");
    assert_eq!(covered_by([&finding]), BTreeSet::new());
    assert_eq!(
        ambient_covered_by([&finding]),
        [AmbientScope::AttentionRouting].into()
    );
    let desk: Vec<&str> = Scope::ALL.iter().map(|s| s.as_str()).collect();
    assert!(!desk.is_empty(), "premise: the Desk has scopes");
    assert!(
        !desk.contains(&AmbientScope::AttentionRouting.as_str()),
        "the ambient scope was added to the Desk's set: {desk:?}"
    );

    // A finding that carries the label but no information covers nothing.
    let mut deferred = finding.clone();
    deferred.status = qip_agents::finding::FindingStatus::Deferred;
    assert_eq!(ambient_covered_by([&deferred]), BTreeSet::new());
    // And a label the scope's is a substring of covers nothing either.
    let mut neighbour = finding;
    for fact in &mut neighbour.facts {
        if fact.label == ATTENTION_ROUTING_FACT {
            fact.label = "attention_routing_total".to_string();
        }
    }
    assert_eq!(ambient_covered_by([&neighbour]), BTreeSet::new());
}

/// A sweep whose router returned nothing composes no finding, rather than a
/// finding stating zero that would cover the scope on an absent router.
/// Mutation: compose from an empty slice instead of returning `None`; the
/// call no longer answers `Ok(None)` and this fails.
#[test]
fn no_attention_events_compose_no_finding() {
    assert_eq!(
        attention_finding(&[], start()).unwrap_or_else(|e| panic!("{e}")),
        None
    );
}

/// One cycle of the deep brain's own run loop, with the research loop and
/// the ambient journal attached as `main.rs` attaches them.
fn run_node(mut research: ResearchLoop) -> (Vec<ResearchRound>, Vec<AmbientRecord>) {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
    let config = PlatformConfig::default();
    let context = Context::new(clock.clone(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(1),
        ..DeepBrainConfig::default()
    };
    let roster = qip_deepbrain::roster::clear(start()).unwrap_or_else(|e| panic!("{e}"));
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start(),
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut ambient =
        AmbientJournal::open(&JournalDestination::InMemory).unwrap_or_else(|e| panic!("{e}"));
    let mut rounds = Vec::new();
    let mut side_failures: Vec<String> = Vec::new();
    node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        None,
        Loops {
            research: Some(&mut research),
            ambient: Some(&mut ambient),
            ..Loops::default()
        },
        |outcome| {
            side_failures.extend(outcome.side_failures.clone());
            rounds.extend(outcome.research.clone());
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        side_failures,
        Vec::<String>::new(),
        "premise: a clean cycle"
    );
    let journal = records(ambient.log()).unwrap_or_else(|e| panic!("{e}"));
    (rounds, journal)
}

/// Through the run loop: the finding is on the ambient journal after the
/// attention records it cites, once per sweep, and absent from the journal
/// of a node whose loop has no router. A finding held only on the round
/// would be a report nobody could reopen. Mutation: delete the
/// `record_attention_finding` call in `node.rs::research_pass`; the
/// with-router journal holds none.
#[test]
fn the_run_loop_journals_the_attention_finding_after_the_events_it_cites_and_not_without_a_router()
{
    let (rounds, entries) = run_node(research());
    assert_eq!(rounds.len(), 1, "premise: one cycle, one sweep");
    let expected = rounds[0]
        .attention_finding
        .clone()
        .expect("premise: the run loop's sweep composed a finding");
    assert_eq!(
        rounds[0].ambient_scopes,
        [AmbientScope::AttentionRouting].into(),
        "the scope is covered on the round the run loop hands on"
    );

    let findings: Vec<(usize, &AmbientRecord)> = entries
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, AmbientRecord::AttentionFinding { .. }))
        .collect();
    assert_eq!(findings.len(), 1, "one finding per sweep: {entries:?}");
    let AmbientRecord::AttentionFinding { cycle, finding } = findings[0].1 else {
        panic!("filtered to attention findings");
    };
    assert_eq!(finding, &expected, "the journal holds the round's finding");
    let sweep_cycles: BTreeSet<u64> = entries
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Attention { cycle, .. } => Some(*cycle),
            _ => None,
        })
        .collect();
    assert_eq!(
        sweep_cycles,
        [*cycle].into(),
        "the finding is filed under the cycle of the events it cites"
    );
    let cited: Vec<(usize, String)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r {
            AmbientRecord::Attention { event, .. } => Some((i, event.id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(cited.len(), 1, "premise: one attention record: {entries:?}");
    assert_eq!(
        cited.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>(),
        expected.evidence,
        "every event the finding cites has its Attention record, and no other"
    );
    assert!(
        cited.iter().all(|(i, _)| *i < findings[0].0),
        "the finding precedes a record it cites: {entries:?}"
    );

    let (rounds, entries) = run_node(research().without_attention_router());
    assert_eq!(rounds.len(), 1, "premise: the routerless node swept too");
    assert_eq!(
        rounds[0].signals.len(),
        1,
        "premise: with a signal to route"
    );
    assert_eq!(rounds[0].ambient_scopes, BTreeSet::new());
    assert_eq!(
        entries
            .iter()
            .filter(|r| matches!(r, AmbientRecord::Signal { .. }))
            .count(),
        1,
        "premise: the routerless sweep journaled its signal: {entries:?}"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|r| matches!(r, AmbientRecord::AttentionFinding { .. }))
            .count(),
        0,
        "a node with no router journals no attention finding"
    );
}
