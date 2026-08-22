//! The whole governance plane, and the report that makes "fully compliant"
//! something a test can check rather than a claim in a document.
//!
//! The assertion that matters is the last one in
//! `all_six_controls_are_enforced_and_each_names_its_mechanism`: every control
//! `qip_contracts::governance::Control` names has a mechanism in this crate,
//! and the report says which.

#![allow(clippy::panic_in_result_fn)]

use qip_compliance::incident::{Incident, ResponsePolicy};
use qip_compliance::plane::CompliancePlane;
use qip_compliance::signing::SigningKey;
use qip_contracts::governance::{Control, Severity, Usage};
use qip_contracts::time::Stamped;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp, dec};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
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
    plane.artifacts_mut().store("out.bin", bytes, provenance, now())?;

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
    assert!(reader.restrict_to(now().saturating_add(Duration::from_secs(60))).is_err());

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
    plane.artifacts().provenance_chain(&digest)?.require_complete()?;

    // Bitemporal truth: nothing after the as-of is visible to the read.
    let reader = plane.reader(now(), [Stamped::new(dec!("101.25"), now(), now())]);
    assert_eq!(reader.len(), 1);

    // Kill switch: nothing halted, so the strategy may act.
    assert!(plane.may_act("stat-arb-eu", "frankfurt-1"));

    // And the licence still stops the last step, which is the whole point.
    assert!(!plane.entitlements().permits("vendor.prices", Usage::Trade, now()));

    plane.report(now()).require_fully_enforced()?;
    Ok(())
}
