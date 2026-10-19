//! `qip agency shadow`, run as the shipped binary.
//!
//! The agency's conduct gate, tool registry, approval chain and eligibility
//! register were a library this binary could call and usually did not: it
//! ran with no policy, built a guard only if the request carried one, and
//! printed the approvals it was handed and dropped them. These tests drive
//! the process — a library call would pass with the `agency` arm missing
//! from `main`, or with `main` not reading the policy variable — and read the
//! journal it leaves.
//!
//! The premise comes first. "Exits three" proves nothing unless the same
//! document, inside its bounds, exits zero.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use qip_agency::estimate::Experiment;
use qip_agency::journal::{self, AgencyRecord};
use qip_core::{Duration, Timestamp};
use qip_events::EventLog;
use qip_twin::futures::{FutureTree, TreeConfig, TwinState, WorldDriver};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// R12-COVERAGE-7188: evidence for a treated-minus-held-out effect of 3.0
/// from a hold-out the binary draws itself (twelve cells, a quarter held
/// out). Replaces the `randomised_experiment` fixture, whose groups the
/// operator formed.
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

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("qip-agency-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }
    fn write(&self, name: &str, document: &Value) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, document.to_string()).unwrap();
        path
    }
    fn journal(&self) -> PathBuf {
        self.dir.join("journal.jsonl")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What the binary printed, what it complained of, and what it exited with.
fn qip(args: &[&PathBuf], flags: &[&str], env: &[(&str, &PathBuf)]) -> (String, String, i32) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_qip"));
    command
        .args(["agency", "shadow"])
        .env_remove("QIP_AGENCY_POLICY_FILE")
        .env_remove("QIP_AGENCY_JOURNAL");
    for (flag, path) in flags.iter().zip(args) {
        command.arg(flag).arg(path);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("the qip binary runs");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn records(journal: &Path) -> Vec<AgencyRecord> {
    let log = EventLog::inspect(journal).unwrap();
    assert!(log.verify_chain().is_ok(), "the journal's chain verifies");
    journal::records(&log).unwrap()
}

/// R12-AGENCY-1056..1069: no conduct policy, no pass; no journal, no pass;
/// and the policy may come from the variable `main` reads.
///
/// Mutations, each verified: in `agency::paths`, drop the journal
/// requirement — the no-journal case runs the pass and exits zero; default a
/// missing policy to an empty path — the process still exits 1 (the path
/// does not read), so it is the assertion on the refusal's own wording that
/// fires, not the exit code; ignore `QIP_AGENCY_POLICY_FILE` — the
/// variable-only case exits 1.
#[test]
fn the_binary_refuses_to_run_a_pass_without_a_conduct_policy_or_a_journal() {
    let scratch = Scratch::new("refuses");
    let policy = scratch.write("policy.json", &policy_document());
    let pass = scratch.write("pass.json", &request(1));
    let journal = scratch.journal();

    // Premise: with both, the pass runs and is shadowed.
    let (out, err, code) = qip(
        &[&policy, &journal, &pass],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 0, "premise: stdout {out} stderr {err}");
    assert!(out.contains("outcome: shadowed"), "{out}");
    // R12-OBS-7131: the pass is read back from the journal as one chain.
    assert!(
        out.contains(
            "lineage: pass 1 reads back as one connected chain (goal approval present, plan \
             approval present, execution none, 0 tool decision(s), 0 voice(s) released)"
        ),
        "{out}"
    );

    let (out, err, code) = qip(&[&journal, &pass], &["--journal", "--request"], &[]);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("no conduct policy is configured"), "{err}");
    assert!(!out.contains("outcome"), "{out}");

    let (out, err, code) = qip(&[&policy, &pass], &["--policy", "--request"], &[]);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("no journal is configured"), "{err}");

    // The variable `main` reads stands in for the flag.
    let (out, err, code) = qip(
        &[&journal, &pass],
        &["--journal", "--request"],
        &[("QIP_AGENCY_POLICY_FILE", &policy)],
    );
    assert_eq!(code, 0, "stdout {out} stderr {err}");

    // A policy registering no tool is refused at load.
    let mut toolless = policy_document();
    toolless["guard"]["tools"] = json!([]);
    let toolless = scratch.write("toolless.json", &toolless);
    let (_, err, code) = qip(
        &[&toolless, &journal, &pass],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 1);
    assert!(err.contains("registers no tool"), "{err}");

    // A request that brings its own guard is not run.
    let mut smuggled = request(1);
    smuggled["guards"] = policy_document()["guard"].clone();
    let smuggled = scratch.write("smuggled.json", &smuggled);
    let (_, err, code) = qip(
        &[&policy, &journal, &smuggled],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 1);
    assert!(err.contains("carries its own `guards`"), "{err}");
}

/// GOV-6124 from a shipped binary: an acting pass, refused at the adapter
/// (ADR 0114), leaves its goal approval, plan approval, execution record and
/// tool decision on the chain — and the next invocation, resuming from that
/// journal, has already spent the window. Two passes in one invocation share
/// one guard the same way.
///
/// Mutation: in `Session::open`, skip `guard.resume(...)` — the second
/// invocation then attempts again instead of being withheld on the rate
/// limit. Or in `agency::Session::pass`, skip `journal::record_pass` — the
/// journal is empty.
#[test]
fn every_pass_is_journaled_and_one_guard_holds_the_rate_window_across_passes_and_invocations() {
    let scratch = Scratch::new("journaled");
    let policy = scratch.write("policy.json", &policy_document());
    let act = scratch.write("act.json", &acting(1));
    let journal = scratch.journal();

    let (out, err, code) = qip(
        &[&policy, &journal, &act],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 3, "stdout {out} stderr {err}");
    assert!(
        out.contains("widened Market on a HoldOutExperiment estimate"),
        "{out}"
    );
    assert!(out.contains("outcome: attempted and refused"), "{out}");
    assert!(out.contains("execution by desk-agent"), "{out}");

    let first = records(&journal);
    let kinds: Vec<&str> = first
        .iter()
        .map(|r| match r {
            AgencyRecord::PassOpened { .. } => "opened",
            AgencyRecord::Widened { .. } => "widened",
            AgencyRecord::WideningRefused { .. } => "widening refused",
            AgencyRecord::GoalApproved { .. } => "goal",
            AgencyRecord::PlanApproved { .. } => "plan",
            AgencyRecord::Executed { .. } => "executed",
            AgencyRecord::ToolDecision { .. } => "tool",
            AgencyRecord::VoiceReleased { .. } => "voice",
            AgencyRecord::IntentRecorded { .. } => "intent",
            AgencyRecord::PlanBlocked { .. } => "blocked",
            AgencyRecord::Learned { .. } => "learned",
            AgencyRecord::LearningWithheld { .. } => "learning withheld",
            AgencyRecord::PassConcluded { .. } => "concluded",
        })
        .collect();
    // The attempt was refused at the adapter, so there is no outcome to learn
    // from, and the record says so rather than staying silent.
    assert_eq!(
        kinds,
        [
            "opened",
            "widened",
            "goal",
            "plan",
            "executed",
            // R12-CONTRACT-7082: the §27 intent of the cleared step.
            "intent",
            "tool",
            // R12-AGENCY-1064: what the admitted step said, as whom, so a
            // restart still knows who said it.
            "voice",
            "learning withheld",
            "concluded"
        ]
    );
    let Some(AgencyRecord::PassConcluded { provenance, .. }) = first.last() else {
        panic!("concluded last")
    };
    assert!(
        provenance.code_version.starts_with("qip-cli "),
        "{provenance:?}"
    );
    assert!(
        provenance.policy_version.starts_with("sha256:"),
        "{provenance:?}"
    );

    // A second invocation on the same journal, same window: the budget the
    // first spent is spent.
    let (out, _, code) = qip(
        &[&policy, &journal, &act],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("agency pass 2"), "{out}");
    assert!(out.contains("rate limit"), "{out}");
    assert!(!out.contains("attempted and refused"), "{out}");
    let second = records(&journal);
    assert!(second.len() > first.len());
    assert!(
        !second[first.len()..]
            .iter()
            .any(|r| matches!(r, AgencyRecord::Executed { .. }))
    );

    // Going back to window 0 for a fresh allowance is refused outright.
    let rewound = scratch.write("rewound.json", &acting(0));
    let (_, err, code) = qip(
        &[&policy, &journal, &rewound],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 1);
    assert!(err.contains("not reopened"), "{err}");

    // Within one invocation, on a fresh journal: the second pass in window 2
    // is attempted, the third in window 2 is rate-limited.
    let scratch = Scratch::new("one-invocation");
    let policy = scratch.write("policy.json", &policy_document());
    let a = scratch.write("a.json", &acting(2));
    let b = scratch.write("b.json", &acting(2));
    let journal = scratch.journal();
    let (out, err, code) = qip(
        &[&policy, &journal, &a, &b],
        &["--policy", "--journal", "--request", "--request"],
        &[],
    );
    assert_eq!(code, 3, "stdout {out} stderr {err}");
    assert_eq!(out.matches("attempted and refused").count(), 1, "{out}");
    assert!(out.contains("rate limit"), "{out}");
}

/// A widening the estimator does not reproduce is refused and journaled,
/// and the pass stays shadow. Mutation: in `Session::widen`, skip
/// `estimate.supports` — the relabelled correlation widens.
#[test]
fn a_correlation_labelled_as_an_experiment_widens_nothing_and_says_so_on_the_record() {
    let scratch = Scratch::new("correlation");
    let policy = scratch.write("policy.json", &policy_document());
    let mut document = acting(1);
    document["widening"]["evidence"] =
        json!({"correlation": {"x": [0.0, 1.0, 2.0], "y": [0.0, 3.0, 6.0]}});
    let pass = scratch.write("pass.json", &document);
    let journal = scratch.journal();
    let (out, err, code) = qip(
        &[&policy, &journal, &pass],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 0, "stdout {out} stderr {err}");
    assert!(out.contains("widening Market refused"), "{out}");
    assert!(out.contains("outcome: shadowed"), "{out}");
    let records = records(&journal);
    assert!(records.iter().any(|r| matches!(
        r,
        AgencyRecord::WideningRefused { reason, .. } if reason.contains("earned by the method")
    )));
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, AgencyRecord::Executed { .. }))
    );
}

/// R12-COVERAGE-7188: the shadow binary widens on a hold-out it drew itself,
/// and refuses the same evidence once an operator drops a held-out cell.
/// Mutation: in `Experiment::analyse`, delete the missing-outcome check — the
/// second pass then widens on a hold-out missing its inconvenient cell.
#[test]
fn the_binary_widens_on_a_hold_out_it_drew_and_refuses_one_with_a_cell_dropped() {
    let scratch = Scratch::new("holdout");
    let policy = scratch.write("policy.json", &policy_document());
    let units: Vec<String> = (0..12).map(|i| format!("cell-{i:02}")).collect();
    let design = Experiment::design("seed-2026-10-18", &units, 2500).unwrap();
    let outcomes: serde_json::Map<String, Value> = units
        .iter()
        .map(|u| {
            let value = if design.treated().contains(u) {
                4.0
            } else {
                1.0
            };
            (u.clone(), json!(value))
        })
        .collect();
    let widening = |outcomes: &serde_json::Map<String, Value>| {
        let mut document = acting(1);
        document["widening"]["evidence"] = json!({"hold_out_experiment": {
            "seed": "seed-2026-10-18", "units": units, "hold_out_bps": 2500,
            "outcomes": outcomes
        }});
        document
    };

    let whole = scratch.write("whole.json", &widening(&outcomes));
    let (out, err, code) = qip(
        &[&policy, &scratch.journal(), &whole],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 3, "stdout {out} stderr {err}");
    assert!(
        out.contains("widened Market on a HoldOutExperiment estimate"),
        "{out}"
    );

    let mut dropped = outcomes.clone();
    let cell = design.control().iter().next().unwrap().clone();
    dropped.remove(&cell);
    let partial = scratch.write("partial.json", &widening(&dropped));
    let fresh = scratch.dir.join("fresh-holdout.jsonl");
    let (out, err, code) = qip(
        &[&policy, &fresh, &partial],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 0, "stdout {out} stderr {err}");
    assert!(out.contains("widening Market refused"), "{out}");
    assert!(
        out.contains("a unit is not dropped after the fact"),
        "{out}"
    );
    assert!(out.contains("outcome: shadowed"), "{out}");
}

/// R12-COVERAGE-7188: an operator whose hold-out the binary refused (a cell
/// dropped) cannot resubmit the same outcomes as two groups of their own
/// forming and widen anyway. The groups are the very ones the design drew
/// and the outcomes the very ones supplied, so the only thing that changed
/// is who claims to have formed the groups. Claiming `experiment` is refused
/// because the method does not earn it; claiming what the method does earn
/// (`not_identified`) is refused because nothing unidentified widens.
///
/// Mutation: in `estimate::operator_formed_comparison` restore
/// `Identification::Experiment` — the first resubmission then widens and the
/// binary exits 3 with "widened Market on a RandomisedExperiment estimate".
#[test]
fn the_binary_refuses_to_widen_on_operator_chosen_groups_resubmitted_as_the_same_data() {
    let scratch = Scratch::new("operator-groups");
    let policy = scratch.write("policy.json", &policy_document());
    let units: Vec<String> = (0..12).map(|i| format!("cell-{i:02}")).collect();
    let design = Experiment::design("seed-2026-10-18", &units, 2500).unwrap();
    let outcome = |u: &String| {
        if design.treated().contains(u) {
            4.0
        } else {
            1.0
        }
    };
    let group = |cells: &std::collections::BTreeSet<String>| -> Vec<f64> {
        cells.iter().map(outcome).collect()
    };
    let resubmitted = json!({"randomised_experiment": {
        "treated": group(design.treated()), "control": group(design.control())
    }});

    // Premise: the unmodified hold-out widens, so a refusal below is the
    // resubmission's and not a fixture fault.
    let whole = scratch.write("whole.json", &acting(1));
    let (out, err, code) = qip(
        &[&policy, &scratch.journal(), &whole],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 3, "premise: stdout {out} stderr {err}");
    assert!(
        out.contains("widened Market on a HoldOutExperiment"),
        "{out}"
    );

    for (name, label, identifiability, reason) in [
        (
            "claims-experiment",
            json!("experiment"),
            0.8,
            "earned by the method",
        ),
        (
            "claims-nothing",
            json!("not_identified"),
            0.0,
            "not identified",
        ),
    ] {
        let mut document = acting(1);
        document["widening"]["evidence"] = resubmitted.clone();
        document["widening"]["attribution"]["identification"] = label;
        document["widening"]["attribution"]["identifiability"] = json!(identifiability);
        let path = scratch.write(&format!("{name}.json"), &document);
        let fresh = scratch.dir.join(format!("{name}.jsonl"));
        let (out, err, code) = qip(
            &[&policy, &fresh, &path],
            &["--policy", "--journal", "--request"],
            &[],
        );
        assert_eq!(code, 0, "{name}: stdout {out} stderr {err}");
        assert!(out.contains("widening Market refused"), "{name}: {out}");
        assert!(!out.contains("widened Market"), "{name}: {out}");
        assert!(out.contains("outcome: shadowed"), "{name}: {out}");
        let journaled = records(&fresh);
        assert!(
            journaled.iter().any(|r| matches!(
                r,
                AgencyRecord::WideningRefused { reason: why, .. } if why.contains(reason)
            )),
            "{name}: expected a refusal containing `{reason}`: {out}"
        );
        assert!(
            !journaled.iter().any(|r| matches!(
                r,
                AgencyRecord::Widened { .. } | AgencyRecord::Executed { .. }
            )),
            "{name}: nothing widened or executed"
        );
    }
}

/// R12-AGENCY-6088: a pass is simulated against the futures of a market twin
/// the document describes, beside its declared scenarios, under every
/// structural model. Mutation: in `Session::pass`, pass `None` instead of the
/// twin to `run_governed_with` — the journal holds only the declared two.
#[test]
fn the_binary_simulates_a_pass_against_the_futures_of_the_twin_the_document_describes() {
    let scratch = Scratch::new("twin");
    let policy = scratch.write("policy.json", &policy_document());
    let series = BTreeMap::from([(
        "EURUSD".to_string(),
        (1..=12)
            .map(|i| {
                (
                    Timestamp::from_secs(i),
                    1.10 + [0.0, 0.0005, -0.0007, 0.0003][(i % 4) as usize],
                )
            })
            .collect::<Vec<_>>(),
    )]);
    let config = TreeConfig {
        horizon: Duration::from_secs(60),
        shock_sigmas: vec![1.0, 2.0],
        propagation: vec![0.5, 1.0],
        prune_below: 0.01,
        split_beyond_sigmas: 4.0,
        max_branches: 64,
    };
    let drivers = vec![WorldDriver {
        id: "evt-rates".into(),
        sensitivities: BTreeMap::from([("EURUSD".to_string(), 1.0)]),
        evidence: vec!["doc-1".into()],
    }];
    let tree = FutureTree::grow(
        &TwinState::fit(&series, Timestamp::from_secs(12)).unwrap(),
        &drivers,
        config.clone(),
    )
    .unwrap();
    let live = tree.live().count();
    assert!(live >= 2, "premise: the twin grew {live} live futures");

    let mut shadow = acting(1);
    shadow.as_object_mut().unwrap().remove("widening");
    shadow.as_object_mut().unwrap().remove("act_as");
    let scenarios = |document: &Value, name: &str| -> Vec<String> {
        let path = scratch.write(&format!("{name}.json"), document);
        let journal = scratch.dir.join(format!("{name}.jsonl"));
        let (out, err, code) = qip(
            &[&policy, &journal, &path],
            &["--policy", "--journal", "--request"],
            &[],
        );
        assert_eq!(code, 0, "{name}: stdout {out} stderr {err}");
        records(&journal)
            .into_iter()
            .find_map(|r| match r {
                AgencyRecord::PassConcluded { simulations, .. } => {
                    Some(simulations.into_iter().map(|b| b.scenario).collect())
                }
                _ => None,
            })
            .expect("the pass is concluded on the journal")
    };
    let declared = scenarios(&shadow, "declared");
    assert_eq!(
        declared,
        ["calm", "stressed"],
        "premise: no twin, two scenarios"
    );

    let mut with_twin = shadow.clone();
    with_twin["twin"] = json!({
        "as_of": Timestamp::from_secs(12),
        "series": series,
        "drivers": drivers,
        "config": config,
        "bind": {"spread": "EURUSD"}
    });
    let simulated = scenarios(&with_twin, "twin");
    let from_twin = simulated.iter().filter(|s| s.starts_with("twin:")).count();
    assert_eq!(from_twin, live, "one result per live future: {simulated:?}");
    assert_eq!(simulated.len(), 2 + live, "the declared two beside them");

    // A binding to an instrument the twin does not hold is refused, not left
    // at the declared value.
    let mut unbound = with_twin.clone();
    unbound["twin"]["bind"] = json!({"spread": "GBPUSD"});
    let path = scratch.write("unbound.json", &unbound);
    let (out, err, code) = qip(
        &[&policy, &scratch.dir.join("unbound.jsonl"), &path],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 1, "stdout {out} stderr {err}");
    assert!(err.contains("GBPUSD"), "{err}");
}

/// R12-AGENCY-1056 / 1057: the binary resolves every citation against the
/// evidence the operator's policy holds. A requester who cites a record the
/// operator never listed, claims what the record does not say, or declares an
/// authorized source for evidence recorded under another is withheld, and a
/// policy that lists no evidence withholds every pass.
/// Mutation: in `Session::open`, build the guard with no `with_evidence` —
/// every case below executes as before.
#[test]
fn the_binary_resolves_every_citation_against_the_operators_evidence_and_a_policy_with_none_withholds_every_pass()
 {
    let scratch = Scratch::new("citations");
    let run = |name: &str, policy: &Value, pass: &Value| -> (String, i32) {
        let policy = scratch.write(&format!("{name}-policy.json"), policy);
        let pass = scratch.write(&format!("{name}-pass.json"), pass);
        let journal = scratch.dir.join(format!("{name}.jsonl"));
        let (out, err, code) = qip(
            &[&policy, &journal, &pass],
            &["--policy", "--journal", "--request"],
            &[],
        );
        (format!("{out}{err}"), code)
    };
    let (out, code) = run("honest", &policy_document(), &request(1));
    assert_eq!(code, 0, "premise: a resolving citation is shadowed: {out}");

    let mut no_evidence = policy_document();
    no_evidence.as_object_mut().unwrap().remove("evidence");
    let (out, code) = run("none", &no_evidence, &request(1));
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("resolves to no evidence"), "{out}");

    let mut unlisted = request(1);
    unlisted["pass"]["communications"]["quoter"]["evidence_citations"][0]["evidence_id"] =
        json!("spread-series-9");
    let (out, code) = run("unlisted", &policy_document(), &unlisted);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("resolves to no evidence"), "{out}");

    let mut misrecorded = policy_document();
    misrecorded["evidence"][0]["source_id"] = json!("blog");
    let (out, code) = run("forged", &misrecorded, &request(1));
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("the record names 'blog'"), "{out}");

    let mut other_claim = request(1);
    other_claim["pass"]["communications"]["quoter"]["evidence_citations"][0]["claim"] =
        json!("quoted spreads on venue-a are tight");
    let (out, code) = run("claim", &policy_document(), &other_claim);
    assert_eq!(code, 3, "{out}");
    assert!(
        out.contains("is not the statement the record holds"),
        "{out}"
    );
}

/// R12-AGENCY-1066: the binary withholds a draft that names an instrument
/// the operator's policy says the platform holds unless it carries a
/// `position` disclosure. Mutation: in `Session::open`, build the guard with
/// no `with_holdings` — the undisclosed promotion is shadowed.
#[test]
fn the_binary_withholds_a_draft_promoting_a_held_instrument_without_a_position_disclosure() {
    let scratch = Scratch::new("holdings");
    let mut policy = policy_document();
    policy["holdings"] = json!(["venue-a bonds"]);
    let policy = scratch.write("policy.json", &policy);
    let run = |name: &str, pass: &Value| -> (String, i32) {
        let pass = scratch.write(&format!("{name}.json"), pass);
        let journal = scratch.dir.join(format!("{name}.jsonl"));
        let (out, err, code) = qip(
            &[&policy, &journal, &pass],
            &["--policy", "--journal", "--request"],
            &[],
        );
        (format!("{out}{err}"), code)
    };
    let mut promotional = request(1);
    promotional["pass"]["communications"]["quoter"]["content"] =
        json!("Quote 20 lots: venue-a bonds are the better risk on this tape");
    let (out, code) = run("undisclosed", &promotional);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("undisclosed promotion"), "{out}");

    promotional["pass"]["communications"]["quoter"]["disclosures"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind": "position", "text": "The desk holds venue-a bonds"}));
    let (out, code) = run("disclosed", &promotional);
    assert_eq!(
        code, 0,
        "premise: with the disclosure it is shadowed: {out}"
    );
}

/// The conduct gate and the eligibility register run in the binary, and
/// their refusals are on the record. Mutation: in `Session::pass`, pass
/// `eligibility: None` to `run_governed` — the GB-only venue is shadowed.
#[test]
fn the_binary_withholds_a_pass_the_conduct_gate_or_the_eligibility_register_refuses() {
    let scratch = Scratch::new("refusals");
    let policy_path = scratch.write("policy.json", &policy_document());
    let journal = scratch.journal();

    let mut rumour = request(1);
    rumour["pass"]["communications"]["quoter"]["content"] =
        json!("Quote 20 lots; rumour has it venue-a widens");
    let rumour = scratch.write("rumour.json", &rumour);
    let (out, _, code) = qip(
        &[&policy_path, &journal, &rumour],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("no_rumour_engineering"), "{out}");
    // R12-RES-7110: the withheld plan and the alternative sought are said
    // and journaled. Mutation: in `engine::execute`, return `blocked: None`
    // from the guard-refusal arm — neither the line nor the record appears.
    assert!(
        out.contains("plan withheld and kept: 1 step(s), 1 "),
        "{out}"
    );
    assert!(out.contains("truthful alternative: do nothing"), "{out}");
    assert!(
        records(&journal).iter().any(|r| matches!(
            r,
            AgencyRecord::PlanBlocked { alternative, .. } if alternative == "no_action"
        )),
        "the withheld plan is on the journal"
    );

    let mut elsewhere = policy_document();
    elsewhere["eligibility"][0]["jurisdictions"] = json!(["GB"]);
    let elsewhere = scratch.write("elsewhere.json", &elsewhere);
    let fresh = scratch.dir.join("fresh.jsonl");
    let pass = scratch.write("pass.json", &request(1));
    let (out, _, code) = qip(
        &[&elsewhere, &fresh, &pass],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(code, 3, "{out}");
    assert!(
        out.contains("ineligible: venue `venue-a` is not permitted in `US`"),
        "{out}"
    );
    assert!(out.contains("outcome: no action"), "{out}");
    let journaled = records(&fresh);
    assert!(journaled.iter().any(|r| matches!(
        r,
        AgencyRecord::PassConcluded { eligibility_refusals, .. } if !eligibility_refusals.is_empty()
    )));
}
/// One shadow pass of `document` under the standing policy, on a journal
/// of its own.
fn shadow(scratch: &Scratch, name: &str, document: &Value) -> (String, String, i32) {
    let policy = scratch.write("policy.json", &policy_document());
    let pass = scratch.write(&format!("{name}.json"), document);
    let journal = scratch.dir.join(format!("{name}.jsonl"));
    qip(
        &[&policy, &journal, &pass],
        &["--policy", "--journal", "--request"],
        &[],
    )
}

/// The planning gates, unchanged by the conduct policy now standing in
/// front of them: a bounded plan is shadowed, and one outside its goal,
/// unmeasured, deceptive or on a system nobody owns is declined by name.
/// Every assertion here predates the policy; only the invocation gained the
/// `--policy` and `--journal` it now requires.
#[test]
fn the_shipped_binary_shadows_a_bounded_plan_and_declines_one_outside_its_goal() {
    let scratch = Scratch::new("bounded");
    // Premise: inside every bound the plan clears every gate, is reported as
    // what would have run, and the run exits zero.
    let (out, err, code) = shadow(&scratch, "bounded", &request(1));
    assert_eq!(code, 0, "premise: stdout {out} stderr {err}");
    assert!(
        out.contains(
            "stages passed: [Observe, Predict, Specify, IdentifyLevers, Generate, Simulate, Gate]"
        ),
        "{out}"
    );
    assert!(out.contains("outcome: shadowed"), "{out}");
    assert!(out.contains("quoter on quote_size, exposure 20"), "{out}");

    // Exposure 51 against an envelope of 50: refused by the plan's own
    // constructor, named, and the baseline is what remains.
    let mut over = request(1);
    over["proposals"][0]["root"]["step"]["exposure"] = json!("51");
    let (out, _, code) = shadow(&scratch, "over", &over);
    assert_eq!(code, 3, "{out}");
    assert!(
        out.contains("refused proposal 1: plan exposure 51 exceeds risk envelope 50"),
        "{out}"
    );
    assert!(out.contains("outcome: no action"), "{out}");

    // A lever nobody has measured stops at the gate, however good the plan.
    let mut unmeasured = request(1);
    unmeasured["evidence"] = json!({});
    let (out, _, code) = shadow(&scratch, "unmeasured", &unmeasured);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("stage failed: Gate"), "{out}");
    assert!(
        out.contains("outcome: gather evidence; nothing is measured about: quote_size"),
        "{out}"
    );

    // A deceptive method is infeasible whatever effect is claimed for it.
    let mut deceptive = request(1);
    deceptive["tool_edges"][0]["edge"]["method"] = json!("spoofing");
    deceptive["proposals"][0]["comparison"]["expected_causal_effect"] = json!("1000000000");
    let (out, _, code) = shadow(&scratch, "deceptive", &deceptive);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("deceptive or manipulative method"), "{out}");

    // An operational tool on a system outside the owned registry is not a
    // lever this binary will plan with; the same edge on an owned one is.
    let mut foreign = request(1);
    foreign["tool_edges"][0]["edge"]["method"] = json!("operational");
    let (out, err, code) = shadow(&scratch, "foreign", &foreign);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("owned-system registry"), "{err}");
    foreign["variables"][0]["owned"] = json!(true);
    let (out, err, code) = shadow(&scratch, "owned", &foreign);
    assert_eq!(code, 0, "stdout {out} stderr {err}");

    // A goal missing a declaration is not a request: exit one, nothing run.
    let mut vague = request(1);
    vague["goal"].as_object_mut().unwrap().remove("budget");
    let (out, err, code) = shadow(&scratch, "vague", &vague);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("`budget` is not declared"), "{err}");
    assert!(!out.contains("outcome"), "{out}");
}

/// R12-GOV-6123: the revocation path is reachable from the shipped binary.
/// A tool the operator's policy revokes is refused on the next pass, and the
/// refusal is journaled with the hash of the policy that revoked it; a
/// revocation naming no registered tool stops the binary instead of leaving
/// the tool it meant live.
///
/// Mutation: in `Session::open`, drop the `guard.revoke` loop — the revoked
/// pass is shadowed and exits zero, and the typo is accepted.
#[test]
fn a_tool_the_operator_revokes_is_refused_on_the_next_pass_and_the_refusal_is_journaled() {
    let scratch = Scratch::new("revoked");
    // Premise: the same pass under the unrevoked policy is shadowed.
    let (out, err, code) = shadow(&scratch, "live", &request(1));
    assert_eq!(code, 0, "premise: stdout {out} stderr {err}");
    assert!(out.contains("outcome: shadowed"), "{out}");

    let mut revoked = policy_document();
    revoked["revoked_tools"] = json!(["quoter"]);
    let policy = scratch.write("revoked-policy.json", &revoked);
    let journal = scratch.dir.join("revoked.jsonl");
    let pass = scratch.write("revoked-pass.json", &request(1));
    let (out, err, code) = qip(
        &[&policy, &journal, &pass],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(
        code, 3,
        "a revoked tool's pass was not withheld: {out} {err}"
    );
    assert!(out.contains("tool is revoked"), "{out}");
    assert!(!out.contains("outcome: shadowed"), "{out}");

    let policy_hash = format!(
        "sha256:{}",
        qip_core::sha256_hex(std::fs::read_to_string(&policy).unwrap().as_bytes())
    );
    let concluded: Vec<_> = records(&journal)
        .into_iter()
        .filter_map(|record| match record {
            AgencyRecord::PassConcluded {
                outcome,
                detail,
                provenance,
                ..
            } => Some((outcome, detail, provenance.policy_version)),
            _ => None,
        })
        .collect();
    assert_eq!(concluded.len(), 1, "{concluded:?}");
    let (outcome, detail, version) = &concluded[0];
    assert_ne!(outcome, "shadowed", "{concluded:?}");
    assert!(detail.contains("tool is revoked"), "{concluded:?}");
    assert_eq!(
        version, &policy_hash,
        "the refusal is not attributed to the policy that revoked the tool"
    );

    let mut typo = policy_document();
    typo["revoked_tools"] = json!(["qouter"]);
    let typo = scratch.write("typo-policy.json", &typo);
    let fresh = scratch.dir.join("typo.jsonl");
    let (out, err, code) = qip(
        &[&typo, &fresh, &pass],
        &["--policy", "--journal", "--request"],
        &[],
    );
    assert_eq!(
        code, 1,
        "a revocation naming no tool was accepted: {out} {err}"
    );
    assert!(
        err.contains("revokes `qouter`, which it does not register"),
        "{err}"
    );
}
