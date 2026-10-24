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

use qip_agency::autonomy::Authority;
use qip_agency::memory::Observed;
use qip_agency::outbox::DRAFT_TOOL;
use qip_agency::outbox::{OUTBOX_ORIGIN, drafts};
use qip_contracts::ambient::Disposition;
use qip_contracts::expansion::Identification;
use qip_core::{Clock, Context, Duration, ManualClock, Timestamp};
use qip_deepbrain::ambient_agency::{
    ACTING_IDENTITY, AmbientAgency, DraftLearning, LEARNING_ORIGIN, learnings,
};
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
    learnings: Vec<DraftLearning>,
    learning_log_ok: bool,
    learning_producers: Vec<String>,
    memory_len: usize,
    authority: Authority,
    /// The platform's own event log, as event ids in order: what the
    /// decision loop recorded, independent of anything the agency did.
    platform_events: Vec<String>,
}

/// How to run the node for one test.
struct Run {
    every: u64,
    attach_agency: bool,
    outbox: EventLog,
    learning: EventLog,
    cycles: u64,
    /// How far the clock moves after each cycle. `None` holds it still, as
    /// the earlier tests always did.
    advance: Option<Duration>,
    /// Revoke the drafting tool before the first cycle, so every draft the
    /// guard reaches is withheld.
    revoke: bool,
}

impl Run {
    fn new(every: u64, attach_agency: bool, outbox: EventLog) -> Self {
        Self {
            every,
            attach_agency,
            outbox,
            learning: EventLog::in_memory(),
            cycles: 2,
            advance: None,
            revoke: false,
        }
    }

    /// Three cycles with a moving clock: the second sweep reads the first
    /// sweep's subjects again, a minute later.
    fn learning(outbox: EventLog) -> Self {
        Self {
            cycles: 3,
            advance: Some(Duration::from_secs(60)),
            ..Self::new(1, true, outbox)
        }
    }
}

fn run_node(every: u64, attach_agency: bool, outbox: EventLog) -> Ran {
    run(Run::new(every, attach_agency, outbox))
}

fn run(options: Run) -> Ran {
    let manual = Arc::new(ManualClock::new(start()));
    let clock: Arc<dyn Clock> = manual.clone();
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
        max_cycles: Some(options.cycles),
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
            every_cycles: options.every,
            ..ResearchConfig::default()
        },
        coverage,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut ambient = AmbientJournal::open(&JournalDestination::InMemory).unwrap();
    let mut agency = AmbientAgency::over_with(options.outbox, options.learning)
        .unwrap_or_else(|e| panic!("{e}"));
    if options.revoke {
        agency.intake_mut().revoke().unwrap();
    }
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
            agency: if options.attach_agency {
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
            if let Some(step) = options.advance {
                manual.advance(step);
            }
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let log = agency.intake().outbox();
    let learning_log = agency.learning_log();
    Ran {
        journal: records(ambient.log()).unwrap(),
        outbox: drafts(log).unwrap(),
        outbox_log_ok: log.verify_chain().is_ok(),
        producers: log.events().map(|e| e.lineage.producer.clone()).collect(),
        rounds,
        side_failures,
        learnings: learnings(learning_log).unwrap(),
        learning_log_ok: learning_log.verify_chain().is_ok(),
        learning_producers: learning_log
            .events()
            .map(|e| e.lineage.producer.clone())
            .collect(),
        memory_len: agency.memory().len(),
        authority: agency.policy().authority(),
        platform_events: platform
            .event_log()
            .events()
            .map(|e| e.event_id.as_str().to_string())
            .collect(),
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

/// Every stage of §24's loop, as the learning record spells them.
fn all_eleven() -> Vec<String> {
    [
        "Observe",
        "Predict",
        "Specify",
        "IdentifyLevers",
        "Generate",
        "Simulate",
        "Gate",
        "Act",
        "ObserveEffect",
        "Attribute",
        "Update",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// R12-AGENCY-1006 / 1025 / 1152 / 6107 / 7148: the one executed act in the
/// platform — the outbox write — is held to a measured outcome on the
/// default path. A draft's subject read again on a later sweep runs
/// observe-effect, attribute and update through `qip_agency::engine::learn`;
/// the memory keeps the outcome as measured, the effect is `observed -
/// counterfactual` with the status quo as the no-action baseline, and the
/// attribution says it is not identified. Premise asserted first: a draft
/// was written and its subject was raised again later. Mutation: delete the
/// `self.observe(..)` call in `AmbientAgency::route` — no learning is
/// journaled, the memory is empty, and the premise still holds.
#[test]
fn a_drafted_subject_read_again_on_a_later_sweep_is_learned_from_as_a_measured_null_effect() {
    let ran = run(Run::learning(EventLog::in_memory()));
    assert!(ran.side_failures.is_empty(), "{:?}", ran.side_failures);
    let first_drafts: BTreeSet<&str> = ran
        .outbox
        .iter()
        .filter(|d| d.rate_window == 1)
        .map(|d| d.draft_id.as_str())
        .collect();
    assert!(!first_drafts.is_empty(), "premise: the first sweep drafted");
    let later_subjects: BTreeSet<String> = ran
        .journal
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Signal { cycle, signal } if *cycle > 1 => {
                Some(signal.subject().to_string())
            }
            _ => None,
        })
        .collect();
    assert!(
        !later_subjects.is_empty(),
        "premise: a later sweep raised a signal"
    );

    assert!(
        !ran.learnings.is_empty(),
        "a follow-up was learned from: {:?}",
        ran.rounds
    );
    assert!(ran.learning_log_ok);
    assert!(ran.learning_producers.iter().all(|p| p == LEARNING_ORIGIN));
    let learned: Vec<&DraftLearning> = ran
        .learnings
        .iter()
        .filter(|l| l.learned.is_some())
        .collect();
    assert!(
        !learned.is_empty(),
        "at least one learning passed every stage: {:?}",
        ran.learnings
            .iter()
            .map(|l| (&l.failed, &l.withheld))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        ran.memory_len,
        learned.len(),
        "the memory holds what was learned"
    );
    for learning in &ran.learnings {
        assert!(
            first_drafts.contains(learning.draft_id.as_str())
                || ran.outbox.iter().any(|d| d.draft_id == learning.draft_id),
            "{}",
            learning.draft_id
        );
        assert!(learning.observed_cycle > learning.drafted_cycle);
        assert!(later_subjects.contains(&learning.subject));
    }
    for learning in learned {
        let kept = learning.learned.as_ref().unwrap();
        assert_eq!(learning.passed, all_eleven());
        assert_eq!(learning.failed, None);
        assert_eq!(kept.outcome.observed, Observed::Measured);
        assert_eq!(kept.outcome.action_sequence, vec![DRAFT_TOOL.to_string()]);
        assert_eq!(
            kept.effect,
            kept.outcome
                .actual_observations
                .checked_sub(kept.outcome.no_action_counterfactual)
                .unwrap()
        );
        assert_eq!(kept.outcome.delay_secs, 60, "one sweep interval later");
        assert_eq!(
            kept.attribution.identification,
            Identification::NotIdentified
        );
        assert!(kept.attribution.identifiability.abs() < f64::EPSILON);
        assert!((kept.attribution.effect.mean - kept.effect.to_f64()).abs() < 1e-9);
        assert_eq!(kept.outcome.predicted_effect, qip_core::Decimal::ZERO);
        assert_eq!(kept.authority_before, Authority::Shadow);
        assert_eq!(kept.authority_after, Authority::Shadow);
    }
    assert_eq!(
        ran.authority,
        Authority::Shadow,
        "nothing on this path widens"
    );
    let learned_on_lines: usize = ran.rounds.iter().map(|r| r.learned).sum();
    assert_eq!(
        learned_on_lines,
        ran.learnings.iter().filter(|l| l.learned.is_some()).count()
    );
}

/// Failure: a follow-up a stage refuses is journaled withheld, naming the
/// stage, and the node goes on. With the clock held still the follow-up
/// sits at a horizon of zero milliseconds, which the attribution contract
/// refuses at `Attribute`; observe-effect has passed by then. Mutation: in
/// `AmbientAgency::observe`, skip the journal write when `withheld` is
/// `Some` — no record names the stage.
#[test]
fn a_follow_up_a_stage_refuses_is_journaled_withheld_naming_the_stage() {
    let ran = run(Run {
        advance: None,
        ..Run::learning(EventLog::in_memory())
    });
    assert!(ran.side_failures.is_empty(), "{:?}", ran.side_failures);
    assert!(!ran.outbox.is_empty(), "premise: drafts were written");
    assert!(
        !ran.learnings.is_empty(),
        "premise: a follow-up reached the learn stages: {:?}",
        ran.rounds
    );
    for learning in &ran.learnings {
        assert!(learning.learned.is_none(), "{learning:?}");
        assert_eq!(learning.failed.as_deref(), Some("Attribute"));
        assert!(learning.passed.contains(&"ObserveEffect".to_string()));
        assert!(
            learning
                .withheld
                .as_deref()
                .is_some_and(|w| w.contains("zero horizon")),
            "{:?}",
            learning.withheld
        );
    }
    assert_eq!(ran.memory_len, 0, "a refused follow-up is not remembered");
    let withheld: usize = ran
        .rounds
        .iter()
        .map(|r| r.learning_withheld.values().sum::<usize>())
        .sum();
    assert_eq!(withheld, ran.learnings.len(), "the cycle line counts each");
}

/// R12-AGENCY-6108: the memory is asked before every draft, for the regime
/// and the audience, and its answer is on the cycle line. Nothing it says
/// gates the draft (module documentation says why). Mutation: delete the
/// `self.ask(..)` call in `route` — `asked` stays zero while drafts are
/// written.
#[test]
fn the_memory_is_asked_before_every_offer_and_its_answer_is_on_the_cycle_line() {
    let ran = run(Run::learning(EventLog::in_memory()));
    assert!(ran.side_failures.is_empty(), "{:?}", ran.side_failures);
    let offered: usize = ran.rounds.iter().map(|r| r.offered).sum();
    assert!(offered > 0, "premise: opportunities were offered");
    let asked: usize = ran.rounds.iter().map(|r| r.asked).sum();
    assert_eq!(asked, offered, "every offer asked the memory first");
    for round in &ran.rounds {
        assert_eq!(
            round.asked,
            round.recommended_draft + round.recommended_nothing
        );
        // Every outcome this door files is at attribution confidence zero,
        // which the memory does not read (ADR 0145): it never recommends
        // drafting on this run, however the readings moved.
        assert_eq!(round.recommended_draft, 0, "{round:?}");
    }
}

/// R12-REASON-1137: the same ambient signals yield the same drafts on
/// replay. Two runs from the same seed, clock and coverage produce
/// byte-identical outboxes, so a draft can be reproduced from the sweep that
/// made it. Mutation: in `route`, build the opportunity's correlation from a
/// process-wide counter instead of the journal's — the second run's drafts
/// carry different correlations and the outboxes differ.
#[test]
fn the_same_sweep_replayed_yields_an_identical_outbox() {
    let first = run_node(1, true, EventLog::in_memory());
    let second = run_node(1, true, EventLog::in_memory());
    assert!(!first.outbox.is_empty(), "premise: drafts were written");
    assert_eq!(first.outbox, second.outbox);
    assert_eq!(first.journal.len(), second.journal.len());
}

/// R12-AGENCY-10063 / 1063: an agency refusal in one cycle is counted, never
/// raised, and changes nothing the decision loop recorded. With the
/// drafting tool revoked every draft is withheld by the guard, the node
/// completes every cycle, and the platform's event log is identical to a
/// run with no agency attached. Premise: drafts were withheld. Mutation: in
/// `route`, return `Err` on `Intake::Withheld` — the node reports a side
/// failure.
#[test]
fn a_revoked_drafting_tool_withholds_every_draft_and_leaves_the_platform_log_identical() {
    let revoked = run(Run {
        revoke: true,
        ..Run::new(1, true, EventLog::in_memory())
    });
    assert!(
        revoked.side_failures.is_empty(),
        "{:?}",
        revoked.side_failures
    );
    let withheld: usize = revoked
        .rounds
        .iter()
        .map(|r| r.withheld.values().sum::<usize>())
        .sum();
    assert!(
        withheld > 0,
        "premise: the guard withheld: {:?}",
        revoked.rounds
    );
    assert!(
        revoked
            .rounds
            .iter()
            .flat_map(|r| r.withheld.keys())
            .all(|reason| reason.contains(DRAFT_TOOL)),
        "{:?}",
        revoked.rounds
    );
    assert!(revoked.outbox.is_empty());
    let detached = run_node(1, false, EventLog::in_memory());
    assert!(
        !detached.platform_events.is_empty(),
        "premise: the loop recorded"
    );
    assert_eq!(revoked.platform_events, detached.platform_events);
}

/// R12-AGENCY-1063: every draft the node writes is executed under the one
/// identity the door registers, and promoted and planned by the two policy
/// identities, so there is no seam on this path for a draft under another
/// sender. Mutation: pass a different acting identity to `IntakeConfig::new`
/// in `AmbientAgency::over_with` — the executor on every draft changes.
#[test]
fn every_draft_the_node_writes_is_executed_under_the_one_registered_identity() {
    let ran = run_node(1, true, EventLog::in_memory());
    assert!(!ran.outbox.is_empty(), "premise: drafts were written");
    for draft in &ran.outbox {
        assert_eq!(draft.execution.executor, ACTING_IDENTITY);
        assert_eq!(
            draft.plan_approval.approver,
            qip_deepbrain::ambient_agency::PLAN_APPROVER
        );
        assert_ne!(draft.execution.executor, draft.plan_approval.approver);
        assert_ne!(draft.execution.executor, draft.promoted_by);
    }
}
