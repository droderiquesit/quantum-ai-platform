//! R12-GOV-6124 and R12-AGENCY-1056..1069: one guard held across passes,
//! each pass beginning and ending with its own declarations, and every pass's
//! approvals and tool decisions on the hash-chained log.
//!
//! Each test names the mutation that should make it fail.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use qip_agency::affordance::{Method, ToolEdgeDraft};
use qip_agency::attribution::{Effect, EffectAttribution, HorizonObservation, Identification};
use qip_agency::autonomy::{ActionPolicy, Authority, WIDEN_AUTHORITY};
use qip_agency::comparison::ComparisonDraft;
use qip_agency::conduct_brain::{CommunicationDraft, ConductPolicy, EvidenceCitation};
use qip_agency::engine::{
    Adapter, EdgeDecl, Governance, LeverEvidence, Outcome, ProposalDraft, Request, VariableDecl,
    run_governed,
};
use qip_agency::goal::{GoalClass, GoalSpecDraft};
use qip_agency::guard::{ActionGuard, ApprovalDecl, GuardPolicy, PassDecl};
use qip_agency::journal::{self, AgencyRecord, PassEntry, Provenance};
use qip_agency::plan::{ActingIdentity, PlanNode, Step};
use qip_agency::tools::ToolRegistration;
use qip_compliance::eligibility::{EligibilityRecord, EligibilityRegister, Subject, SubjectKind};
use qip_contracts::action::Ground;
use qip_core::{Decimal, Error, Timestamp};
use qip_events::EventLog;
use std::collections::{BTreeMap, BTreeSet};

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}

fn request(rate_window: u64) -> Request {
    let s = |x: &str| Some(x.to_string());
    Request {
        goal: GoalSpecDraft {
            class: Some(GoalClass::Liquidity),
            target_state: s("spread under 5 bps"),
            entities_affected: Some(vec!["venue-a".into()]),
            time_horizon_secs: Some(3600),
            success_metric: s("median spread"),
            acceptable_uncertainty: Some(Decimal::from_scaled(2, 1).unwrap()),
            budget: Some(d(100)),
            risk_envelope: Some(d(50)),
            jurisdictions: Some(vec!["US".into()]),
            acting_identity: s("desk-agent"),
            prohibited_side_effects: Some(vec![]),
            prohibited_methods: Some(vec![]),
            stop_conditions: Some(vec!["spread widens".into()]),
        }
        .build()
        .unwrap(),
        targets: vec!["spread".into()],
        variables: vec![
            VariableDecl {
                name: "quote_size".into(),
                observable: true,
                controllable: true,
                owned: false,
            },
            VariableDecl {
                name: "spread".into(),
                observable: true,
                controllable: false,
                owned: false,
            },
        ],
        causes: vec![("quote_size".into(), "spread".into())],
        tool_edges: vec![EdgeDecl {
            variable: "quote_size".into(),
            edge: ToolEdgeDraft {
                tool: s("quoter"),
                method: Some(Method::Market),
                reversible: Some(true),
                latency_ms: Some(10),
                cost: Some(d(30)),
                side_effects: Some(vec![]),
                dependencies: Some(vec![]),
                authority: s("quote"),
            },
        }],
        identity: ActingIdentity {
            name: "desk-agent".into(),
            authorities: BTreeSet::from(["quote".to_string()]),
        },
        evidence: BTreeMap::from([(
            "quote_size".to_string(),
            LeverEvidence {
                identifiability: Decimal::from_scaled(9, 1).unwrap(),
                experimentable: true,
            },
        )]),
        observed_facts: BTreeSet::new(),
        proposals: vec![ProposalDraft {
            root: PlanNode::Step(Step {
                tool: "quoter".into(),
                variable: "quote_size".into(),
                exposure: d(20),
            }),
            comparison: ComparisonDraft {
                expected_causal_effect: Some(d(5)),
                confidence: Some(Decimal::from_scaled(5, 1).unwrap()),
                cost: Some(d(1)),
                capital_usage: Some(d(1)),
                time_to_effect_secs: Some(60),
                reversibility: Some(d(1)),
                legally_eligible: Some(true),
                conduct_risk: Some(d(0)),
                downside: Some(d(2)),
            },
        }],
        guards: None,
        pass: Some(pass(rate_window)),
        structural: None,
        touches: vec![Subject::new(SubjectKind::Venue, "venue-a")],
    }
}

fn pass(rate_window: u64) -> PassDecl {
    PassDecl {
        communications: BTreeMap::from([(
            "quoter".to_string(),
            CommunicationDraft {
                id: "quote-1".into(),
                content: "Quote 20 lots on venue-a at the published spread".into(),
                channel: "paper-venue".into(),
                sender_identity: "desk-agent".into(),
                evidence_citations: vec![EvidenceCitation {
                    source_id: "venue-feed".into(),
                    evidence_id: "spread-series-1".into(),
                    confidence_bps: 9000,
                }],
                disclosures: vec![],
                target_audience: "venue-a participants".into(),
                jurisdiction: "US".into(),
            },
        )]),
        grounds: vec![Ground::Evidence("spread series spread-series-1".into())],
        approvals: ApprovalDecl {
            goal_approver: "pm-a".into(),
            goal_approved_at: Timestamp::from_secs(10),
            plan_approver: "risk-b".into(),
            plan_approved_at: Timestamp::from_secs(20),
            execute_at: Timestamp::from_secs(30),
        },
        rate_window,
    }
}

fn guard() -> ActionGuard {
    ActionGuard::from_policy(GuardPolicy {
        conduct: ConductPolicy {
            authorized_sources: vec!["venue-feed".into()],
            authorized_identities: vec!["desk-agent".into()],
            required_disclosures: vec![],
            channel_policies: BTreeMap::from([(
                "paper-venue".to_string(),
                vec!["venue-a participants".to_string()],
            )]),
            permitted_jurisdictions: vec!["US".into()],
            conduct_rules: qip_agency::conduct_brain::MANDATORY_RULES
                .iter()
                .map(|r| r.to_string())
                .collect(),
        },
        tools: vec![ToolRegistration {
            tool: "quoter".into(),
            acting_identity: "desk-agent".into(),
            permission_scope: BTreeSet::from(["quote".to_string()]),
            disclosure_policy: "none required on a paper venue".into(),
            channels: BTreeSet::from(["paper-venue".to_string()]),
            max_actions_per_window: 1,
            budget_units: 1_000,
            revocation_path: "operator revokes in the policy file".into(),
        }],
    })
    .unwrap()
}

fn register(venue_jurisdiction: &str) -> EligibilityRegister {
    let record = |kind, id: &str, place: &str| EligibilityRecord {
        subject: Subject::new(kind, id),
        jurisdictions: BTreeSet::from([place.to_string()]),
        granted_by: "compliance-officer".into(),
        granted_at: Timestamp::from_secs(0),
        expires_at: Timestamp::from_secs(1_000_000),
        revoked: None,
    };
    EligibilityRegister::from_records(vec![
        record(SubjectKind::Venue, "venue-a", venue_jurisdiction),
        record(SubjectKind::Jurisdiction, "US", "US"),
    ])
    .unwrap()
}

/// A market policy at narrow-reversible authority, widened the only way.
fn narrow() -> ActionPolicy {
    let mut policy = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    policy
        .widen(
            &EffectAttribution {
                goal_id: "g1".into(),
                intervention_id: "i1".into(),
                action_id: "a1".into(),
                action_class: Method::Market,
                identification: Identification::Experiment,
                observations: vec![HorizonObservation {
                    horizon_ms: 60_000,
                    value: 3.0,
                }],
                counterfactual_method: "randomised control".into(),
                effect: Effect {
                    mean: 3.0,
                    std_dev: 0.5,
                },
                causal_chain: vec!["quote_size".into(), "spread".into()],
                confounders: vec!["none considered".into()],
                competing_explanations: vec!["seasonality".into()],
                side_effects: vec!["none observed".into()],
                identifiability: 0.8,
                resulting_updates: vec!["raise quote-size prior".into()],
            },
            &ActingIdentity {
                name: "risk-officer".into(),
                authorities: BTreeSet::from([WIDEN_AUTHORITY.to_string()]),
            },
        )
        .unwrap();
    assert_eq!(policy.authority(), Authority::NarrowReversible);
    policy
}

/// What ADR 0114 leaves every shipped binary: an adapter that refuses.
#[derive(Default)]
struct Refusing {
    attempts: usize,
}

impl Adapter for Refusing {
    fn call(&mut self, step: &Step) -> Result<(), Error> {
        self.attempts += 1;
        Err(Error::denied(format!("`{}` was not sent", step.tool)))
    }
}

fn provenance() -> Provenance {
    Provenance {
        code_version: "test 0".into(),
        policy_version: "sha256:policy".into(),
    }
}

/// GOV-6124's check: one intervention leaves three linked records, each with
/// its own actor and time, on a chain that verifies — and an attempt the
/// adapter refused is still an attempt on the record.
///
/// Mutation: in `journal::record_pass`, drop the `Executed` write (or, in
/// `engine::execute`, return `Err` on the adapter's refusal as it used to,
/// losing the chain) — the three-record assertion fails.
#[test]
fn an_attempted_pass_leaves_three_linked_approval_records_on_the_chain() {
    let policy = narrow();
    let mut guard = guard();
    let registry = register("US");
    let mut adapter = Refusing::default();
    let report = run_governed(
        request(1),
        Governance {
            policy: Some(&policy),
            guard: Some(&mut guard),
            eligibility: Some((&registry, Timestamp::from_secs(100))),
        },
        &mut adapter,
    )
    .unwrap();
    let Outcome::Refused { attempted, .. } = &report.outcome else {
        panic!("premise: the pass is attempted, got {:?}", report.outcome)
    };
    assert_eq!(attempted.len(), 1);
    assert_eq!(adapter.attempts, 1, "premise: the adapter was reached once");
    assert!(!guard.in_pass(), "the pass ended with its approvals");

    let mut log = EventLog::in_memory();
    journal::record_pass(
        &mut log,
        PassEntry {
            pass: 1,
            provenance: &provenance(),
            report: &report,
            widening: None,
            audit_from: 0,
            goal_target: "spread under 5 bps",
            acting_identity: "desk-agent",
            at: Timestamp::from_secs(100),
        },
    )
    .unwrap();
    assert!(log.verify_chain().is_ok());
    let records = journal::records(&log).unwrap();
    let goal = records.iter().find_map(|r| match r {
        AgencyRecord::GoalApproved { approval, .. } => Some(approval.clone()),
        _ => None,
    });
    let plan = records.iter().find_map(|r| match r {
        AgencyRecord::PlanApproved { approval, .. } => Some(approval.clone()),
        _ => None,
    });
    let executed = records.iter().find_map(|r| match r {
        AgencyRecord::Executed { record, .. } => Some(record.clone()),
        _ => None,
    });
    let (goal, plan, executed) = (goal.unwrap(), plan.unwrap(), executed.unwrap());
    assert_eq!(plan.goal, goal.id);
    assert_eq!(executed.plan, plan.id);
    assert_eq!(
        (
            goal.approver.as_str(),
            plan.approver.as_str(),
            executed.executor.as_str()
        ),
        ("pm-a", "risk-b", "desk-agent")
    );
    assert!(goal.at < plan.at && plan.at < executed.at);
    // The admitted, charged decision is on the record with what it cost.
    let decisions: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            AgencyRecord::ToolDecision { decision, .. } => Some(decision.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(decisions.len(), 1);
    assert!(decisions[0].admitted && decisions[0].cost_units == 30 && decisions[0].window == 1);
    // Linked by lineage: every record after the first is caused by the one
    // before it.
    let events: Vec<_> = log.events().collect();
    for pair in events.windows(2) {
        assert_eq!(
            pair[1]
                .lineage
                .causation_id
                .as_ref()
                .map(|c| c.as_str().to_string()),
            Some(pair[0].event_id.as_str().to_string())
        );
    }
    let Some(AgencyRecord::PassConcluded {
        outcome,
        provenance: recorded,
        ..
    }) = records.last()
    else {
        panic!("the pass is concluded last")
    };
    assert_eq!(outcome, "refused");
    assert_eq!(recorded, &provenance());
}

/// A budget is not reset by starting again. Mutation: make
/// `ToolRegistry::resume` push the audit without charging the slot — the
/// resumed guard then admits the second pass.
#[test]
fn a_guard_resumed_from_the_journal_has_already_spent_what_the_journal_records() {
    let policy = narrow();
    let registry = register("US");
    let mut log = EventLog::in_memory();
    let mut first = guard();
    let report = run_governed(
        request(1),
        Governance {
            policy: Some(&policy),
            guard: Some(&mut first),
            eligibility: Some((&registry, Timestamp::from_secs(100))),
        },
        &mut Refusing::default(),
    )
    .unwrap();
    assert!(matches!(report.outcome, Outcome::Refused { .. }), "premise");
    journal::record_pass(
        &mut log,
        PassEntry {
            pass: 1,
            provenance: &provenance(),
            report: &report,
            widening: None,
            audit_from: 0,
            goal_target: "t",
            acting_identity: "desk-agent",
            at: Timestamp::from_secs(100),
        },
    )
    .unwrap();

    // Premise: a fresh guard, not resumed, admits the same pass.
    let mut fresh = guard();
    let again = run_governed(
        request(1),
        Governance {
            policy: Some(&policy),
            guard: Some(&mut fresh),
            eligibility: Some((&registry, Timestamp::from_secs(100))),
        },
        &mut Refusing::default(),
    )
    .unwrap();
    assert!(matches!(again.outcome, Outcome::Refused { .. }), "premise");

    let resumed = journal::resume(&log).unwrap();
    assert_eq!(resumed.next_pass, 2);
    let mut restarted = guard();
    restarted.resume(resumed.decisions).unwrap();
    let mut adapter = Refusing::default();
    let limited = run_governed(
        request(1),
        Governance {
            policy: Some(&policy),
            guard: Some(&mut restarted),
            eligibility: Some((&registry, Timestamp::from_secs(100))),
        },
        &mut adapter,
    )
    .unwrap();
    let Outcome::Stopped(why) = &limited.outcome else {
        panic!("{:?}", limited.outcome)
    };
    assert!(why.contains("rate limit"), "{why}");
    assert_eq!(adapter.attempts, 0);

    // And going back to window 0 to get a fresh allowance is refused.
    let err = run_governed(
        request(0),
        Governance {
            policy: Some(&policy),
            guard: Some(&mut restarted),
            eligibility: Some((&registry, Timestamp::from_secs(100))),
        },
        &mut adapter,
    )
    .unwrap_err();
    assert!(err.to_string().contains("not reopened"), "{err}");
}

/// Between passes the guard holds no approvals. Mutation: remove the
/// `guard.end_pass()` in `run_governed` — the second, pass-less run then
/// reuses the first pass's approvals and is shadowed.
#[test]
fn an_approval_does_not_outlive_its_pass() {
    let mut guard = guard();
    let registry = register("US");
    let governed = |guard: &mut ActionGuard, request: Request| {
        run_governed(
            request,
            Governance {
                policy: None,
                guard: Some(guard),
                eligibility: Some((&registry, Timestamp::from_secs(100))),
            },
            &mut Refusing::default(),
        )
        .unwrap()
    };
    let first = governed(&mut guard, request(1));
    assert!(matches!(first.outcome, Outcome::Shadowed(_)), "premise");
    let mut bare = request(1);
    bare.pass = None;
    let second = governed(&mut guard, bare);
    let Outcome::Stopped(why) = &second.outcome else {
        panic!("{:?}", second.outcome)
    };
    assert!(why.contains("no pass is begun"), "{why}");
}

/// R12-GOV-6116 in the agency path: a venue the register does not permit in
/// the goal's jurisdiction makes every candidate ineligible, by name.
/// Mutation: in `stages`, ignore `legally_eligible` when marking operator
/// proposals — the plan is then shadowed.
#[test]
fn a_goal_touching_a_venue_not_eligible_in_its_jurisdiction_chooses_nothing() {
    let mut guard = guard();
    let run = |registry: &EligibilityRegister, guard: &mut ActionGuard, request: Request| {
        run_governed(
            request,
            Governance {
                policy: None,
                guard: Some(guard),
                eligibility: Some((registry, Timestamp::from_secs(100))),
            },
            &mut Refusing::default(),
        )
        .unwrap()
    };
    let eligible = run(&register("US"), &mut guard, request(1));
    assert!(matches!(eligible.outcome, Outcome::Shadowed(_)), "premise");
    assert!(eligible.eligibility.is_empty());

    let elsewhere = run(&register("GB"), &mut guard, request(2));
    assert_eq!(elsewhere.outcome, Outcome::NoAction);
    assert!(
        elsewhere
            .eligibility
            .iter()
            .any(|r| r.contains("venue `venue-a` is not permitted in `US`")),
        "{:?}",
        elsewhere.eligibility
    );
    assert!(
        elsewhere.refused[0]
            .1
            .to_string()
            .contains("not legally eligible")
    );

    // An affected entity nobody declared the kind of is itself a refusal.
    let mut undeclared = request(3);
    undeclared.touches.clear();
    let silent = run(&register("US"), &mut guard, undeclared);
    assert_eq!(silent.outcome, Outcome::NoAction);
    assert!(
        silent
            .eligibility
            .iter()
            .any(|r| r.contains("not declared in `touches`"))
    );
}
