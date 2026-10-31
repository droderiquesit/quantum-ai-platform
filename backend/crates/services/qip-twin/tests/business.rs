//! Company and macro twins over a supply network (R12-WORLD-9037).

#![allow(clippy::panic_in_result_fn, clippy::float_cmp)]

use qip_core::error::Result;
use qip_twin::business::{BusinessTwin, MacroLink, SupplyLink};

fn supplies(supplier: &str, customer: &str, share: f64) -> SupplyLink {
    SupplyLink {
        supplier: supplier.to_string(),
        customer: customer.to_string(),
        share,
        source: format!("filing:{supplier}->{customer}"),
    }
}

/// A chain: a foundry supplies 40% of a chipmaker's input, the chipmaker
/// supplies half of a device maker's, and an unrelated grocer buys from
/// neither.
fn chain() -> Result<BusinessTwin> {
    BusinessTwin::new(
        vec![
            supplies("FOUNDRY", "CHIPS", 0.4),
            supplies("CHIPS", "DEVICES", 0.5),
            supplies("FARM", "GROCER", 0.7),
        ],
        vec![MacroLink {
            driver: "rates".to_string(),
            entity: "FOUNDRY".to_string(),
            elasticity: -2.0,
            source: "study:foundry-rates".to_string(),
        }],
    )
}

/// R12-WORLD-9037's unit acceptance: a shock to an upstream supplier
/// propagates through the twin to its customers, and theirs, attenuated by
/// each customer's share, and reaches nobody outside the chain. Each effect
/// names the links it was carried through.
///
/// Mutations: carry a customer's change without its share; stop after one
/// hop; leave the link's source out of the effect.
#[test]
fn a_shock_to_an_upstream_supplier_propagates_through_the_twin_to_its_customers() -> Result<()> {
    let twin = chain()?;
    let response = twin.shock_entity("FOUNDRY", -0.10, 4)?;
    let change = |entity: &str| response.effects.get(entity).map(|e| e.change);
    assert_eq!(change("FOUNDRY"), Some(-0.10));
    let chips = change("CHIPS").expect("the direct customer was reached");
    let devices = change("DEVICES").expect("the second-tier customer was reached");
    assert!((chips - -0.04).abs() < 1e-12, "{chips}");
    assert!((devices - -0.02).abs() < 1e-12, "{devices}");
    assert_eq!(response.effects["CHIPS"].hops, 1);
    assert_eq!(response.effects["DEVICES"].hops, 2);
    assert!(change("GROCER").is_none() && change("FARM").is_none());
    assert!(!response.truncated);
    assert_eq!(
        response.effects["DEVICES"].sources,
        ["filing:CHIPS->DEVICES", "filing:FOUNDRY->CHIPS"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert!(twin.manifest().contains("filing:FARM->GROCER"));
    Ok(())
}

/// A macro twin: a rate shock lands on the exposed entity at its elasticity
/// and travels down its supply chain, citing the exposure study it rests
/// on. A driver nothing is exposed to is refused rather than read as one
/// with no effect.
///
/// Mutations: ignore the elasticity; accept an unknown driver.
#[test]
fn a_macro_shock_lands_at_its_elasticity_and_travels_down_the_supply_chain() -> Result<()> {
    let twin = chain()?;
    let response = twin.shock_macro("rates", 0.01, 4)?;
    let foundry = response.effects["FOUNDRY"].change;
    assert!((foundry - -0.02).abs() < 1e-12, "{foundry}");
    let devices = response.effects["DEVICES"].change;
    assert!((devices - -0.004).abs() < 1e-12, "{devices}");
    assert!(
        response.effects["DEVICES"]
            .sources
            .contains("study:foundry-rates")
    );
    assert!(twin.shock_macro("oil", 0.1, 4).is_err());
    Ok(())
}

/// A link the twin cannot carry honestly is refused, never clamped or
/// dropped. A customer's shares summing past all of its input would make
/// every round amplify a shock instead of carrying it, and a cycle in the
/// network stays bounded by the hop cap.
///
/// Mutations: drop the share-sum check; drop the range check's lower bound.
#[test]
fn a_link_the_twin_cannot_carry_honestly_is_refused_and_a_cycle_stays_bounded() -> Result<()> {
    assert!(
        BusinessTwin::new(
            vec![supplies("A", "C", 0.7), supplies("B", "C", 0.6)],
            Vec::new()
        )
        .is_err(),
        "shares summing past one were accepted"
    );
    assert!(BusinessTwin::new(vec![supplies("A", "C", 1.5)], Vec::new()).is_err());
    // A share of nothing, or a negative one, is no supply: refused by the
    // range check, which no sum check would catch.
    assert!(BusinessTwin::new(vec![supplies("A", "C", 0.0)], Vec::new()).is_err());
    assert!(
        BusinessTwin::new(vec![supplies("A", "C", -0.2)], Vec::new()).is_err(),
        "a negative share was accepted"
    );
    assert!(BusinessTwin::new(vec![supplies("A", "A", 0.5)], Vec::new()).is_err());
    assert!(
        BusinessTwin::new(
            vec![supplies("A", "C", 0.5), supplies("A", "C", 0.2)],
            Vec::new()
        )
        .is_err()
    );
    let cycle = BusinessTwin::new(
        vec![supplies("A", "B", 0.5), supplies("B", "A", 0.5)],
        Vec::new(),
    )?;
    let response = cycle.shock_entity("A", -0.1, 8)?;
    // The fixed point: A = -0.1 + 0.5 B and B = 0.5 A, so A = -0.1 / 0.75.
    let a = response.effects["A"].change;
    let b = response.effects["B"].change;
    assert!(
        (a - -0.1 / 0.75).abs() < 1e-3,
        "A did not reach its fixed point: {a}"
    );
    assert!(
        (b - -0.05 / 0.75).abs() < 1e-3,
        "B did not reach its fixed point: {b}"
    );
    assert!(
        cycle.shock_entity("A", -0.1, 9).is_err(),
        "the hop cap was not held"
    );
    assert!(
        cycle.shock_entity("A", -0.1, 0).is_err(),
        "a shock carried nowhere was accepted"
    );
    Ok(())
}

/// An output cannot fall by more than all of itself, and a linear twin that
/// reported -150% would be read as a real loss. A shock below -100%, a
/// macro move whose elasticity lands one there, and a full-share cycle that
/// grows past it are each refused. `truncated` says whether effects were
/// still moving at the cap: a chain exactly as deep as the cap has
/// converged, and one deeper has not.
///
/// Mutations: drop the per-round check on a change past -100% (it also
/// holds a seeded change, since every seeded entity is in each round); read
/// `truncated` as true whenever the cap was reached.
#[test]
fn a_change_past_all_of_an_output_is_refused_and_truncated_says_whether_effects_still_moved()
-> Result<()> {
    let twin = chain()?;
    assert!(
        twin.shock_entity("FOUNDRY", -1.5, 4).is_err(),
        "-150% was accepted"
    );
    assert!(
        twin.shock_macro("rates", 0.6, 4).is_err(),
        "-120% from an elasticity was accepted"
    );
    let full = BusinessTwin::new(
        vec![supplies("A", "B", 1.0), supplies("B", "A", 1.0)],
        Vec::new(),
    )?;
    assert!(
        full.shock_entity("A", -0.5, 8).is_err(),
        "a full-share cycle grew past -100% and was reported"
    );
    // FOUNDRY -> CHIPS -> DEVICES is two hops deep.
    assert!(
        !twin.shock_entity("FOUNDRY", -0.1, 2)?.truncated,
        "a converged chain read truncated"
    );
    assert!(
        twin.shock_entity("FOUNDRY", -0.1, 1)?.truncated,
        "a cut chain read complete"
    );
    Ok(())
}
