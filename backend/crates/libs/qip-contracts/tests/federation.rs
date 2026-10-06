use qip_contracts::federation::{
    ConditionalScenario, FederationSnapshot, FederationSurprise, Hypothesis, Outcome,
};
use qip_core::{Decimal, Duration, Timestamp};
use std::collections::BTreeMap;

#[test]
fn a_hypothesis_without_id_model_id_statement_subject_or_evidence_is_refused() {
    let now = Timestamp::from_secs(1000);
    assert!(
        Hypothesis::new(
            "",
            "m1",
            "s",
            "subj",
            5000,
            now,
            Duration::ZERO,
            vec!["e1".into()]
        )
        .is_err()
    );
    assert!(
        Hypothesis::new(
            "h1",
            "",
            "s",
            "subj",
            5000,
            now,
            Duration::ZERO,
            vec!["e1".into()]
        )
        .is_err()
    );
    assert!(
        Hypothesis::new(
            "h1",
            "m1",
            "",
            "subj",
            5000,
            now,
            Duration::ZERO,
            vec!["e1".into()]
        )
        .is_err()
    );
    assert!(
        Hypothesis::new(
            "h1",
            "m1",
            "s",
            "",
            5000,
            now,
            Duration::ZERO,
            vec!["e1".into()]
        )
        .is_err()
    );
    assert!(Hypothesis::new("h1", "m1", "s", "subj", 5000, now, Duration::ZERO, vec![]).is_err());
    assert!(
        Hypothesis::new(
            "h1",
            "m1",
            "s",
            "subj",
            5000,
            now,
            Duration::ZERO,
            vec!["".into()]
        )
        .is_err()
    );
}

#[test]
fn a_hypothesis_with_confidence_above_10000bp_is_refused() {
    assert!(
        Hypothesis::new(
            "h1",
            "m1",
            "s",
            "subj",
            10_001,
            Timestamp::from_secs(1000),
            Duration::ZERO,
            vec!["e1".into()]
        )
        .is_err()
    );
}

#[test]
fn a_valid_hypothesis_is_accepted() {
    let h = Hypothesis::new(
        "h1",
        "model-v1",
        "volatility_increases",
        "EURUSD",
        7500,
        Timestamp::from_secs(1000),
        Duration::from_secs(3600),
        vec!["hist-vol-calc".into(), "regime-detector".into()],
    )
    .expect("valid hypothesis");
    assert_eq!(h.id, "h1");
    assert_eq!(h.model_id, "model-v1");
    assert_eq!(h.confidence_bp, 7500);
    assert_eq!(h.evidence_ids.len(), 2);
}

#[test]
fn a_scenario_without_id_hypothesis_id_or_conditioning_is_refused() {
    let now = Timestamp::from_secs(1000);
    assert!(
        ConditionalScenario::new(
            "",
            "h1",
            BTreeMap::from([("rate".into(), "250bp".into())]),
            Decimal::ZERO,
            now
        )
        .is_err()
    );
    assert!(
        ConditionalScenario::new(
            "s1",
            "",
            BTreeMap::from([("rate".into(), "250bp".into())]),
            Decimal::ZERO,
            now
        )
        .is_err()
    );
    assert!(ConditionalScenario::new("s1", "h1", BTreeMap::new(), Decimal::ZERO, now).is_err());
    assert!(
        ConditionalScenario::new(
            "s1",
            "h1",
            BTreeMap::from([("".into(), "250bp".into())]),
            Decimal::ZERO,
            now
        )
        .is_err()
    );
    assert!(
        ConditionalScenario::new(
            "s1",
            "h1",
            BTreeMap::from([("rate".into(), "".into())]),
            Decimal::ZERO,
            now
        )
        .is_err()
    );
}

#[test]
fn a_valid_scenario_is_accepted() {
    let now = Timestamp::from_secs(1000);
    let cond = BTreeMap::from([
        ("rate_shock_bp".into(), "250".into()),
        ("fx_move_pct".into(), "-2.5".into()),
    ]);
    let s = ConditionalScenario::new("s1", "h1", cond.clone(), Decimal::from(-50_000), now)
        .expect("valid scenario");
    assert_eq!(s.id, "s1");
    assert_eq!(s.hypothesis_id, "h1");
    assert_eq!(s.conditioning, cond);
}

#[test]
fn an_outcome_without_id_observed_state_or_resolution_target_is_refused() {
    let now = Timestamp::from_secs(1000);
    assert!(Outcome::new("", None, None, "state", true, now).is_err());
    assert!(Outcome::new("o1", None, None, "state", true, now).is_err());
    assert!(Outcome::new("o1", None, Some("s1".into()), "", true, now).is_err());
}

#[test]
fn an_outcome_resolving_either_hypothesis_or_scenario_is_accepted() {
    let now = Timestamp::from_secs(1000);
    let o1 = Outcome::new("o1", Some("h1".into()), None, "volatility_broke", true, now)
        .expect("outcome with hypothesis");
    assert_eq!(o1.id, "o1");
    assert!(o1.hypothesis_id.is_some());
    assert!(o1.scenario_id.is_none());

    let o2 = Outcome::new(
        "o2",
        None,
        Some("s1".into()),
        "loss_in_scenario",
        false,
        now,
    )
    .expect("outcome with scenario");
    assert_eq!(o2.id, "o2");
    assert!(o2.hypothesis_id.is_none());
    assert!(o2.scenario_id.is_some());
}

#[test]
fn a_surprise_without_id_models_subject_or_evidence_is_refused() {
    let now = Timestamp::from_secs(1000);
    assert!(
        FederationSurprise::new(
            "",
            vec!["m1".into()],
            "AAPL",
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            now,
            vec!["e1".into()]
        )
        .is_err()
    );
    assert!(
        FederationSurprise::new(
            "s1",
            vec![],
            "AAPL",
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            now,
            vec!["e1".into()]
        )
        .is_err()
    );
    assert!(
        FederationSurprise::new(
            "s1",
            vec!["m1".into()],
            "",
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            now,
            vec!["e1".into()]
        )
        .is_err()
    );
    assert!(
        FederationSurprise::new(
            "s1",
            vec!["m1".into()],
            "AAPL",
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            now,
            vec![]
        )
        .is_err()
    );
}

#[test]
fn a_valid_surprise_is_accepted() {
    let now = Timestamp::from_secs(1000);
    let s = FederationSurprise::new(
        "surp1",
        vec!["model-v1".into(), "causal-dag".into()],
        "EURUSD",
        Decimal::from_int(105) / Decimal::from_int(100),
        Decimal::from_int(100) / Decimal::from_int(100),
        Decimal::from_int(325) / Decimal::from_int(100),
        now,
        vec!["obs-feed".into()],
    )
    .expect("valid surprise");
    assert_eq!(s.id, "surp1");
    assert_eq!(s.model_ids.len(), 2);
    assert_eq!(s.subject, "EURUSD");
}

#[test]
fn federation_snapshot_with_empty_lists_is_accepted() {
    let now = Timestamp::from_secs(1000);
    let snap = FederationSnapshot::new(vec![], vec![], vec![], vec![], now);
    assert_eq!(snap.hypotheses.len(), 0);
    assert_eq!(snap.scenarios.len(), 0);
    assert_eq!(snap.surprises.len(), 0);
    assert_eq!(snap.outcomes.len(), 0);
}

#[test]
fn federation_snapshot_with_mixed_content_round_trips_through_serde() {
    let now = Timestamp::from_secs(1000);
    let h = Hypothesis::new(
        "h1",
        "m1",
        "vol_up",
        "AAPL",
        6000,
        now,
        Duration::from_secs(3600),
        vec!["e1".into()],
    )
    .unwrap();
    let s = ConditionalScenario::new(
        "s1",
        "h1",
        BTreeMap::from([("rate".into(), "250bp".into())]),
        Decimal::from(-1000),
        now,
    )
    .unwrap();
    let surp = FederationSurprise::new(
        "surp1",
        vec!["m1".into()],
        "AAPL",
        Decimal::from_int(102) / Decimal::from_int(100),
        Decimal::from_int(100) / Decimal::from_int(100),
        Decimal::from_int(250) / Decimal::from_int(100),
        now,
        vec!["hist".into()],
    )
    .unwrap();
    let outcome = Outcome::new("o1", Some("h1".into()), None, "confirmed", true, now).unwrap();

    let snap = FederationSnapshot::new(
        vec![h.clone()],
        vec![s.clone()],
        vec![surp.clone()],
        vec![outcome.clone()],
        now,
    );

    let json = serde_json::to_string(&snap).expect("serialize snapshot");
    let restored: FederationSnapshot = serde_json::from_str(&json).expect("deserialize snapshot");

    assert_eq!(restored.hypotheses[0].id, h.id);
    assert_eq!(restored.scenarios[0].id, s.id);
    assert_eq!(restored.surprises[0].id, surp.id);
    assert_eq!(restored.outcomes[0].id, outcome.id);
    assert_eq!(restored.snapshot_at, now);
}
