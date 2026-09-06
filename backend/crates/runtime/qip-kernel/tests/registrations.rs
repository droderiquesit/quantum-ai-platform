//! Venue registrations in the kernel: an operator's approval, the committed
//! configuration, and the log that rebuilds both.
//!
//! The failure each test guards is the one `registration.rs` exists to
//! refuse — a credential the platform reads with nobody's name on it — and
//! its quieter cousin: a name that lives in the registry's memory and
//! nowhere else, so that a restart or an audit cannot say who registered.
//! Every test asserts its premise first: that the source *was* pending, that
//! the log *was* empty of registrations, before asserting what moved.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ObjectId, dec};
use qip_data_finder::registration::{
    NOT_OFFERED, RegistrationRecord, RegistrationRequirement, RegistrationStanding,
};
use qip_events::Topic;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::{Platform, RegistrationEntry, RegistrationSource};
use qip_market_ingestion::connector::manifest::SecretRef;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::OperatorIdentity;
use qip_streaming::StreamEnvelope;

const INSTRUMENT: &str = "obj-AAA";
/// A source the shipped table says needs an account, so it is pending until
/// somebody's name is on it.
const ACCOUNT_SOURCE: &str = "alpaca-daily-bars";
const TERMS: &str = "https://alpaca.markets/terms-and-conditions";
const SLOT: &str = "QIP_ALPACA_API_SECRET_KEY";

/// The producer the kernel writes registration records under. A literal
/// here so the test reads the log the way an auditor would — by what the
/// record says about itself — rather than through the kernel's constant.
const REGISTRATION_PRODUCER: &str = "kernel/registration";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(INSTRUMENT),
            "AAA",
            InstrumentType::CommonStock,
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

fn limits() -> LimitSet {
    LimitSet::new("registrations-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe()?, limits())
}

/// Every registration the event log holds, oldest first, decoded from the
/// frame the way a replay would.
fn registration_entries(platform: &Platform) -> Result<Vec<RegistrationEntry>> {
    let mut entries = Vec::new();
    for record in platform.event_log().records() {
        if record.event.topic != Topic::ComplianceEvaluated
            || record.event.lineage.producer != REGISTRATION_PRODUCER
        {
            continue;
        }
        let envelope = StreamEnvelope::from_frame(&record.event)?;
        entries.push(envelope.decode::<RegistrationEntry>()?.body);
    }
    Ok(entries)
}

/// The pending refusal for a source, or a panic naming the standing it had.
fn pending_reason(platform: &Platform, source_id: &str) -> String {
    match platform.registration_standing(source_id) {
        Err(refused) => refused.message().to_string(),
        Ok(standing) => panic!("{source_id} is not pending: {standing:?}"),
    }
}

#[test]
fn an_operator_approval_moves_a_pending_source_to_registered_and_the_log_replays_it() -> Result<()>
{
    let mut platform = platform(PlatformConfig::default())?;

    // Premise: the source needs an account, nobody has registered, and the
    // log holds no registration — so what follows is the approval's doing.
    assert_eq!(
        platform.registrations().requirement(ACCOUNT_SOURCE),
        Some(RegistrationRequirement::Account)
    );
    let reason = pending_reason(&platform, ACCOUNT_SOURCE);
    assert!(reason.contains(NOT_OFFERED), "{reason}");
    assert!(registration_entries(&platform)?.is_empty());

    let later = start().saturating_add(Duration::from_secs(60));
    let operator = OperatorIdentity::verified("ops-dana", "hardware-token", later);
    let record = platform.approve_registration(
        ACCOUNT_SOURCE,
        &operator,
        TERMS,
        SecretRef::new(SLOT)?,
        later,
    )?;

    // The record carries the operator's own subject — not a name the caller
    // chose — and the instant, the terms and the slot as given.
    assert_eq!(record.operator(), "ops-dana");
    assert_eq!(record.terms_read_at(), later);
    assert_eq!(record.terms(), TERMS);
    assert_eq!(record.secret().variable(), SLOT);

    // The source now stands registered, by the same person.
    match platform.registration_standing(ACCOUNT_SOURCE)? {
        RegistrationStanding::Registered { record: standing } => {
            assert_eq!(standing, record);
        }
        RegistrationStanding::Keyless => panic!("an account source stood as keyless"),
    }

    // One record on the log, from an operator, and the log alone rebuilds
    // the registry the platform is now admitting the source on.
    let entries = registration_entries(&platform)?;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].source, RegistrationSource::Operator);
    assert_eq!(entries[0].record, record);
    let replayed = platform.replay_registrations()?;
    assert_eq!(&replayed, platform.registrations());
    assert_eq!(
        replayed
            .record(ACCOUNT_SOURCE)
            .map(RegistrationRecord::operator),
        Some("ops-dana")
    );
    Ok(())
}

#[test]
fn a_committed_registration_is_journaled_at_assembly_under_the_configuration_source() -> Result<()>
{
    let committed = RegistrationRecord::new(
        ACCOUNT_SOURCE,
        "desk-owner",
        start(),
        TERMS,
        SecretRef::new(SLOT)?,
    )?;
    let config = PlatformConfig::default().with_venue_registrations(vec![committed.clone()]);

    // Premise: the configuration's own registry holds the record, which is
    // what a composition root admits a connector against before the
    // platform exists.
    assert_eq!(
        config.registration_registry()?.record(ACCOUNT_SOURCE),
        Some(&committed)
    );

    let assembled = platform(config)?;
    match assembled.registration_standing(ACCOUNT_SOURCE)? {
        RegistrationStanding::Registered { record } => assert_eq!(record, committed),
        RegistrationStanding::Keyless => panic!("an account source stood as keyless"),
    }
    // And the platform's registry is the configuration's: one function
    // builds both, so the feed and the platform cannot disagree.
    assert_eq!(
        assembled.registrations(),
        &assembled.config().registration_registry()?
    );

    // The log says where the record came from, and rebuilds it.
    let entries = registration_entries(&assembled)?;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].source, RegistrationSource::Configuration);
    assert_eq!(entries[0].record, committed);
    assert_eq!(
        &assembled.replay_registrations()?,
        assembled.registrations()
    );

    // A committed record for a source with no declared requirement stops
    // assembly with the source named, rather than assembling a platform that
    // silently holds a configuration it did not apply.
    let undeclared = RegistrationRecord::new(
        "some-venue-nobody-declared",
        "desk-owner",
        start(),
        TERMS,
        SecretRef::new(SLOT)?,
    )?;
    let refused = platform(PlatformConfig::default().with_venue_registrations(vec![undeclared]))
        .expect_err("a platform assembled on a registration for an undeclared source");
    assert!(
        refused.message().contains("some-venue-nobody-declared")
            && refused
                .message()
                .contains("no registration requirement is declared"),
        "{}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_stale_operator_or_an_undeclared_source_is_refused_and_nothing_reaches_the_log() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let now = start().saturating_add(Duration::from_mins(20));

    // Premise: a fresh operator on the same inputs is accepted, so the two
    // refusals below are about the freshness and the source, nothing else.
    let fresh = OperatorIdentity::verified("ops-dana", "hardware-token", now);
    assert!(
        platform
            .registrations()
            .clone()
            .with_record(RegistrationRecord::new(
                ACCOUNT_SOURCE,
                fresh.subject(),
                now,
                TERMS,
                SecretRef::new(SLOT)?,
            )?)
            .is_ok()
    );

    // Authenticated twenty minutes ago: past the fifteen the kernel holds
    // an eligibility decision and an autonomy change to.
    let stale = OperatorIdentity::verified("ops-dana", "hardware-token", start());
    let refused = platform
        .approve_registration(ACCOUNT_SOURCE, &stale, TERMS, SecretRef::new(SLOT)?, now)
        .expect_err("a stale operator credential approved a registration");
    assert!(
        refused.message().contains("re-authenticate"),
        "{}",
        refused.message()
    );

    // A source nobody declared a requirement for: refused by name, so a
    // later `keyless` declaration cannot erase that somebody had to register.
    let refused = platform
        .approve_registration(
            "some-venue-nobody-declared",
            &fresh,
            TERMS,
            SecretRef::new(SLOT)?,
            now,
        )
        .expect_err("a registration was approved for a source with no declared requirement");
    assert!(
        refused.message().contains("some-venue-nobody-declared"),
        "{}",
        refused.message()
    );

    // Neither refusal reached the log or the registry.
    assert!(registration_entries(&platform)?.is_empty());
    let reason = pending_reason(&platform, ACCOUNT_SOURCE);
    assert!(reason.contains(NOT_OFFERED), "{reason}");
    assert!(platform.registrations().record(ACCOUNT_SOURCE).is_none());
    Ok(())
}
