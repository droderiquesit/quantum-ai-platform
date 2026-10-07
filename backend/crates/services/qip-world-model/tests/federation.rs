//! The World Model Federation (blueprint §9.3): declared models, branches with
//! lineage, expiry, calibration and an arbitration that records disagreement.
//!
//! Each test asserts its own premise before the claim, because a filter over
//! an empty list passes for the wrong reason.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::expansion::{Effect, EffectAttribution, HorizonObservation};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_world_model::federation::{
    Abstraction, Assessment, Calibration, CalibrationClaim, Decision, Declaration, Event,
    Federation, MATERIAL_DISAGREEMENT_SPREAD, ModelKind, Scope, Status, WorldModelSpec,
    lineage_from_journal,
};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn scope(
    domain: &str,
    region: &str,
    days: i64,
    abs: Abstraction,
    hyp: &str,
    kind: ModelKind,
) -> Scope {
    Scope {
        domain: domain.into(),
        region: region.into(),
        horizon: Duration::from_days(days),
        abstraction: abs,
        hypothesis: hyp.into(),
        kind,
    }
}

fn full(id: &str, s: Scope, expires: i64) -> Declaration {
    Declaration {
        id: id.into(),
        scope: Some(s),
        evidence_lineage: vec!["feed:rates".into()],
        calibration: Some(CalibrationClaim { max_brier: 0.25 }),
        update_cadence: Some(Duration::from_hours(1)),
        expires_at: Some(at(expires)),
    }
}

fn plain(id: &str, expires: i64) -> Declaration {
    full(
        id,
        scope(
            "macro",
            "global",
            30,
            Abstraction::Macro,
            "base",
            ModelKind::Observational,
        ),
        expires,
    )
}

fn view(prop: &str, p: f64, fit: f64, unc: f64, t: i64) -> Assessment {
    Assessment {
        proposition: prop.into(),
        probability: p,
        evidence_fit: fit,
        causal_coherence: 0.7,
        uncertainty: unc,
        at: at(t),
    }
}

fn register(f: &mut Federation, d: Declaration) -> Result<()> {
    f.register(WorldModelSpec::declare(d)?, at(0))
}

/// Give every named model one resolved, correct forecast (Brier 0.09) so each
/// is scored and none is closed.
fn warm_up(f: &mut Federation, ids: &[&str]) -> Result<()> {
    for id in ids {
        f.assess(id, view("warmup", 0.7, 0.5, 0.5, 1))?;
    }
    f.resolve("warmup", true, at(2));
    Ok(())
}

/// Deterministic generator, so a "generated" test replays identically.
fn lcg(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*state >> 11) as f64 / (1u64 << 53) as f64
}

#[test]
fn a_declaration_missing_any_of_the_five_fields_is_refused_naming_it() -> Result<()> {
    assert!(
        WorldModelSpec::declare(plain("ok", 100)).is_ok(),
        "premise: the complete one is admitted"
    );
    type Strip = fn(&mut Declaration);
    let cases: [(&str, Strip); 5] = [
        ("scope", |d| d.scope = None),
        ("evidence lineage", |d| d.evidence_lineage.clear()),
        ("calibration", |d| d.calibration = None),
        ("update cadence", |d| d.update_cadence = None),
        ("expiry", |d| d.expires_at = None),
    ];
    for (field, strip) in cases {
        let mut d = plain("m", 100);
        strip(&mut d);
        let err = WorldModelSpec::declare(d)
            .err()
            .map(|e| e.message().to_string());
        let msg = err.unwrap_or_default();
        assert!(
            msg.contains(&format!("declares no {field}")),
            "{field}: {msg}"
        );
    }
    Ok(())
}

#[test]
fn a_population_differs_on_all_five_axes_and_every_member_produced_state_this_cycle() -> Result<()>
{
    let mut f = Federation::new();
    let s = [
        (
            "rates",
            scope(
                "rates",
                "us",
                7,
                Abstraction::Micro,
                "h-dovish",
                ModelKind::Observational,
            ),
        ),
        (
            "fx",
            scope(
                "fx",
                "eu",
                30,
                Abstraction::Meso,
                "h-hawkish",
                ModelKind::Observational,
            ),
        ),
        (
            "scen",
            scope(
                "macro",
                "apac",
                90,
                Abstraction::Macro,
                "h-shock",
                ModelKind::Scenario,
            ),
        ),
        (
            "cf",
            scope(
                "macro",
                "global",
                365,
                Abstraction::Macro,
                "h-what-if",
                ModelKind::Counterfactual,
            ),
        ),
    ];
    for (id, sc) in s {
        register(&mut f, full(id, sc, 1_000))?;
    }
    assert!(
        !f.spanned().domains.is_empty(),
        "premise: models are registered"
    );
    assert_eq!(
        f.silent_since(at(10)).len(),
        4,
        "premise: nobody has produced state yet"
    );
    for id in ["rates", "fx", "scen", "cf"] {
        f.assess(id, view("p", 0.5, 0.5, 0.5, 10))?;
    }
    let sp = f.spanned();
    assert!(sp.domains.len() >= 2 && sp.regions.len() >= 2 && sp.horizons.len() >= 2);
    assert!(sp.abstractions.len() >= 2 && sp.hypotheses.len() >= 2);
    assert!(
        sp.kinds.contains(&ModelKind::Scenario) && sp.kinds.contains(&ModelKind::Counterfactual)
    );
    assert!(f.silent_since(at(10)).is_empty());
    Ok(())
}

#[test]
fn arbitration_over_generated_diverging_views_keeps_every_view_and_measures_the_spread()
-> Result<()> {
    let mut seed = 7_u64;
    for round in 0..40 {
        let n = 2 + (round % 4);
        let mut f = Federation::new();
        let ids: Vec<String> = (0..n).map(|i| format!("m{i}")).collect();
        for id in &ids {
            register(&mut f, plain(id, 1_000))?;
        }
        warm_up(&mut f, &ids.iter().map(String::as_str).collect::<Vec<_>>())?;
        let mut ps = Vec::new();
        for id in &ids {
            let p = lcg(&mut seed);
            ps.push(p);
            f.assess(id, view("q", p, lcg(&mut seed), lcg(&mut seed) * 0.7, 5))?;
        }
        let a = f.arbitrate("q", at(6))?;
        let kept: Vec<f64> = a.disagreement.views.iter().map(|v| v.probability).collect();
        assert_eq!(
            kept, ps,
            "round {round}: every member's view survives, minority included"
        );
        let hi = ps.iter().copied().fold(f64::MIN, f64::max);
        let lo = ps.iter().copied().fold(f64::MAX, f64::min);
        assert!((a.disagreement.spread - (hi - lo)).abs() < 1e-12);
        assert!(
            a.disagreement.spread > 0.0,
            "premise: the generated views do diverge"
        );
        assert_eq!(
            a.arbitrated_probability.is_some(),
            a.decision == Decision::Merge
        );
    }
    Ok(())
}

#[test]
fn arbitration_produces_each_of_the_four_actions_with_a_reason_and_the_right_output() -> Result<()>
{
    let mk = |ps: (f64, f64),
              fits: (f64, f64),
              unc: f64|
     -> Result<qip_world_model::federation::Arbitration> {
        let mut f = Federation::new();
        register(&mut f, plain("a", 1_000))?;
        register(&mut f, plain("b", 1_000))?;
        warm_up(&mut f, &["a", "b"])?;
        f.assess("a", view("q", ps.0, fits.0, unc, 5))?;
        f.assess("b", view("q", ps.1, fits.1, unc, 5))?;
        f.arbitrate("q", at(6))
    };
    let merge = mk((0.50, 0.52), (0.6, 0.6), 0.3)?;
    assert_eq!(merge.decision, Decision::Merge);
    assert!(merge.arbitrated_probability.is_some());

    let branch = mk((0.2, 0.8), (0.80, 0.75), 0.3)?;
    assert_eq!(branch.decision, Decision::Branch);
    assert!(branch.arbitrated_probability.is_none());

    let abstain = mk((0.2, 0.8), (0.8, 0.75), 0.9)?;
    assert_eq!(abstain.decision, Decision::Abstain);
    assert!(
        abstain.arbitrated_probability.is_none(),
        "abstaining produces no view"
    );
    assert_eq!(
        abstain.disagreement.views.len(),
        2,
        "but the disagreement is still recorded"
    );

    let mut f = Federation::new();
    register(&mut f, plain("a", 1_000))?;
    register(&mut f, plain("b", 1_000))?;
    warm_up(&mut f, &["a"])?;
    f.assess("a", view("q", 0.2, 0.8, 0.3, 5))?;
    f.assess("b", view("q", 0.8, 0.8, 0.3, 5))?;
    let ask = f.arbitrate("q", at(6))?;
    match &ask.decision {
        Decision::RequestMoreInformation(r) => {
            assert_eq!(r.proposition, "q");
            assert_eq!(
                r.models,
                vec!["b".to_string()],
                "the request names the unscored model"
            );
        }
        other => panic!("expected a request, got {other:?}"),
    }
    for a in [&merge, &branch, &abstain, &ask] {
        assert!(!a.reason.is_empty());
    }
    Ok(())
}

#[test]
fn arbitration_records_all_four_dimensions_and_ranks_by_evidence_fit_when_only_that_differs()
-> Result<()> {
    let mut f = Federation::new();
    register(&mut f, plain("weak", 1_000))?;
    register(&mut f, plain("strong", 1_000))?;
    warm_up(&mut f, &["weak", "strong"])?;
    f.assess("weak", view("q", 0.4, 0.2, 0.3, 5))?;
    f.assess("strong", view("q", 0.6, 0.9, 0.3, 5))?;
    let a = f.arbitrate("q", at(6))?;
    assert_eq!(a.disagreement.views.len(), 2, "premise: both compared");
    for v in &a.disagreement.views {
        assert!(v.predictive_performance.is_some() && v.composite.is_some());
        assert!((v.causal_coherence - 0.7).abs() < 1e-12 && (v.uncertainty - 0.3).abs() < 1e-12);
    }
    let by = |id: &str| {
        a.disagreement
            .views
            .iter()
            .find(|v| v.model == id)
            .and_then(|v| v.composite)
    };
    assert!(by("strong") > by("weak"));
    assert_eq!(
        a.disagreement
            .views
            .iter()
            .map(|v| v.predictive_performance)
            .collect::<Vec<_>>()[0],
        a.disagreement
            .views
            .iter()
            .map(|v| v.predictive_performance)
            .collect::<Vec<_>>()[1],
        "premise: performance is identical, so the ranking is evidence fit alone"
    );
    Ok(())
}

#[test]
fn a_model_scores_its_own_resolved_forecasts_and_reports_unscored_before_any() -> Result<()> {
    let mut f = Federation::new();
    register(&mut f, plain("m", 1_000))?;
    assert_eq!(f.calibration("m")?, Calibration::Unscored);
    for (i, (p, outcome)) in [(0.9, true), (0.8, false), (0.3, false)]
        .into_iter()
        .enumerate()
    {
        let prop = format!("p{i}");
        f.assess("m", view(&prop, p, 0.5, 0.5, 1))?;
        f.resolve(&prop, outcome, at(2));
    }
    let expected = (0.01_f64 + 0.64 + 0.09) / 3.0;
    match f.calibration("m")? {
        Calibration::Brier {
            score,
            resolved,
            within_claim,
        } => {
            assert_eq!(resolved, 3);
            assert!((score - expected).abs() < 1e-12, "{score} vs {expected}");
            assert!(within_claim, "0.25 ceiling vs {score}");
        }
        Calibration::Unscored => panic!("three forecasts resolved"),
    }
    Ok(())
}

#[test]
fn an_expired_model_is_excluded_exactly_when_the_clock_reaches_its_expiry_and_returns_on_renewal()
-> Result<()> {
    let mut seed = 99_u64;
    for _ in 0..30 {
        let expiry = 100 + (lcg(&mut seed) * 1_000.0) as i64;
        let now = 100 + (lcg(&mut seed) * 1_000.0) as i64;
        let mut f = Federation::new();
        register(&mut f, plain("m", expiry))?;
        f.assess("m", view("q", 0.5, 0.5, 0.5, 1))?;
        let a = f.arbitrate("q", at(now))?;
        let included = a.disagreement.views.iter().any(|v| v.model == "m");
        assert_eq!(included, now < expiry, "now={now} expiry={expiry}");
        assert_eq!(a.expired == vec!["m".to_string()], now >= expiry);
        let expiries = f
            .journal()
            .iter()
            .filter(|e| matches!(e, Event::Expired { .. }))
            .count();
        assert_eq!(expiries, usize::from(now >= expiry));
    }
    let mut f = Federation::new();
    register(&mut f, plain("m", 100))?;
    f.assess("m", view("q", 0.5, 0.5, 0.5, 1))?;
    assert_eq!(
        f.arbitrate("q", at(99))?.disagreement.views.len(),
        1,
        "premise: live just before expiry"
    );
    assert!(
        f.arbitrate("q", at(100))?.disagreement.views.is_empty(),
        "expired at the instant itself"
    );
    assert!(
        f.arbitrate("q", at(200))?.disagreement.views.is_empty(),
        "premise: expired"
    );
    assert!(
        f.renew("m", at(150), at(200)).is_err(),
        "a renewal into the past is refused"
    );
    f.renew("m", at(900), at(200))?;
    assert_eq!(f.arbitrate("q", at(201))?.disagreement.views.len(), 1);
    Ok(())
}

#[test]
fn a_branch_records_parent_trigger_and_time_and_the_tree_rebuilds_from_the_journal_alone()
-> Result<()> {
    let mut f = Federation::new();
    register(&mut f, plain("root", 1_000))?;
    f.branch("root", "hawk", "h-hawkish", "CPI surprise 2026-09", at(50))?;
    f.branch("hawk", "hawk-2", "h-hawkish-sticky", "wage print", at(60))?;
    let l = f.lineage("hawk-2")?;
    assert_eq!(l.parents, vec!["hawk".to_string()]);
    assert_eq!(l.trigger.as_deref(), Some("wage print"));
    assert_eq!(l.created_at, at(60));
    assert!(
        f.branch("root", "x", "h", "  ", at(1)).is_err(),
        "a branch without a trigger is refused"
    );
    let rebuilt = lineage_from_journal(f.journal());
    assert_eq!(
        rebuilt.len(),
        3,
        "premise: the journal holds all three nodes"
    );
    for id in ["root", "hawk", "hawk-2"] {
        assert_eq!(rebuilt.get(id), Some(f.lineage(id)?), "{id}");
    }
    Ok(())
}

#[test]
fn a_merged_model_names_both_parents_and_each_parents_history_stays_readable() -> Result<()> {
    let mut f = Federation::new();
    register(&mut f, plain("root", 1_000))?;
    f.branch("root", "a", "h-a", "event one", at(10))?;
    f.branch("root", "b", "h-b", "event two", at(11))?;
    f.assess("a", view("q", 0.4, 0.5, 0.5, 12))?;
    f.merge("a", "b", "ab", "evidence converged", at(20))?;
    assert_eq!(
        f.lineage("ab")?.parents,
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        f.lineage("a")?.parents,
        vec!["root".to_string()],
        "parent history readable"
    );
    assert!(matches!(f.status("a")?, Status::Merged { into, .. } if into == "ab"));
    assert!(
        f.assess("a", view("q", 0.4, 0.5, 0.5, 21)).is_err(),
        "a merged parent is no longer updated"
    );
    assert_eq!(
        lineage_from_journal(f.journal()).get("ab"),
        Some(f.lineage("ab")?)
    );
    Ok(())
}

#[test]
fn competing_branches_stay_live_until_the_outcome_and_the_refuted_one_is_closed_with_its_score()
-> Result<()> {
    let mut f = Federation::new();
    register(&mut f, plain("root", 1_000))?;
    f.branch("root", "up", "h-up", "rally", at(10))?;
    f.branch("root", "down", "h-down", "selloff", at(10))?;
    for t in [20, 30] {
        f.assess("up", view("close", 0.8, 0.5, 0.4, t))?;
        f.assess("down", view("close", 0.2, 0.5, 0.4, t))?;
        assert_eq!(
            f.arbitrate("close", at(t))?.disagreement.views.len(),
            2,
            "both live and updated at {t}"
        );
    }
    let closed = f.resolve("close", true, at(40));
    assert_eq!(closed, vec!["down".to_string()]);
    match f.status("down")? {
        Status::Closed {
            score, proposition, ..
        } => {
            assert!((score - 0.64).abs() < 1e-12);
            assert_eq!(proposition, "close");
        }
        other => panic!("expected closed, got {other:?}"),
    }
    assert_eq!(f.status("up")?, &Status::Live);
    assert!(f.lineage("down").is_ok(), "closed, not deleted");
    assert!(f.assess("down", view("next", 0.5, 0.5, 0.5, 50)).is_err());
    assert!(matches!(
        f.calibration("down")?,
        Calibration::Brier { resolved: 1, .. }
    ));
    Ok(())
}

#[test]
fn a_material_disagreement_raises_exactly_one_research_task_citing_its_record_and_a_small_one_raises_none()
-> Result<()> {
    // WORLD-061. The failure this prevents: a disagreement was recorded and
    // nobody was asked to resolve it, so "the models are far apart on this"
    // stayed a journal line. And the opposite failure: a task per arbitration
    // of a disagreement that persists, burying the question under copies.
    let mut f = Federation::new();
    register(&mut f, plain("a", 1_000))?;
    register(&mut f, plain("b", 1_000))?;
    warm_up(&mut f, &["a", "b"])?;
    // Premise: nothing has been raised, and the threshold sits where the two
    // fixtures below are on opposite sides of it.
    assert!(f.research_tasks().is_empty());
    let (material, small) = (0.3, 0.1);
    assert!(small < MATERIAL_DISAGREEMENT_SPREAD && MATERIAL_DISAGREEMENT_SPREAD <= material);

    // One disagreement above the threshold, on `rates`.
    f.assess("a", view("rates", 0.75, 0.5, 0.4, 10))?;
    f.assess("b", view("rates", 0.75 - material, 0.5, 0.4, 10))?;
    let above = f.arbitrate("rates", at(11))?;
    assert!(
        (above.disagreement.spread - material).abs() < 1e-12,
        "premise"
    );

    // One below it, on `growth`.
    f.assess("a", view("growth", 0.6, 0.5, 0.4, 10))?;
    f.assess("b", view("growth", 0.6 - small, 0.5, 0.4, 10))?;
    let below = f.arbitrate("growth", at(12))?;
    assert!((below.disagreement.spread - small).abs() < 1e-12, "premise");

    // Exactly one task, raised by the first and not by the second.
    assert!(
        below.research.is_none(),
        "a small disagreement raised a task"
    );
    let task = above
        .research
        .expect("a material disagreement raises a task");
    assert_eq!(f.research_tasks(), vec![&task]);
    assert_eq!(task.proposition, "rates");
    assert_eq!(task.models, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(task.raised_at, at(11));

    // The citation resolves, in the journal, to the disagreement record of
    // that arbitration: the same views, not a restatement of them.
    match &f.journal()[task.cites] {
        Event::Decided {
            proposition,
            at: decided_at,
            disagreement,
            ..
        } => {
            assert_eq!(proposition, "rates");
            assert_eq!(*decided_at, at(11));
            assert_eq!(disagreement, &above.disagreement);
            assert_eq!(disagreement.views.len(), 2, "every view is in the record");
        }
        other => panic!("the task cites {other:?}, which is not a disagreement record"),
    }

    // The disagreement persists and is arbitrated again: the question is
    // already open, so no second task.
    let again = f.arbitrate("rates", at(13))?;
    assert!(
        (again.disagreement.spread - material).abs() < 1e-12,
        "premise"
    );
    assert!(
        again.research.is_none(),
        "an open question was raised twice"
    );
    assert_eq!(f.research_tasks().len(), 1);

    // Reality answers it; a later material disagreement under the same name
    // is a new question and is raised.
    f.resolve("rates", true, at(14));
    f.assess("a", view("rates", 0.75, 0.5, 0.4, 15))?;
    f.assess("b", view("rates", 0.75 - material, 0.5, 0.4, 15))?;
    assert!(f.arbitrate("rates", at(16))?.research.is_some());
    assert_eq!(f.research_tasks().len(), 2);
    Ok(())
}

#[test]
fn an_effect_attribution_journals_an_attributed_update_citing_the_attribution_id() -> Result<()> {
    // AGENCY-002. The failure this prevents: an EffectAttribution for a
    // simulated intervention is created but never recorded in the federation's
    // journal, so its updates never feed back into world models and the agency
    // loop is broken.
    let mut f = Federation::new();
    register(&mut f, plain("goal-1", 1_000))?;
    assert_eq!(
        f.journal().len(),
        1,
        "premise: the declaration is journaled"
    );

    // Create an EffectAttribution representing the observed effect of a
    // simulated intervention with proper goal, intervention, and action IDs.
    let attribution = valid_attribution("goal-1");

    let before_len = f.journal().len();
    let result_pos = f.update_from_attribution(&attribution, at(100))?;
    let after_len = f.journal().len();

    // Journal grew by exactly one entry.
    assert_eq!(
        after_len,
        before_len + 1,
        "one attribution update journaled"
    );
    assert_eq!(
        result_pos, before_len,
        "returned position points to the new entry"
    );

    // The new journal entry is AttributionApplied with correct fields.
    match &f.journal()[result_pos] {
        Event::AttributionApplied {
            attribution_id,
            model_id,
            update_kind,
            at: recorded_at,
        } => {
            // Attribution ID is properly formatted as goal-intervention-action.
            assert_eq!(
                attribution_id, "attr-goal-1-intervention-99-action-42",
                "attribution_id cites all three identifiers"
            );

            // Model ID matches the goal being updated.
            assert_eq!(model_id, "goal-1", "update targets the goal model");

            // Update kind encodes the effect statistics.
            assert!(update_kind.contains("effect_mean_0.05"), "mean encoded");
            assert!(
                update_kind.contains("std_0.01"),
                "std_dev encoded (trailing precision preserved)"
            );

            // Timestamp is preserved.
            assert_eq!(*recorded_at, at(100), "timestamp recorded");
        }
        other => panic!("expected AttributionApplied, got {other:?}"),
    }

    Ok(())
}

#[test]
fn an_effect_attribution_with_empty_goal_id_is_refused_naming_the_field() -> Result<()> {
    let mut f = Federation::new();
    register(&mut f, plain("goal-1", 1_000))?;

    let attribution = EffectAttribution {
        goal_id: "".into(),
        intervention_id: "intervention-1".into(),
        action_id: "action-1".into(),
        observations: vec![],
        counterfactual_method: "simulated".into(),
        effect: Effect {
            mean: 0.05,
            std_dev: 0.01,
        },
        confounders: vec![],
        side_effects: vec![],
        identifiability: 0.85,
        resulting_updates: vec![],
    };

    let result = f.update_from_attribution(&attribution, at(100));
    assert!(result.is_err(), "empty goal_id refused");
    let err = result.unwrap_err();
    assert!(
        err.message().to_string().contains("goal"),
        "error names the missing field: {}",
        err.message()
    );
    Ok(())
}

/// An attribution the contract admits: every list names at least one entry,
/// as `EffectAttribution::validate` requires.
fn valid_attribution(goal: &str) -> EffectAttribution {
    EffectAttribution {
        goal_id: goal.into(),
        intervention_id: "intervention-99".into(),
        action_id: "action-42".into(),
        observations: vec![HorizonObservation {
            horizon_ms: 60_000,
            value: 0.047,
        }],
        counterfactual_method: "simulated".into(),
        effect: Effect {
            mean: 0.047,
            std_dev: 0.012,
        },
        confounders: vec!["none identified".into()],
        side_effects: vec!["none observed".into()],
        identifiability: 0.85,
        resulting_updates: vec![format!("{goal} effect prior")],
    }
}

#[test]
fn an_attribution_to_a_model_the_federation_does_not_hold_is_refused_and_not_journalled()
-> Result<()> {
    // The failure this prevents: `model_id` is the attribution's goal id,
    // and nothing looked it up, so an update citing a model that never
    // existed was journalled as an update to it.
    let mut f = Federation::new();
    register(&mut f, plain("goal-1", 1_000))?;
    let before = f.journal().len();
    assert!(
        f.update_from_attribution(&valid_attribution("goal-1"), at(100))
            .is_ok(),
        "premise: the same attribution to a registered model is admitted"
    );
    let admitted = f.journal().len();
    assert_eq!(
        admitted,
        before + 1,
        "premise: the admitted one is journalled"
    );

    let refused = f.update_from_attribution(&valid_attribution("no-such-model"), at(101));
    assert!(
        refused.is_err(),
        "an attribution to an unregistered model is refused"
    );
    assert_eq!(
        f.journal().len(),
        admitted,
        "and nothing is journalled for it"
    );
    Ok(())
}

#[test]
fn an_attribution_the_contract_refuses_is_refused_and_not_journalled() -> Result<()> {
    // The failure this prevents: the method checked only for a blank goal id
    // and journalled attributions with no observation behind them.
    let mut f = Federation::new();
    register(&mut f, plain("goal-1", 1_000))?;
    let mut unobserved = valid_attribution("goal-1");
    unobserved.observations.clear();
    let before = f.journal().len();
    assert!(
        unobserved.validate().is_err(),
        "premise: the contract itself refuses an attribution with no observation"
    );
    assert!(f.update_from_attribution(&unobserved, at(100)).is_err());
    assert_eq!(
        f.journal().len(),
        before,
        "nothing journalled for a refused attribution"
    );
    Ok(())
}
