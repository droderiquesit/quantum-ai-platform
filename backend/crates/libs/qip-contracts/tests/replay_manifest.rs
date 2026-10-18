//! `qip_contracts::replay::{PassMarker, ReplayManifest}` — the reproducibility
//! hash a replay run checks itself against, and the applied readings
//! (journal pressure, the polled halt flag, the region wire) that reach the
//! digest chain without being one of replay's exogenous inputs.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::replay::{
    AppliedReadings, ControlPosition, PassMarker, PressureRecord, ReplayAssumptions,
    ReplayManifest, WireRecord,
};
use qip_core::EventId;
use qip_core::error::Result;
use std::collections::BTreeMap;

fn marker() -> PassMarker {
    PassMarker {
        cell: "nynj".to_string(),
        session: "session-2026-09-25T00".to_string(),
        pass: 7,
        now_ns: 1_760_000_000_000_000_000,
        tape_digest: "tape-digest-abc".to_string(),
        tape_from: 100,
        tape_to: 140,
        control: vec![ControlPosition::new(
            "policy",
            0,
            12,
            EventId::from_string("evt-00000000000000000000000001"),
        )],
        readings: AppliedReadings::default(),
        config_digest: "config-digest-1".to_string(),
        plan_digest: "plan-digest-1".to_string(),
        gateway_seed: 42,
        binary_version: "qip-edge-node@sha256:deadbeef".to_string(),
    }
}

#[test]
fn a_replay_manifest_hash_changes_when_any_input_seed_or_digest_changes() -> Result<()> {
    let base = marker();
    let base_manifest = ReplayManifest::new(base.clone())?;

    // Premise: two manifests built from an *independently cloned* but
    // identical marker agree, so a difference asserted below is attributable
    // to the field that changed rather than to nondeterminism in the hash
    // itself.
    let repeat = ReplayManifest::new(base.clone())?;
    assert_eq!(
        repeat.reproducibility_hash(),
        base_manifest.reproducibility_hash(),
        "hashing an identical marker twice gave two different answers"
    );

    let mut different_seed = base.clone();
    different_seed.gateway_seed += 1;
    assert_ne!(
        ReplayManifest::new(different_seed)?.reproducibility_hash(),
        base_manifest.reproducibility_hash(),
        "changing gateway_seed did not change the reproducibility hash"
    );

    let mut different_tape = base.clone();
    different_tape.tape_digest = "a-different-tape-digest".to_string();
    assert_ne!(
        ReplayManifest::new(different_tape)?.reproducibility_hash(),
        base_manifest.reproducibility_hash(),
        "changing tape_digest did not change the reproducibility hash"
    );

    let mut different_config = base.clone();
    different_config.config_digest = "a-different-config-digest".to_string();
    assert_ne!(
        ReplayManifest::new(different_config)?.reproducibility_hash(),
        base_manifest.reproducibility_hash(),
        "changing config_digest did not change the reproducibility hash"
    );

    let mut different_plan = base;
    different_plan.plan_digest = "a-different-plan-digest".to_string();
    assert_ne!(
        ReplayManifest::new(different_plan)?.reproducibility_hash(),
        base_manifest.reproducibility_hash(),
        "changing plan_digest did not change the reproducibility hash"
    );
    Ok(())
}

#[test]
fn a_pass_marker_hash_changes_when_an_applied_pressure_or_wire_reading_changes() -> Result<()> {
    let base = marker();
    let base_hash = ReplayManifest::hash_of(&base)?;

    // Premise: hashing the same marker twice agrees, so a difference below is
    // attributable to the reading that changed.
    assert_eq!(
        ReplayManifest::hash_of(&base)?,
        base_hash,
        "hashing an identical marker twice gave two different answers"
    );
    assert!(
        base.readings == AppliedReadings::default(),
        "premise: the baseline marker carries no readings, so the changes \
         below are additions rather than edits to an existing value"
    );

    let mut journal_changed = base.clone();
    journal_changed.readings.journal_pressure =
        Some(PressureRecord::new("red", "queue depth 900k events"));
    assert_ne!(
        ReplayManifest::hash_of(&journal_changed)?,
        base_hash,
        "an applied journal-pressure reading did not move the reproducibility hash"
    );

    let mut halt_changed = base.clone();
    halt_changed.readings.halt_flag =
        Some(WireRecord::new("engaged", "operator halt file present"));
    assert_ne!(
        ReplayManifest::hash_of(&halt_changed)?,
        base_hash,
        "an applied polled-halt reading did not move the reproducibility hash"
    );

    let mut region_changed = base;
    region_changed.readings.region_wire = Some(WireRecord::new("narrowed", "share capped at 0.4"));
    assert_ne!(
        ReplayManifest::hash_of(&region_changed)?,
        base_hash,
        "an applied region-wire reading did not move the reproducibility hash"
    );
    Ok(())
}

fn assumptions() -> ReplayAssumptions {
    ReplayAssumptions {
        clock: "the pass clock reading".to_string(),
        latency: "none: every offset is zero".to_string(),
        queue: "price-time priority on the simulated book".to_string(),
        fill: "the matching engine's own fills at the resting price".to_string(),
        impact: "none modelled".to_string(),
    }
}

fn versions() -> BTreeMap<String, String> {
    BTreeMap::from([("fill".to_string(), "sim-exchange-1".to_string())])
}

/// Mutation: make `hash_with` ignore `assumptions` (hash the marker alone) —
/// two runs under different fill assumptions share a reproducibility hash.
#[test]
fn a_manifest_states_its_assumptions_and_model_versions_and_a_change_to_either_changes_the_hash()
-> Result<()> {
    let bare = ReplayManifest::new(marker())?;
    assert!(
        bare.assumptions().is_none(),
        "premise: a bare manifest states none"
    );
    let text = serde_json::to_string(&bare)?;
    assert!(
        !text.contains("assumptions") && !text.contains("model_versions"),
        "a manifest that states none must encode as it always did"
    );

    let stated = ReplayManifest::with_assumptions(marker(), assumptions(), versions())?;
    assert!(stated.is_consistent()?);
    assert_ne!(
        stated.reproducibility_hash(),
        bare.reproducibility_hash(),
        "stating assumptions must be part of what is reproduced"
    );
    let mut other_fill = assumptions();
    other_fill.fill = "every order fills in full at the touch".to_string();
    assert_ne!(
        ReplayManifest::with_assumptions(marker(), other_fill, versions())?.reproducibility_hash(),
        stated.reproducibility_hash()
    );
    let mut other_versions = versions();
    other_versions.insert("fill".to_string(), "sim-exchange-2".to_string());
    assert_ne!(
        ReplayManifest::with_assumptions(marker(), assumptions(), other_versions)?
            .reproducibility_hash(),
        stated.reproducibility_hash()
    );

    let back: ReplayManifest = serde_json::from_str(&serde_json::to_string(&stated)?)?;
    assert_eq!(back, stated);
    assert_eq!(back.assumptions(), Some(&assumptions()));
    assert_eq!(back.model_versions(), &versions());
    assert!(back.is_consistent()?);
    // Altering an assumption after the hash was taken is caught.
    let altered = serde_json::to_string(&stated)?.replace("none modelled", "linear impact");
    assert!(!serde_json::from_str::<ReplayManifest>(&altered)?.is_consistent()?);
    Ok(())
}

#[test]
fn a_blank_assumption_or_a_missing_model_version_is_refused_naming_what_is_missing() {
    // Premise: the complete statement is accepted.
    assert!(ReplayManifest::with_assumptions(marker(), assumptions(), versions()).is_ok());
    type Edit = fn(&mut ReplayAssumptions);
    let edits: [(&str, Edit); 5] = [
        ("clock", |a| a.clock = " ".to_string()),
        ("latency", |a| a.latency.clear()),
        ("queue", |a| a.queue.clear()),
        ("fill", |a| a.fill.clear()),
        ("impact", |a| a.impact.clear()),
    ];
    for (name, blank) in edits {
        let mut broken = assumptions();
        blank(&mut broken);
        let refused = ReplayManifest::with_assumptions(marker(), broken, versions())
            .expect_err("a blank assumption must be refused");
        assert!(
            refused.message().contains(&format!("no {name} assumption")),
            "{name}: {}",
            refused.message()
        );
    }
    assert!(
        ReplayManifest::with_assumptions(marker(), assumptions(), BTreeMap::new()).is_err(),
        "no model versions stated"
    );
    let blank_version = BTreeMap::from([("fill".to_string(), String::new())]);
    assert!(ReplayManifest::with_assumptions(marker(), assumptions(), blank_version).is_err());
}
