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

#[test]
fn the_two_credential_windows_that_claim_to_be_the_same_window_agree_on_the_same_credential() {
    // Found by tracing the halt flow end to end. Two crates each police "how
    // stale a credential may be when a human authorises something", each uses
    // fifteen minutes, and each says in its own doc comment that it matches the
    // other. `qip_compliance::approval` opens "The same window
    // `qip_risk_engine::autonomy` uses".
    //
    // **Nothing holds them together.** `qip-compliance` does not depend on
    // `qip-risk-engine` — its manifest lists six `qip-*` crates and that is not
    // one of them — so the crate documenting the agreement cannot see the thing
    // it claims to agree with. Two independent claims about one fact will
    // eventually disagree, and the failure here is silent and asymmetric: widen
    // one and the control it guards weakens while the other still reads as
    // fifteen minutes and both doc comments still say they match.
    //
    // Asserted behaviourally rather than by comparing constants, because the
    // risk engine's window is a private field with no accessor. Driving
    // `request_change` exercises the value actually in force.
    //
    // This is the first place in the workspace that can see both, which is why
    // the assertion lives here rather than in either crate's own tests.
    use qip_compliance::approval::{MAXIMUM_CREDENTIAL_AGE, OperatorCredential};
    use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel, OperatorIdentity};

    let authenticated_at = now();
    let inside = authenticated_at.saturating_add(Duration::from_mins(14));
    let outside = authenticated_at.saturating_add(Duration::from_mins(16));

    for (label, at, expected_fresh) in [("inside", inside, true), ("outside", outside, false)] {
        let compliance = OperatorCredential::verified("op", "hardware-key", authenticated_at)
            .expect("a well-formed credential");
        let compliance_fresh = compliance.is_fresh(at, MAXIMUM_CREDENTIAL_AGE);

        let mut controller = AutonomyController::new();
        let identity = OperatorIdentity::verified("op", "hardware-key", authenticated_at);
        let risk_accepted = controller
            .request_change(
                AutonomyLevel::Advisory,
                &identity,
                "exercising the credential window",
                at,
            )
            .is_ok();

        assert_eq!(
            compliance_fresh, expected_fresh,
            "{label}: the compliance window no longer treats a credential this \
             old as {expected_fresh}"
        );
        assert_eq!(
            risk_accepted,
            compliance_fresh,
            "{label}: at {at} the risk engine {} the credential while compliance \
             called it {}. Both crates document this as the same fifteen-minute \
             window; whichever is now wider is weakening a control while reading \
             as unchanged",
            if risk_accepted { "accepted" } else { "refused" },
            if compliance_fresh { "fresh" } else { "stale" }
        );
    }
}

// --- the withdrawal that is absent rather than false --------------------------
//
// ADR 0021 refuses the path by which capital leaves the platform, and the two
// types below are how that refusal is held: an eligibility record with no
// withdrawal field at all, and an entitlement whose withdrawal arm has one
// variant. Both are *absences*, which is why they are asserted here and why
// each assertion below is paired with a premise proving it is looking at
// something.
//
// The trap this file has to step around is the one the testing rules name.
// `can_withdraw` appears in both places a naive scan would read:
//
//   * `qip-capital`'s `eligibility.rs` has a whole doc-comment section headed
//     "There is no `can_withdraw` here", explaining why the field is absent;
//   * `qip-api`'s `ledger_views.rs` declares `can_withdraw` on
//     `EntitlementView`, where it is the always-refused capability being
//     rendered.
//
// So `!source.contains("can_withdraw")` is false today, for two reasons that
// are both correct. A test written that way would have been deleted as broken
// rather than kept as a guard. The wire form is therefore asserted on the
// serialised object's own keys, and the source scans are scoped — to the code
// lines of one file, and to the body of one struct.

/// The keys of a serialised value, as a sorted list.
///
/// Key equality rather than a substring search over the JSON text: `withdraw`
/// is a substring of `can_withdraw` and `can_invest` is a substring of
/// `can_investigate`, and the whole point of this pair of tests is a field
/// that is either there or not.
fn object_keys(value: &impl serde::Serialize) -> Vec<String> {
    let encoded = serde_json::to_value(value).expect("the value serialises");
    let object = encoded
        .as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {encoded}"));
    object.keys().cloned().collect()
}

/// The lines of `source` that are not a comment.
///
/// Crude on purpose — a leading `//` and nothing else — because that is
/// exactly the distinction being drawn: prose explaining why a field is
/// absent must not be mistaken for the field.
fn code_lines(source: &str) -> Vec<(usize, &str)> {
    source
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line))
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .collect()
}

/// Whether `line` names `token` as a whole word rather than inside a longer
/// one.
fn names_token(line: &str, token: &str) -> bool {
    line.match_indices(token).any(|(index, _)| {
        let before = line[..index].chars().next_back();
        let after = line[index + token.len()..].chars().next();
        let boundary = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        boundary(before) && boundary(after)
    })
}

/// The body of a `pub enum <name> {` declaration.
fn enum_body<'a>(source: &'a str, name: &str) -> &'a str {
    let header = format!("pub enum {name} {{");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("no `{header}` in the source"));
    let open = start + header.len() - 1;
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..index];
                }
            }
            _ => {}
        }
        index += 1;
    }
    panic!("the body of `pub enum {name}` does not close");
}

/// The variant names of an enum body: the identifier at the head of each
/// top-level entry.
fn enum_variants(body: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let mut depth = 0usize;
    for line in body.lines() {
        let trimmed = line.trim();
        if depth == 0 && !trimmed.starts_with("//") && !trimmed.starts_with('#') {
            let name: String = trimmed
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && name.starts_with(char::is_uppercase) {
                variants.push(name);
            }
        }
        depth += line.matches(['{', '(']).count();
        depth = depth.saturating_sub(line.matches(['}', ')']).count());
    }
    variants
}

#[test]
fn the_eligibility_a_user_is_admitted_on_carries_no_withdrawal_key_on_the_wire() {
    use qip_capital::ledger::{Eligibility, EligibilityTerms, Jurisdiction};

    // The wire form itself, not a description of it. `Eligibility` serialises
    // through `EligibilityTerms`, so what an operator's decision looks like
    // when it is journalled, replayed or read back is exactly these keys.
    let terms = EligibilityTerms {
        verified_at: now(),
        can_invest: true,
        jurisdiction: Jurisdiction::new("GB").expect("a two-letter code"),
        expires_at: expiry(),
    };
    let eligibility = Eligibility::new(terms).expect("an expiry after the verification");
    let keys = object_keys(&eligibility);

    // Premise: the record does carry the investment flag under the name this
    // test would find a withdrawal one by. Without it the absence below could
    // be an empty object, a renamed convention, or a type that does not
    // serialise as an object at all.
    assert!(
        keys.iter().any(|key| key == "can_invest"),
        "the eligibility no longer carries can_invest; the naming this test reads has \
         changed and the withdrawal assertion below means nothing: {keys:?}"
    );
    assert!(
        !keys.iter().any(|key| key == "can_withdraw"),
        "an eligibility now carries a withdrawal flag. ADR 0021 refuses the path by which \
         capital leaves the platform, and a field — even one always false — is a value a \
         transfer path can one day read: {keys:?}"
    );
    assert_eq!(
        keys,
        vec![
            "can_invest".to_string(),
            "expires_at".to_string(),
            "jurisdiction".to_string(),
            "verified_at".to_string(),
        ],
        "the eligibility's wire form has changed shape"
    );

    // The same absence in the API's view of it, because the two are separate
    // structs and only one of them was ever going to be checked by hand.
    //
    // The literal is exhaustive, which is load-bearing rather than incidental:
    // a field added to `EligibilityView` cannot compile until somebody comes
    // here and writes it down, so the review of a withdrawal flag on the
    // browser's surface happens in this file whether or not the assertions
    // below are read.
    let view = qip_api::ledger_views::EligibilityView {
        eligible: true,
        verified_at: Some(now().to_rfc3339()),
        can_invest: Some(true),
        jurisdiction: Some("GB".to_string()),
        expires_at: Some(expiry().to_rfc3339()),
        refused: None,
        reason: None,
    };
    let rendered = object_keys(&view);
    assert!(
        rendered.iter().any(|key| key == "can_invest"),
        "the API's eligibility view no longer carries can_invest: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|key| key == "can_withdraw"),
        "the API's eligibility view now offers a withdrawal flag to a browser, which is \
         where a withdraw control gets built next: {rendered:?}"
    );

    // And the source, scoped. The file's own prose says the field is absent
    // and why; the assertion is that no *code* line reintroduces it.
    let ledger =
        qip_acceptance::read("backend/crates/services/qip-capital/src/ledger/eligibility.rs");
    // Premise: the token really is in the file, in the comment that explains
    // its absence. This is the whole reason the scan is line-scoped — a
    // whole-file `contains` would be red today for the best possible reason.
    assert!(
        ledger.contains("There is no `can_withdraw` here"),
        "eligibility.rs no longer explains why the field is absent; the explanation is \
         what stops the next author adding it back as an oversight"
    );
    let offenders: Vec<usize> = code_lines(&ledger)
        .into_iter()
        .filter(|(_, line)| names_token(line, "can_withdraw"))
        .map(|(number, _)| number)
        .collect();
    assert!(
        offenders.is_empty(),
        "eligibility.rs has code naming can_withdraw at line(s) {offenders:?}; the field \
         is absent by decision, not merely undocumented"
    );
    // Premise on the scan: `can_invest` is on a code line of the same file, so
    // the walk is reading code and the matcher does fire.
    assert!(
        code_lines(&ledger)
            .iter()
            .any(|(_, line)| names_token(line, "can_invest")),
        "the code-line walk found no can_invest either; it is not reading the code"
    );

    // The API view's struct body, likewise scoped: `can_withdraw` is declared
    // in the same file on `EntitlementView`, where it is the always-refused
    // capability being rendered and belongs.
    let views = qip_acceptance::read("backend/crates/apps/qip-api/src/ledger_views.rs");
    assert!(
        names_token(&views, "can_withdraw"),
        "ledger_views.rs no longer names can_withdraw anywhere; EntitlementView renders \
         the refused capability and this test's scoping assumes it does"
    );
    let header = "pub struct EligibilityView {";
    let start = views
        .find(header)
        .expect("EligibilityView is declared in ledger_views.rs");
    let end = views[start..]
        .find("\n}")
        .expect("EligibilityView's declaration closes")
        + start;
    let declaration = &views[start..end];
    assert!(
        names_token(declaration, "can_invest"),
        "EligibilityView no longer declares can_invest; the slice being scanned is not \
         the struct: {declaration}"
    );
    assert!(
        !names_token(declaration, "can_withdraw"),
        "EligibilityView now declares a withdrawal field: {declaration}"
    );
}

#[test]
fn the_withdrawal_entitlement_still_has_exactly_one_arm_and_every_evaluation_refuses() {
    use qip_capital::ledger::{
        Capability, Entitlement, Jurisdiction, Mandate, MandateTerms, PermittedFamilies,
        ProductEligibility, Role, UserId,
    };
    use qip_core::money::Currency;

    let source =
        qip_acceptance::read("backend/crates/services/qip-capital/src/ledger/entitlement.rs");

    // Premise on the counter, before it is trusted about the type that
    // matters: `Capability` is the two-armed neighbour declared in the same
    // file. A parser that always answered "one" would pass the assertion
    // below and prove nothing, and that is not a hypothetical — it is the
    // shape of every vacuous structural test.
    let neighbour = enum_variants(enum_body(&source, "Capability"));
    assert_eq!(
        neighbour,
        vec!["Granted".to_string(), "Refused".to_string()],
        "the variant counter no longer reads Capability's two arms, so its answer about \
         WithdrawalEntitlement cannot be believed"
    );

    let arms = enum_variants(enum_body(&source, "WithdrawalEntitlement"));
    assert_eq!(
        arms,
        vec!["Refused".to_string()],
        "WithdrawalEntitlement has arms other than Refused. ADR 0021 refuses the path by \
         which capital leaves the platform and ADR 0023 keeps that in force; a second arm \
         here is that decision reversed in a derive rather than in an ADR"
    );

    // It serialises so an evaluation can be journalled and does not
    // deserialise, so a stored record cannot decide anything. A `Deserialize`
    // added here would let a granted withdrawal arrive from a file.
    let declaration = source
        .find("pub enum WithdrawalEntitlement")
        .expect("WithdrawalEntitlement is declared");
    let derive_line = source[..declaration]
        .lines()
        .next_back()
        .expect("a line precedes the declaration");
    assert!(
        derive_line.contains("Serialize"),
        "WithdrawalEntitlement no longer serialises, so an evaluation cannot be \
         journalled: {derive_line}"
    );
    assert!(
        !derive_line.contains("Deserialize"),
        "WithdrawalEntitlement now deserialises; a record is evidence of what was \
         decided, never an input that decides: {derive_line}"
    );

    // And the behaviour, so this is not only a claim about text. The
    // evaluation is a real one: an investor whose family is eligible in their
    // own jurisdiction, with capital to invest — the case that grants
    // everything it is able to grant.
    let user = UserId::new("user-compliance-proof").expect("a well-formed id");
    let jurisdiction = Jurisdiction::new("GB").expect("a two-letter code");
    let mandate = Mandate::new(MandateTerms {
        capital: dec!("250000"),
        currency: Currency::USD,
        risk_tolerance: dec!("0.2"),
        permitted_families: PermittedFamilies::Only(
            ["mean-reversion".to_string()].into_iter().collect(),
        ),
        liquidity_floor: dec!("50000"),
        exploration_share: dec!("0.05"),
        jurisdiction,
    })
    .expect("a well-formed mandate");
    let mut product = ProductEligibility::new("mean-reversion");
    product.eligible_in.insert(jurisdiction);

    let entitlement = Entitlement::evaluate(&user, &mandate, Role::Investor, &product, now());

    // Premise: this evaluation grants what it can. An entitlement that
    // refused everything would make the withdrawal refusal below indis-
    // tinguishable from a user who simply has no rights.
    assert!(
        matches!(entitlement.can_view(), Capability::Granted { .. }),
        "the evaluation refused viewing, so it is not the permissive case this test needs: \
         {:?}",
        entitlement.can_view()
    );
    assert!(
        entitlement.can_invest().is_granted(),
        "the evaluation refused investing, so the withdrawal refusal below proves nothing \
         about withdrawal specifically: {:?}",
        entitlement.can_invest()
    );

    // The withdrawal arm, on the wire, from the most permissive evaluation
    // this crate can produce.
    let withdrawal = serde_json::to_value(entitlement.can_withdraw())
        .expect("the withdrawal capability serialises");
    let object = withdrawal
        .as_object()
        .unwrap_or_else(|| panic!("expected an externally tagged object, got {withdrawal}"));
    assert_eq!(
        object.keys().cloned().collect::<Vec<_>>(),
        vec!["Refused".to_string()],
        "the withdrawal capability serialised as something other than a refusal: \
         {withdrawal}"
    );
    assert!(
        entitlement.can_withdraw().reason().contains("ADR 0021"),
        "the refusal no longer cites the decision it enforces, so a reader of the record \
         cannot find out why: {}",
        entitlement.can_withdraw().reason()
    );
}
