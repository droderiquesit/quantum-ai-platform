//! The composite `R12-COVERAGE-7186`: a causal agency engine that maps goals
//! to controllable levers, generates action and no-action plans, simulates
//! them against competing world models and executes only through an
//! authorised, typed tool.
//!
//! Each clause is proved over the path `qip agency` runs
//! (`qip_cli::agency::shadow` -> `qip_agency::engine`), by reading the
//! hash-chained journal it leaves rather than the text it prints, because a
//! suite that greps stdout passes when the stage is skipped and a line is
//! printed anyway.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use qip_agency::journal::{self, AgencyRecord};
use qip_core::SystemClock;
use qip_events::EventLog;
use serde_json::{Value, json};

/// The agency's standing policy: one paper-venue tool that may act once per
/// rate window, a venue eligible in the US, and a market action policy.
fn policy_document() -> Value {
    json!({
        "guard": {
            "conduct": {
                "authorized_sources": ["venue-feed"],
                "authorized_identities": ["desk-agent"],
                "required_disclosures": ["paper_venue"],
                "channel_policies": {"paper-venue": ["venue-a participants"]},
                "permitted_jurisdictions": ["US"],
                "conduct_rules": [
                    "no_false_claims", "no_manipulation", "no_fake_consensus",
                    "no_rumour_engineering"
                ]
            },
            "tools": [{
                "tool": "quoter",
                "acting_identity": "desk-agent",
                "permission_scope": ["quote"],
                "disclosure_policy": "none required on a paper venue",
                "channels": ["paper-venue"],
                "max_actions_per_window": 1,
                "budget_units": 1000,
                "revocation_path": "operator removes it from this file",
                "provenance": "in-tree test fixture, no third-party dependency",
                "sandbox_tests": ["quoter-sandbox-smoke"],
                "cost_latency": {"cost_units": 1, "latency_ms": 100},
                "allowed_callers": ["desk-agent"]
            }]
        },
        "evidence": [{
            "id": "spread-series-1", "source_id": "venue-feed",
            "statement": "quoted spreads on venue-a are wider than the desk target"
        }],
        "eligibility": [
            {
                "subject": {"kind": "venue", "id": "venue-a"},
                "jurisdictions": ["US"],
                "granted_by": "compliance-officer",
                "granted_at": "2020-01-01T00:00:00Z",
                "expires_at": "2100-01-01T00:00:00Z"
            },
            {
                "subject": {"kind": "jurisdiction", "id": "US"},
                "jurisdictions": ["US"],
                "granted_by": "compliance-officer",
                "granted_at": "2020-01-01T00:00:00Z",
                "expires_at": "2100-01-01T00:00:00Z"
            }
        ],
        "action_policies": [{
            "class": "market",
            "uplift_low": "1",
            "uplift_high": "5",
            "predicted_side_effects": []
        }]
    })
}

/// A goal with budget 100 and risk envelope 50, tolerating uncertainty 0.2;
/// one lever, `quote_size`, identified at 0.9; one proposal on it; and this
/// pass's drafts, grounds and approvals in rate window `window`.
fn request(window: u64) -> Value {
    json!({
        "goal": {
            "class": "liquidity",
            "target_state": "spread under 5 bps",
            "entities_affected": ["venue-a"],
            "time_horizon_secs": 3600,
            "success_metric": "median spread",
            "acceptable_uncertainty": "0.2",
            "budget": "100",
            "risk_envelope": "50",
            "jurisdictions": ["US"],
            "acting_identity": "desk-agent",
            "prohibited_side_effects": [],
            "prohibited_methods": [],
            "stop_conditions": ["spread widens"],
            "rollback_conditions": ["restore the prior quote width"]
        },
        "targets": ["spread"],
        "variables": [
            {"name": "quote_size", "observable": true, "controllable": true},
            {"name": "spread", "observable": true, "controllable": false}
        ],
        "causes": [["quote_size", "spread"]],
        "tool_edges": [{
            "variable": "quote_size",
            "edge": {
                "tool": "quoter", "method": "market", "reversible": true,
                "latency_ms": 10, "cost": "30", "side_effects": [],
                "dependencies": [], "authority": "quote"
            }
        }],
        "identity": {"name": "desk-agent", "authorities": ["quote"]},
        "evidence": {"quote_size": {"identifiability": "0.9", "experimentable": true}},
        "observed_facts": [],
        "proposals": [{
            "root": {"step": {"tool": "quoter", "variable": "quote_size", "exposure": "20"}},
            "comparison": {
                "expected_causal_effect": "5", "confidence": "0.5", "cost": "1",
                "capital_usage": "1", "time_to_effect_secs": 60, "reversibility": "1",
                "legally_eligible": true, "conduct_risk": "0", "downside": "2"
            }
        }],
        "touches": [{"kind": "venue", "id": "venue-a"}],
        "pass": {
            "communications": {"quoter": {
                "id": "quote-1",
                "content": "Quote 20 lots on venue-a at the published spread",
                "channel": "paper-venue",
                "sender_identity": "desk-agent",
                "evidence_citations": [{
                    "source_id": "venue-feed", "evidence_id": "spread-series-1",
                    "claim": "quoted spreads on venue-a are wider than the desk target",
                    "confidence_bps": 9000
                }],
                "disclosures": [{"kind": "paper_venue", "text": "Paper venue: no live order is placed"}],
                "target_audience": "venue-a participants",
                "jurisdiction": "US"
            }},
            "grounds": [{"evidence": "spread series spread-series-1 shows quotes under-sized"}],
            "approvals": {
                "goal_approver": "pm-a", "goal_approved_at": "2026-01-01T00:00:10Z",
                "plan_approver": "risk-b", "plan_approved_at": "2026-01-01T00:00:20Z",
                "execute_at": "2026-01-01T00:00:30Z"
            },
            "rate_window": window
        }
    })
}

/// The request, asking to widen the market policy on a hold-out
/// experiment whose data reproduces the attribution, and to act under it.
fn acting(window: u64) -> Value {
    let mut document = request(window);
    document["act_as"] = json!("market");
    // A pass acts only over a plan simulated against counterfactual
    // branches (R12-MODEL-4014): the desk's structural model, two scenarios.
    document["structural"] = json!({
        "models": [{
            "name": "desk",
            "links": [{"cause": "quote_size", "effect": "spread", "coefficient": -0.6}],
            "intercepts": {"spread": 6.0}
        }],
        "scenarios": [
            {"name": "calm", "exogenous": {}},
            {"name": "stressed", "exogenous": {"spread": 3.0}}
        ],
        "objective": {"variable": "spread", "direction": "decrease"},
        "actuation": {"quoter": 10.0}
    });
    document["widening"] = json!({
        "class": "market",
        "approver": {"name": "risk-officer", "authorities": ["agency:widen-autonomy"]},
        "attribution": {
            "goal_id": "g1", "intervention_id": "i1", "action_id": "a1",
            "action_class": "market",
            "identification": "experiment",
            "observations": [{"horizon_ms": 60000, "value": 3.0}],
            "counterfactual_method": "randomised control",
            "effect": {"mean": 3.0, "std_dev": 0.5},
            "causal_chain": ["quote_size", "spread"],
            "confounders": ["none considered"],
            "competing_explanations": ["seasonality"],
            "side_effects": ["none observed"],
            "identifiability": 0.8,
            "resulting_updates": ["raise quote-size prior"]
        },
        "evidence": hold_out_evidence()
    });
    document
}

/// Evidence for a treated-minus-held-out effect of 3.0 from a hold-out the
/// binary draws itself (twelve cells, a quarter held out).
fn hold_out_evidence() -> Value {
    let units: Vec<String> = (0..12).map(|i| format!("cell-{i:02}")).collect();
    let design = qip_agency::estimate::Experiment::design("seed-2026-10-18", &units, 2500).unwrap();
    let outcomes: serde_json::Map<String, Value> = units
        .iter()
        .map(|u| {
            (
                u.clone(),
                json!(if design.treated().contains(u) {
                    4.0
                } else {
                    1.0
                }),
            )
        })
        .collect();
    json!({"hold_out_experiment": {
        "seed": "seed-2026-10-18", "units": units, "hold_out_bps": 2500, "outcomes": outcomes
    }})
}

fn run(policy: &Value, documents: &[Value], name: &str) -> (Vec<String>, u8, Vec<AgencyRecord>) {
    let path = std::env::temp_dir().join(format!(
        "qip-agency-composite-{}-{name}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut log = EventLog::open(path.to_str().unwrap()).unwrap();
    let requests: Vec<String> = documents.iter().map(Value::to_string).collect();
    let (lines, code) =
        qip_cli::agency::shadow(&policy.to_string(), &requests, &mut log, &SystemClock).unwrap();
    drop(log);
    let inspected = EventLog::inspect(path.to_str().unwrap()).unwrap();
    assert!(
        inspected.verify_chain().is_ok(),
        "the journal's chain verifies"
    );
    let records = journal::records(&inspected).unwrap();
    let _ = std::fs::remove_file(&path);
    (lines, code, records)
}

fn concluded(records: &[AgencyRecord]) -> &AgencyRecord {
    records
        .iter()
        .find(|r| matches!(r, AgencyRecord::PassConcluded { .. }))
        .expect("every pass is concluded on the journal")
}

fn second_model() -> Value {
    json!({"name": "rival",
           "links": [{"cause": "quote_size", "effect": "spread", "coefficient": -0.2}],
           "intercepts": {"spread": 6.0}})
}

/// Clause 1: a goal is mapped to the controllable levers that can move it,
/// and a goal with no controllable lever has none to plan with.
#[test]
fn a_goal_is_mapped_to_the_controllable_levers_that_reach_its_target() {
    let (lines, code, records) = run(&policy_document(), &[request(1)], "levers");
    assert_eq!(code, 0, "premise: the bounded request clears: {lines:?}");
    assert!(lines.iter().any(|l| l == "levers: quote_size"), "{lines:?}");
    assert!(matches!(
        &records[0],
        AgencyRecord::PassOpened { goal_target, .. } if goal_target == "spread under 5 bps"
    ));

    let mut stuck = request(1);
    stuck["variables"][0]["controllable"] = json!(false);
    // A lever that cannot be controlled cannot hold a tool edge: the engine
    // refuses the request rather than plan on a lever nobody can move.
    let path = std::env::temp_dir().join(format!(
        "qip-agency-composite-{}-nl.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut log = EventLog::open(path.to_str().unwrap()).unwrap();
    let refusal = qip_cli::agency::shadow(
        &policy_document().to_string(),
        &[stuck.to_string()],
        &mut log,
        &SystemClock,
    )
    .expect_err("an uncontrollable lever is refused");
    let _ = std::fs::remove_file(&path);
    assert!(refusal.to_string().contains("observable-only"), "{refusal}");
}

/// Clause 2: both an action plan and the no-action baseline are candidates,
/// and each can be the one chosen.
#[test]
fn both_an_action_plan_and_the_no_action_baseline_can_be_chosen() {
    let (lines, code, _) = run(&policy_document(), &[request(1)], "action");
    assert_eq!(code, 0, "{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("outcome: shadowed")),
        "{lines:?}"
    );

    let mut harmful = request(1);
    harmful["proposals"][0]["comparison"]["expected_causal_effect"] = json!("-5");
    let (lines, code, records) = run(&policy_document(), &[harmful], "no-action");
    assert_eq!(code, 3, "{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("outcome: no action")),
        "{lines:?}"
    );
    match concluded(&records) {
        AgencyRecord::PassConcluded { steps, .. } => assert!(steps.is_empty(), "{steps:?}"),
        other => panic!("{other:?}"),
    }
}

/// Clause 3: a plan is simulated against every declared model in every
/// declared scenario.
#[test]
fn a_plan_is_simulated_against_every_declared_model_and_scenario() {
    let mut document = acting(1);
    document["structural"]["models"]
        .as_array_mut()
        .unwrap()
        .push(second_model());
    let (_, _, records) = run(&policy_document(), &[document], "models");
    let AgencyRecord::PassConcluded { simulations, .. } = concluded(&records) else {
        panic!("concluded record expected")
    };
    let mut seen: Vec<(String, String)> = simulations
        .iter()
        .map(|b| (b.model.clone(), b.scenario.clone()))
        .collect();
    seen.sort();
    let expected: Vec<(String, String)> = ["desk", "rival"]
        .iter()
        .flat_map(|m| {
            ["calm", "stressed"]
                .iter()
                .map(move |s| (m.to_string(), s.to_string()))
        })
        .collect();
    assert_eq!(seen, expected);
}

/// Clause 3, consequence: authority is used only when every competing model
/// agrees the plan helps.
#[test]
fn authority_is_withheld_when_a_competing_model_says_the_plan_does_not_help() {
    let mut document = acting(1);
    document["structural"]["models"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "contrarian",
            "links": [{"cause": "quote_size", "effect": "spread", "coefficient": 0.6}],
            "intercepts": {"spread": 6.0}}));
    let (lines, _, records) = run(&policy_document(), &[document], "dissent");
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, AgencyRecord::Executed { .. })),
        "{lines:?}"
    );
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, AgencyRecord::IntentRecorded { .. })),
        "{lines:?}"
    );
}

/// Clause 4: execution happens only through a registered, authorised tool;
/// the same pass with the tool revoked executes nothing, and the one
/// attempted step in the unrevoked pass is recorded against the tool.
#[test]
fn execution_goes_only_through_a_registered_and_authorised_tool() {
    let (lines, _, records) = run(&policy_document(), &[acting(1)], "tool");
    let executed = records
        .iter()
        .filter(|r| matches!(r, AgencyRecord::Executed { .. }))
        .count();
    assert_eq!(
        executed, 1,
        "premise: the authorised pass executes: {lines:?}"
    );
    assert!(records.iter().any(|r| matches!(
        r,
        AgencyRecord::ToolDecision { decision, .. } if decision.tool == "quoter" && decision.admitted
    )));
    // No adapter is wired (ADR 0114): nothing is sent, and the record says so.
    assert!(records.iter().any(|r| matches!(
        r,
        AgencyRecord::PassConcluded { outcome, .. } if outcome == "refused"
    )));

    let mut revoked = policy_document();
    revoked["revoked_tools"] = json!(["quoter"]);
    let (lines, _, records) = run(&revoked, &[acting(1)], "revoked");
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, AgencyRecord::Executed { .. })),
        "{lines:?}"
    );

    let mut unknown = acting(1);
    unknown["tool_edges"][0]["edge"]["tool"] = json!("freelancer");
    unknown["proposals"][0]["root"]["step"]["tool"] = json!("freelancer");
    unknown["pass"]["communications"] = json!({});
    unknown["structural"]["actuation"] = json!({"freelancer": 10.0});
    let (lines, _, records) = run(&policy_document(), &[unknown], "unregistered");
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, AgencyRecord::Executed { .. })),
        "{lines:?}"
    );
}
