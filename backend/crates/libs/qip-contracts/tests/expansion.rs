//! CONTRACT-027/028/030/031/032: each goal-to-effect record is refused when any
//! listed field is missing or blank, and an attribution must name a goal,
//! intervention and action that exist.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::expansion::{
    CurriculumItem, EffectAttribution, GapSignal, GoalSpec, InterventionPlan,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn goal() -> Value {
    json!({
        "desired_state": "drawdown below 4 percent", "target_entities": ["book-a"],
        "horizon_ms": 86_400_000u64, "success_metric": "max drawdown", "budget": "1000",
        "risk_constraints": ["no new venue"], "identity": "desk-1", "jurisdiction": "US",
        "authority_class": "shadow", "prohibited_methods": ["live orders"],
        "stop_rollback_conditions": ["drawdown above 6 percent"]
    })
}

fn plan() -> Value {
    json!({
        "causal_hypothesis": "wider spreads cut fills", "levers": ["quote width"],
        "candidate_actions": ["widen by 1 bp"], "tool_ids": ["sim-1"],
        "expected_effect": {"mean": 0.5, "std_dev": 0.1}, "cost": "10", "timing_ms": 5000,
        "reversibility": "reversible", "simulations_run": ["twin-run-7"],
        "no_action_baseline": "keep current width", "required_approvals": ["risk desk"],
        "abort_rules": ["fill rate halves"]
    })
}

fn attribution() -> Value {
    json!({
        "goal_id": "g1", "intervention_id": "i1", "action_id": "a1",
        "observations": [{"horizon_ms": 1000, "value": 0.4}],
        "counterfactual_method": "synthetic control", "effect": {"mean": 0.4, "std_dev": 0.2},
        "confounders": ["regime shift"], "side_effects": ["none observed in window"],
        "identifiability": 0.6, "resulting_updates": ["raise width prior"]
    })
}

fn gap() -> Value {
    json!({
        "observation": "surprise on CPI", "affected_domains": ["macro"], "evidence": ["ev-1"],
        "severity": 0.7, "economic_value": "500", "gap_class": "causal"
    })
}

fn item() -> Value {
    json!({
        "research_question": "does CPI lead rates", "expected_information_value": 1.5,
        "expected_economic_value": "900", "tools_and_data": ["cpi series"], "budget": "50",
        "owner": "deepbrain", "evaluation_suite": "macro-suite", "stop_conditions": ["budget spent"]
    })
}

type Decode = fn(&[u8]) -> qip_core::error::Result<()>;

fn decoders() -> Vec<(&'static str, Value, Decode)> {
    vec![
        ("goal", goal(), |b| GoalSpec::decode(b).map(|_| ())),
        ("plan", plan(), |b| InterventionPlan::decode(b).map(|_| ())),
        ("gap", gap(), |b| GapSignal::decode(b).map(|_| ())),
        ("item", item(), |b| CurriculumItem::decode(b).map(|_| ())),
        ("attribution", attribution(), |b| {
            serde_json::from_slice::<EffectAttribution>(b)
                .map_err(|e| qip_core::error::Error::schema(e.to_string()))?
                .validate()
        }),
    ]
}

#[test]
fn a_complete_record_of_each_kind_is_accepted_and_a_missing_field_is_refused_by_every_decoder() {
    for (name, valid, decode) in decoders() {
        let bytes = serde_json::to_vec(&valid).expect("encodes");
        assert!(decode(&bytes).is_ok(), "{name} complete record was refused");
        let keys: Vec<String> = valid.as_object().expect("object").keys().cloned().collect();
        assert!(
            keys.len() >= 6,
            "{name} premise: the record lists its fields"
        );
        for key in keys {
            let mut v = valid.clone();
            v.as_object_mut().expect("object").remove(&key);
            let bytes = serde_json::to_vec(&v).expect("encodes");
            assert!(decode(&bytes).is_err(), "{name} without {key} was accepted");
        }
    }
}

#[test]
fn a_blank_or_empty_listed_field_is_refused_naming_the_field() {
    for (name, valid, decode) in decoders() {
        let obj = valid.as_object().expect("object").clone();
        let mut refused = 0;
        for (key, val) in &obj {
            let blank = match val {
                Value::String(_) => json!(""),
                Value::Array(_) => json!([]),
                _ => continue,
            };
            // Enum arms are refused by serde with their own variant list.
            if ["authority_class", "gap_class", "reversibility"].contains(&key.as_str()) {
                continue;
            }
            let mut v = valid.clone();
            v[key] = blank;
            let err = decode(&serde_json::to_vec(&v).expect("encodes"))
                .expect_err(&format!("{name}.{key} blank was accepted"));
            assert!(
                err.to_string().contains(key.as_str()) || err.to_string().contains("decimal"),
                "{name}.{key}: refusal did not name the field: {err}"
            );
            refused += 1;
        }
        assert!(refused >= 3, "{name} premise: some fields were blanked");
    }
}

#[test]
fn a_goal_with_no_stop_or_rollback_condition_or_a_zero_horizon_or_budget_is_refused() {
    let mut g = goal();
    g["stop_rollback_conditions"] = json!([]);
    assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
    let mut g = goal();
    g["horizon_ms"] = json!(0);
    assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
    let mut g = goal();
    g["budget"] = json!("0");
    assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
}

#[test]
fn an_authority_class_has_no_live_arm() {
    let mut g = goal();
    g["authority_class"] = json!("live");
    assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
}

#[test]
fn a_plan_without_a_no_action_baseline_or_abort_rules_is_refused() {
    let mut p = plan();
    p["no_action_baseline"] = json!("  ");
    let err = InterventionPlan::decode(&serde_json::to_vec(&p).expect("e")).expect_err("blank");
    assert!(err.to_string().contains("no_action_baseline"));
    let mut p = plan();
    p["abort_rules"] = json!([]);
    assert!(InterventionPlan::decode(&serde_json::to_vec(&p).expect("e")).is_err());
}

#[test]
fn a_gap_class_outside_the_section_23_1_classes_is_refused_and_each_named_class_is_accepted() {
    for class in [
        "data",
        "ontology",
        "causal",
        "memory",
        "model",
        "tool",
        "specialist",
        "execution",
        "compute_quantum_research",
    ] {
        let mut g = gap();
        g["gap_class"] = json!(class);
        assert!(
            GapSignal::decode(&serde_json::to_vec(&g).expect("e")).is_ok(),
            "{class}"
        );
    }
    let mut g = gap();
    g["gap_class"] = json!("vibes");
    assert!(GapSignal::decode(&serde_json::to_vec(&g).expect("e")).is_err());
}

#[test]
fn an_attribution_must_name_a_goal_an_intervention_and_an_action_that_exist() {
    let a: EffectAttribution = serde_json::from_value(attribution()).expect("decodes");
    let set = |s: &str| BTreeSet::from([s.to_string()]);
    assert!(
        a.validate_references(&set("g1"), &set("i1"), &set("a1"))
            .is_ok()
    );
    for (g, i, ac, missing) in [
        ("x", "i1", "a1", "goal"),
        ("g1", "x", "a1", "intervention"),
        ("g1", "i1", "x", "action"),
    ] {
        let err = a
            .validate_references(&set(g), &set(i), &set(ac))
            .expect_err("unknown reference");
        assert!(err.to_string().contains(missing), "{err}");
    }
}

#[test]
fn an_attribution_with_no_confounders_or_an_out_of_range_identifiability_is_refused() {
    let mut v = attribution();
    v["identifiability"] = json!(1.5);
    let a: EffectAttribution = serde_json::from_value(v).expect("decodes");
    assert!(a.validate().is_err());
    let mut v = attribution();
    v["confounders"] = json!([]);
    let a: EffectAttribution = serde_json::from_value(v).expect("decodes");
    assert!(a.validate().is_err());
}

#[test]
fn a_curriculum_item_without_budget_owner_or_stop_conditions_is_refused() {
    let mut v = item();
    v["budget"] = json!("0");
    assert!(CurriculumItem::decode(&serde_json::to_vec(&v).expect("e")).is_err());
    let mut v = item();
    v["owner"] = json!("");
    assert!(CurriculumItem::decode(&serde_json::to_vec(&v).expect("e")).is_err());
    let mut v = item();
    v["stop_conditions"] = json!([]);
    assert!(CurriculumItem::decode(&serde_json::to_vec(&v).expect("e")).is_err());
}
