//! Cross-venue alignment of recorded ticks (R12-TICK-1124): an order between
//! two venues' ticks is stated only beyond both recorded uncertainties.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_sequencing::{CrossVenueAligner, Order, VenueTick};

fn tick(venue: &str, instrument: &str, sequence: u64, nanos: i64, uncertainty: i64) -> VenueTick {
    VenueTick {
        venue: venue.to_string(),
        instrument: instrument.to_string(),
        sequence,
        normalized_time: Timestamp::from_nanos(1_000_000_000 + nanos),
        uncertainty: Duration::from_nanos(uncertainty),
        arrival: Timestamp::from_nanos(1_000_000_000 + nanos + 500),
    }
}

#[test]
fn a_tick_is_ordered_against_another_venue_only_beyond_both_uncertainties() -> Result<()> {
    let mut aligner = CrossVenueAligner::new();
    assert!(aligner.relate(tick("XNAS", "AAPL", 1, 0, 100))?.is_empty());

    // 150 ns apart on clocks known to 100 each: within 200, so unordered.
    let near = aligner.relate(tick("XBATS", "AAPL", 7, 150, 100))?;
    assert_eq!(near.len(), 1, "premise: one other venue to relate to");
    assert_eq!(near[0].order_of_other, Order::Unordered);
    assert_eq!(near[0].lag_ns, 150);
    assert_eq!(near[0].lag_uncertainty_ns, 200);
    assert_eq!(near[0].other_sequence, 1);

    // 5 000 ns apart: far beyond 200, so the order is stated.
    let far = aligner.relate(tick("XBATS", "AAPL", 8, 5_000, 100))?;
    assert_eq!(far.len(), 1);
    assert_eq!(far[0].order_of_other, Order::Before);
    assert_eq!(far[0].lag_ns, 5_000);
    Ok(())
}

#[test]
fn a_tick_is_not_related_to_its_own_venue_or_to_another_instrument() -> Result<()> {
    let mut aligner = CrossVenueAligner::new();
    aligner.relate(tick("XNAS", "AAPL", 1, 0, 10))?;
    aligner.relate(tick("XBATS", "MSFT", 2, 10, 10))?;
    assert!(
        aligner.relate(tick("XNAS", "AAPL", 3, 20, 10))?.is_empty(),
        "the same venue's earlier tick, and another instrument's, are not cross-venue"
    );
    assert_eq!(aligner.held(), 2);
    Ok(())
}

#[test]
fn a_negative_uncertainty_or_a_blank_name_is_refused_rather_than_read_as_exact() {
    let mut aligner = CrossVenueAligner::new();
    assert!(aligner.relate(tick("XNAS", "AAPL", 1, 0, -1)).is_err());
    assert!(aligner.relate(tick(" ", "AAPL", 1, 0, 1)).is_err());
    assert!(aligner.relate(tick("XNAS", "", 1, 0, 1)).is_err());
    assert_eq!(aligner.held(), 0, "a refused tick is not held");
}
