//! COMMERCE-006: the seven commerce source kinds are catalogued, and every
//! one is refused until a licence record exists (ADR 0157).
//!
//! The seven commerce source kinds are:
//! 1. SKU listings (product reference data)
//! 2. Inventories (warehouse stock levels)
//! 3. Prices (e-commerce price indices)
//! 4. Shipping estimates (logistics rate data)
//! 5. Marketplace fees (operator fee structures)
//! 6. Auction data (transaction prices and volumes)
//! 7. Resale markets (secondary market pricing)
//!
//! They are placeholders: each sits on a reserved `.example.com` host with a
//! fee nobody quoted. Until this file said otherwise each also carried a
//! `Declared` licence nobody had read, which made the catalogue assert a legal
//! determination that was never made. Which commerce source is licensed is the
//! owner's decision and is billed (ADR 0157); until a record quoting the terms
//! it was read from is committed, the posture is `Undetermined` and the answer
//! to every usage is `Unknown`, which this crate treats as not permitted.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

mod common;

use common::now;
use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_data_finder::catalogue::load;
use qip_data_finder::legal::LicensingPosture;
use std::collections::BTreeSet;

/// The committed catalogue as the deployment mounts it.
const COMMITTED: &str = include_str!("../../../../../data/datasets/source-candidates.json");

/// The seven required commerce source kinds.
const COMMERCE_SOURCES: &[&str] = &[
    "sku-listings-catalog",
    "inventory-levels",
    "ecommerce-price-index",
    "shipping-estimates",
    "marketplace-fee-registry",
    "auction-data-feed",
    "resale-marketplace-api",
];

/// Whether `host` is one RFC 2606 reserves for documentation, so no real
/// publisher stands behind it and no licence can have been read from it.
fn is_reserved_documentation_host(host: &str) -> bool {
    host == "example.com" || host.ends_with(".example.com")
}

fn commerce_entries(
    loaded: &qip_data_finder::catalogue::LoadedCandidates,
) -> Vec<&qip_data_finder::catalogue::CandidateEntry> {
    loaded
        .entries
        .iter()
        .filter(|entry| COMMERCE_SOURCES.contains(&entry.candidate.id()))
        .collect()
}

#[test]
fn all_seven_commerce_source_kinds_are_catalogued_and_refused_until_a_licence_record_exists()
-> Result<()> {
    let loaded = load(COMMITTED, now())?;
    assert!(
        !loaded.is_empty(),
        "premise: the committed catalogue parsed to something"
    );
    let catalogued_ids: BTreeSet<String> = loaded
        .entries
        .iter()
        .map(|entry| entry.candidate.id().to_string())
        .collect();
    for kind in COMMERCE_SOURCES {
        assert!(
            catalogued_ids.contains(*kind),
            "commerce source {kind} is not catalogued in source-candidates.json"
        );
    }
    let commerce = commerce_entries(&loaded);
    assert_eq!(
        commerce.len(),
        COMMERCE_SOURCES.len(),
        "expected all {} commerce sources to load",
        COMMERCE_SOURCES.len()
    );

    for entry in commerce {
        let candidate = &entry.candidate;
        let id = candidate.id();
        assert_eq!(
            candidate.declared_licensing(),
            &LicensingPosture::Undetermined,
            "source {id} must stay undetermined until a licence record quoting its terms is committed"
        );
        for usage in [Usage::Derive, Usage::Trade] {
            let verdict = candidate.declared_licensing().legality_for(usage, now());
            assert!(
                !verdict.is_permitted(),
                "source {id} is permitted for {usage:?}: {verdict:?}"
            );
        }
    }
    Ok(())
}

/// The seven placeholders were committed with `Declared` licences and invented
/// fees on hosts nobody owns, so the catalogue vouched for terms no one read.
/// Mutation: restore one placeholder's posture to `declared` in
/// `source-candidates.json` — this test fails naming that entry. The premise
/// asserts the seven placeholder hosts exist, so an emptied or renamed file
/// cannot pass.
#[test]
fn no_candidate_on_a_reserved_documentation_host_carries_a_declared_licence() -> Result<()> {
    let loaded = load(COMMITTED, now())?;
    let placeholders: Vec<_> = loaded
        .entries
        .iter()
        .filter(|entry| is_reserved_documentation_host(entry.candidate.endpoint().host()))
        .collect();
    assert_eq!(
        placeholders.len(),
        COMMERCE_SOURCES.len(),
        "premise: exactly the seven commerce placeholders sit on reserved hosts"
    );
    for entry in placeholders {
        assert!(
            !matches!(
                entry.candidate.declared_licensing(),
                LicensingPosture::Declared { .. }
            ),
            "source {} sits on a reserved documentation host and carries a declared licence",
            entry.candidate.id()
        );
    }
    Ok(())
}
