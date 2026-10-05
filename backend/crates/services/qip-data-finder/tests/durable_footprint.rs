//! DATA-012 the durable knowledge a fetch leaves is smaller than the source
//! content it came from.
//!
//! The fixture campaign is fixed on purpose: a threshold measured against a
//! different campaign each run would be a number nobody could catch being
//! wrong. Every body is a distinct few kilobytes, and what is measured is
//! what the reference ledger serialises — the only durable thing this crate
//! keeps about a fetch.

// A test returning `Result` so it can use `?` still has to assert; the abort
// is its reporting mechanism, not a defect.
#![allow(clippy::panic_in_result_fn)]

use qip_core::Duration;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::schema::{FieldType, SourceSchema};
use qip_data_finder::{DataPeriod, DataReference, ReferenceLedger};
use qip_events::Topic;
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::adapter::SourceDescriptor;

/// Durable bytes must stay under this fraction of the bytes fetched. A
/// reference is a hash, a length, a locator and two small sets, so a fetch of
/// a few kilobytes costs a few hundred; one quarter leaves room for a longer
/// locator and fails the moment a body is copied anywhere the ledger keeps.
const MAX_DURABLE_FRACTION: f64 = 0.25;

fn descriptor() -> SourceDescriptor {
    SourceDescriptor {
        name: "fixture-wire".to_string(),
        provider: "this process".to_string(),
        licensing: LicensingClass::Synthetic,
        topics: vec![Topic::MarketBar],
        expected_latency: Duration::ZERO,
        production_requirement: None,
    }
}

/// A body of `len` bytes that differs for every `index`, so no fetch hashes
/// like another and no compression-shaped shortcut flatters the ratio.
fn body(index: i64, len: usize) -> Vec<u8> {
    (0..len)
        .map(|offset| b'a' + ((index as usize * 31 + offset * 7) % 26) as u8)
        .collect()
}

/// Run the fixed campaign: forty fetches of `len` bytes. Returns the bytes
/// fetched and the bytes the ledger would persist.
fn campaign(len: usize) -> Result<(usize, usize)> {
    let start = Timestamp::from_secs(1_760_000_000);
    let mut ledger = ReferenceLedger::bounded();
    let mut fetched = 0;
    let mut durable = 0;
    for index in 0..40i64 {
        let at = start.saturating_add(Duration::from_secs(index));
        let bytes = body(index, len);
        fetched += bytes.len();
        let reference = DataReference::of_generated(
            &descriptor(),
            format!("bars://AAA/{index}"),
            ["AAA".to_string()],
            DataPeriod::instant(at),
            SourceSchema::from_fields([("close".to_string(), FieldType::Number)]),
            &bytes,
            at,
        )?;
        // The ledger's own key is a struct and cannot be a JSON object key, so
        // the references it holds are measured one by one: the same bytes a
        // log record of each would carry.
        durable += serde_json::to_vec(&reference)?.len();
        ledger.record(reference, at);
    }
    assert_eq!(ledger.len(), 40, "premise: every extent was recorded");
    Ok((fetched, durable))
}

#[test]
fn the_bytes_a_campaign_keeps_are_a_small_fraction_of_the_bytes_it_fetched() -> Result<()> {
    let (fetched, durable) = campaign(4_096)?;
    assert!(
        fetched > 100_000,
        "premise: the campaign fetched real volume"
    );
    assert!(durable > 0, "premise: something was recorded");
    let fraction = durable as f64 / fetched as f64;
    assert!(
        fraction < MAX_DURABLE_FRACTION,
        "durable {durable} bytes against {fetched} fetched is {fraction:.3}; the bound is \
         {MAX_DURABLE_FRACTION}"
    );
    Ok(())
}

/// A ratio could pass by luck on one body size. What proves no body is held
/// is that the durable size does not move when the bodies grow sixteenfold.
#[test]
fn the_durable_footprint_does_not_grow_with_the_size_of_what_was_fetched() -> Result<()> {
    let (small_fetched, small_durable) = campaign(1_024)?;
    let (large_fetched, large_durable) = campaign(16_384)?;
    assert_eq!(
        large_fetched,
        small_fetched * 16,
        "premise: bodies grew 16x"
    );
    let growth = large_durable.abs_diff(small_durable);
    assert!(
        growth <= small_durable / 50,
        "durable bytes went {small_durable} -> {large_durable} when the fetched bytes grew \
         sixteenfold; a record that tracks the body's size holds the body"
    );
    Ok(())
}
