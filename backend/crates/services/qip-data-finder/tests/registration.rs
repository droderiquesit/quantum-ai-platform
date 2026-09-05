//! Who registered with a venue: the finder's answer to a request for a
//! scraper that signs up on its own.
//!
//! Each test names the failure it prevents. The one this file exists for is
//! a credentialed source polled under an account nobody owns, with terms
//! nobody read — which is what "self-registers, anonymously" means once the
//! words are unpacked.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, coverage, licensed_for, now, permissive_robots, probe_for};
use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::{Currency, Timestamp};
use qip_data_finder::coverage::{SourceRegion, UpdateFrequency};
use qip_data_finder::decision::{DecisionOutcome, LifecycleStage, RegistrationDecision};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::legal::LicensingPosture;
use qip_data_finder::quality::SourceCost;
use qip_data_finder::registration::{
    NOT_OFFERED, RegistrationRecord, RegistrationRegistry, RegistrationRequirement,
};
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_data_finder::tier::CredentialReference;
use qip_events::Topic;
use qip_market_ingestion::connector::manifest::SecretRef;

fn candidate_with(
    id: &str,
    url: &str,
    mechanism: AccessMechanism,
    licensing: LicensingPosture,
) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Venue Inc")?,
        SourceEndpoint::parse(url, mechanism)?,
        coverage(&["EU0001"], UpdateFrequency::Minutely)?,
        licensing,
        SourceCost::free(Currency::EUR),
        SourceRegion::Europe,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        now(),
    )
}

fn open_rest() -> AccessMechanism {
    AccessMechanism::Rest {
        auth: AuthRequirement::None,
        incremental_parameter: Some("since".to_string()),
        page_size: 500,
    }
}

fn keyed_rest() -> AccessMechanism {
    AccessMechanism::Rest {
        auth: AuthRequirement::ApiKey {
            header: "X-Api-Key".to_string(),
        },
        incremental_parameter: Some("since".to_string()),
        page_size: 500,
    }
}

fn config() -> Result<FinderConfig> {
    FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)
}

fn first(decisions: &[RegistrationDecision]) -> Result<&RegistrationDecision> {
    decisions
        .first()
        .ok_or_else(|| qip_core::error::Error::invalid("no decision was produced"))
}

fn rejection_reason(decision: &RegistrationDecision) -> Result<String> {
    match decision.outcome() {
        DecisionOutcome::Rejected { reason } => Ok(reason.clone()),
        other => Err(qip_core::error::Error::invalid(format!(
            "expected a rejection, got {}: {}",
            other.as_str(),
            decision.reasoning().describe()
        ))),
    }
}

fn owner_record(source_id: &str) -> Result<RegistrationRecord> {
    RegistrationRecord::new(
        source_id,
        "desk-owner",
        now(),
        "https://venue.example/api-terms",
        SecretRef::new("QIP_EXAMPLE_VENUE_KEY")?,
    )
}

#[test]
fn a_source_needing_an_account_is_refused_by_the_finder_without_a_record_naming_the_requirement()
-> Result<()> {
    let url = "https://venue.example/api/quotes";
    let candidate = candidate_with(
        "venue-quotes",
        url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
    )?;
    let mut probe = probe_for(url, "venue.example", permissive_robots());

    // Premise: with a credential reference *and* a record, the same source
    // registers — so the refusal below is the missing record and nothing
    // upstream of it.
    let mut complete = DataFinder::new(
        config()?
            .with_credential_reference("venue.example", CredentialReference::new("venue-key")?)
            .with_registration_requirement("venue-quotes", RegistrationRequirement::Account)
            .with_registration(owner_record("venue-quotes")?)?,
    );
    let decisions = complete.assess(vec![candidate.clone()], &mut probe, now())?;
    let decision = first(&decisions)?;
    assert!(
        decision.is_registered(),
        "{}",
        decision.reasoning().describe()
    );
    assert!(
        decision
            .reasoning()
            .at(LifecycleStage::Register)
            .iter()
            .any(|finding| finding.contains("registered by desk-owner")),
        "the decision record does not name who registered: {}",
        decision.reasoning().describe()
    );

    // The same source with the credential named but nobody recorded as
    // having registered for it: refused, by requirement, naming who must.
    let mut anonymous = DataFinder::new(
        config()?
            .with_credential_reference("venue.example", CredentialReference::new("venue-key")?)
            .with_registration_requirement("venue-quotes", RegistrationRequirement::Account),
    );
    let decisions = anonymous.assess(vec![candidate], &mut probe, now())?;
    let reason = rejection_reason(first(&decisions)?)?;
    assert!(
        reason.contains("`venue-quotes` requires an account with the venue")
            && reason.contains("requirement `account`"),
        "the refusal does not name the source and its requirement: {reason}"
    );
    assert!(
        reason.contains("owner must register with the venue under their own identity"),
        "the refusal does not say who must register: {reason}"
    );
    assert!(
        anonymous.registry().is_empty(),
        "a refused source reached the finder's registry"
    );
    Ok(())
}

#[test]
fn the_refusal_says_anonymous_or_automated_registration_is_not_offered() -> Result<()> {
    // The sentence is a constant so that this test, the runbook and the
    // refusal an operator reads all carry the same words; a paraphrase in
    // one of them would let the test pass on text nobody sees.
    assert_eq!(
        NOT_OFFERED,
        "anonymous or automated registration is not a path this platform offers"
    );

    let url = "https://venue.example/api/quotes";
    let candidate = candidate_with(
        "venue-quotes",
        url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
    )?;
    let mut probe = probe_for(url, "venue.example", permissive_robots());
    let mut finder = DataFinder::new(
        config()?
            .with_credential_reference("venue.example", CredentialReference::new("venue-key")?)
            .with_registration_requirement(
                "venue-quotes",
                RegistrationRequirement::AccountWithIdentityVerification,
            ),
    );
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let reason = rejection_reason(first(&decisions)?)?;
    assert!(
        reason.contains(NOT_OFFERED),
        "the refusal does not say that anonymous registration is not offered: {reason}"
    );
    assert!(
        reason.contains("identity verification"),
        "the refusal does not name what the venue requires: {reason}"
    );
    Ok(())
}

#[test]
fn a_credentialed_source_with_no_declared_requirement_is_refused_rather_than_read_as_keyless()
-> Result<()> {
    // The failure: a keyed source registered because nobody wrote down what
    // kind of registration issues its key — unknown read as "none needed".
    let url = "https://venue.example/api/quotes";
    let candidate = candidate_with(
        "venue-quotes",
        url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
    )?;
    let mut probe = probe_for(url, "venue.example", permissive_robots());
    let mut finder = DataFinder::new(
        config()?
            .with_credential_reference("venue.example", CredentialReference::new("venue-key")?),
    );
    // Premise: nothing is declared for the source.
    assert!(
        finder
            .config()
            .registrations()
            .requirement("venue-quotes")
            .is_none()
    );
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let reason = rejection_reason(first(&decisions)?)?;
    assert!(
        reason.contains("no registration requirement is declared"),
        "the refusal is not about the undeclared requirement: {reason}"
    );
    assert!(reason.contains(NOT_OFFERED), "{reason}");
    Ok(())
}

#[test]
fn a_keyless_source_is_admitted_as_before_and_recorded_as_keyless() -> Result<()> {
    // The gate must not turn every public endpoint into a registration
    // chore: an open endpoint with nothing declared is keyless by its own
    // description, and one declared keyless is too.
    let url = "https://open.example/api/quotes";
    let candidate = candidate_with(
        "open-quotes",
        url,
        open_rest(),
        licensed_for(&[Usage::Derive])?,
    )?;
    let mut probe = probe_for(url, "open.example", permissive_robots());

    for config in [
        config()?,
        config()?.with_registration_requirement("open-quotes", RegistrationRequirement::Keyless),
    ] {
        let mut finder = DataFinder::new(config);
        let decisions = finder.assess(vec![candidate.clone()], &mut probe, now())?;
        let decision = first(&decisions)?;
        assert!(
            decision.is_registered(),
            "{}",
            decision.reasoning().describe()
        );
        assert!(
            decision
                .reasoning()
                .at(LifecycleStage::Register)
                .iter()
                .any(|finding| finding.contains("keyless; no registration needed")),
            "the decision record does not say the source is keyless: {}",
            decision.reasoning().describe()
        );
    }
    Ok(())
}

#[test]
fn a_record_without_an_operator_cannot_be_built_and_the_config_refuses_one_with_no_requirement()
-> Result<()> {
    // Premise: the same inputs with an operator build.
    let secret = SecretRef::new("QIP_EXAMPLE_VENUE_KEY")?;
    RegistrationRecord::new(
        "venue-quotes",
        "desk-owner",
        Timestamp::from_secs(1_760_000_000),
        "https://venue.example/api-terms",
        secret.clone(),
    )?;

    let refused = RegistrationRecord::new(
        "venue-quotes",
        "   ",
        Timestamp::from_secs(1_760_000_000),
        "https://venue.example/api-terms",
        secret,
    )
    .expect_err("a record with no operator was built");
    assert!(
        refused.message().contains(NOT_OFFERED),
        "{}",
        refused.message()
    );

    // A record cannot be attached to a config that never declared what it
    // satisfies: the requirement comes first, or a later `Keyless`
    // declaration would erase the fact that anyone had to register.
    let refused = config()?
        .with_registration(owner_record("venue-quotes")?)
        .expect_err("a record was accepted for a source with no declared requirement");
    assert!(
        refused
            .message()
            .contains("no registration requirement is declared"),
        "{}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_record_carrying_a_key_shaped_secret_reference_is_refused() -> Result<()> {
    // The shape screen is the ingestion manifest's own, so a key that could
    // not be written into a manifest cannot be written into a record either.
    // Three shapes a pasted credential takes; the refusal must not echo any.
    for pasted in [
        "sk-live-9f2a7c1e4b8d",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "QIP_EXAMPLE_VENUE_KEY=abc123",
    ] {
        let refused = SecretRef::new(pasted)
            .expect_err("a key-shaped value was accepted as a secret reference");
        assert!(
            !refused.message().contains(pasted),
            "the refusal echoed the value: {}",
            refused.message()
        );
    }
    // And the registry as a whole survives a serde round trip with the
    // record's constructor on the load path, so a config file is held to
    // the same rule as a call.
    let registry = RegistrationRegistry::empty()
        .with_requirement("venue-quotes", RegistrationRequirement::Account)
        .with_record(owner_record("venue-quotes")?)?;
    let text = serde_json::to_string(&registry)?;
    let back: RegistrationRegistry = serde_json::from_str(&text)?;
    assert_eq!(back, registry);
    let smuggled = text.replace("QIP_EXAMPLE_VENUE_KEY", "sk-live-9f2a7c1e4b8d");
    assert_ne!(
        smuggled, text,
        "the fixture did not contain the reference to replace"
    );
    let refused = serde_json::from_str::<RegistrationRegistry>(&smuggled)
        .expect_err("a key-shaped reference was loaded from a config file");
    assert!(
        refused
            .to_string()
            .contains("not a deployment variable name"),
        "{refused}"
    );
    Ok(())
}
