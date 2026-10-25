//! Every path that widens what the platform lets a model, a tool or an action
//! do is held to a measured-value record.
//!
//! R12-EXPAND-6077. Each widening seam checks its own evidence, and each had a
//! test of its own in its own crate. No single test enumerated the paths, so a
//! fourth seam added without the check, or a check relaxed on one of the
//! three, left every other test green. This suite drives the three existing
//! seams with *empty* evidence and requires each to refuse and to leave the
//! authority where it was.
//!
//! The three seams, and what they require:
//!
//! * a challenger is evaluated only against a classical baseline on record
//!   with a fitted calibration (`qip_kernel::central::models`, ADR 0006);
//! * a tool's scope widens only through a promotion citing its evaluation
//!   (`qip_agents::tools::ToolRegistry::promote`);
//! * an action policy leaves shadow only on an identified effect attribution
//!   and an approver holding the widening authority
//!   (`qip_agency::autonomy::ActionPolicy::widen`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_agency::autonomy::{ActionPolicy, Authority, WIDEN_AUTHORITY};
use qip_agency::plan::ActingIdentity;
use qip_agents::tools::{ToolKind, ToolName, ToolPermission, ToolRegistry};
use qip_ai::registry::ModelRegistry;
use qip_contracts::action::Method;
use qip_contracts::expansion::{Effect, EffectAttribution, HorizonObservation, Identification};
use qip_core::Decimal;
use qip_kernel::central::models::require_calibrated_baseline;
use std::collections::BTreeSet;

fn approver() -> ActingIdentity {
    ActingIdentity {
        name: "desk-approver".to_string(),
        authorities: BTreeSet::from([WIDEN_AUTHORITY.to_string()]),
    }
}

fn policy() -> ActionPolicy {
    ActionPolicy::new(
        Method::Communication,
        Decimal::ONE,
        Decimal::from_int(10),
        BTreeSet::new(),
    )
    .expect("a communication policy with a sane interval is admitted")
}

/// A complete, identified attribution for the policy's class.
fn identified() -> EffectAttribution {
    EffectAttribution {
        goal_id: "g1".into(),
        intervention_id: "i1".into(),
        action_id: "a1".into(),
        action_class: Method::Communication,
        identification: Identification::Experiment,
        observations: vec![HorizonObservation {
            horizon_ms: 60_000,
            value: 3.0,
        }],
        counterfactual_method: "staged rollout control".into(),
        effect: Effect {
            mean: 3.0,
            std_dev: 0.5,
        },
        causal_chain: vec!["quote_size".into(), "spread".into()],
        confounders: vec!["macro_rate".into()],
        competing_explanations: vec!["seasonality".into()],
        side_effects: vec!["none observed".into()],
        identifiability: 1.0,
        resulting_updates: vec!["raise quote-size prior".into()],
    }
}

#[test]
fn every_authority_widening_path_refuses_without_a_measured_value_record() {
    // Seam 1: no baseline on record. The empty registry is the empty evidence.
    let empty = ModelRegistry::new();
    assert!(
        empty.get("baseline@1").is_none(),
        "premise: the registry is empty"
    );
    assert!(
        require_calibrated_baseline(&empty, "baseline@1").is_err(),
        "a candidate was evaluated against a baseline nobody recorded"
    );

    // Seam 2: a blank evaluation. The tool stays read-only afterwards.
    let mut tools = ToolRegistry::new();
    let fetch = ToolName::parse("fetch").unwrap();
    tools
        .register(fetch.clone(), ToolKind::Connector)
        .expect("a tool registers read-only");
    assert!(
        tools.authorise(&fetch, ToolPermission::Read).is_ok(),
        "premise: a new tool holds read"
    );
    assert!(
        tools.authorise(&fetch, ToolPermission::Write).is_err(),
        "premise: a new tool holds no write"
    );
    for blank in ["", "   ", "\n\t"] {
        assert!(
            tools.promote(&fetch, ToolPermission::Write, blank).is_err(),
            "a scope widened on a blank evaluation"
        );
    }
    assert!(
        tools.authorise(&fetch, ToolPermission::Write).is_err(),
        "a refused promotion still widened the scope"
    );

    // Seam 3: an attribution with no observed horizon, then one that is a
    // correlation, then an identified one measuring another class. Each is
    // refused and the policy stays in shadow.
    let mut widening = policy();
    assert_eq!(widening.authority(), Authority::Shadow, "premise: shadow");
    let mut unobserved = identified();
    unobserved.observations.clear();
    let mut correlation = identified();
    correlation.identification = Identification::NotIdentified;
    correlation.identifiability = 0.0;
    let mut other_class = identified();
    other_class.action_class = Method::Capital;
    for bad in [unobserved, correlation, other_class] {
        assert!(
            widening.widen(&bad, &approver()).is_err(),
            "a policy widened on evidence that measures nothing it is held to"
        );
        assert_eq!(widening.authority(), Authority::Shadow);
    }
    let nobody = ActingIdentity {
        name: "agent".to_string(),
        authorities: BTreeSet::new(),
    };
    assert!(
        widening.widen(&identified(), &nobody).is_err(),
        "evidence alone widened a policy; an approver is a precondition"
    );
    assert_eq!(widening.authority(), Authority::Shadow);

    // The admitting half: the same seams do let complete evidence through, so
    // the refusals above are the checks and not a seam that refuses everything.
    assert!(widening.widen(&identified(), &approver()).is_ok());
    assert_eq!(widening.authority(), Authority::NarrowReversible);
    assert!(
        tools
            .promote(
                &fetch,
                ToolPermission::Write,
                "eval-2026-10-18: reads and writes only its own cache"
            )
            .is_ok()
    );
    assert!(tools.authorise(&fetch, ToolPermission::Write).is_ok());
}
