//! The governance plane's own evidence, checked against the crates it governs.
//!
//! Six controls apply to every subsystem rather than to one of them, and
//! `qip_contracts::Control` names them. `qip-compliance` builds one enforcement
//! mechanism per control and produces a [`ComplianceReport`] saying which. That
//! report is the artifact an auditor is handed, and the failure mode of a
//! governance plane is not a broken control — it is a control nobody noticed
//! was never wired in, behind a report that reads as complete.
//!
//! The crate's own tests check that the report is well formed. What is checked
//! here is the part no single crate can check about itself:
//!
//! * that every mechanism the report names is a **real path in the crate**,
//!   read off disk, rather than a plausible-looking string;
//! * that the report keeps its **caveats**, because a report whose honest gaps
//!   were tidied away is a regression dressed as an improvement;
//! * that it **survives being filed**, since evidence that cannot be read back
//!   is evidence of nothing;
//! * and that one control's **claim and behaviour are the same object** —
//!   a research-only entitlement built where datasets are registered
//!   (`qip-mesh`) is refused a trade by the mechanism `qip-compliance` names,
//!   and the refusal then appears in the report as evidence.
//!
//! The last is the proof. A report that agreed with itself and with nothing
//! else would pass every other test in this file.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_compliance::incident::ResponsePolicy;
use qip_compliance::licensing::LicensedData;
use qip_compliance::plane::{CompliancePlane, ComplianceReport};
use qip_compliance::signing::SigningKey;
use qip_contracts::governance::{Control, Entitlement, Usage};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp, dec};
use qip_mesh::catalog::{Catalog, DatasetRegistration};
use qip_mesh::provider::MeshPort;

/// The dataset the cross-check runs on: licensed to be looked at, never to be
/// traded on. The common shape of a real market data contract, and the common
/// breach.
const DATASET: &str = "vendor.sentiment.v3";

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn expiry() -> Timestamp {
    now().saturating_add(Duration::from_days(365))
}

fn plane() -> Result<CompliancePlane> {
    CompliancePlane::new(
        SigningKey::from_secret("acceptance-key-2026-01", &[11u8; 32])?,
        dec!("1000000"),
        ResponsePolicy::standard(),
    )
}

/// Every `crate::module::Item` path a mechanism sentence names.
///
/// Parsed rather than pattern-matched against a hardcoded list, because the
/// point of the test that uses it is to follow whatever the report claims
/// today to whatever is on disk today.
fn mechanism_paths(mechanism: &str) -> Vec<(String, String)> {
    let mut paths = Vec::new();
    for fragment in mechanism.split("crate::").skip(1) {
        let mut segments = fragment.split("::");
        let (Some(module), Some(item)) = (segments.next(), segments.next()) else {
            continue;
        };
        let module: String = module
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let item: String = item
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !module.is_empty() && !item.is_empty() {
            paths.push((module, item));
        }
    }
    paths
}

// --- what the report says ---------------------------------------------------

#[test]
fn the_report_covers_all_six_controls_and_names_a_real_mechanism_for_each() -> Result<()> {
    // Two claims, and the second is the one that is hard to fake. The first is
    // that nothing is missing: the report is built by iterating `Control::all`,
    // so a seventh control added to the contract turns up here unenforced
    // rather than being quietly omitted.
    //
    // The second is that each control names something that exists. A status
    // reading "policy requires that…" would satisfy every structural check and
    // mean nothing, so each mechanism is followed to a module declared in
    // `qip-compliance`'s `lib.rs` and a type defined in that module's file.
    // Prose cannot survive that; a renamed type fails it, which is the point —
    // a report describing a mechanism that has been renamed away is a report
    // describing a control that may no longer exist.
    let plane = plane()?;
    let report = plane.report(now());

    assert_eq!(report.statuses().len(), Control::all().len());
    assert_eq!(report.statuses().len(), 6);
    assert!(report.is_fully_enforced());
    assert!(report.unenforced().is_empty());
    report.require_fully_enforced()?;

    let lib = qip_acceptance::read("backend/crates/libs/qip-compliance/src/lib.rs");
    for control in Control::all() {
        let status = report
            .status(control)
            .ok_or_else(|| Error::not_found(format!("no status for {}", control.as_str())))?;
        assert!(
            status.enforced,
            "{} is not enforced: {status:?}",
            control.as_str()
        );
        assert!(
            !status.evidence.is_empty(),
            "{} claims a mechanism and shows nothing for it",
            control.as_str()
        );

        let paths = mechanism_paths(&status.mechanism);
        assert!(
            !paths.is_empty(),
            "{} describes its mechanism in prose rather than naming it: {}",
            control.as_str(),
            status.mechanism
        );
        for (module, item) in paths {
            assert!(
                lib.contains(&format!("pub mod {module};")),
                "{} names crate::{module}, which qip-compliance does not declare",
                control.as_str()
            );
            let source = qip_acceptance::read(&format!(
                "backend/crates/libs/qip-compliance/src/{module}.rs"
            ));
            assert!(
                source.contains(&format!("pub struct {item}"))
                    || source.contains(&format!("pub enum {item}"))
                    || source.contains(&format!("pub trait {item}")),
                "{} names crate::{module}::{item}, which {module}.rs does not define",
                control.as_str()
            );
        }
    }
    Ok(())
}

#[test]
fn the_report_still_states_its_own_gaps() -> Result<()> {
    // A control described as structural when it is advisory is worse than one
    // labelled advisory, because the label is what a reader calibrates on.
    // Caveats are therefore part of the deliverable, and a change that removes
    // them is a regression however much better the report reads afterwards.
    //
    // Asserted for every control rather than only for the famous one: a status
    // that has found nothing to be honest about is usually a status nobody has
    // looked at recently.
    let plane = plane()?;
    let report = plane.report(now());

    let caveats = report.caveats();
    assert!(!caveats.is_empty(), "the report claims it has no gaps");

    for control in Control::all() {
        let status = report
            .status(control)
            .ok_or_else(|| Error::not_found(format!("no status for {}", control.as_str())))?;
        assert!(
            !status.caveats.is_empty(),
            "{} records no caveat at all",
            control.as_str()
        );
        for caveat in &status.caveats {
            assert!(
                caveat.len() > 40,
                "{} has a caveat too short to act on: {caveat:?}",
                control.as_str()
            );
        }
    }

    // The largest gap by some distance, and the one a deployment has to plan
    // around: possession of a shared secret is not the identity of a signer.
    let signing = report
        .status(Control::SignedArtifactsAndProvenance)
        .ok_or_else(|| Error::not_found("no signing status"))?;
    let stated = signing.caveats.join(" ");
    for term in ["HMAC", "asymmetric", "KMS", "revocation"] {
        assert!(
            stated.contains(term),
            "the signing caveat no longer mentions {term}: {stated}"
        );
    }
    Ok(())
}

#[test]
fn the_report_round_trips_through_json_so_it_can_be_filed_as_evidence() -> Result<()> {
    // The report is what an auditor is handed and what a deployment's start-up
    // check reads back. One that cannot survive being stored is not evidence
    // of anything, and the field most likely to be lost on the way — because
    // it is the one nobody would miss — is the caveats.
    let plane = plane()?;
    let report = plane.report(now());

    let encoded = serde_json::to_string(&report)?;
    let decoded: ComplianceReport = serde_json::from_str(&encoded)?;
    assert_eq!(decoded, report);

    // Encoding the decoded copy again must give the same bytes, so a report
    // filed and re-filed does not drift.
    assert_eq!(serde_json::to_string(&decoded)?, encoded);

    assert_eq!(decoded.generated_at(), now());
    assert_eq!(decoded.statuses().len(), Control::all().len());
    assert!(decoded.is_fully_enforced());
    decoded.require_fully_enforced()?;
    assert_eq!(decoded.caveats().len(), report.caveats().len());
    Ok(())
}

// --- the claim against the mechanism ----------------------------------------

#[test]
fn a_dataset_the_catalogue_licenses_for_research_is_refused_a_trade_by_the_named_mechanism()
-> Result<()> {
    // The end-to-end proof, and the reason this file is at the workspace level
    // rather than inside `qip-compliance`.
    //
    // The entitlement is built where datasets are actually registered — the
    // mesh catalogue — and never restated by hand for the governance plane.
    // Both crates hold `qip_contracts::Entitlement`, and sharing the type is
    // what makes it impossible for the catalogue and the control to disagree
    // about what a licence says. If they held separate vocabularies this test
    // would be the only place the disagreement showed up.
    let mut plane = plane()?;

    let mut catalog = Catalog::new();
    catalog.register(
        DatasetRegistration::new(
            DATASET,
            "research-data-engineering",
            MeshPort::Analytical,
            now(),
        )?
        .licensed(Entitlement::Granted {
            dataset: DATASET.to_string(),
            usage: Usage::Research,
            expires_at: expiry(),
        }),
    )?;
    let registration = catalog.require(DATASET)?;
    assert!(registration.permits(Usage::Research, now()));
    assert!(
        !registration.permits(Usage::Trade, now()),
        "the catalogue thinks a research licence covers trading"
    );

    // Carry the catalogue's entitlements into the plane verbatim.
    for entitlement in &registration.entitlements {
        match entitlement {
            Entitlement::Granted {
                dataset,
                usage,
                expires_at,
            } => plane
                .entitlements_mut()
                .grant(dataset.as_str(), *usage, *expires_at, now())?,
            Entitlement::Denied {
                dataset,
                usage,
                reason,
            } => plane
                .entitlements_mut()
                .deny(dataset.as_str(), *usage, reason.as_str())?,
        }
    }

    // `LicensedData` is the mechanism the report names for this control. The
    // value is private and the only ways to it take a usage and the registry,
    // so reaching the number is the same act as proving the use is licensed.
    let sentiment = LicensedData::from_dataset(DATASET, 0.42_f64);
    assert!(sentiment.is_available_for(plane.entitlements(), Usage::Research, now()));
    assert!(!sentiment.is_available_for(plane.entitlements(), Usage::Trade, now()));
    sentiment.open(plane.entitlements_mut(), Usage::Research, now())?;

    let refusal = sentiment
        .open(plane.entitlements_mut(), Usage::Trade, now())
        .expect_err("a research-only dataset was opened to base an order on");
    assert!(refusal.message().contains(DATASET), "{refusal}");
    assert!(refusal.message().contains("trade"), "{refusal}");
    assert!(
        refusal.message().contains("not as permission"),
        "an unrecorded licence must not read as a granted one: {refusal}"
    );

    // And the report generated afterwards carries what just happened. The
    // status is not a description of the mechanism written alongside it — it
    // is produced from the same registry the refusal was recorded in, which
    // is what makes the claim and the behaviour one object rather than two
    // that have to be kept in agreement.
    let report = plane.report(now());
    let status = report
        .status(Control::LicensingAndEntitlements)
        .ok_or_else(|| Error::not_found("no licensing status"))?;
    assert!(status.enforced);
    assert!(
        status.mechanism.contains("crate::licensing::LicensedData"),
        "the report names a different mechanism than the one just exercised: {}",
        status.mechanism
    );
    assert!(
        status
            .evidence
            .iter()
            .any(|line| line == "1 refusals recorded"),
        "the refusal did not reach the report: {:?}",
        status.evidence
    );
    assert!(
        status
            .evidence
            .iter()
            .any(|line| line == "1 entitlements registered"),
        "the report disagrees with the catalogue about how much is licensed: {:?}",
        status.evidence
    );

    // The refusal is on the record with the dataset and the usage on it,
    // because an audit trail saying "entitlement check failed" tells nobody
    // which contract to go and read.
    let refusals = plane.entitlements().refusals();
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].dataset, DATASET);
    assert_eq!(refusals[0].usage, Usage::Trade);
    assert!(!refusals[0].granted);
    assert!(!refusals[0].refusal.is_empty());

    // The whole plane still reports as compliant. Refusing a use is the
    // control working, not the control failing, and a report that downgraded
    // itself every time it did its job would be switched off within a week.
    report.require_fully_enforced()?;
    Ok(())
}
