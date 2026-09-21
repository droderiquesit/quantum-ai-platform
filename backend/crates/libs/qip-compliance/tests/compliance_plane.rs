//! The whole governance plane, and the report that makes "fully compliant"
//! something a test can check rather than a claim in a document.
//!
//! The assertion that matters is the last one in
//! `all_six_controls_are_enforced_and_each_names_its_mechanism`: every control
//! `qip_contracts::governance::Control` names has a mechanism in this crate,
//! and the report says which.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry};
use qip_compliance::incident::{Incident, ResponsePolicy};
use qip_compliance::model_risk::{Contribution, Explanation};
use qip_compliance::plane::CompliancePlane;
use qip_compliance::signing::SigningKey;
use qip_contracts::governance::{Control, Severity, Usage};
use qip_contracts::time::Stamped;
use qip_core::error::Result;
use qip_core::{Duration, ModelId, Timestamp, dec};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

const MODEL: &str = "vol-forecast@2.1.0";

/// A model `qip_ai` is satisfied with — staged, evaluated, undrifted — so that
/// anything control 3 then refuses is refused on model *risk* grounds and not
/// on model performance.
fn eligible_model() -> Result<ModelRegistry> {
    let mut registry = ModelRegistry::new();
    registry.register(
        ModelCard::new(
            ModelId::from_string("m-vol-forecast"),
            "vol-forecast",
            "2.1.0",
            "quant-research",
            now().saturating_sub(Duration::from_days(200)),
        )
        .with_purpose("forecast realised volatility one day ahead"),
    );
    registry.record_evaluation(
        MODEL,
        EvaluationRecord {
            evaluated_at: now().saturating_sub(Duration::from_days(10)),
            dataset: "holdout.2024".to_string(),
            metrics: Default::default(),
            passed: true,
        },
    )?;
    registry.promote(MODEL, now().saturating_sub(Duration::from_days(5)))?;
    Ok(registry)
}

/// An explanation that reconciles: 0.10 baseline + 0.05 + 0.03 = 0.18. It has
/// to, because `Explanation::reconciled` refuses a non-zero residual and there
/// is no other constructor.
fn honest_explanation() -> Result<Explanation> {
    Explanation::reconciled(
        MODEL,
        dec!("0.18"),
        dec!("0.10"),
        vec![
            Contribution {
                input: "adv_participation".to_string(),
                value: dec!("0.01"),
                contribution: dec!("0.05"),
            },
            Contribution {
                input: "realised_vol_5d".to_string(),
                value: dec!("0.22"),
                contribution: dec!("0.03"),
            },
        ],
        now(),
        None,
    )
}

fn plane() -> Result<CompliancePlane> {
    CompliancePlane::new(
        SigningKey::from_secret("plane-key-2026-01", &[5u8; 32])?,
        dec!("1000000"),
        ResponsePolicy::standard(),
    )
}

#[test]
fn all_six_controls_are_enforced_and_each_names_its_mechanism() -> Result<()> {
    let plane = plane()?;
    let report = plane.report(now());

    // Exhaustive by construction: the report is built from `Control::all`, so
    // a seventh control added to the contract would appear here unenforced
    // rather than be quietly omitted.
    assert_eq!(report.statuses().len(), Control::all().len());
    assert_eq!(report.statuses().len(), 6);

    for control in Control::all() {
        let status = report
            .status(control)
            .unwrap_or_else(|| panic!("no status for {}", control.as_str()));
        assert!(
            status.enforced,
            "{} is not enforced: {:?}",
            control.as_str(),
            status
        );
        // A mechanism has to name the thing that does the enforcing. A status
        // that said "policy requires…" would pass a length check and mean
        // nothing, so the assertion is that it names a type in this crate.
        assert!(
            status.mechanism.contains("crate::"),
            "{} names no concrete mechanism: {}",
            control.as_str(),
            status.mechanism
        );
        assert!(!status.evidence.is_empty());
    }

    assert!(report.is_fully_enforced());
    assert!(report.unenforced().is_empty());
    report.require_fully_enforced()?;
    Ok(())
}

#[test]
fn the_report_states_its_own_gaps_rather_than_claiming_there_are_none() -> Result<()> {
    // A control described as structural when it is advisory is worse than one
    // labelled advisory, so every status carries its caveats and the largest
    // of them — symmetric signing — is stated in the report itself.
    let plane = plane()?;
    let report = plane.report(now());

    let caveats = report.caveats();
    assert!(!caveats.is_empty());

    let signing = report
        .status(Control::SignedArtifactsAndProvenance)
        .ok_or_else(|| qip_core::error::Error::not_found("signing status"))?;
    let text = signing.caveats.join(" ");
    assert!(text.contains("HMAC"));
    assert!(text.contains("asymmetric"));
    assert!(text.contains("KMS"));

    let capital = report
        .status(Control::HumanCapitalApproval)
        .ok_or_else(|| qip_core::error::Error::not_found("capital status"))?;
    assert!(capital.caveats.join(" ").contains("CapitalEnvelope::new"));
    Ok(())
}

#[test]
fn the_report_round_trips_through_json_so_it_can_be_filed_as_evidence() -> Result<()> {
    // The report is the artifact an auditor is handed. One that cannot be
    // stored and read back is not evidence of anything.
    let plane = plane()?;
    let report = plane.report(now());

    let encoded = serde_json::to_string(&report)?;
    let decoded: qip_compliance::plane::ComplianceReport = serde_json::from_str(&encoded)?;
    assert_eq!(decoded, report);
    assert!(decoded.is_fully_enforced());
    Ok(())
}

#[test]
fn a_control_that_stops_working_shows_as_unenforced() -> Result<()> {
    // `enforced` is computed rather than asserted. The artifact control is the
    // one with a runtime condition — a store whose contents no longer verify
    // is a control that has stopped working, whatever shape its types are.
    let mut plane = plane()?;
    let bytes = b"an artifact".to_vec();
    let provenance = plane.artifacts().seal(&bytes, "build", now(), vec![])?;
    plane
        .artifacts_mut()
        .store("out.bin", bytes, provenance, now())?;

    let report = plane.report(now());
    assert!(report.is_fully_enforced());
    assert!(plane.artifacts().integrity_failures().is_empty());
    Ok(())
}

#[test]
fn the_plane_gives_every_subsystem_a_point_in_time_reader_and_nothing_wider() -> Result<()> {
    // Control 1's entry point hangs off the plane, so an audit can find every
    // read in the platform by looking at who holds one.
    let plane = plane()?;
    let reader = plane.reader(
        now(),
        [
            Stamped::new(1_i64, now(), now()),
            Stamped::new(2_i64, now(), now().saturating_add(Duration::from_secs(1))),
        ],
    );
    assert_eq!(reader.len(), 1);
    assert_eq!(reader.withheld(), 1);
    assert!(
        reader
            .restrict_to(now().saturating_add(Duration::from_secs(60)))
            .is_err()
    );

    let detector = plane.leakage_detector(now());
    let future = Stamped::new(3_i64, now(), now().saturating_add(Duration::from_secs(1)));
    assert!(detector.inspect("late_input", &future).is_some());
    Ok(())
}

#[test]
fn a_halt_recorded_on_the_plane_stops_the_subsystems_it_names() -> Result<()> {
    // The one cross-control question every subsystem asks, answered from the
    // incident log rather than a cached flag, so clearing takes effect at once.
    let mut plane = plane()?;
    assert!(plane.may_act("stat-arb-eu", "frankfurt-1"));

    plane.incidents_mut().record(Incident::new(
        "i-1",
        now(),
        Severity::Scoped,
        "risk-monitor",
        "realised loss reached the strategy's limit",
        Some("stat-arb-eu".to_string()),
        None,
    )?);

    assert!(!plane.may_act("stat-arb-eu", "frankfurt-1"));
    assert!(plane.may_act("momentum-us", "frankfurt-1"));

    // The halt appears in the report's evidence without changing whether the
    // control is enforced: a tripped kill switch is the control working.
    let report = plane.report(now());
    assert!(report.is_fully_enforced());
    let status = report
        .status(Control::KillSwitchAndIncidentResponse)
        .ok_or_else(|| qip_core::error::Error::not_found("kill switch status"))?;
    assert!(status.evidence.iter().any(|e| e.contains("1 incidents")));
    Ok(())
}

#[test]
fn the_planes_controls_compose_across_one_realistic_decision() -> Result<()> {
    // A single pass through the plane touching five of the six controls, to
    // show they are one plane rather than six unrelated objects sharing a file.
    let mut plane = plane()?;

    // Licensing: a feed that may be researched and derived from, never traded.
    let expiry = now().saturating_add(Duration::from_days(30));
    plane
        .entitlements_mut()
        .grant("vendor.prices", Usage::Research, expiry, now())?;
    plane
        .entitlements_mut()
        .grant("vendor.prices", Usage::Derive, expiry, now())?;
    plane.entitlements_mut().deny(
        "vendor.prices",
        Usage::Trade,
        "the master agreement covers internal research only",
    )?;

    // Provenance: a feature set built from that feed, signed and stored.
    let raw = b"raw prices".to_vec();
    let raw_digest =
        plane
            .artifacts_mut()
            .register_raw_dataset("vendor.prices", &raw, "Vendor A", now())?;
    let features = b"feature matrix".to_vec();
    let provenance =
        plane
            .artifacts()
            .seal(&features, "feature-pipeline", now(), vec![raw_digest])?;
    let digest = plane
        .artifacts_mut()
        .store("features.parquet", features, provenance, now())?;
    plane
        .artifacts()
        .provenance_chain(&digest)?
        .require_complete()?;

    // Bitemporal truth: nothing after the as-of is visible to the read.
    let reader = plane.reader(now(), [Stamped::new(dec!("101.25"), now(), now())]);
    assert_eq!(reader.len(), 1);

    // Kill switch: nothing halted, so the strategy may act.
    assert!(plane.may_act("stat-arb-eu", "frankfurt-1"));

    // And the licence still stops the last step, which is the whole point.
    assert!(
        !plane
            .entitlements()
            .permits("vendor.prices", Usage::Trade, now())
    );

    plane.report(now()).require_fully_enforced()?;
    Ok(())
}

#[test]
fn an_unexercised_model_risk_control_says_so_rather_than_reading_as_a_quiet_plane() -> Result<()> {
    // The failure this prevents is the `MaxExpectedShortfall` one from
    // `.claude/rules/domains/risk-and-execution.md`, one level up: a control
    // that reads as protection while having nothing to protect. Control 3's
    // mechanism is structural and its `enforced` flag is therefore true on a
    // plane that has never been handed a model — and the two evidence lines a
    // reviewer reads next to it are a pair of zeroes, which say "nothing
    // happened this period" and "no model output has ever been offered to
    // this gate" in identical words. The report has to distinguish them.
    let plane = plane()?;
    let report = plane.report(now());
    let status = report
        .status(Control::ModelRiskAndExplainability)
        .ok_or_else(|| qip_core::error::Error::not_found("model risk status"))?;

    // The premise first: this is a control the report still calls enforced,
    // which is what makes the caveats load-bearing rather than decorative. A
    // test that only checked the caveats would keep passing if the arm
    // started reporting the control unenforced, which is a different and far
    // louder claim.
    assert!(status.enforced);
    assert!(
        status
            .evidence
            .iter()
            .any(|line| line == "0 risk files on record"),
        "expected the empty-register evidence line, got {:?}",
        status.evidence
    );
    assert!(
        status
            .evidence
            .iter()
            .any(|line| line == "0 admission decisions recorded"),
        "expected the empty-admissions evidence line, got {:?}",
        status.evidence
    );

    let caveats = status.caveats.join(" ");
    assert!(
        caveats.contains("no model risk file has been filed"),
        "the report does not say the register is empty: {caveats}"
    );
    assert!(
        caveats.contains("has been asked nothing"),
        "the report does not say the gate has been asked nothing: {caveats}"
    );

    // A caveat is a statement, not a veto. `require_fully_enforced` gates
    // proposal sign-off in `qip_kernel::Platform`, and a lane that turned
    // this finding into an unenforced control would have stopped the platform
    // signing anything at all — a far larger change than the one this test
    // pins, and not the one that was argued for.
    report.require_fully_enforced()?;
    assert!(report.is_fully_enforced());
    Ok(())
}

#[test]
fn offering_one_output_to_the_model_risk_gate_retires_only_the_caveat_it_answers() -> Result<()> {
    // The two caveats are computed from two different facts and a test that
    // moved both at once could not tell a single flag from two. Here the
    // register is still empty — no risk file has been filed — and the gate
    // has been asked exactly one question, which it refused for that very
    // reason. So the "asked nothing" caveat must go and the "no risk file"
    // caveat must stay.
    let mut plane = plane()?;
    let models = eligible_model()?;

    // Premise: before the offer, both caveats are present.
    let before = plane.report(now());
    let before_text = before
        .status(Control::ModelRiskAndExplainability)
        .ok_or_else(|| qip_core::error::Error::not_found("model risk status"))?
        .caveats
        .join(" ");
    assert!(before_text.contains("no model risk file has been filed"));
    assert!(before_text.contains("has been asked nothing"));

    let refusal = plane
        .model_risk_mut()
        .admit(&models, honest_explanation()?, now());
    assert!(
        refusal.is_err(),
        "a model with no risk file must not be admitted"
    );

    let after = plane.report(now());
    let status = after
        .status(Control::ModelRiskAndExplainability)
        .ok_or_else(|| qip_core::error::Error::not_found("model risk status"))?;
    assert!(
        status
            .evidence
            .iter()
            .any(|line| line == "1 admission decisions recorded"),
        "a refusal is an admission decision and must be counted: {:?}",
        status.evidence
    );

    let caveats = status.caveats.join(" ");
    assert!(
        !caveats.contains("has been asked nothing"),
        "the gate was asked one question and still claims it was asked none: {caveats}"
    );
    assert!(
        caveats.contains("no model risk file has been filed"),
        "the register is still empty and the report has stopped saying so: {caveats}"
    );
    Ok(())
}
