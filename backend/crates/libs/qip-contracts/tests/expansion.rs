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
        "class": "risk_reduction", "target_state": "drawdown below 4 percent",
        "entities_affected": ["book-a"], "time_horizon_secs": 86_400u64,
        "success_metric": "max drawdown", "acceptable_uncertainty": "0.2", "budget": "1000",
        "risk_envelope": "50", "jurisdictions": ["US"], "acting_identity": "desk-1",
        "prohibited_side_effects": ["public statement"], "prohibited_methods": ["live orders"],
        "stop_conditions": ["drawdown above 6 percent"],
        "rollback_conditions": ["close the hedge opened for this goal"]
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
        "action_class": "market", "identification": "experiment",
        "observations": [{"horizon_ms": 1000, "value": 0.4}, {"horizon_ms": 60000, "value": 0.3}],
        "counterfactual_method": "synthetic control", "effect": {"mean": 0.4, "std_dev": 0.2},
        "causal_chain": ["quote width", "fill rate"], "confounders": ["regime shift"],
        "competing_explanations": ["seasonal volume"], "side_effects": ["none observed in window"],
        "identifiability": 0.6, "resulting_updates": ["raise width prior"]
    })
}

/// The attribution with an observational identification, the variant that
/// carries a field of its own.
fn observational_attribution() -> Value {
    let mut v = attribution();
    v["identification"] = json!({"observational": {"strategy": "difference in differences"}});
    v
}

fn gap() -> Value {
    json!({
        "observation": "surprise on CPI", "affected_domains": ["macro"], "evidence": ["ev-1"],
        "severity": 0.7, "economic_value": "500", "risk_value": "120",
        "gap_class": "causal"
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

/// CONTRACT-7083: the attribution's nested required fields are refused when
/// missing, as its top-level ones are. The test above removes top-level keys
/// only, so a `#[serde(default)]` on `Effect::std_dev` decoded an attribution
/// with no spread as `std_dev = 0`, which `validate` admits. Every test still
/// passed, and an effect with no stated uncertainty read as a certain one
/// (the AUDIT-5 verification, 2026-10-24).
///
/// Mutation: add `#[serde(default)]` to `Effect::std_dev`. The case naming
/// `effect.std_dev` is then accepted, and the assertion fails.
#[test]
fn an_attribution_missing_a_nested_required_field_is_refused() {
    let decode = |v: &Value| {
        serde_json::from_value::<EffectAttribution>(v.clone())
            .map_err(|e| e.to_string())
            .and_then(|a| a.validate().map_err(|e| e.message().to_string()))
    };
    let observational = observational_attribution();
    // Premise: both complete fixtures are accepted, so each refusal below is
    // the missing field's and not a fixture that never decoded.
    assert!(
        decode(&attribution()).is_ok(),
        "the fixture is refused whole"
    );
    assert!(
        decode(&observational).is_ok(),
        "the observational fixture is refused whole"
    );
    let cases: [(&str, Value); 5] = [
        ("effect.mean", {
            let mut v = attribution();
            v["effect"].as_object_mut().expect("object").remove("mean");
            v
        }),
        ("effect.std_dev", {
            let mut v = attribution();
            v["effect"]
                .as_object_mut()
                .expect("object")
                .remove("std_dev");
            v
        }),
        ("observations[].horizon_ms", {
            let mut v = attribution();
            v["observations"][0]
                .as_object_mut()
                .expect("object")
                .remove("horizon_ms");
            v
        }),
        ("observations[].value", {
            let mut v = attribution();
            v["observations"][0]
                .as_object_mut()
                .expect("object")
                .remove("value");
            v
        }),
        ("identification.observational.strategy", {
            let mut v = observational.clone();
            v["identification"]["observational"]
                .as_object_mut()
                .expect("object")
                .remove("strategy");
            v
        }),
    ];
    for (field, v) in cases {
        assert!(
            decode(&v).is_err(),
            "an attribution without {field} was accepted"
        );
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
            if [
                "class",
                "action_class",
                "identification",
                "gap_class",
                "reversibility",
            ]
            .contains(&key.as_str())
            {
                continue;
            }
            // A goal's prohibition lists may be declared empty ("none beyond
            // the platform's own");
            // `a_goal_must_declare_its_prohibitions_even_when_it_declares_none`
            // holds them to being declared.
            if name == "goal" && key.starts_with("prohibited_") {
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
fn a_goal_with_no_stop_condition_or_a_zero_horizon_or_budget_is_refused() {
    assert!(
        GoalSpec::decode(&serde_json::to_vec(&goal()).expect("e")).is_ok(),
        "premise: the complete goal decodes"
    );
    let mut g = goal();
    g["stop_conditions"] = json!([]);
    assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
    let mut g = goal();
    g["time_horizon_secs"] = json!(0);
    assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
    let mut g = goal();
    g["budget"] = json!("0");
    assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
}

#[test]
fn a_goal_must_declare_its_prohibitions_even_when_it_declares_none() {
    for key in ["prohibited_side_effects", "prohibited_methods"] {
        let mut g = goal();
        g[key] = json!([]);
        assert!(
            GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_ok(),
            "{key} declared empty is a declaration"
        );
        g[key] = json!(["  "]);
        assert!(GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).is_err());
        g.as_object_mut().expect("object").remove(key);
        let err = GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).expect_err(key);
        assert!(err.to_string().contains(key), "{err}");
    }
}

#[test]
fn a_goal_carries_no_authority_and_a_document_asking_for_one_at_any_level_is_refused() {
    // The goal used to carry an `authority_class` with no live arm. It now
    // carries no authority at all (authority is the action policy's, and that
    // starts at shadow), so a document asking for one is refused rather than
    // having the field quietly ignored — including at the harmless levels,
    // because an ignored field is a field its author believes was read.
    assert!(
        GoalSpec::decode(&serde_json::to_vec(&goal()).expect("e")).is_ok(),
        "premise"
    );
    for asked in ["live", "paper_trading", "shadow"] {
        let mut g = goal();
        g["authority_class"] = json!(asked);
        let err = GoalSpec::decode(&serde_json::to_vec(&g).expect("e")).expect_err(asked);
        assert!(err.to_string().contains("authority_class"), "{err}");
    }
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

/// R12-AGENCY-6104: one attribution carries both halves. Each half was once
/// absent from one of two types, so each is checked here by removing it.
#[test]
fn an_attribution_without_a_causal_chain_or_rival_explanation_or_distinct_horizons_is_refused() {
    let decode = |v: Value| serde_json::from_value::<EffectAttribution>(v).expect("decodes");
    assert!(decode(attribution()).validate().is_ok(), "premise");
    for key in ["causal_chain", "competing_explanations"] {
        let mut v = attribution();
        v[key] = json!([]);
        let err = decode(v).validate().expect_err(key);
        assert!(err.to_string().contains(key), "{err}");
    }
    let mut v = attribution();
    v["observations"] = json!([]);
    assert!(decode(v).validate().is_err());
    let mut v = attribution();
    v["observations"] = json!([
        {"horizon_ms": 1000, "value": 0.4},
        {"horizon_ms": 1000, "value": 0.1}
    ]);
    let err = decode(v).validate().expect_err("one horizon twice");
    assert!(err.to_string().contains("twice"), "{err}");
}

/// R12-AGENCY-6103: correlation is never upgraded to causal certainty by a
/// number written beside it.
#[test]
fn an_unidentified_effect_recorded_with_any_identifiability_above_zero_is_refused() {
    let decode = |v: Value| serde_json::from_value::<EffectAttribution>(v).expect("decodes");
    let mut v = attribution();
    v["identification"] = json!("not_identified");
    v["identifiability"] = json!(0.0);
    assert!(
        decode(v.clone()).validate().is_ok(),
        "premise: a correlation recorded as one is admitted"
    );
    v["identifiability"] = json!(0.05);
    let err = decode(v).validate().expect_err("upgraded correlation");
    assert!(err.to_string().contains("not identified"), "{err}");
    // The same figure under a named strategy is a claim with a basis.
    let mut v = attribution();
    v["identification"] = json!({"observational": {"strategy": "difference in differences"}});
    v["identifiability"] = json!(0.05);
    assert!(decode(v.clone()).validate().is_ok());
    v["identification"] = json!({"observational": {"strategy": " "}});
    assert!(
        decode(v).validate().is_err(),
        "a blank strategy is no strategy"
    );
}

/// CONTRACT-7081, CONTRACT-7085, CONTRACT-7084, CONTRACT-7083: a record that decodes must
/// encode back to the same document, field for field. A field dropped by a
/// `skip_serializing_if`, renamed on one side, or narrowed to an `Option`
/// that serialises as absent survives every refusal test above — they only
/// ever decode — and the record a replay reads back would silently differ
/// from the one the desk wrote.
#[test]
fn every_decoded_record_encodes_back_to_the_same_document_field_for_field() {
    type RoundTrip = fn(&[u8]) -> Value;
    let encoders: Vec<(&str, Value, RoundTrip)> = vec![
        ("goal", goal(), |b| {
            serde_json::to_value(GoalSpec::decode(b).expect("goal decodes")).expect("e")
        }),
        ("plan", plan(), |b| {
            serde_json::to_value(InterventionPlan::decode(b).expect("plan decodes")).expect("e")
        }),
        ("gap", gap(), |b| {
            serde_json::to_value(GapSignal::decode(b).expect("gap decodes")).expect("e")
        }),
        ("item", item(), |b| {
            serde_json::to_value(CurriculumItem::decode(b).expect("item decodes")).expect("e")
        }),
        // CONTRACT-7083's own proof names the round trip beside the refusal,
        // and until 2026-10-24 the attribution was the one record this test
        // left out (AUDIT-5 verification).
        ("attribution", attribution(), |b| {
            serde_json::to_value(
                serde_json::from_slice::<EffectAttribution>(b).expect("attribution decodes"),
            )
            .expect("e")
        }),
        // The experiment fixture never carries `Identification::Observational`,
        // so its `strategy` was the one attribution field no round trip saw.
        (
            "observational attribution",
            observational_attribution(),
            |b| {
                serde_json::to_value(
                    serde_json::from_slice::<EffectAttribution>(b).expect("attribution decodes"),
                )
                .expect("e")
            },
        ),
    ];
    for (name, valid, round_trip) in encoders {
        let fields = valid.as_object().expect("object");
        assert!(
            fields.len() >= 6,
            "{name} premise: the fixture lists its fields"
        );
        let back = round_trip(&serde_json::to_vec(&valid).expect("encodes"));
        let back = back.as_object().expect("encodes as an object");
        let sent: BTreeSet<&String> = fields.keys().collect();
        let got: BTreeSet<&String> = back.keys().collect();
        assert_eq!(sent, got, "{name}: the field set changed on the way back");
        for (key, value) in fields {
            assert_eq!(
                &back[key], value,
                "{name}.{key} did not survive the round trip"
            );
        }
        // Byte-identical once re-encoded: the second trip changes nothing.
        let again = round_trip(&serde_json::to_vec(back).expect("encodes"));
        assert_eq!(
            serde_json::to_vec(&again).expect("e"),
            serde_json::to_vec(back).expect("e"),
            "{name}: a second round trip changed the bytes"
        );
    }
}

/// CONTRACT-7081: the plan's refusal must say which field is missing. A
/// refusal that only says "not an InterventionPlan" is a refusal the planner
/// cannot act on, and the decoder is where the twelve fields are the
/// contract.
#[test]
fn a_plan_missing_any_one_of_its_twelve_fields_is_refused_naming_that_field() {
    let valid = plan();
    let keys: Vec<String> = valid.as_object().expect("object").keys().cloned().collect();
    assert_eq!(
        keys.len(),
        12,
        "premise: the plan fixture carries all twelve fields"
    );
    assert!(
        InterventionPlan::decode(&serde_json::to_vec(&valid).expect("e")).is_ok(),
        "premise: the complete plan decodes"
    );
    for key in keys {
        let mut v = valid.clone();
        v.as_object_mut().expect("object").remove(&key);
        let err = InterventionPlan::decode(&serde_json::to_vec(&v).expect("e"))
            .expect_err(&format!("plan without {key} was accepted"));
        let named = format!("`{key}`");
        assert!(
            err.to_string().contains(&named),
            "plan without {key}: refusal did not name the field: {err}"
        );
    }
}

/// CONTRACT-7084: a gap states what closing it would earn and what leaving
/// it open exposes the book to, as two figures. One figure ranked a liquidity
/// hole worth nothing to close as harmless to leave.
#[test]
fn a_gap_carries_a_risk_value_distinct_from_its_economic_value_and_refuses_a_negative_one() {
    let g: GapSignal = GapSignal::decode(&serde_json::to_vec(&gap()).expect("e")).expect("decodes");
    assert_eq!(g.economic_value.to_string(), "500");
    assert_eq!(g.risk_value.to_string(), "120");
    assert_ne!(
        g.economic_value, g.risk_value,
        "premise: the fixture tells them apart"
    );

    let mut v = gap();
    v["risk_value"] = json!("-1");
    let err = GapSignal::decode(&serde_json::to_vec(&v).expect("e")).expect_err("negative risk");
    assert!(err.to_string().contains("GapSignal.risk_value"), "{err}");

    let mut v = gap();
    v.as_object_mut().expect("object").remove("risk_value");
    let err = GapSignal::decode(&serde_json::to_vec(&v).expect("e")).expect_err("missing risk");
    assert!(err.to_string().contains("`risk_value`"), "{err}");

    let mut v = gap();
    v["risk_value"] = json!("0");
    assert!(
        GapSignal::decode(&serde_json::to_vec(&v).expect("e")).is_ok(),
        "a stated zero exposure is admitted"
    );
}

/// CONTRACT-7080: a goal declares what a stop unwinds, not only when to stop.
/// A goal that halts mid-intervention with no rollback declared leaves its
/// half-applied lever in place, and nobody wrote down that it should not be
/// there. A missing, empty or blank declaration is refused, naming the field.
///
/// Mutation: build `rollback_conditions` with `unwrap_or_default()` instead of
/// `declared_list`. The missing and empty documents are then accepted.
#[test]
fn a_goal_without_rollback_conditions_is_refused_and_a_declared_one_survives_decoding() {
    let g = GoalSpec::decode(&serde_json::to_vec(&goal()).expect("e")).expect("premise: decodes");
    assert_eq!(
        g.rollback_conditions,
        vec!["close the hedge opened for this goal".to_string()]
    );
    let mut missing = goal();
    missing
        .as_object_mut()
        .expect("object")
        .remove("rollback_conditions");
    for (case, doc) in [
        ("missing", missing),
        ("empty", {
            let mut v = goal();
            v["rollback_conditions"] = json!([]);
            v
        }),
        ("blank", {
            let mut v = goal();
            v["rollback_conditions"] = json!(["  "]);
            v
        }),
    ] {
        let err = GoalSpec::decode(&serde_json::to_vec(&doc).expect("e")).expect_err(&format!(
            "a goal with {case} rollback conditions was accepted"
        ));
        assert!(
            err.to_string().contains("`rollback_conditions`"),
            "{case}: refusal did not name the field: {err}"
        );
    }
}
