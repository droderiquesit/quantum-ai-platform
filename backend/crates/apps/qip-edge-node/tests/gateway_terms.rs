//! The quote terms the node's gateways state, through every wrapper the cell
//! is handed.
//!
//! The cell journals a fill's quote unit from `Placer::quote_terms`, and the
//! trait's default is `None`. The pass loop never hands the cell the
//! simulated gateway itself — it hands it a `RequotingPlacer` over it — and
//! `main.rs` holds a `NodeGateway`. A wrapper that forgot to delegate would
//! compile, answer the default, and journal every simulated fill without the
//! currency its listing states, with nothing failing anywhere. This suite is
//! what fails.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::dec;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_edge::cell::{Placer, QuoteTerms};
use qip_edge_node::gateway::{NodeGateway, SimulatedGateway};
use qip_edge_node::reprice::RequotingPlacer;

const VENUE: &str = "XLON";

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME".to_string())
}

#[test]
fn the_requoting_placer_and_the_node_gateway_answer_the_simulated_gateways_quote_terms()
-> Result<()> {
    let venue = VenueId::new(VENUE);
    let mut gateway = SimulatedGateway::new(venue.clone(), 7, at())?;
    // Placing is what lists the instrument at the simulated venue.
    gateway.place(
        "ord-1",
        &object(),
        &venue,
        BookSide::Ask,
        dec!("10"),
        dec!("100"),
        at(),
    )?;
    let listing = gateway
        .listing(&object())
        .ok_or_else(|| Error::not_found("the premise is an instrument the venue has listed"))?;
    let stated = QuoteTerms {
        quote_unit: listing.currency,
    };

    // The simulated gateway's own answer is its listing's currency, and it
    // speaks for nothing it has not listed and no venue it does not reach.
    assert_eq!(
        gateway.quote_terms(&object(), &venue),
        Some(stated),
        "the simulated gateway does not state its listing's quote currency"
    );
    assert_eq!(
        gateway.quote_terms(&ObjectId::from_string("obj-UNLISTED".to_string()), &venue),
        None,
        "the simulated gateway stated terms for an instrument it never listed"
    );
    assert_eq!(
        gateway.quote_terms(&object(), &VenueId::new("XPAR")),
        None,
        "the simulated gateway stated terms for a venue it does not reach"
    );

    // The wrapper the pass loop hands the cell.
    let wrapped = RequotingPlacer::new(&mut gateway, None);
    assert_eq!(
        wrapped.quote_terms(&object(), &venue),
        Some(stated),
        "the requoting placer does not delegate quote terms, so every fill the pass loop \
         confirms is journaled without a quote unit"
    );

    // The seam `main.rs` holds.
    let node = NodeGateway::Simulated(gateway);
    assert_eq!(
        node.quote_terms(&object(), &venue),
        Some(stated),
        "the node gateway does not delegate quote terms"
    );
    Ok(())
}
