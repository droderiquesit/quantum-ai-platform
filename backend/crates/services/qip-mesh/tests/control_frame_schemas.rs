//! The three P0 control frames register in one schema registry without
//! colliding.
//!
//! This check ran inside `qip-api`'s start-up until 2026-10-07. To do it, the
//! deployed API process built a `CapitalEnvelope` and signed a policy payload
//! and a halt under a key it chose itself (`b"schema-registration-key"`), and
//! then discarded the registry. That broke `api_boundary`'s two rules: the
//! application layer constructs no envelope and signs nothing but the centre's
//! policy and halt. The check depends on nothing at run time, because the
//! frames' topics and shapes are fixed at compile time. Run here, CI catches
//! the same mistakes, and no deployed binary holds a signing key it invented.
//!
//! The failure it prevents: two control frames claiming one topic, or a frame
//! whose serialised shape changes under an unchanged SCHEMA_VERSION. Either
//! one is refused by `SchemaRegistry::admit`.
// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{
    AdversaryProfiles, BeliefPriors, CausalDigest, CycleWhitelist, Dispositions, EpisodicDigest,
    FeasibilityConstraints, GrantManifest, HaltCommand, InventoryTargets, ModelManifest,
    PlanDigest, PolicyPayload, RegimeState, RiskEnvelopeSnapshot, Slot,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Timestamp, dec};
use qip_events::{EventBody, SchemaRegistry};
use qip_mesh::spine::{CapitalGrantFrame, HaltFrame, PolicyFrame};
use std::collections::BTreeMap;

#[test]
fn the_three_p0_control_frames_each_register_under_their_own_topic() -> qip_core::error::Result<()>
{
    let mut registry = SchemaRegistry::new();

    // A test fixture key. It signs nothing that leaves this test.
    let now = Timestamp::from_secs(1_760_000_000);
    const KEY: &[u8] = b"schema-registration-test-fixture";

    // Register CapitalGrantFrame: a capital allocation to a cell strategy
    let capital_envelope = CapitalEnvelope::new(
        StrategyId::new("strat-1"),
        "cell",
        dec!("1000000"),
        dec!("100000"),
        dec!("50000"),
        vec![VenueId::new("XNYS")],
        now,
        Timestamp::from_secs(1_760_003_600),
        "op@example.com",
        "sig",
    )?;
    registry.register(&CapitalGrantFrame(capital_envelope))?;

    // Register PolicyFrame: the full policy payload with all slots populated
    let mut policy_payload = PolicyPayload::unproduced(1, "cell", now);
    policy_payload.valid_for = qip_core::Duration::from_secs(300);
    policy_payload.halted = false;

    // Populate all 13 required slots
    policy_payload.trained_models = Slot::produced(
        ModelManifest {
            models: BTreeMap::from([("m1".to_string(), "d1".to_string())]),
        },
        now,
    );
    policy_payload.compiled_plan = Slot::produced(
        PlanDigest {
            digest: "d1".to_string(),
            strategies: 1,
        },
        now,
    );
    policy_payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::from([("AAPL".to_string(), 0.6)]),
        },
        now,
    );
    policy_payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d1".to_string(),
            episodes: 1,
        },
        now,
    );
    policy_payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: vec!["e1".to_string()],
        },
        now,
    );
    policy_payload.regime_state = Slot::produced(
        RegimeState {
            regime: "risk_on".to_string(),
            confidence: 0.8,
        },
        now,
    );
    policy_payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec!["sig-1".to_string()],
        },
        now,
    );
    policy_payload.cycle_whitelist = Slot::produced(
        CycleWhitelist {
            cycles: BTreeMap::from([("c1".to_string(), "p1".to_string())]),
            conversions: vec![],
            start_sizes: BTreeMap::from([("USD".to_string(), dec!("1000000"))]),
        },
        now,
    );
    policy_payload.risk_envelope = Slot::produced(
        RiskEnvelopeSnapshot {
            limits: serde_json::json!({"max_gross": "1000000"}),
        },
        now,
    );
    policy_payload.inventory_targets = Slot::produced(
        InventoryTargets {
            targets: BTreeMap::from([("AAPL".to_string(), dec!("100"))]),
            reference_prices: BTreeMap::from([("AAPL".to_string(), dec!("150"))]),
        },
        now,
    );
    policy_payload.feasibility_constraints = Slot::produced(
        FeasibilityConstraints {
            minimum_order: BTreeMap::from([("XNYS".to_string(), dec!("1"))]),
            fee_floor: BTreeMap::from([("XNYS".to_string(), dec!("0.001"))]),
            tick: BTreeMap::from([("XNYS".to_string(), dec!("0.01"))]),
            withdrawn_venues: std::collections::BTreeSet::new(),
            dark_regions: std::collections::BTreeSet::new(),
        },
        now,
    );
    policy_payload.adversary_profiles = Slot::produced(
        AdversaryProfiles {
            venues: BTreeMap::from([(
                "XNYS".to_string(),
                serde_json::json!({"posture": "defensive"}),
            )]),
        },
        now,
    );
    policy_payload.dispositions = Slot::produced(
        Dispositions {
            unwinds: BTreeMap::from([(
                StrategyId::new("strat-1"),
                BTreeMap::from([("AAPL".to_string(), dec!("-10"))]),
            )]),
        },
        now,
    );

    let signed_policy = policy_payload.signed(KEY)?;
    registry.register(&PolicyFrame(signed_policy))?;

    // Register HaltFrame: a halt command signed by the operator's key
    let halt_cmd = HaltCommand::new("cell", now, "sample halt").signed(KEY)?;
    registry.register(&HaltFrame(halt_cmd))?;

    // Premise: each registration was admitted, so the registry holds one
    // descriptor per frame. A collision would have returned Err above.
    assert_eq!(
        registry.len(),
        3,
        "expected exactly the three P0 control frames"
    );
    for (topic, type_name) in [
        (
            CapitalGrantFrame::TOPIC,
            std::any::type_name::<CapitalGrantFrame>(),
        ),
        (PolicyFrame::TOPIC, std::any::type_name::<PolicyFrame>()),
        (HaltFrame::TOPIC, std::any::type_name::<HaltFrame>()),
    ] {
        let descriptor = registry
            .get(topic)
            .unwrap_or_else(|| panic!("{topic} has no registered schema"));
        assert_eq!(
            descriptor.type_name, type_name,
            "{topic} is claimed by the wrong frame"
        );
    }
    Ok(())
}
