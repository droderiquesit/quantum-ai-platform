//! A model card records the licensed data it was trained on and where it
//! may run, and a deployment anywhere else is refused (R12-TICK-2091).

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::ModelCard;
use qip_core::error::Result;
use qip_core::{ModelId, Timestamp};
use std::collections::BTreeSet;

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn card() -> ModelCard {
    ModelCard::new(
        ModelId::from_string("fill-1"),
        "tick-fill-probability",
        "1",
        "desk",
        Timestamp::from_secs(1_791_676_800),
    )
}

/// Mutation: make `authorise_location` return `Ok(())` unconditionally —
/// the out-of-licence region is admitted.
#[test]
fn a_card_carries_its_training_entitlements_and_refuses_a_location_they_do_not_permit() -> Result<()>
{
    let card = card()
        .with_data_entitlements(set(&["lse-level2", "xpar-depth"]), set(&["europe-west2"]))?;
    assert_eq!(card.data_entitlements, set(&["lse-level2", "xpar-depth"]));
    card.authorise_location("europe-west2")?;
    let refusal = card
        .authorise_location("us-east4")
        .expect_err("a region the licence does not permit is refused");
    assert_eq!(refusal.code(), "denied");
    assert!(refusal.to_string().contains("us-east4") && refusal.to_string().contains("lse-level2"));
    // The record survives a round trip, so a shipped card still says it.
    let back: ModelCard = serde_json::from_str(&serde_json::to_string(&card)?)?;
    assert_eq!(back.permitted_locations, set(&["europe-west2"]));
    assert!(back.authorise_location("us-east4").is_err());
    Ok(())
}

/// Mutation: drop the empty-locations refusal in `with_data_entitlements` —
/// a card trained on licensed data registers with nowhere to run.
#[test]
fn licensed_training_data_with_no_permitted_location_is_refused_at_the_card() -> Result<()> {
    let refusal = card()
        .with_data_entitlements(set(&["lse-level2"]), BTreeSet::new())
        .expect_err("a licence that permits no location is a misread licence");
    assert_eq!(refusal.code(), "invalid");
    // A card that records no licensed data is not restricted by a licence.
    card().authorise_location("anywhere")?;
    Ok(())
}
