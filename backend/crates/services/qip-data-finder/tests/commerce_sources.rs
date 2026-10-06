//! COMMERCE-006: Commerce data source catalogue enumerates seven required kinds,
//! each with evaluated licensing posture.
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
//! Each source is catalogued with an evaluated licensing posture (Declared,
//! not Undetermined), so the platform can decide whether to use each source
//! based on a reviewed legal determination rather than falling back on
//! assembly-time uncertainty.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

mod common;

use common::now;
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

#[test]
fn all_seven_commerce_source_kinds_are_catalogued_with_evaluated_licensing_posture() -> Result<()> {
    // Load the committed catalogue.
    let loaded = load(COMMITTED, now())?;
    assert!(
        !loaded.is_empty(),
        "premise: the committed catalogue parsed to something"
    );

    // Extract source IDs from the loaded catalogue.
    let catalogued_ids: BTreeSet<String> = loaded
        .entries
        .iter()
        .map(|entry| entry.candidate.id().to_string())
        .collect();

    // Check that all seven commerce kinds are present in the catalogue.
    for kind in COMMERCE_SOURCES {
        assert!(
            catalogued_ids.contains(*kind),
            "commerce source {} is not catalogued in source-candidates.json",
            kind
        );
    }

    // Verify each commerce source has evaluated licensing posture (Declared,
    // not Undetermined or Ambiguous). A source with Undetermined licensing
    // cannot be used and should not reach the registry.
    let commerce_entries: Vec<_> = loaded
        .entries
        .iter()
        .filter(|entry| COMMERCE_SOURCES.contains(&entry.candidate.id()))
        .collect();

    assert_eq!(
        commerce_entries.len(),
        COMMERCE_SOURCES.len(),
        "expected all {} commerce sources to load",
        COMMERCE_SOURCES.len()
    );

    for entry in commerce_entries {
        let candidate = &entry.candidate;
        let id = candidate.id();
        let posture = candidate.declared_licensing();

        // The licensing posture must be Declared (evaluated), not Undetermined
        // or Ambiguous. This ensures a human has reviewed the source's legal
        // terms and made a decision, rather than the platform operating under
        // uncertainty.
        match posture {
            LicensingPosture::Declared { license } => {
                // Declared is the evaluated state. The platform knows what
                // this source permits and can enforce those bounds.
                assert!(
                    !license.identifier().is_empty(),
                    "source {} has declared licensing but no identifier",
                    id
                );
            }
            LicensingPosture::Ambiguous { evidence } => {
                panic!(
                    "source {} has ambiguous licensing (requires human review): {}",
                    id, evidence
                );
            }
            LicensingPosture::Undetermined => {
                panic!("source {} has undetermined licensing (not evaluated)", id);
            }
        }
    }

    Ok(())
}
