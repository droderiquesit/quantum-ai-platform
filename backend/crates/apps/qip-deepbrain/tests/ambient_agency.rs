//! The ambient mesh reaching the agency guard in the deep brain's run loop
//! (R12-AGENCY-7029, R12-COVERAGE-7190): a REACHABILITY test, not a source
//! grep. The node runs two cycles with the research loop, the ambient journal
//! and the agency door attached; every draft in the outbox must answer a
//! signal the journal recorded as activated, under the journal's correlation.
//! Removing the `agency.route` call in `node::run_with`, or the guard behind
//! it, empties the outbox and fails these tests.
//!
//! Each test asserts its premise first, so an empty outbox cannot pass for a
//! quiet mesh.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_agency::outbox::{OUTBOX_ORIGIN, drafts};
use qip_contracts::ambient::Disposition;
use qip_core::{Clock, Context, Duration, ManualClock, Timestamp};
use qip_deepbrain::ambient_agency::AmbientAgency;
use qip_deepbrain::ambient_journal::{AmbientJournal, AmbientRecord, JournalDestination, records};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop};
use qip_deepbrain::status::NodeStatus;
use qip_events::EventLog;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

struct Ran {
    journal: Vec<AmbientRecord>,
    outbox: Vec<qip_agency::outbox::OutboxDraft>,
    outbox_log_ok: bool,
    producers: Vec<String>,
    rounds: Vec<qip_deepbrain::ambient_agency::AgencyRound>,
    side_failures: Vec<String>,
}

fn run_node(every: u64, attach_agency: bool, outbox: EventLog) -> Ran {
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
        max_cycles: Some(2),
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
    let coverage = BTreeMap::from([("Equity".to_string(), set(&["obj-A", "obj-B"]))]);
    let mut research = ResearchLoop::new(
        ResearchConfig {
            every_cycles: every,
            ..ResearchConfig::default()
        },
        coverage,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut ambient = AmbientJournal::open(&JournalDestination::InMemory).unwrap();
    let mut agency = AmbientAgency::over(outbox).unwrap_or_else(|e| panic!("{e}"));
    let mut rounds = Vec::new();
    let mut side_failures = Vec::new();
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
            desk: None,
            research: Some(&mut research),
            ambient: Some(&mut ambient),
            agency: if attach_agency {
                Some(&mut agency)
            } else {
                None
            },
            ..Loops::default()
        },
        |outcome| {
            side_failures.extend(outcome.side_failures.clone());
            if let Some(round) = &outcome.agency {
                rounds.push(round.clone());
            }
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let log = agency.intake().outbox();
    Ran {
        journal: records(ambient.log()).unwrap(),
        outbox: drafts(log).unwrap(),
        outbox_log_ok: log.verify_chain().is_ok(),
        producers: log.events().map(|e| e.lineage.producer.clone()).collect(),
        rounds,
        side_failures,
    }
}

fn activated_ids(journal: &[AmbientRecord]) -> BTreeSet<String> {
    journal
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Attention { event, .. }
                if event.disposition == Disposition::Activated =>
            {
                Some(event.signal_id.clone())
            }
            _ => None,
        })
        .collect()
}

/// Success and reachability: ambient output flows run loop -> promotion ->
/// guard -> outbox, with the signal's identity, evidence, detector and the
/// journal's correlation intact. Mutation: delete the `agency.route` call in
/// `node::run_with` (or make `AmbientAgency::route` return early) — the
/// outbox is empty and the premise assertion fails.
#[test]
fn the_run_loop_routes_activated_ambient_signals_through_the_guard_into_the_outbox() {
    let ran = run_node(1, true, EventLog::in_memory());
    assert!(ran.side_failures.is_empty(), "{:?}", ran.side_failures);
    let activated = activated_ids(&ran.journal);
    assert!(
        !activated.is_empty(),
        "premise: the router activated a signal"
    );
    assert!(
        !ran.outbox.is_empty(),
        "the outbox holds the drafts: {:?}",
        ran.rounds
    );
    assert!(ran.outbox_log_ok);
    assert!(ran.producers.iter().all(|p| p == OUTBOX_ORIGIN));

    let signals: BTreeMap<String, &qip_contracts::ambient::AmbientSignal> = ran
        .journal
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Signal { signal, .. } => Some((signal.id().to_string(), signal)),
            _ => None,
        })
        .collect();
    for draft in &ran.outbox {
        assert!(
            activated.contains(&draft.origin_signal),
            "{}",
            draft.origin_signal
        );
        let signal = signals[&draft.origin_signal];
        assert_eq!(draft.detected_at, signal.detected_at());
        assert_eq!(draft.evidence_ids, signal.evidence_ids());
        assert!(draft.source.ends_with(match signal.trigger() {
            qip_contracts::ambient::Trigger::Schedule(n)
            | qip_contracts::ambient::Trigger::Event(n) => n.as_str(),
        }));
        assert!(
            draft.correlation_id.starts_with("ambient-sweep-"),
            "{}",
            draft.correlation_id
        );
        assert_eq!(
            draft.promoted_by,
            qip_deepbrain::ambient_agency::PROMOTION_APPROVER
        );
        assert!(draft.tool_decisions.iter().all(|d| d.admitted));
    }
    let offered: usize = ran.rounds.iter().map(|r| r.offered).sum();
    let activated_events = ran
        .journal
        .iter()
        .filter(|r| matches!(r, AmbientRecord::Attention { event, .. } if event.disposition == Disposition::Activated))
        .count();
    assert_eq!(
        offered, activated_events,
        "exactly the activated events are offered"
    );
    assert!(
        ran.rounds
            .iter()
            .all(|r| r.refused.is_empty() && r.withheld.is_empty()),
        "a healthy sweep is drafted whole: {:?}",
        ran.rounds
    );
    let drafted: usize = ran.rounds.iter().map(|r| r.drafted).sum();
    assert_eq!(
        drafted,
        ran.outbox.len(),
        "the cycle lines count what the outbox holds"
    );
}

/// Disabled paths write nothing: no agency attached, or a research loop that
/// never ran. Mutation: have `AmbientAgency::route` draft from `round.signals`
/// directly, skipping attention — the second case writes a draft.
#[test]
fn without_the_agency_or_without_a_sweep_nothing_is_drafted() {
    let detached = run_node(1, false, EventLog::in_memory());
    assert!(
        !activated_ids(&detached.journal).is_empty(),
        "premise: signals were activated"
    );
    assert!(detached.outbox.is_empty());
    assert!(detached.rounds.is_empty());
    let idle = run_node(0, true, EventLog::in_memory());
    assert!(idle.journal.is_empty(), "premise: no sweep ran");
    assert!(idle.outbox.is_empty());
}

/// Failure: a full outbox does not stop the node. The refusal is counted
/// (held for a retry), the node completes both cycles, and the log still
/// verifies. Mutation: in `AmbientIntake::attempt`, return the write failure
/// as an `Err` instead of holding the draft — `route` counts it refused and
/// the deferred-or-abandoned assertion fails.
#[test]
fn a_full_outbox_defers_drafts_and_never_stops_the_node() {
    let ran = run_node(1, true, EventLog::in_memory().with_capacity(1).unwrap());
    assert!(ran.side_failures.is_empty(), "{:?}", ran.side_failures);
    assert_eq!(ran.outbox.len(), 1, "premise: room for exactly one draft");
    let deferred: usize = ran.rounds.iter().map(|r| r.deferred).sum();
    let abandoned: usize = ran.rounds.iter().map(|r| r.abandoned).sum();
    assert!(
        deferred + abandoned > 0,
        "the overflow is on the cycle line: {:?}",
        ran.rounds
    );
    assert!(ran.outbox_log_ok);
}
