//! The §27 contracts the register held PARTIAL whose gap is closable inside
//! `qip-contracts`: the typed `RiskEnvelope` (R12-CONTRACT-7067), the single
//! `Episode` (7070), and per-field decode refusals for the existing
//! `BeliefState` (7063) and `ModelPack` (7064), which had none.
//!
//! Mutation for every `*_refuses_*` test here: delete any one guard in the
//! contract's `check` — the matching labelled case is then admitted and the
//! test fails naming the label. For the two per-field tests: add
//! `#[serde(default)]` to any field of `BeliefState` or `ModelPack` — the
//! decode without that field then succeeds and the test fails naming it.

#![allow(clippy::panic_in_result_fn, clippy::expect_used)]

mod common;

use common::{DIGEST_A, each_refused, refuses_missing_unknown_and_foreign_version, round_trips};
use qip_contracts::episode::{
    Counterfactual, DecisionKind, Episode, EpisodeDecision, EpisodeFields, HorizonOutcome,
};
use qip_contracts::policy::{BeliefState, ModelPack, UncertaintyType};
use qip_contracts::risk_envelope::{
    DriverLimit, EmergencyMode, HardLimit, HealthRequirement, HealthSubject, RiskEnvelope,
    RiskEnvelopeFields, ScenarioLimit,
};
use qip_core::error::Result;
use qip_core::{Duration, Timestamp, dec};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
use std::fmt::Debug;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn s(v: &str) -> String {
    v.to_string()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

// ---- RiskEnvelope (7067) -----------------------------------------------

fn envelope() -> RiskEnvelopeFields {
    RiskEnvelopeFields {
        schema_version: 1,
        envelope_id: s("re-1"),
        hard_limits: vec![
            HardLimit {
                metric: s("gross_notional"),
                max: dec!("1000000"),
            },
            HardLimit {
                metric: s("single_name_concentration_bp"),
                max: dec!("2000"),
            },
        ],
        scenario_limits: vec![ScenarioLimit {
            scenario: s("sc-equity-down-10"),
            max_loss: dec!("50000"),
        }],
        driver_limits: vec![DriverLimit {
            driver: s("us-rates"),
            max_exposure: dec!("250000"),
        }],
        health_requirements: vec![
            HealthRequirement {
                subject: HealthSubject::Model,
                id: s("fill-model@3"),
                max_staleness: Duration::from_hours(24),
                min_quality_bp: 6_000,
            },
            HealthRequirement {
                subject: HealthSubject::Data,
                id: s("xnys-itch"),
                max_staleness: Duration::from_secs(5),
                min_quality_bp: 9_900,
            },
        ],
        emergency_mode: EmergencyMode::Normal,
        issued_at: t(0),
        expires_at: t(3_600),
    }
}

#[test]
fn a_risk_envelope_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(envelope())?;
    refuses_missing_unknown_and_foreign_version(envelope());
    round_trips(RiskEnvelopeFields {
        driver_limits: vec![],
        emergency_mode: EmergencyMode::ReduceOnly,
        ..envelope()
    })?;
    let e: RiskEnvelope = RiskEnvelope::new(envelope())?;
    assert_eq!(e.emergency_mode, EmergencyMode::Normal);
    Ok(())
}

#[test]
fn a_risk_envelope_refuses_each_broken_invariant() {
    each_refused(
        &envelope(),
        vec![
            ("blank id", Box::new(|f| f.envelope_id = s(""))),
            ("no hard limit", Box::new(|f| f.hard_limits.clear())),
            (
                "zero hard limit",
                Box::new(|f| f.hard_limits[0].max = dec!("0")),
            ),
            (
                "metric twice",
                Box::new(|f| f.hard_limits[1].metric = s("gross_notional")),
            ),
            (
                "blank metric",
                Box::new(|f| f.hard_limits[0].metric = s("")),
            ),
            ("no scenario limit", Box::new(|f| f.scenario_limits.clear())),
            (
                "negative scenario loss",
                Box::new(|f| f.scenario_limits[0].max_loss = dec!("-1")),
            ),
            (
                "scenario twice",
                Box::new(|f| f.scenario_limits.push(f.scenario_limits[0].clone())),
            ),
            (
                "zero driver exposure",
                Box::new(|f| f.driver_limits[0].max_exposure = dec!("0")),
            ),
            (
                "driver twice",
                Box::new(|f| f.driver_limits.push(f.driver_limits[0].clone())),
            ),
            (
                "no health requirement",
                Box::new(|f| f.health_requirements.clear()),
            ),
            (
                "health subject twice",
                Box::new(|f| f.health_requirements.push(f.health_requirements[0].clone())),
            ),
            (
                "blank health id",
                Box::new(|f| f.health_requirements[0].id = s("")),
            ),
            (
                "zero staleness",
                Box::new(|f| f.health_requirements[0].max_staleness = Duration::ZERO),
            ),
            (
                "quality above one",
                Box::new(|f| f.health_requirements[0].min_quality_bp = 10_001),
            ),
            ("expiry at issue", Box::new(|f| f.expires_at = f.issued_at)),
        ],
    );
}

// ---- Episode (7070) ----------------------------------------------------

fn episode() -> EpisodeFields {
    EpisodeFields {
        schema_version: 1,
        episode_id: s("ep-1"),
        world_state_digest: s(DIGEST_A),
        beliefs: strings(&["belief-7"]),
        decisions: vec![
            EpisodeDecision {
                id: s("d-buy"),
                kind: DecisionKind::Acted,
                reason: s("edge 12 bp net of costs"),
            },
            EpisodeDecision {
                id: s("d-hedge"),
                kind: DecisionKind::Declined,
                reason: s("hedge cost exceeded protected loss"),
            },
        ],
        outcomes: vec![
            HorizonOutcome {
                horizon: Duration::from_mins(5),
                realised: dec!("120.50"),
            },
            HorizonOutcome {
                horizon: Duration::from_hours(1),
                realised: dec!("-40.00"),
            },
        ],
        surprise_bp: 1_800,
        counterfactuals: vec![Counterfactual {
            alternative: s("hedged"),
            estimated_outcome: dec!("15.00"),
            method: s("replay-twin@2"),
        }],
        started_at: t(0),
        ended_at: t(3_600),
    }
}

#[test]
fn an_episode_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(episode())?;
    refuses_missing_unknown_and_foreign_version(episode());
    let e: Episode = Episode::new(episode())?;
    assert_eq!(e.decisions.len(), 2);
    Ok(())
}

#[test]
fn an_episode_refuses_each_broken_invariant_including_one_with_no_counterfactual() {
    each_refused(
        &episode(),
        vec![
            ("blank id", Box::new(|f| f.episode_id = s(""))),
            ("bad digest", Box::new(|f| f.world_state_digest = s("x"))),
            ("no belief", Box::new(|f| f.beliefs.clear())),
            ("no decision", Box::new(|f| f.decisions.clear())),
            (
                "decision twice",
                Box::new(|f| f.decisions[1].id = s("d-buy")),
            ),
            (
                "decline without reason",
                Box::new(|f| f.decisions[1].reason = s("")),
            ),
            ("no outcome", Box::new(|f| f.outcomes.clear())),
            (
                "zero horizon",
                Box::new(|f| f.outcomes[0].horizon = Duration::ZERO),
            ),
            ("horizons out of order", Box::new(|f| f.outcomes.reverse())),
            ("surprise above one", Box::new(|f| f.surprise_bp = 10_001)),
            ("no counterfactual", Box::new(|f| f.counterfactuals.clear())),
            (
                "counterfactual without a method",
                Box::new(|f| f.counterfactuals[0].method = s("")),
            ),
            ("end at start", Box::new(|f| f.ended_at = f.started_at)),
        ],
    );
}

// ---- BeliefState (7063) and ModelPack (7064): per-field decode ---------

/// Every field present round-trips; every field removed in turn is refused
/// on decode, naming the field.
fn every_field_is_required<T>(value: &T) -> Result<()>
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let json = serde_json::to_value(value).expect("serialises");
    let back: T = serde_json::from_value(json.clone()).expect("premise: the intact value decodes");
    assert_eq!(&back, value, "a round trip changed a field");
    let object = json.as_object().expect("an object");
    assert!(object.len() > 1, "premise: there are fields to remove");
    for key in object.keys() {
        let mut missing = object.clone();
        missing.remove(key);
        let refusal = serde_json::from_value::<T>(serde_json::Value::Object(missing))
            .expect_err(&format!("decoded without `{key}`"));
        assert!(
            refusal.to_string().contains(&format!("`{key}`")),
            "the refusal for `{key}` names something else: {refusal}"
        );
    }
    Ok(())
}

#[test]
fn a_belief_state_missing_any_of_its_six_fields_is_refused_on_decode() -> Result<()> {
    let belief = BeliefState {
        proposition: s("ACME mean-reverts intraday"),
        evidence_set: strings(&["ev-1"]),
        causal_path: strings(&["liquidity provision", "inventory pressure"]),
        confidence: 0.7,
        uncertainty_type: UncertaintyType::Epistemic,
        expires_at: 1_760_003_600,
    };
    belief.validate(1_760_000_000)?;
    every_field_is_required(&belief)
}

#[test]
fn a_model_pack_missing_any_of_its_nine_fields_is_refused_on_decode() -> Result<()> {
    let pack = ModelPack {
        artifact_digest: s(DIGEST_A),
        signature: s("sig"),
        features: strings(&["mid-price"]),
        calibration: 0.8,
        allowed_universes: BTreeSet::from([s("XNYS:ACME")]),
        budget_microseconds: 50,
        expires_at: 1_760_003_600,
        rollback_parent: s(""),
        publication: qip_contracts::trust::Publication {
            author: "author".to_string(),
            approver: "approver".to_string(),
            approved_at: Timestamp::from_secs(1_760_000_000),
        },
    };
    pack.validate()?;
    every_field_is_required(&pack)
}
