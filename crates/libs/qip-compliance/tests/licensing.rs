//! Control 2 — data licensing and entitlements.
//!
//! The case modelled throughout is the real one: a vendor feed licensed for
//! research and derivation but explicitly not for trading. Every test tries to
//! get that data into a trading decision by a different route.

#![allow(clippy::panic_in_result_fn)]

use qip_compliance::licensing::{EntitlementRegistry, LicensedData};
use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// The feed everything here is about: research and derive, never trade.
fn research_only_registry() -> Result<EntitlementRegistry> {
    let mut registry = EntitlementRegistry::new();
    let expiry = now().saturating_add(Duration::from_days(30));
    registry.grant("vendor.sentiment", Usage::Research, expiry, now())?;
    registry.grant("vendor.sentiment", Usage::Derive, expiry, now())?;
    registry.deny(
        "vendor.sentiment",
        Usage::Trade,
        "the master agreement covers internal research only",
    )?;
    Ok(registry)
}

#[test]
fn data_licensed_only_for_research_cannot_reach_a_trading_decision() -> Result<()> {
    let mut registry = research_only_registry()?;
    let signal = LicensedData::from_dataset("vendor.sentiment", 42_i64);

    // The permitted uses work.
    assert_eq!(*signal.open(&mut registry, Usage::Research, now())?, 42);

    // The forbidden one does not, and the refusal names both the dataset and
    // the usage so somebody can go and read the right contract.
    let error = signal
        .open(&mut registry, Usage::Trade, now())
        .expect_err("research-only data must not reach a trading decision");
    assert!(error.message().contains("vendor.sentiment"));
    assert!(error.message().contains("trade"));
    assert!(error.message().contains("internal research only"));
    Ok(())
}

#[test]
fn there_is_no_way_to_read_the_value_without_stating_a_usage() -> Result<()> {
    // `LicensedData` exposes exactly three ways out — open, into_inner and
    // derive — and all three take a usage and the registry. The dataset name
    // is the only thing readable for free, which is what a refusal must quote.
    let mut registry = research_only_registry()?;
    let signal = LicensedData::from_dataset("vendor.sentiment", 7_i64);

    assert_eq!(signal.dataset(), "vendor.sentiment");
    assert!(signal.into_inner(&mut registry, Usage::Trade, now()).is_err());
    Ok(())
}

#[test]
fn a_derived_value_inherits_the_licence_of_what_it_was_derived_from() -> Result<()> {
    // Without this, laundering a licence takes one `map`: derive a feature from
    // a research-only feed, and the feature looks unencumbered forever after.
    let mut registry = research_only_registry()?;
    let raw = LicensedData::from_dataset("vendor.sentiment", 10_i64);

    let feature = raw.derive(&mut registry, now(), |v| v * 2)?;
    assert_eq!(feature.dataset(), "vendor.sentiment");
    assert_eq!(*feature.open(&mut registry, Usage::Research, now())?, 20);

    let error = feature
        .open(&mut registry, Usage::Trade, now())
        .expect_err("a feature derived from research-only data is research-only");
    assert!(error.message().contains("vendor.sentiment"));
    Ok(())
}

#[test]
fn deriving_from_data_not_licensed_for_derivation_is_refused() -> Result<()> {
    let mut registry = EntitlementRegistry::new();
    let expiry = now().saturating_add(Duration::from_days(30));
    registry.grant("vendor.prices", Usage::Research, expiry, now())?;

    let raw = LicensedData::from_dataset("vendor.prices", 1_i64);
    let error = raw
        .derive(&mut registry, now(), |v| v + 1)
        .expect_err("look-but-do-not-model data cannot be modelled");
    assert!(error.message().contains("vendor.prices"));
    assert!(error.message().contains("derive"));
    Ok(())
}

#[test]
fn an_expired_licence_is_refused_and_says_when_it_expired() -> Result<()> {
    let mut registry = EntitlementRegistry::new();
    let expiry = now().saturating_add(Duration::from_days(1));
    registry.grant("vendor.sentiment", Usage::Trade, expiry, now())?;

    let signal = LicensedData::from_dataset("vendor.sentiment", 1_i64);
    // Inside the term.
    assert!(signal.open(&mut registry, Usage::Trade, now()).is_ok());

    // A day later the contract has lapsed. Nothing changed in the code path;
    // the licence simply ran out, which is how licences actually fail.
    let later = now().saturating_add(Duration::from_days(2));
    let error = signal
        .open(&mut registry, Usage::Trade, later)
        .expect_err("an expired licence must be refused");
    assert!(error.message().contains("expired"));
    assert!(error.message().contains("vendor.sentiment"));
    Ok(())
}

#[test]
fn registering_an_already_expired_licence_is_refused_at_registration() {
    // A licence that reads as configured and behaves as absent is the worst of
    // both, so it is rejected where somebody is still looking at it.
    let mut registry = EntitlementRegistry::new();
    let error = registry
        .grant("vendor.sentiment", Usage::Trade, now(), now())
        .expect_err("a licence expiring at the moment it is granted covers nothing");
    assert!(error.message().contains("vendor.sentiment"));
}

#[test]
fn a_dataset_nobody_recorded_a_licence_for_is_treated_as_unlicensed() -> Result<()> {
    // The permissive reading of a missing entry is how licences get exceeded:
    // "we never wrote it down" must not mean "we may do anything".
    let mut registry = research_only_registry()?;
    let mystery = LicensedData::from_dataset("scraped.forum_posts", 1_i64);

    for usage in [Usage::Research, Usage::Derive, Usage::Trade, Usage::Redistribute] {
        let error = mystery
            .open(&mut registry, usage, now())
            .expect_err("an unrecorded dataset permits nothing");
        assert!(error.message().contains("scraped.forum_posts"));
        assert!(error.message().contains("no recorded entitlement"));
    }
    Ok(())
}

#[test]
fn every_attempt_is_recorded_including_the_refused_ones() -> Result<()> {
    // Code repeatedly asking whether it may trade on a research-only feed is a
    // finding in its own right, and it is only visible if refusals are kept.
    let mut registry = research_only_registry()?;
    let signal = LicensedData::from_dataset("vendor.sentiment", 1_i64);

    let _ = signal.open(&mut registry, Usage::Research, now());
    let _ = signal.open(&mut registry, Usage::Trade, now());
    let _ = signal.open(&mut registry, Usage::Trade, now());

    assert_eq!(registry.checks().len(), 3);
    assert_eq!(registry.refusals().len(), 2);
    assert!(registry.refusals().iter().all(|c| c.usage == Usage::Trade));
    assert!(registry.refusals().iter().all(|c| !c.refusal.is_empty()));
    Ok(())
}

#[test]
fn asking_whether_a_use_is_permitted_does_not_itself_grant_it() -> Result<()> {
    // The adapt-rather-than-fail path must not be a way around the control.
    let registry = research_only_registry()?;
    let signal = LicensedData::from_dataset("vendor.sentiment", 1_i64);

    assert!(signal.is_available_for(&registry, Usage::Research, now()));
    assert!(!signal.is_available_for(&registry, Usage::Trade, now()));
    assert_eq!(
        registry.permitted_usages("vendor.sentiment", now()),
        vec![Usage::Research, Usage::Derive]
    );
    Ok(())
}
