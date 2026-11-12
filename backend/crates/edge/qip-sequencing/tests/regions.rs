//! Cross-region alignment (R12-TICK-2048, ADR 0438): ticks two cells
//! captured are ordered only beyond both ticks' uncertainty and both cells'
//! stated clock bound, never on a sort of two clocks' timestamp columns.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::sync::{AssetKey, ClockSource, CrossPartition, SyncMetadata, SyncOrder};
use qip_core::{Duration, Timestamp};
use qip_sequencing::{CrossRegionAligner, MAX_REGION_PARTITIONS};

const US: i64 = 1_000;

fn sync(region: &str, cell: &str, venue: &str, instrument: &str) -> SyncMetadata {
    SyncMetadata::new(
        region,
        cell,
        ClockSource::OffsetEstimate,
        AssetKey {
            venue: venue.to_string(),
            feed: "depth".to_string(),
            partition: 0,
            instrument: instrument.to_string(),
        },
        CrossPartition::Aligned(Vec::new()),
    )
    .unwrap()
}

fn at(nanos: i64) -> Timestamp {
    Timestamp::from_nanos(1_800_000_000_000_000_000 + nanos)
}

/// Ticks 30 µs apart on clocks known to 5 µs each, with each cell's clock
/// stated to 10 µs of the reference: the pair's bound is 5+10+5+10 = 30 µs,
/// so 30 µs touches it and is not an order; 31 µs is. Without the cell
/// bound the same 30 µs would read as 10 µs of doubt and be ordered.
#[test]
fn two_regions_ticks_are_ordered_only_beyond_both_clocks_and_both_cell_bounds() {
    let mut aligner = CrossRegionAligner::new(Duration::from_micros(10)).unwrap();
    let doubt = Duration::from_micros(5);
    let london = sync("europe-west2", "edge-lon", "XLON", "ACME");
    let virginia = sync("us-east4", "edge-nyc", "XNYS", "ACME-US");

    let first = aligner.relate(&london, 1, at(0), doubt).unwrap();
    assert!(first.is_empty(), "nothing yet from another cell: {first:?}");

    let touching = aligner.relate(&virginia, 1, at(30 * US), doubt).unwrap();
    assert_eq!(touching.len(), 1, "premise: related to London's latest");
    let pair = &touching[0];
    assert!(pair.crosses_regions());
    assert_eq!(pair.other_region, "europe-west2");
    assert_eq!(pair.other_instrument, "ACME");
    assert_eq!(pair.instrument, "ACME-US");
    assert_eq!(pair.lag_ns, 30 * US);
    assert_eq!(
        pair.lag_uncertainty_ns,
        30 * US,
        "both doubts and both bounds"
    );
    assert_eq!(pair.order_of_other, SyncOrder::Unordered);

    let beyond = aligner.relate(&virginia, 2, at(31 * US), doubt).unwrap();
    assert_eq!(beyond.len(), 1);
    assert_eq!(beyond[0].order_of_other, SyncOrder::Before);
    assert_eq!(beyond[0].other_sequence, 1);
}

/// One cell's own partitions are aligned on its lake line, on one clock; the
/// cross-cell aligner relates a tick only to *other* cells' ticks.
#[test]
fn a_cells_own_partitions_are_not_related_across_cells() {
    let mut aligner = CrossRegionAligner::new(Duration::ZERO).unwrap();
    let doubt = Duration::from_micros(1);
    aligner
        .relate(&sync("r1", "edge-a", "XLON", "ACME"), 1, at(0), doubt)
        .unwrap();
    let same_cell = aligner
        .relate(&sync("r1", "edge-a", "XPAR", "ACME"), 1, at(US), doubt)
        .unwrap();
    assert!(same_cell.is_empty(), "same cell related: {same_cell:?}");
    let other_cell = aligner
        .relate(&sync("r1", "edge-b", "XPAR", "ACME"), 1, at(2 * US), doubt)
        .unwrap();
    assert_eq!(other_cell.len(), 2, "both of edge-a's partitions");
    assert!(other_cell.iter().all(|pair| !pair.crosses_regions()));
}

/// No bound is read as zero, and an unplaced tick has no instant to relate.
#[test]
fn a_negative_bound_and_an_unplaced_tick_are_refused() {
    assert!(CrossRegionAligner::new(Duration::from_nanos(-1)).is_err());
    let mut aligner = CrossRegionAligner::new(Duration::ZERO).unwrap();
    let unplaced = SyncMetadata::new(
        "r1",
        "edge-a",
        ClockSource::Unplaced,
        AssetKey {
            venue: "XLON".to_string(),
            feed: "depth".to_string(),
            partition: 0,
            instrument: "ACME".to_string(),
        },
        CrossPartition::Refused {
            refused: "no estimate".to_string(),
        },
    )
    .unwrap();
    assert!(aligner.relate(&unplaced, 1, at(0), Duration::ZERO).is_err());
    assert_eq!(aligner.held(), 0, "a refused tick became a latest");
}

/// The working set is bounded however many partitions a session names.
#[test]
fn the_aligner_holds_at_most_its_bound_of_partitions() {
    let mut aligner = CrossRegionAligner::new(Duration::ZERO).unwrap();
    for index in 0..(MAX_REGION_PARTITIONS + 10) {
        let tick = sync("r1", "edge-a", "XLON", &format!("I{index:05}"));
        aligner
            .relate(&tick, 1, at(index as i64 * US), Duration::ZERO)
            .unwrap();
    }
    assert_eq!(aligner.held(), MAX_REGION_PARTITIONS);
}
