//! Event venues feed directly into the cell on the hot path.
//!
//! This suite proves that event venue adapters (prediction markets, commerce
//! venues) can be integrated into the pass loop without going through the
//! Fabric or regional services, and that their order book updates reach the
//! cell as LevelSet messages.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::message::{BookSide, MessageBody};
use qip_core::Timestamp;
use qip_edge_node::event_venues::EventVenue;
use qip_prediction::adapter::{PredictionAdapter, SyntheticPredictionVenue, SyntheticVenueConfig};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

#[test]
fn event_venue_adapter_is_refused_if_not_synthetic() -> Result<(), Box<dyn std::error::Error>> {
    // An event venue that is not synthetic (live) is an architecture decision
    // (ADR 0003), not a configuration value. The node refuses it at start-up
    // rather than letting it slip through to a later layer.
    let live_adapter = Box::new(qip_prediction::adapter::VenueApiAdapter::new(
        qip_prediction::adapter::VenueApiConfig::standard(qip_contracts::venue::VenueId::new(
            "LIVE-PREDICT",
        )),
        false,
        false,
    ));
    let result = EventVenue::new(live_adapter);
    assert!(
        result.is_err(),
        "a live venue adapter should be refused at node start-up"
    );
    let err = result.unwrap_err();
    assert!(
        err.message().contains("only synthetic event venues"),
        "error should name the paper-boundary rule: {err}"
    );
    Ok(())
}

#[test]
fn a_synthetic_prediction_venue_polls_and_publishes_book_updates()
-> Result<(), Box<dyn std::error::Error>> {
    // A synthetic prediction venue holds a deterministic market and publishes
    // its order book on each poll. The node's event venue adapter converts
    // these updates to LevelSet messages the cell can consume.
    let config = SyntheticVenueConfig::demo(42)?;
    let synthetic = SyntheticPredictionVenue::new(config, t(0))?;
    let mut venue = EventVenue::new(Box::new(synthetic))?;

    // First poll: the market is listed and initial depth is published.
    let update1 = venue.poll(t(10))?;
    assert!(
        !update1.messages.is_empty(),
        "first poll should publish market listing and depth"
    );

    // Second poll: at t(10) only the market is listed; no books yet since
    // the synthetic venue's step interval (5 minutes) has not elapsed.
    // Poll at a much later time to trigger book updates.
    let update2 = venue.poll(t(600))?; // 10 minutes out, well past the 5-minute step
    assert!(
        !update2.messages.is_empty(),
        "second poll should publish depth updates after step interval"
    );

    Ok(())
}

#[test]
fn book_updates_are_converted_to_level_set_messages() -> Result<(), Box<dyn std::error::Error>> {
    // Order book updates from a prediction venue are converted to LevelSet
    // messages: one message per price level on each side of the book.
    let config = SyntheticVenueConfig::demo(99)?;
    let synthetic = SyntheticPredictionVenue::new(config, t(0))?;
    let mut venue = EventVenue::new(Box::new(synthetic))?;

    let update = venue.poll(t(100))?;

    // Filter to just the LevelSet messages (the first poll has MarketListed
    // plus depth updates, so we look for the depth part).
    let level_sets: Vec<_> = update
        .messages
        .iter()
        .filter(|(_, msg)| matches!(msg, MessageBody::LevelSet { .. }))
        .collect();

    assert!(
        !level_sets.is_empty(),
        "book updates should include LevelSet messages"
    );

    // Check that we have both bid and ask levels.
    let has_bids = level_sets.iter().any(|(_, msg)| {
        matches!(
            msg,
            MessageBody::LevelSet {
                side: BookSide::Bid,
                ..
            }
        )
    });
    let has_asks = level_sets.iter().any(|(_, msg)| {
        matches!(
            msg,
            MessageBody::LevelSet {
                side: BookSide::Ask,
                ..
            }
        )
    });
    assert!(
        has_bids && has_asks,
        "book should have both bids and asks: bids={}, asks={}",
        has_bids,
        has_asks
    );

    // Check that prices and quantities are populated.
    for (_, msg) in &level_sets {
        if let MessageBody::LevelSet {
            price, quantity, ..
        } = msg
        {
            assert!(
                price.is_positive(),
                "level price should be positive: {price}"
            );
            assert!(
                quantity.is_positive(),
                "level quantity should be positive: {quantity}"
            );
        }
    }

    Ok(())
}

#[test]
fn event_venue_descriptor_is_synthetic() -> Result<(), Box<dyn std::error::Error>> {
    // The descriptor of a synthetic prediction venue reports it as synthetic,
    // so the node knows the feed is paper-trading and can be safely integrated
    // without additional checks.
    let config = SyntheticVenueConfig::demo(7)?;
    let synthetic = SyntheticPredictionVenue::new(config, t(0))?;
    let descriptor = synthetic.descriptor();
    assert!(
        descriptor.is_synthetic,
        "synthetic venue should report is_synthetic=true"
    );
    Ok(())
}
