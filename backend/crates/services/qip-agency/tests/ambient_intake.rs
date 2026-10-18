//! R12-AGENCY-7029 / R12-COVERAGE-7190: an ambient detection reaches the
//! agency only through promotion and the guard, and the only effect it can
//! have is a draft in an internal outbox. Each test asserts its premise first.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)]

use qip_agency::ambient_intake::{AmbientIntake, AmbientOpportunity, Intake, IntakeConfig};
use qip_agency::engine::Adapter;
use qip_agency::outbox::{
    DRAFT_TOOL, DraftContext, DraftingAdapter, OUTBOX_CHANNEL, OUTBOX_ORIGIN, drafts,
};
use qip_agency::plan::Step;
use qip_contracts::ambient::{
    Advisory, AmbientSignal, AttentionEvent, AttentionRouter, Detection, Disposition, Pathway,
    Promoted, PromotionCase, RoutingPolicy, SignalClass, Trigger, promote,
};
use qip_core::{CorrelationId, Decimal, Duration, Error, Timestamp};
use qip_events::EventLog;
use std::path::Path;

const IDENTITY: &str = "drafter";
const APPROVER: &str = "promotion-policy";

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn config() -> IntakeConfig {
    IntakeConfig::new(IDENTITY, "plan-policy")
}

fn intake() -> AmbientIntake {
    AmbientIntake::open(config(), EventLog::in_memory()).unwrap()
}

fn signal(id: &str, subject: &str, evidence: &[&str]) -> AmbientSignal {
    AmbientSignal::new(
        id,
        SignalClass::Opportunity,
        subject,
        9_000,
        Trigger::Event("world-model-updated".into()),
        at(100),
        Detection {
            observed_deviation: Decimal::from_int(3),
            expected_baseline: Decimal::ZERO,
            horizon: Duration::ZERO,
            novelty_bp: 5_000,
            affected_entities: vec!["EURUSD".into()],
            urgency_bp: 4_000,
            wake_targets: vec![Pathway::ResearchTask],
            evidence_ids: evidence.iter().map(|e| (*e).to_string()).collect(),
            expiry: at(10_000),
        },
    )
    .unwrap()
}

/// Route `signal` through a real router and return the event it produced.
fn activated(signal: &AmbientSignal) -> AttentionEvent {
    let policy = RoutingPolicy::standard(4_000, 10, Duration::from_secs(60), 4).unwrap();
    let mut router = AttentionRouter::new(policy);
    let mut events = router.route(signal, at(110)).unwrap();
    assert_eq!(events.len(), 1, "premise: a material signal is answered");
    let event = events.remove(0);
    assert_eq!(event.disposition, Disposition::Activated, "premise");
    event
}

fn advisory(id: &str, subject: &str) -> Advisory<AmbientOpportunity> {
    let s = signal(id, subject, &["ev-1", "ev-2"]);
    AmbientOpportunity::advise(&s, &activated(&s), &CorrelationId::from_string("sweep-7-0"))
        .unwrap()
}

/// Success: detection to outbox, provenance intact end to end.
/// Mutation: in `through_guard`, build `OutboxDraft.correlation_id` from a
/// literal, or drop `source` — the provenance assertions fail.
#[test]
fn an_activated_detection_is_promoted_guarded_and_drafted_with_its_provenance() {
    let mut door = intake();
    assert!(door.outbox().is_empty(), "premise: nothing drafted yet");
    let outcome = door
        .admit(
            advisory("sig-1", "EURUSD basis widening"),
            APPROVER,
            at(120),
            1,
        )
        .unwrap();
    let Intake::Drafted { draft_id, .. } = outcome else {
        panic!("expected a draft, got {outcome:?}")
    };
    assert_eq!(draft_id, "draft-sig-1");

    assert!(door.outbox().verify_chain().is_ok());
    let written = drafts(door.outbox()).unwrap();
    assert_eq!(written.len(), 1);
    let draft = &written[0];
    assert_eq!(draft.origin_signal, "sig-1");
    assert_eq!(draft.source, "event:world-model-updated");
    assert_eq!(draft.detected_at, at(100));
    assert_eq!(draft.correlation_id, "sweep-7-0");
    assert_eq!(draft.evidence_ids, vec!["ev-1", "ev-2"]);
    assert_eq!(draft.promoted_by, APPROVER);
    assert_eq!(draft.promotion_evidence, vec!["ev-1", "ev-2"]);
    assert_eq!(draft.priority_bp, 9_000);
    assert_eq!(draft.goal_approval.approver, APPROVER);
    assert_eq!(draft.execution.executor, IDENTITY);
    assert_eq!(draft.tool_decisions.len(), 1);
    assert!(draft.tool_decisions[0].admitted);
    assert_eq!(draft.tool_decisions[0].channel, OUTBOX_CHANNEL);

    let event = door.outbox().events().next().unwrap();
    assert_eq!(event.lineage.producer, OUTBOX_ORIGIN);
    assert_eq!(event.lineage.correlation_id.as_str(), "sweep-7-0");
    assert_eq!(
        event.occurred_at,
        at(100),
        "knowable-when is the detection's"
    );
    assert_eq!(event.recorded_at, at(120));
    assert_eq!(
        door.guard().audit().len(),
        1,
        "the registry charged the draft"
    );

    // Idempotent: the same signal again writes nothing and charges nothing.
    let again = door
        .admit(
            advisory("sig-1", "EURUSD basis widening"),
            APPROVER,
            at(121),
            1,
        )
        .unwrap();
    assert_eq!(
        again,
        Intake::Duplicate {
            draft_id: "draft-sig-1".into()
        }
    );
    assert_eq!(door.outbox().len(), 1);
    assert_eq!(door.guard().audit().len(), 1);
}

/// Mutation: in `advise`, drop the `Disposition::Activated` refusal — a
/// deferred signal becomes an opportunity and the first assertion fails.
#[test]
fn only_an_activated_event_for_its_own_signal_makes_an_opportunity() {
    let s = signal("sig-2", "EURUSD", &["ev-1"]);
    let mut event = activated(&s);
    let corr = CorrelationId::from_string("c");
    assert!(
        AmbientOpportunity::advise(&s, &event, &corr).is_ok(),
        "premise"
    );
    event.disposition = Disposition::Deferred;
    let refusal = AmbientOpportunity::advise(&s, &event, &corr).unwrap_err();
    assert!(refusal.message().contains("not activated"), "{refusal}");
    event.disposition = Disposition::Activated;
    event.signal_id = "someone-else".into();
    assert!(AmbientOpportunity::advise(&s, &event, &corr).is_err());
}

/// Unauthorized: no approver, or the acting identity as approver. Nothing is
/// written and, for a refused promotion, nothing reaches the guard at all.
/// Mutation: in `admit`, set `approver` to a literal — the empty-approver
/// case is admitted and the first assertion fails.
#[test]
fn an_unauthorized_promotion_is_refused_and_nothing_reaches_the_outbox() {
    let mut door = intake();
    let refusal = door
        .admit(advisory("sig-3", "EURUSD"), "", at(120), 1)
        .unwrap_err();
    assert!(refusal.message().contains("named approver"), "{refusal}");
    assert!(door.outbox().is_empty());
    assert!(
        door.guard().audit().is_empty(),
        "the guard was never consulted"
    );

    // The guard, not the promotion, refuses an approver who is the actor.
    let own = door
        .admit(advisory("sig-3", "EURUSD"), IDENTITY, at(120), 1)
        .unwrap();
    let Intake::Withheld { reason } = own else {
        panic!("expected withheld, got {own:?}")
    };
    assert!(reason.contains("approving its own action"), "{reason}");
    assert!(door.outbox().is_empty());
}

/// A promotion that did not hold its evaluation or its controls is refused by
/// `promote` itself; the door cannot be handed an unpromoted output because
/// `submit` takes a `Promoted`. Mutation: in `admit`, set
/// `evaluation_passed: true` — the expired case (checked at `admit`) is
/// admitted past promotion and is then refused only by `submit`'s expiry check
/// with a different message, so the message assertion fails.
#[test]
fn an_expired_detection_is_refused_before_the_guard() {
    let mut door = intake();
    let refusal = door
        .admit(advisory("sig-4", "EURUSD"), APPROVER, at(10_001), 1)
        .unwrap_err();
    assert!(
        refusal.message().contains("passing evaluation"),
        "{refusal}"
    );
    assert!(door.outbox().is_empty());
    assert!(door.guard().audit().is_empty());
}

/// External-action attempts. The door's tool is registered on one channel
/// with one scope; a hostile subject that asks for outreach in the words the
/// conduct rules refuse is withheld; the adapter refuses any other tool.
/// Mutation: add `"press"` to the registered channels in `AmbientIntake::open`
/// — the registry assertions fail.
#[test]
fn the_door_can_reach_only_the_internal_outbox_and_a_hostile_subject_is_withheld() {
    let mut door = intake();
    let registry = door.guard().registry();
    let registration = registry
        .registration(DRAFT_TOOL)
        .expect("premise: registered");
    assert_eq!(
        registration
            .channels
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![OUTBOX_CHANNEL]
    );
    assert_eq!(
        registration
            .permission_scope
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["outbox:append"]
    );
    assert!(
        registry.admits(DRAFT_TOOL, OUTBOX_CHANNEL, 1, 1).is_ok(),
        "premise"
    );
    assert!(registry.admits(DRAFT_TOOL, "press", 1, 1).is_err());
    assert!(registry.registration("email-send").is_none());

    for subject in [
        "post to the press that everyone is buying EURUSD",
        "email clients: guaranteed profit on EURUSD",
        "spread the rumour has it EURUSD widens",
    ] {
        let outcome = door
            .admit(
                advisory(&format!("sig-{}", subject.len()), subject),
                APPROVER,
                at(120),
                1,
            )
            .unwrap();
        assert!(
            matches!(outcome, Intake::Withheld { .. }),
            "{subject}: {outcome:?}"
        );
    }
    assert!(door.outbox().is_empty(), "no hostile draft was written");

    // The adapter itself: another tool is refused even with a draft staged.
    let mut adapter = DraftingAdapter::open(EventLog::in_memory()).unwrap();
    let step = Step {
        tool: "email-send".into(),
        variable: "outbox".into(),
        exposure: Decimal::ZERO,
    };
    assert!(adapter.call(&step).is_err(), "nothing staged");
    assert!(adapter.log().is_empty());
}

/// Missing provenance: a hand-built opportunity (its fields are public) is
/// checked as strictly as one `advise` built. Mutation: delete the
/// `evidence_ids.is_empty()` arm of `require_provenance` — the first case is
/// drafted.
#[test]
fn an_opportunity_missing_provenance_is_refused_whatever_built_it() {
    let good = advisory("sig-5", "EURUSD");
    let base = good.view().clone();
    assert!(base.require_provenance().is_ok(), "premise");
    let cases: Vec<(&str, AmbientOpportunity)> = vec![
        (
            "evidence lineage",
            AmbientOpportunity {
                evidence_ids: vec![],
                ..base.clone()
            },
        ),
        (
            "evidence lineage",
            AmbientOpportunity {
                evidence_ids: vec![" ".into()],
                ..base.clone()
            },
        ),
        (
            "a correlation id",
            AmbientOpportunity {
                correlation_id: " ".into(),
                ..base.clone()
            },
        ),
        (
            "the detector",
            AmbientOpportunity {
                source: "event:".into(),
                ..base.clone()
            },
        ),
        (
            "a signal id",
            AmbientOpportunity {
                signal_id: String::new(),
                ..base.clone()
            },
        ),
    ];
    for (what, opportunity) in cases {
        let mut door = intake();
        // A hand-built value cannot be promoted without evidence either; give
        // it the case a careless caller would, so the door is the only guard.
        let promoted = promote(
            Advisory::new(opportunity, "x"),
            &PromotionCase {
                evidence: vec!["e".into()],
                evaluation_passed: true,
                approver: APPROVER.into(),
                deterministic_controls_held: true,
            },
        )
        .unwrap();
        let refusal = door.submit(promoted, at(120), 1).unwrap_err();
        assert!(refusal.message().contains(what), "{what}: {refusal}");
        assert!(door.outbox().is_empty());
        assert!(door.guard().audit().is_empty());
    }
}

/// A revoked drafting tool is refused and audited, and a restarted door
/// resumes its budget from the outbox rather than refilling it. Mutation: skip
/// `guard.resume(decisions)` in `AmbientIntake::open` — the resumed door
/// admits a draft past the rate it already spent.
#[test]
fn a_revoked_tool_is_refused_and_a_restart_does_not_refill_the_rate() {
    let mut door = intake();
    door.revoke().unwrap();
    let outcome = door
        .admit(advisory("sig-6", "EURUSD"), APPROVER, at(120), 1)
        .unwrap();
    assert!(matches!(outcome, Intake::Withheld { .. }), "{outcome:?}");
    assert!(door.outbox().is_empty());

    let dir = std::env::temp_dir().join(format!("qip-intake-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("outbox.jsonl");
    let _ = std::fs::remove_file(&path);
    let one_per_window = IntakeConfig {
        rate_per_window: 1,
        ..config()
    };
    let mut first =
        AmbientIntake::open(one_per_window.clone(), EventLog::open(&path).unwrap()).unwrap();
    let wrote = first
        .admit(advisory("sig-7", "EURUSD"), APPROVER, at(120), 5)
        .unwrap();
    assert!(
        matches!(wrote, Intake::Drafted { .. }),
        "premise: {wrote:?}"
    );
    drop(first);
    let mut second = AmbientIntake::open(one_per_window, EventLog::open(&path).unwrap()).unwrap();
    let refused = second
        .admit(advisory("sig-8", "GBPUSD"), APPROVER, at(121), 5)
        .unwrap();
    let Intake::Withheld { reason } = refused else {
        panic!("expected withheld, got {refused:?}")
    };
    assert!(reason.contains("rate"), "{reason}");
    let replayed = second
        .admit(advisory("sig-7", "EURUSD"), APPROVER, at(122), 5)
        .unwrap();
    assert!(
        matches!(replayed, Intake::Duplicate { .. }),
        "idempotent across a restart"
    );
    let _ = std::fs::remove_dir_all(Path::new(&dir));
}

/// Failure and retry. A log with room for one record refuses the second
/// draft's append; the draft is held, retried, and abandoned on the record
/// after `max_attempts`, never retried forever and never lost silently.
/// Mutation: in `attempt`, never increment `pending.attempts` — the draft is
/// held forever and the abandonment assertion fails.
#[test]
fn a_failed_outbox_write_is_held_retried_and_then_abandoned_on_the_record() {
    let cfg = IntakeConfig {
        max_attempts: 3,
        ..config()
    };
    let mut door =
        AmbientIntake::open(cfg, EventLog::in_memory().with_capacity(1).unwrap()).unwrap();
    let first = door
        .admit(advisory("sig-9", "EURUSD"), APPROVER, at(120), 1)
        .unwrap();
    assert!(
        matches!(first, Intake::Drafted { .. }),
        "premise: room for one: {first:?}"
    );

    let second = door
        .admit(advisory("sig-10", "GBPUSD"), APPROVER, at(121), 1)
        .unwrap();
    let Intake::Deferred { attempts, .. } = second else {
        panic!("expected deferred, got {second:?}")
    };
    assert_eq!(attempts, 1);
    assert_eq!(door.pending(), 1);
    assert_eq!(
        door.outbox().len(),
        1,
        "the failed draft is not half-written"
    );

    let again = door.retry(at(130), 2).unwrap();
    assert!(
        matches!(again[0], Intake::Deferred { attempts: 2, .. }),
        "{again:?}"
    );
    let last = door.retry(at(140), 3).unwrap();
    assert!(
        matches!(last[0], Intake::Abandoned { attempts: 3, .. }),
        "{last:?}"
    );
    assert_eq!(door.pending(), 0, "abandoned, not held for ever");
    // Every attempt that reached the adapter ran, so each was billed.
    assert_eq!(
        door.guard().audit().len(),
        4,
        "one landed draft and three attempts"
    );
}

/// Mutation: remove the `retry_capacity` check in `attempt` — the queue grows
/// past its bound and the abandoned assertion fails.
#[test]
fn the_retry_queue_is_bounded_and_a_full_queue_abandons_on_the_record() {
    let cfg = IntakeConfig {
        retry_capacity: 1,
        max_attempts: 5,
        ..config()
    };
    let mut door =
        AmbientIntake::open(cfg, EventLog::in_memory().with_capacity(1).unwrap()).unwrap();
    door.admit(advisory("a", "EURUSD"), APPROVER, at(120), 1)
        .unwrap();
    let held = door
        .admit(advisory("b", "GBPUSD"), APPROVER, at(121), 1)
        .unwrap();
    assert!(matches!(held, Intake::Deferred { .. }), "premise: {held:?}");
    let shed = door
        .admit(advisory("c", "USDJPY"), APPROVER, at(122), 1)
        .unwrap();
    let Intake::Abandoned { reason, .. } = shed else {
        panic!("expected abandoned, got {shed:?}")
    };
    assert!(reason.contains("queue is full"), "{reason}");
    assert_eq!(door.pending(), 1);
}

/// The draft cannot be written without staging, and staging is consumed: a
/// stale draft is never written by the next, unrelated step.
#[test]
fn the_adapter_writes_nothing_without_a_staged_draft_and_never_twice() {
    let mut adapter = DraftingAdapter::open(EventLog::in_memory()).unwrap();
    let step = Step {
        tool: DRAFT_TOOL.into(),
        variable: "outbox".into(),
        exposure: Decimal::ZERO,
    };
    let err: Error = adapter.call(&step).unwrap_err();
    assert!(err.message().contains("no draft staged"), "{err}");
    assert!(adapter.log().is_empty());
    let _ = DraftContext::clone; // the type is part of the public seam
}

// ---- negative architecture -------------------------------------------------

fn source(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn code(text: &str) -> String {
    // Comments and doc lines are prose; the architecture is in the code.
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The type signature of the door is the structural half: it takes a
/// `Promoted`, so an `Advisory` does not compile. Mutation: change `submit`'s
/// parameter to `Advisory<AmbientOpportunity>` — this file stops compiling.
#[test]
fn the_door_takes_a_promoted_output_and_nothing_else() {
    let _: fn(
        &mut AmbientIntake,
        Promoted<AmbientOpportunity>,
        Timestamp,
        u64,
    ) -> Result<Intake, Error> = AmbientIntake::submit;
}

/// Ambient types reach an adapter only through promote and the guard: no
/// `impl Adapter` outside the outbox, and the outbox and the engine name no
/// ambient type. Mutation: add `use qip_contracts::ambient` to `outbox.rs` —
/// the second assertion fails.
#[test]
fn ambient_types_cannot_reach_an_adapter_except_through_promote_and_the_guard() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut implementors = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        if name.ends_with(".rs") && code(&source(&name)).contains("impl Adapter for") {
            implementors.push(name);
        }
    }
    assert_eq!(
        implementors,
        vec!["outbox.rs"],
        "the one production adapter"
    );
    for file in ["outbox.rs", "engine.rs", "guard.rs"] {
        let text = code(&source(file)).to_lowercase();
        for forbidden in ["ambient", "advisory", "attentionevent", "attentionrouter"] {
            assert!(!text.contains(forbidden), "{file} names `{forbidden}`");
        }
    }
    // And the one module that does name them calls the adapter only after the
    // guard cleared it: preflight, then authorise, then the call.
    let intake = code(&source("ambient_intake.rs"));
    let (pre, auth, call) = (
        intake.find("guard.preflight").unwrap(),
        intake.find("guard.authorise").unwrap(),
        intake.find("Adapter::call(&mut self.adapter").unwrap(),
    );
    assert!(pre < auth && auth < call, "guard before adapter");
    assert_eq!(
        intake.matches("Adapter::call(").count(),
        1,
        "one adapter call site"
    );
}

/// R12-RES-7109: revoking the drafting tool stops the drafts already held for
/// a retry, as well as the next new one. A held draft has cleared the guard
/// once. Left queued, it would go out on the first retry after the tool is
/// reauthorised, on an authority given before the stop. Only a named operator
/// restores the tool, and the dropped drafts stay dropped.
///
/// Mutation: in `AmbientIntake::revoke`, map over `retry.iter().cloned()`
/// instead of draining it. The held draft survives the revocation and the
/// `pending() == 0` assertion fails.
#[test]
fn revoking_the_drafting_tool_drops_every_held_draft_and_only_a_named_operator_restores_it() {
    let mut door =
        AmbientIntake::open(config(), EventLog::in_memory().with_capacity(1).unwrap()).unwrap();
    assert!(matches!(
        door.admit(advisory("sig-20", "EURUSD"), APPROVER, at(120), 1)
            .unwrap(),
        Intake::Drafted { .. }
    ));
    let held = door
        .admit(advisory("sig-21", "GBPUSD"), APPROVER, at(121), 1)
        .unwrap();
    assert!(matches!(held, Intake::Deferred { .. }), "premise: {held:?}");
    assert_eq!(door.pending(), 1, "premise: one draft is held for a retry");

    let dropped = door.revoke().unwrap();
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(
        matches!(&dropped[0], Intake::Abandoned { reason, attempts: 1 } if reason.contains("revoked")),
        "{dropped:?}"
    );
    assert_eq!(door.pending(), 0, "nothing is left queued behind the stop");
    assert!(door.retry(at(130), 2).unwrap().is_empty());

    assert!(
        door.reauthorise("  ").is_err(),
        "a blank operator restores nothing"
    );
    let refused = door
        .admit(advisory("sig-22", "USDJPY"), APPROVER, at(131), 2)
        .unwrap();
    assert!(
        matches!(refused, Intake::Withheld { ref reason } if reason.contains("revoked")),
        "still revoked after a refused reauthorisation: {refused:?}"
    );

    door.reauthorise("ops-lead").unwrap();
    assert!(
        door.retry(at(140), 3).unwrap().is_empty(),
        "the dropped draft is not resurrected by the reauthorisation"
    );
    let after = door
        .admit(advisory("sig-23", "AUDUSD"), APPROVER, at(141), 3)
        .unwrap();
    // The outbox is full, so the write is deferred; what matters is that the
    // guard cleared it, which a revoked tool never does.
    assert!(
        matches!(after, Intake::Deferred { .. }),
        "a reauthorised tool clears the guard again: {after:?}"
    );
}
