//! The agency loop's journal, read back from the shipped `qip agency shadow`
//! binary, one row of R12's Causal Agency Engine at a time: the goal, the
//! affordance graph's levers and tools, the structural causal model's
//! simulated branches, the policy-constrained plan and the no-action
//! baseline it falls back to, the registry's refusal of an undeclared tool,
//! an incomplete tool declaration, and the identified estimators a widening
//! is checked against (R12-AGENCY-1146..1151, 3004, 6091, 6102, 6104, 9063).
//!
//! A separate file from `agency.rs` because that suite and `src/agency.rs`
//! are another lane's while this one runs; the helpers are copied, not
//! shared, so neither lane edits the other's file. Every test drives the
//! process, so a pass that `main` does not reach, or a record the journal
//! does not keep, fails here rather than in a library test that would pass
//! without either.
//!
//! The premise comes first: a refusal proves nothing unless the same
//! document, inside its bounds, is admitted.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use qip_agency::autonomy::Authority;
use qip_agency::journal::{self, AgencyRecord};
use qip_agency::memory::Observed;
use qip_core::Decimal;
use qip_events::EventLog;
use serde_json::{Value, json};
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
                "revocation_path": "operator removes it from this file"
            }]
        },
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
            "stop_conditions": ["spread widens"]
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

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("qip-agency-record-{}-{name}", std::process::id()));
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

/// One shadow pass of `document` under `policy`, on a journal of its own;
/// what it printed, exited with, and journaled.
fn pass(name: &str, policy: &Value, document: &Value) -> (String, i32, Vec<AgencyRecord>) {
    let scratch = Scratch::new(name);
    let policy = scratch.write("policy.json", policy);
    let request = scratch.write("request.json", document);
    let journal = scratch.journal();
    let (out, err, code) = qip(
        &[&policy, &journal, &request],
        &["--policy", "--journal", "--request"],
        &[],
    );
    let journaled = if journal.exists() {
        records(&journal)
    } else {
        Vec::new()
    };
    (format!("{out}\n{err}"), code, journaled)
}

fn concluded(records: &[AgencyRecord]) -> (&str, &[String], &[String], usize) {
    match records.last() {
        Some(AgencyRecord::PassConcluded {
            outcome,
            stages_passed,
            steps,
            simulations,
            ..
        }) => (outcome, stages_passed, steps, simulations.len()),
        other => panic!("the pass ends concluded: {other:?}"),
    }
}

/// The desk's structural model: quote size narrows the spread, in a calm and
/// a stressed scenario.
fn with_structural_model(mut document: Value) -> Value {
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
    document
}

/// R12-AGENCY-1146/1147/1148/1150: a pass journals the goal it pursued and
/// who pursued it, the stages that identified its levers on the affordance
/// graph and simulated them, the tool and lever each step pulls, and one
/// simulated branch per structural model and scenario.
///
/// Mutations: in `journal::record_pass`, write `goal_target` as an empty
/// string (the goal assertion fails); in `engine::run_governed`, drop the
/// simulations from the report (the branch assertion fails).
#[test]
fn a_shadow_pass_journals_its_goal_levers_tools_and_each_simulated_causal_branch() {
    let (out, code, journaled) = pass(
        "goal",
        &policy_document(),
        &with_structural_model(request(1)),
    );
    assert_eq!(
        code, 0,
        "premise: inside every bound the pass clears: {out}"
    );
    let Some(AgencyRecord::PassOpened {
        goal_target,
        acting_identity,
        ..
    }) = journaled.first()
    else {
        panic!("the pass opens with its goal: {journaled:?}")
    };
    assert_eq!(goal_target, "spread under 5 bps");
    assert_eq!(acting_identity, "desk-agent");
    let (outcome, stages, steps, simulations) = concluded(&journaled);
    assert_eq!(outcome, "shadowed");
    for stage in ["Specify", "IdentifyLevers", "Simulate", "Gate"] {
        assert!(stages.iter().any(|s| s == stage), "{stage}: {stages:?}");
    }
    assert_eq!(steps, ["quoter on quote_size, exposure 20"]);
    assert_eq!(
        simulations, 2,
        "one branch per scenario under the one model"
    );
    let Some(AgencyRecord::PassConcluded { simulations, .. }) = journaled.last() else {
        panic!()
    };
    let branches: Vec<(&str, &str)> = simulations
        .iter()
        .map(|b| (b.model.as_str(), b.scenario.as_str()))
        .collect();
    assert_eq!(branches, [("desk", "calm"), ("desk", "stressed")]);
}

/// R12-AGENCY-1149/1151: the plan is constrained by the goal's risk envelope
/// and the venue's legal eligibility, and what remains when every proposal
/// is refused is the no-action baseline, journaled as the outcome.
/// Mutation: in `plan::InterventionPlan`'s envelope check, compare against
/// an envelope twice as wide — the over-envelope pass is shadowed.
#[test]
fn a_plan_outside_its_envelope_is_refused_and_the_journal_records_the_no_action_baseline() {
    let (out, code, journaled) = pass("inside", &policy_document(), &request(1));
    assert_eq!(code, 0, "premise: {out}");
    assert_eq!(concluded(&journaled).0, "shadowed");

    let mut over = request(1);
    over["proposals"][0]["root"]["step"]["exposure"] = json!("51");
    let (out, code, journaled) = pass("over", &policy_document(), &over);
    assert_eq!(code, 3, "{out}");
    let (outcome, _, steps, _) = concluded(&journaled);
    assert_eq!(outcome, "no_action", "{journaled:?}");
    assert!(steps.is_empty(), "the baseline pulls no lever: {steps:?}");
}

/// R12-AGENCY-3004/6091: a step on a tool the standing policy never
/// registered, or on a lever whose authority the acting identity lacks, is
/// refused and leaves no shadowed step on the record.
/// Mutation: in `guard::ActionGuard::preflight`, skip the registry lookup —
/// the unregistered tool is shadowed.
#[test]
fn a_tool_the_policy_never_registered_or_an_authority_not_held_is_refused_on_the_record() {
    let (_, code, _) = pass("registered", &policy_document(), &request(1));
    assert_eq!(code, 0, "premise: the registered tool clears");

    let mut unregistered = request(1);
    unregistered["tool_edges"][0]["edge"]["tool"] = json!("spoofer");
    unregistered["proposals"][0]["root"]["step"]["tool"] = json!("spoofer");
    unregistered["pass"]["communications"] = json!({"spoofer":
        unregistered["pass"]["communications"]["quoter"].clone()});
    let (out, code, journaled) = pass("unregistered", &policy_document(), &unregistered);
    assert_eq!(code, 3, "{out}");
    let (outcome, _, steps, _) = concluded(&journaled);
    assert_ne!(outcome, "shadowed", "{out}");
    assert!(steps.is_empty() || outcome == "stopped", "{journaled:?}");
    assert!(
        out.contains("tool `spoofer` is not registered"),
        "the registry is what refused: {out}"
    );

    let mut unauthorised = request(1);
    unauthorised["identity"]["authorities"] = json!([]);
    let (out, code, journaled) = pass("authority", &policy_document(), &unauthorised);
    assert_eq!(code, 3, "{out}");
    assert_ne!(concluded(&journaled).0, "shadowed", "{out}");
    assert!(
        out.contains("identity `desk-agent` does not hold authority `quote`"),
        "the authority check is what refused: {out}"
    );
}

/// R12-AGENCY-9063: a tool declaration missing its acting identity, its
/// rate limit, its revocation path or its disclosure policy stops the binary
/// before any pass. Mutation: in `ToolRegistration`'s validation, accept a
/// blank revocation path — the blank case exits zero.
#[test]
fn a_tool_declaration_missing_any_control_stops_the_binary_before_a_pass() {
    let (_, code, journaled) = pass("complete", &policy_document(), &request(1));
    assert_eq!(code, 0, "premise: the complete declaration runs");
    assert!(!journaled.is_empty());

    for (field, blank) in [
        ("acting_identity", json!("")),
        ("revocation_path", json!("")),
        ("disclosure_policy", json!("")),
        ("max_actions_per_window", json!(0)),
    ] {
        let mut policy = policy_document();
        policy["guard"]["tools"][0][field] = blank;
        let (out, code, journaled) = pass(field, &policy, &request(1));
        assert_eq!(code, 1, "{field} blank: {out}");
        assert!(journaled.is_empty(), "{field}: no pass ran");

        let mut policy = policy_document();
        policy["guard"]["tools"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let (out, code, _) = pass(&format!("{field}-missing"), &policy, &request(1));
        assert_eq!(code, 1, "{field} missing: {out}");
        assert!(out.contains(field), "{field} named: {out}");
    }
}

/// An attribution of `effect` under `identification`, on otherwise complete
/// evidence.
fn attribution(identification: Value, effect: f64) -> Value {
    json!({
        "goal_id": "g1", "intervention_id": "i1", "action_id": "a1",
        "action_class": "market",
        "identification": identification,
        "observations": [{"horizon_ms": 60000, "value": effect}],
        "counterfactual_method": "named by the estimator",
        "effect": {"mean": effect, "std_dev": 0.5},
        "causal_chain": ["quote_size", "spread"],
        "confounders": ["none considered"],
        "competing_explanations": ["seasonality"],
        "side_effects": ["none observed"],
        "identifiability": 0.8,
        "resulting_updates": ["raise quote-size prior"]
    })
}

fn widening(attribution: Value, evidence: Value) -> Value {
    let mut document = request(1);
    document["widening"] = json!({
        "class": "market",
        "approver": {"name": "risk-officer", "authorities": ["agency:widen-autonomy"]},
        "attribution": attribution,
        "evidence": evidence
    });
    document
}

/// R12-AGENCY-6102: each identified estimator the binary runs is journaled
/// with its method, and the label an attribution claims must be the one the
/// method earns — a difference-in-differences estimate written up as an
/// experiment widens nothing. Mutation: in `estimate::Estimate::supports`,
/// skip the identification comparison — the relabelled pass widens.
#[test]
fn each_estimator_is_journaled_with_its_method_and_a_label_it_did_not_earn_is_refused() {
    let did = json!({"difference_in_differences": {
        "treated_before": [1.0, 2.0], "treated_after": [5.0, 6.0],
        "control_before": [1.0, 2.0], "control_after": [2.0, 3.0]
    }});
    let iv = json!({"instrumental_variable": {
        "instrument": [0.0, 1.0, 0.0, 1.0],
        "treatment": [0.0, 1.0, 0.0, 1.0],
        "outcome": [0.0, 3.0, 0.0, 3.0]
    }});
    let cases = [
        (
            "did",
            json!({"observational": {"strategy": "difference-in-differences under parallel trends"}}),
            did.clone(),
            "difference_in_differences",
        ),
        (
            "iv",
            json!({"observational": {"strategy": "instrumental variable (Wald)"}}),
            iv,
            "instrumental_variable",
        ),
        (
            "rct",
            json!("experiment"),
            json!({"randomised_experiment": {"treated": [3.0, 4.0, 5.0], "control": [0.0, 1.0, 2.0]}}),
            "randomised_experiment",
        ),
    ];
    for (name, label, evidence, method) in cases {
        let (out, _, journaled) = pass(
            name,
            &policy_document(),
            &widening(attribution(label, 3.0), evidence),
        );
        let widened: Vec<String> = journaled
            .iter()
            .filter_map(|r| match r {
                AgencyRecord::Widened { estimator, .. } => Some(
                    serde_json::to_value(estimator)
                        .unwrap()
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(widened, [method], "{name}: {out}");
    }

    let (out, _, journaled) = pass(
        "relabelled",
        &policy_document(),
        &widening(attribution(json!("experiment"), 3.0), did),
    );
    assert!(
        journaled.iter().any(|r| matches!(
            r,
            AgencyRecord::WideningRefused { reason, .. } if reason.contains("earned by the method")
        )),
        "{out}"
    );
    assert!(
        !journaled
            .iter()
            .any(|r| matches!(r, AgencyRecord::Widened { .. }))
    );
}

/// R12-AGENCY-6104: an attribution with no causal chain, no confounders, no
/// rival explanation or no horizon observation is refused on the record and
/// widens nothing. Mutation: in `EffectAttribution::validate`, skip the
/// chain check — the chainless attribution widens.
#[test]
fn an_attribution_missing_its_chain_confounders_rivals_or_horizons_is_refused_on_the_record() {
    let evidence =
        json!({"randomised_experiment": {"treated": [3.0, 4.0, 5.0], "control": [0.0, 1.0, 2.0]}});
    let (out, _, journaled) = pass(
        "complete-attribution",
        &policy_document(),
        &widening(attribution(json!("experiment"), 3.0), evidence.clone()),
    );
    assert!(
        journaled
            .iter()
            .any(|r| matches!(r, AgencyRecord::Widened { .. })),
        "premise: the complete attribution widens: {out}"
    );
    for field in [
        "causal_chain",
        "confounders",
        "competing_explanations",
        "observations",
    ] {
        let mut incomplete = attribution(json!("experiment"), 3.0);
        incomplete[field] = json!([]);
        let (out, _, journaled) = pass(
            field,
            &policy_document(),
            &widening(incomplete, evidence.clone()),
        );
        assert!(
            journaled
                .iter()
                .any(|r| matches!(r, AgencyRecord::WideningRefused { .. })),
            "{field}: {out}"
        );
        assert!(
            !journaled
                .iter()
                .any(|r| matches!(r, AgencyRecord::Widened { .. })),
            "{field}: {out}"
        );
    }
}

/// A statistic matches the hand computation to well inside a JSON round
/// trip's precision.
fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

/// The one `Learned` record of a pass, or `None`.
fn learned(records: &[AgencyRecord]) -> Option<&AgencyRecord> {
    let mut found = records
        .iter()
        .filter(|r| matches!(r, AgencyRecord::Learned { .. }));
    let first = found.next();
    assert!(found.next().is_none(), "one pass learns once: {records:?}");
    first
}

/// The reasons every `LearningWithheld` record of a pass gives.
fn withheld(records: &[AgencyRecord]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|r| match r {
            AgencyRecord::LearningWithheld { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect()
}

/// R12-AGENCY-1006 / 1152 / 1199 / 6106 / 6107 / 7133: a shadowed pass with a
/// structural model runs the observe-effect, attribute and update stages on
/// the outcome it simulated, and the journal carries that outcome labelled
/// simulated, its attribution, and the lever update — the effect the policy
/// was held to, the interval it predicted, and its authority before and
/// after — so the update is recomputable from the log alone.
///
/// The figures are computed by hand from the fixture: `spread = 6 - 0.6 *
/// quote_size`, one quoter step moves `quote_size` by 10. Calm: 6 without
/// the action, 0 with it. Stressed (+3 exogenous): 9 and 3. The objective is
/// a decrease, so figures are signed toward the goal: observed -1.5 (the
/// mean of 0 and 3, negated), counterfactual -7.5 (the mean of 6 and 9,
/// negated), effect 6 in both branches, standard deviation 0. The market
/// policy was widened on an experiment and predicts uplift in [1, 5]; a
/// simulated 6 is outside it, so the widened policy returns to shadow.
/// Under a policy predicting [1, 10] it keeps its authority.
///
/// Mutations: in `agency::Session::learn`, return `Learning::withheld` for
/// every pass (no `Learned` record: the first assertion fails); in
/// `simulated::evidence`, write `Effect { mean: 0.0, std_dev: 0.0 }` and the
/// horizon value as `0.0` (the hand-computed effect assertions fail).
#[test]
fn a_shadowed_pass_learns_from_its_simulated_outcome_and_the_journal_carries_the_lever_update() {
    let evidence =
        json!({"randomised_experiment": {"treated": [3.0, 4.0, 5.0], "control": [0.0, 1.0, 2.0]}});
    let document = with_structural_model(widening(attribution(json!("experiment"), 3.0), evidence));
    let (out, code, journaled) = pass("learned", &policy_document(), &document);
    assert_eq!(code, 0, "premise: the pass is shadowed: {out}");
    assert!(
        journaled
            .iter()
            .any(|r| matches!(r, AgencyRecord::Widened { .. })),
        "premise: the policy was widened before the pass: {out}"
    );
    let Some(AgencyRecord::Learned {
        outcome,
        attribution,
        effect,
        predicted_uplift_low,
        predicted_uplift_high,
        predicted_side_effects,
        authority_before,
        authority_after,
        ..
    }) = learned(&journaled)
    else {
        panic!("the pass learned from its simulated outcome: {out}\n{journaled:?}")
    };
    assert!(withheld(&journaled).is_empty(), "{journaled:?}");

    // The outcome, as the memory kept it, says it is simulated and credits
    // exactly the step that was shadowed.
    assert_eq!(outcome.observed, Observed::Simulated);
    assert_eq!(outcome.action_sequence, ["quoter"]);
    assert_eq!(
        outcome.environment_state,
        "simulated: desk/calm, desk/stressed"
    );
    assert_eq!(outcome.audience, "simulated: nobody reached");
    assert_eq!(outcome.delay_secs, 60, "the plan's own time to effect");
    assert_eq!(outcome.predicted_effect, Decimal::from_int(5));
    assert_eq!(
        outcome.actual_observations,
        Decimal::from_f64(-1.5).unwrap()
    );
    assert_eq!(
        outcome.no_action_counterfactual,
        Decimal::from_f64(-7.5).unwrap()
    );
    assert_eq!(
        outcome.attribution_confidence,
        Decimal::from_f64(0.9).unwrap()
    );
    assert_eq!(*effect, Decimal::from_int(6));

    // The attribution names the model, not the world, and carries the chain.
    assert!(
        close(attribution.effect.mean, 6.0),
        "{:?}",
        attribution.effect
    );
    assert!(
        close(attribution.effect.std_dev, 0.0),
        "{:?}",
        attribution.effect
    );
    assert!(
        close(attribution.identifiability, 0.9),
        "{}",
        attribution.identifiability
    );
    assert_eq!(attribution.observations.len(), 1);
    assert_eq!(attribution.observations[0].horizon_ms, 60_000);
    assert!(
        close(attribution.observations[0].value, 6.0),
        "{:?}",
        attribution.observations
    );
    assert_eq!(attribution.causal_chain, ["quote_size", "spread"]);
    let strategy = serde_json::to_value(&attribution.identification).unwrap();
    let strategy = strategy["observational"]["strategy"]
        .as_str()
        .expect("an observational label naming the model");
    assert!(
        strategy.starts_with("simulated:") && strategy.contains("nothing was observed"),
        "{strategy}"
    );

    // The update: 6 lies outside the predicted [1, 5], so the widened policy
    // is back at shadow, and the record carries what it was judged against.
    assert_eq!(
        (*predicted_uplift_low, *predicted_uplift_high),
        (Decimal::from_int(1), Decimal::from_int(5))
    );
    assert!(predicted_side_effects.is_empty());
    assert_eq!(
        (*authority_before, *authority_after),
        (Authority::NarrowReversible, Authority::Shadow)
    );
    let (verdict, stages, _, _) = concluded(&journaled);
    assert_eq!(verdict, "shadowed");
    for stage in ["ObserveEffect", "Attribute", "Update"] {
        assert!(stages.iter().any(|s| s == stage), "{stage}: {stages:?}");
    }
    assert!(
        !stages.iter().any(|s| s == "Act"),
        "a shadowed act is not listed as passed: {stages:?}"
    );
    assert!(
        out.contains("learned from the simulated outcome, not a measured one: effect 6"),
        "{out}"
    );

    // Inside its predicted interval the widened policy keeps its authority.
    let mut wide = policy_document();
    wide["action_policies"][0]["uplift_high"] = json!("10");
    let (out, code, journaled) = pass("kept", &wide, &document);
    assert_eq!(code, 0, "{out}");
    let Some(AgencyRecord::Learned {
        authority_before,
        authority_after,
        predicted_uplift_high,
        ..
    }) = learned(&journaled)
    else {
        panic!("{out}\n{journaled:?}")
    };
    assert_eq!(*predicted_uplift_high, Decimal::from_int(10));
    assert_eq!(
        (*authority_before, *authority_after),
        (Authority::NarrowReversible, Authority::NarrowReversible)
    );
}

/// A pass whose outcome is absent learns nothing and says why, rather than
/// learning from an outcome somebody made up: no structural model simulated
/// the plan; no action policy is configured for the plan's class; the pass
/// was not shadowed at all. Each leaves a `LearningWithheld` record naming
/// the absence and no `Learned` record, and the stages stop where the pass
/// stopped.
///
/// Mutation: in `agency::Session::learn`, replace the reason passed to
/// `Learning::withheld` on the evidence refusal with a fixed string — the
/// reason assertions fail, because a refusal that does not name what is
/// missing cannot be acted on.
#[test]
fn a_pass_with_no_simulated_outcome_journals_a_withheld_learning_naming_what_is_absent() {
    let (out, code, journaled) = pass(
        "premise-learns",
        &policy_document(),
        &with_structural_model(request(1)),
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        learned(&journaled).is_some(),
        "premise: with a model and a policy the same pass learns: {out}"
    );

    // No structural model: nothing was simulated.
    let (out, code, journaled) = pass("no-model", &policy_document(), &request(1));
    assert_eq!(code, 0, "the pass itself still clears every gate: {out}");
    assert!(learned(&journaled).is_none(), "{journaled:?}");
    let reasons = withheld(&journaled);
    assert_eq!(reasons.len(), 1, "{journaled:?}");
    assert!(
        reasons[0].contains("no structural model simulated"),
        "{}",
        reasons[0]
    );
    let (verdict, stages, _, _) = concluded(&journaled);
    assert_eq!(verdict, "shadowed");
    assert!(
        !stages.iter().any(|s| s == "ObserveEffect"),
        "no learn stage ran: {stages:?}"
    );
    assert!(
        out.contains("learning withheld: no structural model simulated"),
        "{out}"
    );

    // A model but no policy for the class: nothing to hold to a prediction.
    let mut unpoliced = policy_document();
    unpoliced["action_policies"] = json!([]);
    let (out, code, journaled) = pass("no-policy", &unpoliced, &with_structural_model(request(1)));
    assert_eq!(code, 0, "{out}");
    assert!(learned(&journaled).is_none(), "{journaled:?}");
    let reasons = withheld(&journaled);
    assert_eq!(reasons.len(), 1, "{journaled:?}");
    assert!(
        reasons[0].contains("no action policy is configured for Market"),
        "{}",
        reasons[0]
    );

    // Not shadowed: the plan was refused, so no action was simulated.
    let mut over = with_structural_model(request(1));
    over["proposals"][0]["root"]["step"]["exposure"] = json!("51");
    let (out, code, journaled) = pass("not-shadowed", &policy_document(), &over);
    assert_eq!(code, 3, "{out}");
    assert!(learned(&journaled).is_none(), "{journaled:?}");
    let reasons = withheld(&journaled);
    assert_eq!(reasons.len(), 1, "{journaled:?}");
    assert!(reasons[0].contains("not shadowed"), "{}", reasons[0]);
    assert_eq!(concluded(&journaled).0, "no_action");
}
